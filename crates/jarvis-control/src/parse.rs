//! FAST PATH: rule-based ru/en command parser. No LLM, no network. Unknown phrases -> `None`.

use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::{json, Value};

use crate::action::{Intent, Lang};
use crate::files::{drive_letter, known_folder_key};
use crate::text::{extract_number, fraction_percent, normalize, rewrite_digit_fractions};

type Build = fn(&Captures, &str) -> Option<Intent>;

struct RuleDef {
    pattern: &'static str,
    build: Build,
}

struct Rule {
    regex: Regex,
    build: Build,
}

struct Compiled {
    ru: Vec<Rule>,
    en: Vec<Rule>,
    address: Option<Regex>,
    suffix: Option<Regex>,
    delete: Option<Regex>,
    defined: usize,
    compiled: usize,
}

fn intent(action: &str, args: Value) -> Option<Intent> {
    Some(Intent::new(action, args))
}

fn simple(action: &str) -> Option<Intent> {
    intent(action, json!({}))
}

fn compile(defs: &[RuleDef], counters: &mut (usize, usize)) -> Vec<Rule> {
    let mut rules = Vec::with_capacity(defs.len());
    for def in defs {
        counters.0 += 1;
        if let Ok(regex) = Regex::new(def.pattern) {
            counters.1 += 1;
            rules.push(Rule {
                regex,
                build: def.build,
            });
        }
    }
    rules
}

fn compiled() -> &'static Compiled {
    static CELL: OnceLock<Compiled> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut counters = (0usize, 0usize);
        let ru = compile(&ru_rules(), &mut counters);
        let en = compile(&en_rules(), &mut counters);
        let mut extra = |pattern: &str| {
            counters.0 += 1;
            let regex = Regex::new(pattern).ok();
            if regex.is_some() {
                counters.1 += 1;
            }
            regex
        };
        let address = extra(
            r"^(?:(?:леха|леша|лешка|алексей|алекс|джарвис|jarvis|эй|hey|ok|okay|окей|слушай|ну|слышь|давай|пожалуйста|please|можешь|можете|ты|вы|будь добр|будь добра|а ну|could you|can you|would you|will you|а|и)\s+)+",
        );
        let suffix = extra(r"(?:\s+(?:пожалуйста|please|плиз|for me|мне))+$");
        let delete = extra(
            r#"(?i)\b(?:удали|удалить|delete|remove)\s+(?:(?:файл|file|папку|folder)\s+)?"?([A-Za-z]:\\[^"<>|?*\r\n]+?)"?(?:\s+(?:в корзину|to (?:the )?recycle bin))?\s*$"#,
        );
        Compiled {
            ru,
            en,
            address,
            suffix,
            delete,
            defined: counters.0,
            compiled: counters.1,
        }
    })
}

/// (patterns defined, patterns that compiled). Equal unless a pattern is broken.
pub fn rule_health() -> (usize, usize) {
    let compiled = compiled();
    (compiled.defined, compiled.compiled)
}

/// Normalized text without the leading address ("лёха, ...", "алексей ...") and polite fillers.
pub fn clean(text: &str) -> String {
    let compiled = compiled();
    let mut t = normalize(&rewrite_digit_fractions(text));
    if let Some(address) = &compiled.address {
        t = address.replace(&t, "").into_owned();
    }
    if let Some(suffix) = &compiled.suffix {
        t = suffix.replace(&t, "").into_owned();
    }
    t.trim().to_owned()
}

