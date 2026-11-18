mod common;

use common::{render_text, similarity};
use jarvis_vision::{recognize, Frame};

const EN: [&str; 3] = [
    "The quick brown fox jumps over the lazy dog",
    "Error 404: File not found in folder C:\\Users\\Test",
    "Settings, Network and Internet, Wi-Fi 2024",
];
const RU: [&str; 3] = [
    "Съешь же ещё этих мягких французских булок",
    "Ошибка 404: файл не найден в папке Документы",
    "Настройки, сеть и интернет, Wi-Fi 2024",
];

fn run(lines: &[&str], font: &str, height: i32) -> (f64, String, String) {
    let frame = render_text(lines, font, height);
    let output = recognize(&frame, &["ru", "en"]).expect("OCR must run");
    let expected = lines.join("\n");
    (
        similarity(&expected, &output.text),
        output.language,
        output.text,
    )
}

#[test]
fn english_text_is_recognized() {
    let (score, language, text) = run(&EN, "Segoe UI", 30);
    println!("en: score={score:.3} language={language}\n{text}");
    assert!(score >= 0.92, "score {score}: {text}");
}

#[test]
fn russian_text_is_recognized() {
    let (score, language, text) = run(&RU, "Segoe UI", 30);
    println!("ru: score={score:.3} language={language}\n{text}");
    assert!(score >= 0.92, "score {score}: {text}");
    assert!(language.to_lowercase().starts_with("ru"), "{language}");
}

#[test]
fn a_second_font_works_too() {
    let (score, _, text) = run(&RU, "Arial", 28);
    assert!(score >= 0.9, "score {score}: {text}");
}

#[test]
fn lines_carry_boxes() {
    let frame = render_text(&EN, "Segoe UI", 30);
    let output = recognize(&frame, &["en"]).expect("OCR");
    assert!(output.lines.len() >= 3, "{:?}", output.lines);
    for line in &output.lines {
        assert!(line.rect.width > 50.0 && line.rect.height > 5.0);
        assert!(!line.words.is_empty());
    }
    assert!(output.lines[1].rect.y > output.lines[0].rect.y);
}

#[test]
fn missing_language_is_reported_and_another_is_used() {
    let frame = render_text(&EN, "Segoe UI", 30);
    // Japanese is not an installed OCR pack on this machine; English must still be read.
    let output = recognize(&frame, &["en", "xx"]).expect("OCR");
    let notice = output.notice.expect("a notice about the missing pack");
    assert!(notice.contains("xx"), "{notice}");
    assert!(similarity(&EN.join("\n"), &output.text) >= 0.9);
}

#[test]
fn downscaled_and_png_roundtrip_frames_still_read() {
    let frame = render_text(&EN, "Segoe UI", 40);
    // PNG round trip keeps the picture intact.
    let png = frame.to_png().expect("png");
    let back = Frame::from_png(&png).expect("decode");
    assert_eq!(
        (back.width(), back.height()),
        (frame.width(), frame.height())
    );
    assert_eq!(back.bgra(), frame.bgra());
    // Downscaling to the VLM size keeps larger text readable.
    let small = back.scaled_to_max(1000);
    let output = recognize(&small, &["en"]).expect("OCR");
    assert!(
        similarity(&EN.join("\n"), &output.text) >= 0.85,
        "{}",
        output.text
    );
}

#[test]
fn mixed_russian_and_english_lines_are_each_read_correctly() {
    let lines = [
        "te5t: The quick brown fox jumps over 12345",
        "Проверка зрения Джарвис: съешь ещё этих мягких булок",
        "Plain text, Windows (CRLF), UTF-8",
        "Настройки и параметры программы",
    ];
    for (font, height) in [("Segoe UI", 20), ("Consolas", 20)] {
        let (score, language, text) = run(&lines, font, height);
        println!("mixed {font}: score={score:.3} language={language}\n{text}");
        assert!(score >= 0.9, "{font}: score {score}: {text}");
    }
}
