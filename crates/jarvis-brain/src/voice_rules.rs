//! Voice phrases about the assistant itself: "remember that ...", "what do you remember about me",
//! "forget ...", "unload the model". They become registry actions (`memory.*`, `ai.unload`), so the
//! executor still validates them and asks for confirmation where the risk says so.

use crate::memory::{KINDS, MAX_KEY_CHARS, MAX_VALUE_CHARS};
use jarvis_control::Intent;
use jarvis_core::{memory::words, Database};
use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

/// What a recognized phrase asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// Run this action through the executor.
    Act(Intent),
    /// Answer this text and do nothing else.
    Say(String),
}

struct Patterns {
    address: Regex,
    remember_ru: Regex,
    remember_en: Regex,
    recall: Regex,
    forget_ru: Regex,
    forget_en: Regex,
    unload: Regex,
    split_ru: Regex,
    split_en: Regex,
    path: Regex,
}

fn patterns() -> Option<&'static Patterns> {
    static CELL: OnceLock<Option<Patterns>> = OnceLock::new();
    CELL.get_or_init(|| {
        Some(Patterns {
            address: Regex::new(
                r"(?i)^\s*(?:(?:леха|лёха|леша|алексей|джарвис|jarvis|эй|hey|пожалуйста|please|слушай|ну)[\s,]+)+",
            )
            .ok()?,
            remember_ru: Regex::new(r"(?is)^\s*запомни[\s,:]+(?:что\s+)?(.+?)\s*$").ok()?,
            remember_en: Regex::new(r"(?is)^\s*remember[\s,:]+(?:that\s+)?(.+?)\s*$").ok()?,
            recall: Regex::new(
                r"(?i)^\s*(?:что\s+ты\s+(?:обо\s+мне\s+)?(?:помнишь|знаешь)(?:\s+обо\s+мне)?|что\s+ты\s+помнишь|what\s+do\s+you\s+(?:remember|know)(?:\s+about\s+me)?|what\s+(?:have\s+you|did\s+you)\s+(?:remembered|remember))\s*[?.!]*\s*$",
            )
            .ok()?,
            forget_ru: Regex::new(r"(?is)^\s*забудь[\s,:]+(?:что\s+)?(.+?)\s*$").ok()?,
            forget_en: Regex::new(r"(?is)^\s*forget[\s,:]+(?:that\s+|about\s+)?(.+?)\s*$").ok()?,
            unload: Regex::new(
                r"(?i)^\s*(?:выгруз(?:и|ить)\s+(?:языков\w+\s+)?модел\w*|unload\s+(?:the\s+)?(?:language\s+)?model)\s*[.!]*\s*$",
            )
            .ok()?,
            split_ru: Regex::new(
                r"(?is)^(.+?)\s+(?:лежит\s+в|лежат\s+в|находится\s+в|находятся\s+в|хранится\s+в|это|называется|зовут|—|=|-)\s+(.+)$",
            )
            .ok()?,
            split_en: Regex::new(
                r"(?is)^(.+?)\s+(?:is\s+located\s+in|is\s+in|lives\s+in|is\s+called|is|are|=)\s+(.+)$",
            )
            .ok()?,
            path: Regex::new(r"[A-Za-z]:\\").ok()?,
        })
    })
    .as_ref()
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The kind a fact most likely has.
fn guess_kind(sentence: &str, patterns_path: &Regex) -> &'static str {
    let lower = sentence.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if patterns_path.is_match(sentence)
        || has(&["проект", "project", "папк", "folder", "репозитор"])
    {
        "project"
    } else if has(&[
        "люблю",
        "нравится",
        "предпочитаю",
        "любим",
        "i like",
        "i prefer",
        "my favorite",
        "favourite",
    ]) {
        "preference"
    } else if has(&[
        "наушник",
        "колонк",
        "монитор",
        "микрофон",
        "принтер",
        "устройств",
        "headphones",
        "speaker",
        "monitor",
        "device",
    ]) {
        "device"
    } else if has(&[
        "когда я говорю",
        "сценари",
        "when i say",
        "routine",
        "scenario",
    ]) {
        "scenario"
    } else {
        "note"
    }
}

/// Splits "мой проект лежит в E:\x" into a key and a value; falls back to the first words and the
/// whole sentence.
fn key_value(sentence: &str, patterns: &Patterns) -> (String, String) {
    let sentence = sentence.trim().trim_end_matches(['.', '!']);
    for split in [&patterns.split_ru, &patterns.split_en] {
        if let Some(captures) = split.captures(sentence) {
            let key = captures.get(1).map_or("", |m| m.as_str()).trim();
            let value = captures.get(2).map_or("", |m| m.as_str()).trim();
            if !key.is_empty() && !value.is_empty() && key.chars().count() <= MAX_KEY_CHARS {
                return (key.to_owned(), truncate(value, MAX_VALUE_CHARS));
            }
        }
    }
    let key: String = sentence
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    (
        truncate(&key, MAX_KEY_CHARS),
        truncate(sentence, MAX_VALUE_CHARS),
    )
}

fn remember_intent(sentence: &str, patterns: &Patterns) -> Rule {
    let (key, value) = key_value(sentence, patterns);
    let kind = guess_kind(sentence, &patterns.path);
    debug_assert!(KINDS.contains(&kind));
    Rule::Act(Intent::new(
        "memory.remember",
        json!({"kind": kind, "key": key, "value": value}),
    ))
}

