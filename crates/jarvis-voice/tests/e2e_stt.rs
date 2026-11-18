//! Real end-to-end run: WAV -> wake word -> utterance -> STT worker (faster-whisper) -> transcript.
//! Slow and needs `uv`, `workers/stt` with its model: run with `cargo test -p jarvis-voice --test e2e_stt -- --ignored --nocapture`.

use jarvis_core::Event;
use jarvis_voice::{default_transcriber, process_wav, vosk_model_dir, VoiceConfig, WakeMode};
use std::{path::PathBuf, time::Instant};

/// Lower case, `ё` as `е`.
fn fold(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

fn utterance_of(name: &str) -> Vec<i16> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/audio")
        .join(name);
    let events = process_wav(
        &path,
        &VoiceConfig::default(),
        &vosk_model_dir(),
        WakeMode::InProcess,
    )
    .expect("wav");
    events
        .iter()
        .find_map(|event| match event {
            Event::Utterance { pcm16k, .. } => Some(pcm16k.to_vec()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name}: no utterance in {events:?}"))
}

#[test]
#[ignore = "needs the STT worker (uv + faster-whisper model)"]
fn wake_utterance_to_transcript() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let stt = default_transcriber();
    for (index, (name, word)) in [
        ("wake_open_notepad.wav", "блокнот"),
        ("alexey_louder.wav", "громче"),
    ]
    .into_iter()
    .enumerate()
    {
        let pcm = utterance_of(name);
        let started = Instant::now();
        let spoken = runtime
            .block_on(stt.transcribe(&pcm))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let latency = started.elapsed();
        println!(
            "{name}: {} worker: lang={} text={:?} worker_ms={} utterance_to_transcript_ms={}",
            if index == 0 { "cold" } else { "warm" },
            spoken.lang,
            spoken.text,
            spoken.ms,
            latency.as_millis()
        );
        assert!(
            fold(&spoken.text).contains(word),
            "{name}: {:?} lacks {word}",
            spoken.text
        );
        assert_eq!(spoken.lang, "ru");
    }
}
