//! Parent side of the wake word worker: starts `<exe> --wake-worker <model dir>` when the first speech
//! arrives, keeps it while speech keeps coming, and closes it after a quiet minute, which gives the
//! ~550 MB of the Vosk model back to the system (freeing it inside this process does not).

use crate::{
    error::{VoiceError, VoiceResult},
    pipeline::WakeBackend,
    wake::HeardWord,
    wake_worker::{
        decode_words, encode_names, encode_pcm, read_frame, write_frame, MSG_AUDIO, MSG_DISCARD,
        MSG_FINISH, MSG_NAMES, MSG_QUIT, REPLY_ERROR, REPLY_READY, REPLY_WORDS, WORKER_FLAG,
    },
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

/// Frames (20 ms each) without speech after which the worker is closed: one minute.
pub const IDLE_UNLOAD_FRAMES: u32 = 3_000;
/// Loading the model takes about a second; this is only a safety limit.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const RESULT_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_GRACE: Duration = Duration::from_secs(3);
/// `CREATE_NO_WINDOW`
const NO_WINDOW: u32 = 0x0800_0000;

/// Environment variable that points to the executable that serves `--wake-worker` (tests, tools).
pub const WORKER_EXE_ENV: &str = "JARVIS_WAKE_WORKER_EXE";

/// Which executable to start as the worker.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerCommand {
    pub exe: PathBuf,
}

impl WorkerCommand {
    /// The override from `JARVIS_WAKE_WORKER_EXE`, or the running executable (which handles `--wake-worker`).
    pub fn detect() -> VoiceResult<Self> {
        if let Some(exe) = std::env::var_os(WORKER_EXE_ENV) {
            return Ok(Self {
                exe: PathBuf::from(exe),
            });
        }
        std::env::current_exe()
            .map(|exe| Self { exe })
            .map_err(|error| VoiceError::Model(format!("cannot find the executable: {error}")))
    }
}

enum Reply {
    Ready,
    Words(Vec<HeardWord>),
    Error(String),
    Closed,
}

struct Running {
    process: Child,
    stdin: ChildStdin,
    replies: Receiver<Reply>,
    ready: bool,
}

pub struct ChildWake {
    command: WorkerCommand,
    model_dir: PathBuf,
    names: Vec<String>,
    running: Option<Running>,
    pending: Vec<i16>,
    idle_frames: u32,
}

