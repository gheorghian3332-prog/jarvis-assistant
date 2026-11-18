//! Management of on-demand Python workers.
//!
//! A worker is a child process that speaks the binary frame protocol from
//! `API_CONTRACTS.md` on stdin/stdout (stderr carries its logs). [`WorkerHandle`] owns one
//! such process; [`SttService`] starts the speech-to-text worker lazily on the first request
//! and shuts it down after an idle timeout.

use crate::service::{CoreError, CoreResult, Service};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, Mutex},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};
use tracing::{debug, info, warn};

/// Frame types of the core <-> worker protocol.
pub mod frame_type {
    pub const HELLO: u8 = 0x01;
    pub const PING: u8 = 0x02;
    pub const PONG: u8 = 0x03;
    pub const AUDIO: u8 = 0x10;
    pub const END_UTTERANCE: u8 = 0x11;
    pub const TRANSCRIPT: u8 = 0x20;
    pub const TTS_REQ: u8 = 0x30;
    pub const TTS_PCM: u8 = 0x31;
    pub const ERROR: u8 = 0x7E;
    pub const SHUTDOWN: u8 = 0x7F;
}

/// Upper bound for one frame (type byte + payload); matches the Python worker.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;
/// Audio is sent to the worker in frames of at most this many bytes (0.5 s of PCM16 at 16 kHz).
pub const AUDIO_CHUNK_BYTES: usize = 16_000;
/// Longest utterance the STT worker accepts (120 s of PCM16 at 16 kHz).
pub const MAX_UTTERANCE_BYTES: usize = 16_000 * 2 * 120;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STDERR_TAIL_LINES: usize = 12;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("stream ended in the middle of a frame")]
    Truncated,
    #[error("frame length must be at least 1")]
    InvalidLength,
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN} byte limit")]
    TooLarge(usize),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

/// Encodes `u32 LE len | u8 type | payload`, where `len` counts the type byte and the payload.
pub fn encode_frame(kind: u8, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    let len = payload.len() + 1;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut out = Vec::with_capacity(4 + len);
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Reads one frame. `Ok(None)` is a clean end of stream at a frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Frame>, FrameError> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < prefix.len() {
        let read = reader.read(&mut prefix[filled..]).await?;
        if read == 0 {
            return if filled == 0 {
                Ok(None)
            } else {
                Err(FrameError::Truncated)
            };
        }
        filled += read;
    }
    let len = u32::from_le_bytes(prefix) as usize;
    if len == 0 {
        return Err(FrameError::InvalidLength);
    }
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut kind = [0u8; 1];
    let mut payload = vec![0u8; len - 1];
    for buffer in [&mut kind[..], &mut payload[..]] {
        match reader.read_exact(buffer).await {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(FrameError::Truncated)
            }
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    Ok(Some(Frame {
        kind: kind[0],
        payload,
    }))
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("failed to start worker process: {0}")]
    Spawn(io::Error),
    #[error("worker did not send HELLO within {0:?}")]
    HelloTimeout(Duration),
    #[error("worker failed to start: {0}")]
    Startup(String),
    #[error("worker exited: {0}")]
    Exited(String),
    #[error("worker request timed out after {0:?}")]
    Timeout(Duration),
    #[error("worker reported {code}: {message}")]
    Reported { code: String, message: String },
    #[error("worker protocol violation: {0}")]
    Protocol(String),
    #[error("invalid request: {0}")]
    InvalidInput(String),
    #[error("I/O error talking to worker: {0}")]
    Io(#[from] io::Error),
}

impl From<FrameError> for WorkerError {
    fn from(error: FrameError) -> Self {
        match error {
            FrameError::Io(error) => Self::Io(error),
            other => Self::Protocol(other.to_string()),
        }
    }
}

impl From<WorkerError> for CoreError {
    fn from(error: WorkerError) -> Self {
        CoreError::Service(error.to_string())
    }
}

/// How to launch a worker process.
#[derive(Debug, Clone)]
pub struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    pub envs: Vec<(OsString, OsString)>,
}

