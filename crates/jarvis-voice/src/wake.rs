use crate::config::normalize_name;
use crate::error::{VoiceError, VoiceResult};
use std::path::Path;
use vosk::{CompleteResult, DecodingState, LogLevel, Model, Recognizer};

/// A segment this short (speech only) may contain the wake word anywhere.
pub const SHORT_SEGMENT_SECONDS: f32 = 1.5;
/// The first recognized word counts as the first word of the segment if it starts this soon after the
/// speech began (Vosk often omits `[unk]` words, so position in the list alone proves nothing).
pub const FIRST_WORD_WINDOW_SECONDS: f32 = 0.6;
/// Vosk's word for anything outside the grammar.
pub const UNKNOWN_WORD: &str = "[unk]";

#[derive(Debug, Clone, PartialEq)]
pub struct HeardWord {
    pub word: String,
    pub conf: f32,
    pub start: f32,
    pub end: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WakeMatch {
    /// The configured name that matched (as the user wrote it).
    pub name: String,
    pub confidence: f32,
    /// End of the name inside the segment, in seconds from the segment start.
    pub end_seconds: f32,
}

/// Vosk grammar: the names (with `ё` and `е` spellings) and the catch-all `[unk]`.
pub fn grammar(names: &[String]) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut add = |word: String| {
        if !words.contains(&word) {
            words.push(word);
        }
    };
    for name in names {
        let lower = name.trim().to_lowercase();
        add(lower.replace('ё', "е"));
        add(lower);
    }
    add(UNKNOWN_WORD.to_owned());
    words
}

/// Decides whether a finished segment contains the wake word.
///
/// The name must be the first word of the segment (the first recognized word, starting within
/// `FIRST_WORD_WINDOW_SECONDS` of the speech start), or the speech must be at most
/// `SHORT_SEGMENT_SECONDS` long. Words below `threshold` confidence are ignored.
pub fn evaluate(
    words: &[HeardWord],
    names: &[String],
    threshold: f32,
    speech_seconds: f32,
    speech_start_seconds: f32,
) -> Option<WakeMatch> {
    let normalized: Vec<(String, &String)> = names
        .iter()
        .map(|name| (normalize_name(name), name))
        .collect();
    let find = |word: &HeardWord| -> Option<WakeMatch> {
        if word.word == UNKNOWN_WORD || word.conf < threshold {
            return None;
        }
        let heard = normalize_name(&word.word);
        normalized
            .iter()
            .find(|(candidate, _)| *candidate == heard)
            .map(|(_, original)| WakeMatch {
                name: (*original).clone(),
                confidence: word.conf,
                end_seconds: word.end,
            })
    };
    let first = words.first()?;
    if first.start <= speech_start_seconds + FIRST_WORD_WINDOW_SECONDS {
        if let Some(found) = find(first) {
            return Some(found);
        }
    }
    if speech_seconds <= SHORT_SEGMENT_SECONDS {
        return words.iter().find_map(find);
    }
    None
}

/// Vosk small-ru with a closed grammar; feed it only while someone is speaking.
pub struct WakeRecognizer {
    // Declared first so it is dropped before the model.
    recognizer: Recognizer,
    model: Model,
    collected: Vec<HeardWord>,
}

fn collect(result: CompleteResult<'_>, into: &mut Vec<HeardWord>) {
    if let CompleteResult::Single(single) = result {
        into.extend(single.result.into_iter().map(|word| HeardWord {
            word: word.word.to_owned(),
            conf: word.conf,
            start: word.start,
            end: word.end,
        }));
    }
}

fn make_recognizer(model: &Model, names: &[String]) -> VoiceResult<Recognizer> {
    let mut recognizer = Recognizer::new_with_grammar(model, 16_000.0, &grammar(names))
        .ok_or_else(|| VoiceError::Model("could not create the recognizer".into()))?;
    recognizer.set_words(true);
    Ok(recognizer)
}

impl WakeRecognizer {
    /// Loads the model (about a second) and creates the recognizer.
    pub fn load(model_dir: &Path, names: &[String]) -> VoiceResult<Self> {
        if !model_dir.is_dir() {
            return Err(VoiceError::Model(format!(
                "model folder {} not found; run scripts/fetch-models.ps1",
                model_dir.display()
            )));
        }
        vosk::set_log_level(LogLevel::Error);
        let model = Model::new(model_dir.to_string_lossy().into_owned())
            .ok_or_else(|| VoiceError::Model(format!("cannot load {}", model_dir.display())))?;
        let recognizer = make_recognizer(&model, names)?;
        Ok(Self {
            recognizer,
            model,
            collected: Vec::new(),
        })
    }

