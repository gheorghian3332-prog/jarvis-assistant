//! Start Menu shortcut scanning (`.lnk` via `IShellLinkW`) and UWP/Store enumeration (`shell:AppsFolder`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellLinkW,
    SHGetKnownFolderItem, ShellLink, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING, SLGP_RAWPATH,
};

use crate::com::ComGuard;

const MAX_DEPTH: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub name: String,
    pub path: PathBuf,
    pub target: String,
}

pub fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(program_data) = env::var_os("ProgramData") {
        dirs.push(PathBuf::from(program_data).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Some(app_data) = env::var_os("APPDATA") {
        dirs.push(PathBuf::from(app_data).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    dirs
}

/// Resolved target of a `.lnk` file, or `None` when it cannot be loaded.
pub fn read_lnk_target(path: &Path) -> Option<String> {
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: standard IShellLinkW/IPersistFile usage; buffers are local and sized by the API contract.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut buffer = [0u16; 520];
        let mut find = WIN32_FIND_DATAW::default();
        if link
            .GetPath(&mut buffer, &mut find, SLGP_RAWPATH.0 as u32)
            .is_err()
        {
            return Some(String::new());
        }
        let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}

/// Names that never make useful launch targets.
pub fn is_noise(name: &str, target: &str) -> bool {
    let lower = name.to_lowercase();
    let target_lower = target.to_lowercase();
    let target_file = target_lower.rsplit('\\').next().unwrap_or("");
    lower.contains("uninstall")
        || lower.contains("удалить")
        || lower.contains("удаление")
        || lower.starts_with("readme")
        || lower.contains("documentation")
        || lower.contains("release notes")
        || target_file.starts_with("unins")
        || target_file.ends_with(".chm")
        || target_file.ends_with(".txt")
        || target_file.ends_with(".pdf")
        || target_file.ends_with(".html")
        || target_file.ends_with(".htm")
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(&path, depth + 1, out);
        } else if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            let extension = extension.to_ascii_lowercase();
            if extension == "lnk" || extension == "url" {
                out.push(path);
            }
        }
    }
}

/// All usable shortcuts under the given Start Menu folders.
pub fn scan(dirs: &[PathBuf]) -> Vec<Shortcut> {
    let _com = ComGuard::init();
    let mut files = Vec::new();
    for dir in dirs {
        walk(dir, 0, &mut files);
    }
    let mut shortcuts = Vec::new();
    for path in files {
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let is_lnk = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
        let target = if is_lnk {
            read_lnk_target(&path).unwrap_or_default()
        } else {
            String::new()
        };
        if is_noise(name, &target) {
            continue;
        }
        shortcuts.push(Shortcut {
            name: name.to_owned(),
            path,
            target,
        });
    }
    shortcuts
}

fn take_string(pwstr: windows::core::PWSTR) -> String {
    // SAFETY: the shell returns a NUL-terminated CoTaskMem string that we free exactly once.
    unsafe {
        let text = pwstr.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pwstr.0 as *const _));
        text
    }
}

/// UWP / Store apps from `shell:AppsFolder`: (display name, AppUserModelID).
/// Only packaged apps (AppUserModelID contains `!`) are returned; classic apps come from the Start Menu scan.
pub fn enumerate_uwp() -> Vec<(String, String)> {
    let _com = ComGuard::init();
    let mut apps = Vec::new();
    // SAFETY: COM calls on an initialized apartment; every returned string is freed via `take_string`.
    unsafe {
        let Ok(folder) = SHGetKnownFolderItem::<_, IShellItem>(
            &FOLDERID_AppsFolder,
            KF_FLAG_DEFAULT,
            HANDLE::default(),
        ) else {
            return apps;
        };
        let Ok(enumerator) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems)
        else {
            return apps;
        };
        loop {
            let mut items: [Option<IShellItem>; 1] = [None];
            let mut fetched = 0u32;
            if enumerator.Next(&mut items, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = items[0].take() else {
                break;
            };
            let Ok(display) = item.GetDisplayName(SIGDN_NORMALDISPLAY) else {
                continue;
            };
            let Ok(parsing) = item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING) else {
                CoTaskMemFree(Some(display.0 as *const _));
                continue;
            };
            let name = take_string(display);
            let aumid = take_string(parsing);
            if aumid.contains('!') && !name.is_empty() {
                apps.push((name, aumid));
            }
        }
    }
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_filter() {
        assert!(is_noise("Uninstall Foo", ""));
        assert!(is_noise("Foo", "C:\\Foo\\unins000.exe"));
        assert!(is_noise("Foo manual", "C:\\Foo\\manual.pdf"));
        assert!(!is_noise("Discord", "C:\\Users\\x\\Discord\\Update.exe"));
    }

    #[test]
    fn scan_reads_fixture_url_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("Sub")).expect("mkdir");
        fs::write(
            dir.path().join("Sub").join("Cool Game.url"),
            "[InternetShortcut]\nURL=steam://rungameid/1\n",
        )
        .expect("write");
        fs::write(dir.path().join("Uninstall Cool Game.url"), "").expect("write");
        let found = scan(&[dir.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Cool Game");
    }
}
