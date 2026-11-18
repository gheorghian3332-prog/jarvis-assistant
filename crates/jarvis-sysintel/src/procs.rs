//! Process rankings, built only on request (or when the RAM alert fires): the process table is
//! read into a short-lived `System` that is dropped afterwards, so nothing stays in memory.

use crate::metrics::ProcessUsage;
use std::collections::HashMap;
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Ram,
    Cpu,
}

/// Interval between the two reads that CPU percentages need.
const CPU_WINDOW: Duration = Duration::from_millis(500);

/// Groups raw rows `(exe, path, memory_bytes, cpu_percent)` by executable and ranks them.
pub fn rank_rows(
    rows: Vec<(String, Option<String>, u64, f32)>,
    by: Rank,
    limit: usize,
    with_cpu: bool,
    mut display_name: impl FnMut(&str, Option<&str>) -> String,
) -> Vec<ProcessUsage> {
    struct Group {
        exe: String,
        path: Option<String>,
        instances: u32,
        memory: u64,
        cpu: f32,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    for (exe, path, memory, cpu) in rows {
        let group = groups.entry(exe.to_lowercase()).or_insert_with(|| Group {
            exe: exe.clone(),
            path: path.clone(),
            instances: 0,
            memory: 0,
            cpu: 0.0,
        });
        group.instances += 1;
        group.memory += memory;
        group.cpu += cpu;
        if group.path.is_none() {
            group.path = path;
        }
    }
    let mut list: Vec<Group> = groups.into_values().collect();
    match by {
        Rank::Ram => list.sort_by_key(|g| std::cmp::Reverse(g.memory)),
        Rank::Cpu => list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
    }
    list.truncate(limit);
    list.into_iter()
        .map(|g| ProcessUsage {
            display_name: display_name(&g.exe, g.path.as_deref()),
            exe: g.exe,
            instances: g.instances,
            memory_mb: g.memory / (1024 * 1024),
            cpu_percent: with_cpu.then_some(g.cpu),
        })
        .collect()
}

/// Top `limit` programs by memory (working set) or CPU. Blocks (CPU ranking waits half a second).
pub fn top_processes(by: Rank, limit: usize) -> Vec<ProcessUsage> {
    let mut system = System::new();
    let kind = match by {
        Rank::Ram => ProcessRefreshKind::new()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet),
        Rank::Cpu => ProcessRefreshKind::new()
            .with_memory()
            .with_cpu()
            .with_exe(UpdateKind::OnlyIfNotSet),
    };
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    if by == Rank::Cpu {
        std::thread::sleep(CPU_WINDOW);
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }
    let rows = system
        .processes()
        .values()
        .map(|p| {
            (
                p.name().to_string_lossy().into_owned(),
                p.exe().map(|path| path.to_string_lossy().into_owned()),
                p.memory(),
                p.cpu_usage(),
            )
        })
        .collect();
    let mut cache: HashMap<String, String> = HashMap::new();
    rank_rows(rows, by, limit, by == Rank::Cpu, |exe, path| {
        cache
            .entry(exe.to_lowercase())
            .or_insert_with(|| friendly_name(exe, path))
            .clone()
    })
}

/// The main consumer of memory, by program name for people (`Google Chrome`).
pub fn top_ram_consumer() -> Option<String> {
    top_processes(Rank::Ram, 1)
        .into_iter()
        .next()
        .map(|p| p.display_name)
}

/// File description from the executable's version resource, falling back to the exe stem.
fn friendly_name(exe: &str, path: Option<&str>) -> String {
    path.and_then(file_description)
        .unwrap_or_else(|| exe_stem(exe))
}

fn exe_stem(exe: &str) -> String {
    let len = exe.len();
    if len > 4 && exe.is_char_boundary(len - 4) && exe[len - 4..].eq_ignore_ascii_case(".exe") {
        exe[..len - 4].to_string()
    } else {
        exe.to_string()
    }
}

fn file_description(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: all buffers live across the calls; the version block is only read through VerQueryValueW.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wide.as_ptr()), 0, size, block.as_mut_ptr().cast()).ok()?;
        let mut translation: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        let key: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
        if !VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut translation,
            &mut length,
        )
        .as_bool()
            || translation.is_null()
            || length < 4
        {
            return None;
        }
        let pair = translation.cast::<u16>();
        let (language, codepage) = (*pair, *pair.add(1));
        let query: Vec<u16> =
            format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription\0")
                .encode_utf16()
                .collect();
        let mut text: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut chars = 0u32;
        if !VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(query.as_ptr()),
            &mut text,
            &mut chars,
        )
        .as_bool()
            || text.is_null()
            || chars == 0
        {
            return None;
        }
        let units = std::slice::from_raw_parts(text.cast::<u16>(), chars as usize);
        let end = units.iter().position(|c| *c == 0).unwrap_or(units.len());
        let description = String::from_utf16_lossy(&units[..end]).trim().to_string();
        (!description.is_empty()).then_some(description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<(String, Option<String>, u64, f32)> {
        let mb = 1024 * 1024;
        vec![
            ("chrome.exe".into(), None, 400 * mb, 3.0),
            ("Chrome.exe".into(), None, 700 * mb, 12.0),
            ("code.exe".into(), None, 900 * mb, 1.0),
            ("small.exe".into(), None, 10 * mb, 40.0),
        ]
    }

    #[test]
    fn groups_instances_and_sorts_by_memory() {
        let list = rank_rows(rows(), Rank::Ram, 2, false, |exe, _| exe_stem(exe));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].display_name, "chrome");
        assert_eq!(list[0].instances, 2);
        assert_eq!(list[0].memory_mb, 1100);
        assert_eq!(list[0].cpu_percent, None);
        assert_eq!(list[1].display_name, "code");
    }

    #[test]
    fn sorts_by_cpu() {
        let list = rank_rows(rows(), Rank::Cpu, 1, true, |exe, _| exe_stem(exe));
        assert_eq!(list[0].exe, "small.exe");
        assert_eq!(list[0].cpu_percent, Some(40.0));
    }

    #[test]
    fn real_ranking_has_a_consumer_with_a_name() {
        let top = top_processes(Rank::Ram, 5);
        assert!(!top.is_empty());
        assert!(top.windows(2).all(|w| w[0].memory_mb >= w[1].memory_mb));
        assert!(top.iter().all(|p| !p.display_name.is_empty()));
    }

    #[test]
    fn stem_strips_exe_only() {
        assert_eq!(exe_stem("chrome.exe"), "chrome");
        assert_eq!(exe_stem("Tool"), "Tool");
    }
}
