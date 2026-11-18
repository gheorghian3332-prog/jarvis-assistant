//! `jarvisd --vision <describe|read-window|ocr-file <png>> [--question "..."] [--model <name>]
//! [--delay <seconds>]`: checks the vision pipeline from a terminal. Captures happen only here, on
//! request, and nothing is written to disk.

use anyhow::Context;
use jarvis_vision::{Answer, CaptureObserver, Frame, Vision, VisionConfig, WindowInfo};
use std::sync::Arc;
use std::time::Duration;

pub const FLAG: &str = "--vision";

pub const USAGE: &str = "usage: jarvisd --vision describe [--question <text>] [--model <name>] \
[--delay <seconds>] | jarvisd --vision read-window [--delay <seconds>] | jarvisd --vision explain-error \
[--delay <seconds>] | jarvisd --vision ocr-file <image.png> [--question <text>] [--model <name>]";

/// Prints the indicator transitions, standing in for the UI's `vision://capture` event.
struct PrintObserver;

impl CaptureObserver for PrintObserver {
    fn capture_started(&self) {
        eprintln!("[vision://capture active=true]");
    }
    fn capture_finished(&self) {
        eprintln!("[vision://capture active=false]");
    }
}

struct Options {
    question: Option<String>,
    model: Option<String>,
    delay: Duration,
    file: Option<String>,
}

fn parse(arguments: &[String], wants_file: bool) -> anyhow::Result<Options> {
    let mut options = Options {
        question: None,
        model: None,
        delay: Duration::ZERO,
        file: None,
    };
    let mut iter = arguments.iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--question" => options.question = Some(iter.next().context(USAGE)?.clone()),
            "--model" => options.model = Some(iter.next().context(USAGE)?.clone()),
            "--delay" => {
                let seconds: u64 = iter.next().context(USAGE)?.parse().context(USAGE)?;
                options.delay = Duration::from_secs(seconds);
            }
            other if wants_file && options.file.is_none() && !other.starts_with("--") => {
                options.file = Some(other.to_owned());
            }
            _ => anyhow::bail!(USAGE),
        }
    }
    Ok(options)
}

fn print_window(window: &WindowInfo) {
    println!("Window title={:?}", window.title);
    println!(
        "Window process={} pid={} exe={}",
        window.process_name,
        window.process_id,
        window.exe_path.as_deref().unwrap_or("?")
    );
}

fn print_answer(answer: &Answer) {
    if let Some(window) = &answer.window {
        print_window(window);
    }
    println!("OCR language={}", answer.ocr_language);
    for notice in &answer.notices {
        println!("Notice {notice}");
    }
    println!("Answer source={:?} model={}", answer.source, answer.model);
    println!("{}", answer.text);
    println!(
        "Timing capture_ms={} ocr_ms={} model_ms={}",
        answer.capture_ms, answer.ocr_ms, answer.model_ms
    );
}

pub async fn run(arguments: &[String]) -> anyhow::Result<()> {
    let command = arguments.first().context(USAGE)?.as_str();
    let options = parse(&arguments[1..], command == "ocr-file")?;
    let mut config = VisionConfig::default();
    if let Some(model) = &options.model {
        config.vlm_model.clone_from(model);
        config.text_model.clone_from(model);
    }
    let vision = Vision::new(config, Arc::new(PrintObserver));
    if !options.delay.is_zero() {
        eprintln!(
            "capturing in {} s: switch to the window you want",
            options.delay.as_secs()
        );
        tokio::time::sleep(options.delay).await;
    }
    match command {
        "describe" => {
            let answer = vision
                .describe_screen(options.question.as_deref())
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            print_answer(&answer);
        }
        "explain-error" => {
            let answer = vision
                .explain_error()
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            print_answer(&answer);
        }
        "read-window" => {
            let reading = vision
                .read_window()
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            print_window(&reading.window);
            println!(
                "OCR language={} available={} lines={}",
                reading.ocr.language,
                reading.ocr.available_languages.join(","),
                reading.ocr.lines.len()
            );
            if let Some(notice) = &reading.ocr.notice {
                println!("Notice {notice}");
            }
            println!("{}", reading.ocr.text);
            println!(
                "Timing capture_ms={} ocr_ms={}",
                reading.capture_ms, reading.ocr_ms
            );
        }
        "ocr-file" => {
            let path = options.file.context(USAGE)?;
            let bytes = std::fs::read(&path).with_context(|| format!("reading {path}"))?;
            let frame = Frame::from_png(&bytes).map_err(|error| anyhow::anyhow!(error))?;
            println!("Image {}x{}", frame.width(), frame.height());
            match options.question.as_deref() {
                Some(question) => {
                    let answer = vision
                        .ask_about_frame(frame, question)
                        .await
                        .map_err(|error| anyhow::anyhow!(error))?;
                    print_answer(&answer);
                }
                None => {
                    let ocr = vision
                        .ocr_frame(frame)
                        .await
                        .map_err(|error| anyhow::anyhow!(error))?;
                    println!(
                        "OCR language={} available={} lines={}",
                        ocr.language,
                        ocr.available_languages.join(","),
                        ocr.lines.len()
                    );
                    if let Some(notice) = &ocr.notice {
                        println!("Notice {notice}");
                    }
                    println!("{}", ocr.text);
                }
            }
        }
        _ => anyhow::bail!(USAGE),
    }
    Ok(())
}
