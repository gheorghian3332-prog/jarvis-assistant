//! Application index (Start Menu, Steam, UWP, aliases) with fuzzy ru/en search, and launching.

pub mod shortcuts;
pub mod steam;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::com::shell_open;
use crate::config::ControlConfig;
use crate::error::ControlResult;
use crate::text::{canonical, phonetic, similarity};

/// The index is rebuilt at most this often unless a refresh is requested.
pub const REBUILD_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Minimum score for a match to be considered a hit.
pub const MIN_SCORE: f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppSource {
    Alias,
    Builtin,
    StartMenu,
    Steam,
    Uwp,
}

impl AppSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AppSource::Alias => "alias",
            AppSource::Builtin => "builtin",
            AppSource::StartMenu => "start-menu",
            AppSource::Steam => "steam",
            AppSource::Uwp => "uwp",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub name: String,
    /// What `ShellExecuteW` opens: shortcut path, `steam://rungameid/ID`, `shell:AppsFolder\AUMID`, exe or URI.
    pub launch: String,
    pub source: AppSource,
    /// Canonical match keys (name first) with their phonetic forms.
    keys: Vec<(String, String)>,
}

impl AppEntry {
    pub fn new(name: &str, launch: &str, source: AppSource, extra_keys: &[&str]) -> Self {
        let mut keys = Vec::new();
        for key in std::iter::once(name).chain(extra_keys.iter().copied()) {
            let canon = canonical(key);
            if !canon.is_empty() && !keys.iter().any(|(known, _)| known == &canon) {
                let phon = phonetic(&canon);
                keys.push((canon, phon));
            }
        }
        Self {
            name: name.to_owned(),
            launch: launch.to_owned(),
            source,
            keys,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppMatch {
    pub name: String,
    pub launch: String,
    pub source: AppSource,
    pub score: f64,
}

#[derive(Debug, Clone, Default)]
pub struct AppIndex {
    entries: Vec<AppEntry>,
}

/// Built-in system tools that are not reliably present as Start Menu shortcuts.
fn builtin_entries() -> Vec<AppEntry> {
    vec![
        AppEntry::new("Notepad", "notepad.exe", AppSource::Builtin, &["блокнот"]),
        AppEntry::new(
            "Calculator",
            "calc.exe",
            AppSource::Builtin,
            &["calc", "калькулятор"],
        ),
        AppEntry::new(
            "Task Manager",
            "taskmgr.exe",
            AppSource::Builtin,
            &["taskmgr", "диспетчер задач"],
        ),
        AppEntry::new(
            "Settings",
            "ms-settings:",
            AppSource::Builtin,
            &["ms settings", "настройки", "параметры"],
        ),
        AppEntry::new(
            "File Explorer",
            "explorer.exe",
            AppSource::Builtin,
            &["explorer", "проводник"],
        ),
    ]
}

/// Spoken-name -> canonical names to look for, best first.
fn synonyms(canon: &str) -> Option<&'static [&'static str]> {
    let list: &'static [&'static str] = match canon {
        "дискорд" | "дискорт" => &["discord"],
        "стим" | "стим клиент" => &["steam"],
        "хром"
        | "гугл хром"
        | "браузер"
        | "интернет браузер"
        | "веб браузер"
        | "browser"
        | "web browser" => &["chrome", "edge", "firefox", "brave", "opera"],
        "телега" | "телеграм" | "телеграмм" | "телек" => &["telegram"],
        "дота" | "дота 2" | "доту" | "доту 2" | "dota" => &["dota 2"],
        "данжен квест 3" | "данжен квест" | "дк 3" | "дк3" | "dq 3" | "dq3" | "dungeon quest 3" => {
            &["dungeon quest 3"]
        }
        "обсидиан" => &["obsidian"],
        "вивальди" => &["vivaldi"],
        "блокнот" => &["notepad"],
        "калькулятор" => &["calculator", "calc"],
        "диспетчер задач" => &["task manager", "taskmgr"],
        "настройки" | "параметры" | "настройка" => {
            &["settings", "ms settings"]
        }
        "проводник" => &["file explorer", "explorer"],
        "ворд" => &["word"],
        "эксель" => &["excel"],
        "паинт" | "пейнт" => &["paint"],
        "спотифай" | "спотик" => &["spotify"],
        "терминал" => &["terminal", "windows terminal"],
        "панель управления" => &["control panel"],
        "вс код" | "виэс код" => &["visual studio code"],
        _ => return None,
    };
    Some(list)
}

fn strip_fillers(canon: &str) -> String {
    const FILLERS: [&str; 14] = [
        "приложение",
        "программу",
        "программа",
        "игру",
        "игра",
        "app",
        "application",
        "program",
        "game",
        "the",
        "мне",
        "пожалуйста",
        "please",
        "клиент",
    ];
    let kept: Vec<&str> = canon
        .split_whitespace()
        .filter(|token| !FILLERS.contains(token))
        .collect();
    if kept.is_empty() {
        canon.to_owned()
    } else {
        kept.join(" ")
    }
}

/// Candidate canonical names for a spoken query: dictionary expansions first, then the query itself.
pub fn expand_query(query: &str) -> Vec<String> {
    let canon = strip_fillers(&canonical(query));
    let mut candidates: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        let value = value.to_owned();
        if !value.is_empty() && !candidates.contains(&value) {
            candidates.push(value);
        }
    };
    let mut dictionary = synonyms(&canon);
    if dictionary.is_none() {
        // Inflected forms: "стима", "дискорду", "блокнота".
        let chars: Vec<char> = canon.chars().collect();
        if chars.len() >= 5 {
            if let Some(last) = chars.last() {
                if "уаыеюи".contains(*last) {
                    let stem: String = chars[..chars.len() - 1].iter().collect();
                    dictionary = synonyms(&stem);
                }
            }
        }
    }
    if let Some(list) = dictionary {
        for candidate in list {
            push(candidate);
        }
    }
    push(&canon);
    candidates
}

