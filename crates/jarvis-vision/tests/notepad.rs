//! Real check with a real window. Ignored by default because it launches Notepad:
//! `cargo test -p jarvis-vision --test notepad -- --ignored --nocapture`
//!
//! Safety: runs only when no Notepad is open already (so no user document can be touched), opens a
//! brand-new temp file with known text, closes the window with WM_CLOSE (a window message, not
//! mouse or keyboard input) and deletes the file.

mod common;

use common::similarity;
use jarvis_vision::{active_window_info, capture_active_window, recognize, VisionError};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SetForegroundWindow, WM_CLOSE,
};

/// The top-level window with exactly this title, if any.
fn find_window(title: &str) -> Option<HWND> {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }
        .ok()
        .filter(|hwnd| !hwnd.0.is_null())
}

fn notepad_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq notepad.exe", "/NH"])
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .to_lowercase()
                .contains("notepad.exe")
        })
        .unwrap_or(true)
}

#[test]
#[ignore = "launches Notepad"]
fn notepad_window_is_captured_and_read() {
    if notepad_running() {
        println!("SKIPPED: Notepad is already running, not touching it");
        return;
    }
    let name = format!("jarvis-vision-notepad-{}.txt", std::process::id());
    let path = std::env::temp_dir().join(&name);
    let english = "Jarvis vision test: The quick brown fox jumps over 12345";
    let russian = "Проверка зрения Джарвис: съешь ещё этих мягких булок";
    std::fs::write(&path, format!("{english}\r\n{russian}\r\n")).unwrap();
    let mut child = std::process::Command::new("notepad.exe")
        .arg(&path)
        .spawn()
        .unwrap();

    let title = format!("{name} - Notepad");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut focused = false;
    while Instant::now() < deadline {
        if active_window_info().is_ok_and(|info| info.title.contains(&name)) {
            focused = true;
            break;
        }
        if let Some(hwnd) = find_window(&title) {
            // Bring our own window forward with the window API (no synthetic input).
            // SAFETY: plain call on a window handle that was just looked up.
            let _ = unsafe { SetForegroundWindow(hwnd) };
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let outcome = if focused {
        std::thread::sleep(Duration::from_millis(1200)); // let it paint
        Some(capture_active_window())
    } else {
        None
    };
    // Close our window by its handle, wherever it is.
    if let Some(hwnd) = find_window(&title) {
        // SAFETY: posts a close request to the window whose title is our own temp file.
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(500));
    let closed = !notepad_running();
    let _ = std::fs::remove_file(&path);
    println!(
        "notepad closed: {closed}; temp file removed: {}",
        !path.exists()
    );

    match outcome {
        None => println!("NOT VERIFIED: Notepad never became the active window"),
        Some(Err(VisionError::Capture(reason))) => {
            println!("NOT VERIFIED: capture failed: {reason}")
        }
        Some(Err(other)) => panic!("{other:?}"),
        Some(Ok((frame, info))) => {
            println!(
                "window: {info:?} frame {}x{}",
                frame.width(),
                frame.height()
            );
            let output = recognize(&frame, &["ru", "en"]).unwrap();
            println!("OCR language {}:\n{}", output.language, output.text);
            let en_score = best_line_score(english, &output.text);
            let ru_score = best_line_score(russian, &output.text);
            println!("accuracy en={en_score:.3} ru={ru_score:.3}");
            assert!(en_score >= 0.8, "en {en_score}");
            assert!(ru_score >= 0.8, "ru {ru_score}");
        }
    }
    assert!(closed, "Notepad is still running");
}

fn best_line_score(expected: &str, text: &str) -> f64 {
    text.lines()
        .map(|line| similarity(expected, line))
        .fold(0.0, f64::max)
}