impl WorkerCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            envs: Vec::new(),
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    /// Development launch of the STT worker:
    /// `uv run --project workers/stt python -m jarvis_stt` from the repository root.
    pub fn dev_stt(repo_root: &Path) -> Self {
        Self::new("uv")
            .args([
                OsString::from("run"),
                OsString::from("--project"),
                repo_root.join("workers").join("stt").into_os_string(),
                OsString::from("python"),
                OsString::from("-m"),
                OsString::from("jarvis_stt"),
            ])
            .current_dir(repo_root)
    }

    /// Development launch of the Kokoro TTS worker:
    /// `uv run --project workers/tts_kokoro python -m jarvis_tts_kokoro` from the repository root.
    pub fn dev_kokoro(repo_root: &Path) -> Self {
        Self::new("uv")
            .args([
                OsString::from("run"),
                OsString::from("--project"),
                repo_root
                    .join("workers")
                    .join("tts_kokoro")
                    .into_os_string(),
                OsString::from("python"),
                OsString::from("-m"),
                OsString::from("jarvis_tts_kokoro"),
            ])
            .current_dir(repo_root)
    }

    /// Direct launch of a Python interpreter (for example the project's venv) without `uv`.
    pub fn python_module(python: impl Into<PathBuf>, module: &str, cwd: Option<PathBuf>) -> Self {
        let mut command = Self::new(python).args(["-m", module]);
        command.current_dir = cwd;
        command
    }
}

/// Content of the worker's HELLO frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub worker: String,
    pub version: String,
    pub device: String,
    pub model: Option<String>,
}

impl Hello {
    fn parse(payload: &[u8]) -> Result<Self, WorkerError> {
        let value: Value = serde_json::from_slice(payload)
            .map_err(|error| WorkerError::Protocol(format!("invalid HELLO: {error}")))?;
        let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        let required = |name: &str| {
            field(name).ok_or_else(|| WorkerError::Protocol(format!("HELLO lacks `{name}`")))
        };
        Ok(Self {
            worker: required("worker")?,
            version: required("version")?,
            device: required("device")?,
            model: field("model"),
        })
    }
}

#[derive(Debug)]
enum Event {
    Frame(Frame),
    Closed(Option<String>),
}

/// Result of stopping a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The worker exited by itself within the grace period.
    Graceful,
    /// The grace period expired and the process was killed.
    Killed,
}

/// One running worker process.
pub struct WorkerHandle {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::Receiver<Event>,
    hello: Hello,
    pid: u32,
    stderr_tail: Arc<StdMutex<VecDeque<String>>>,
    tasks: Vec<JoinHandle<()>>,
    // Closing the job kills the worker and any process it started (for example `uv` -> python).
    _job: Option<job::JobObject>,
}

impl WorkerHandle {
    /// Starts the process and waits for HELLO. The process is killed if that fails.
    pub async fn spawn(
        command: &WorkerCommand,
        hello_timeout: Duration,
    ) -> Result<Self, WorkerError> {
        let mut process = Command::new(&command.program);
        process
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        if let Some(dir) = &command.current_dir {
            process.current_dir(dir);
        }
        for (key, value) in &command.envs {
            process.env(key, value);
        }
        let mut child = process.spawn().map_err(WorkerError::Spawn)?;
        let pid = child.id().unwrap_or(0);
        let job = job::JobObject::adopt(&child);
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill().await;
            return Err(WorkerError::Protocol("worker pipes are unavailable".into()));
        };

