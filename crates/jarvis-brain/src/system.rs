//! A short view of the machine for the model's context.
//!
//! P7-B wires the real system intelligence (GPU, alerts) behind [`SystemSnapshot`]; until then the
//! snapshot comes from the metrics the core's `Monitor` already polls (CPU and RAM), at no extra cost.

use jarvis_core::monitor::Monitor;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GpuState {
    pub name: String,
    pub util_percent: Option<f32>,
    pub vram_used_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SystemState {
    pub cpu_percent: Option<f32>,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub gpu: Option<GpuState>,
    /// The latest alerts, newest first.
    pub alerts: Vec<String>,
    /// The heaviest processes, when somebody asked for them recently (for example "Chrome 1200 MB").
    pub top_processes: Vec<String>,
}

impl SystemState {
    /// One or two lines for the prompt; empty when nothing is known.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cpu) = self.cpu_percent {
            parts.push(format!("CPU {cpu:.0}%"));
        }
        if self.ram_total_bytes > 0 {
            let gib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            parts.push(format!(
                "RAM {:.1}/{:.1} GiB",
                gib(self.ram_used_bytes),
                gib(self.ram_total_bytes)
            ));
        }
        if let Some(gpu) = &self.gpu {
            let mut text = format!("GPU {}", gpu.name);
            if let Some(util) = gpu.util_percent {
                text.push_str(&format!(" {util:.0}%"));
            }
            if let (Some(used), Some(total)) = (gpu.vram_used_mb, gpu.vram_total_mb) {
                text.push_str(&format!(", VRAM {used}/{total} MB"));
            }
            parts.push(text);
        }
        if !self.top_processes.is_empty() {
            let top: Vec<&str> = self
                .top_processes
                .iter()
                .take(3)
                .map(String::as_str)
                .collect();
            parts.push(format!("Heaviest processes: {}", top.join(", ")));
        }
        let mut line = parts.join("; ");
        if !self.alerts.is_empty() {
            let alerts: Vec<&str> = self.alerts.iter().take(3).map(String::as_str).collect();
            line.push_str(&format!("\nRecent alerts: {}", alerts.join(" | ")));
        }
        line
    }
}

/// Where the model's system context comes from.
pub trait SystemSnapshot: Send + Sync {
    fn snapshot(&self) -> SystemState;
}

/// No information (tests, or before the first poll).
pub struct NoSnapshot;

impl SystemSnapshot for NoSnapshot {
    fn snapshot(&self) -> SystemState {
        SystemState::default()
    }
}

/// CPU and RAM from the core's `Monitor`.
pub struct MonitorSnapshot {
    monitor: Arc<Monitor>,
}

impl MonitorSnapshot {
    pub fn new(monitor: Arc<Monitor>) -> Self {
        Self { monitor }
    }
}

impl SystemSnapshot for MonitorSnapshot {
    fn snapshot(&self) -> SystemState {
        match self.monitor.latest() {
            Some(metrics) => SystemState {
                cpu_percent: Some(metrics.cpu_percent),
                ram_used_bytes: metrics.used_memory_bytes,
                ram_total_bytes: metrics.total_memory_bytes,
                gpu: None,
                alerts: Vec::new(),
                top_processes: Vec::new(),
            },
            None => SystemState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_lists_what_is_known() {
        assert_eq!(SystemState::default().summary(), "");
        let state = SystemState {
            cpu_percent: Some(12.4),
            ram_used_bytes: 8 * 1024 * 1024 * 1024,
            ram_total_bytes: 16 * 1024 * 1024 * 1024,
            gpu: Some(GpuState {
                name: "GTX 1660".into(),
                util_percent: Some(30.0),
                vram_used_mb: Some(1135),
                vram_total_mb: Some(6144),
            }),
            alerts: vec!["CPU hot".into()],
            top_processes: vec!["Chrome 1200 MB".into()],
        };
        let text = state.summary();
        assert!(text.contains("CPU 12%") && text.contains("RAM 8.0/16.0 GiB"));
        assert!(text.contains("VRAM 1135/6144 MB") && text.contains("CPU hot"));
        assert!(text.contains("Heaviest processes: Chrome 1200 MB"));
    }

    #[test]
    fn the_monitor_snapshot_is_empty_before_the_first_poll() {
        let monitor = Arc::new(Monitor::new(jarvis_core::EventBus::new()));
        assert_eq!(
            MonitorSnapshot::new(monitor).snapshot(),
            SystemState::default()
        );
    }
}
