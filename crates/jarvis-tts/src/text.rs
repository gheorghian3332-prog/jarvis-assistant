//! Text preparation: language detection, cleaning and splitting into sentences.

/// Language of a phrase; selects the engine chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}

/// Share of Cyrillic letters above which a phrase is Russian.
pub const CYRILLIC_SHARE: f32 = 0.30;
/// Sentences longer than this are cut at a comma or a space so the first audio is not delayed.
pub const MAX_SENTENCE_CHARS: usize = 220;

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}')
}

/// Russian when more than 30% of the letters are Cyrillic, English otherwise.
pub fn detect_lang(text: &str) -> Lang {
    let mut letters = 0usize;
    let mut cyrillic = 0usize;
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if is_cyrillic(c) {
            cyrillic += 1;
        }
    }
    if letters > 0 && cyrillic as f32 / letters as f32 > CYRILLIC_SHARE {
        Lang::Ru
    } else {
        Lang::En
    }
}

/// Removes markup characters that a voice must not read out; keeps line breaks.
pub fn clean_for_speech(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| match c {
            '*' | '#' | '`' | '~' | '|' | '<' | '>' | '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' => ' ',
            '\r' => '\n',
            other => other,
        })
        .collect();
    replaced
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Abbreviations (lower case, without the final dot) that do not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    // Russian
    "т.е", "т.д", "т.п", "т.к", "т.н", "т.о", "др", "пр", "г", "гг", "ул", "пл", "пр-т", "им",
    "руб", "коп", "тыс", "млн", "млрд", "трлн", "см", "рис", "стр", "табл", "проф", "акад", "доц",
    "д-р", "д", "кв", "корп", "оф", "эт", "ок", "напр", "англ", "рус", "хх", "в", "вв", "п", "пп",
    "ст", "обл", "р-н", "мин", "сек", "кг", "км", "тел", "гр", "с", "ред", "изд", "т", "тт", "ин",
    "им", "св", "ум", "зам", "нач", "гл", // English
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "a.m", "p.m",
    "u.s", "u.k", "no", "fig", "inc", "ltd", "co", "corp", "mt", "ave", "approx", "dept", "est",
    "gen", "col", "lt", "capt", "sgt", "rev", "hon", "jan", "feb", "mar", "apr", "jun", "jul",
    "aug", "sep", "sept", "oct", "nov", "dec",
];

/// Abbreviations that may also end a sentence when a capital letter follows.
const MAY_END_SENTENCE: &[&str] = &[
    "т.д", "т.п", "др", "пр", "etc", "руб", "коп", "тыс", "млн", "млрд", "no", "inc", "ltd", "co",
    "p.m", "a.m", "u.s", "u.k", "с", "гг", "кг", "км", "мин", "сек",
];

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

fn is_closing(c: char) -> bool {
    matches!(c, '"' | '\'' | '»' | '”' | '’' | ')' | ']')
}

fn is_opening(c: char) -> bool {
    matches!(
        c,
        '"' | '\'' | '«' | '“' | '‘' | '(' | '[' | '-' | '—' | '–'
    )
}

/// Decides whether the terminator run `chars[term..run_end]` closes a sentence.
fn is_boundary(chars: &[char], start: usize, term: usize, run_end: usize, after: usize) -> bool {
    let next = chars[after.min(chars.len())..]
        .iter()
        .copied()
        .find(|c| !c.is_whitespace());
    let Some(next) = next else {
        return true;
    };
    let run = &chars[term..run_end];
    if run != ['.'] {
        // "!" and "?" always end a sentence; "..." / "…" only before a new sentence.
        return if run.iter().all(|c| matches!(c, '.' | '…')) {
            next.is_uppercase() || next.is_ascii_digit() || is_opening(next)
        } else {
            true
        };
    }
    let word_start = chars[start..term]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(start, |position| start + position + 1);
    let word: String = chars[word_start..term]
        .iter()
        .collect::<String>()
        .trim_start_matches(['«', '“', '"', '\'', '(', '[', '‘'])
        .to_owned();
    if word.is_empty() {
        return false;
    }
    let letters: Vec<char> = word.chars().collect();
    if letters.len() == 1 && letters[0].is_alphabetic() && letters[0].is_uppercase() {
        // An initial ("А. С. Пушкин").
        return false;
    }
    let lower = word.to_lowercase();
    if ABBREVIATIONS.contains(&lower.as_str()) {
        return MAY_END_SENTENCE.contains(&lower.as_str()) && next.is_uppercase();
    }
    if letters.iter().all(|c| c.is_ascii_digit()) && word_start == start {
        // A list marker ("1. Первый пункт").
        return false;
    }
    if word.contains('.') && word.split('.').all(|part| part.chars().count() <= 1) {
        // "U.S.", "т.е." and similar dotted abbreviations.
        return false;
    }
    next.is_uppercase() || next.is_ascii_digit() || is_opening(next)
}

