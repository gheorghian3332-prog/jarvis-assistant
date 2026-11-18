use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{
    sync::{watch, Notify},
    task::JoinHandle,
    time::{sleep, Instant},
};
use tracing::{error, info, warn};

pub const BACKOFF_MIN: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("task join error: {0}")]
    Join(String),
    #[error("service error: {0}")]
    Service(String),
}

impl CoreError {
    pub fn join(error: tokio::task::JoinError) -> Self {
        Self::Join(error.to_string())
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

#[async_trait]
pub trait Service: Send + Sync {
    fn name(&self) -> &'static str;
    /// Runs until finished, failed or cancelled (the supervisor drops the future on stop).
    async fn start(&self) -> CoreResult<()>;
    async fn stop(&self) -> CoreResult<()>;
    fn critical(&self) -> bool;
    /// For non-critical services: stop after this long without `Supervisor::touch`.
    fn idle_timeout(&self) -> Option<Duration> {
        None
    }
}

/// Called when a service failed or panicked and is about to be restarted: `(service, message)`.
pub type FailureNotifier = Arc<dyn Fn(&'static str, &str) + Send + Sync>;

/// A running `Service::start`, in its own task so that a panic is caught instead of ending the supervisor.
/// Dropping it cancels the service, exactly like dropping the future did before.
struct Run(JoinHandle<CoreResult<()>>);

impl Run {
    fn spawn(service: &Arc<dyn Service>) -> Self {
        let service = Arc::clone(service);
        Self(tokio::spawn(async move { service.start().await }))
    }

    async fn finished(&mut self) -> CoreResult<()> {
        match (&mut self.0).await {
            Ok(result) => result,
            Err(error) if error.is_panic() => {
                Err(CoreError::Service("the service panicked".into()))
            }
            Err(error) => Err(CoreError::join(error)),
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Managed {
    service: Arc<dyn Service>,
    activity: Arc<Notify>,
}

/// Starts services, restarts failed ones with exponential backoff (1 s to 30 s),
/// and stops non-critical ones after their idle timeout until they are touched again.
pub struct Supervisor {
    services: Vec<Managed>,
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    notifier: Option<FailureNotifier>,
}

impl Supervisor {
    pub fn new(services: Vec<Arc<dyn Service>>) -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            services: services
                .into_iter()
                .map(|service| Managed {
                    service,
                    activity: Arc::new(Notify::new()),
                })
                .collect(),
            stop,
            tasks: Vec::new(),
            notifier: None,
        }
    }

    /// Reports failures and panics (the UI shows WARNING) in addition to the log.
    #[must_use]
    pub fn with_notifier(mut self, notifier: FailureNotifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    pub fn start(&mut self) {
        for managed in &self.services {
            let service = Arc::clone(&managed.service);
            let activity = Arc::clone(&managed.activity);
            let stop = self.stop.subscribe();
            let notifier = self.notifier.clone();
            self.tasks
                .push(tokio::spawn(supervise(service, activity, stop, notifier)));
        }
    }

    /// Marks a service as used: resets its idle timer and restarts it if it was idle-stopped.
    pub fn touch(&self, name: &str) {
        for managed in &self.services {
            if managed.service.name() == name {
                managed.activity.notify_one();
            }
        }
    }

    pub async fn stop(mut self) -> CoreResult<()> {
        self.stop.send_replace(true);
        for managed in &self.services {
            if let Err(error) = managed.service.stop().await {
                error!(service = managed.service.name(), %error, "service stop failed");
            }
        }
        for task in self.tasks.drain(..) {
            task.await.map_err(CoreError::join)?;
        }
        Ok(())
    }
}

enum Outcome {
    Exited(CoreResult<()>),
    Idle,
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

async fn idle_expired(idle: Option<Duration>, activity: &Notify) {
    match idle {
        None => std::future::pending().await,
        Some(limit) => loop {
            tokio::select! {
                () = sleep(limit) => return,
                () = activity.notified() => {}
            }
        },
    }
}

/// Waits until the service is touched. Returns false if the supervisor is stopping.
async fn wait_for_activity(activity: &Notify, stop: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = activity.notified() => true,
        () = stopped(stop) => false,
    }
}

async fn supervise(
    service: Arc<dyn Service>,
    activity: Arc<Notify>,
    mut stop: watch::Receiver<bool>,
    notifier: Option<FailureNotifier>,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let started = Instant::now();
        let idle = if service.critical() {
            None
        } else {
            service.idle_timeout()
        };
        let mut run = Run::spawn(&service);
        let outcome = tokio::select! {
            result = run.finished() => Outcome::Exited(result),
            () = idle_expired(idle, &activity) => Outcome::Idle,
            () = stopped(&mut stop) => return,
        };
        drop(run);
        match outcome {
            Outcome::Idle => {
                info!(service = service.name(), "idle timeout; stopping service");
                if let Err(error) = service.stop().await {
                    error!(service = service.name(), %error, "service stop failed");
                }
                if !wait_for_activity(&activity, &mut stop).await {
                    return;
                }
                backoff = BACKOFF_MIN;
            }
            Outcome::Exited(Ok(())) if !service.critical() => {
                info!(service = service.name(), "service finished");
                if !wait_for_activity(&activity, &mut stop).await {
                    return;
                }
                backoff = BACKOFF_MIN;
            }
            Outcome::Exited(result) => {
                match result {
                    Ok(()) => warn!(
                        service = service.name(),
                        "critical service exited; restarting"
                    ),
                    Err(error) => {
                        error!(service = service.name(), %error, "service failed; restarting");
                        if let Some(notify) = &notifier {
                            notify(service.name(), &error.to_string());
                        }
                    }
                }
                if started.elapsed() >= BACKOFF_MAX {
                    backoff = BACKOFF_MIN;
                }
                tokio::select! {
                    () = sleep(backoff) => {}
                    () = stopped(&mut stop) => return,
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Failing {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl Service for Failing {
        fn name(&self) -> &'static str {
            "failing"
        }
        async fn start(&self) -> CoreResult<()> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::Service("failed".into()))
        }
        async fn stop(&self) -> CoreResult<()> {
            Ok(())
        }
        fn critical(&self) -> bool {
            true
        }
    }

    struct Lazy {
        starts: AtomicUsize,
        stops: AtomicUsize,
    }

    #[async_trait]
    impl Service for Lazy {
        fn name(&self) -> &'static str {
            "lazy"
        }
        async fn start(&self) -> CoreResult<()> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
        async fn stop(&self) -> CoreResult<()> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn critical(&self) -> bool {
            false
        }
        fn idle_timeout(&self) -> Option<Duration> {
            Some(Duration::from_secs(10))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_failed_service_with_exponential_backoff() {
        let service = Arc::new(Failing {
            attempts: AtomicUsize::new(0),
        });
        let mut supervisor = Supervisor::new(vec![service.clone()]);
        supervisor.start();
        // Attempts happen at t = 0, 1, 3, 7 s (delays 1, 2, 4).
        sleep(Duration::from_millis(500)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 1);
        sleep(Duration::from_secs(1)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 2);
        sleep(Duration::from_secs(2)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 3);
        sleep(Duration::from_secs(4)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 4);
        supervisor.stop().await.expect("stop");
    }

    struct Panicking {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl Service for Panicking {
        fn name(&self) -> &'static str {
            "panicking"
        }
        async fn start(&self) -> CoreResult<()> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            assert!(attempt >= 2, "boom");
            std::future::pending().await
        }
        async fn stop(&self) -> CoreResult<()> {
            Ok(())
        }
        fn critical(&self) -> bool {
            true
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_service_is_restarted_with_backoff_and_reported() {
        let service = Arc::new(Panicking {
            attempts: AtomicUsize::new(0),
        });
        let reports: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let sink = Arc::clone(&reports);
        let mut supervisor =
            Supervisor::new(vec![service.clone()]).with_notifier(Arc::new(move |name, message| {
                sink.lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(format!("{name}: {message}"));
            }));
        supervisor.start();
        // Attempts at t = 0 (panic), 1 (panic), 3 (stays up).
        sleep(Duration::from_millis(500)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 1);
        sleep(Duration::from_secs(1)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 2);
        sleep(Duration::from_secs(2)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 3);
        let seen = reports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].contains("panicked"), "{seen:?}");
        supervisor
            .stop()
            .await
            .expect("a panic must not break the shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_is_capped_at_thirty_seconds() {
        let service = Arc::new(Failing {
            attempts: AtomicUsize::new(0),
        });
        let mut supervisor = Supervisor::new(vec![service.clone()]);
        supervisor.start();
        // Attempts at 0, 1, 3, 7, 15, 31 s (6 attempts), then every 30 s.
        sleep(Duration::from_secs(32)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 6);
        sleep(Duration::from_secs(30)).await;
        assert_eq!(service.attempts.load(Ordering::SeqCst), 7);
        supervisor.stop().await.expect("stop");
    }

    #[tokio::test(start_paused = true)]
    async fn stops_idle_service_and_restarts_on_touch() {
        let service = Arc::new(Lazy {
            starts: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
        });
        let mut supervisor = Supervisor::new(vec![service.clone()]);
        supervisor.start();
        sleep(Duration::from_secs(5)).await;
        assert_eq!(service.stops.load(Ordering::SeqCst), 0);
        supervisor.touch("lazy");
        sleep(Duration::from_secs(8)).await;
        assert_eq!(
            service.stops.load(Ordering::SeqCst),
            0,
            "touch resets timer"
        );
        sleep(Duration::from_secs(3)).await;
        assert_eq!(service.stops.load(Ordering::SeqCst), 1);
        assert_eq!(service.starts.load(Ordering::SeqCst), 1);
        supervisor.touch("lazy");
        sleep(Duration::from_secs(1)).await;
        assert_eq!(service.starts.load(Ordering::SeqCst), 2);
        supervisor.stop().await.expect("stop");
    }
}
