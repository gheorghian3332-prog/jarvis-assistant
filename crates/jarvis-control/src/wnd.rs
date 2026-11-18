//! Top-level window controller: list, focus, minimize, maximize, close (`WM_CLOSE`), minimize all.

use std::path::Path;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetWindow, GetWindowLongW, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible,
    PostMessageW, SetForegroundWindow, ShowWindowAsync, GWL_EXSTYLE, GW_OWNER, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, WM_CLOSE, WM_COMMAND, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

use crate::apps::expand_query;
use crate::error::{ControlError, ControlResult};
use crate::text::{canonical, phonetic};

/// `WM_COMMAND` id of "Show the desktop / minimize all" handled by `Shell_TrayWnd`.
const TRAY_MINIMIZE_ALL: usize = 419;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub class: String,
    pub pid: u32,
    pub process: String,
    pub minimized: bool,
    pub hung: bool,
}

fn window_text(hwnd: HWND) -> String {
    // SAFETY: buffer sized from GetWindowTextLengthW; the API writes at most that many units.
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: fixed local buffer.
    let copied = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

/// Executable file name (e.g. `chrome.exe`) of a process id.
pub fn process_name(pid: u32) -> String {
    // SAFETY: handle is closed on every path; the buffer length is passed in/out per the API contract.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buffer = [0u16; 520];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if !ok {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buffer[..size as usize]);
        Path::new(&full)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: out-parameter points at a local u32 of the size passed.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the Vec passed by `list_windows`, alive during EnumWindows.
    let windows = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let title = window_text(hwnd);
    if title.is_empty() {
        return BOOL(1);
    }
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    let owned = GetWindow(hwnd, GW_OWNER)
        .map(|owner| owner.0 as isize != 0)
        .unwrap_or(false);
    let tool = ex_style & WS_EX_TOOLWINDOW.0 != 0 && ex_style & WS_EX_APPWINDOW.0 == 0;
    if tool || (owned && ex_style & WS_EX_APPWINDOW.0 == 0) || is_cloaked(hwnd) {
        return BOOL(1);
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    windows.push(WindowInfo {
        hwnd: hwnd.0 as isize,
        title,
        class: class_name(hwnd),
        pid,
        process: process_name(pid),
        minimized: IsIconic(hwnd).as_bool(),
        hung: IsHungAppWindow(hwnd).as_bool(),
    });
    BOOL(1)
}

/// Visible, user-facing top-level windows (no tool windows, owned popups or cloaked UWP ghosts).
pub fn list_windows() -> ControlResult<Vec<WindowInfo>> {
    let mut windows: Vec<WindowInfo> = Vec::new();
    // SAFETY: the callback only dereferences the pointer we pass, which outlives the call.
    unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<WindowInfo> as isize),
        )?;
    }
    Ok(windows)
}

/// Does a window match a spoken query (title or executable name, ru/en, synonyms)?
pub fn matches_query(window: &WindowInfo, candidates: &[String]) -> bool {
    let title = canonical(&window.title);
    let title_phon = phonetic(&title);
    let stem = canonical(
        Path::new(&window.process)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(""),
    );
    candidates.iter().any(|candidate| {
        if candidate.is_empty() {
            return false;
        }
        let candidate_phon = phonetic(candidate);
        (!stem.is_empty() && (stem == *candidate || stem.contains(candidate.as_str())))
            || title.contains(candidate.as_str())
            || (candidate_phon.chars().count() >= 4 && title_phon.contains(&candidate_phon))
    })
}

pub fn find_windows(query: &str) -> ControlResult<Vec<WindowInfo>> {
    let candidates = expand_query(query);
    Ok(list_windows()?
        .into_iter()
        .filter(|window| matches_query(window, &candidates))
        .collect())
}

fn first_match(query: &str) -> ControlResult<WindowInfo> {
    find_windows(query)?
        .into_iter()
        .next()
        .ok_or_else(|| ControlError::not_found(format!("no window matches \"{query}\"")))
}

fn hwnd_of(window: &WindowInfo) -> HWND {
    HWND(window.hwnd as *mut core::ffi::c_void)
}

/// Brings a window to the foreground (restoring it when minimized).
pub fn focus(query: &str) -> ControlResult<WindowInfo> {
    let window = first_match(query)?;
    let hwnd = hwnd_of(&window);
    // SAFETY: plain window-handle calls; stale handles just fail.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            return Err(ControlError::system(
                "Windows refused to move the window to the foreground",
            ));
        }
    }
    Ok(window)
}

pub fn minimize(query: &str) -> ControlResult<WindowInfo> {
    let window = first_match(query)?;
    // SAFETY: plain window-handle call.
    unsafe {
        let _ = ShowWindowAsync(hwnd_of(&window), SW_MINIMIZE);
    }
    Ok(window)
}

pub fn maximize(query: &str) -> ControlResult<WindowInfo> {
    let window = first_match(query)?;
    // SAFETY: plain window-handle call.
    unsafe {
        let _ = ShowWindowAsync(hwnd_of(&window), SW_MAXIMIZE);
    }
    Ok(window)
}

/// Asks every matching window to close (`WM_CLOSE`); applications prompt about unsaved work themselves.
pub fn close(query: &str) -> ControlResult<Vec<WindowInfo>> {
    let matches = find_windows(query)?;
    if matches.is_empty() {
        return Err(ControlError::not_found(format!(
            "no window matches \"{query}\""
        )));
    }
    for window in &matches {
        close_hwnd(window.hwnd)?;
    }
    Ok(matches)
}

/// Posts `WM_CLOSE` to one window handle.
pub fn close_hwnd(hwnd: isize) -> ControlResult<()> {
    let handle = HWND(hwnd as *mut core::ffi::c_void);
    // SAFETY: plain window-handle calls.
    unsafe {
        if !IsWindow(handle).as_bool() {
            return Err(ControlError::not_found("window no longer exists"));
        }
        PostMessageW(handle, WM_CLOSE, WPARAM(0), LPARAM(0))?;
    }
    Ok(())
}

/// "Minimize all" through the taskbar window (`WM_COMMAND` 419), no keyboard synthesis.
pub fn minimize_all() -> ControlResult<()> {
    // SAFETY: FindWindowW/PostMessageW with static strings.
    unsafe {
        let tray = FindWindowW(windows::core::w!("Shell_TrayWnd"), None)?;
        PostMessageW(tray, WM_COMMAND, WPARAM(TRAY_MINIMIZE_ALL), LPARAM(0))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(title: &str, process: &str) -> WindowInfo {
        WindowInfo {
            hwnd: 1,
            title: title.into(),
            class: "X".into(),
            pid: 1,
            process: process.into(),
            minimized: false,
            hung: false,
        }
    }

    #[test]
    fn query_matches_by_process_title_and_synonym() {
        let chrome = window("New Tab - Google Chrome", "chrome.exe");
        let discord = window("#general - Discord", "Discord.exe");
        let notepad = window("Untitled - Notepad", "notepad.exe");
        assert!(matches_query(&chrome, &expand_query("браузер")));
        assert!(matches_query(&discord, &expand_query("дискорд")));
        assert!(matches_query(&notepad, &expand_query("блокнот")));
        assert!(!matches_query(&notepad, &expand_query("дискорд")));
        assert!(!matches_query(&chrome, &expand_query("")));
    }
}
