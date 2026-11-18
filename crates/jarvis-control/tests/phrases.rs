//! Phrase table for the fast path: phrase -> expected intent (or `None` for the LLM).

use jarvis_control::parse::rule_health;
use jarvis_control::{parse, Intent, Lang};
use serde_json::{json, Value};

struct Case {
    phrase: &'static str,
    lang: Lang,
    expected: Option<(&'static str, Value)>,
}

fn ok(phrase: &'static str, lang: Lang, action: &'static str, args: Value) -> Case {
    Case {
        phrase,
        lang,
        expected: Some((action, args)),
    }
}

fn none(phrase: &'static str, lang: Lang) -> Case {
    Case {
        phrase,
        lang,
        expected: None,
    }
}

fn q(text: &str) -> Value {
    json!({ "query": text })
}

fn cases() -> Vec<Case> {
    use Lang::{En, Ru};
    vec![
        // applications
        ok("открой Discord", Ru, "app.open", q("discord")),
        ok("открой дискорд", Ru, "app.open", q("дискорд")),
        ok("запусти Steam", Ru, "app.open", q("steam")),
        ok("запусти стим", Ru, "app.open", q("стим")),
        ok(
            "открой Dungeon Quest 3",
            Ru,
            "app.open",
            q("dungeon quest 3"),
        ),
        ok(
            "запусти данжен квест три",
            Ru,
            "app.open",
            q("данжен квест три"),
        ),
        ok("открой телеграм", Ru, "app.open", q("телеграм")),
        ok("включи обсидиан", Ru, "app.open", q("обсидиан")),
        ok("запусти игру дота 2", Ru, "app.open", q("дота 2")),
        ok("открой приложение хром", Ru, "app.open", q("хром")),
        ok("открой браузер", Ru, "app.open", q("браузер")),
        ok("Лёха, открой Discord", Ru, "app.open", q("discord")),
        ok("алексей запусти стим", Ru, "app.open", q("стим")),
        ok(
            "Джарвис, пожалуйста, открой телеграм",
            Ru,
            "app.open",
            q("телеграм"),
        ),
        ok("открой телеграм пожалуйста", Ru, "app.open", q("телеграм")),
        ok("диспетчер задач", Ru, "app.open", q("диспетчер задач")),
        ok("открой калькулятор", Ru, "app.open", q("калькулятор")),
        ok("блокнот", Ru, "app.open", q("блокнот")),
        ok("открой настройки", Ru, "app.open", q("настройки")),
        ok("открой проводник", Ru, "app.open", q("проводник")),
        ok("open Discord", En, "app.open", q("discord")),
        ok("launch steam", En, "app.open", q("steam")),
        ok("please open notepad", En, "app.open", q("notepad")),
        ok(
            "hey jarvis open task manager",
            En,
            "app.open",
            q("task manager"),
        ),
        ok("start the calculator", En, "app.open", q("calculator")),
        // system status and screen vision (P7-B / P8-B)
        ok(
            "какая температура видеокарты",
            Ru,
            "system.status",
            json!({"topic": "gpu_temp"}),
        ),
        ok(
            "Джарвис, какая температура видеокарты?",
            Ru,
            "system.status",
            json!({"topic": "gpu_temp"}),
        ),
        ok(
            "не греется ли видеокарта",
            Ru,
            "system.status",
            json!({"topic": "gpu_temp"}),
        ),
        ok(
            "загрузка видеокарты",
            Ru,
            "system.status",
            json!({"topic": "gpu"}),
        ),
        ok(
            "сколько видеопамяти занято",
            Ru,
            "system.status",
            json!({"topic": "gpu"}),
        ),
        ok(
            "сколько свободно памяти",
            Ru,
            "system.status",
            json!({"topic": "memory"}),
        ),
        ok(
            "сколько осталось оперативки",
            Ru,
            "system.status",
            json!({"topic": "memory"}),
        ),
        ok(
            "загрузка процессора",
            Ru,
            "system.status",
            json!({"topic": "cpu"}),
        ),
        ok(
            "насколько загружен процессор",
            Ru,
            "system.status",
            json!({"topic": "cpu"}),
        ),
        ok(
            "что тормозит компьютер",
            Ru,
            "system.status",
            json!({"topic": "slowdown"}),
        ),
        ok(
            "почему тормозит",
            Ru,
            "system.status",
            json!({"topic": "slowdown"}),
        ),
        ok(
            "проверь систему",
            Ru,
            "system.status",
            json!({"topic": "overview"}),
        ),
        ok(
            "проверь компьютер",
            Ru,
            "system.status",
            json!({"topic": "overview"}),
        ),
        ok(
            "what's the GPU temperature",
            En,
            "system.status",
            json!({"topic": "gpu_temp"}),
        ),
        ok("gpu usage", En, "system.status", json!({"topic": "gpu"})),
        ok(
            "how much memory is free",
            En,
            "system.status",
            json!({"topic": "memory"}),
        ),
        ok("cpu usage", En, "system.status", json!({"topic": "cpu"})),
        ok(
            "what's slowing down my computer",
            En,
            "system.status",
            json!({"topic": "slowdown"}),
        ),
        ok(
            "check the system",
            En,
            "system.status",
            json!({"topic": "overview"}),
        ),
        ok(
            "что у меня на экране",
            Ru,
            "vision.describe_screen",
            json!({}),
        ),
        ok(
            "что написано в этом окне",
            Ru,
            "vision.read_window",
            json!({}),
        ),
        ok("объясни эту ошибку", Ru, "vision.explain_error", json!({})),
        ok("сделай скриншот", Ru, "vision.save_screenshot", json!({})),
        ok(
            "what's on my screen",
            En,
            "vision.describe_screen",
            json!({}),
        ),
        ok(
            "what does this window say",
            En,
            "vision.read_window",
            json!({}),
        ),
        ok("explain this error", En, "vision.explain_error", json!({})),
        ok("take a screenshot", En, "vision.save_screenshot", json!({})),
        // settings help carries a question for the model
        Case {
            phrase: "помоги с настройками этой программы",
            lang: Ru,
            expected: Some((
                "vision.describe_screen",
                json!({"question": "Помоги с настройками этой программы: объясни, что значат видимые настройки и что стоит выбрать."}),
            )),
        },
        // these must keep their old meaning
        ok("проверь интернет", Ru, "network.check", json!({})),
        ok(
            "что грузит процессор",
            Ru,
            "process.list",
            json!({"sort": "cpu", "limit": 10}),
        ),
        ok("зависшие окна", Ru, "process.hung", json!({})),
        // volume
        ok(
            "уменьши громкость до 30 процентов",
            Ru,
            "audio.set_volume",
            json!({"level": 30}),
        ),
        ok(
            "уменьши громкость до тридцати процентов",
            Ru,
            "audio.set_volume",
            json!({"level": 30}),
        ),
        ok(
            "поставь громкость на 50",
            Ru,
            "audio.set_volume",
            json!({"level": 50}),
        ),
        ok(
            "установи громкость сорок пять процентов",
            Ru,
            "audio.set_volume",
            json!({"level": 45}),
        ),
        ok(
            "громкость на максимум",
            Ru,
            "audio.set_volume",
            json!({"level": 100}),
        ),
        ok(
            "прибавь громкость на 20",
            Ru,
            "audio.change_volume",
            json!({"delta": 20}),
        ),
        ok(
            "убавь громкость на десять процентов",
            Ru,
            "audio.change_volume",
            json!({"delta": -10}),
        ),
        ok(
            "сделай тише на 15",
            Ru,
            "audio.change_volume",
            json!({"delta": -15}),
        ),
        // fractions are shares of the scale, never the number word inside them
        ok(
            "громкость на две трети",
            Ru,
            "audio.set_volume",
            json!({"level": 67}),
        ),
        ok(
            "поставь громкость на половину",
            Ru,
            "audio.set_volume",
            json!({"level": 50}),
        ),
        ok(
            "громкость на треть",
            Ru,
            "audio.set_volume",
            json!({"level": 33}),
        ),
        ok(
            "громкость на четверть",
            Ru,
            "audio.set_volume",
            json!({"level": 25}),
        ),
        ok(
            "сделай громкость три четверти",
            Ru,
            "audio.set_volume",
            json!({"level": 75}),
        ),
        ok(
            "громкость до двух третей",
            Ru,
            "audio.set_volume",
            json!({"level": 67}),
        ),
        ok(
            "убавь громкость на треть",
            Ru,
            "audio.change_volume",
            json!({"delta": -33}),
        ),
        ok(
            "убавь громкость наполовину",
            Ru,
            "audio.change_volume",
            json!({"delta": -50}),
        ),
        ok(
            "прибавь громкость на четверть",
            Ru,
            "audio.change_volume",
            json!({"delta": 25}),
        ),
        ok(
            "громкость на 2/3",
            Ru,
            "audio.set_volume",
            json!({"level": 67}),
        ),
        ok(
            "set the volume to half",
            En,
            "audio.set_volume",
            json!({"level": 50}),
        ),
        ok(
            "set the volume to two thirds",
            En,
            "audio.set_volume",
            json!({"level": 67}),
        ),
        ok(
            "lower the volume by a quarter",
            En,
            "audio.change_volume",
            json!({"delta": -25}),
        ),
        ok("громче", Ru, "audio.change_volume", json!({"delta": 10})),
        ok(
            "сделай потише",
            Ru,
            "audio.change_volume",
            json!({"delta": -10}),
        ),
        ok("тише", Ru, "audio.change_volume", json!({"delta": -10})),
        ok(
            "прибавь звук",
            Ru,
            "audio.change_volume",
            json!({"delta": 10}),
        ),
        ok("без звука", Ru, "audio.mute", json!({})),
        ok("выключи звук", Ru, "audio.mute", json!({})),
        ok("включи звук", Ru, "audio.unmute", json!({})),
        ok("какая сейчас громкость", Ru, "audio.get_volume", json!({})),
        ok(
            "set the volume to 40 percent",
            En,
            "audio.set_volume",
            json!({"level": 40}),
        ),
        ok(
            "set volume to twenty five percent",
            En,
            "audio.set_volume",
            json!({"level": 25}),
        ),
        ok(
            "turn the volume down",
            En,
            "audio.change_volume",
            json!({"delta": -10}),
        ),
        ok("volume up", En, "audio.change_volume", json!({"delta": 10})),
        ok(
            "lower the volume by 5",
            En,
            "audio.change_volume",
            json!({"delta": -5}),
        ),
        ok("mute", En, "audio.mute", json!({})),
        ok("unmute", En, "audio.unmute", json!({})),
        ok("what is the volume", En, "audio.get_volume", json!({})),
        // media
        ok("следующий трек", Ru, "audio.media", json!({"key": "next"})),
        ok("переключи песню", Ru, "audio.media", json!({"key": "next"})),
        ok("предыдущий трек", Ru, "audio.media", json!({"key": "prev"})),
        ok("пауза", Ru, "audio.media", json!({"key": "play_pause"})),
        ok(
            "останови музыку",
            Ru,
            "audio.media",
            json!({"key": "play_pause"}),
        ),
        ok("next track", En, "audio.media", json!({"key": "next"})),
        ok("previous song", En, "audio.media", json!({"key": "prev"})),
        ok("pause", En, "audio.media", json!({"key": "play_pause"})),
        // windows and processes
        ok("покажи запущенные программы", Ru, "window.list", json!({})),
        ok("какие программы запущены", Ru, "window.list", json!({})),
        ok("show running programs", En, "window.list", json!({})),
        ok("закрой браузер", Ru, "window.close", q("браузер")),
        ok("закрой дискорд", Ru, "window.close", q("дискорд")),
        ok("close the browser", En, "window.close", q("browser")),
        ok("сверни всё", Ru, "window.minimize_all", json!({})),
        ok("покажи рабочий стол", Ru, "window.minimize_all", json!({})),
        ok("minimize all windows", En, "window.minimize_all", json!({})),
        ok("сверни телеграм", Ru, "window.minimize", q("телеграм")),
        ok("разверни хром", Ru, "window.maximize", q("хром")),
        ok("переключись на дискорд", Ru, "window.focus", q("дискорд")),
        ok(
            "покажи процессы",
            Ru,
            "process.list",
            json!({"sort": "cpu", "limit": 10}),
        ),
        ok(
            "что жрёт память",
            Ru,
            "process.list",
            json!({"sort": "ram", "limit": 10}),
        ),
        ok("покажи зависшие программы", Ru, "process.hung", json!({})),
        ok(
            "завершить процесс notepad",
            Ru,
            "process.kill",
            json!({"name": "notepad"}),
        ),
        ok(
            "убей процесс 1234",
            Ru,
            "process.kill",
            json!({"pid": 1234}),
        ),
        ok(
            "kill process notepad",
            En,
            "process.kill",
            json!({"name": "notepad"}),
        ),
        // files
        ok(
            "открой папку загрузок",
            Ru,
            "files.open_folder",
            json!({"name": "downloads"}),
        ),
        ok(
            "открой загрузки",
            Ru,
            "files.open_folder",
            json!({"name": "downloads"}),
        ),
        ok(
            "открой папку документы",
            Ru,
            "files.open_folder",
            json!({"name": "documents"}),
        ),
        ok(
            "открой рабочий стол",
            Ru,
            "files.open_folder",
            json!({"name": "desktop"}),
        ),
        ok(
            "открой папку музыка",
            Ru,
            "files.open_folder",
            json!({"name": "music"}),
        ),
        ok(
            "open the downloads folder",
            En,
            "files.open_folder",
            json!({"name": "downloads"}),
        ),
        ok(
            "открой диск E",
            Ru,
            "files.open_folder",
            json!({"path": "E:\\"}),
        ),
        ok(
            "открой диск D",
            Ru,
            "files.open_folder",
            json!({"path": "D:\\"}),
        ),
        ok(
            "open drive e",
            En,
            "files.open_folder",
            json!({"path": "E:\\"}),
        ),
        ok(
            "что в папке загрузки",
            Ru,
            "files.list",
            json!({"name": "downloads"}),
        ),
        ok(
            "удали файл C:\\Users\\Test\\tmp\\old.txt",
            Ru,
            "files.delete",
            json!({"path": "C:\\Users\\Test\\tmp\\old.txt"}),
        ),
        // network, info
        ok(
            "проверь подключение к интернету",
            Ru,
            "network.check",
            json!({}),
        ),
        ok("есть ли интернет", Ru, "network.check", json!({})),
        ok(
            "check the internet connection",
            En,
            "network.check",
            json!({}),
        ),
        ok("который час", Ru, "info.time", json!({})),
        ok("сколько времени", Ru, "info.time", json!({})),
        ok("what time is it", En, "info.time", json!({})),
        ok("какое сегодня число", Ru, "info.date", json!({})),
        ok("what is the date", En, "info.date", json!({})),
        // power
        ok("заблокируй компьютер", Ru, "power.lock", json!({})),
        ok("lock the computer", En, "power.lock", json!({})),
        ok("спящий режим", Ru, "power.sleep", json!({})),
        ok("гибернация", Ru, "power.hibernate", json!({})),
        ok("выключи компьютер", Ru, "power.shutdown", json!({})),
        ok("shut down the computer", En, "power.shutdown", json!({})),
        ok("перезагрузи компьютер", Ru, "power.restart", json!({})),
        ok("restart the computer", En, "power.restart", json!({})),
        ok("отмени выключение", Ru, "power.abort", json!({})),
        ok("cancel shutdown", En, "power.abort", json!({})),
        // negatives: must go to the LLM
        none("", Ru),
        none("Лёха", Ru),
        none("расскажи анекдот", Ru),
        none("как дела", Ru),
        none("кто такой Наполеон", Ru),
        none("я вчера открыл дверь", Ru),
        none("открой", Ru),
        none("включи музыку", Ru),
        none("включи свет", Ru),
        none("перезагрузи страницу", Ru),
        none("выключи телевизор", Ru),
        none("закрой окно", Ru),
        none("закрой всё", Ru),
        none("what is the capital of France", En),
        none("tell me a joke", En),
        none("open", En),
        none("play some jazz for me", En),
        none(
            "запусти что-нибудь интересное для меня на вечер сегодня",
            Ru,
        ),
    ]
}

#[test]
fn phrase_table_has_at_least_80_cases() {
    assert!(cases().len() >= 80, "only {} cases", cases().len());
}

#[test]
fn every_rule_pattern_compiles() {
    let (defined, compiled) = rule_health();
    assert_eq!(defined, compiled, "some fast-path regex failed to compile");
}

#[test]
fn phrase_table_pass_rate() {
    let mut failures = Vec::new();
    let all = cases();
    for case in &all {
        let got = parse(case.phrase, case.lang);
        let want = case
            .expected
            .as_ref()
            .map(|(action, args)| Intent::new(*action, args.clone()));
        if got != want {
            failures.push(format!("{:?}: expected {want:?}, got {got:?}", case.phrase));
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{} phrases failed:\n{}",
        failures.len(),
        all.len(),
        failures.join("\n")
    );
    println!("phrase table: {}/{} passed", all.len(), all.len());
}

#[test]
fn language_hint_does_not_block_other_language() {
    assert_eq!(
        parse("open discord", Lang::Ru),
        Some(Intent::new("app.open", q("discord")))
    );
    assert_eq!(
        parse("открой дискорд", Lang::En),
        Some(Intent::new("app.open", q("дискорд")))
    );
}

#[test]
fn dangerous_phrases_map_to_confirm_level_actions_only() {
    for phrase in [
        "выключи компьютер",
        "перезагрузи компьютер",
        "спящий режим",
        "убей процесс 4321",
    ] {
        let intent = parse(phrase, Lang::Ru).expect("parses");
        assert!(
            matches!(
                intent.action.as_str(),
                "power.shutdown" | "power.restart" | "power.sleep" | "process.kill"
            ),
            "{phrase} -> {}",
            intent.action
        );
    }
}