fn split_line(line: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < chars.len() {
        if !is_terminator(chars[index]) {
            index += 1;
            continue;
        }
        let mut run_end = index + 1;
        while run_end < chars.len() && is_terminator(chars[run_end]) {
            run_end += 1;
        }
        let mut after = run_end;
        while after < chars.len() && is_closing(chars[after]) {
            after += 1;
        }
        let spaced = after >= chars.len() || chars[after].is_whitespace();
        if spaced && is_boundary(&chars, start, index, run_end, after) {
            out.push(chars[start..after].iter().collect());
            start = after;
        }
        index = after.max(index + 1);
    }
    if start < chars.len() {
        out.push(chars[start..].iter().collect());
    }
}

/// Cuts a sentence longer than `max` characters at the last comma-like break (or space) before `max`.
fn wrap_long(sentence: &str, max: usize, out: &mut Vec<String>) {
    let mut rest: Vec<char> = sentence.chars().collect();
    while rest.len() > max {
        let window = &rest[..max];
        let cut = window
            .iter()
            .rposition(|c| matches!(c, ',' | ';' | ':'))
            .filter(|position| *position > max / 3)
            .map(|position| position + 1)
            .or_else(|| window.iter().rposition(|c| c.is_whitespace()))
            .filter(|position| *position > 0)
            .unwrap_or(max);
        let head: String = rest[..cut].iter().collect();
        out.push(head);
        rest = rest[cut..]
            .iter()
            .copied()
            .skip_while(|c| c.is_whitespace())
            .collect();
    }
    if !rest.is_empty() {
        out.push(rest.into_iter().collect());
    }
}

