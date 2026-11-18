//! OCR through the built-in Windows engine (`Windows.Media.Ocr`): offline, no models to ship.

use crate::error::{Result, VisionError};
use crate::frame::Frame;
use serde::Serialize;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Language tags asked for by default: Russian first, then English.
pub const DEFAULT_LANGUAGES: [&str; 2] = ["ru", "en"];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TextRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OcrWord {
    pub text: String,
    pub rect: TextRect,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OcrLine {
    pub text: String,
    pub rect: TextRect,
    pub words: Vec<OcrWord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrOutput {
    /// All lines joined with newlines.
    pub text: String,
    pub lines: Vec<OcrLine>,
    /// Tag of the recognizer language that produced this result (for example `ru`).
    pub language: String,
    /// Every OCR language pack installed on this machine.
    pub available_languages: Vec<String>,
    /// Set when a requested language is missing or another one was used instead.
    pub notice: Option<String>,
}

/// Recognizes text in `frame`, trying `wanted` language tags (prefix match against the installed
/// recognizer languages, so `en` finds `en-US`). Missing packs are reported in `notice`; if none of
/// the wanted ones is installed, the user-profile languages are used.
pub fn recognize(frame: &Frame, wanted: &[&str]) -> Result<OcrOutput> {
    let frame = frame.clone();
    let wanted: Vec<String> = wanted.iter().map(|tag| (*tag).to_owned()).collect();
    // A fresh thread is always a clean MTA, where blocking on WinRT operations is safe.
    std::thread::scope(|scope| {
        scope
            .spawn(|| recognize_on_this_thread(&frame, &wanted))
            .join()
            .unwrap_or_else(|_| Err(VisionError::Ocr("the OCR thread panicked".into())))
    })
}

fn ocr_error(error: windows::core::Error) -> VisionError {
    VisionError::Ocr(error.to_string())
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

/// Share of Cyrillic among all letters in `text` (0.0 when there are no letters).
fn cyrillic_share(text: &str) -> f32 {
    let (mut cyrillic, mut letters) = (0u32, 0u32);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if is_cyrillic(c) {
            cyrillic += 1;
        }
    }
    if letters == 0 {
        0.0
    } else {
        cyrillic as f32 / letters as f32
    }
}

fn recognize_on_this_thread(frame: &Frame, wanted: &[String]) -> Result<OcrOutput> {
    // SAFETY: initialises COM for this (fresh) thread; the result is irrelevant when already done.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    let mut available: Vec<(String, Language)> = Vec::new();
    for language in OcrEngine::AvailableRecognizerLanguages().map_err(ocr_error)? {
        let tag = language.LanguageTag().map_err(ocr_error)?.to_string();
        available.push((tag, language));
    }
    let available_tags: Vec<String> = available.iter().map(|(tag, _)| tag.clone()).collect();

    let mut engines: Vec<(String, OcrEngine)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for tag in wanted {
        let found = available.iter().find(|(candidate, _)| {
            let candidate = candidate.to_lowercase();
            let tag = tag.to_lowercase();
            candidate == tag || candidate.starts_with(&format!("{tag}-"))
        });
        match found {
            Some((full_tag, language)) => {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(language) {
                    engines.push((full_tag.clone(), engine));
                    continue;
                }
                missing.push(tag.clone());
            }
            None => missing.push(tag.clone()),
        }
    }
    let mut notice = None;
    if engines.is_empty() {
        let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
            VisionError::Ocr(format!(
                "no OCR language pack is available (wanted: {}, installed: {})",
                wanted.join(", "),
                available_tags.join(", ")
            ))
        })?;
        let tag = engine
            .RecognizerLanguage()
            .and_then(|language| language.LanguageTag())
            .map(|tag| tag.to_string())
            .unwrap_or_else(|_| "user-profile".into());
        notice = Some(format!(
            "OCR language packs {} are not installed; used {tag} instead",
            missing.join(", ")
        ));
        engines.push((tag, engine));
    } else if !missing.is_empty() {
        notice = Some(format!(
            "OCR language pack {} is not installed; recognized with {}",
            missing.join(", "),
            engines
                .iter()
                .map(|(tag, _)| tag.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let max_side = OcrEngine::MaxImageDimension().map_err(ocr_error)?;
    let scaled = frame.scaled_to_max(max_side);
    let back_scale = frame.width() as f32 / scaled.width() as f32;
    let bitmap = to_bitmap(&scaled)?;

    let mut results: Vec<(String, OcrOutput)> = Vec::new();
    for (tag, engine) in &engines {
        let lines = read_lines(engine, &bitmap, back_scale)?;
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        results.push((
            tag.clone(),
            OcrOutput {
                text,
                lines,
                language: tag.clone(),
                available_languages: available_tags.clone(),
                notice: notice.clone(),
            },
        ));
    }
    Ok(merge_results(results))
}

fn is_ru(tag: &str) -> bool {
    tag.to_lowercase().starts_with("ru")
}

/// The Russian engine is the only one that reads Cyrillic, but on Latin text it makes look-alike
/// mistakes that the English engine does not. So: pure Latin picture -> the other engine; mixed
/// picture -> Russian lines, with each Latin-only line replaced by the overlapping line of the
/// other engine.
fn merge_results(mut results: Vec<(String, OcrOutput)>) -> OcrOutput {
    let ru_at = results.iter().position(|(tag, _)| is_ru(tag));
    let other_at = results.iter().position(|(tag, _)| !is_ru(tag));
    let (ru_at, other_at) = match (ru_at, other_at) {
        (Some(ru), Some(other)) => (ru, other),
        (Some(only), None) | (None, Some(only)) => return results.swap_remove(only).1,
        (None, None) => return results.swap_remove(0).1,
    };
    if cyrillic_share(&results[ru_at].1.text) < 0.05 {
        return results.swap_remove(other_at).1;
    }
    let other = results[other_at].1.clone();
    let mut merged = results[ru_at].1.clone();
    let mut replaced = false;
    for line in &mut merged.lines {
        if cyrillic_share(&line.text) >= 0.25 {
            continue;
        }
        let centre = line.rect.y + line.rect.height / 2.0;
        let candidate = other.lines.iter().find(|candidate| {
            centre >= candidate.rect.y
                && centre <= candidate.rect.y + candidate.rect.height
                && candidate.rect.x < line.rect.x + line.rect.width
                && line.rect.x < candidate.rect.x + candidate.rect.width
        });
        if let Some(candidate) = candidate {
            *line = candidate.clone();
            replaced = true;
        }
    }
    if replaced {
        merged.language = format!("{}+{}", merged.language, other.language);
        merged.text = merged
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(
                "
",
            );
    }
    merged
}

fn to_bitmap(frame: &Frame) -> Result<SoftwareBitmap> {
    let writer = DataWriter::new().map_err(ocr_error)?;
    writer.WriteBytes(frame.bgra()).map_err(ocr_error)?;
    let buffer = writer.DetachBuffer().map_err(ocr_error)?;
    let bitmap = SoftwareBitmap::Create(
        BitmapPixelFormat::Bgra8,
        frame.width() as i32,
        frame.height() as i32,
    )
    .map_err(ocr_error)?;
    bitmap.CopyFromBuffer(&buffer).map_err(ocr_error)?;
    Ok(bitmap)
}

fn read_lines(engine: &OcrEngine, bitmap: &SoftwareBitmap, scale: f32) -> Result<Vec<OcrLine>> {
    let result = engine
        .RecognizeAsync(bitmap)
        .map_err(ocr_error)?
        .get()
        .map_err(ocr_error)?;
    let mut lines = Vec::new();
    for line in result.Lines().map_err(ocr_error)? {
        let text = line.Text().map_err(ocr_error)?.to_string();
        let mut words = Vec::new();
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for word in line.Words().map_err(ocr_error)? {
            let rect = word.BoundingRect().map_err(ocr_error)?;
            let rect = TextRect {
                x: rect.X * scale,
                y: rect.Y * scale,
                width: rect.Width * scale,
                height: rect.Height * scale,
            };
            x0 = x0.min(rect.x);
            y0 = y0.min(rect.y);
            x1 = x1.max(rect.x + rect.width);
            y1 = y1.max(rect.y + rect.height);
            words.push(OcrWord {
                text: word.Text().map_err(ocr_error)?.to_string(),
                rect,
            });
        }
        if words.is_empty() {
            continue;
        }
        lines.push(OcrLine {
            text,
            rect: TextRect {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            },
            words,
        });
    }
    Ok(lines)
}
