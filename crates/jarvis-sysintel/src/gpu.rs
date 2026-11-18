//! GPU metrics from NVML (`nvml.dll` ships with the NVIDIA driver). The library is loaded lazily on
//! the first sample; any failure (no NVIDIA GPU, no driver) yields `None`, never made-up numbers.

use crate::metrics::GpuMetrics;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::Nvml;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// After a failed load, wait this long before trying again (a driver may be installed later).
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(300);

type Loader = Box<dyn Fn() -> Option<Nvml> + Send>;

pub struct Gpu {
    loader: Loader,
    nvml: Option<Nvml>,
    failed_at: Option<Instant>,
}

impl Gpu {
    /// Uses the real `nvml.dll`.
    pub fn new() -> Self {
        Self::with_loader(Box::new(|| Nvml::init().ok()))
    }

    /// Custom loader, for tests that simulate a machine without NVML.
    pub fn with_loader(loader: Loader) -> Self {
        Self {
            loader,
            nvml: None,
            failed_at: None,
        }
    }

    /// Reads the first GPU. `None` when NVML is unavailable.
    pub fn sample(&mut self) -> Option<GpuMetrics> {
        if self.nvml.is_none() {
            if self
                .failed_at
                .is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE)
            {
                return None;
            }
            match (self.loader)() {
                Some(nvml) => {
                    self.nvml = Some(nvml);
                    self.failed_at = None;
                }
                None => {
                    self.failed_at = Some(Instant::now());
                    return None;
                }
            }
        }
        let nvml = self.nvml.as_ref()?;
        let device = nvml.device_by_index(0).ok()?;
        let memory = device.memory_info().ok();
        Some(GpuMetrics {
            name: device.name().ok(),
            utilization_percent: device.utilization_rates().ok().map(|u| u.gpu),
            temperature_c: device.temperature(TemperatureSensor::Gpu).ok(),
            vram_used_bytes: memory.as_ref().map(|m| m.used),
            vram_total_bytes: memory.as_ref().map(|m| m.total),
            graphics_clock_mhz: device.clock_info(Clock::Graphics).ok(),
            memory_clock_mhz: device.clock_info(Clock::Memory).ok(),
            power_watts: device.power_usage().ok().map(|mw| mw as f32 / 1000.0),
        })
    }
}

impl Default for Gpu {
    fn default() -> Self {
        Self::new()
    }
}

/// Where GPU readings come from.
///
/// `nvml.dll` cannot be unloaded once initialised and holds about 20 MB of private memory, so a
/// resident background process should not carry it: it reads NVML through a small worker process
/// (the same executable started with a worker flag, see [`run_worker`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuSource {
    /// Load NVML into this process (simple; costs the memory above).
    InProcess,
    /// Ask a helper process that owns NVML. It exits when this process goes away.
    Worker { exe: PathBuf, args: Vec<String> },
    /// Read the kernel graphics statistics (`gdi32`, no NVML, no helper): what the tray uses.
    D3dkmt,
    /// Tray (no window): `D3dkmt`. Window open: the NVML helper, started lazily and ended again
    /// when the window closes, so the resident process never carries NVML.
    Adaptive { exe: PathBuf, args: Vec<String> },
    /// Do not read the GPU.
    Off,
}

impl GpuSource {
    /// [`GpuSource::Adaptive`] with this executable re-run with `flag` as the helper.
    pub fn adaptive_from_current_exe(flag: &str) -> std::io::Result<Self> {
        Ok(Self::Adaptive {
            exe: std::env::current_exe()?,
            args: vec![flag.to_string()],
        })
    }

    /// This executable re-run with `flag` as its only argument.
    pub fn worker_from_current_exe(flag: &str) -> std::io::Result<Self> {
        Ok(Self::Worker {
            exe: std::env::current_exe()?,
            args: vec![flag.to_string()],
        })
    }
}

/// Body of the worker process: answers every line on stdin with one JSON line holding the GPU
/// reading (`null` when unavailable) until stdin closes. Returns the exit code.
pub fn run_worker() -> i32 {
    use std::io::{BufRead, Write};
    let mut gpu = Gpu::new();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        if line.is_err() {
            break;
        }
        let reply = serde_json::to_string(&gpu.sample()).unwrap_or_else(|_| "null".into());
        if writeln!(stdout, "{reply}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break;
        }
    }
    0
}

const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_RESPAWN_BACKOFF: Duration = Duration::from_secs(60);
/// `CREATE_NO_WINDOW`
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct WorkerConn {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Talks to the GPU worker; starts it on first use and again after it died (with a back-off).
pub struct WorkerClient {
    exe: PathBuf,
    args: Vec<String>,
    conn: Option<WorkerConn>,
    failed_at: Option<Instant>,
}

impl WorkerClient {
    pub fn new(exe: PathBuf, args: Vec<String>) -> Self {
        Self {
            exe,
            args,
            conn: None,
            failed_at: None,
        }
    }

    fn spawn(&self) -> std::io::Result<WorkerConn> {
        let mut command = Command::new(&self.exe);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        let mut child = command.spawn()?;
        let missing = || std::io::Error::other("worker pipe missing");
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        Ok(WorkerConn {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// True while a helper process is running.
    pub fn is_running(&self) -> bool {
        self.conn.is_some()
    }

    /// Process id of the running helper.
    pub fn pid(&self) -> Option<u32> {
        self.conn.as_ref().and_then(|conn| conn.child.id())
    }

    /// Ends the helper process now and waits until it is gone. A later `sample` starts a new one.
    pub async fn shutdown(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            // Closing stdin lets the helper leave by itself; kill covers a stuck one.
            drop(conn.stdin);
            let _ = conn.child.kill().await;
        }
        self.failed_at = None;
    }

    /// One reading, `None` when the GPU (or the worker) is unavailable.
    pub async fn sample(&mut self) -> Option<GpuMetrics> {
        if self.conn.is_none() {
            if self
                .failed_at
                .is_some_and(|at| at.elapsed() < WORKER_RESPAWN_BACKOFF)
            {
                return None;
            }
            match self.spawn() {
                Ok(conn) => self.conn = Some(conn),
                Err(error) => {
                    tracing::warn!(%error, "GPU worker did not start");
                    self.failed_at = Some(Instant::now());
                    return None;
                }
            }
        }
        let conn = self.conn.as_mut()?;
        let exchange = async {
            conn.stdin.write_all(b"sample\n").await.ok()?;
            conn.stdin.flush().await.ok()?;
            let mut line = String::new();
            let read = conn.stdout.read_line(&mut line).await.ok()?;
            (read > 0).then_some(line)
        };
        match tokio::time::timeout(WORKER_TIMEOUT, exchange).await {
            Ok(Some(line)) => serde_json::from_str::<Option<GpuMetrics>>(line.trim()).ok()?,
            _ => {
                // Broken pipe, EOF or timeout: drop the worker (kill_on_drop) and retry later.
                if let Some(mut conn) = self.conn.take() {
                    let _ = conn.child.start_kill();
                }
                self.failed_at = Some(Instant::now());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_nvml_gives_none() {
        let mut gpu = Gpu::with_loader(Box::new(|| None));
        assert_eq!(gpu.sample(), None);
        // Immediately asking again does not retry the load.
        assert_eq!(gpu.sample(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_worker_gives_none_and_backs_off() {
        let mut client = WorkerClient::new(PathBuf::from(r"Z:\no\such\worker.exe"), vec![]);
        assert_eq!(client.sample().await, None);
        // Within the back-off window nothing is retried.
        assert_eq!(client.sample().await, None);
    }
}
