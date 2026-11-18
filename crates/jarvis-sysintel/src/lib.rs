//! System intelligence for J.A.R.V.I.S.: the single source of system metrics and the alert engine.
//!
//! Only real sensors are read; anything that is not available on the machine is `None`.
//! Everything runs without admin rights and without drivers (read-only system access).

pub mod alerts;
pub mod bridge;
pub mod config;
pub mod crash;
pub mod d3dkmt;
pub mod gpu;
pub mod hung;
pub mod metrics;
pub mod network;
pub mod procs;
pub mod report;
pub mod service;
pub mod storage;
pub mod wmi;

pub use alerts::{Alert, AlertEngine, AlertInput, AlertKind, DiskSample, Severity};
pub use config::{AlertConfig, Mode, QuietHours, Rates, SysIntelConfig};
pub use metrics::{
    AppCrashed, CpuMetrics, DriveHealth, GpuMetrics, HungWindow, Metrics, NetworkMetrics,
    ProcessUsage, RamMetrics, StorageMetrics, SysEvent, VolumeMetrics,
};
pub use procs::Rank;
pub use service::SysIntel;