impl ChildWake {
    pub fn new(command: WorkerCommand, model_dir: &Path, names: &[String]) -> Self {
        Self {
            command,
            model_dir: model_dir.to_path_buf(),
            names: names.to_vec(),
            running: None,
            pending: Vec::new(),
            idle_frames: 0,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    fn spawn(&self) -> VoiceResult<Running> {
        let mut command = Command::new(&self.command.exe);
        command
            .arg(OsString::from(WORKER_FLAG))
            .arg(&self.model_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(NO_WINDOW);
        }
        let mut process = command
            .spawn()
            .map_err(|error| VoiceError::Model(format!("cannot start the wake worker: {error}")))?;
        let (Some(mut stdin), Some(mut stdout)) = (process.stdin.take(), process.stdout.take())
        else {
            let _ = process.kill();
            return Err(VoiceError::Model("the wake worker has no pipes".into()));
        };
        write_frame(&mut stdin, MSG_NAMES, &encode_names(&self.names))
            .map_err(|error| VoiceError::Model(format!("wake worker: {error}")))?;
        let (sender, replies) = mpsc::channel();
        std::thread::Builder::new()
            .name("jarvis-wake-reader".into())
            .spawn(move || loop {
                let reply = match read_frame(&mut stdout) {
                    Ok((REPLY_READY, _)) => Reply::Ready,
                    Ok((REPLY_WORDS, payload)) => Reply::Words(decode_words(&payload)),
                    Ok((REPLY_ERROR, payload)) => {
                        Reply::Error(String::from_utf8_lossy(&payload).into_owned())
                    }
                    Ok(_) => continue,
                    Err(_) => {
                        let _ = sender.send(Reply::Closed);
                        return;
                    }
                };
                if sender.send(reply).is_err() {
                    return;
                }
            })
            .map_err(|error| VoiceError::Model(error.to_string()))?;
        tracing::info!("wake word worker started");
        Ok(Running {
            process,
            stdin,
            replies,
            ready: false,
        })
    }

    fn ensure_running(&mut self) -> bool {
        if self.running.is_some() {
            return true;
        }
        match self.spawn() {
            Ok(running) => {
                self.running = Some(running);
                true
            }
            Err(error) => {
                tracing::error!(%error, "wake word worker did not start");
                false
            }
        }
    }

    /// Reads whatever the worker has said so far; returns false if it is gone.
    fn poll(&mut self) -> bool {
        let Some(running) = self.running.as_mut() else {
            return false;
        };
        loop {
            match running.replies.try_recv() {
                Ok(Reply::Ready) => running.ready = true,
                Ok(Reply::Words(_)) => {}
                Ok(Reply::Error(message)) => {
                    tracing::error!(%message, "wake word worker failed");
                    self.stop_worker();
                    return false;
                }
                Ok(Reply::Closed) | Err(mpsc::TryRecvError::Disconnected) => {
                    tracing::warn!("wake word worker exited");
                    self.stop_worker();
                    return false;
                }
                Err(mpsc::TryRecvError::Empty) => return true,
            }
        }
    }

    fn send_pending(&mut self) -> bool {
        let Some(running) = self.running.as_mut() else {
            return false;
        };
        if self.pending.is_empty() {
            return true;
        }
        let payload = encode_pcm(&self.pending);
        self.pending.clear();
        write_frame(&mut running.stdin, MSG_AUDIO, &payload).is_ok()
    }

    /// Waits until the worker reports that the model is loaded.
    fn wait_ready(&mut self) -> bool {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let Some(running) = self.running.as_mut() else {
                return false;
            };
            if running.ready {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match running.replies.recv_timeout(left) {
                Ok(Reply::Ready) => running.ready = true,
                Ok(Reply::Words(_)) => {}
                Ok(Reply::Error(message)) => {
                    tracing::error!(%message, "wake word worker failed");
                    self.stop_worker();
                    return false;
                }
                Ok(Reply::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    self.stop_worker();
                    return false;
                }
                Err(RecvTimeoutError::Timeout) => {
                    tracing::error!("wake word worker did not load in time");
                    self.stop_worker();
                    return false;
                }
            }
        }
    }

    /// Closes the worker (asks it to quit, then kills it if it does not).
    pub fn stop_worker(&mut self) {
        let Some(mut running) = self.running.take() else {
            return;
        };
        let _ = write_frame(&mut running.stdin, MSG_QUIT, &[]);
        drop(running.stdin);
        let deadline = Instant::now() + EXIT_GRACE;
        loop {
            match running.process.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = running.process.kill();
                    let _ = running.process.wait();
                    return;
                }
            }
        }
    }
}

impl WakeBackend for ChildWake {
    fn accept(&mut self, pcm: &[i16]) {
        self.idle_frames = 0;
        if !self.ensure_running() || !self.poll() {
            return;
        }
        self.pending.extend_from_slice(pcm);
        let ready = self.running.as_ref().is_some_and(|running| running.ready);
        if ready && !self.send_pending() {
            self.stop_worker();
        }
    }

    fn finish(&mut self) -> Vec<HeardWord> {
        self.idle_frames = 0;
        if self.running.is_none() || !self.wait_ready() {
            self.pending.clear();
            return Vec::new();
        }
        if !self.send_pending() {
            self.stop_worker();
            return Vec::new();
        }
        let Some(running) = self.running.as_mut() else {
            return Vec::new();
        };
        if write_frame(&mut running.stdin, MSG_FINISH, &[]).is_err() {
            self.stop_worker();
            return Vec::new();
        }
        loop {
            let Some(running) = self.running.as_mut() else {
                return Vec::new();
            };
            match running.replies.recv_timeout(RESULT_TIMEOUT) {
                Ok(Reply::Words(words)) => return words,
                Ok(Reply::Ready) => {}
                Ok(Reply::Error(_) | Reply::Closed)
                | Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {
                    tracing::warn!("wake word worker gave no result");
                    self.stop_worker();
                    return Vec::new();
                }
            }
        }
    }

    fn discard(&mut self) {
        self.pending.clear();
        if let Some(running) = self.running.as_mut() {
            if running.ready && write_frame(&mut running.stdin, MSG_DISCARD, &[]).is_err() {
                self.stop_worker();
            }
        }
    }

    fn set_names(&mut self, names: &[String]) -> VoiceResult<()> {
        self.names = names.to_vec();
        // The next speech starts a fresh worker with the new grammar.
        self.stop_worker();
        self.pending.clear();
        Ok(())
    }

    fn idle_tick(&mut self) {
        if self.running.is_none() {
            return;
        }
        self.idle_frames += 1;
        if self.idle_frames >= IDLE_UNLOAD_FRAMES {
            tracing::info!("no speech for a minute: wake word worker closed, model unloaded");
            self.stop_worker();
            self.idle_frames = 0;
        }
    }
}

impl Drop for ChildWake {
    fn drop(&mut self) {
        self.stop_worker();
    }
}
