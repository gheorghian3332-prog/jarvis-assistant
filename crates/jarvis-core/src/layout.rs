//! Where things live on disk, for a development checkout and for an installed copy.
//!
//! Installed (per-user NSIS): `<install>\jarvis.exe`, `<install>\libvosk.dll`, `<install>\uv.exe` and
//! `<install>\resources\workers\<name>` (worker sources with `uv.lock`). The Python environments are
//! created at first run, outside the install folder, in `%LOCALAPPDATA%\Jarvis\workers\<name>\.venv`.
//! A development checkout is recognised by `workers/stt/pyproject.toml` above the executable (or
//! `JARVIS_REPO_ROOT`) and keeps working as before.

use crate::WorkerCommand;
use std::path::{Path, PathBuf};

/// Overrides the folder of the worker environments (tests, isolated installs).
pub const WORKERS_DIR_ENV: &str = "JARVIS_WORKERS_DIR";

/// Folder of the running executable.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// `<install>\resources` when this is an installed copy (it holds the worker sources). The marker is the
/// uninstaller next to the executable: a development build also finds copies of the resources in
/// `target\release`, but never an `uninstall.exe`.
pub fn resources_dir() -> Option<PathBuf> {
    let exe = exe_dir()?;
    if !exe.join("uninstall.exe").is_file() {
        return None;
    }
    let dir = exe.join("resources");
    dir.join("workers")
        .join("stt")
        .join("pyproject.toml")
        .is_file()
        .then_some(dir)
}

/// `%LOCALAPPDATA%\Jarvis`.
pub fn local_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Jarvis")
}

/// `%LOCALAPPDATA%\Jarvis\workers` (or `JARVIS_WORKERS_DIR`).
pub fn workers_home() -> PathBuf {
    match std::env::var_os(WORKERS_DIR_ENV) {
        Some(dir) => PathBuf::from(dir),
        None => local_root().join("workers"),
    }
}

/// Working folder of one worker: sources copied from the install plus its `.venv`.
pub fn worker_home(name: &str) -> PathBuf {
    workers_home().join(name)
}

/// The interpreter of a worker's environment.
pub fn venv_python(name: &str) -> PathBuf {
    worker_home(name)
        .join(".venv")
        .join("Scripts")
        .join("python.exe")
}

/// `uv.exe` shipped next to `jarvis.exe`, falling back to `uv` from `PATH`.
pub fn uv_exe() -> PathBuf {
    exe_dir()
        .map(|dir| dir.join("uv.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("uv"))
}

/// How to start a worker of an installed copy. The command exists as soon as the install carries the
/// worker sources (or an environment is already there); a start before the first-run wizard created
/// the environment fails with "program not found" and is retried by the next request, so no restart
/// of the application is needed afterwards.
pub fn installed_worker(name: &str, module: &str) -> Option<WorkerCommand> {
    let python = venv_python(name);
    (python.is_file() || resources_dir().is_some())
        .then(|| WorkerCommand::python_module(python, module, Some(worker_home(name))))
}

/// Copies the worker sources from the install resources to `worker_home(name)` (never the `.venv`).
pub fn sync_worker_sources(name: &str) -> std::io::Result<PathBuf> {
    let source = resources_dir()
        .map(|dir| dir.join("workers").join(name))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "worker sources are not installed next to jarvis.exe",
            )
        })?;
    let target = worker_home(name);
    copy_tree(&source, &target)?;
    Ok(target)
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".venv" || name == "__pycache__" {
            continue;
        }
        let from = entry.path();
        let to = target.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_paths_live_under_the_workers_home() {
        let home = worker_home("stt");
        assert!(home.ends_with("stt"));
        assert!(venv_python("stt").starts_with(&home));
        assert!(venv_python("stt").ends_with(".venv/Scripts/python.exe"));
    }

    #[test]
    fn copies_sources_but_not_the_environment() {
        let source = tempfile::tempdir().expect("source");
        let target = tempfile::tempdir().expect("target");
        std::fs::create_dir_all(source.path().join(".venv").join("Scripts")).expect("venv");
        std::fs::create_dir_all(source.path().join("pkg").join("__pycache__")).expect("cache");
        std::fs::write(source.path().join("pyproject.toml"), "x").expect("write");
        std::fs::write(source.path().join("pkg").join("a.py"), "y").expect("write");
        std::fs::write(source.path().join(".venv").join("Scripts").join("p"), "z").expect("write");
        copy_tree(source.path(), target.path()).expect("copy");
        assert!(target.path().join("pyproject.toml").is_file());
        assert!(target.path().join("pkg").join("a.py").is_file());
        assert!(!target.path().join(".venv").exists());
        assert!(!target.path().join("pkg").join("__pycache__").exists());
    }

    #[test]
    fn installed_worker_needs_an_install_or_an_environment() {
        // Neither an installed layout (tests run from cargo's folders) nor such an environment.
        assert!(installed_worker("no-such-worker-for-test", "m").is_none());
    }
}
