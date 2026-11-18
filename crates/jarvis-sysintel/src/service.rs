//! The sampling service: one task that reads every sensor group at its own adaptive rate, keeps the
//! latest [`Metrics`], feeds the alert engine and publishes events.
//!
//! Heavy or blocking reads (TCP probe, WMI) run on the blocking pool and report back through a
//! channel; everything else is a few microseconds to milliseconds and runs inline. All sensor
//! objects (sysinfo `System`, NVML, counters) are created once and reused.

use crate::alerts::{Alert, AlertEngine, AlertInput, DiskSample};
use crate::config::{Mode, Rates, SysIntelConfig, HUNG_RETRY, RTT_RETRY_WHILE_DOWN};
use crate::crash::CrashWatcher;
use crate::d3dkmt::D3dkmtGpu;
use crate::gpu::{Gpu, GpuSource, WorkerClient};
use crate::hung::find_hung_windows;
use crate::metrics::{
    AppCrashed, CpuMetrics, DriveHealth, GpuMetrics, Metrics, ProcessUsage, RamMetrics, SysEvent,
};
use crate::network::{probe_rtt, Throughput};
use crate::procs::{top_processes, top_ram_consumer, Rank};
use crate::storage::Storage;
use crate::wmi::{drive_health, libre_hardware_monitor_cpu_temp};
use async_trait::async_trait;
use jarvis_core::service::{CoreError, CoreResult, Service};
use jarvis_core::{Event, EventBus};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::time;
use windows::Win32::System::SystemInformation::GetLocalTime;

