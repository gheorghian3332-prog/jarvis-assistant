//! Test hooks for `scripts/e2e.ps1`, active only when their environment variables are set.
//!
//! * `JARVIS_E2E_TTS_WAV=<file.wav>`: speech goes to this WAV file instead of the sound device
//!   (the real-time WAV sink of `jarvis-tts`), so tests never make a sound.
//! * `JARVIS_E2E_VOICE_WAV=<a.wav;b.wav>`: the files are fed through the real voice front (denoise,
//!   VAD, wake word worker, segmentation) as if they came from the microphone, one after another with
//!   `JARVIS_E2E_VOICE_GAP_SECS` between them. The events go onto the application bus, so the whole
//!   chain runs: wake -> STT -> command -> speech.
//!
//! A user never sets these; a process that can set the environment of Jarvis already owns it.

use crate::state::AppState;
use jarvis_voice::{process_wav_with, vosk_model_dir, WakeMode, WorkerCommand};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const TTS_WAV_ENV: &str = "JARVIS_E2E_TTS_WAV";
pub const VOICE_WAV_ENV: &str = "JARVIS_E2E_VOICE_WAV";
/// Seconds between two files of `JARVIS_E2E_VOICE_WAV` (default 20).
pub const VOICE_GAP_ENV: &str = "JARVIS_E2E_VOICE_GAP_SECS";

/// Applies the hooks that the environment asks for.
pub fn install(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(path) = std::env::var_os(TTS_WAV_ENV).map(PathBuf::from) {
        tracing::warn!(path = %path.display(), "e2e: speech goes to a WAV file");
        state
            .tts
            .set_sink(Some(jarvis_tts::OutputSink::Wav { path }));
    }
    if let Some(list) = std::env::var_os(VOICE_WAV_ENV) {
        let paths = split_paths(&list.to_string_lossy());
        let gap = std::env::var(VOICE_GAP_ENV)
            .ok()
            .and_then(|text| text.parse::<u64>().ok())
            .map_or(Duration::from_secs(20), Duration::from_secs);
        let bus = state.bus.clone();
        let config = state.voice.config();
        std::thread::spawn(move || {
            let command = match WorkerCommand::detect() {
                Ok(command) => command,
                Err(error) => {
                    tracing::error!(%error, "e2e: no wake worker executable");
                    return;
                }
            };
            for (index, path) in paths.iter().enumerate() {
                if index > 0 {
                    // Lets the script break something (kill a worker) between two phrases.
                    std::thread::sleep(gap);
                }
                tracing::warn!(path = %path.display(), "e2e: feeding a WAV file through the voice front");
                let bus = bus.clone();
                let publish = move |event: &jarvis_core::Event| {
                    // Nobody may be subscribed; that is fine.
                    let _ = bus.publish(event.clone());
                };
                if let Err(error) = process_wav_with(
                    path,
                    &config,
                    &vosk_model_dir(),
                    WakeMode::Worker(command.clone()),
                    true,
                    Box::new(publish),
                ) {
                    tracing::error!(%error, "e2e: the WAV file was not processed");
                }
            }
        });
    }
}

/// `a.wav;b.wav` -> the paths, in order.
fn split_paths(list: &str) -> Vec<PathBuf> {
    list.split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_list_of_files() {
        assert_eq!(
            split_paths("a.wav; b.wav;;"),
            vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")]
        );
        assert!(split_paths("").is_empty());
    }
}