fn score_key(candidate: &str, candidate_phon: &str, key: &str, key_phon: &str) -> f64 {
    if candidate == key {
        return 100.0;
    }
    if candidate_phon == key_phon {
        return 97.0;
    }
    let candidate_tokens: Vec<&str> = candidate.split(' ').collect();
    let key_tokens: Vec<&str> = key.split(' ').collect();
    let extra = key_tokens.len().saturating_sub(candidate_tokens.len()) as f64;
    if candidate_tokens
        .iter()
        .all(|token| key_tokens.contains(token))
    {
        return 90.0 - (extra * 2.0).min(15.0);
    }
    let candidate_phon_tokens: Vec<&str> = candidate_phon.split(' ').collect();
    let key_phon_tokens: Vec<&str> = key_phon.split(' ').collect();
    if candidate_phon_tokens
        .iter()
        .all(|token| key_phon_tokens.contains(token))
    {
        return 86.0 - (extra * 2.0).min(15.0);
    }
    if candidate.chars().count() >= 3 && key.starts_with(candidate) {
        return 80.0;
    }
    if candidate_phon.chars().count() >= 4 && key_phon.contains(candidate_phon) {
        return 72.0;
    }
    let whole = similarity(candidate_phon, key_phon);
    if whole >= 0.8 {
        return 50.0 + whole * 20.0;
    }
    if candidate_phon_tokens.len() == 1 && candidate_phon.chars().count() >= 4 {
        let best = key_phon_tokens
            .iter()
            .map(|token| similarity(candidate_phon, token))
            .fold(0.0_f64, f64::max);
        if best >= 0.8 {
            return 50.0 + best * 12.0;
        }
    }
    0.0
}