/// Splits text into sentences (Russian and English): line breaks and `. ! ? …` end a sentence
/// unless they belong to an abbreviation, an initial, a list number or a decimal number.
pub fn split_sentences(text: &str) -> Vec<String> {
    let cleaned = clean_for_speech(text);
    let mut raw = Vec::new();
    for line in cleaned.lines() {
        split_line(line, &mut raw);
    }
    let mut sentences = Vec::new();
    for sentence in raw {
        let trimmed = sentence.trim();
        if trimmed.chars().any(char::is_alphanumeric) {
            wrap_long(trimmed, MAX_SENTENCE_CHARS, &mut sentences);
        }
    }
    sentences
        .into_iter()
        .map(|sentence| sentence.trim().to_owned())
        .filter(|sentence| sentence.chars().any(char::is_alphanumeric))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_is_russian_above_thirty_percent_cyrillic() {
        assert_eq!(detect_lang("Привет, как дела?"), Lang::Ru);
        assert_eq!(detect_lang("Hello, how are you?"), Lang::En);
        assert_eq!(detect_lang("Открой Chrome и Telegram"), Lang::Ru);
        assert_eq!(detect_lang("Open Chrome и"), Lang::En);
        assert_eq!(detect_lang("12345 !!!"), Lang::En);
        assert_eq!(detect_lang(""), Lang::En);
        // 3 Cyrillic of 10 letters is exactly 30%: not above the threshold.
        assert_eq!(detect_lang("abcdefg але"), Lang::En);
        assert_eq!(detect_lang("abcdef алеш"), Lang::Ru);
    }

    #[test]
    fn splits_simple_russian_and_english() {
        assert_eq!(
            split_sentences("Привет. Как дела? Всё хорошо!"),
            ["Привет.", "Как дела?", "Всё хорошо!"]
        );
        assert_eq!(
            split_sentences("Good evening. All systems are running! Shall I continue?"),
            [
                "Good evening.",
                "All systems are running!",
                "Shall I continue?"
            ]
        );
    }

    #[test]
    fn abbreviations_do_not_end_sentences() {
        assert_eq!(
            split_sentences("Он купил хлеб, молоко и т.д. в магазине. Потом ушёл."),
            ["Он купил хлеб, молоко и т.д. в магазине.", "Потом ушёл."]
        );
        assert_eq!(
            split_sentences("Встреча на ул. Ленина, д. 5 состоится завтра. Приходите."),
            [
                "Встреча на ул. Ленина, д. 5 состоится завтра.",
                "Приходите."
            ]
        );
        assert_eq!(
            split_sentences("Mr. Smith met Dr. Jones at 5 p.m. yesterday. They talked."),
            [
                "Mr. Smith met Dr. Jones at 5 p.m. yesterday.",
                "They talked."
            ]
        );
        assert_eq!(
            split_sentences("Buy milk, eggs, etc. Then go home."),
            ["Buy milk, eggs, etc.", "Then go home."]
        );
        assert_eq!(
            split_sentences("Это стоит 5 руб. Дорого."),
            ["Это стоит 5 руб.", "Дорого."]
        );
    }

    #[test]
    fn initials_and_list_numbers_stay_attached() {
        assert_eq!(
            split_sentences("А. С. Пушкин родился в 1799 году. Он поэт."),
            ["А. С. Пушкин родился в 1799 году.", "Он поэт."]
        );
        assert_eq!(
            split_sentences("1. Открыть окно\n2. Закрыть дверь"),
            ["1. Открыть окно", "2. Закрыть дверь"]
        );
    }

    #[test]
    fn numbers_with_separators_are_not_split() {
        assert_eq!(
            split_sentences("Температура 36.6 градусов, курс 1,5 раза. Это норма."),
            ["Температура 36.6 градусов, курс 1,5 раза.", "Это норма."]
        );
        assert_eq!(
            split_sentences("Version 3.14.2 is out. Update now."),
            ["Version 3.14.2 is out.", "Update now."]
        );
        assert_eq!(
            split_sentences("В 2020. Потом в 2021."),
            ["В 2020.", "Потом в 2021."]
        );
    }

    #[test]
    fn ellipsis_quotes_and_lowercase_continuations() {
        assert_eq!(
            split_sentences("Ну... не знаю. Может быть?! Посмотрим."),
            ["Ну... не знаю.", "Может быть?!", "Посмотрим."]
        );
        assert_eq!(
            split_sentences("Он сказал: «Иду домой.» Потом ушёл."),
            ["Он сказал: «Иду домой.»", "Потом ушёл."]
        );
        assert_eq!(
            split_sentences("Wait… Really? Yes."),
            ["Wait…", "Really?", "Yes."]
        );
    }

    #[test]
    fn line_breaks_split_and_markup_is_removed() {
        assert_eq!(
            split_sentences("**Готово**\n\nФайл `a.txt` сохранён"),
            ["Готово", "Файл a.txt сохранён"]
        );
        assert!(split_sentences("   \n ... \n —").is_empty());
    }

    #[test]
    fn long_sentences_are_cut_at_commas_then_spaces() {
        let long = format!(
            "{}, {}.",
            "слово ".repeat(30).trim(),
            "слово ".repeat(30).trim()
        );
        let parts = split_sentences(&long);
        assert!(parts.len() >= 2);
        assert!(parts
            .iter()
            .all(|part| part.chars().count() <= MAX_SENTENCE_CHARS));
        let joined = parts.join(" ");
        assert_eq!(joined.split_whitespace().count(), 60);
        let no_spaces = "а".repeat(500);
        let hard = split_sentences(&no_spaces);
        assert_eq!(hard.len(), 3);
        assert_eq!(hard.concat(), no_spaces);
    }
}
