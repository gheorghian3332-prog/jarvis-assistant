//! Shared helpers: synthetic pictures with known text (rendered with GDI, in memory only) and a
//! text similarity metric.
#![allow(dead_code, clippy::chunks_exact_to_as_chunks)]

use jarvis_vision::Frame;
use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GetDC, PatBlt,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, TRANSPARENT, WHITENESS,
};

/// Black text on white, one string per line, in the given font and pixel height.
pub fn render_text(lines: &[&str], font: &str, pixel_height: i32) -> Frame {
    let line_step = pixel_height * 3 / 2;
    let width = 1400i32;
    let height = line_step * lines.len() as i32 + pixel_height * 2;
    // SAFETY: plain GDI drawing into a private DIB section; all objects are freed before return.
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        ReleaseDC(None, screen);
        let old_bitmap = SelectObject(dc, bitmap);
        let _ = PatBlt(dc, 0, 0, width, height, WHITENESS);
        let face: Vec<u16> = font.encode_utf16().chain(std::iter::once(0)).collect();
        let hfont = CreateFontW(
            -pixel_height,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1, // DEFAULT_CHARSET
            0,
            0,
            5, // CLEARTYPE_QUALITY
            0,
            PCWSTR(face.as_ptr()),
        );
        let old_font = SelectObject(dc, hfont);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, windows::Win32::Foundation::COLORREF(0));
        for (index, line) in lines.iter().enumerate() {
            let wide: Vec<u16> = line.encode_utf16().collect();
            let _ = TextOutW(
                dc,
                pixel_height,
                pixel_height + index as i32 * line_step,
                &wide,
            );
        }
        let data =
            std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec();
        SelectObject(dc, old_font);
        let _ = DeleteObject(hfont);
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        let mut data = data;
        for pixel in data.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Frame::new(width as u32, height as u32, data).unwrap()
    }
}

fn normalize(text: &str) -> Vec<char> {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect()
}

/// 1.0 = identical (case and whitespace insensitive), based on the Levenshtein distance.
pub fn similarity(expected: &str, actual: &str) -> f64 {
    let a = normalize(expected);
    let b = normalize(actual);
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    1.0 - previous[b.len()] as f64 / a.len().max(b.len()) as f64
}