impl AppIndex {
    /// Builds an index from raw entries, keeping the first entry per canonical name.
    pub fn from_entries(entries: Vec<AppEntry>) -> Self {
        let mut seen: HashSet<String> = HashSet::new();
        let mut unique = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some((canon, _)) = entry.keys.first() else {
                continue;
            };
            if seen.insert(canon.clone()) {
                unique.push(entry);
            }
        }
        Self { entries: unique }
    }

    /// Scans the real machine: aliases, built-ins, Start Menu, Steam, UWP (in dedupe priority order).
    pub fn build(config: &ControlConfig) -> Self {
        let mut entries = Vec::new();
        for (alias, target) in &config.app_aliases {
            entries.push(AppEntry::new(alias, target, AppSource::Alias, &[]));
        }
        entries.extend(builtin_entries());
        for shortcut in shortcuts::scan(&shortcuts::start_menu_dirs()) {
            entries.push(AppEntry::new(
                &shortcut.name,
                &shortcut.path.to_string_lossy(),
                AppSource::StartMenu,
                &[],
            ));
        }
        let roots = steam::steam_roots(&config.steam_roots);
        for game in steam::discover_games(&roots) {
            entries.push(AppEntry::new(
                &game.name,
                &game.launch_uri(),
                AppSource::Steam,
                &[],
            ));
        }
        for (name, aumid) in shortcuts::enumerate_uwp() {
            entries.push(AppEntry::new(
                &name,
                &format!("shell:AppsFolder\\{aumid}"),
                AppSource::Uwp,
                &[],
            ));
        }
        Self::from_entries(entries)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[AppEntry] {
        &self.entries
    }

    pub fn count_by_source(&self, source: AppSource) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.source == source)
            .count()
    }

    /// Ranked matches (best first) with score >= [`MIN_SCORE`].
    pub fn search(&self, query: &str, limit: usize) -> Vec<AppMatch> {
        let candidates: Vec<(String, String)> = expand_query(query)
            .into_iter()
            .map(|candidate| {
                let phon = phonetic(&candidate);
                (candidate, phon)
            })
            .collect();
        let mut matches: Vec<AppMatch> = Vec::new();
        for entry in &self.entries {
            let mut best = 0.0_f64;
            for (rank, (candidate, candidate_phon)) in candidates.iter().enumerate() {
                for (key, key_phon) in &entry.keys {
                    let score =
                        score_key(candidate, candidate_phon, key, key_phon) - rank as f64 * 0.5;
                    best = best.max(score);
                }
            }
            if best >= MIN_SCORE {
                matches.push(AppMatch {
                    name: entry.name.clone(),
                    launch: entry.launch.clone(),
                    source: entry.source,
                    score: best,
                });
            }
        }
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.name.len().cmp(&b.name.len()))
        });
        matches.truncate(limit);
        matches
    }

    pub fn best(&self, query: &str) -> Option<AppMatch> {
        self.search(query, 1).into_iter().next()
    }
}

/// Launches an indexed application via `ShellExecuteW`.
pub fn launch(entry: &AppMatch) -> ControlResult<()> {
    shell_open(&entry.launch, None)
}

/// Index with a rebuild throttle (at most once per [`REBUILD_INTERVAL`] unless forced).
#[derive(Debug, Default)]
pub struct AppCache {
    built_at: Option<Instant>,
    index: Option<std::sync::Arc<AppIndex>>,
}

