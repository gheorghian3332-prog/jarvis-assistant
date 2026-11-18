//! Steam library discovery: `libraryfolders.vdf` + `appmanifest_*.acf` -> installed games.

use std::fs;
use std::path::{Path, PathBuf};

use windows::core::w;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Case-insensitive child lookup (Steam files mix `AppID` / `appid`).
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(entries) => entries
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value),
            Vdf::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(text) => Some(text),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(entries) => entries,
            Vdf::Str(_) => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Text(String),
    Open,
    Close,
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '"' => {
                let mut text = String::new();
                while let Some(inner) = chars.next() {
                    match inner {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => text.push('\n'),
                            Some('t') => text.push('\t'),
                            Some(other) => text.push(other),
                            None => break,
                        },
                        other => text.push(other),
                    }
                }
                tokens.push(Token::Text(text));
            }
            '/' if chars.peek() == Some(&'/') => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    tokens
}

fn parse_object(tokens: &[Token], position: &mut usize) -> Option<Vec<(String, Vdf)>> {
    let mut entries = Vec::new();
    while *position < tokens.len() {
        match &tokens[*position] {
            Token::Close => {
                *position += 1;
                return Some(entries);
            }
            Token::Text(key) => {
                *position += 1;
                match tokens.get(*position)? {
                    Token::Text(value) => {
                        entries.push((key.clone(), Vdf::Str(value.clone())));
                        *position += 1;
                    }
                    Token::Open => {
                        *position += 1;
                        let child = parse_object(tokens, position)?;
                        entries.push((key.clone(), Vdf::Obj(child)));
                    }
                    Token::Close => return None,
                }
            }
            Token::Open => return None,
        }
    }
    Some(entries)
}

/// Parses a VDF/ACF document into a root object. Returns `None` for malformed input.
pub fn parse_vdf(input: &str) -> Option<Vdf> {
    let tokens = tokenize(input);
    let mut position = 0;
    let entries = parse_object(&tokens, &mut position)?;
    Some(Vdf::Obj(entries))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
}

impl SteamGame {
    pub fn launch_uri(&self) -> String {
        format!("steam://rungameid/{}", self.appid)
    }
}

/// Library folders (`path` values) from `libraryfolders.vdf`.
pub fn library_paths(vdf_text: &str) -> Vec<PathBuf> {
    let Some(root) = parse_vdf(vdf_text) else {
        return Vec::new();
    };
    let Some(folders) = root.get("libraryfolders") else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for (key, value) in folders.entries() {
        if !key.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let path = match value {
            Vdf::Obj(_) => value.get("path").and_then(Vdf::as_str),
            Vdf::Str(text) => Some(text.as_str()),
        };
        if let Some(path) = path {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

fn is_tooling(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("steamworks")
        || lower.starts_with("steam linux runtime")
        || lower.starts_with("proton")
        || lower.contains("redistributable")
        || lower.starts_with("steam controller")
}

/// One `appmanifest_*.acf` -> game. Tooling entries (Steamworks, Proton, redistributables) are skipped.
pub fn parse_manifest(acf_text: &str) -> Option<SteamGame> {
    let root = parse_vdf(acf_text)?;
    let state = root.get("AppState")?;
    let appid = state.get("appid")?.as_str()?.trim().parse().ok()?;
    let name = state.get("name")?.as_str()?.trim().to_owned();
    if name.is_empty() || is_tooling(&name) {
        return None;
    }
    Some(SteamGame { appid, name })
}

fn steam_path_from_registry() -> Option<PathBuf> {
    let mut buffer = [0u16; 520];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer outlives the call and `size` states its byte capacity.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let units = (size as usize / 2).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..units]);
    let trimmed = text.trim_end_matches('\0');
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed.replace('/', "\\")))
    }
}