/// Recognizes the phrases that need no database (remember, recall, unload). `forget` is
/// recognized by [`forget_target`] because it must look the fact up.
pub fn parse(text: &str) -> Option<Rule> {
    let p = patterns()?;
    let stripped = p.address.replace(text, "");
    let t = stripped.trim();
    if p.unload.is_match(t) {
        return Some(Rule::Act(Intent::new("ai.unload", json!({}))));
    }
    if p.recall.is_match(t) {
        return Some(Rule::Act(Intent::new("memory.recall", json!({}))));
    }
    for remember in [&p.remember_ru, &p.remember_en] {
        if let Some(captures) = remember.captures(t) {
            let sentence = captures.get(1).map_or("", |m| m.as_str());
            if !sentence.trim().is_empty() {
                return Some(remember_intent(sentence, p));
            }
        }
    }
    None
}

/// The words after "forget ..." when the phrase is a forget request.
pub fn forget_target(text: &str) -> Option<String> {
    let p = patterns()?;
    let stripped = p.address.replace(text, "");
    let t = stripped.trim();
    for forget in [&p.forget_ru, &p.forget_en] {
        if let Some(captures) = forget.captures(t) {
            return captures.get(1).map(|m| m.as_str().trim().to_owned());
        }
    }
    None
}

fn is_everything(target: &str) -> bool {
    let normalized = jarvis_control::text::normalize(target);
    matches!(
        normalized.as_str(),
        "все" | "всё" | "все что я говорил" | "everything" | "all" | "everything about me"
    )
}

/// Resolves a forget request against the stored facts.
pub async fn forget(target: &str, db: &Database, russian: bool) -> Rule {
    let say = |ru: &str, en: &str| Rule::Say(if russian { ru } else { en }.to_owned());
    if is_everything(target) {
        return say(
            "Всю память удаляют на вкладке PRIVACY, с подтверждением.",
            "All memory is deleted on the PRIVACY tab, with a confirmation.",
        );
    }
    if words(target, 2).is_empty() {
        return say("Что именно забыть?", "What should I forget?");
    }
    match db.fact_list(Some(target.to_owned()), 1).await {
        Ok(found) => match found.first() {
            Some(fact) => Rule::Act(Intent::new(
                "memory.forget",
                json!({
                    "id": fact.id,
                    "preview": truncate(&format!("{}: {}", fact.key, fact.value), 200),
                }),
            )),
            None => say("Такого я не помню.", "I do not remember that."),
        },
        Err(error) => {
            tracing::warn!(%error, "memory lookup failed");
            say(
                "Память сейчас недоступна.",
                "Memory is unavailable right now.",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_control::{parse as fast_parse, Lang};

    fn act(text: &str) -> Intent {
        match parse(text) {
            Some(Rule::Act(intent)) => intent,
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn the_spec_phrases_are_not_taken_by_the_fast_path() {
        for text in [
            r"запомни, что мой проект лежит в C:\Users\example\jarvis",
            "что ты обо мне помнишь?",
            "забудь мой проект",
            "выгрузи модель",
            "remember that my favorite music is jazz",
            "what do you remember about me",
            "unload the model",
        ] {
            assert_eq!(fast_parse(text, Lang::Ru), None, "{text}");
            assert_eq!(fast_parse(text, Lang::En), None, "{text}");
        }
    }

    #[test]
    fn remember_keeps_the_path_exactly() {
        let intent = act(r"запомни, что мой проект лежит в C:\Users\example\jarvis");
        assert_eq!(intent.action, "memory.remember");
        assert_eq!(intent.args["kind"], "project");
        assert_eq!(intent.args["key"], "мой проект");
        assert_eq!(intent.args["value"], r"C:\Users\example\jarvis");
    }

    #[test]
    fn remember_in_english_and_with_an_address() {
        let intent = act("Jarvis, remember that my favorite music is jazz");
        assert_eq!(intent.args["kind"], "preference");
        assert_eq!(intent.args["key"], "my favorite music");
        assert_eq!(intent.args["value"], "jazz");
        let plain = act("запомни я люблю чай");
        assert_eq!(plain.args["kind"], "preference");
        assert_eq!(plain.args["value"], "я люблю чай");
    }

    #[test]
    fn recall_and_unload() {
        assert_eq!(act("что ты обо мне помнишь?").action, "memory.recall");
        assert_eq!(act("лёха, что ты помнишь").action, "memory.recall");
        assert_eq!(
            act("what do you remember about me?").action,
            "memory.recall"
        );
        assert_eq!(act("выгрузи модель").action, "ai.unload");
        assert_eq!(act("Unload the model.").action, "ai.unload");
        assert_eq!(parse("какая погода"), None);
        assert_eq!(parse("запомни"), None);
    }

    #[test]
    fn forget_targets() {
        assert_eq!(
            forget_target("забудь мой проект").as_deref(),
            Some("мой проект")
        );
        assert_eq!(
            forget_target("лёха, забудь что я люблю чай").as_deref(),
            Some("я люблю чай")
        );
        assert_eq!(forget_target("forget about jazz").as_deref(), Some("jazz"));
        assert_eq!(forget_target("открой блокнот"), None);
    }

    #[tokio::test]
    async fn forget_finds_the_fact_or_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path().join("t.db")).expect("db");
        db.fact_upsert(
            "project".into(),
            "мой проект".into(),
            r"C:\Users\example\jarvis".into(),
        )
        .await
        .expect("fact");
        let Rule::Act(intent) = forget("мой проект", &db, true).await else {
            panic!("expected an action");
        };
        assert_eq!(intent.action, "memory.forget");
        assert!(intent.args["id"].as_i64().is_some());
        assert!(intent.args["preview"]
            .as_str()
            .expect("preview")
            .contains("jarvis"));
        assert!(matches!(forget("погоду", &db, true).await, Rule::Say(_)));
        assert!(
            matches!(forget("всё", &db, true).await, Rule::Say(text) if text.contains("PRIVACY"))
        );
        assert!(matches!(forget("a", &db, false).await, Rule::Say(_)));
    }
}
