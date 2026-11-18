//! GPU metrics without NVML: the WDDM kernel-mode thunks in `gdi32.dll` (`D3DKMT*`).
//!
//! `nvml.dll` cannot be unloaded and costs about 20 MB of private memory, so the tray (no window)
//! reads the GPU through the graphics kernel statistics instead. They need no admin rights and no
//! extra DLL (`gdi32` is already loaded by every GUI process):
//!
//! * temperature, memory clock: `D3DKMTQueryStatistics(PHYSICAL_ADAPTER)` -> `AdapterPerfData`
//!   (WDDM 2.4+; temperature in 0.1 degree Celsius);
//! * utilization: `D3DKMTQueryStatistics(NODE)` -> `RunningTime` (100 ns units) of every engine,
//!   differenced between two samples (the busiest engine is the GPU load, as Task Manager's
//!   per-engine graphs show it);
//! * VRAM: `D3DKMTQueryStatistics(SEGMENT)` of the local (non-aperture) segments;
//! * name: `D3DKMTQueryAdapterInfo(ADAPTERREGISTRYINFO)`.
//!
//! Anything the driver does not report is `None`. Power in watts is not available this way
//! (`AdapterPerfData.Power` is a percentage of the board limit), so `power_watts` stays `None`.

use crate::metrics::GpuMetrics;
use std::time::Instant;
use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo,
    D3DKMTQueryStatistics, D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_CLOSEADAPTER,
    D3DKMT_ENUMADAPTERS2, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO,
    D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE,
    D3DKMT_QUERYSTATISTICS_PHYSICAL_ADAPTER, D3DKMT_QUERYSTATISTICS_SEGMENT,
    KMTQAITYPE_ADAPTERREGISTRYINFO,
};
use windows::Win32::Foundation::{LUID, NTSTATUS};

/// A GPU read through `D3DKMTQueryStatistics`. Holds no OS handle (queries go by adapter LUID).
#[derive(Debug)]
pub struct D3dkmtGpu {
    luid: LUID,
    name: Option<String>,
    nodes: u32,
    segments: u32,
    /// Running time per node (100 ns units) at the previous sample.
    previous: Option<(Instant, Vec<i64>)>,
}

fn succeeded(status: NTSTATUS) -> bool {
    status.0 >= 0
}

/// One `D3DKMTQueryStatistics` call; the result lives in the returned structure.
fn query(
    luid: LUID,
    kind: windows::Wdk::Graphics::Direct3D::D3DKMT_QUERYSTATISTICS_TYPE,
    fill: impl FnOnce(&mut D3DKMT_QUERYSTATISTICS),
) -> Option<Box<D3DKMT_QUERYSTATISTICS>> {
    let mut statistics: Box<D3DKMT_QUERYSTATISTICS> = Box::default();
    statistics.Type = kind;
    statistics.AdapterLuid = luid;
    fill(&mut statistics);
    let pointer: *mut D3DKMT_QUERYSTATISTICS = &mut *statistics;
    // SAFETY: `pointer` is a valid, writable structure for the whole call; the kernel fills the
    // `QueryResult` member of it.
    let status = unsafe { D3DKMTQueryStatistics(pointer) };
    succeeded(status).then_some(statistics)
}

/// All WDDM adapters (LUIDs). Their open handles are closed again at once.
fn adapters() -> Vec<LUID> {
    let mut request = D3DKMT_ENUMADAPTERS2::default();
    // SAFETY: first call with a null buffer only reports the count.
    let _ = unsafe { D3DKMTEnumAdapters2(&mut request) };
    let count = request.NumAdapters as usize;
    if count == 0 {
        return Vec::new();
    }
    let mut list = vec![D3DKMT_ADAPTERINFO::default(); count];
    request.NumAdapters = count as u32;
    request.pAdapters = list.as_mut_ptr();
    // SAFETY: `list` holds `NumAdapters` writable entries for the whole call.
    let status = unsafe { D3DKMTEnumAdapters2(&mut request) };
    if !succeeded(status) {
        return Vec::new();
    }
    list.truncate(request.NumAdapters as usize);
    list.iter()
        .map(|adapter| {
            // SAFETY: the handle was just returned by the enumeration and is closed exactly once.
            let _ = unsafe {
                D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
                    hAdapter: adapter.hAdapter,
                })
            };
            adapter.AdapterLuid
        })
        .collect()
}

