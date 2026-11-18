//! Metric snapshot types. Every value that a sensor may not provide is an `Option`: `None` means
//! "unavailable on this machine", never a placeholder number.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CpuMetrics {
    /// Total load, 0..100.
    pub usage_percent: Option<f32>,
    /// Load of every logical core, 0..100.
    pub per_core_percent: Vec<f32>,
    /// Current frequency of the first core in MHz (sysinfo reads `CallNtPowerInformation`).
    pub frequency_mhz: Option<u64>,
    /// Package temperature. Only available when LibreHardwareMonitor runs and publishes WMI.
    pub temperature_c: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuMetrics {
    pub name: Option<String>,
    pub utilization_percent: Option<u32>,
    pub temperature_c: Option<u32>,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub graphics_clock_mhz: Option<u32>,
    pub memory_clock_mhz: Option<u32>,
    pub power_watts: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RamMetrics {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

impl RamMetrics {
    /// Used share of physical memory, 0..100.
    pub fn used_percent(&self) -> Option<f32> {
        (self.total_bytes > 0).then(|| self.used_bytes as f32 / self.total_bytes as f32 * 100.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct VolumeMetrics {
    /// Root path such as `C:\`.
    pub mount: String,
    pub is_system: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// Bytes per second since the previous sample; `None` for the first sample or when the volume
    /// does not expose `IOCTL_DISK_PERFORMANCE`.
    pub read_bytes_per_sec: Option<u64>,
    pub write_bytes_per_sec: Option<u64>,
}

impl VolumeMetrics {
    pub fn free_percent(&self) -> Option<f32> {
        (self.total_bytes > 0).then(|| self.free_bytes as f32 / self.total_bytes as f32 * 100.0)
    }
}

/// Health of a physical drive as Windows reports it without elevation (`MSFT_PhysicalDisk`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriveHealth {
    pub name: String,
    /// `Healthy`, `Warning`, `Unhealthy` or `Unknown`.
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StorageMetrics {
    pub volumes: Vec<VolumeMetrics>,
    /// `None` when the storage WMI namespace cannot be read.
    pub health: Option<Vec<DriveHealth>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NetworkMetrics {
    /// At least one physical adapter is connected. `None` if the adapter table cannot be read.
    pub link_up: Option<bool>,
    /// The last TCP-connect probe succeeded. `None` until the first probe finished.
    pub internet_reachable: Option<bool>,
    pub down_bytes_per_sec: Option<u64>,
    pub up_bytes_per_sec: Option<u64>,
    /// Round trip of the last successful TCP connect, milliseconds.
    pub rtt_ms: Option<f32>,
    pub rtt_host: Option<String>,
}

/// A visible top-level window that does not answer messages (`IsHungAppWindow`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HungWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub process: Option<String>,
    pub title: String,
}

/// One entry of a process ranking (all processes sharing an executable name are summed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessUsage {
    /// Executable file name, for example `chrome.exe`.
    pub exe: String,
    /// Name for people, for example `Google Chrome` (file description, falls back to the exe stem).
    pub display_name: String,
    pub instances: u32,
    pub memory_mb: u64,
    /// Percent of one logical core, `None` for rankings that were not sampled over an interval.
    pub cpu_percent: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Metrics {
    /// Unix time of the snapshot, milliseconds.
    pub timestamp_ms: u64,
    pub cpu: CpuMetrics,
    pub gpu: Option<GpuMetrics>,
    pub ram: RamMetrics,
    pub storage: StorageMetrics,
    pub network: NetworkMetrics,
    pub hung_windows: Vec<HungWindow>,
}

/// What the alert engine raised (or the crash watcher saw), delivered on the event channel.
#[derive(Debug, Clone, PartialEq)]
pub enum SysEvent {
    Alert(crate::alerts::Alert),
    /// An application ended unexpectedly (Windows Error Reporting / Application Error).
    AppCrashed(AppCrashed),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppCrashed {
    /// Faulting executable, for example `chrome.exe`.
    pub name: String,
    /// Event time as Unix milliseconds.
    pub time_ms: u64,
}
