//! Stand-alone wake word worker (`jarvis-wake-worker --wake-worker <model dir>`). The apps serve the same
//! protocol from their own executables; this binary exists for tests and tools.

fn main() {
    let mut arguments = std::env::args().skip(1);
    let model_dir = match (arguments.next().as_deref(), arguments.next()) {
        (Some(jarvis_voice::wake_worker::WORKER_FLAG), Some(dir)) => std::path::PathBuf::from(dir),
        _ => {
            eprintln!("usage: jarvis-wake-worker --wake-worker <model dir>");
            std::process::exit(2);
        }
    };
    std::process::exit(jarvis_voice::wake_worker::run(&model_dir));
}