/// `AdapterString` from the adapter's registry info, for example `NVIDIA GeForce GTX 1660`.
fn adapter_name(luid: LUID) -> Option<String> {
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: luid,
        hAdapter: 0,
    };
    // SAFETY: plain in/out structure.
    if !succeeded(unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }) {
        return None;
    }
    let mut info = Box::<D3DKMT_ADAPTERREGISTRYINFO>::default();
    let mut request = D3DKMT_QUERYADAPTERINFO {
        hAdapter: open.hAdapter,
        Type: KMTQAITYPE_ADAPTERREGISTRYINFO,
        pPrivateDriverData: (&mut *info as *mut D3DKMT_ADAPTERREGISTRYINFO).cast(),
        PrivateDriverDataSize: std::mem::size_of::<D3DKMT_ADAPTERREGISTRYINFO>() as u32,
    };
    // SAFETY: the buffer is exactly the structure the information type documents.
    let status = unsafe { D3DKMTQueryAdapterInfo(&mut request) };
    // SAFETY: the handle was opened above and is closed once.
    let _ = unsafe {
        D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
            hAdapter: open.hAdapter,
        })
    };
    if !succeeded(status) {
        return None;
    }
    let end = info
        .AdapterString
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(info.AdapterString.len());
    let name = String::from_utf16_lossy(&info.AdapterString[..end]);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// `(local VRAM limit, local VRAM committed)` in bytes over the non-aperture segments.
fn vram(luid: LUID, segments: u32) -> Option<(u64, u64)> {
    let mut total = 0u64;
    let mut used = 0u64;
    for segment in 0..segments {
        let Some(result) = query(luid, D3DKMT_QUERYSTATISTICS_SEGMENT, |q| {
            q.Anonymous.QuerySegment.SegmentId = segment;
        }) else {
            continue;
        };
        // SAFETY: a successful SEGMENT query filled `SegmentInformation`.
        let info = unsafe { result.QueryResult.SegmentInformation };
        // Aperture segments are system memory shared with the GPU, not VRAM.
        if info.Aperture == 0 {
            total = total.saturating_add(info.CommitLimit);
            used = used.saturating_add(info.BytesCommitted);
        }
    }
    (total > 0).then_some((total, used))
}

impl D3dkmtGpu {
    /// The first hardware adapter that reports engines; the one with the most VRAM when several do.
    /// `None` when the machine has no WDDM adapter or the driver does not support the queries.
    pub fn open() -> Option<Self> {
        let mut best: Option<(u64, Self)> = None;
        for luid in adapters() {
            let Some(info) = query(luid, D3DKMT_QUERYSTATISTICS_ADAPTER, |_| {}) else {
                continue;
            };
            // SAFETY: a successful ADAPTER query filled `AdapterInformation`.
            let adapter = unsafe { info.QueryResult.AdapterInformation };
            if adapter.NodeCount == 0 {
                continue;
            }
            let memory = vram(luid, adapter.NbSegments).map_or(0, |(total, _)| total);
            let gpu = Self {
                luid,
                name: adapter_name(luid),
                nodes: adapter.NodeCount,
                segments: adapter.NbSegments,
                previous: None,
            };
            if best
                .as_ref()
                .is_none_or(|(best_memory, _)| memory > *best_memory)
            {
                best = Some((memory, gpu));
            }
        }
        best.map(|(_, gpu)| gpu)
    }

    /// `AdapterPerfData`: `(temperature in 0.1 C, memory frequency in Hz)`.
    fn perf(&self) -> (Option<u32>, Option<u64>) {
        let Some(result) = query(self.luid, D3DKMT_QUERYSTATISTICS_PHYSICAL_ADAPTER, |q| {
            q.Anonymous.QueryPhysAdapter.PhysicalAdapterIndex = 0;
        }) else {
            return (None, None);
        };
        // SAFETY: a successful PHYSICAL_ADAPTER query filled `PhysAdapterInformation`.
        let data = unsafe { result.QueryResult.PhysAdapterInformation.AdapterPerfData };
        (
            (data.Temperature > 0).then_some(data.Temperature),
            (data.MemoryFrequency > 0).then_some(data.MemoryFrequency),
        )
    }

    /// Running time of every node and the frequency of node 0.
    fn nodes(&self) -> (Vec<i64>, Option<u64>) {
        let mut running = Vec::with_capacity(self.nodes as usize);
        let mut frequency = None;
        for node in 0..self.nodes {
            let Some(result) = query(self.luid, D3DKMT_QUERYSTATISTICS_NODE, |q| {
                q.Anonymous.QueryNode.NodeId = node;
            }) else {
                running.push(0);
                continue;
            };
            // SAFETY: a successful NODE query filled `NodeInformation`.
            let info = unsafe { result.QueryResult.NodeInformation };
            running.push(info.GlobalInformation.RunningTime);
            if node == 0 && info.NodePerfData.Frequency > 0 {
                frequency = Some(info.NodePerfData.Frequency);
            }
        }
        (running, frequency)
    }

