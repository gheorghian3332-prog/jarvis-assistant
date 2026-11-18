use crate::service::{CoreResult, Service};
use crate::{Event, EventBus, Metrics};
use async_trait::async_trait;
use std::{sync::Mutex, time::Duration};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::{sync::watch, time};

/// Poll period when nobody listens to `Event::Metrics` (0.2 Hz).
pub const IDLE_INTERVAL: Duration = Duration::from_secs(5);
/// Poll period when at least one subscriber exists (1 Hz).
pub const ACTIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Publishes CPU and memory metrics. Reuses a single `System` and never refreshes processes.
pub struct Monitor {
    bus: EventBus,
    system: Mutex<System>,
    refresh: RefreshKind,
    stop: watch::Sender<bool>,
    latest: Mutex<Option<Metrics>>,
    active_interval: Duration,
    idle_interval: Duration,
}

impl Monitor {
    pub fn new(bus: EventBus) -> Self {
        Self::with_intervals(bus, ACTIVE_INTERVAL, IDLE_INTERVAL)
    }

    pub fn with_intervals(
        bus: EventBus,
        active_interval: Duration,
        idle_interval: Duration,
    ) -> Self {
        let (stop, _) = watch::channel(false);
        let refresh = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram());
        Self {
            bus,
            system: Mutex::new(System::new_with_specifics(refresh)),
            refresh,
            stop,
            latest: Mutex::new(None),
            active_interval,
            idle_interval,
        }
    }

    /// Current poll period, derived from the number of bus subscribers.
    pub fn interval(&self) -> Duration {
        if self.bus.receiver_count() == 0 {
            self.idle_interval
        } else {
            self.active_interval
        }
    }

    /// The metrics of the last poll (none before the first one). Costs nothing: the poll runs anyway.
    pub fn latest(&self) -> Option<Metrics> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn sample(&self) -> Metrics {
        let mut system = self
            .system
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        system.refresh_specifics(self.refresh);
        Metrics {
            cpu_percent: system.global_cpu_usage(),
            used_memory_bytes: system.used_memory(),
            total_memory_bytes: system.total_memory(),
        }
    }
}

#[async_trait]
impl Service for Monitor {
    fn name(&self) -> &'static str {
        "monitor"
    }

    async fn start(&self) -> CoreResult<()> {
        let mut stop = self.stop.subscribe();
        loop {
            if *stop.borrow_and_update() {
                return Ok(());
            }
            tokio::select! {
                () = time::sleep(self.interval()) => {
                    // No subscribers is a normal state; the event is simply dropped.
                    let metrics = self.sample();
                    *self
                        .latest
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(metrics.clone());
                    let _ = self.bus.publish(Event::Metrics(metrics));
                }
                changed = stop.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn stop(&self) -> CoreResult<()> {
        self.stop.send_replace(true);
        Ok(())
    }

    fn critical(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn interval_follows_subscribers() {
        let bus = EventBus::new();
        let monitor = Monitor::new(bus.clone());
        assert_eq!(monitor.interval(), Duration::from_secs(5));
        let receiver = bus.subscribe();
        assert_eq!(monitor.interval(), Duration::from_secs(1));
        drop(receiver);
        assert_eq!(monitor.interval(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn publishes_faster_with_a_subscriber() {
        let bus = EventBus::new();
        let monitor = Arc::new(Monitor::new(bus.clone()));
        let runner = Arc::clone(&monitor);
        let task = tokio::spawn(async move { runner.start().await });

        // Ticks fire at t = 5, 10, ... while nobody is subscribed.
        time::sleep(Duration::from_millis(10_500)).await;
        let mut receiver = bus.subscribe();
        // The tick at t = 15 was armed at the idle rate; afterwards the rate is 1 Hz.
        time::sleep(Duration::from_millis(4_600)).await;
        while receiver.try_recv().is_ok() {}
        time::sleep(Duration::from_secs(10)).await;
        let mut fast = 0;
        while let Ok(event) = receiver.try_recv() {
            assert!(matches!(*event, Event::Metrics(_)));
            fast += 1;
        }
        assert_eq!(fast, 10);

        // Back to the idle rate once the subscriber is gone.
        drop(receiver);
        time::sleep(Duration::from_secs(1)).await;
        let mut late = bus.subscribe();
        drop(late.try_recv());
        assert!(monitor.latest().is_some());
        monitor.stop().await.expect("stop");
        task.await.expect("join").expect("monitor result");
    }
}
