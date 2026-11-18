//! All user-facing reply templates live here so Phase 9 (personality) can replace them in one place.
//! Style: CLASSIC JARVIS - calm, brief, polite.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub ru: String,
    pub en: String,
}

impl Reply {
    pub fn new(ru: impl Into<String>, en: impl Into<String>) -> Self {
        Self {
            ru: ru.into(),
            en: en.into(),
        }
    }
}

/// Russian plural form for "процент".
pub fn percent_ru(n: u32) -> &'static str {
    let last_two = n % 100;
    let last = n % 10;
    if (11..=14).contains(&last_two) {
        "процентов"
    } else if last == 1 {
        "процент"
    } else if (2..=4).contains(&last) {
        "процента"
    } else {
        "процентов"
    }
}

pub fn open_app(name: &str) -> Reply {
    Reply::new(format!("Открываю {name}."), format!("Opening {name}."))
}

pub fn volume_is(level: u32) -> Reply {
    Reply::new(
        format!("Громкость — {level} {}.", percent_ru(level)),
        format!("Volume is {level} percent."),
    )
}

pub fn volume_set(level: u32) -> Reply {
    volume_is(level)
}

pub fn muted() -> Reply {
    Reply::new("Звук выключен.", "Sound muted.")
}

pub fn unmuted() -> Reply {
    Reply::new("Звук включён.", "Sound restored.")
}

pub fn media(kind: &str) -> Reply {
    match kind {
        "next" => Reply::new("Следующий трек.", "Next track."),
        "prev" => Reply::new("Предыдущий трек.", "Previous track."),
        _ => Reply::new("Готово.", "Done."),
    }
}

pub fn folder_opened(name: &str) -> Reply {
    Reply::new(
        format!("Открываю папку {name}."),
        format!("Opening the {name} folder."),
    )
}

pub fn drive_opened(drive: &str) -> Reply {
    Reply::new(
        format!("Открываю диск {drive}."),
        format!("Opening drive {drive}."),
    )
}

pub fn listing(count: usize) -> Reply {
    Reply::new(
        format!("В папке элементов: {count}."),
        format!("The folder holds {count} items."),
    )
}

pub fn deleted_to_recycle_bin() -> Reply {
    Reply::new("Перемещено в корзину.", "Moved to the Recycle Bin.")
}

pub fn windows_listed(count: usize) -> Reply {
    Reply::new(
        format!("Открытых окон: {count}."),
        format!("{count} windows are open."),
    )
}

pub fn processes_listed(count: usize) -> Reply {
    Reply::new(
        format!("Показываю процессы: {count}."),
        format!("Showing {count} processes."),
    )
}

pub fn hung_listed(count: usize) -> Reply {
    if count == 0 {
        Reply::new("Зависших окон нет.", "No hung windows.")
    } else {
        Reply::new(
            format!("Зависших окон: {count}."),
            format!("{count} windows are not responding."),
        )
    }
}

pub fn window_focused(title: &str) -> Reply {
    Reply::new(
        format!("Переключаюсь на {title}."),
        format!("Switching to {title}."),
    )
}

pub fn window_minimized(title: &str) -> Reply {
    Reply::new(
        format!("Сворачиваю {title}."),
        format!("Minimizing {title}."),
    )
}

pub fn window_maximized(title: &str) -> Reply {
    Reply::new(
        format!("Разворачиваю {title}."),
        format!("Maximizing {title}."),
    )
}

pub fn window_closed(title: &str) -> Reply {
    Reply::new(format!("Закрываю {title}."), format!("Closing {title}."))
}

pub fn minimized_all() -> Reply {
    Reply::new("Все окна свёрнуты.", "All windows minimized.")
}

pub fn process_killed(name: &str) -> Reply {
    Reply::new(
        format!("Процесс {name} завершён."),
        format!("Process {name} terminated."),
    )
}

pub fn internet(online: bool) -> Reply {
    if online {
        Reply::new("Интернет работает.", "The internet connection is up.")
    } else {
        Reply::new("Интернета нет.", "There is no internet connection.")
    }
}

pub fn locked() -> Reply {
    Reply::new("Компьютер заблокирован.", "Computer locked.")
}

pub fn sleeping() -> Reply {
    Reply::new("Перехожу в спящий режим.", "Going to sleep.")
}

pub fn hibernating() -> Reply {
    Reply::new("Перехожу в гибернацию.", "Hibernating.")
}

pub fn shutdown_scheduled(seconds: u32) -> Reply {
    Reply::new(
        format!(
            "Выключение через {seconds} секунд. Скажите «отмени выключение», чтобы остановить."
        ),
        format!("Shutting down in {seconds} seconds. Say \"cancel shutdown\" to stop."),
    )
}

pub fn restart_scheduled(seconds: u32) -> Reply {
    Reply::new(
        format!(
            "Перезагрузка через {seconds} секунд. Скажите «отмени выключение», чтобы остановить."
        ),
        format!("Restarting in {seconds} seconds. Say \"cancel shutdown\" to stop."),
    )
}