impl AppCache {
    pub fn get(&mut self, config: &ControlConfig, force: bool) -> std::sync::Arc<AppIndex> {
        let fresh = self
            .built_at
            .is_some_and(|built| built.elapsed() < REBUILD_INTERVAL);
        if let (false, true, Some(index)) = (force, fresh, self.index.as_ref()) {
            return index.clone();
        }
        let index = std::sync::Arc::new(AppIndex::build(config));
        self.index = Some(index.clone());
        self.built_at = Some(Instant::now());
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> AppIndex {
        let mut entries = builtin_entries();
        for (name, launch) in [
            ("Discord", "C:\\Start\\Discord.lnk"),
            ("Telegram", "C:\\Start\\Telegram.lnk"),
            ("Steam", "C:\\Start\\Steam.lnk"),
            ("Google Chrome", "C:\\Start\\Google Chrome.lnk"),
            ("Microsoft Edge", "C:\\Start\\Edge.lnk"),
            ("Obsidian", "C:\\Start\\Obsidian.lnk"),
            ("Vivaldi", "C:\\Start\\vivaldi.lnk"),
            ("Visual Studio Code", "C:\\Start\\Code.lnk"),
            ("Spotify", "C:\\Start\\Spotify.lnk"),
            ("OBS Studio", "C:\\Start\\OBS.lnk"),
        ] {
            entries.push(AppEntry::new(name, launch, AppSource::StartMenu, &[]));
        }
        for (name, id) in [
            ("DUNGEON QUEST III", 374320),
            ("Dota 2", 570),
            ("Dungeon Quest Remastered", 570_940),
        ] {
            entries.push(AppEntry::new(
                name,
                &format!("steam://rungameid/{id}"),
                AppSource::Steam,
                &[],
            ));
        }
        AppIndex::from_entries(entries)
    }

    fn best_name(index: &AppIndex, query: &str) -> Option<String> {
        index.best(query).map(|found| found.name)
    }

    #[test]
    fn finds_apps_by_russian_and_english_names() {
        let index = fixture();
        assert_eq!(best_name(&index, "дискорд").as_deref(), Some("Discord"));
        assert_eq!(best_name(&index, "Discord").as_deref(), Some("Discord"));
        assert_eq!(best_name(&index, "стим").as_deref(), Some("Steam"));
        assert_eq!(best_name(&index, "телеграм").as_deref(), Some("Telegram"));
        assert_eq!(best_name(&index, "телега").as_deref(), Some("Telegram"));
        assert_eq!(best_name(&index, "хром").as_deref(), Some("Google Chrome"));
        assert_eq!(
            best_name(&index, "браузер").as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(best_name(&index, "обсидиан").as_deref(), Some("Obsidian"));
        assert_eq!(best_name(&index, "вивальди").as_deref(), Some("Vivaldi"));
        assert_eq!(best_name(&index, "дота").as_deref(), Some("Dota 2"));
    }

    #[test]
    fn dungeon_quest_roman_and_arabic_numerals() {
        let index = fixture();
        for query in [
            "Dungeon Quest 3",
            "dungeon quest III",
            "данжен квест 3",
            "данжен квест три",
            "дк3",
            "дк 3",
        ] {
            assert_eq!(
                best_name(&index, query).as_deref(),
                Some("DUNGEON QUEST III"),
                "query {query}"
            );
        }
    }

    #[test]
    fn system_tools_and_synonyms() {
        let index = fixture();
        assert_eq!(best_name(&index, "блокнот").as_deref(), Some("Notepad"));
        assert_eq!(
            best_name(&index, "калькулятор").as_deref(),
            Some("Calculator")
        );
        assert_eq!(
            best_name(&index, "диспетчер задач").as_deref(),
            Some("Task Manager")
        );
        assert_eq!(best_name(&index, "настройки").as_deref(), Some("Settings"));
        assert_eq!(
            best_name(&index, "проводник").as_deref(),
            Some("File Explorer")
        );
        assert_eq!(
            best_name(&index, "вс код").as_deref(),
            Some("Visual Studio Code")
        );
    }

    #[test]
    fn inflections_and_typos() {
        let index = fixture();
        assert_eq!(best_name(&index, "стима").as_deref(), Some("Steam"));
        assert_eq!(best_name(&index, "дискорда").as_deref(), Some("Discord"));
        assert_eq!(best_name(&index, "spotifi").as_deref(), Some("Spotify"));
    }

    #[test]
    fn unknown_names_do_not_match() {
        let index = fixture();
        assert!(index.best("квантовый телепорт").is_none());
        assert!(index.best("zzzz").is_none());
    }

    #[test]
    fn aliases_take_priority_and_dedupe() {
        let mut entries = vec![AppEntry::new(
            "рабочая вивальди",
            "D:\\vivaldi\\vivaldi.exe",
            AppSource::Alias,
            &[],
        )];
        entries.extend(builtin_entries());
        entries.push(AppEntry::new("Notepad", "dup", AppSource::StartMenu, &[]));
        let index = AppIndex::from_entries(entries);
        assert_eq!(index.len(), 6);
        assert_eq!(
            index.best("рабочая вивальди").map(|found| found.source),
            Some(AppSource::Alias)
        );
        assert_eq!(index.count_by_source(AppSource::Builtin), 5);
    }
}