        let stderr_tail = Arc::new(StdMutex::new(VecDeque::new()));
        let (sender, events) = mpsc::channel(64);
        let tasks = vec![
            tokio::spawn(read_stdout(stdout, sender)),
            tokio::spawn(read_stderr(stderr, Arc::clone(&stderr_tail), pid)),
        ];
        let mut partial = Self {
            child,
            stdin: Some(stdin),
            events,
            hello: Hello {
                worker: String::new(),
                version: String::new(),
                device: String::new(),
                model: None,
            },
            pid,
            stderr_tail,
            tasks,
            _job: job,
        };
        match timeout(hello_timeout, partial.wait_hello()).await {
            Ok(Ok(hello)) => {
                partial.hello = hello;
                info!(pid, device = %partial.hello.device, "worker is ready");
                Ok(partial)
            }
            Ok(Err(error)) => {
                partial.kill().await;
                Err(error)
            }
            Err(_) => {
                partial.kill().await;
                Err(WorkerError::HelloTimeout(hello_timeout))
            }
        }
    }

    async fn wait_hello(&mut self) -> Result<Hello, WorkerError> {
        loop {
            match self.events.recv().await {
                Some(Event::Frame(frame)) => match frame.kind {
                    frame_type::HELLO => return Hello::parse(&frame.payload),
                    frame_type::ERROR => {
                        let (code, message) = parse_error(&frame.payload);
                        return Err(WorkerError::Startup(format!("{code}: {message}")));
                    }
                    other => debug!(kind = other, "ignoring frame before HELLO"),
                },
                Some(Event::Closed(reason)) => {
                    return Err(WorkerError::Startup(self.exit_description(reason).await));
                }
                None => {
                    return Err(WorkerError::Startup(self.exit_description(None).await));
                }
            }
        }
    }

    async fn exit_description(&mut self, reason: Option<String>) -> String {
        let status = match timeout(Duration::from_millis(500), self.child.wait()).await {
            Ok(Ok(status)) => format!("exit status {status}"),
            Ok(Err(error)) => format!("exit status unknown ({error})"),
            Err(_) => "stdout closed".to_owned(),
        };
        let tail = self.stderr_tail();
        let mut text = status;
        if let Some(reason) = reason {
            text.push_str(&format!("; {reason}"));
        }
        if !tail.is_empty() {
            text.push_str(&format!("; stderr: {tail}"));
        }
        text
    }

    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Last lines the worker wrote to stderr, joined with " | ".
    pub fn stderr_tail(&self) -> String {
        match self.stderr_tail.lock() {
            Ok(lines) => lines.iter().cloned().collect::<Vec<_>>().join(" | "),
            Err(_) => String::new(),
        }
    }

    /// True while the process has not exited.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub async fn send(&mut self, kind: u8, payload: &[u8]) -> Result<(), WorkerError> {
        let frame = encode_frame(kind, payload)?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| WorkerError::Exited("stdin is closed".into()))?;
        stdin.write_all(&frame).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Next frame from the worker, or an error if it exits first.
    pub async fn recv(&mut self) -> Result<Frame, WorkerError> {
        match self.events.recv().await {
            Some(Event::Frame(frame)) => Ok(frame),
            Some(Event::Closed(reason)) => {
                Err(WorkerError::Exited(self.exit_description(reason).await))
            }
            None => Err(WorkerError::Exited(self.exit_description(None).await)),
        }
    }

    /// Drops frames that arrived while no request was running (late replies, stray PONG).
    pub fn discard_pending(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            if let Event::Frame(frame) = event {
                debug!(kind = frame.kind, "discarding stale worker frame");
            }
        }
    }

    /// Health check: PING and wait for PONG.
    pub async fn ping(&mut self, limit: Duration) -> Result<(), WorkerError> {
        self.discard_pending();
        let result = timeout(limit, async {
            self.send(frame_type::PING, b"jarvis").await?;
            loop {
                let frame = self.recv().await?;
                match frame.kind {
                    frame_type::PONG => return Ok(()),
                    frame_type::ERROR => {
                        let (code, message) = parse_error(&frame.payload);
                        return Err(WorkerError::Reported { code, message });
                    }
                    _ => {}
                }
            }
        })
        .await;
        result.unwrap_or(Err(WorkerError::Timeout(limit)))
    }

    /// Asks the worker to exit, waits `grace`, then kills it.
    pub async fn shutdown(mut self, grace: Duration) -> ShutdownOutcome {
        let _ = timeout(grace, self.send(frame_type::SHUTDOWN, &[])).await;
        self.stdin = None;
        let outcome = match timeout(grace, self.child.wait()).await {
            Ok(Ok(_)) => ShutdownOutcome::Graceful,
            _ => {
                warn!(pid = self.pid, "worker ignored SHUTDOWN; killing");
                let _ = self.child.kill().await;
                ShutdownOutcome::Killed
            }
        };
        self.finish();
        outcome
    }

    /// Kills the process immediately.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
        self.finish();
    }

    fn finish(&mut self) {
        self.stdin = None;
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self._job = None;
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

fn parse_error(payload: &[u8]) -> (String, String) {
    let value: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
    let text = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    (text("code"), text("message"))
}

async fn read_stdout(stdout: tokio::process::ChildStdout, sender: mpsc::Sender<Event>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let event = match read_frame(&mut reader).await {
            Ok(Some(frame)) => Event::Frame(frame),
            Ok(None) => Event::Closed(None),
            Err(error) => Event::Closed(Some(error.to_string())),
        };
        let closed = matches!(event, Event::Closed(_));
        if sender.send(event).await.is_err() || closed {
            return;
        }
    }
}

