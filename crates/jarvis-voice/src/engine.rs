use crate::{
    child::{ChildWake, WorkerCommand},
    config::{vosk_model_dir, VoiceConfig},
    error::{VoiceError, VoiceResult},
    pipeline::{Pipeline, WakeBackend},
    source::{AudioSource, MicSource, ReadOutcome, WavSource},
    wake::WakeRecognizer,
};
use jarvis_core::{Event, MicStatus, VoiceHint};
use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

/// How long to wait for the audio device and the model when starting.
const START_TIMEOUT: Duration = Duration::from_secs(60);
const READ_TIMEOUT: Duration = Duration::from_millis(50);
/// Silence appended to a WAV file so that its last phrase can end.
pub const WAV_TAIL_SECONDS: f32 = 3.0;

/// Receives every event the engine produces.
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Ptt(bool),
    Speaking(bool),
    FinishProcessing,
    Reconfigure {
        sensitivity: f32,
        wake_names: Vec<String>,
    },
    Stop,
}

/// What a running engine does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// Permanent listening: wake word recognition (Vosk loaded) and push-to-talk.
    WakeWord,
    /// Microphone opened for one push-to-talk phrase only; no model; exits afterwards.
    PushToTalk,
}

/// A running capture thread. Dropping the handle stops the thread and frees the model.
pub struct EngineHandle {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    kind: EngineKind,
    device: Option<String>,
    level_enabled: Arc<AtomicBool>,
}

fn mic_status(device: &str, active: bool, error: Option<String>) -> Event {
    Event::VoiceStateHint(VoiceHint::Mic(MicStatus {
        active,
        device: device.into(),
        peak: None,
        no_signal: false,
        error: error.map(Into::into),
    }))
}

impl EngineHandle {
    /// Starts the capture thread and waits until the microphone (and the model) are ready.
    pub fn spawn(
        kind: EngineKind,
        config: &VoiceConfig,
        level_enabled: Arc<AtomicBool>,
        sink: EventSink,
    ) -> VoiceResult<Self> {
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<VoiceResult<()>>(1);
        let thread_config = config.clone();
        let thread_level = Arc::clone(&level_enabled);
        let thread = std::thread::Builder::new()
            .name("jarvis-voice".into())
            .spawn(move || {
                run_thread(kind, thread_config, thread_level, sink, receiver, ready_tx);
            })
            .map_err(|error| VoiceError::Device(error.to_string()))?;
        let handle = Self {
            commands,
            thread: Some(thread),
            kind,
            device: config.input_device.clone(),
            level_enabled,
        };
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(handle),
            Ok(Err(error)) => {
                handle.stop();
                Err(error)
            }
            Err(_) => {
                handle.stop();
                Err(VoiceError::Device(
                    "the voice engine did not start in time".into(),
                ))
            }
        }
    }

    pub fn kind(&self) -> EngineKind {
        self.kind
    }

    /// The device name that was requested (`None` means the system default).
    pub fn requested_device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Returns false when the thread is gone.
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    pub fn level_enabled(&self) -> &Arc<AtomicBool> {
        &self.level_enabled
    }

    /// Stops the thread and waits for it: the microphone is closed and the model freed on return.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::error!("voice thread panicked");
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_thread(
    kind: EngineKind,
    config: VoiceConfig,
    level_enabled: Arc<AtomicBool>,
    sink: EventSink,
    commands: Receiver<Command>,
    ready: mpsc::SyncSender<VoiceResult<()>>,
) {
    let setup = || -> VoiceResult<(Box<dyn AudioSource>, Pipeline)> {
        let source = MicSource::open(config.input_device.as_deref())?;
        let wake: Option<Box<dyn WakeBackend>> = match kind {
            // The model lives in a worker process that exists only while someone speaks.
            EngineKind::WakeWord => Some(Box::new(ChildWake::new(
                WorkerCommand::detect()?,
                &vosk_model_dir(),
                &config.wake_names,
            ))),
            EngineKind::PushToTalk => None,
        };
        let emit_sink = Arc::clone(&sink);
        let pipeline = Pipeline::new(
            source.sample_rate(),
            source.device_name(),
            config.sensitivity,
            config.wake_names.clone(),
            wake,
            level_enabled,
            Box::new(move |event| emit_sink(event)),
        )?;
        Ok((Box::new(source), pipeline))
    };
    let (mut source, mut pipeline) = match setup() {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(VoiceError::Device(error.to_string())));
            return;
        }
    };
    let device = source.device_name().to_owned();
    tracing::info!(%device, rate = source.sample_rate(), ?kind, "voice capture started");
    sink(mic_status(&device, true, None));
    let _ = ready.send(Ok(()));
    let outcome = capture_loop(kind, &mut *source, &mut pipeline, &commands);
    // Free the microphone and the model before reporting.
    drop(source);
    drop(pipeline);
    let error = outcome.err().map(|error| error.to_string());
    if let Some(message) = &error {
        tracing::error!(%device, error = %message, "voice capture failed");
    }
    sink(Event::VoiceStateHint(VoiceHint::State("IDLE".into())));
    sink(mic_status(&device, false, error));
    tracing::info!(%device, "voice capture stopped");
}

