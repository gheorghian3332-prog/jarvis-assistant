use jarvis_core::worker::{encode_frame, frame_type, read_frame, Frame, FrameError, MAX_FRAME_LEN};
use std::path::PathBuf;

/// Byte vector produced by the Python codec (`workers/stt/tests/make_vectors.py`).
fn vector() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../workers/stt/tests/vectors/frames.bin");
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn expected() -> Vec<(u8, Vec<u8>)> {
    vec![
        (frame_type::PING, Vec::new()),
        (frame_type::AUDIO, vec![0x00, 0x01, 0x02, 0x03, 0xFF, 0x7F]),
        (frame_type::END_UTTERANCE, br#"{"lang_hint":"ru"}"#.to_vec()),
        (
            frame_type::TRANSCRIPT,
            r#"{"text":"привет","lang":"ru","final":true,"ms":123}"#
                .as_bytes()
                .to_vec(),
        ),
        (frame_type::ERROR, br#"{"code":"x","message":"y"}"#.to_vec()),
        (frame_type::SHUTDOWN, Vec::new()),
    ]
}

#[tokio::test]
async fn decodes_the_python_vector() {
    let bytes = vector();
    let mut reader = bytes.as_slice();
    let mut decoded = Vec::new();
    while let Some(frame) = read_frame(&mut reader).await.expect("decode") {
        decoded.push((frame.kind, frame.payload));
    }
    assert_eq!(decoded, expected());
}

#[tokio::test]
async fn encoding_reproduces_the_python_vector_byte_for_byte() {
    let mut bytes = Vec::new();
    for (kind, payload) in expected() {
        bytes.extend(encode_frame(kind, &payload).expect("encode"));
    }
    assert_eq!(bytes, vector());
}

#[test]
fn length_prefix_counts_the_type_byte() {
    let bytes = encode_frame(frame_type::AUDIO, &[1, 2, 3]).expect("encode");
    assert_eq!(bytes, vec![4, 0, 0, 0, 0x10, 1, 2, 3]);
}

#[tokio::test]
async fn round_trips_every_type() {
    for kind in [
        frame_type::HELLO,
        frame_type::PING,
        frame_type::PONG,
        frame_type::AUDIO,
        frame_type::END_UTTERANCE,
        frame_type::TRANSCRIPT,
        frame_type::ERROR,
        frame_type::SHUTDOWN,
        0x55,
    ] {
        for payload in [vec![], vec![7], (0..=255u8).collect::<Vec<_>>()] {
            let bytes = encode_frame(kind, &payload).expect("encode");
            let frame = read_frame(&mut bytes.as_slice()).await.expect("decode");
            assert_eq!(frame, Some(Frame { kind, payload }));
        }
    }
}

#[tokio::test]
async fn clean_end_of_stream_is_none() {
    let mut empty: &[u8] = &[];
    assert!(read_frame(&mut empty).await.expect("read").is_none());
}

#[tokio::test]
async fn truncated_length_prefix_is_an_error() {
    let mut bytes: &[u8] = &[5, 0];
    assert!(matches!(
        read_frame(&mut bytes).await,
        Err(FrameError::Truncated)
    ));
}

#[tokio::test]
async fn truncated_body_is_an_error() {
    let full = encode_frame(frame_type::AUDIO, b"abcdef").expect("encode");
    let mut bytes = &full[..full.len() - 2];
    assert!(matches!(
        read_frame(&mut bytes).await,
        Err(FrameError::Truncated)
    ));
}

#[tokio::test]
async fn zero_length_is_rejected() {
    let mut bytes: &[u8] = &[0, 0, 0, 0];
    assert!(matches!(
        read_frame(&mut bytes).await,
        Err(FrameError::InvalidLength)
    ));
}

#[tokio::test]
async fn oversized_frames_are_rejected() {
    let huge = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes();
    let mut bytes: &[u8] = &huge;
    assert!(matches!(
        read_frame(&mut bytes).await,
        Err(FrameError::TooLarge(_))
    ));
    assert!(matches!(
        encode_frame(frame_type::AUDIO, &vec![0u8; MAX_FRAME_LEN]),
        Err(FrameError::TooLarge(_))
    ));
}

#[tokio::test]
async fn unknown_type_is_decoded_as_is() {
    let bytes = encode_frame(0x55, b"zz").expect("encode");
    let frame = read_frame(&mut bytes.as_slice()).await.expect("decode");
    assert_eq!(
        frame,
        Some(Frame {
            kind: 0x55,
            payload: b"zz".to_vec()
        })
    );
}