const RTT_TIMEOUT: Duration = Duration::from_secs(2);
const CHANNEL_CAPACITY: usize = 16;
/// How often the CPU temperature provider is asked while it answers / after it did not.
const CPU_TEMP_EVERY: Duration = Duration::from_secs(10);
const CPU_TEMP_RETRY_MISSING: Duration = Duration::from_secs(300);
const HEALTH_EVERY: Duration = Duration::from_secs(600);
/// Placeholder due time while a blocking read is in flight; its result sets the real one.
const IN_FLIGHT: Duration = Duration::from_secs(3600);
/// Look again this soon when a result is still pending at its due time.
const PENDING_RECHECK: Duration = Duration::from_secs(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The last process ranking somebody asked for.
struct TopCache {
    at: Instant,
    by: Rank,
    list: Vec<ProcessUsage>,
}

/// How many alerts are kept for `recent_alerts`.
const RECENT_ALERTS: usize = 50;

/// Requests to the task that owns the GPU helper process.
enum WorkerCmd {
    Sample,
    /// End the helper process (the window closed).
    Stop,
}

/// Results of work done on the blocking pool.
enum Background {
    Gpu(Option<GpuMetrics>),
    Rtt(Option<Duration>),
    CpuTemp(Option<f32>),
    Health(Option<Vec<DriveHealth>>),
}

pub struct SysIntel {
    bus: EventBus,
    config: Mutex<SysIntelConfig>,
    window_open: AtomicBool,
    wake: Notify,
    metrics_tx: broadcast::Sender<Arc<Metrics>>,
    events_tx: broadcast::Sender<Arc<SysEvent>>,
    latest: Mutex<Metrics>,
    engine: Mutex<AlertEngine>,
    gpu_source: GpuSource,
    stop: watch::Sender<bool>,
    recent: Mutex<VecDeque<(u64, Alert)>>,
    top_cache: Mutex<Option<TopCache>>,
    /// Process id of the GPU helper, 0 while none runs.
    worker_pid: Arc<AtomicU32>,
}

impl SysIntel {
    pub fn new(bus: EventBus, config: SysIntelConfig) -> Self {
        let (metrics_tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        let (events_tx, _) = broadcast::channel(CHANNEL_CAPACITY * 4);
        let (stop, _) = watch::channel(false);
        Self {
            bus,
            engine: Mutex::new(AlertEngine::new(config.alerts)),
            gpu_source: GpuSource::InProcess,
            config: Mutex::new(config),
            window_open: AtomicBool::new(false),
            wake: Notify::new(),
            metrics_tx,
            events_tx,
            latest: Mutex::new(Metrics::default()),
            stop,
            recent: Mutex::new(VecDeque::new()),
            top_cache: Mutex::new(None),
            worker_pid: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Chooses where GPU readings come from (default: NVML loaded into this process).
    #[must_use]
    pub fn with_gpu_source(mut self, source: GpuSource) -> Self {
        self.gpu_source = source;
        self
    }

    /// The window opened or closed; selects the polling table row.
    pub fn set_window_open(&self, open: bool) {
        self.window_open.store(open, Ordering::Relaxed);
        self.wake.notify_one();
    }

    /// Current polling mode: the window is open or somebody listens to the rich metrics stream.
    pub fn mode(&self) -> Mode {
        if self.window_open.load(Ordering::Relaxed) || self.metrics_tx.receiver_count() > 0 {
            Mode::Window
        } else {
            Mode::Tray
        }
    }

    /// Stream of full snapshots; subscribing switches the service to the faster window rates.
    pub fn subscribe_metrics(&self) -> broadcast::Receiver<Arc<Metrics>> {
        let receiver = self.metrics_tx.subscribe();
        self.wake.notify_one();
        receiver
    }

    /// Alerts and crash notices.
    pub fn subscribe_events(&self) -> broadcast::Receiver<Arc<SysEvent>> {
        self.events_tx.subscribe()
    }

    /// The latest values (all fields unset until the first sample).
    pub fn snapshot(&self) -> Metrics {
        lock(&self.latest).clone()
    }

    /// The latest alerts, newest first (at most `limit`).
    pub fn recent_alerts(&self, limit: usize) -> Vec<Alert> {
        lock(&self.recent)
            .iter()
            .rev()
            .take(limit)
            .map(|(_, alert)| alert.clone())
            .collect()
    }

    /// The latest alerts with their Unix time in milliseconds, newest first.
    pub fn recent_alerts_timed(&self, limit: usize) -> Vec<(u64, Alert)> {
        lock(&self.recent)
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// The last process ranking that was asked for, if it is not older than `max_age`. Nothing is
    /// sampled here: rankings exist only because somebody requested them.
    pub fn cached_top_processes(&self, max_age: Duration) -> Option<(Rank, Vec<ProcessUsage>)> {
        let cache = lock(&self.top_cache);
        let cache = cache.as_ref().filter(|c| c.at.elapsed() <= max_age)?;
        Some((cache.by, cache.list.clone()))
    }

    /// Process id of the NVML helper while it runs (only the window mode starts it).
    pub fn gpu_worker_pid(&self) -> Option<u32> {
        match self.worker_pid.load(Ordering::Relaxed) {
            0 => None,
            pid => Some(pid),
        }
    }

    pub fn set_config(&self, config: SysIntelConfig) {
        lock(&self.engine).set_config(config.alerts);
        *lock(&self.config) = config;
    }

    /// Process ranking on demand (not part of the periodic sample).
    pub async fn top_processes(&self, by: Rank, limit: usize) -> CoreResult<Vec<ProcessUsage>> {
        let list = tokio::task::spawn_blocking(move || top_processes(by, limit))
            .await
            .map_err(CoreError::join)?;
        *lock(&self.top_cache) = Some(TopCache {
            at: Instant::now(),
            by,
            list: list.clone(),
        });
        Ok(list)
    }

    fn publish_legacy(&self, metrics: &Metrics) {
        let Some(cpu) = metrics.cpu.usage_percent else {
            return;
        };
        // No receiver is a normal state; the event is dropped.
        let _ = self.bus.publish(Event::Metrics(jarvis_core::Metrics {
            cpu_percent: cpu,
            used_memory_bytes: metrics.ram.used_bytes,
            total_memory_bytes: metrics.ram.total_bytes,
        }));
    }

    /// Delivers an event as if the engine had raised it (diagnostics and tests).
    pub fn inject_event(&self, event: SysEvent) {
        self.emit(event);
    }

    fn emit(&self, event: SysEvent) {
        match &event {
            SysEvent::Alert(alert) => tracing::warn!(
                id = %alert.id,
                kind = alert.kind.as_str(),
                severity = alert.severity.as_str(),
                speak = alert.speak,
                message = %alert.message_ru,
                "alert raised"
            ),
            SysEvent::AppCrashed(crash) => {
                tracing::warn!(name = %crash.name, time_ms = crash.time_ms, "application crashed");
            }
        }
        if let SysEvent::Alert(alert) = &event {
            let mut recent = lock(&self.recent);
            if recent.len() >= RECENT_ALERTS {
                recent.pop_front();
            }
            recent.push_back((unix_ms(), alert.clone()));
        }
        // The shell listens to the core bus; no receiver is a normal state.
        let _ = self.bus.publish(crate::bridge::to_bus_event(&event));
        let _ = self.events_tx.send(Arc::new(event));
    }

    fn handle_crash(&self, crash: AppCrashed, started: Instant) {
        let now_ms = started.elapsed().as_millis() as u64;
        let alert = lock(&self.engine).on_crash(&crash, now_ms, local_minute());
        self.emit(SysEvent::AppCrashed(crash));
        if let Some(alert) = alert {
            self.emit(SysEvent::Alert(alert));
        }
    }

    async fn run(&self) -> CoreResult<()> {
        let mut stop = self.stop.subscribe();
        let started = Instant::now();
        let (bg_tx, mut bg_rx) = mpsc::unbounded_channel::<Background>();
        let spawn_worker = |exe: &std::path::Path, args: &[String]| {
            let (request_tx, mut request_rx) = mpsc::unbounded_channel::<WorkerCmd>();
            let mut client = WorkerClient::new(exe.to_path_buf(), args.to_vec());
            let reply = bg_tx.clone();
            let pid = Arc::clone(&self.worker_pid);
            // Ends (and kills the worker) when the sampler drops `request_tx`.
            tokio::spawn(async move {
                while let Some(command) = request_rx.recv().await {
                    match command {
                        WorkerCmd::Sample => {
                            let reading = client.sample().await;
                            pid.store(client.pid().unwrap_or(0), Ordering::Relaxed);
                            let _ = reply.send(Background::Gpu(reading));
                        }
                        WorkerCmd::Stop => {
                            client.shutdown().await;
                            pid.store(0, Ordering::Relaxed);
                        }
                    }
                }
                client.shutdown().await;
                pid.store(0, Ordering::Relaxed);
            });
            request_tx
        };
        let backend = match &self.gpu_source {
            GpuSource::InProcess => GpuBackend::InProcess(Box::default()),
            GpuSource::Off => GpuBackend::Off,
            GpuSource::D3dkmt => GpuBackend::D3dkmt(D3dState::default()),
            GpuSource::Worker { exe, args } => GpuBackend::Worker(spawn_worker(exe, args)),
            GpuSource::Adaptive { exe, args } => GpuBackend::Adaptive {
                d3d: D3dState::default(),
                worker: spawn_worker(exe, args),
                up: false,
            },
        };
        let mut sampler = Sampler::new(lock(&self.config).rtt_host.clone(), backend);
        let (crash_tx, mut crash_rx) = mpsc::unbounded_channel::<AppCrashed>();
        // Without the subscription crashes are simply not reported; everything else still works.
        let _watcher = match CrashWatcher::start(crash_tx) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                tracing::warn!(%error, "crash event subscription unavailable");
                None
            }
        };
        let mut due = Due::new(Instant::now());
        let mut dirty = false;
        let mut last_mode = self.mode();
        loop {
            if *stop.borrow_and_update() {
                return Ok(());
            }
            let mode = self.mode();
            sampler.set_rtt_host(&lock(&self.config).rtt_host);
            let rates = Rates::for_mode(mode);
            let now = Instant::now();
            due.clamp(now, &rates);
            if mode != last_mode {
                // The GPU source depends on the mode: switch (and end the helper) right away.
                last_mode = mode;
                sampler.mode_changed(&mut due, now);
            }
            let sampled = sampler.sample_due(&mut due, now, &rates, mode, &bg_tx);
            if sampled.any || dirty {
                dirty = false;
                let snapshot = sampler.snapshot();
                *lock(&self.latest) = snapshot.clone();
                if sampled.cpu_ram {
                    self.publish_legacy(&snapshot);
                }
                self.evaluate_alerts(&snapshot, started);
                if self.mode() == Mode::Window {
                    let _ = self.metrics_tx.send(Arc::new(snapshot));
                }
            }
            let sleep_for = due.next().saturating_duration_since(Instant::now());
            tokio::select! {
                () = time::sleep(sleep_for) => {}
                () = self.wake.notified() => {}
                Some(result) = bg_rx.recv() => {
                    sampler.apply_background(result, &mut due, Instant::now(), &Rates::for_mode(self.mode()));
                    dirty = true;
                }
                Some(crash) = crash_rx.recv() => self.handle_crash(crash, started),
                changed = stop.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn evaluate_alerts(&self, metrics: &Metrics, started: Instant) {
        let system_disk = metrics
            .storage
            .volumes
            .iter()
            .find(|v| v.is_system)
            .map(|v| DiskSample {
                free_bytes: v.free_bytes,
                total_bytes: v.total_bytes,
            });
        let internet_up = if metrics.network.link_up == Some(false) {
            Some(false)
        } else {
            metrics.network.internet_reachable
        };
        let input = AlertInput {
            now_ms: started.elapsed().as_millis() as u64,
            local_minute: local_minute(),
            cpu_percent: metrics.cpu.usage_percent,
            gpu_temp_c: metrics
                .gpu
                .as_ref()
                .and_then(|g| g.temperature_c)
                .map(|t| t as f32),
            cpu_temp_c: metrics.cpu.temperature_c,
            ram_percent: metrics.ram.used_percent(),
            system_disk,
            internet_up,
            hung: &metrics.hung_windows,
        };
        let alerts = lock(&self.engine).evaluate(&input, &mut top_ram_consumer);
        for alert in alerts {
            self.emit(SysEvent::Alert(alert));
        }
    }
}

/// Minutes since local midnight.
fn local_minute() -> u16 {
    // SAFETY: GetLocalTime only fills the returned structure.
    let time = unsafe { GetLocalTime() };
    time.wHour * 60 + time.wMinute
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Next due time per sensor group.
struct Due {
    cpu_ram: Instant,
    gpu: Instant,
    net_io: Instant,
    rtt: Instant,
    hung: Instant,
    volumes: Instant,
    cpu_temp: Instant,
    health: Instant,
}

/// What a sampling pass did.
struct Sampled {
    any: bool,
    cpu_ram: bool,
}

impl Due {
    fn new(now: Instant) -> Self {
        Self {
            cpu_ram: now,
            gpu: now,
            net_io: now,
            rtt: now,
            hung: now,
            volumes: now,
            cpu_temp: now,
            health: now,
        }
    }

    /// A faster mode pulls the next reads closer; a slower one leaves them (they were sooner).
    fn clamp(&mut self, now: Instant, rates: &Rates) {
        self.cpu_ram = self.cpu_ram.min(now + rates.cpu_ram);
        self.gpu = self.gpu.min(now + rates.gpu);
        self.net_io = self.net_io.min(now + rates.net_io);
        self.rtt = self.rtt.min(now + rates.rtt);
        self.hung = self.hung.min(now + rates.hung_windows);
        self.volumes = self.volumes.min(now + rates.volumes);
    }

    fn next(&self) -> Instant {
        [
            self.cpu_ram,
            self.gpu,
            self.net_io,
            self.rtt,
            self.hung,
            self.volumes,
            self.cpu_temp,
            self.health,
        ]
        .into_iter()
        .min()
        .unwrap_or(self.cpu_ram)
    }
}

/// The D3DKMT reader, opened on first use (`None` after a failed attempt: no WDDM adapter).
#[derive(Default)]
struct D3dState {
    gpu: Option<Box<D3dkmtGpu>>,
    tried: bool,
}

impl D3dState {
    fn sample(&mut self) -> Option<GpuMetrics> {
        if !self.tried {
            self.tried = true;
            self.gpu = D3dkmtGpu::open().map(Box::new);
        }
        self.gpu.as_mut()?.sample()
    }
}

enum GpuBackend {
    InProcess(Box<Gpu>),
    Worker(mpsc::UnboundedSender<WorkerCmd>),
    D3dkmt(D3dState),
    /// D3DKMT in the tray, the NVML helper while the window is open.
    Adaptive {
        d3d: D3dState,
        worker: mpsc::UnboundedSender<WorkerCmd>,
        /// A helper may be running (it was asked for a sample and not stopped since).
        up: bool,
    },
    Off,
}

/// Owns every sensor object and the current snapshot.
struct Sampler {
    system: System,
    refresh: RefreshKind,
    gpu: GpuBackend,
    gpu_inflight: bool,
    storage: Storage,
    throughput: Throughput,
    rtt_host: String,
    current: Metrics,
    rtt_inflight: bool,
    cpu_temp_inflight: bool,
    health_inflight: bool,
}

impl Sampler {
    fn new(rtt_host: String, gpu: GpuBackend) -> Self {
        let refresh = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage().with_frequency())
            .with_memory(MemoryRefreshKind::new().with_ram());
        Self {
            system: System::new_with_specifics(refresh),
            refresh,
            gpu,
            gpu_inflight: false,
            storage: Storage::new(),
            throughput: Throughput::new(),
            rtt_host,
            current: Metrics::default(),
            rtt_inflight: false,
            cpu_temp_inflight: false,
            health_inflight: false,
        }
    }

    fn set_rtt_host(&mut self, host: &str) {
        if self.rtt_host != host {
            self.rtt_host = host.to_owned();
        }
    }

    fn snapshot(&self) -> Metrics {
        let mut snapshot = self.current.clone();
        snapshot.timestamp_ms = unix_ms();
        snapshot
    }

    /// Reads whatever is due. Returns true when any value changed.
    fn sample_due(
        &mut self,
        due: &mut Due,
        now: Instant,
        rates: &Rates,
        mode: Mode,
        bg: &mpsc::UnboundedSender<Background>,
    ) -> Sampled {
        let mut changed = false;
        let mut cpu_ram = false;
        if now >= due.cpu_ram {
            self.read_cpu_ram();
            due.cpu_ram = now + rates.cpu_ram;
            cpu_ram = true;
            changed = true;
        }
        if now >= due.gpu {
            match &mut self.gpu {
                GpuBackend::InProcess(gpu) => {
                    self.current.gpu = gpu.sample();
                    due.gpu = now + rates.gpu;
                    changed = true;
                }
                GpuBackend::Off => due.gpu = now + IN_FLIGHT,
                GpuBackend::D3dkmt(d3d) => {
                    self.current.gpu = d3d.sample();
                    due.gpu = now + rates.gpu;
                    changed = true;
                }
                GpuBackend::Worker(requests) => {
                    if self.gpu_inflight {
                        due.gpu = now + PENDING_RECHECK;
                    } else if requests.send(WorkerCmd::Sample).is_ok() {
                        self.gpu_inflight = true;
                        due.gpu = now + IN_FLIGHT;
                    } else {
                        due.gpu = now + IN_FLIGHT;
                    }
                }
                GpuBackend::Adaptive { d3d, worker, up } => {
                    if mode == Mode::Tray {
                        if *up && worker.send(WorkerCmd::Stop).is_ok() {
                            *up = false;
                        }
                        self.current.gpu = d3d.sample();
                        due.gpu = now + rates.gpu;
                        changed = true;
                    } else if self.gpu_inflight {
                        due.gpu = now + PENDING_RECHECK;
                    } else if worker.send(WorkerCmd::Sample).is_ok() {
                        *up = true;
                        self.gpu_inflight = true;
                        due.gpu = now + IN_FLIGHT;
                    } else {
                        due.gpu = now + IN_FLIGHT;
                    }
                }
            }
        }
        if now >= due.net_io {
            let (link, down, up) = self.throughput.sample();
            self.current.network.link_up = link;
            self.current.network.down_bytes_per_sec = down;
            self.current.network.up_bytes_per_sec = up;
            self.storage
                .refresh_activity(&mut self.current.storage.volumes);
            due.net_io = now + rates.net_io;
            changed = true;
        }
        if now >= due.volumes {
            let health = self.current.storage.health.take();
            self.current.storage.volumes = self.storage.sample(true);
            self.current.storage.health = health;
            due.volumes = now + rates.volumes;
            changed = true;
        }
        if now >= due.hung {
            self.current.hung_windows = find_hung_windows();
            due.hung = now
                + if self.current.hung_windows.is_empty() {
                    rates.hung_windows
                } else {
                    HUNG_RETRY
                };
            changed = true;
        }
        if now >= due.rtt {
            if self.rtt_inflight {
                due.rtt = now + PENDING_RECHECK;
            } else {
                self.rtt_inflight = true;
                let host = self.rtt_host.clone();
                let bg = bg.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = bg.send(Background::Rtt(probe_rtt(&host, RTT_TIMEOUT)));
                });
                due.rtt = now + IN_FLIGHT;
            }
        }
        if now >= due.cpu_temp {
            if self.cpu_temp_inflight {
                due.cpu_temp = now + PENDING_RECHECK;
            } else {
                self.cpu_temp_inflight = true;
                let bg = bg.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = bg.send(Background::CpuTemp(libre_hardware_monitor_cpu_temp()));
                });
                due.cpu_temp = now + IN_FLIGHT;
            }
        }
        if now >= due.health {
            if self.health_inflight {
                due.health = now + PENDING_RECHECK;
            } else {
                self.health_inflight = true;
                let bg = bg.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = bg.send(Background::Health(drive_health()));
                });
                due.health = now + IN_FLIGHT;
            }
        }
        Sampled {
            any: changed,
            cpu_ram,
        }
    }

    /// The window opened or closed: read the GPU again at once, and end the NVML helper when the
    /// window is gone (the tray reads D3DKMT instead).
    fn mode_changed(&mut self, due: &mut Due, now: Instant) {
        if let GpuBackend::Adaptive { worker, up, .. } = &mut self.gpu {
            // Queued behind a pending sample, so the helper never dies mid-request.
            if *up && worker.send(WorkerCmd::Stop).is_ok() {
                *up = false;
            }
            if !self.gpu_inflight {
                due.gpu = now;
            }
        }
    }

    fn read_cpu_ram(&mut self) {
        self.system.refresh_specifics(self.refresh);
        let cpus = self.system.cpus();
        self.current.cpu = CpuMetrics {
            usage_percent: Some(self.system.global_cpu_usage()),
            per_core_percent: cpus.iter().map(|c| c.cpu_usage()).collect(),
            frequency_mhz: cpus.first().map(|c| c.frequency()).filter(|f| *f > 0),
            temperature_c: self.current.cpu.temperature_c,
        };
        let total = self.system.total_memory();
        let available = self.system.available_memory();
        self.current.ram = RamMetrics {
            used_bytes: self.system.used_memory(),
            total_bytes: total,
            available_bytes: available,
        };
    }

    fn apply_background(&mut self, result: Background, due: &mut Due, now: Instant, rates: &Rates) {
        match result {
            Background::Gpu(gpu) => {
                self.gpu_inflight = false;
                // No NVML (or the helper could not start): the kernel statistics still answer.
                let gpu = match (&mut self.gpu, gpu) {
                    (GpuBackend::Adaptive { d3d, .. }, None) => d3d.sample(),
                    (_, gpu) => gpu,
                };
                self.current.gpu = gpu;
                due.gpu = now + rates.gpu;
            }
            Background::Rtt(rtt) => {
                self.rtt_inflight = false;
                // While the connection is down, look again soon so a short outage is measured.
                due.rtt = now
                    + if rtt.is_some() {
                        rates.rtt
                    } else {
                        RTT_RETRY_WHILE_DOWN
                    };
                self.current.network.rtt_host = Some(self.rtt_host.clone());
                self.current.network.internet_reachable = Some(rtt.is_some());
                self.current.network.rtt_ms = rtt.map(|d| d.as_secs_f32() * 1000.0);
            }
            Background::CpuTemp(temp) => {
                self.cpu_temp_inflight = false;
                self.current.cpu.temperature_c = temp;
                due.cpu_temp = now
                    + if temp.is_some() {
                        CPU_TEMP_EVERY
                    } else {
                        CPU_TEMP_RETRY_MISSING
                    };
            }
            Background::Health(health) => {
                self.health_inflight = false;
                self.current.storage.health = health;
                due.health = now + HEALTH_EVERY;
            }
        }
    }
}

#[async_trait]
impl Service for SysIntel {
    fn name(&self) -> &'static str {
        "sysintel"
    }

    async fn start(&self) -> CoreResult<()> {
        self.run().await
    }

    async fn stop(&self) -> CoreResult<()> {
        self.stop.send_replace(true);
        Ok(())
    }

    fn critical(&self) -> bool {
        true
    }
}
