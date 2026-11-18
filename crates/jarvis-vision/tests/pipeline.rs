#![allow(clippy::chunks_exact_to_as_chunks)]

mod common;

use common::render_text;
use jarvis_vision::{
    active_window_info, capture_primary_monitor, check_destination, is_local_endpoint,
    save_png_into, AnswerSource, CaptureObserver, CloudPolicy, Frame, OllamaClient, Vision,
    VisionConfig, VisionError,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Default)]
struct Recorder(Mutex<Vec<&'static str>>);

impl Recorder {
    fn events(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

impl CaptureObserver for Recorder {
    fn capture_started(&self) {
        self.0.lock().unwrap().push("started");
    }
    fn capture_finished(&self) {
        self.0.lock().unwrap().push("finished");
    }
}

fn vision(recorder: &Arc<Recorder>, config: VisionConfig) -> Vision {
    Vision::new(config, Arc::clone(recorder) as Arc<dyn CaptureObserver>)
}

/// Tiny Ollama stand-in: answers each request with the next canned (status, body) and records the
/// request bodies.
async fn mock_ollama(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let bodies: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&bodies);
    tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut data = Vec::new();
            let mut chunk = [0u8; 65536];
            while let Ok(read) = socket.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&data);
                if let Some(split) = text.find("\r\n\r\n") {
                    let length = text[..split]
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if data.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&data).into_owned();
            let body_start = text.find("\r\n\r\n").map_or(0, |at| at + 4);
            seen.lock().unwrap().push(text[body_start..].to_owned());
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (url, bodies)
}

fn ok_answer(text: &str) -> (u16, String) {
    (
        200,
        serde_json::json!({"message": {"role": "assistant", "content": text}}).to_string(),
    )
}

fn local_config(url: String) -> VisionConfig {
    VisionConfig {
        ollama_url: url,
        request_timeout: Duration::from_secs(20),
        ..VisionConfig::default()
    }
}

#[test]
fn scaling_keeps_aspect_ratio_and_never_upsizes() {
    let big = Frame::new(3000, 1500, vec![200; 3000 * 1500 * 4]).unwrap();
    let small = big.scaled_to_max(1280);
    assert_eq!((small.width(), small.height()), (1280, 640));
    assert!(small
        .bgra()
        .chunks_exact(4)
        .all(|p| p == [200, 200, 200, 255]));
    let tall = Frame::new(900, 2700, vec![10; 900 * 2700 * 4]).unwrap();
    let tall = tall.scaled_to_max(1280);
    assert_eq!((tall.width(), tall.height()), (427, 1280));
    let tiny = Frame::new(100, 50, vec![0; 100 * 50 * 4]).unwrap();
    assert_eq!(tiny.scaled_to_max(1280).width(), 100);
}

#[test]
fn scaling_averages_pixels() {
    // Left half black, right half white: a 2:1 shrink must keep the split.
    let mut data = Vec::new();
    for _ in 0..4 {
        for x in 0..8 {
            let v = if x < 4 { 0 } else { 255 };
            data.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let frame = Frame::new(8, 4, data).unwrap();
    let half = frame.scaled_to_max(4);
    assert_eq!((half.width(), half.height()), (4, 2));
    assert_eq!(&half.bgra()[0..4], &[0, 0, 0, 255]);
    assert_eq!(&half.bgra()[12..16], &[255, 255, 255, 255]);
}

#[test]
fn cloud_needs_both_flags() {
    let remote = "https://api.example.com";
    for (cloud, allow, ok) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let policy = CloudPolicy {
            ai_cloud_enabled: cloud,
            vision_allow_cloud: allow,
        };
        assert_eq!(
            check_destination(remote, policy).is_ok(),
            ok,
            "{cloud} {allow}"
        );
        // A local endpoint is always fine.
        assert!(check_destination("http://127.0.0.1:11434", policy).is_ok());
    }
    assert!(is_local_endpoint("http://localhost:11434"));
    assert!(is_local_endpoint("http://[::1]:11434"));
    assert!(!is_local_endpoint("http://localhost.evil.example"));
    assert!(!is_local_endpoint("http://1.1.1.1:11434"));
    assert!(!is_local_endpoint("not a url"));
}

#[tokio::test]
async fn remote_endpoint_is_refused_before_any_connection() {
    let client = OllamaClient::new(
        "http://8.8.8.8:11434",
        CloudPolicy {
            ai_cloud_enabled: true,
            vision_allow_cloud: false,
        },
        Duration::from_secs(2),
    );
    let started = std::time::Instant::now();
    let result = client.chat("m", "p", &[vec![1, 2, 3]]).await;
    assert!(
        matches!(result, Err(VisionError::CloudDenied(_))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn cloud_denial_stops_before_capture_and_indicator() {
    let recorder = Arc::new(Recorder::default());
    let config = VisionConfig {
        ollama_url: "https://api.example.com".into(),
        ai_cloud_enabled: true,
        allow_cloud: false,
        ..VisionConfig::default()
    };
    let vision = vision(&recorder, config);
    let result = vision.describe_screen(None).await;
    assert!(
        matches!(result, Err(VisionError::CloudDenied(_))),
        "{result:?}"
    );
    assert!(matches!(
        vision.explain_error().await,
        Err(VisionError::CloudDenied(_))
    ));
    // Nothing was captured, so the indicator never lit up.
    assert!(recorder.events().is_empty());
}

#[tokio::test]
async fn disabled_vision_does_nothing() {
    let recorder = Arc::new(Recorder::default());
    let config = VisionConfig {
        enabled: false,
        ..VisionConfig::default()
    };
    let vision = vision(&recorder, config);
    assert!(matches!(
        vision.read_window().await,
        Err(VisionError::Disabled)
    ));
    assert!(matches!(
        vision.save_screenshot().await,
        Err(VisionError::Disabled)
    ));
    assert!(recorder.events().is_empty());
}

#[tokio::test]
async fn vision_model_gets_downscaled_picture_ocr_text_and_question() {
    let (url, bodies) = mock_ollama(vec![ok_answer("На экране английский текст.")]).await;
    let recorder = Arc::new(Recorder::default());
    let vision = vision(&recorder, local_config(url));
    // Wider than 1280 px so the downscale is visible in what the model receives.
    let frame = render_text(&["Hello Jarvis vision test"], "Segoe UI", 40);
    let wide = {
        let mut data = Vec::new();
        for row in 0..frame.height() {
            let start = (row * frame.width()) as usize * 4;
            data.extend_from_slice(&frame.bgra()[start..start + frame.width() as usize * 4]);
            data.extend(std::iter::repeat_n(255u8, 1400 * 4));
        }
        Frame::new(frame.width() + 1400, frame.height(), data).unwrap()
    };
    assert!(wide.width() > 1280);
    let answer = vision
        .ask_about_frame(wide, "Что написано?")
        .await
        .expect("answer");
    assert_eq!(answer.source, AnswerSource::VisionModel);
    assert_eq!(answer.text, "На экране английский текст.");
    assert_eq!(recorder.events(), vec!["started", "finished"]);

    let sent = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&sent[0]).unwrap();
    assert_eq!(request["model"], "gemma3:4b");
    assert_eq!(request["stream"], false);
    let content = request["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("Что написано?"));
    assert!(content.to_lowercase().contains("hello jarvis"), "{content}");
    let image = request["messages"][0]["images"][0].as_str().unwrap();
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD
        .decode(image)
        .unwrap();
    let decoded = Frame::from_png(&png).unwrap();
    assert_eq!(decoded.width().max(decoded.height()), 1280);
}

#[tokio::test]
async fn text_model_answers_when_the_vision_model_is_missing() {
    let (url, bodies) = mock_ollama(vec![
        (404, r#"{"error":"model 'gemma3:4b' not found"}"#.into()),
        ok_answer("Ответ по тексту."),
    ])
    .await;
    let recorder = Arc::new(Recorder::default());
    let mut config = local_config(url);
    config.text_model = "text-model".into();
    let vision = vision(&recorder, config);
    let frame = render_text(&["Привет мир"], "Segoe UI", 30);
    let answer = vision
        .ask_about_frame(frame, "Что тут?")
        .await
        .expect("answer");
    assert_eq!(answer.source, AnswerSource::TextModel);
    assert_eq!(answer.model, "text-model");
    assert!(!answer.notices.is_empty());
    let sent = bodies.lock().unwrap();
    let second: serde_json::Value = serde_json::from_str(&sent[1]).unwrap();
    assert!(second["messages"][0].get("images").is_none());
    assert!(second["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Привет"));
}

#[tokio::test]
async fn indicator_is_cleared_when_the_model_is_unreachable() {
    let recorder = Arc::new(Recorder::default());
    // Port 1 on loopback: nothing listens there.
    let vision = vision(&recorder, local_config("http://127.0.0.1:1".into()));
    let frame = render_text(&["x"], "Segoe UI", 30);
    assert!(vision.ask_about_frame(frame, "q").await.is_err());
    assert_eq!(recorder.events(), vec!["started", "finished"]);
}

/// Files directly inside `directory` (non-recursive: the temp folder can be huge).
fn listing(directory: &Path) -> BTreeSet<PathBuf> {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn capture_and_ocr_write_no_files() {
    let dirs = [
        std::env::temp_dir(),
        std::env::current_dir().unwrap(),
        jarvis_vision::pictures_dir().unwrap(),
    ];
    // Anything image-like that appears in these folders during the run is a failure.
    let before: Vec<_> = dirs.iter().map(|dir| listing(dir)).collect();
    let recorder = Arc::new(Recorder::default());
    let vision = vision(&recorder, VisionConfig::default());
    let reading = vision.read_window().await;
    let monitor = capture_primary_monitor();
    println!(
        "read_window ok={} monitor ok={}",
        reading.is_ok(),
        monitor.is_ok()
    );
    let synthetic = render_text(&["No files please"], "Segoe UI", 30);
    vision.ocr_frame(synthetic).await.expect("ocr");
    for (dir, before) in dirs.iter().zip(before) {
        let created: Vec<_> = listing(dir)
            .difference(&before)
            .filter(|path| {
                let name = path.to_string_lossy().to_lowercase();
                [".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp"]
                    .iter()
                    .any(|ext| name.ends_with(ext))
            })
            .cloned()
            .collect();
        assert!(
            created.is_empty(),
            "image files appeared in {dir:?}: {created:?}"
        );
    }
    assert!(recorder.events().starts_with(&["started", "finished"]));
}

#[test]
fn saving_is_explicit_and_never_overwrites() {
    let directory = std::env::temp_dir().join(format!("jarvis-vision-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let frame = render_text(&["saved"], "Segoe UI", 30);
    let first = save_png_into(&directory, &frame).unwrap();
    let second = save_png_into(&directory, &frame).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        Frame::from_png(&std::fs::read(&first).unwrap())
            .unwrap()
            .bgra(),
        frame.bgra()
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

/// Real machine: capture may fail on a locked or switched-off screen; that must be a clean error,
/// and the test then reports it instead of failing.
#[test]
fn real_capture_works_or_fails_cleanly() {
    match capture_primary_monitor() {
        Ok(frame) => {
            println!("monitor capture: {}x{}", frame.width(), frame.height());
            assert!(frame.width() >= 640 && !frame.is_blank());
        }
        Err(VisionError::Capture(reason)) => {
            println!("CAPTURE UNAVAILABLE (screen locked/off?): {reason}");
        }
        Err(other) => panic!("unexpected error kind: {other:?}"),
    }
    match active_window_info() {
        Ok(info) => println!("active window: {info:?}"),
        Err(error) => println!("no active window info: {error}"),
    }
}