/// Steam installation roots: registry, common locations, plus caller-supplied extras.
pub fn steam_roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut candidates: Vec<PathBuf> = extra.to_vec();
    if let Some(path) = steam_path_from_registry() {
        candidates.push(path);
    }
    for fallback in [
        "C:\\Program Files (x86)\\Steam",
        "C:\\Program Files\\Steam",
        "D:\\Steam",
        "E:\\Steam",
    ] {
        candidates.push(PathBuf::from(fallback));
    }
    for candidate in candidates {
        let known = roots.iter().any(|root| {
            root.to_string_lossy()
                .eq_ignore_ascii_case(&candidate.to_string_lossy())
        });
        if candidate.join("steamapps").is_dir() && !known {
            roots.push(candidate);
        }
    }
    roots
}

/// Installed games from every library folder of the given Steam roots.
pub fn discover_games(roots: &[PathBuf]) -> Vec<SteamGame> {
    let mut libraries: Vec<PathBuf> = Vec::new();
    for root in roots {
        let steamapps = root.join("steamapps");
        if !libraries.contains(root) {
            libraries.push(root.clone());
        }
        if let Ok(text) = fs::read_to_string(steamapps.join("libraryfolders.vdf")) {
            for path in library_paths(&text) {
                if !libraries.contains(&path) {
                    libraries.push(path);
                }
            }
        }
    }
    let mut games: Vec<SteamGame> = Vec::new();
    for library in libraries {
        games_in_library(&library.join("steamapps"), &mut games);
    }
    games
}

fn games_in_library(steamapps: &Path, games: &mut Vec<SteamGame>) {
    let Ok(read_dir) = fs::read_dir(steamapps) else {
        return;
    };
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_lowercase();
        if !(file_name.starts_with("appmanifest_") && file_name.ends_with(".acf")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(game) = parse_manifest(&text) {
            if !games.iter().any(|known| known.appid == game.appid) {
                games.push(game);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"228980"		"123"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}
"#;

    const DUNGEON_QUEST: &str = r#"
"AppState"
{
	"appid"		"374320"
	"Universe"		"1"
	"name"		"DUNGEON QUEST III"
	"StateFlags"		"4"
	"installdir"		"DUNGEON QUEST III"
	// comment
}
"#;

    #[test]
    fn parses_library_folders() {
        let paths = library_paths(LIBRARY);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("C:\\Program Files (x86)\\Steam"),
                PathBuf::from("D:\\SteamLibrary")
            ]
        );
    }

    #[test]
    fn parses_manifest() {
        let game = parse_manifest(DUNGEON_QUEST).expect("manifest parses");
        assert_eq!(game.appid, 374320);
        assert_eq!(game.name, "DUNGEON QUEST III");
        assert_eq!(game.launch_uri(), "steam://rungameid/374320");
    }

    #[test]
    fn skips_tooling_and_garbage() {
        let tooling =
            r#""AppState" { "appid" "228980" "name" "Steamworks Common Redistributables" }"#;
        assert!(parse_manifest(tooling).is_none());
        assert!(parse_manifest("\"AppState\" {").is_none() || parse_manifest("garbage").is_none());
        assert!(parse_manifest("not vdf at all").is_none());
        assert!(library_paths("}{").is_empty());
    }

    #[test]
    fn discovers_games_in_fixture_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let steamapps = dir.path().join("steamapps");
        fs::create_dir_all(&steamapps).expect("mkdir");
        fs::write(steamapps.join("appmanifest_374320.acf"), DUNGEON_QUEST).expect("write");
        fs::write(
            steamapps.join("appmanifest_570.acf"),
            r#""AppState" { "appid" "570" "name" "Dota 2" }"#,
        )
        .expect("write");
        fs::write(
            steamapps.join("libraryfolders.vdf"),
            "\"libraryfolders\" { }",
        )
        .expect("write");
        let mut games = discover_games(&[dir.path().to_path_buf()]);
        games.sort_by_key(|game| game.appid);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].name, "Dota 2");
        assert_eq!(games[1].appid, 374320);
    }
}
