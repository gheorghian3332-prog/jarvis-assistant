//! Volumes: free space (`GetDiskFreeSpaceExW`) and activity.
//!
//! Activity comes from `IOCTL_DISK_PERFORMANCE` sent to the volume (`\\.\C:`) opened with *zero*
//! access rights. That needs no admin rights, no PDH query objects (so no extra DLLs or memory in
//! the always-on process) and gives cumulative byte counters per volume, from which the rates follow.
//! PDH `\PhysicalDisk(*)` would also work but loads the whole performance-counter stack.

use crate::metrics::VolumeMetrics;
use std::collections::HashMap;
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE};
use windows::Win32::System::IO::DeviceIoControl;

const DRIVE_FIXED: u32 = 3;

#[derive(Debug, Clone, Copy)]
struct Counters {
    read: u64,
    written: u64,
    at: Instant,
}

#[derive(Default)]
pub struct Storage {
    previous: HashMap<char, Counters>,
}

/// Fixed-drive letters present now.
fn fixed_drive_letters() -> Vec<char> {
    // SAFETY: both calls only read system state.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|bit| mask & (1 << bit) != 0)
        .filter_map(|bit| char::from_u32(u32::from(b'A') + bit))
        .filter(|letter| {
            let root = wide(&format!("{letter}:\\"));
            // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
            unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED }
        })
        .collect()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn free_space(letter: char) -> Option<(u64, u64)> {
    let root = wide(&format!("{letter}:\\"));
    let mut available = 0u64;
    let mut total = 0u64;
    // SAFETY: the pointers are valid for the duration of the call.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(root.as_ptr()),
            Some(&mut available),
            Some(&mut total),
            None,
        )
        .ok()?;
    }
    Some((available, total))
}

/// Cumulative (bytes read, bytes written) of a volume; `None` when the counters are not exposed.
fn disk_counters(letter: char) -> Option<(u64, u64)> {
    let path = wide(&format!("\\\\.\\{letter}:"));
    // SAFETY: access 0 only allows querying; the handle is closed before returning.
    unsafe {
        let handle: HANDLE = CreateFileW(
            PCWSTR(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()?;
        let mut perf = DISK_PERFORMANCE::default();
        let mut returned = 0u32;
        let result = DeviceIoControl(
            handle,
            IOCTL_DISK_PERFORMANCE,
            None,
            0,
            Some(std::ptr::addr_of_mut!(perf).cast()),
            std::mem::size_of::<DISK_PERFORMANCE>() as u32,
            Some(&mut returned),
            None,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        Some((
            u64::try_from(perf.BytesRead).ok()?,
            u64::try_from(perf.BytesWritten).ok()?,
        ))
    }
}

/// Bytes per second between two counter readings.
pub fn rate(before: u64, after: u64, seconds: f64) -> Option<u64> {
    if seconds <= 0.0 || after < before {
        return None;
    }
    Some(((after - before) as f64 / seconds) as u64)
}

impl Storage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Free space of every fixed volume. Rates are filled from the previous call's counters.
    pub fn sample(&mut self, with_activity: bool) -> Vec<VolumeMetrics> {
        let system = system_drive_letter();
        let mut volumes = Vec::new();
        for letter in fixed_drive_letters() {
            let Some((free, total)) = free_space(letter) else {
                continue;
            };
            let mut volume = VolumeMetrics {
                mount: format!("{letter}:\\"),
                is_system: Some(letter) == system,
                total_bytes: total,
                free_bytes: free,
                read_bytes_per_sec: None,
                write_bytes_per_sec: None,
            };
            if with_activity {
                self.fill_activity(letter, &mut volume);
            }
            volumes.push(volume);
        }
        volumes
    }

    fn fill_activity(&mut self, letter: char, volume: &mut VolumeMetrics) {
        let Some((read, written)) = disk_counters(letter) else {
            self.previous.remove(&letter);
            return;
        };
        let now = Instant::now();
        if let Some(before) = self.previous.get(&letter) {
            let seconds = now.duration_since(before.at).as_secs_f64();
            volume.read_bytes_per_sec = rate(before.read, read, seconds);
            volume.write_bytes_per_sec = rate(before.written, written, seconds);
        }
        self.previous.insert(
            letter,
            Counters {
                read,
                written,
                at: now,
            },
        );
    }

    /// Activity only (free space is not re-read): updates the rates in `volumes`.
    pub fn refresh_activity(&mut self, volumes: &mut [VolumeMetrics]) {
        for volume in volumes {
            if let Some(letter) = volume.mount.chars().next() {
                self.fill_activity(letter, volume);
            }
        }
    }
}

fn system_drive_letter() -> Option<char> {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|drive| drive.chars().next())
        .map(|c| c.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_bytes_per_second() {
        assert_eq!(rate(1000, 3000, 2.0), Some(1000));
        assert_eq!(rate(5, 5, 1.0), Some(0));
        // Counter reset (volume re-mounted) or zero interval: no rate rather than nonsense.
        assert_eq!(rate(10, 5, 1.0), None);
        assert_eq!(rate(1, 2, 0.0), None);
    }

    #[test]
    fn finds_the_system_volume_with_space() {
        let mut storage = Storage::new();
        let volumes = storage.sample(true);
        let system = volumes.iter().find(|v| v.is_system).expect("system volume");
        assert!(system.total_bytes > 0 && system.free_bytes <= system.total_bytes);
        // The second sample has rates when the volume exposes the counters.
        let again = storage.sample(true);
        let system = again.iter().find(|v| v.is_system).expect("system volume");
        if disk_counters('C').is_some() {
            assert!(system.read_bytes_per_sec.is_some());
        }
    }
}
