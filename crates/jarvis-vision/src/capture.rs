//! Screen capture, on demand only, into memory only.
//!
//! Method: plain GDI. The primary monitor is copied with `BitBlt` from the screen DC; the active
//! window is rendered with `PrintWindow(PW_RENDERFULLCONTENT)` (works for occluded and
//! DWM/Chromium-composited windows) and cropped to the DWM extended frame bounds, with a `BitBlt`
//! of the visible rectangle as fallback. GDI needs no administrator rights, no consent prompt and
//! no capture border (Windows.Graphics.Capture shows a yellow frame and needs a D3D device and a
//! frame pool, which is a lot of machinery for a single still). Nothing is ever written to disk.

use crate::error::{Result, VisionError};
use crate::frame::Frame;
use serde::Serialize;
use std::ffi::c_void;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, SM_CXSCREEN, SM_CYSCREEN,
};

/// `PW_RENDERFULLCONTENT`: ask DWM-composited windows to paint their real content.
const PW_RENDERFULLCONTENT: u32 = 2;

/// What is known about the foreground window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowInfo {
    pub title: String,
    pub process_id: u32,
    /// File name of the executable without the extension (for example `notepad`).
    pub process_name: String,
    pub exe_path: Option<String>,
}

/// Per-monitor DPI awareness for the current thread while a capture runs, so that window
/// rectangles and screen metrics are physical pixels. Restored on drop.
struct DpiScope(DPI_AWARENESS_CONTEXT);

impl DpiScope {
    fn enter() -> Self {
        // SAFETY: plain thread-state call with a documented constant.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiScope {
    fn drop(&mut self) {
        // SAFETY: restores the context returned by the matching set call.
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

/// A memory DC with a 32-bit top-down DIB section selected into it.
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *const u8,
    width: u32,
    height: u32,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > 32_768 || height > 32_768 {
            return Err(VisionError::Capture(format!(
                "unsupported capture size {width}x{height}"
            )));
        }
        // SAFETY: standard GDI object creation; every created object is released in `drop`.
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(VisionError::Capture("cannot get the screen DC".into()));
            }
            let dc = CreateCompatibleDC(screen);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, None, 0);
            ReleaseDC(None, screen);
            let bitmap = match bitmap {
                Ok(bitmap) if !bits.is_null() && !dc.is_invalid() => bitmap,
                other => {
                    if let Ok(bitmap) = other {
                        let _ = DeleteObject(bitmap);
                    }
                    if !dc.is_invalid() {
                        let _ = DeleteDC(dc);
                    }
                    return Err(VisionError::Capture("cannot create a bitmap".into()));
                }
            };
            let previous = SelectObject(dc, bitmap);
            Ok(Self {
                dc,
                bitmap,
                previous,
                bits: bits as *const u8,
                width,
                height,
            })
        }
    }

    /// Copies the pixels out as a frame with alpha forced to 255 (GDI leaves it at 0).
    fn to_frame(&self) -> Result<Frame> {
        let length = self.width as usize * self.height as usize * 4;
        // SAFETY: `bits` points at the DIB section of exactly `length` bytes, alive until drop.
        let mut data = unsafe { std::slice::from_raw_parts(self.bits, length) }.to_vec();
        for pixel in data.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Frame::new(self.width, self.height, data)
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: releases the objects created in `new`, in the right order.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

/// `BitBlt` of a screen rectangle (physical pixels) into a new frame.
fn blit_screen(x: i32, y: i32, width: u32, height: u32) -> Result<Frame> {
    let canvas = Canvas::new(width, height)?;
    // SAFETY: the screen DC is released right after the copy.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(VisionError::Capture("cannot get the screen DC".into()));
        }
        let result = BitBlt(
            canvas.dc,
            0,
            0,
            width as i32,
            height as i32,
            screen,
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(None, screen);
        result.map_err(|error| VisionError::Capture(format!("BitBlt failed: {error}")))?;
    }
    canvas.to_frame()
}

fn ensure_visible(frame: Frame, what: &str) -> Result<Frame> {
    if frame.is_blank() {
        return Err(VisionError::Capture(format!(
            "{what} came out completely black: the screen is probably locked, switched off or on a secure desktop"
        )));
    }
    Ok(frame)
}

/// The whole primary monitor.
pub fn capture_primary_monitor() -> Result<Frame> {
    let _dpi = DpiScope::enter();
    // SAFETY: plain metric queries.
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if width <= 0 || height <= 0 {
        return Err(VisionError::Capture("no primary monitor".into()));
    }
    ensure_visible(
        blit_screen(0, 0, width as u32, height as u32)?,
        "the screen",
    )
}

fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn window_info_of(hwnd: HWND) -> WindowInfo {
    let mut title_buffer = [0u16; 512];
    // SAFETY: the buffer outlives the calls; the pid pointer is a local.
    let (title, process_id) = unsafe {
        let length = GetWindowTextW(hwnd, &mut title_buffer).max(0) as usize;
        let mut process_id = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        (
            String::from_utf16_lossy(&title_buffer[..length.min(title_buffer.len())]),
            process_id,
        )
    };
    let exe_path = exe_path_of(process_id);
    let process_name = exe_path
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    WindowInfo {
        title,
        process_id,
        process_name,
        exe_path,
    }
}

fn exe_path_of(process_id: u32) -> Option<String> {
    if process_id == 0 {
        return None;
    }
    // SAFETY: the handle is closed before returning; the buffer outlives the call.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(wide_to_string(
            &buffer[..(length as usize).min(buffer.len())],
        ))
    }
}

/// Title, process and executable path of the foreground window (no capture involved).
pub fn active_window_info() -> Result<WindowInfo> {
    // SAFETY: plain query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Err(VisionError::Capture("there is no active window".into()));
    }
    Ok(window_info_of(hwnd))
}

fn extended_frame(hwnd: HWND) -> Result<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid RECT of the size passed.
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if dwm.is_ok() && rect.right > rect.left && rect.bottom > rect.top {
        return Ok(rect);
    }
    // SAFETY: `rect` is a valid out parameter.
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|error| VisionError::Capture(format!("GetWindowRect failed: {error}")))?;
    Ok(rect)
}

/// The foreground window (its visible frame, without the invisible resize border) and its info.
pub fn capture_active_window() -> Result<(Frame, WindowInfo)> {
    // SAFETY: plain query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Err(VisionError::Capture("there is no active window".into()));
    }
    capture_hwnd(hwnd)
}

/// A specific top-level window (`hwnd` as the raw handle value), wherever it is on screen. For
/// checks and tools that must not change the focus.
pub fn capture_window(hwnd: isize) -> Result<(Frame, WindowInfo)> {
    if hwnd == 0 {
        return Err(VisionError::Capture("there is no such window".into()));
    }
    capture_hwnd(HWND(hwnd as *mut c_void))
}

fn capture_hwnd(hwnd: HWND) -> Result<(Frame, WindowInfo)> {
    let _dpi = DpiScope::enter();
    // SAFETY: plain query.
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return Err(VisionError::Capture(
            "the active window is minimized".into(),
        ));
    }
    let info = window_info_of(hwnd);
    let frame_bounds = extended_frame(hwnd)?;
    let mut outer = RECT::default();
    // SAFETY: valid out parameter.
    unsafe { GetWindowRect(hwnd, &mut outer) }
        .map_err(|error| VisionError::Capture(format!("GetWindowRect failed: {error}")))?;
    let (outer_w, outer_h) = (
        (outer.right - outer.left).max(0) as u32,
        (outer.bottom - outer.top).max(0) as u32,
    );

    let printed = Canvas::new(outer_w, outer_h).and_then(|canvas| {
        // SAFETY: `canvas.dc` is a valid memory DC; `hwnd` was just obtained.
        let ok = unsafe { PrintWindow(hwnd, canvas.dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)) };
        if !ok.as_bool() {
            return Err(VisionError::Capture("PrintWindow failed".into()));
        }
        let full = ensure_visible(canvas.to_frame()?, "the window")?;
        let x = (frame_bounds.left - outer.left).max(0) as u32;
        let y = (frame_bounds.top - outer.top).max(0) as u32;
        let w = (frame_bounds.right - frame_bounds.left).max(0) as u32;
        let h = (frame_bounds.bottom - frame_bounds.top).max(0) as u32;
        full.crop(x, y, w, h)
            .ok_or_else(|| VisionError::Capture("the window frame is empty".into()))
    });
    let frame = match printed {
        Ok(frame) => frame,
        Err(error) => {
            tracing::debug!(%error, "PrintWindow path failed, copying the visible rectangle");
            let w = (frame_bounds.right - frame_bounds.left).max(0) as u32;
            let h = (frame_bounds.bottom - frame_bounds.top).max(0) as u32;
            ensure_visible(
                blit_screen(frame_bounds.left, frame_bounds.top, w, h)?,
                "the window",
            )?
        }
    };
    Ok((frame, info))
}