async fn read_stderr(
    stderr: tokio::process::ChildStderr,
    tail: Arc<StdMutex<VecDeque<String>>>,
    pid: u32,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        debug!(pid, "worker: {line}");
        if let Ok(mut tail) = tail.lock() {
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
}

pub mod job {
    //! Windows job object that kills the worker (and its children) when it is closed.
    use tokio::process::Child;
    use tracing::warn;
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    pub struct JobObject(HANDLE);

    // SAFETY: a job object handle is a process-wide kernel handle usable from any thread.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
        /// Puts `child` into a new kill-on-close job. Returns None (with a warning) on failure;
        /// the worker still runs, it is just not covered by the job.
        pub fn adopt(child: &Child) -> Option<Self> {
            let process = HANDLE(child.raw_handle()?);
            // SAFETY: plain Win32 calls with valid arguments; the handle is closed in Drop or on
            // the error paths below.
            unsafe {
                let job = match CreateJobObjectW(None, None) {
                    Ok(job) => job,
                    Err(error) => {
                        warn!(%error, "cannot create job object for worker");
                        return None;
                    }
                };
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                let assigned = configured.and_then(|()| AssignProcessToJobObject(job, process));
                match assigned {
                    Ok(()) => Some(Self(job)),
                    Err(error) => {
                        warn!(%error, "cannot assign worker to job object");
                        let _ = CloseHandle(job);
                        None
                    }
                }
            }
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            // SAFETY: the handle was created by `adopt` and is closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Speech-to-text service
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SttError {
    #[error(transparent)]
    Worker(#[from] WorkerError),
}

/// Settings of [`SttService`].
#[derive(Debug, Clone)]
pub struct SttConfig {
    pub command: WorkerCommand,
    /// Stop the worker after this long without a request.
    pub idle_timeout: Duration,
    /// Limit for one `transcribe` call once the worker is running.
    pub request_timeout: Duration,
    /// HELLO limit for the first start of the process (the model may be downloaded and loaded).
    pub first_hello_timeout: Duration,
    /// HELLO limit for later starts.
    pub hello_timeout: Duration,
    /// How long to wait for the worker to exit after SHUTDOWN before killing it.
    pub shutdown_grace: Duration,
}

impl SttConfig {
    pub fn new(command: WorkerCommand) -> Self {
        Self {
            command,
            idle_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(20),
            first_hello_timeout: Duration::from_secs(60),
            hello_timeout: Duration::from_secs(20),
            shutdown_grace: Duration::from_secs(2),
        }
    }
}

/// Result of one recognized utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub text: String,
    pub lang: String,
    pub is_final: bool,
    /// Processing time reported by the worker.
    pub ms: u64,
}

impl Transcript {
    fn parse(payload: &[u8]) -> Result<Self, WorkerError> {
        let value: Value = serde_json::from_slice(payload)
            .map_err(|error| WorkerError::Protocol(format!("invalid TRANSCRIPT: {error}")))?;
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkerError::Protocol("TRANSCRIPT lacks `text`".into()))?;
        Ok(Self {
            text: text.to_owned(),
            lang: value
                .get("lang")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            is_final: value.get("final").and_then(Value::as_bool).unwrap_or(true),
            ms: value.get("ms").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

struct State {
    worker: Option<WorkerHandle>,
    /// True after the first attempt to start a worker (later starts use the shorter HELLO limit).
    started_before: bool,
    last_used: Instant,
    /// Bumped whenever the worker is replaced or removed; ends stale idle watchers.
    generation: u64,
    idle_task: Option<JoinHandle<()>>,
}

impl State {
    fn clear_idle_task(&mut self) {
        self.generation += 1;
        if let Some(task) = self.idle_task.take() {
            task.abort();
        }
    }
}

struct Inner {
    label: &'static str,
    config: SttConfig,
    state: Mutex<State>,
}

/// Boxed future borrowing the worker for one request (see [`ManagedWorker::run`]).
pub type WorkerFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, WorkerError>> + Send + 'a>>;

/// A lazily started worker process shared by many callers (used for STT and for TTS).
///
/// At most one request runs at a time; later callers wait for their turn. The worker is
/// started by the first request, stopped after `idle_timeout` without requests, and replaced by
/// a fresh process on the next request after it dies. A crash fails the request in flight and is
/// not retried automatically.
#[derive(Clone)]
pub struct ManagedWorker {
    inner: Arc<Inner>,
}

impl ManagedWorker {
    pub fn new(label: &'static str, config: SttConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                label,
                config,
                state: Mutex::new(State {
                    worker: None,
                    started_before: false,
                    last_used: Instant::now(),
                    generation: 0,
                    idle_task: None,
                }),
            }),
        }
    }

    /// Runs one request on the worker (starting it first if needed) with the configured
    /// `request_timeout`. A worker-reported error leaves the worker usable; any other failure
    /// replaces it on the next request.
    pub async fn run<D, T>(
        &self,
        data: D,
        request: for<'a> fn(&'a mut WorkerHandle, D) -> WorkerFuture<'a, T>,
    ) -> Result<T, WorkerError> {
        let mut state = self.inner.state.lock().await;
        self.ensure_worker(&mut state).await?;
        let Some(worker) = state.worker.as_mut() else {
            return Err(WorkerError::Exited("worker is not running".into()));
        };
        let limit = self.inner.config.request_timeout;
        let outcome = match timeout(limit, request(worker, data)).await {
            Ok(result) => result,
            Err(_) => Err(WorkerError::Timeout(limit)),
        };
        state.last_used = Instant::now();
        if let Err(error) = &outcome {
            if !matches!(error, WorkerError::Reported { .. }) {
                warn!(worker = self.inner.label, %error, "worker request failed; discarding the worker");
                self.discard_worker(&mut state).await;
            }
        }
        outcome
    }

    /// Starts the worker now (if it is not running) so that its cold start overlaps other work.
    /// Waits for HELLO; a request that comes meanwhile waits for its turn.
    pub async fn prewarm(&self) -> Result<(), WorkerError> {
        let mut state = self.inner.state.lock().await;
        self.ensure_worker(&mut state).await?;
        state.last_used = Instant::now();
        Ok(())
    }

    /// PING/PONG health check of the running worker. Returns false if no worker is running.
    pub async fn ping(&self, limit: Duration) -> Result<bool, WorkerError> {
        let mut state = self.inner.state.lock().await;
        let Some(worker) = state.worker.as_mut() else {
            return Ok(false);
        };
        if !worker.is_running() {
            self.discard_worker(&mut state).await;
            return Ok(false);
        }
        let result = match state.worker.as_mut() {
            Some(worker) => worker.ping(limit).await,
            None => return Ok(false),
        };
        match result {
            Ok(()) => Ok(true),
            Err(error) => {
                self.discard_worker(&mut state).await;
                Err(error)
            }
        }
    }

    /// True while a worker process is alive.
    pub async fn is_running(&self) -> bool {
        let mut state = self.inner.state.lock().await;
        state.worker.as_mut().is_some_and(WorkerHandle::is_running)
    }

    /// Process id of the running worker, if any.
    pub async fn worker_pid(&self) -> Option<u32> {
        let state = self.inner.state.lock().await;
        state.worker.as_ref().map(WorkerHandle::pid)
    }

    /// HELLO of the running worker, if any.
    pub async fn hello(&self) -> Option<Hello> {
        let state = self.inner.state.lock().await;
        state.worker.as_ref().map(|worker| worker.hello().clone())
    }

    /// Stops the worker (SHUTDOWN, then kill after the grace period) and its idle watcher.
    pub async fn stop(&self) {
        let mut state = self.inner.state.lock().await;
        state.clear_idle_task();
        if let Some(worker) = state.worker.take() {
            worker.shutdown(self.inner.config.shutdown_grace).await;
        }
    }

    async fn ensure_worker(&self, state: &mut State) -> Result<(), WorkerError> {
        if let Some(worker) = state.worker.as_mut() {
            if worker.is_running() {
                return Ok(());
            }
            info!(
                worker = self.inner.label,
                "worker is gone; starting a new one"
            );
            self.discard_worker(state).await;
        }
        let config = &self.inner.config;
        let limit = if state.started_before {
            config.hello_timeout
        } else {
            config.first_hello_timeout
        };
        state.started_before = true;
        let worker = WorkerHandle::spawn(&config.command, limit).await?;
        state.worker = Some(worker);
        state.last_used = Instant::now();
        state.clear_idle_task();
        let generation = state.generation;
        state.idle_task = Some(tokio::spawn(idle_watch(
            Arc::clone(&self.inner),
            generation,
        )));
        Ok(())
    }

    async fn discard_worker(&self, state: &mut State) {
        state.clear_idle_task();
        if let Some(mut worker) = state.worker.take() {
            worker.kill().await;
        }
    }
}

/// Stops the worker once it has been idle for `idle_timeout`.
async fn idle_watch(inner: Arc<Inner>, generation: u64) {
    let limit = inner.config.idle_timeout;
    let mut wait = limit;
    loop {
        sleep(wait).await;
        let mut state = inner.state.lock().await;
        if state.generation != generation || state.worker.is_none() {
            return;
        }
        let idle = state.last_used.elapsed();
        if idle < limit {
            wait = limit - idle;
            continue;
        }
        info!(worker = inner.label, "idle timeout; stopping the worker");
        state.generation += 1;
        state.idle_task = None;
        if let Some(worker) = state.worker.take() {
            worker.shutdown(inner.config.shutdown_grace).await;
        }
        return;
    }
}

/// Recognizes speech with a lazily started worker process (see [`ManagedWorker`]).
#[derive(Clone)]
pub struct SttService {
    worker: ManagedWorker,
}

impl SttService {
    pub fn new(config: SttConfig) -> Self {
        Self {
            worker: ManagedWorker::new("stt", config),
        }
    }

    /// Recognizes one utterance of PCM16 mono 16 kHz audio.
    pub async fn transcribe(
        &self,
        pcm16k: &[i16],
        lang_hint: Option<&str>,
    ) -> Result<Transcript, SttError> {
        if pcm16k.is_empty() {
            return Err(WorkerError::InvalidInput("no audio".into()).into());
        }
        if pcm16k.len() * 2 > MAX_UTTERANCE_BYTES {
            return Err(
                WorkerError::InvalidInput("utterance is longer than 120 seconds".into()).into(),
            );
        }
        let mut bytes = Vec::with_capacity(pcm16k.len() * 2);
        for sample in pcm16k {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let end = match lang_hint {
            Some(lang) => json!({ "lang_hint": lang }),
            None => json!({}),
        }
        .to_string();
        let transcript = self
            .worker
            .run((bytes, end), |worker, (audio, end)| {
                Box::pin(async move { request(worker, &audio, end.as_bytes()).await })
            })
            .await?;
        Ok(transcript)
    }

    /// Starts the worker now (if it is not running) so that its cold start overlaps other work.
    pub async fn prewarm(&self) -> Result<(), SttError> {
        Ok(self.worker.prewarm().await?)
    }

    /// PING/PONG health check of the running worker. Returns false if no worker is running.
    pub async fn ping(&self, limit: Duration) -> Result<bool, SttError> {
        Ok(self.worker.ping(limit).await?)
    }

    /// True while a worker process is alive.
    pub async fn is_running(&self) -> bool {
        self.worker.is_running().await
    }

    /// Process id of the running worker, if any.
    pub async fn worker_pid(&self) -> Option<u32> {
        self.worker.worker_pid().await
    }

    /// HELLO of the running worker, if any.
    pub async fn hello(&self) -> Option<Hello> {
        self.worker.hello().await
    }
}

async fn request(
    worker: &mut WorkerHandle,
    audio: &[u8],
    end_utterance: &[u8],
) -> Result<Transcript, WorkerError> {
    worker.discard_pending();
    for chunk in audio.chunks(AUDIO_CHUNK_BYTES) {
        worker.send(frame_type::AUDIO, chunk).await?;
    }
    worker
        .send(frame_type::END_UTTERANCE, end_utterance)
        .await?;
    loop {
        let frame = worker.recv().await?;
        match frame.kind {
            frame_type::TRANSCRIPT => return Transcript::parse(&frame.payload),
            frame_type::ERROR => {
                let (code, message) = parse_error(&frame.payload);
                return Err(WorkerError::Reported { code, message });
            }
            other => debug!(kind = other, "ignoring unexpected worker frame"),
        }
    }
}

#[async_trait]
impl Service for SttService {
    fn name(&self) -> &'static str {
        "stt"
    }

    /// Does nothing until cancelled: the worker process is started by the first request.
    async fn start(&self) -> CoreResult<()> {
        std::future::pending().await
    }

    async fn stop(&self) -> CoreResult<()> {
        self.worker.stop().await;
        Ok(())
    }

    fn critical(&self) -> bool {
        false
    }
}

/// Synthesizes speech on a worker that speaks `TTS_REQ` / `TTS_PCM` (the Kokoro worker).
/// Returns PCM16 mono samples (24 kHz).
pub async fn tts_request(
    worker: &mut WorkerHandle,
    text: &str,
    voice: &str,
    speed: f32,
) -> Result<Vec<i16>, WorkerError> {
    worker.discard_pending();
    let body = json!({ "text": text, "voice": voice, "speed": speed }).to_string();
    worker.send(frame_type::TTS_REQ, body.as_bytes()).await?;
    loop {
        let frame = worker.recv().await?;
        match frame.kind {
            frame_type::TTS_PCM => {
                if frame.payload.len() % 2 != 0 {
                    return Err(WorkerError::Protocol("TTS_PCM has an odd length".into()));
                }
                let (pairs, _) = frame.payload.as_chunks::<2>();
                return Ok(pairs.iter().map(|pair| i16::from_le_bytes(*pair)).collect());
            }
            frame_type::ERROR => {
                let (code, message) = parse_error(&frame.payload);
                return Err(WorkerError::Reported { code, message });
            }
            other => debug!(kind = other, "ignoring unexpected worker frame"),
        }
    }
}