pub fn shutdown_aborted() -> Reply {
    Reply::new("Выключение отменено.", "Shutdown cancelled.")
}

pub fn nothing_to_abort() -> Reply {
    Reply::new(
        "Запланированного выключения нет.",
        "No shutdown is scheduled.",
    )
}

pub fn time_now(hour: u32, minute: u32) -> Reply {
    Reply::new(
        format!("Сейчас {hour:02}:{minute:02}."),
        format!("It is {hour:02}:{minute:02}."),
    )
}

pub fn date_today(day: u32, month: u32, year: u32, weekday: usize) -> Reply {
    const MONTHS_RU: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    const MONTHS_EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const DAYS_RU: [&str; 7] = [
        "воскресенье",
        "понедельник",
        "вторник",
        "среда",
        "четверг",
        "пятница",
        "суббота",
    ];
    const DAYS_EN: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let month_index = (month.clamp(1, 12) - 1) as usize;
    let day_index = weekday % 7;
    Reply::new(
        format!(
            "Сегодня {day} {} {year} года, {}.",
            MONTHS_RU[month_index], DAYS_RU[day_index]
        ),
        format!(
            "Today is {}, {} {day}, {year}.",
            DAYS_EN[day_index], MONTHS_EN[month_index]
        ),
    )
}

/// How the assistant words its answers (the personality modes; `custom` speaks like `classic`
/// here, its instructions only steer the language model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    Formal,
    #[default]
    Classic,
    Friendly,
}

impl Style {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "formal" => Some(Self::Formal),
            "classic" | "custom" => Some(Self::Classic),
            "friendly" => Some(Self::Friendly),
            _ => None,
        }
    }
}

/// The variant of a template reply for `style`. `classic` returns the text unchanged; `formal`
/// addresses the user as "sir"; `friendly` opens with a cheerful word when the reply is very short
/// and ends with "!" otherwise. Short confirmations stay within six words.
pub fn restyle(style: Style, lang: crate::Lang, text: &str) -> String {
    let body = text.trim();
    let Some(core) = body.strip_suffix('.') else {
        return body.to_owned();
    };
    if core.contains('.') || core.is_empty() {
        return body.to_owned();
    }
    let (sir, sure) = match lang {
        crate::Lang::Ru => (", сэр.", "Конечно! "),
        crate::Lang::En => (", sir.", "Sure! "),
    };
    match style {
        Style::Classic => body.to_owned(),
        Style::Formal => format!("{core}{sir}"),
        Style::Friendly => {
            if core.split_whitespace().count() <= 3 {
                format!("{sure}{core}.")
            } else {
                format!("{core}!")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_change_the_wording_but_stay_short() {
        use crate::Lang;
        assert_eq!(
            restyle(Style::Classic, Lang::Ru, "Звук выключен."),
            "Звук выключен."
        );
        assert_eq!(
            restyle(Style::Formal, Lang::Ru, "Звук выключен."),
            "Звук выключен, сэр."
        );
        assert_eq!(
            restyle(Style::Formal, Lang::En, "Sound muted."),
            "Sound muted, sir."
        );
        assert_eq!(
            restyle(Style::Friendly, Lang::Ru, "Звук выключен."),
            "Конечно! Звук выключен."
        );
        assert_eq!(
            restyle(Style::Friendly, Lang::En, "Volume is 30 percent."),
            "Volume is 30 percent!"
        );
        for reply in [
            muted(),
            unmuted(),
            locked(),
            time_now(9, 5),
            volume_set(30),
            open_app("Discord"),
            minimized_all(),
            shutdown_aborted(),
        ] {
            for style in [Style::Formal, Style::Classic, Style::Friendly] {
                for (lang, text) in [(Lang::Ru, &reply.ru), (Lang::En, &reply.en)] {
                    let styled = restyle(style, lang, text);
                    assert!(styled.split_whitespace().count() <= 6, "{styled}");
                }
            }
        }
    }

    #[test]
    fn multi_sentence_and_unterminated_text_is_left_alone() {
        use crate::Lang;
        let long = "Выключение через 30 секунд. Скажите «отмени».";
        assert_eq!(restyle(Style::Formal, Lang::Ru, long), long);
        assert_eq!(restyle(Style::Friendly, Lang::Ru, "Готово"), "Готово");
        assert_eq!(Style::parse("custom"), Some(Style::Classic));
        assert_eq!(Style::parse("nope"), None);
    }

    #[test]
    fn percent_forms() {
        assert_eq!(percent_ru(1), "процент");
        assert_eq!(percent_ru(2), "процента");
        assert_eq!(percent_ru(5), "процентов");
        assert_eq!(percent_ru(11), "процентов");
        assert_eq!(percent_ru(21), "процент");
        assert_eq!(percent_ru(30), "процентов");
    }

    #[test]
    fn volume_reply_matches_style() {
        assert_eq!(volume_set(30).ru, "Громкость — 30 процентов.");
        assert_eq!(locked().ru, "Компьютер заблокирован.");
        assert_eq!(open_app("Discord").ru, "Открываю Discord.");
    }
}
