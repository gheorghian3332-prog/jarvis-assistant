//! Playback: a thread-safe sample queue that the audio device (or a WAV file) drains, the RMS level
//! meter over what is actually played, and immediate interruption.
//!
//! The device callback and the WAV writer both call [`PlayQueue::pull`], so the level envelope,
//! the volume and the interruption behave the same for both outputs.

use crate::{
    clip::{resample, rms_f32, write_wav},
    error::{TtsError, TtsResult},
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tracing::{info, warn};

/// The level envelope is sent this many times per second (one RMS value per window).
pub const LEVEL_HZ: u32 = 30;
/// Fade-out applied when playback is interrupted (avoids a click).
pub const FADE_OUT_MS: u32 = 10;
/// Sample rate of the WAV sink.
pub const WAV_SAMPLE_RATE: u32 = 24_000;

/// Receives one RMS value (0..1, after the volume) per 1/30 s of played audio.
pub type LevelCallback = Arc<dyn Fn(f32) + Send + Sync>;

/// Where speech goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSink {
    /// The named output device, or the system default when `None` (or not found).
    Device(Option<String>),
    /// A WAV file written in real time (for tests and measurements; no sound is played).
    Wav { path: PathBuf },
}

struct Inner {
    buffer: VecDeque<f32>,
    window: usize,
    sum_squares: f64,
    in_window: usize,
    /// Set by the first `push`; the meter is idle before that.
    started: bool,
    /// No more audio will be pushed.
    feeding_done: bool,
    /// Samples left of the fade-out after `stop`.
    fade_left: Option<usize>,
    fade_total: usize,
    stopped: bool,
    /// The final zero level was already reported.
    closed: bool,
    played: u64,
}

/// Mono samples at the output sample rate, drained by the output.
pub struct PlayQueue {
    rate: u32,
    gain: AtomicU32,
    inner: Mutex<Inner>,
    level: Option<LevelCallback>,
    changed: Notify,
}

