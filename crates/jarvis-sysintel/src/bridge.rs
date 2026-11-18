//! The bridge from the system intelligence's events to the core event bus (`jarvis_core::Event`),
//! so that the shell hears alerts and crashes without depending on this crate's types.

use crate::alerts::{Alert, Severity};
use crate::metrics::SysEvent;
use jarvis_core::{AlertNotice, AlertSeverity, Event};

pub fn notice_from(alert: &Alert) -> AlertNotice {
    AlertNotice {
        id: alert.id.as_str().into(),
        kind: alert.kind.as_str().into(),
        severity: match alert.severity {
            Severity::Info => AlertSeverity::Info,
            Severity::Warning => AlertSeverity::Warning,
            Severity::Critical => AlertSeverity::Critical,
        },
        message_ru: alert.message_ru.as_str().into(),
        message_en: alert.message_en.as_str().into(),
        speak: alert.speak,
    }
}

/// `SysEvent::Alert` -> `Event::Alert`, `SysEvent::AppCrashed` -> `Event::AppCrashed`.
pub fn to_bus_event(event: &SysEvent) -> Event {
    match event {
        SysEvent::Alert(alert) => Event::Alert(notice_from(alert)),
        SysEvent::AppCrashed(crash) => Event::AppCrashed {
            name: crash.name.as_str().into(),
            time_ms: crash.time_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::AlertKind;
    use crate::metrics::AppCrashed;

    fn alert(severity: Severity, speak: bool) -> Alert {
        Alert {
            id: "a-7".into(),
            kind: AlertKind::GpuTemp,
            severity,
            message_ru: "Видеокарта перегрета".into(),
            message_en: "The GPU is overheating".into(),
            speak,
        }
    }

    #[test]
    fn an_alert_becomes_a_bus_alert() {
        let Event::Alert(notice) = to_bus_event(&SysEvent::Alert(alert(Severity::Critical, true)))
        else {
            panic!("expected Event::Alert");
        };
        assert_eq!(&*notice.id, "a-7");
        assert_eq!(&*notice.kind, "gpu_temp");
        assert_eq!(notice.severity, AlertSeverity::Critical);
        assert_eq!(&*notice.message_ru, "Видеокарта перегрета");
        assert_eq!(&*notice.message_en, "The GPU is overheating");
        assert!(notice.speak);
        let Event::Alert(quiet) = to_bus_event(&SysEvent::Alert(alert(Severity::Info, false)))
        else {
            panic!("expected Event::Alert");
        };
        assert_eq!(quiet.severity, AlertSeverity::Info);
        assert!(!quiet.speak);
    }

    #[test]
    fn a_crash_becomes_a_bus_crash() {
        let event = to_bus_event(&SysEvent::AppCrashed(AppCrashed {
            name: "game.exe".into(),
            time_ms: 1234,
        }));
        assert_eq!(
            event,
            Event::AppCrashed {
                name: "game.exe".into(),
                time_ms: 1234
            }
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_service_publishes_what_it_emits_on_the_bus() {
        use crate::{SysIntel, SysIntelConfig};
        let bus = jarvis_core::EventBus::new();
        let mut receiver = bus.subscribe();
        let service = SysIntel::new(bus, SysIntelConfig::default());
        service.inject_event(SysEvent::Alert(alert(Severity::Warning, false)));
        let event = receiver.recv().await.expect("bus event");
        assert!(matches!(&*event, Event::Alert(n) if &*n.id == "a-7"));
        assert_eq!(service.recent_alerts(5).len(), 1);
    }
}
