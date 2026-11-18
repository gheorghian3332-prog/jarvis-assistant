//! Windows that stopped answering messages (`IsHungAppWindow`). Only looks; never touches the app.

use crate::metrics::HungWindow;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow,
    IsWindowVisible, GW_OWNER,
};

/// Visible top-level windows with a title that are hung right now.
pub fn find_hung_windows() -> Vec<HungWindow> {
    let mut found: Vec<HungWindow> = Vec::new();
    // SAFETY: the callback only runs during EnumWindows and `found` outlives the call.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(std::ptr::addr_of_mut!(found) as isize));
    }
    found
}

unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() || !IsHungAppWindow(hwnd).as_bool() {
        return BOOL(1);
    }
    // Owned windows (dialogs) belong to their owner's entry.
    if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.0.is_null()) {
        return BOOL(1);
    }
    let mut title_buffer = [0u16; 256];
    let length = GetWindowTextW(hwnd, &mut title_buffer);
    if length <= 0 {
        return BOOL(1);
    }
    let title = String::from_utf16_lossy(&title_buffer[..length as usize]);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    // SAFETY: `param` is the pointer to the Vec passed by `find_hung_windows`.
    let found = &mut *(param.0 as *mut Vec<HungWindow>);
    found.push(HungWindow {
        hwnd: hwnd.0 as isize,
        pid,
        process: process_exe_name(pid),
        title,
    });
    BOOL(1)
}

/// File name of the process image (`notepad.exe`), `None` if it cannot be queried.
pub fn process_exe_name(pid: u32) -> Option<String> {
    process_image_path(pid).and_then(|path| {
        path.rsplit(['\\', '/'])
            .next()
            .map(str::to_string)
            .filter(|name| !name.is_empty())
    })
}

/// Full path of the process image.
pub fn process_image_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: limited-query access only; the handle is closed before returning.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; MAX_PATH as usize * 2];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_our_own_exe_name() {
        let name = process_exe_name(std::process::id()).expect("own process");
        assert!(name.to_lowercase().ends_with(".exe"), "{name}");
    }

    #[test]
    fn enumeration_runs() {
        // Nothing is expected to be hung; the call itself must work and not panic.
        let _ = find_hung_windows();
    }
}
