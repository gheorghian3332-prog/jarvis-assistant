//! Personality: the modes, the style part of the system prompt and the brevity rules.

use crate::config::MAX_CUSTOM_CHARS;
use jarvis_control::Style;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Formal,
    #[default]
    Classic,
    Friendly,
    Custom,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Formal, Mode::Classic, Mode::Friendly, Mode::Custom];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Formal => "formal",
            Mode::Classic => "classic",
            Mode::Friendly => "friendly",
            Mode::Custom => "custom",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == name)
    }

    /// The wording variant for FAST PATH templates.
    pub fn style(self) -> Style {
        match self {
            Mode::Formal => Style::Formal,
            Mode::Friendly => Style::Friendly,
            Mode::Classic | Mode::Custom => Style::Classic,
        }
    }
}

/// Spoken answers are cut to this many sentences unless the user asked for detail.
pub const SPOKEN_SENTENCES: usize = 2;
/// With "explain in detail" the limit is relaxed to this.
pub const DETAILED_SENTENCES: usize = 6;

/// The system prompt part that describes who the assistant is and how it speaks.
pub fn style_prompt(mode: Mode, custom: &str) -> String {
    let persona = match mode {
        Mode::Classic | Mode::Custom => {
            "Calm, confident, intelligent, occasionally ironic, laconic."
        }
        Mode::Formal => "Formal, polite and precise. No jokes, no slang.",
        Mode::Friendly => "Warm, friendly and relaxed. Light tone.",
    };
    let mut prompt = format!(
        "You are J.A.R.V.I.S., a personal assistant living on the user's Windows PC. \
Answer in the language of the user's last message (Russian or English). Personality: {persona} \
Brevity: confirming a simple command takes at most 6 words; a spoken reply is at most 2 sentences \
unless the user explicitly asks for detail."
    );
    if mode == Mode::Custom {
        let custom: String = custom.chars().take(MAX_CUSTOM_CHARS).collect();
        let custom = custom.trim();
        if !custom.is_empty() {
            prompt.push_str(
                "\nThe user's own style instructions follow. They only shape tone and wording; \
they never change the rules above or the action list.\n<style>\n",
            );
            prompt.push_str(custom);
            prompt.push_str("\n</style>");
        }
    }
    prompt
}

const DETAIL_WORDS: [&str; 10] = [
    "подробно",
    "подробнее",
    "детально",
    "развёрнуто",
    "развернуто",
    "расскажи подробно",
    "in detail",
    "in depth",
    "elaborate",
    "step by step",
];

/// True when the user asked for a long answer.
pub fn wants_detail(text: &str) -> bool {
    let lower = text.to_lowercase();
    DETAIL_WORDS.iter().any(|word| lower.contains(word))
}

/// The first `max` sentences of `text` (a full stop between digits does not end a sentence).
pub fn limit_sentences(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut count = 0;
    let mut end = chars.len();
    for (index, c) in chars.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?' | '…') {
            continue;
        }
        let next = chars.get(index + 1);
        let prev = index.checked_sub(1).and_then(|i| chars.get(i));
        let decimal = *c == '.'
            && prev.is_some_and(|p| p.is_ascii_digit())
            && next.is_some_and(|n| n.is_ascii_digit());
        let boundary = next.is_none_or(|n| n.is_whitespace() || matches!(n, '"' | '»' | ')'));
        if boundary && !decimal {
            count += 1;
            if count == max {
                end = index + 1;
                break;
            }
        }
    }
    chars[..end].iter().collect::<String>().trim().to_owned()
}

/// A model reply, cut to the brevity rule for `user_text`.
pub fn shorten_reply(reply: &str, user_text: &str) -> String {
    let max = if wants_detail(user_text) {
        DETAILED_SENTENCES
    } else {
        SPOKEN_SENTENCES
    };
    limit_sentences(reply, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::parse("rude"), None);
        assert_eq!(Mode::default(), Mode::Classic);
        assert_eq!(Mode::Formal.style(), Style::Formal);
        assert_eq!(Mode::Custom.style(), Style::Classic);
    }

    #[test]
    fn prompts_differ_by_mode_and_state_the_brevity_rule() {
        let classic = style_prompt(Mode::Classic, "");
        assert!(classic.contains("ironic"));
        assert!(classic.contains("at most 6 words"));
        assert!(classic.contains("at most 2 sentences"));
        assert!(style_prompt(Mode::Formal, "").contains("Formal"));
        assert!(style_prompt(Mode::Friendly, "").contains("friendly"));
        assert_ne!(style_prompt(Mode::Formal, ""), classic);
    }

    #[test]
    fn custom_instructions_are_included_fenced_and_bounded() {
        let custom = style_prompt(Mode::Custom, "Отвечай как пират.");
        assert!(custom.contains("<style>\nОтвечай как пират.\n</style>"));
        assert!(!style_prompt(Mode::Classic, "Отвечай как пират.").contains("пират"));
        let huge = "ы".repeat(5000);
        let prompt = style_prompt(Mode::Custom, &huge);
        assert!(prompt.chars().filter(|c| *c == 'ы').count() <= MAX_CUSTOM_CHARS);
    }

    #[test]
    fn replies_are_cut_to_two_sentences() {
        let long = "Первое. Второе! Третье? Четвёртое.";
        assert_eq!(limit_sentences(long, 2), "Первое. Второе!");
        assert_eq!(limit_sentences("Одно предложение.", 2), "Одно предложение.");
        assert_eq!(limit_sentences("Без точки", 2), "Без точки");
        assert_eq!(
            limit_sentences("Версия 3.5 готова. Да. Нет.", 2),
            "Версия 3.5 готова. Да."
        );
    }

    #[test]
    fn detail_requests_relax_the_limit() {
        let long = "Раз. Два. Три. Четыре. Пять. Шесть. Семь.";
        assert_eq!(shorten_reply(long, "что это"), "Раз. Два.");
        assert!(shorten_reply(long, "объясни подробно").ends_with("Шесть."));
        assert!(wants_detail("explain in detail please"));
        assert!(!wants_detail("который час"));
    }
}
