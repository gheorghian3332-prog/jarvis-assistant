//! The Vosk model needs about 550 MB of private memory, far above the budget of an always-on tray app.
//! It therefore lives in a short-lived child process (`jarvis.exe --wake-worker <model dir>`, see
//! `child.rs`) that exists only while someone is speaking and is closed after a quiet minute.
//! This module is the child's side and the message framing shared by both ends.
//!
//! Frame: `[kind u8][length u32 LE][payload]`. Parent to child: `MSG_NAMES` (names, one per line, must be
//! first), `MSG_AUDIO` (PCM16 LE), `MSG_FINISH`, `MSG_DISCARD`, `MSG_QUIT`. Child to parent: `REPLY_READY`
//! after the model is loaded, `REPLY_WORDS` after `MSG_FINISH`, `REPLY_ERROR` (UTF-8 message).

use crate::wake::{HeardWord, WakeRecognizer};
use std::{
    io::{self, Read, Write},
    path::Path,
};

pub const MSG_NAMES: u8 = 1;
pub const MSG_AUDIO: u8 = 2;
pub const MSG_FINISH: u8 = 3;
pub const MSG_DISCARD: u8 = 4;
pub const MSG_QUIT: u8 = 5;
pub const REPLY_READY: u8 = 100;
pub const REPLY_WORDS: u8 = 101;
pub const REPLY_ERROR: u8 = 102;

/// The flag that turns an executable into the wake word worker.
pub const WORKER_FLAG: &str = "--wake-worker";
/// Largest accepted frame payload (a 12 s segment is about 400 KB).
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

pub fn write_frame(writer: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    writer.write_all(&[kind])?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0_u8; 5];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if length > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok((header[0], payload))
}

pub fn encode_pcm(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

pub fn decode_pcm(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

pub fn encode_names(names: &[String]) -> Vec<u8> {
    names.join("\n").into_bytes()
}

pub fn decode_names(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// One word per line: `word TAB conf TAB start TAB end`.
pub fn encode_words(words: &[HeardWord]) -> Vec<u8> {
    words
        .iter()
        .map(|word| {
            format!(
                "{}\t{}\t{}\t{}\n",
                word.word, word.conf, word.start, word.end
            )
        })
        .collect::<String>()
        .into_bytes()
}

pub fn decode_words(bytes: &[u8]) -> Vec<HeardWord> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(HeardWord {
                word: parts.next()?.to_owned(),
                conf: parts.next()?.parse().ok()?,
                start: parts.next()?.parse().ok()?,
                end: parts.next()?.parse().ok()?,
            })
        })
        .collect()
}

/// Runs the worker on stdin/stdout until `MSG_QUIT` or end of input. Returns the process exit code.
pub fn run(model_dir: &Path) -> i32 {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    match serve(model_dir, &mut input, &mut output) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn serve(model_dir: &Path, input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    let (kind, payload) = read_frame(input)?;
    if kind != MSG_NAMES {
        return write_frame(output, REPLY_ERROR, b"names must be sent first");
    }
    let mut recognizer = match WakeRecognizer::load(model_dir, &decode_names(&payload)) {
        Ok(recognizer) => recognizer,
        Err(error) => return write_frame(output, REPLY_ERROR, error.to_string().as_bytes()),
    };
    write_frame(output, REPLY_READY, &[])?;
    loop {
        let (kind, payload) = match read_frame(input) {
            Ok(frame) => frame,
            // The parent is gone: nothing left to do.
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        match kind {
            MSG_AUDIO => recognizer.accept(&decode_pcm(&payload)),
            MSG_FINISH => write_frame(output, REPLY_WORDS, &encode_words(&recognizer.finish()))?,
            MSG_DISCARD => recognizer.discard(),
            MSG_NAMES => {
                if let Err(error) = recognizer.set_names(&decode_names(&payload)) {
                    return write_frame(output, REPLY_ERROR, error.to_string().as_bytes());
                }
            }
            MSG_QUIT => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, MSG_AUDIO, &[1, 2, 3]).expect("write");
        write_frame(&mut buffer, MSG_FINISH, &[]).expect("write");
        let mut reader = buffer.as_slice();
        assert_eq!(
            read_frame(&mut reader).expect("frame"),
            (MSG_AUDIO, vec![1, 2, 3])
        );
        assert_eq!(
            read_frame(&mut reader).expect("frame"),
            (MSG_FINISH, vec![])
        );
        assert!(read_frame(&mut reader).is_err());
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut header = vec![MSG_AUDIO];
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        let error = read_frame(&mut header.as_slice()).expect_err("too large");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn pcm_round_trips() {
        let pcm = vec![0, 1, -1, i16::MAX, i16::MIN];
        assert_eq!(decode_pcm(&encode_pcm(&pcm)), pcm);
        // A dangling odd byte is ignored.
        assert_eq!(decode_pcm(&[1, 0, 9]), vec![1]);
    }

    #[test]
    fn names_round_trip() {
        let names = vec!["лёха".to_owned(), "Jarvis".to_owned()];
        assert_eq!(decode_names(&encode_names(&names)), names);
    }

    #[test]
    fn words_round_trip() {
        let words = vec![
            HeardWord {
                word: "лёха".into(),
                conf: 0.93,
                start: 0.3,
                end: 0.78,
            },
            HeardWord {
                word: "[unk]".into(),
                conf: 1.0,
                start: 1.0,
                end: 1.5,
            },
        ];
        assert_eq!(decode_words(&encode_words(&words)), words);
        assert!(decode_words(b"broken line\n").is_empty());
    }
}