/// Parses a phrase into an intent. `lang` only decides which rule set is tried first.
pub fn parse(text: &str, lang: Lang) -> Option<Intent> {
    let compiled = compiled();
    let t = clean(text);
    if t.is_empty() {
        return None;
    }
    if let Some(found) = parse_delete(text) {
        return Some(found);
    }
    let (first, second) = match lang {
        Lang::Ru => (&compiled.ru, &compiled.en),
        Lang::En => (&compiled.en, &compiled.ru),
    };
    for rules in [first, second] {
        for rule in rules {
            if let Some(captures) = rule.regex.captures(&t) {
                if let Some(found) = (rule.build)(&captures, &t) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Delete needs the original path spelling (case and backslashes), so it works on the raw text.
fn parse_delete(raw: &str) -> Option<Intent> {
    let regex = compiled().delete.as_ref()?;
    let captures = regex.captures(raw.trim())?;
    let path = captures.get(1)?.as_str().trim();
    intent("files.delete", json!({ "path": path }))
}

// ---------- shared builders ----------

fn clamp_level(value: u32) -> Option<u32> {
    (value <= 100).then_some(value)
}

fn has_word(t: &str, words: &[&str]) -> bool {
    t.split_whitespace().any(|token| words.contains(&token))
}

/// Volume with an explicit number: "до N" sets, "на N" with a direction verb changes, otherwise sets.
fn volume_with_number(
    t: &str,
    up_words: &[&str],
    down_words: &[&str],
    to: &str,
    by: &str,
) -> Option<Intent> {
    // A fraction ("на две трети", "to half") is a share of the whole scale, not the number inside it.
    let number = fraction_percent(t).or_else(|| extract_number(t))?;
    let up = has_word(t, up_words);
    let down = has_word(t, down_words);
    let by_word = has_word(t, &[by]) || t.contains("наполовину");
    let relative = (up || down) && by_word && !has_word(t, &[to]);
    if relative {
        let value = i32::try_from(clamp_level(number)?).ok()?;
        let delta = if down { -value } else { value };
        if delta == 0 {
            return None;
        }
        return intent("audio.change_volume", json!({ "delta": delta }));
    }
    intent("audio.set_volume", json!({ "level": clamp_level(number)? }))
}

fn change(delta: i32) -> Option<Intent> {
    intent("audio.change_volume", json!({ "delta": delta }))
}

fn media(key: &str) -> Option<Intent> {
    intent("audio.media", json!({ "key": key }))
}

fn app(query: &str) -> Option<Intent> {
    intent("app.open", json!({ "query": query.trim() }))
}

fn find_folder(t: &str) -> Option<&'static str> {
    let tokens: Vec<&str> = t.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        if let Some(next) = tokens.get(index + 1) {
            if let Some(key) = known_folder_key(&format!("{token} {next}")) {
                return Some(key);
            }
        }
        if let Some(key) = known_folder_key(token) {
            return Some(key);
        }
    }
    None
}

/// Known folder named in the phrase. Music/pictures/videos need the word "папка/folder" so that
/// "открой музыку" is not mistaken for a folder request.
fn open_folder(t: &str) -> Option<Intent> {
    let key = find_folder(t)?;
    let explicit = t.contains("папк") || t.contains("folder");
    if !explicit && !matches!(key, "downloads" | "documents" | "desktop") {
        return None;
    }
    intent("files.open_folder", json!({ "name": key }))
}

fn list_folder(t: &str) -> Option<Intent> {
    let key = find_folder(t)?;
    intent("files.list", json!({ "name": key }))
}

fn open_drive(captures: &Captures) -> Option<Intent> {
    let letter = drive_letter(captures.get(1)?.as_str())?;
    intent(
        "files.open_folder",
        json!({ "path": format!("{letter}:\\") }),
    )
}

const NOT_APPS: [&str; 14] = [
    "музык",
    "звук",
    "песн",
    "радио",
    "видео",
    "фильм",
    "сериал",
    "свет",
    "воду",
    "окно",
    "дверь",
    "music",
    "sound",
    "the music",
];

/// Generic "open X": rejects obviously non-app objects and long free-form sentences.
fn open_app(raw_query: &str) -> Option<Intent> {
    let query = raw_query
        .trim()
        .trim_start_matches("приложение ")
        .trim_start_matches("программу ")
        .trim_start_matches("игру ")
        .trim_start_matches("the ")
        .trim_start_matches("app ")
        .trim_start_matches("game ")
        .trim();
    if query.is_empty() || query.split_whitespace().count() > 5 {
        return None;
    }
    if NOT_APPS.iter().any(|bad| query.starts_with(bad)) {
        return None;
    }
    app(query)
}

const NOT_WINDOW_TARGETS: [&str; 10] = [
    "окно",
    "это окно",
    "программу",
    "приложение",
    "все",
    "всё",
    "все окна",
    "window",
    "this window",
    "all",
];

fn window_action(action: &str, raw_query: &str) -> Option<Intent> {
    let query = raw_query
        .trim()
        .trim_start_matches("окно ")
        .trim_start_matches("приложение ")
        .trim_start_matches("программу ")
        .trim_start_matches("вкладку ")
        .trim_start_matches("window ")
        .trim_start_matches("the ")
        .trim();
    if query.is_empty()
        || NOT_WINDOW_TARGETS.contains(&query)
        || query.split_whitespace().count() > 4
    {
        return None;
    }
    intent(action, json!({ "query": query }))
}

fn kill(raw_query: &str) -> Option<Intent> {
    let query = raw_query.trim();
    if query.is_empty() {
        return None;
    }
    match query.parse::<u32>() {
        Ok(pid) => intent("process.kill", json!({ "pid": pid })),
        Err(_) => intent("process.kill", json!({ "name": query })),
    }
}

fn process_list(t: &str) -> Option<Intent> {
    let by_ram =
        t.contains("памят") || t.contains("оператив") || t.contains("ram") || t.contains("memory");
    intent(
        "process.list",
        json!({ "sort": if by_ram { "ram" } else { "cpu" }, "limit": 10 }),
    )
}

fn group<'a>(captures: &'a Captures<'a>, index: usize) -> &'a str {
    captures.get(index).map_or("", |found| found.as_str())
}

// ---------- Russian rules (order matters) ----------

fn ru_rules() -> Vec<RuleDef> {
    vec![
        // cancel a scheduled shutdown (before any shutdown rule)
        RuleDef {
            pattern: r"\b(?:отмени|отмена|отбой|прекрати|останови)\b.*\b(?:выключени\w*|перезагрузк\w*|завершени\w*)",
            build: |_, _| simple("power.abort"),
        },
        // sound
        RuleDef {
            pattern: r"\b(?:включи|верни|врубай|вруби|включить)\b.*\bзвук\w*|\bразмьют\b|\bсними мьют\b",
            build: |_, _| simple("audio.unmute"),
        },
        RuleDef {
            pattern: r"\bбез звука\b|\b(?:выключи|отключи|убери|вырубай|выруби)\b.*\bзвук\w*|\bзаглуши\b|\bмьют\b|\bмут\b",
            build: |_, _| simple("audio.mute"),
        },
        RuleDef {
            pattern: r"\bкакая (?:сейчас |текущая )?громкость\b|\bсколько громкост\w*|\bна сколько громкость\b|\bтекущая громкость\b",
            build: |_, _| simple("audio.get_volume"),
        },
        RuleDef {
            pattern: r"\bгромкост\w*.*\b(?:максимум\w*|полную|на всю)\b",
            build: |_, _| intent("audio.set_volume", json!({ "level": 100 })),
        },
        RuleDef {
            pattern: r"\b(?:громкост\w*|звук\w*|громче|погромче|тише|потише)\b",
            build: |_, t| {
                volume_with_number(
                    t,
                    &[
                        "увеличь",
                        "прибавь",
                        "добавь",
                        "повысь",
                        "увеличить",
                        "громче",
                        "погромче",
                    ],
                    &[
                        "уменьши",
                        "убавь",
                        "снизь",
                        "понизь",
                        "уменьшить",
                        "тише",
                        "потише",
                    ],
                    "до",
                    "на",
                )
            },
        },
        RuleDef {
            pattern: r"\b(?:громче|погромче|прибавь(?: звук| громкость)?|увеличь (?:громкость|звук)|сделай громче)\b",
            build: |_, _| change(10),
        },
        RuleDef {
            pattern: r"\b(?:тише|потише|убавь(?: звук| громкость)?|уменьши (?:громкость|звук)|сделай тише)\b",
            build: |_, _| change(-10),
        },
        // media keys
        RuleDef {
            pattern: r"\bследующ\w*\s+(?:трек|песн\w*|композици\w*|музык\w*)\b|\bпереключи (?:трек|песню|композицию)\b|^(?:дальше|следующий|следующая)$",
            build: |_, _| media("next"),
        },
        RuleDef {
            pattern: r"\bпредыдущ\w*\s+(?:трек|песн\w*|композици\w*)\b|\bпрошлый трек\b|\bпрошлую песню\b|\bтрек назад\b",
            build: |_, _| media("prev"),
        },
        RuleDef {
            pattern: r"\bпауз\w*|\bостанови (?:музык\w*|трек|песн\w*|воспроизведение)\b|\bстоп музык\w*|\bпродолжи (?:музык\w*|воспроизведение)\b|\bвоспроизведи\b|\bвозобнови\b|^(?:играй|продолжи|продолжай)(?: музык\w*)?$",
            build: |_, _| media("play_pause"),
        },
        // system status (before the generic process rules: "процессора" starts with "процесс")
        RuleDef {
            pattern: r"\b(?:температур\w*|греется|нагрелась|перегре\w+)\b.*\b(?:видеокарт\w*|видюх\w*|гпу|gpu)\b|\b(?:видеокарт\w*|видюх\w*|gpu)\b.*\b(?:температур\w*|греется|перегре\w+)\b",
            build: |_, _| intent("system.status", json!({ "topic": "gpu_temp" })),
        },
        RuleDef {
            pattern: r"\b(?:загрузк\w*|нагрузк\w*|занят\w*)\b.*\b(?:видеокарт\w*|видюх\w*|gpu)\b|\bсколько видеопамяти\b|\bвидеопамят\w*",
            build: |_, _| intent("system.status", json!({ "topic": "gpu" })),
        },
        RuleDef {
            pattern: r"\bсколько (?:сейчас )?(?:свободно|осталось|занято|свободной)\b.*\b(?:памят\w*|оператив\w*|озу)\b|\b(?:свободн\w+|занят\w+) (?:оператив\w+ )?(?:памят\w*|озу)\b|\bсколько (?:памяти|оперативки|озу)\b|\bзагрузк\w+ (?:оперативн\w+ )?(?:памяти|озу|оперативки)\b",
            build: |_, _| intent("system.status", json!({ "topic": "memory" })),
        },
        RuleDef {
            pattern: r"\b(?:загрузк\w*|нагрузк\w*)\b.*\b(?:процессор\w*|цп|cpu|проц)\b|\bпроцессор\w*\b.*\b(?:загружен\w*|нагружен\w*)\b|\b(?:насколько|на сколько|как) загружен\w* (?:процессор|цп|проц)\b",
            build: |_, _| intent("system.status", json!({ "topic": "cpu" })),
        },
        RuleDef {
            pattern: r"\bчто (?:так )?(?:тормозит|лагает|тупит|замедляет)\b|\b(?:компьютер|комп|пк|система|виндовс|винда|ноутбук) (?:тормозит|лагает|тупит|медленно работает)\b|\bпочему (?:так )?(?:тормозит|лагает|медленно)\b|\b(?:тормозит|тормозят|лагает)\b.*\b(?:компьютер|комп|пк|систем\w*|винд\w*|windows)\b",
            build: |_, _| intent("system.status", json!({ "topic": "slowdown" })),
        },
        // screen vision
        RuleDef {
            pattern: r"\bпомоги (?:мне )?с настройками (?:этой|данной) (?:программы|приложения|игры)\b",
            build: |_, _| {
                intent(
                    "vision.describe_screen",
                    json!({ "question": "Помоги с настройками этой программы: объясни, что значат видимые настройки и что стоит выбрать." }),
                )
            },
        },
        RuleDef {
            pattern: r"\bчто (?:там |сейчас )?(?:у меня )?(?:сейчас )?(?:на экране|на моем экране)\b|\bчто ты видишь(?: на экране)?\b|\bопиши (?:мой )?экран\b|\bпосмотри на (?:мой )?экран\b",
            build: |_, _| simple("vision.describe_screen"),
        },
        RuleDef {
            pattern: r"\bчто (?:там )?написано (?:в|на) (?:этом|этой|данном|данной) (?:окне|окошке|программе|странице|вкладке)\b|\b(?:прочитай|прочти) (?:мне )?(?:это |текущее )?окно\b|\bчто (?:здесь|тут|там) написано\b",
            build: |_, _| simple("vision.read_window"),
        },
        RuleDef {
            pattern: r"\bобъясни (?:мне )?(?:эту |данную |вот эту )?ошибку\b|\bчто (?:это )?за ошибка\b|\bчто значит (?:эта )?ошибка\b",
            build: |_, _| simple("vision.explain_error"),
        },
        RuleDef {
            pattern: r"\b(?:сделай|сними|сохрани|создай)\b.*\b(?:скриншот\w*|скрин\w*|снимок экрана)\b|^скриншот$",
            build: |_, _| simple("vision.save_screenshot"),
        },
        // windows and processes
        RuleDef {
            pattern: r"\bсверни (?:все|всё)(?: окна)?\b|\bсвернуть (?:все|всё)\b|\bпокажи рабочий стол\b",
            build: |_, _| simple("window.minimize_all"),
        },
        RuleDef {
            pattern: r"\bзависш\w+|\bне отвечает\b|\bне отвечают\b|\bподвис\w*",
            build: |_, _| simple("process.hung"),
        },
        RuleDef {
            pattern: r"\b(?:убей|убить|заверши|завершить|прибей|останови)\s+процесс\s+(.+)$",
            build: |c, _| kill(group(c, 1)),
        },
        RuleDef {
            pattern: r"\bпроцесс\w*\b|\bчто (?:грузит|жрет|нагружает|ест)\b|\bнагрузк\w+ (?:на )?(?:процессор|cpu)\b",
            build: |_, t| process_list(t),
        },
        RuleDef {
            pattern: r"\bпокажи (?:мне )?(?:запущенн\w+ (?:программ\w*|приложени\w*)|открыт\w+ (?:окна|программы|приложения)|список окон|окна)\b|\bкакие (?:программы|приложения|окна) (?:запущены|открыты)\b|\bчто (?:сейчас )?(?:запущено|открыто)\b|\bсписок окон\b|\bзапущенные программы\b",
            build: |_, _| simple("window.list"),
        },
        RuleDef {
            pattern: r"^(?:закрой|закрыть|закрывай)\s+(.+)$",
            build: |c, _| window_action("window.close", group(c, 1)),
        },
        RuleDef {
            pattern: r"^(?:сверни|свернуть)\s+(.+)$",
            build: |c, _| window_action("window.minimize", group(c, 1)),
        },
        RuleDef {
            pattern: r"^(?:разверни|развернуть|максимизируй)\s+(.+)$",
            build: |c, _| window_action("window.maximize", group(c, 1)),
        },
        RuleDef {
            pattern: r"^(?:переключись на|перейди в|перейди на|покажи окно|фокус на|сфокусируйся на|активируй)\s+(.+)$",
            build: |c, _| window_action("window.focus", group(c, 1)),
        },
        // network
        RuleDef {
            pattern: r"\b(?:проверь|проверить|проверка)\b.*\b(?:интернет\w*|подключени\w*|сеть|сети|связь)\b|\b(?:есть|работает) ли интернет\b|\bинтернет (?:работает|есть|пропал)\b|\bпингани\b",
            build: |_, _| simple("network.check"),
        },
        RuleDef {
            pattern: r"\b(?:проверь|проверить|проверка|диагностика|диагностируй)\b.*\b(?:систем\w*|компьютер\w*|комп|пк)\b|\bсостояни\w+ (?:системы|компьютера|компа|пк)\b|\bкак (?:там )?(?:система|компьютер|комп)\b",
            build: |_, _| intent("system.status", json!({ "topic": "overview" })),
        },
        // power
        RuleDef {
            pattern: r"\b(?:заблокируй|заблокировать|блокировка|блокируй)\b(?: (?:компьютер|комп|пк|экран|систему|windows|винду))?",
            build: |_, _| simple("power.lock"),
        },
        RuleDef {
            pattern: r"\bгибернаци\w*",
            build: |_, _| simple("power.hibernate"),
        },
        RuleDef {
            pattern: r"\bспящий режим\b|\bусыпи(?: компьютер| комп| пк)?\b|\bв сон\b|\bрежим сна\b|\bзасыпай\b",
            build: |_, _| simple("power.sleep"),
        },
        RuleDef {
            pattern: r"\b(?:выключи|выключить|отключи|вырубай)\s+(?:компьютер|комп|пк|систему|windows|винду|ноутбук)\b|\bзаверши работу\b|\bзавершение работы\b|^(?:выключись|выключение)$",
            build: |_, _| simple("power.shutdown"),
        },
        RuleDef {
            pattern: r"\b(?:перезагрузи|перезагрузить|перезапусти)(?:сь)?\s+(?:компьютер|комп|пк|систему|windows|винду|ноутбук)\b|^(?:перезагрузись|перезагрузка)$",
            build: |_, _| simple("power.restart"),
        },
        // info
        RuleDef {
            pattern: r"\bкоторый час\b|\bсколько (?:сейчас )?времени\b|\bскажи время\b|\bтекущее время\b|^время$",
            build: |_, _| simple("info.time"),
        },
        RuleDef {
            pattern: r"\b(?:какое|какая) (?:сегодня )?(?:число|дата)\b|\bкакой (?:сегодня )?день\b|\bсегодняшн\w+ дат\w+|\bкакое сегодня\b",
            build: |_, _| simple("info.date"),
        },
        // folders and drives
        RuleDef {
            pattern: r"\b(?:открой|открыть|покажи|зайди на)\s+(?:локальный\s+)?диск\s+(\S+)",
            build: |c, _| open_drive(c),
        },
        RuleDef {
            pattern: r"\b(?:что (?:лежит )?в|покажи содержимое|содержимое|список файлов в)\b",
            build: |_, t| list_folder(t),
        },
        RuleDef {
            pattern: r"^(?:открой|открыть|покажи|запусти)\s+(?:папку\s+)?(?:загрузк\w+|документ\w+|рабочий стол|рабочего стола|изображен\w+|картинк\w+|музык\w+|видео|фото\w*)$|\b(?:открой|открыть|покажи|запусти)\b.*\bпапк\w+",
            build: |_, t| open_folder(t),
        },
        // applications (bare words and generic "open X")
        RuleDef {
            pattern: r"^(?:диспетчер задач|калькулятор|блокнот|настройки|проводник|параметры)$",
            build: |_, t| app(t),
        },
        RuleDef {
            pattern: r"^(?:открой|открыть|запусти|запустить|включи|включить|стартуй|запускай|вруби|врубай|открывай)\s+(.+)$",
            build: |c, _| open_app(group(c, 1)),
        },
    ]
}

// ---------- English rules ----------

fn en_rules() -> Vec<RuleDef> {
    vec![
        RuleDef {
            pattern: r"\b(?:cancel|abort|stop)\b.*\b(?:shutdown|shut down|restart|reboot)\b",
            build: |_, _| simple("power.abort"),
        },
        RuleDef {
            pattern: r"\bunmute\b|\b(?:turn on|restore)\b.*\b(?:sound|audio)\b",
            build: |_, _| simple("audio.unmute"),
        },
        RuleDef {
            pattern: r"\bmute\b|\b(?:turn off|silence)\b.*\b(?:sound|audio)\b",
            build: |_, _| simple("audio.mute"),
        },
        RuleDef {
            pattern: r"\bwhat(?: s| is) (?:the )?(?:current )?volume\b|\bhow loud\b",
            build: |_, _| simple("audio.get_volume"),
        },
        RuleDef {
            pattern: r"\bvolume\b.*\b(?:max|maximum|full)\b",
            build: |_, _| intent("audio.set_volume", json!({ "level": 100 })),
        },
        RuleDef {
            pattern: r"\b(?:volume|sound|louder|quieter)\b",
            build: |_, t| {
                volume_with_number(
                    t,
                    &["increase", "raise", "up", "louder"],
                    &["decrease", "lower", "reduce", "down", "quieter"],
                    "to",
                    "by",
                )
            },
        },
        RuleDef {
            pattern: r"\b(?:louder|volume up|turn (?:it |the volume )?up|increase (?:the )?volume|raise (?:the )?volume)\b",
            build: |_, _| change(10),
        },
        RuleDef {
            pattern: r"\b(?:quieter|softer|volume down|turn (?:it |the volume )?down|decrease (?:the )?volume|lower (?:the )?volume|reduce (?:the )?volume)\b",
            build: |_, _| change(-10),
        },
        RuleDef {
            pattern: r"^(?:next(?: track| song)?|skip(?: track| song)?|play next(?: track| song)?)$",
            build: |_, _| media("next"),
        },
        RuleDef {
            pattern: r"^(?:previous|prev|last)(?: track| song)?$|^go back(?: a track)?$",
            build: |_, _| media("prev"),
        },
        RuleDef {
            pattern: r"^(?:pause|resume|play|play music|pause music|stop music|play pause|pause the music|resume music)$",
            build: |_, _| media("play_pause"),
        },
        // system status
        RuleDef {
            pattern: r"\b(?:gpu|graphics card|video card)\b.*\b(?:temp\w*|hot|heat\w*)\b|\btemp\w*\b.*\b(?:gpu|graphics card|video card)\b",
            build: |_, _| intent("system.status", json!({ "topic": "gpu_temp" })),
        },
        RuleDef {
            pattern: r"\b(?:gpu|graphics card|video card)\b.*\b(?:usage|load|utili[sz]ation|vram|memory)\b|\b(?:gpu|vram) (?:usage|load)\b",
            build: |_, _| intent("system.status", json!({ "topic": "gpu" })),
        },
        RuleDef {
            pattern: r"\bhow much (?:memory|ram)\b|\b(?:free|available) (?:memory|ram)\b|\b(?:memory|ram) (?:usage|left)\b",
            build: |_, _| intent("system.status", json!({ "topic": "memory" })),
        },
        RuleDef {
            pattern: r"\b(?:cpu|processor)\b.*\b(?:usage|load|busy)\b|\bhow busy is (?:the |my )?(?:cpu|processor)\b",
            build: |_, _| intent("system.status", json!({ "topic": "cpu" })),
        },
        RuleDef {
            pattern: r"\bwhat s (?:slowing|making)\b.*\b(?:computer|pc|system|laptop)\b|\bwhy is (?:my |the )?(?:computer|pc|system|laptop)\b.*\b(?:slow|lagging)\b|\b(?:computer|pc|laptop) is (?:slow|lagging)\b|\bwhat is slowing\b",
            build: |_, _| intent("system.status", json!({ "topic": "slowdown" })),
        },
        // screen vision
        RuleDef {
            pattern: r"\bhelp me with (?:this|the) (?:program|app|application)(?: s)? settings\b",
            build: |_, _| {
                intent(
                    "vision.describe_screen",
                    json!({ "question": "Help me with the settings of this program: explain what the visible settings mean and what to choose." }),
                )
            },
        },
        RuleDef {
            pattern: r"\bwhat s on (?:my )?screen\b|\bwhat do you see\b|\bdescribe (?:my |the )?screen\b",
            build: |_, _| simple("vision.describe_screen"),
        },
        RuleDef {
            pattern: r"\bwhat does (?:this|the) window say\b|\bread (?:this|the) window\b|\bwhat s written (?:in|on) (?:this|the) (?:window|screen)\b",
            build: |_, _| simple("vision.read_window"),
        },
        RuleDef {
            pattern: r"\bexplain (?:this|the) error\b|\bwhat does this error mean\b",
            build: |_, _| simple("vision.explain_error"),
        },
        RuleDef {
            pattern: r"\b(?:take|make|save)\b.*\bscreenshot\b|^screenshot$",
            build: |_, _| simple("vision.save_screenshot"),
        },
        RuleDef {
            pattern: r"\bminimi[sz]e all(?: windows)?\b|\bshow (?:the )?desktop\b",
            build: |_, _| simple("window.minimize_all"),
        },
        RuleDef {
            pattern: r"\b(?:hung|frozen|not responding)\b",
            build: |_, _| simple("process.hung"),
        },
        RuleDef {
            pattern: r"\b(?:kill|terminate|end)\s+(?:the\s+)?process\s+(.+)$",
            build: |c, _| kill(group(c, 1)),
        },
        RuleDef {
            pattern: r"\b(?:show|list)\b.*\bprocesses\b|\bwhat (?:is|s) (?:using|eating) (?:the )?(?:cpu|memory|ram)\b",
            build: |_, t| process_list(t),
        },
        RuleDef {
            pattern: r"\b(?:show|list|what)\b.*\b(?:running|open)\b.*\b(?:programs|apps|applications|windows)\b|\bwhat s running\b|\blist (?:the )?windows\b",
            build: |_, _| simple("window.list"),
        },
        RuleDef {
            pattern: r"^(?:close|quit|exit)\s+(.+)$",
            build: |c, _| window_action("window.close", group(c, 1)),
        },
        RuleDef {
            pattern: r"^minimi[sz]e\s+(.+)$",
            build: |c, _| window_action("window.minimize", group(c, 1)),
        },
        RuleDef {
            pattern: r"^maximi[sz]e\s+(.+)$",
            build: |c, _| window_action("window.maximize", group(c, 1)),
        },
        RuleDef {
            pattern: r"^(?:switch to|focus(?: on)?|go to|bring up)\s+(.+)$",
            build: |c, _| window_action("window.focus", group(c, 1)),
        },
        RuleDef {
            pattern: r"\b(?:check|test)\b.*\b(?:internet|connection|network)\b|\bam i online\b|\bis the internet (?:up|working|down)\b|\bis there (?:an )?internet\b",
            build: |_, _| simple("network.check"),
        },
        RuleDef {
            pattern: r"\b(?:check|diagnose)\b.*\b(?:system|computer|pc)\b|\bsystem (?:status|check|health)\b|\bhow is (?:my |the )?(?:system|computer|pc)\b",
            build: |_, _| intent("system.status", json!({ "topic": "overview" })),
        },
        RuleDef {
            pattern: r"\block\b.*\b(?:computer|pc|screen|workstation|laptop)\b|^lock(?: it| down)?$",
            build: |_, _| simple("power.lock"),
        },
        RuleDef {
            pattern: r"\bhibernate\b",
            build: |_, _| simple("power.hibernate"),
        },
        RuleDef {
            pattern: r"\b(?:sleep mode|go to sleep|suspend)\b|\bput (?:the )?(?:computer|pc|laptop) to sleep\b|^sleep$",
            build: |_, _| simple("power.sleep"),
        },
        RuleDef {
            pattern: r"\b(?:shut down|shutdown|turn off|power off)\b.*\b(?:computer|pc|laptop|system)\b|^(?:shut down|shutdown)$",
            build: |_, _| simple("power.shutdown"),
        },
        RuleDef {
            pattern: r"\b(?:restart|reboot)\b.*\b(?:computer|pc|laptop|system)\b|^(?:restart|reboot)$",
            build: |_, _| simple("power.restart"),
        },
        RuleDef {
            pattern: r"\bwhat time is it\b|\bcurrent time\b|\bwhat is the time\b|\btell me the time\b|\bwhat s the time\b",
            build: |_, _| simple("info.time"),
        },
        RuleDef {
            pattern: r"\bwhat s (?:the )?date\b|\bwhat day is it\b|\btoday s date\b|\bwhat is (?:the )?date\b|\bwhat is today\b",
            build: |_, _| simple("info.date"),
        },
        RuleDef {
            pattern: r"\bopen\s+(?:local\s+)?(?:drive|disk)\s+(\S+)",
            build: |c, _| open_drive(c),
        },
        RuleDef {
            pattern: r"\b(?:what s in|show contents of|list files in)\b",
            build: |_, t| list_folder(t),
        },
        RuleDef {
            pattern: r"^open (?:downloads|documents|desktop|pictures|music|videos)(?: folder)?$|\bopen\b.*\bfolder\b",
            build: |_, t| open_folder(t),
        },
        RuleDef {
            pattern: r"^(?:task manager|calculator|notepad|settings|file explorer|explorer)$",
            build: |_, t| app(t),
        },
        RuleDef {
            pattern: r"^(?:open|launch|start|run)\s+(.+)$",
            build: |c, _| open_app(group(c, 1)),
        },
    ]
}