    /// One reading. The utilization needs two samples, so the first call leaves it `None`.
    pub fn sample(&mut self) -> Option<GpuMetrics> {
        let now = Instant::now();
        let (running, frequency) = self.nodes();
        let utilization = self.previous.as_ref().and_then(|(at, before)| {
            let elapsed = now.duration_since(*at).as_nanos() / 100;
            if elapsed == 0 || before.len() != running.len() {
                return None;
            }
            let busiest = running
                .iter()
                .zip(before)
                .map(|(after, before)| (after - before).max(0))
                .max()?;
            Some(
                ((busiest as f64 / elapsed as f64) * 100.0)
                    .clamp(0.0, 100.0)
                    .round() as u32,
            )
        });
        self.previous = Some((now, running));
        let (temperature, memory_frequency) = self.perf();
        let memory = vram(self.luid, self.segments);
        Some(GpuMetrics {
            name: self.name.clone(),
            utilization_percent: utilization,
            temperature_c: temperature.map(|deci| (deci + 5) / 10),
            vram_used_bytes: memory.map(|(_, used)| used),
            vram_total_bytes: memory.map(|(total, _)| total),
            graphics_clock_mhz: frequency.map(|hz| (hz / 1_000_000) as u32),
            memory_clock_mhz: memory_frequency.map(|hz| (hz / 1_000_000) as u32),
            power_watts: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_adapter_gives_nothing() {
        // A LUID that no adapter has: every query fails and the reading is simply absent.
        let bogus = LUID {
            LowPart: 0xdead_beef,
            HighPart: 0x7fff,
        };
        assert!(query(bogus, D3DKMT_QUERYSTATISTICS_ADAPTER, |_| {}).is_none());
        assert!(vram(bogus, 4).is_none());
        let mut gpu = D3dkmtGpu {
            luid: bogus,
            name: None,
            nodes: 2,
            segments: 2,
            previous: None,
        };
        let reading = gpu
            .sample()
            .expect("a reading structure is always returned");
        assert_eq!(reading.temperature_c, None);
        assert_eq!(reading.utilization_percent, None);
        assert_eq!(reading.vram_total_bytes, None);
        assert_eq!(adapter_name(bogus), None);
    }

    /// Real hardware: needs a WDDM GPU (every Windows 11 machine has at least the basic adapter).
    #[test]
    fn real_machine_reports_a_named_adapter_with_memory_and_load() {
        let Some(mut gpu) = D3dkmtGpu::open() else {
            eprintln!("no WDDM adapter with engines: nothing to read here");
            return;
        };
        let first = gpu.sample().expect("first sample");
        assert_eq!(first.utilization_percent, None, "needs two samples");
        std::thread::sleep(std::time::Duration::from_millis(600));
        let second = gpu.sample().expect("second sample");
        eprintln!("D3DKMT: {second:?}");
        assert!(second.name.as_deref().is_some_and(|name| !name.is_empty()));
        assert!(second.utilization_percent.is_some_and(|load| load <= 100));
        if let Some(total) = second.vram_total_bytes {
            assert!(total >= second.vram_used_bytes.unwrap_or(0) / 2);
        }
        if let Some(temperature) = second.temperature_c {
            assert!((10..=130).contains(&temperature), "{temperature}");
        }
    }

    /// With an NVIDIA GPU the kernel statistics agree with NVML (same hardware, two sources).
    #[test]
    fn real_machine_agrees_with_nvml_when_present() {
        let mut nvml = crate::gpu::Gpu::new();
        let Some(reference) = nvml.sample() else {
            eprintln!("no NVML here: nothing to compare with");
            return;
        };
        let mut gpu = D3dkmtGpu::open().expect("an adapter next to NVML");
        gpu.sample();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let ours = gpu.sample().expect("sample");
        let reference_again = nvml.sample().expect("nvml");
        eprintln!(
            "D3DKMT {ours:?}
NVML   {reference:?} / {reference_again:?}"
        );
        let (a, b) = (
            ours.temperature_c.expect("temperature") as i64,
            reference_again.temperature_c.expect("nvml temperature") as i64,
        );
        assert!((a - b).abs() <= 5, "temperature {a} vs {b}");
        let (a, b) = (
            ours.vram_total_bytes.expect("vram") as f64,
            reference.vram_total_bytes.expect("nvml vram") as f64,
        );
        assert!((a - b).abs() / b < 0.1, "vram total {a} vs {b}");
        assert_eq!(ours.name, reference.name);
    }
}