fn capture_loop(
    kind: EngineKind,
    source: &mut dyn AudioSource,
    pipeline: &mut Pipeline,
    commands: &Receiver<Command>,
) -> VoiceResult<()> {
    let mut buffer: Vec<f32> = Vec::new();
    let mut ptt_started = false;
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Stop => return Ok(()),
                Command::Ptt(pressed) => {
                    pipeline.set_ptt(pressed);
                    ptt_started |= pressed;
                }
                Command::Speaking(speaking) => pipeline.set_speaking(speaking),
                Command::FinishProcessing => pipeline.finish_processing(),
                Command::Reconfigure {
                    sensitivity,
                    wake_names,
                } => pipeline.reconfigure(sensitivity, &wake_names)?,
            }
        }
        if kind == EngineKind::PushToTalk && ptt_started && pipeline.state() != "LISTENING" {
            return Ok(());
        }
        buffer.clear();
        match source.read(&mut buffer, READ_TIMEOUT)? {
            ReadOutcome::Data => pipeline.push(&buffer)?,
            ReadOutcome::Timeout => {}
            ReadOutcome::Ended => return Ok(()),
        }
    }
}

/// How `process_wav` runs the wake word recognizer.
#[derive(Debug, Clone, PartialEq)]
pub enum WakeMode {
    /// No recognizer (denoise, VAD and segmentation only).
    Off,
    /// Vosk inside this process.
    InProcess,
    /// Vosk in a worker process, as in the app.
    Worker(WorkerCommand),
}

/// Runs a WAV file through the whole pipeline and returns the
/// events. The file is processed as fast as it can be read, with `WAV_TAIL_SECONDS` of silence appended.
pub fn process_wav(
    path: &Path,
    config: &VoiceConfig,
    model_dir: &Path,
    wake: WakeMode,
) -> VoiceResult<Vec<Event>> {
    process_wav_with(path, config, model_dir, wake, false, Box::new(|_| {}))
}

/// Like `process_wav`, but `on_event` sees every event the moment it happens, and `realtime` plays the
/// file at its natural speed (so that work started by an event overlaps the rest of the audio).
pub fn process_wav_with(
    path: &Path,
    config: &VoiceConfig,
    model_dir: &Path,
    wake: WakeMode,
    realtime: bool,
    mut on_event: Box<dyn FnMut(&Event)>,
) -> VoiceResult<Vec<Event>> {
    let mut source = WavSource::open(path, WAV_TAIL_SECONDS)?.realtime(realtime);
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = Rc::clone(&events);
    let backend: Option<Box<dyn WakeBackend>> = match wake {
        WakeMode::Off => None,
        WakeMode::InProcess => Some(Box::new(WakeRecognizer::load(
            model_dir,
            &config.wake_names,
        )?)),
        WakeMode::Worker(command) => Some(Box::new(ChildWake::new(
            command,
            model_dir,
            &config.wake_names,
        ))),
    };
    let mut pipeline = Pipeline::new(
        source.sample_rate(),
        source.device_name(),
        config.sensitivity,
        config.wake_names.clone(),
        backend,
        Arc::new(AtomicBool::new(false)),
        Box::new(move |event| {
            on_event(&event);
            sink.borrow_mut().push(event);
        }),
    )?;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match source.read(&mut buffer, READ_TIMEOUT)? {
            ReadOutcome::Data => pipeline.push(&buffer)?,
            ReadOutcome::Timeout => {}
            ReadOutcome::Ended => break,
        }
    }
    drop(pipeline);
    let result = events.borrow().clone();
    Ok(result)
}

impl EngineHandle {
    /// For tests and tools: the flag the engine reads before sending level events.
    pub fn set_level_enabled(&self, enabled: bool) {
        self.level_enabled.store(enabled, Ordering::Relaxed);
    }
}
