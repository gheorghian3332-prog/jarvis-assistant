#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The same executable doubles as the wake word worker (Vosk lives in a short-lived child process).
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).map(String::as_str) == Some(jarvis_voice::wake_worker::WORKER_FLAG) {
        let Some(model_dir) = arguments.get(2) else {
            std::process::exit(2);
        };
        std::process::exit(jarvis_voice::wake_worker::run(std::path::Path::new(
            model_dir,
        )));
    }
    // ... and the NVML helper of the system intelligence (started only while the window is open).
    if arguments.get(1).map(String::as_str) == Some(jarvis_app_lib::GPU_WORKER_FLAG) {
        std::process::exit(jarvis_sysintel::gpu::run_worker());
    }
    if arguments.get(1).map(String::as_str) == Some(jarvis_app_lib::SETUP_STATUS_FLAG) {
        let Some(path) = arguments.get(2) else {
            std::process::exit(2);
        };
        match jarvis_app_lib::write_setup_status(std::path::Path::new(path)) {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("jarvis: {error:#}");
                std::process::exit(1);
            }
        }
    }
    if let Err(error) = jarvis_app_lib::run() {
        // The release build has no console; the failure is also written to the log file when logging is up.
        eprintln!("jarvis: {error:#}");
        std::process::exit(1);
    }
}
