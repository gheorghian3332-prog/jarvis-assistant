//! The model's view of the machine, fed by the system intelligence: CPU, RAM, GPU, the last five
//! alerts and, when somebody asked for them lately, the three heaviest processes.

use jarvis_brain::{GpuState, SystemSnapshot, SystemState};
use jarvis_sysintel::{Alert, Metrics, ProcessUsage, SysIntel};
use std::{sync::Arc, time::Duration};

/// A process ranking older than this is not shown to the model.
const TOP_MAX_AGE: Duration = Duration::from_secs(120);

pub struct SysIntelSnapshot {
    sys: Arc<SysIntel>,
}

impl SysIntelSnapshot {
    pub fn new(sys: Arc<SysIntel>) -> Self {
        Self { sys }
    }
}

const MIB: u64 = 1024 * 1024;

/// Pure conversion, tested on fixtures.
pub fn state_from(metrics: &Metrics, alerts: &[Alert], top: &[ProcessUsage]) -> SystemState {
    SystemState {
        cpu_percent: metrics.cpu.usage_percent,
        ram_used_bytes: metrics.ram.used_bytes,
        ram_total_bytes: metrics.ram.total_bytes,
        gpu: metrics.gpu.as_ref().map(|gpu| GpuState {
            name: gpu.name.clone().unwrap_or_else(|| "GPU".into()),
            util_percent: gpu.utilization_percent.map(|load| load as f32),
            vram_used_mb: gpu.vram_used_bytes.map(|bytes| bytes / MIB),
            vram_total_mb: gpu.vram_total_bytes.map(|bytes| bytes / MIB),
        }),
        alerts: alerts
            .iter()
            .take(5)
            .map(|alert| alert.message_ru.clone())
            .collect(),
        top_processes: top
            .iter()
            .take(3)
            .map(|process| format!("{} {} MB", process.display_name, process.memory_mb))
            .collect(),
    }
}

impl SystemSnapshot for SysIntelSnapshot {
    fn snapshot(&self) -> SystemState {
        let metrics = self.sys.snapshot();
        let top = self
            .sys
            .cached_top_processes(TOP_MAX_AGE)
            .map(|(_, list)| list)
            .unwrap_or_default();
        state_from(&metrics, &self.sys.recent_alerts(5), &top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_sysintel::alerts::{AlertKind, Severity};
    use jarvis_sysintel::metrics::{GpuMetrics, RamMetrics};
    use jarvis_sysintel::SysEvent;

    fn alert(n: u32) -> Alert {
        Alert {
            id: format!("a-{n}"),
            kind: AlertKind::CpuLoad,
            severity: Severity::Warning,
            message_ru: format!("предупреждение {n}"),
            message_en: format!("alert {n}"),
            speak: false,
        }
    }

    #[test]
    fn metrics_alerts_and_processes_become_the_model_context() {
        let mut metrics = Metrics::default();
        metrics.cpu.usage_percent = Some(31.0);
        metrics.ram = RamMetrics {
            used_bytes: 8 << 30,
            total_bytes: 16 << 30,
            available_bytes: 8 << 30,
        };
        metrics.gpu = Some(GpuMetrics {
            name: Some("GTX 1660".into()),
            utilization_percent: Some(40),
            vram_used_bytes: Some(1024 * MIB),
            vram_total_bytes: Some(6144 * MIB),
            ..GpuMetrics::default()
        });
        let alerts: Vec<Alert> = (1..=7).map(alert).collect();
        let top = vec![ProcessUsage {
            exe: "chrome.exe".into(),
            display_name: "Chrome".into(),
            instances: 9,
            memory_mb: 1500,
            cpu_percent: None,
        }];
        let state = state_from(&metrics, &alerts, &top);
        assert_eq!(state.cpu_percent, Some(31.0));
        assert_eq!(state.alerts.len(), 5, "only the last five");
        assert_eq!(state.alerts[0], "предупреждение 1");
        assert_eq!(state.top_processes, ["Chrome 1500 MB"]);
        let gpu = state.gpu.as_ref().expect("gpu");
        assert_eq!(gpu.vram_total_mb, Some(6144));
        let text = state.summary();
        assert!(text.contains("CPU 31%") && text.contains("VRAM 1024/6144 MB"));
        assert!(text.contains("Heaviest processes: Chrome 1500 MB"));
    }

    #[test]
    fn a_machine_without_a_gpu_reading_has_no_gpu_line() {
        let state = state_from(&Metrics::default(), &[], &[]);
        assert!(state.gpu.is_none() && state.alerts.is_empty());
    }

    #[tokio::test]
    async fn the_live_snapshot_carries_the_alerts_the_service_raised() {
        let sys = Arc::new(SysIntel::new(
            jarvis_core::EventBus::new(),
            jarvis_sysintel::SysIntelConfig::default(),
        ));
        for n in 1..=6 {
            sys.inject_event(SysEvent::Alert(alert(n)));
        }
        let state = SysIntelSnapshot::new(Arc::clone(&sys)).snapshot();
        assert_eq!(state.alerts.len(), 5);
        assert_eq!(state.alerts[0], "предупреждение 6", "newest first");
        // No ranking was requested, so none is shown.
        assert!(state.top_processes.is_empty());
        sys.top_processes(jarvis_sysintel::Rank::Ram, 3)
            .await
            .expect("ranking");
        assert_eq!(SysIntelSnapshot::new(sys).snapshot().top_processes.len(), 3);
    }
}