impl PlayQueue {
    pub fn new(rate: u32, gain: f32, level: Option<LevelCallback>) -> Arc<Self> {
        let window = (rate / LEVEL_HZ).max(1) as usize;
        Arc::new(Self {
            rate,
            gain: AtomicU32::new(gain.clamp(0.0, 1.0).to_bits()),
            inner: Mutex::new(Inner {
                buffer: VecDeque::new(),
                window,
                sum_squares: 0.0,
                in_window: 0,
                started: false,
                feeding_done: false,
                fade_left: None,
                fade_total: ((rate * FADE_OUT_MS) / 1000).max(1) as usize,
                stopped: false,
                closed: false,
                played: 0,
            }),
            level,
            changed: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Changes the volume (0..1) for everything not yet played.
    pub fn set_gain(&self, gain: f32) {
        self.gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    /// Queues samples (already at the output rate). Ignored after `stop`.
    pub fn push(&self, samples: Vec<f32>) {
        let mut inner = self.lock();
        if inner.stopped || inner.fade_left.is_some() || inner.feeding_done {
            return;
        }
        inner.started = true;
        inner.buffer.extend(samples);
    }

    /// No more audio will come: playback ends when the queue is empty.
    pub fn finish(&self) {
        self.lock().feeding_done = true;
        self.changed.notify_waiters();
    }

    /// Interrupts playback: a 10 ms fade-out, then silence; the queue is cleared.
    pub fn stop(&self) {
        let mut inner = self.lock();
        inner.feeding_done = true;
        if inner.stopped {
            return;
        }
        // The envelope goes quiet at once, even if the output stops pulling (device closed).
        let report_zero = inner.started && !inner.closed;
        inner.closed = true;
        if inner.buffer.is_empty() {
            inner.stopped = true;
        } else {
            let fade = inner.fade_total.min(inner.buffer.len());
            // Nothing beyond the fade is played.
            inner.buffer.truncate(fade);
            inner.fade_left = Some(fade);
        }
        drop(inner);
        if report_zero {
            self.report(&[0.0]);
        }
        self.changed.notify_waiters();
    }

    /// Samples waiting to be played.
    pub fn pending(&self) -> usize {
        self.lock().buffer.len()
    }

    /// Seconds of audio waiting to be played.
    pub fn pending_secs(&self) -> f32 {
        self.pending() as f32 / self.rate as f32
    }

    /// True once the first audio has been queued.
    pub fn has_started(&self) -> bool {
        self.lock().started
    }

    /// Samples delivered to the output so far.
    pub fn played(&self) -> u64 {
        self.lock().played
    }

    /// True when playback is over: interrupted, or finished and drained.
    pub fn is_finished(&self) -> bool {
        let inner = self.lock();
        inner.stopped || (inner.feeding_done && inner.buffer.is_empty())
    }

    /// Waits until playback is over (see [`PlayQueue::is_finished`]).
    pub async fn wait_finished(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            // Registered before the check, so a wake-up in between is not lost.
            notified.as_mut().enable();
            if self.is_finished() {
                return;
            }
            notified.await;
        }
    }

    /// Waits until at most `seconds` of audio are left in the queue (or playback is over).
    pub async fn wait_pending_below(&self, seconds: f32) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_finished() || self.pending_secs() <= seconds {
                return;
            }
            notified.await;
        }
    }

    /// Fills `out` (mono) with the next samples at the current volume; the rest is silence.
    /// Returns how many real samples were delivered. Called by the output.
    pub fn pull(&self, out: &mut [f32]) -> usize {
        let gain = self.gain();
        let mut levels: Vec<f32> = Vec::new();
        let delivered;
        {
            let mut inner = self.lock();
            if inner.stopped {
                out.fill(0.0);
                if !inner.closed {
                    inner.closed = true;
                    levels.push(0.0);
                }
                drop(inner);
                self.report(&levels);
                self.changed.notify_waiters();
                return 0;
            }
            let mut count = 0usize;
            for slot in out.iter_mut() {
                let sample = match inner.buffer.pop_front() {
                    Some(sample) => {
                        count += 1;
                        let mut value = sample * gain;
                        if let Some(left) = inner.fade_left {
                            value *= left as f32 / inner.fade_total as f32;
                            if left <= 1 {
                                inner.fade_left = None;
                                inner.buffer.clear();
                                inner.stopped = true;
                            } else {
                                inner.fade_left = Some(left - 1);
                            }
                        }
                        value
                    }
                    None => 0.0,
                };
                *slot = sample;
                if inner.started && !inner.closed {
                    inner.sum_squares += f64::from(sample) * f64::from(sample);
                    inner.in_window += 1;
                    if inner.in_window >= inner.window {
                        levels.push((inner.sum_squares / inner.in_window as f64).sqrt() as f32);
                        inner.sum_squares = 0.0;
                        inner.in_window = 0;
                    }
                }
            }
            inner.played += count as u64;
            if (inner.stopped || (inner.feeding_done && inner.buffer.is_empty())) && !inner.closed {
                // The last partial window and a final zero: the core goes quiet.
                if inner.in_window > 0 {
                    levels.push((inner.sum_squares / inner.in_window as f64).sqrt() as f32);
                }
                levels.push(0.0);
                inner.sum_squares = 0.0;
                inner.in_window = 0;
                inner.closed = true;
            }
            delivered = count;
        }
        self.report(&levels);
        self.changed.notify_waiters();
        delivered
    }

    fn report(&self, levels: &[f32]) {
        if let Some(callback) = &self.level {
            for level in levels {
                callback(*level);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sinks
// ---------------------------------------------------------------------------------------------

/// A running output. Dropping it stops the output thread (and writes the WAV file).
pub struct SinkHandle {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    description: String,
}

impl SinkHandle {
    /// What the sink writes to (device name or file path), for logs.
    pub fn description(&self) -> &str {
        &self.description
    }
}

impl Drop for SinkHandle {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One output device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDevice {
    pub name: String,
    pub is_default: bool,
}

fn device_name(device: &cpal::Device) -> String {
    device.description().map_or_else(
        |_| device.to_string(),
        |description| description.name().to_owned(),
    )
}

fn playback_error<E: std::fmt::Display>(error: E) -> TtsError {
    TtsError::Playback(error.to_string())
}

/// Output devices of the default audio host.
pub fn list_output_devices() -> TtsResult<Vec<OutputDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().map(|d| device_name(&d));
    let mut devices = Vec::new();
    for device in host.output_devices().map_err(playback_error)? {
        let name = device_name(&device);
        devices.push(OutputDevice {
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
        });
    }
    Ok(devices)
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    queue: Arc<PlayQueue>,
    failed: Arc<AtomicBool>,
) -> TtsResult<cpal::Stream>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let channels = usize::from(config.channels).max(1);
    let mut mono: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels;
                mono.resize(frames, 0.0);
                queue.pull(&mut mono);
                for (frame, value) in data.chunks_mut(channels).zip(mono.iter()) {
                    let sample = T::from_sample(value.clamp(-1.0, 1.0));
                    for slot in frame {
                        *slot = sample;
                    }
                }
            },
            move |error| {
                warn!(%error, "audio output error");
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(playback_error)
}

type DeviceSetup = (Arc<PlayQueue>, String);

fn open_device_stream(
    name: Option<&str>,
    gain: f32,
    level: Option<LevelCallback>,
    failed: Arc<AtomicBool>,
) -> TtsResult<(cpal::Stream, DeviceSetup)> {
    let host = cpal::default_host();
    let mut chosen = None;
    if let Some(wanted) = name {
        for device in host.output_devices().map_err(playback_error)? {
            if device_name(&device) == wanted {
                chosen = Some(device);
                break;
            }
        }
        if chosen.is_none() {
            warn!(
                device = wanted,
                "output device not found; using the default"
            );
        }
    }
    let device = match chosen {
        Some(device) => device,
        None => host
            .default_output_device()
            .ok_or_else(|| TtsError::Playback("no output device".into()))?,
    };
    let label = device_name(&device);
    let supported = device.default_output_config().map_err(playback_error)?;
    let format = supported.sample_format();
    let config = supported.config();
    let queue = PlayQueue::new(config.sample_rate, gain, level);
    let stream = match format {
        SampleFormat::F32 => build_stream::<f32>(&device, config, Arc::clone(&queue), failed)?,
        SampleFormat::I16 => build_stream::<i16>(&device, config, Arc::clone(&queue), failed)?,
        SampleFormat::I32 => build_stream::<i32>(&device, config, Arc::clone(&queue), failed)?,
        SampleFormat::U16 => build_stream::<u16>(&device, config, Arc::clone(&queue), failed)?,
        other => {
            return Err(TtsError::Playback(format!(
                "unsupported sample format {other:?}"
            )))
        }
    };
    stream.play().map_err(playback_error)?;
    Ok((stream, (queue, label)))
}

fn spawn_device_sink(
    name: Option<String>,
    gain: f32,
    level: Option<LevelCallback>,
) -> TtsResult<(Arc<PlayQueue>, SinkHandle)> {
    let (ready_tx, ready_rx) = mpsc::channel::<TtsResult<DeviceSetup>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = thread::Builder::new()
        .name("jarvis-tts-device".into())
        .spawn(move || {
            // The stream is not `Send`: it lives and dies on this thread.
            let failed = Arc::new(AtomicBool::new(false));
            let stream = match open_device_stream(name.as_deref(), gain, level, Arc::clone(&failed))
            {
                Ok((stream, setup)) => {
                    let queue = Arc::clone(&setup.0);
                    let _ = ready_tx.send(Ok(setup));
                    Some((stream, queue))
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    None
                }
            };
            let Some((stream, queue)) = stream else {
                return;
            };
            while let Err(mpsc::RecvTimeoutError::Timeout) =
                stop_rx.recv_timeout(Duration::from_millis(100))
            {
                if failed.load(Ordering::Relaxed) {
                    // The device went away: end playback instead of waiting forever.
                    queue.stop();
                    break;
                }
            }
            drop(stream);
        })
        .map_err(playback_error)?;
    let (queue, label) = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| TtsError::Playback("audio device did not open in time".into()))??;
    info!(device = %label, rate = queue.sample_rate(), "audio output opened");
    Ok((
        queue,
        SinkHandle {
            stop: Some(stop_tx),
            thread: Some(thread),
            description: label,
        },
    ))
}

fn spawn_wav_sink(
    path: PathBuf,
    gain: f32,
    level: Option<LevelCallback>,
) -> TtsResult<(Arc<PlayQueue>, SinkHandle)> {
    let queue = PlayQueue::new(WAV_SAMPLE_RATE, gain, level);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let worker_queue = Arc::clone(&queue);
    let target = path.clone();
    let thread = thread::Builder::new()
        .name("jarvis-tts-wav".into())
        .spawn(move || {
            let window = (WAV_SAMPLE_RATE / LEVEL_HZ) as usize;
            let tick = Duration::from_secs_f64(window as f64 / f64::from(WAV_SAMPLE_RATE));
            let mut recorded: Vec<i16> = Vec::new();
            let mut chunk = vec![0.0f32; window];
            let mut next = Instant::now();
            loop {
                let started = worker_queue.has_started();
                worker_queue.pull(&mut chunk);
                if started {
                    recorded.extend(
                        chunk
                            .iter()
                            .map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16),
                    );
                }
                if worker_queue.is_finished() {
                    break;
                }
                next += tick;
                let wait = next.saturating_duration_since(Instant::now());
                match stop_rx.recv_timeout(wait) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => {
                        worker_queue.stop();
                        // Let the fade-out run to its end.
                        while !worker_queue.is_finished() {
                            worker_queue.pull(&mut chunk);
                            recorded.extend(
                                chunk
                                    .iter()
                                    .map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16),
                            );
                        }
                        break;
                    }
                }
            }
            if let Err(error) = write_wav(&target, &recorded, WAV_SAMPLE_RATE) {
                warn!(%error, "could not write the WAV output");
            }
        })
        .map_err(playback_error)?;
    Ok((
        queue,
        SinkHandle {
            stop: Some(stop_tx),
            thread: Some(thread),
            description: path.display().to_string(),
        },
    ))
}

