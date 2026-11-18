//! The whole pipeline (RNNoise, VAD, segmenter, Vosk small-ru) on the synthesized WAV files in
//! `tests/audio`. Needs the models from `scripts/fetch-models.ps1`; the crate does not build without them.
//! Every file runs twice: with Vosk inside the test process and with Vosk in a worker process, as in the app.

use jarvis_core::{Event, UtteranceSource};
use jarvis_voice::{process_wav, vosk_model_dir, VoiceConfig, WakeMode, WorkerCommand};
use std::path::PathBuf;

fn run_with(name: &str, mode: WakeMode) -> Vec<Event> {
    let model = vosk_model_dir();
    assert!(
        model.is_dir(),
        "{} is missing: run scripts/fetch-models.ps1",
        model.display()
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/audio")
        .join(name);
    process_wav(&path, &VoiceConfig::default(), &model, mode).expect("process wav")
}

fn worker() -> WakeMode {
    WakeMode::Worker(WorkerCommand {
        exe: PathBuf::from(env!("CARGO_BIN_EXE_jarvis-wake-worker")),
    })
}

/// Runs the file in both modes and checks that they agree.
fn run(name: &str) -> Vec<Event> {
    let in_process = run_with(name, WakeMode::InProcess);
    let isolated = run_with(name, worker());
    assert_eq!(
        wakes(&in_process),
        wakes(&isolated),
        "{name}: worker and in-process results differ"
    );
    assert_eq!(utterances(&in_process), utterances(&isolated), "{name}");
    isolated
}

fn wakes(events: &[Event]) -> Vec<(String, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            // Confidence is compared to two decimals.
            Event::WakeDetected { name, confidence } => {
                Some((name.to_string(), (confidence * 100.0).round() as u32))
            }
            _ => None,
        })
        .collect()
}

fn utterances(events: &[Event]) -> Vec<(usize, UtteranceSource)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Utterance { pcm16k, source } => Some((pcm16k.len(), *source)),
            _ => None,
        })
        .collect()
}

#[test]
fn wake_word_with_a_command() {
    let events = run("wake_open_notepad.wav");
    let heard = wakes(&events);
    assert_eq!(heard.len(), 1, "{events:?}");
    assert_eq!(heard[0].0, "лёха");
    let phrases = utterances(&events);
    assert_eq!(phrases.len(), 1, "{events:?}");
    assert_eq!(phrases[0].1, UtteranceSource::Wake);
    // "открой блокнот" is about a second long.
    assert!(phrases[0].0 > 16_000 / 2, "{}", phrases[0].0);
}

#[test]
fn wake_word_alone_gives_no_command_phrase() {
    // Nothing follows the name, so the assistant keeps listening and no command phrase exists.
    let events = run("wake_only.wav");
    assert_eq!(wakes(&events).len(), 1, "{events:?}");
    assert!(utterances(&events).is_empty(), "{events:?}");
}

#[test]
fn second_name_is_recognized() {
    let events = run("alexey_louder.wav");
    let heard = wakes(&events);
    assert_eq!(heard.len(), 1, "{events:?}");
    assert_eq!(heard[0].0, "алексей");
    assert_eq!(utterances(&events).len(), 1, "{events:?}");
}

#[test]
fn ordinary_speech_does_not_wake() {
    let events = run("neg_hello.wav");
    assert!(wakes(&events).is_empty(), "{events:?}");
    assert!(utterances(&events).is_empty());
}

#[test]
fn name_in_the_middle_of_a_long_phrase_does_not_wake() {
    let events = run("neg_name_mid_phrase.wav");
    assert!(wakes(&events).is_empty(), "{events:?}");
    assert!(utterances(&events).is_empty());
}

#[test]
fn silence_and_noise_produce_no_events() {
    for name in ["silence_5s.wav", "noise_5s.wav"] {
        let events = run(name);
        assert!(wakes(&events).is_empty(), "{name}: {events:?}");
        assert!(utterances(&events).is_empty(), "{name}");
    }
}