    /// Replaces the grammar (new wake names) without reloading the model.
    pub fn set_names(&mut self, names: &[String]) -> VoiceResult<()> {
        self.recognizer = make_recognizer(&self.model, names)?;
        self.collected.clear();
        Ok(())
    }

    pub fn accept(&mut self, pcm: &[i16]) {
        match self.recognizer.accept_waveform(pcm) {
            Ok(DecodingState::Finalized) => {
                let result = self.recognizer.result();
                collect(result, &mut self.collected);
            }
            Ok(DecodingState::Running) => {}
            Ok(DecodingState::Failed) | Err(_) => {
                tracing::warn!("vosk failed to decode a chunk");
            }
        }
    }

    /// Ends the segment: returns every word heard in it and resets the recognizer.
    pub fn finish(&mut self) -> Vec<HeardWord> {
        let result = self.recognizer.final_result();
        collect(result, &mut self.collected);
        self.recognizer.reset();
        std::mem::take(&mut self.collected)
    }

    /// Forgets the current segment.
    pub fn discard(&mut self) {
        self.recognizer.reset();
        self.collected.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, conf: f32, start: f32, end: f32) -> HeardWord {
        HeardWord {
            word: text.into(),
            conf,
            start,
            end,
        }
    }

    fn names() -> Vec<String> {
        vec!["лёха".into(), "лёх".into(), "алексей".into()]
    }

    #[test]
    fn grammar_has_both_spellings_and_unk() {
        let grammar = grammar(&names());
        assert!(grammar.contains(&"леха".to_owned()));
        assert!(grammar.contains(&"лёха".to_owned()));
        assert!(grammar.contains(&"алексей".to_owned()));
        assert_eq!(grammar.last().map(String::as_str), Some("[unk]"));
        let mut unique = grammar.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), grammar.len());
    }

    #[test]
    fn first_word_name_matches_in_a_long_segment() {
        let words = vec![
            word("лёха", 0.9, 0.2, 0.6),
            word("[unk]", 1.0, 0.7, 1.2),
            word("[unk]", 1.0, 1.2, 2.0),
        ];
        let found = evaluate(&words, &names(), 0.55, 2.5, 0.0).expect("match");
        assert_eq!(found.name, "лёха");
        assert!((found.end_seconds - 0.6).abs() < 1e-6);
    }

    #[test]
    fn yo_and_ye_spellings_are_equal() {
        let words = vec![word("леха", 0.8, 0.0, 0.4)];
        assert!(evaluate(&words, &names(), 0.55, 0.5, 0.0).is_some());
    }

    #[test]
    fn name_in_the_middle_of_a_long_segment_is_rejected() {
        let words = vec![
            word("[unk]", 1.0, 0.0, 0.5),
            word("[unk]", 1.0, 0.5, 1.0),
            word("лёха", 0.95, 1.0, 1.4),
            word("[unk]", 1.0, 1.4, 2.5),
        ];
        assert_eq!(evaluate(&words, &names(), 0.55, 2.8, 0.0), None);
    }

    #[test]
    fn lone_late_name_in_a_long_segment_is_rejected() {
        // Vosk dropped the unknown words: the name is the only result, but it starts 2 s into the speech.
        let words = vec![word("леха", 1.0, 2.28, 2.55)];
        assert_eq!(evaluate(&words, &names(), 0.55, 6.0, 0.3), None);
        let early = vec![word("леха", 1.0, 0.4, 0.8)];
        assert!(evaluate(&early, &names(), 0.55, 6.0, 0.3).is_some());
    }

    #[test]
    fn name_in_a_short_segment_matches_anywhere() {
        let words = vec![word("[unk]", 1.0, 0.0, 0.3), word("алексей", 0.9, 0.3, 0.9)];
        let found = evaluate(&words, &names(), 0.55, 1.2, 0.0).expect("match");
        assert_eq!(found.name, "алексей");
    }

    #[test]
    fn low_confidence_is_rejected_and_sensitivity_moves_the_bar() {
        let words = vec![word("лёха", 0.4, 0.0, 0.4)];
        assert_eq!(evaluate(&words, &names(), 0.55, 0.5, 0.0), None);
        assert!(evaluate(&words, &names(), 0.3, 0.5, 0.0).is_some());
    }

    #[test]
    fn unknown_only_and_empty_results_do_not_match() {
        assert_eq!(evaluate(&[], &names(), 0.1, 0.5, 0.0), None);
        let words = vec![word("[unk]", 1.0, 0.0, 0.5)];
        assert_eq!(evaluate(&words, &names(), 0.1, 0.5, 0.0), None);
    }
}
