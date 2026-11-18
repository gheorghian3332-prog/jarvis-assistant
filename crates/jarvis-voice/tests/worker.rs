//! Life cycle of the wake word worker process: it starts with the first speech, answers, goes away after
//! a quiet minute (the model's memory goes with it) and starts again with the next speech.

use jarvis_voice::{
    child::{ChildWake, WorkerCommand, IDLE_UNLOAD_FRAMES},
    pipeline::WakeBackend,
    vosk_model_dir,
};
use std::path::PathBuf;

fn backend() -> ChildWake {
    ChildWake::new(
        WorkerCommand {
            exe: PathBuf::from(env!("CARGO_BIN_EXE_jarvis-wake-worker")),
        },
        &vosk_model_dir(),
        &["лёха".to_owned()],
    )
}

#[test]
fn starts_on_speech_and_stops_after_a_quiet_minute() {
    let mut wake = backend();
    assert!(!wake.is_running(), "no worker without speech");
    for _ in 0..IDLE_UNLOAD_FRAMES {
        wake.idle_tick();
    }
    assert!(!wake.is_running(), "quiet time alone never starts it");

    wake.accept(&[0; 320]);
    assert!(wake.is_running());
    // Silence decodes to nothing, but the round trip proves the worker is alive and loaded.
    assert!(wake.finish().is_empty());
    assert!(wake.is_running());

    for _ in 0..IDLE_UNLOAD_FRAMES - 1 {
        wake.idle_tick();
    }
    assert!(wake.is_running(), "still inside the quiet minute");
    wake.idle_tick();
    assert!(
        !wake.is_running(),
        "the worker is closed after a quiet minute"
    );

    wake.accept(&[0; 320]);
    assert!(wake.is_running(), "the next speech starts it again");
    assert!(wake.finish().is_empty());
}

#[test]
fn new_names_restart_the_worker_lazily() {
    let mut wake = backend();
    wake.accept(&[0; 320]);
    assert!(wake.is_running());
    wake.set_names(&["алексей".to_owned()]).expect("names");
    assert!(!wake.is_running());
    wake.accept(&[0; 320]);
    assert!(wake.is_running());
}

#[test]
fn a_missing_executable_is_survivable() {
    let mut wake = ChildWake::new(
        WorkerCommand {
            exe: PathBuf::from("definitely-not-here.exe"),
        },
        &vosk_model_dir(),
        &["лёха".to_owned()],
    );
    wake.accept(&[0; 320]);
    assert!(!wake.is_running());
    assert!(wake.finish().is_empty());
}
