//! Process listing (sysinfo), hung windows (`IsHungAppWindow`), protected-process policy and termination.

use std::time::Duration;

use sysinfo::{
    Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

use crate::apps::expand_query;
use crate::error::{ControlError, ControlResult};
use crate::text::canonical;
use crate::wnd::{list_windows, WindowInfo};

/// Core Windows processes plus this application. Terminating any of them is `Forbidden`.
const PROTECTED: [&str; 16] = [
    "csrss",
    "wininit",
    "winlogon",
    "lsass",
    "services",
    "smss",
    "svchost",
    "explorer",
    "msmpeng",
    "system",
    "registry",
    "dwm",
    "system idle process",
    "secure system",
    "memory compression",
    "fontdrvhost",
];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Cpu,
    Ram,
}

fn stem(name: &str) -> String {
    let lower = name.to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// True for protected system processes and for Jarvis itself (any `jarvis*` executable).
pub fn is_protected_name(name: &str) -> bool {
    let stem = stem(name);
    stem.starts_with("jarvis") || PROTECTED.contains(&stem.as_str())
}

pub fn is_protected(pid: u32, name: &str) -> bool {
    pid <= 4 || pid == std::process::id() || is_protected_name(name)
}

/// Snapshot of running processes sorted by CPU or memory.
pub fn list(sort: SortKey, limit: usize) -> Vec<ProcInfo> {
    let kind = ProcessRefreshKind::new().with_cpu().with_memory();
    let mut system = System::new_with_specifics(RefreshKind::new().with_processes(kind));
    if sort == SortKey::Cpu {
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)));
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }
    let mut processes: Vec<ProcInfo> = system
        .processes()
        .values()
        .map(|process| ProcInfo {
            pid: process.pid().as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            cpu_percent: process.cpu_usage(),
            memory_mb: process.memory() / (1024 * 1024),
        })
        .collect();
    match sort {
        SortKey::Cpu => processes.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortKey::Ram => processes.sort_by_key(|process| std::cmp::Reverse(process.memory_mb)),
    }
    processes.truncate(limit);
    processes
}

/// Number of processes currently running.
pub fn count() -> usize {
    let system =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    system.processes().len()
}

/// Windows that are not responding.
pub fn hung_windows() -> ControlResult<Vec<WindowInfo>> {
    Ok(list_windows()?
        .into_iter()
        .filter(|window| window.hung)
        .collect())
}

/// Processes selected by pid or by (spoken) name.
pub fn find(name: Option<&str>, pid: Option<u32>) -> Vec<ProcInfo> {
    let system =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    if let Some(pid) = pid {
        return system
            .process(Pid::from_u32(pid))
            .map(|process| {
                vec![ProcInfo {
                    pid,
                    name: process.name().to_string_lossy().into_owned(),
                    cpu_percent: 0.0,
                    memory_mb: 0,
                }]
            })
            .unwrap_or_default();
    }
    let Some(name) = name else {
        return Vec::new();
    };
    let candidates = expand_query(name);
    let direct = canonical(name);
    system
        .processes()
        .values()
        .filter(|process| {
            let process_stem = canonical(&stem(&process.name().to_string_lossy()));
            process_stem == direct || candidates.contains(&process_stem)
        })
        .map(|process| ProcInfo {
            pid: process.pid().as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            cpu_percent: 0.0,
            memory_mb: 0,
        })
        .collect()
}

/// Terminates one process (`TerminateProcess`). Protected processes are refused.
pub fn terminate(pid: u32, name: &str) -> ControlResult<()> {
    if is_protected(pid, name) {
        return Err(ControlError::forbidden(format!(
            "process {name} ({pid}) is protected"
        )));
    }
    // SAFETY: the handle is closed on every path.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|error| ControlError::system(format!("cannot open process {pid}: {error}")))?;
        let result = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
        result.map_err(|error| ControlError::system(format!("cannot terminate {pid}: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_names() {
        for name in [
            "csrss.exe",
            "wininit.exe",
            "winlogon.exe",
            "lsass.exe",
            "services.exe",
            "smss.exe",
            "svchost.exe",
            "explorer.exe",
            "MsMpEng.exe",
            "System",
            "Registry",
            "dwm.exe",
            "jarvisd.exe",
            "jarvis-app.exe",
            "Jarvisctl.exe",
        ] {
            assert!(is_protected_name(name), "{name}");
        }
        for name in ["notepad.exe", "chrome.exe", "Discord.exe", "steam.exe"] {
            assert!(!is_protected_name(name), "{name}");
        }
    }

    #[test]
    fn low_pids_and_self_are_protected() {
        assert!(is_protected(4, "anything.exe"));
        assert!(is_protected(std::process::id(), "anything.exe"));
        assert!(!is_protected(123_456, "notepad.exe"));
    }

    #[test]
    fn terminate_refuses_protected_without_touching_anything() {
        let error = terminate(99_999, "lsass.exe").expect_err("forbidden");
        assert_eq!(error.code, crate::error::ErrorCode::Forbidden);
    }

    #[test]
    fn listing_sorts_by_ram_and_finds_self() {
        let top = list(SortKey::Ram, 5);
        assert!(!top.is_empty() && top.len() <= 5);
        assert!(top
            .windows(2)
            .all(|pair| pair[0].memory_mb >= pair[1].memory_mb));
        assert!(count() > 5);
        let me = find(None, Some(std::process::id()));
        assert_eq!(me.len(), 1);
    }
}
