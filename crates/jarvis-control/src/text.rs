//! Text helpers: normalization, phonetic transliteration, fuzzy distance and spoken numbers.

/// Lowercase, ё -> е, every non-alphanumeric run becomes a single space.
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = true;
    for ch in input.chars().flat_map(char::to_lowercase) {
        let ch = if ch == 'ё' { 'е' } else { ch };
        if ch.is_alphanumeric() {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Value of a spoken number word (units, teens, tens, hundred) in ru or en.
pub fn number_word(word: &str) -> Option<u32> {
    let value = match word {
        "ноль" | "нуль" | "zero" => 0,
        "один" | "одна" | "одно" | "одну" | "one" => 1,
        "два" | "две" | "двух" | "two" => 2,
        "три" | "трех" | "three" => 3,
        "четыре" | "четырех" | "four" => 4,
        "пять" | "пяти" | "five" => 5,
        "шесть" | "шести" | "six" => 6,
        "семь" | "семи" | "seven" => 7,
        "восемь" | "восьми" | "eight" => 8,
        "девять" | "девяти" | "nine" => 9,
        "десять" | "десяти" | "ten" => 10,
        "одиннадцать" | "одиннадцати" | "eleven" => 11,
        "двенадцать" | "двенадцати" | "twelve" => 12,
        "тринадцать" | "тринадцати" | "thirteen" => 13,
        "четырнадцать" | "четырнадцати" | "fourteen" => 14,
        "пятнадцать" | "пятнадцати" | "fifteen" => 15,
        "шестнадцать" | "шестнадцати" | "sixteen" => 16,
        "семнадцать" | "семнадцати" | "seventeen" => 17,
        "восемнадцать" | "восемнадцати" | "eighteen" => 18,
        "девятнадцать" | "девятнадцати" | "nineteen" => 19,
        "двадцать" | "двадцати" | "twenty" => 20,
        "тридцать" | "тридцати" | "thirty" => 30,
        "сорок" | "сорока" | "forty" => 40,
        "пятьдесят" | "пятидесяти" | "fifty" => 50,
        "шестьдесят" | "шестидесяти" | "sixty" => 60,
        "семьдесят" | "семидесяти" | "seventy" => 70,
        "восемьдесят" | "восьмидесяти" | "eighty" => 80,
        "девяносто" | "девяноста" | "ninety" => 90,
        "сто" | "ста" | "hundred" => 100,
        _ => return None,
    };
    Some(value)
}

/// First number in a normalized text: digits or words ("тридцать пять", "twenty five", "42").
pub fn extract_number(normalized: &str) -> Option<u32> {
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token.chars().all(|c| c.is_ascii_digit()) && !token.is_empty() {
            return token.parse().ok();
        }
        if let Some(first) = number_word(token) {
            let mut total = first;
            if (20..100).contains(&first) {
                if let Some(next) = tokens.get(index + 1).and_then(|t| number_word(t)) {
                    if (1..10).contains(&next) {
                        total += next;
                    }
                }
            } else if (1..10).contains(&first) && tokens.get(index + 1) == Some(&"hundred") {
                total = first * 100;
            }
            return Some(total);
        }
        index += 1;
    }
    None
}

/// A spoken fraction as a percentage: "половина" 50, "две трети" 67, "три четверти" 75, "a third" 33,
/// "three quarters" 75. The numerator is the number word right before the fraction word (default 1).
/// Digit fractions ("2/3") are rewritten before normalization, see [`rewrite_digit_fractions`].
pub fn fraction_percent(normalized: &str) -> Option<u32> {
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        let numerator = || {
            index
                .checked_sub(1)
                .and_then(|before| tokens.get(before))
                .and_then(|word| number_word(word))
                .filter(|n| (1..=9).contains(n))
                .unwrap_or(1)
        };
        match *token {
            "половина" | "половину" | "половины" | "половине" | "наполовину" | "half" => {
                return Some(50)
            }
            "треть" | "трети" | "третей" | "третью" | "third" | "thirds" => {
                let n = numerator();
                return (n < 3).then(|| (n * 100 + 1) / 3);
            }
            "четверть" | "четверти" | "четвертей" | "четвертью" | "quarter" | "quarters" =>
            {
                let n = numerator();
                return (n < 4).then_some(n * 25);
            }
            _ => {}
        }
    }
    None
}