/// Opens the sink and returns the queue that feeds it (at the sink's sample rate).
pub fn open_sink(
    sink: &OutputSink,
    gain: f32,
    level: Option<LevelCallback>,
) -> TtsResult<(Arc<PlayQueue>, SinkHandle)> {
    match sink {
        OutputSink::Device(name) => spawn_device_sink(name.clone(), gain, level),
        OutputSink::Wav { path } => spawn_wav_sink(path.clone(), gain, level),
    }
}

/// Converts a clip to the queue's rate and queues it.
pub fn enqueue_clip(queue: &PlayQueue, clip: &crate::clip::AudioClip) {
    let samples = resample(&clip.to_f32(), clip.sample_rate, queue.sample_rate());
    queue.push(samples);
}

/// RMS of the last `window` of a mono signal; helper for tests and measurements.
pub fn level_of(samples: &[f32]) -> f32 {
    rms_f32(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collector() -> (LevelCallback, Arc<Mutex<Vec<f32>>>) {
        let levels = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&levels);
        (
            Arc::new(move |level| {
                if let Ok(mut levels) = sink.lock() {
                    levels.push(level);
                }
            }),
            levels,
        )
    }

    #[test]
    fn level_windows_report_the_rms_of_played_audio() {
        let (callback, levels) = collector();
        let queue = PlayQueue::new(3_000, 1.0, Some(callback));
        // 100 samples per window at 3 kHz / 30 Hz.
        queue.push(vec![0.5; 200]);
        queue.push(vec![-0.25; 100]);
        let mut out = vec![0.0; 400];
        queue.pull(&mut out);
        let levels = levels.lock().expect("levels").clone();
        assert!(levels.len() >= 3, "{levels:?}");
        assert!((levels[0] - 0.5).abs() < 1e-6);
        assert!((levels[1] - 0.5).abs() < 1e-6);
        assert!((levels[2] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_meter_sees_the_volume_not_the_source() {
        let (callback, levels) = collector();
        let queue = PlayQueue::new(3_000, 0.5, Some(callback));
        queue.push(vec![0.8; 100]);
        let mut out = vec![0.0; 100];
        queue.pull(&mut out);
        assert!((out[0] - 0.4).abs() < 1e-6);
        assert!((levels.lock().expect("levels")[0] - 0.4).abs() < 1e-6);
        queue.set_gain(0.0);
        queue.push(vec![0.8; 100]);
        queue.pull(&mut out);
        assert!(out.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn no_levels_before_audio_and_a_final_zero_after_it() {
        let (callback, levels) = collector();
        let queue = PlayQueue::new(3_000, 1.0, Some(callback));
        let mut out = vec![0.0; 300];
        queue.pull(&mut out);
        assert!(levels.lock().expect("levels").is_empty());
        queue.push(vec![0.5; 50]);
        queue.finish();
        queue.pull(&mut out);
        assert!(queue.is_finished());
        let levels = levels.lock().expect("levels").clone();
        assert_eq!(levels.last(), Some(&0.0));
        let before = levels.len();
        queue.pull(&mut out);
        assert_eq!(levels.len(), before, "closed: nothing more is reported");
    }

    #[test]
    fn stop_fades_out_clears_the_queue_and_ends_playback() {
        let (callback, levels) = collector();
        let queue = PlayQueue::new(10_000, 1.0, Some(callback));
        queue.push(vec![1.0; 50_000]);
        let mut out = vec![0.0; 50];
        queue.pull(&mut out);
        assert_eq!(out[0], 1.0);
        queue.stop();
        assert!(queue.pending() <= 100, "10 ms of fade at 10 kHz");
        let mut tail = vec![0.0; 400];
        let delivered = queue.pull(&mut tail);
        assert_eq!(delivered, 100);
        assert!(tail[0] > tail[50] && tail[50] > tail[99], "fading out");
        assert!(tail[100..].iter().all(|v| *v == 0.0));
        assert!(queue.is_finished());
        assert_eq!(queue.pending(), 0);
        queue.push(vec![1.0; 10]);
        assert_eq!(queue.pending(), 0, "no audio is accepted after stop");
        assert_eq!(levels.lock().expect("levels").last(), Some(&0.0));
    }

    #[test]
    fn stopping_an_empty_queue_finishes_immediately() {
        let queue = PlayQueue::new(1_000, 1.0, None);
        assert!(!queue.is_finished());
        queue.stop();
        assert!(queue.is_finished());
    }

    #[tokio::test]
    async fn waiters_wake_up_when_audio_drains_or_playback_stops() {
        let queue = PlayQueue::new(1_000, 1.0, None);
        queue.push(vec![0.1; 1_000]);
        let waiter = {
            let queue = Arc::clone(&queue);
            tokio::spawn(async move { queue.wait_pending_below(0.2).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        let mut out = vec![0.0; 900];
        queue.pull(&mut out);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("woken")
            .expect("join");
        let finished = {
            let queue = Arc::clone(&queue);
            tokio::spawn(async move { queue.wait_finished().await })
        };
        queue.stop();
        // The output drains the fade-out.
        queue.pull(&mut out);
        tokio::time::timeout(Duration::from_secs(1), finished)
            .await
            .expect("woken")
            .expect("join");
    }

    #[test]
    fn wav_sink_plays_in_real_time_and_writes_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.wav");
        let (callback, levels) = collector();
        let (queue, sink) =
            open_sink(&OutputSink::Wav { path: path.clone() }, 1.0, Some(callback)).expect("sink");
        assert_eq!(queue.sample_rate(), WAV_SAMPLE_RATE);
        let started = Instant::now();
        queue.push(vec![0.3; (WAV_SAMPLE_RATE / 2) as usize]);
        queue.finish();
        while !queue.is_finished() {
            thread::sleep(Duration::from_millis(10));
        }
        drop(sink);
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(450) && elapsed < Duration::from_millis(1_500),
            "paced like real time: {elapsed:?}"
        );
        let clip = crate::clip::AudioClip::read_wav(&path).expect("wav");
        assert_eq!(clip.sample_rate, WAV_SAMPLE_RATE);
        assert!(clip.duration_secs() >= 0.5);
        let levels = levels.lock().expect("levels").clone();
        assert!(
            levels.len() >= 14,
            "about 30 Hz for half a second: {}",
            levels.len()
        );
        assert!((levels[0] - 0.3).abs() < 0.01);
    }

    #[test]
    fn wav_sink_stops_within_100_ms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.wav");
        let (queue, sink) =
            open_sink(&OutputSink::Wav { path: path.clone() }, 1.0, None).expect("sink");
        queue.push(vec![0.3; (WAV_SAMPLE_RATE * 10) as usize]);
        thread::sleep(Duration::from_millis(300));
        let stopped_at = Instant::now();
        queue.stop();
        while !queue.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        let latency = stopped_at.elapsed();
        drop(sink);
        assert!(latency <= Duration::from_millis(100), "{latency:?}");
        let clip = crate::clip::AudioClip::read_wav(&path).expect("wav");
        assert!(
            clip.duration_secs() < 1.0,
            "only what played: {}",
            clip.duration_secs()
        );
        assert!(clip.duration_secs() > 0.2);
    }

    #[test]
    fn enqueue_converts_the_sample_rate() {
        let queue = PlayQueue::new(48_000, 1.0, None);
        let clip = crate::clip::AudioClip::new(vec![1000; 24_000], 24_000);
        enqueue_clip(&queue, &clip);
        assert!((queue.pending() as i64 - 48_000).abs() <= 2);
        assert!(level_of(&[0.5, -0.5]) > 0.49);
    }
}