/// "2/3" -> "67 процентов" (a fraction of a whole, denominator 2..=10), so that normalization does
/// not split it into two unrelated numbers. Other text is left alone.
pub fn rewrite_digit_fractions(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_ascii_digit() && (index == 0 || !chars[index - 1].is_ascii_digit()) {
            let mut end = index;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if chars.get(end) == Some(&'/') {
                let mut den_end = end + 1;
                while den_end < chars.len() && chars[den_end].is_ascii_digit() {
                    den_end += 1;
                }
                let numerator: String = chars[index..end].iter().collect();
                let denominator: String = chars[end + 1..den_end].iter().collect();
                if let (Ok(n), Ok(d)) = (numerator.parse::<u32>(), denominator.parse::<u32>()) {
                    let separated = chars
                        .get(den_end)
                        .is_none_or(|c| !c.is_ascii_digit() && *c != '/');
                    if (2..=10).contains(&d) && n >= 1 && n < d && separated {
                        out.push_str(&format!("{} процентов", (n * 100 + d / 2) / d));
                        index = den_end;
                        continue;
                    }
                }
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn roman_value(token: &str) -> Option<u32> {
    match token {
        "ii" => Some(2),
        "iii" => Some(3),
        "iv" => Some(4),
        "vi" => Some(6),
        "vii" => Some(7),
        "viii" => Some(8),
        "ix" => Some(9),
        _ => None,
    }
}

/// Normalization used for matching names: splits letter/digit boundaries ("dota2" -> "dota 2"),
/// turns roman numerals and number words 1..10 into digits ("III" / "три" -> "3").
pub fn canonical(input: &str) -> String {
    let normalized = normalize(input);
    let mut spaced = String::with_capacity(normalized.len() + 4);
    let mut previous: Option<char> = None;
    for ch in normalized.chars() {
        if let Some(prev) = previous {
            let boundary = (prev.is_ascii_digit() && ch.is_alphabetic())
                || (prev.is_alphabetic() && ch.is_ascii_digit());
            if boundary {
                spaced.push(' ');
            }
        }
        spaced.push(ch);
        previous = Some(ch);
    }
    spaced
        .split_whitespace()
        .map(|token| {
            if let Some(value) = roman_value(token) {
                return value.to_string();
            }
            match number_word(token) {
                Some(value) if (1..=10).contains(&value) => value.to_string(),
                _ => token.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cyrillic_phonetic(ch: char) -> Option<&'static str> {
    Some(match ch {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'ё' => "e",
        'ж' => "zh",
        'з' => "z",
        'и' | 'ы' | 'й' => "i",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' | 'ю' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "ts",
        'ч' => "ch",
        'ш' | 'щ' => "sh",
        'э' => "e",
        'я' => "a",
        'ъ' | 'ь' => "",
        _ => return None,
    })
}

/// Rough phonetic key shared by Russian and English spellings ("дискорд" and "discord" -> "diskord").
pub fn phonetic(input: &str) -> String {
    let chars: Vec<char> = canonical(input).chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        if let Some(mapped) = cyrillic_phonetic(ch) {
            out.push_str(mapped);
        } else {
            match ch {
                'c' => match next {
                    Some('e' | 'i' | 'y') => out.push('s'),
                    Some('h') => {
                        out.push_str("ch");
                        index += 1;
                    }
                    Some('k') => {
                        out.push('k');
                        index += 1;
                    }
                    _ => out.push('k'),
                },
                'p' if next == Some('h') => {
                    out.push('f');
                    index += 1;
                }
                'e' if matches!(next, Some('a' | 'e')) => {
                    out.push('i');
                    index += 1;
                }
                'q' => out.push('k'),
                'w' => out.push('v'),
                'x' => out.push_str("ks"),
                'y' => out.push('i'),
                other if other.is_ascii() => out.push(other),
                _ => {}
            }
        }
        index += 1;
    }
    let mut collapsed = String::with_capacity(out.len());
    for ch in out.chars() {
        if ch.is_ascii_alphabetic() && collapsed.ends_with(ch) {
            continue;
        }
        collapsed.push(ch);
    }
    collapsed
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[b.len()]
}

/// 1.0 = identical, 0.0 = nothing in common.
pub fn similarity(a: &str, b: &str) -> f64 {
    let longest = a.chars().count().max(b.chars().count());
    if longest == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f64 / longest as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(
            normalize("  Лёха,  ОТКРОЙ!! Discord... "),
            "леха открой discord"
        );
        assert_eq!(normalize("Dungeon Quest™ III"), "dungeon quest iii");
    }

    #[test]
    fn numbers_from_words_and_digits() {
        assert_eq!(extract_number("тридцать процентов"), Some(30));
        assert_eq!(extract_number("до двадцати пяти"), Some(25));
        assert_eq!(extract_number("на 45 процентов"), Some(45));
        assert_eq!(extract_number("громкость сто"), Some(100));
        assert_eq!(extract_number("volume to twenty five percent"), Some(25));
        assert_eq!(extract_number("one hundred"), Some(100));
        assert_eq!(extract_number("пятнадцать"), Some(15));
        assert_eq!(extract_number("ничего"), None);
    }

    #[test]
    fn fractions_are_percentages() {
        assert_eq!(fraction_percent("громкость на две трети"), Some(67));
        assert_eq!(fraction_percent("на половину"), Some(50));
        assert_eq!(fraction_percent("на треть"), Some(33));
        assert_eq!(fraction_percent("на одну треть"), Some(33));
        assert_eq!(fraction_percent("на четверть"), Some(25));
        assert_eq!(fraction_percent("на три четверти"), Some(75));
        assert_eq!(fraction_percent("до двух третей"), Some(67));
        assert_eq!(fraction_percent("to half"), Some(50));
        assert_eq!(fraction_percent("by two thirds"), Some(67));
        assert_eq!(fraction_percent("three quarters"), Some(75));
        assert_eq!(fraction_percent("на 30 процентов"), None);
        assert_eq!(fraction_percent("три трети"), None);
    }

    #[test]
    fn digit_fractions_are_rewritten() {
        assert_eq!(
            rewrite_digit_fractions("громкость на 2/3"),
            "громкость на 67 процентов"
        );
        assert_eq!(rewrite_digit_fractions("volume 1/2"), "volume 50 процентов");
        assert_eq!(rewrite_digit_fractions("3/4"), "75 процентов");
        // not a fraction of a whole
        assert_eq!(rewrite_digit_fractions("15/11"), "15/11");
        assert_eq!(rewrite_digit_fractions("5/2"), "5/2");
        assert_eq!(rewrite_digit_fractions("12/05/2026"), "12/05/2026");
    }

    #[test]
    fn canonical_unifies_numerals() {
        assert_eq!(canonical("DUNGEON QUEST III"), "dungeon quest 3");
        assert_eq!(canonical("данжен квест три"), "данжен квест 3");
        assert_eq!(canonical("Dota2"), "dota 2");
    }

    #[test]
    fn phonetic_bridges_scripts() {
        assert_eq!(phonetic("дискорд"), phonetic("discord"));
        assert_eq!(phonetic("телеграм"), phonetic("telegram"));
        assert_eq!(phonetic("стим"), phonetic("steam"));
        assert_eq!(phonetic("обсидиан"), phonetic("obsidian"));
        assert_eq!(phonetic("дота 2"), phonetic("Dota II"));
    }

    #[test]
    fn similarity_scale() {
        assert!(similarity("kitten", "kitten") > 0.99);
        assert!(similarity("hrom", "chrom") >= 0.8);
        assert!(similarity("abc", "xyz") < 0.1);
    }
}
