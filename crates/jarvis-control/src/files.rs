//! Known folders, path sandbox, directory listing and delete-to-Recycle-Bin (`IFileOperation`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use windows::core::HSTRING;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Videos, FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
    SHGetKnownFolderPath, FILEOPERATION_FLAGS, FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO,
    FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, KF_FLAG_DEFAULT,
};

use crate::com::{shell_open, ComGuard};
use crate::error::{ControlError, ControlResult};
use crate::text::normalize;

pub const LIST_LIMIT: usize = 200;

/// Canonical key of a known folder from an English or Russian spoken name.
pub fn known_folder_key(text: &str) -> Option<&'static str> {
    let normalized = normalize(text);
    let normalized = normalized
        .trim_start_matches("папку ")
        .trim_start_matches("папка ")
        .trim_start_matches("папки ")
        .trim_start_matches("folder ")
        .trim();
    let key = match normalized {
        "downloads" | "download" | "загрузки" | "загрузок" | "загрузка" | "загрузкам" => {
            "downloads"
        }
        "documents" | "document" | "документы" | "документов" | "документ" | "мои документы" => {
            "documents"
        }
        "desktop" | "рабочий стол" | "рабочего стола" => "desktop",
        "pictures"
        | "picture"
        | "изображения"
        | "изображений"
        | "картинки"
        | "картинок"
        | "фото"
        | "фотографии"
        | "фотографий" => "pictures",
        "music" | "музыка" | "музыки" | "музыку" => "music",
        "videos" | "video" | "видео" | "видеозаписи" => "videos",
        _ => return None,
    };
    Some(key)
}

/// Path of a known folder key (`downloads`, `documents`, `desktop`, `pictures`, `music`, `videos`).
pub fn known_folder_path(key: &str) -> ControlResult<PathBuf> {
    let id = match key {
        "downloads" => &FOLDERID_Downloads,
        "documents" => &FOLDERID_Documents,
        "desktop" => &FOLDERID_Desktop,
        "pictures" => &FOLDERID_Pictures,
        "music" => &FOLDERID_Music,
        "videos" => &FOLDERID_Videos,
        other => {
            return Err(ControlError::validation(format!(
                "unknown known folder \"{other}\""
            )))
        }
    };
    // SAFETY: the returned CoTaskMem string is copied and freed exactly once.
    unsafe {
        let pwstr = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, HANDLE::default())?;
        let text = pwstr.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pwstr.0 as *const _));
        if text.is_empty() {
            return Err(ControlError::not_found(format!(
                "folder \"{key}\" has no path"
            )));
        }
        Ok(PathBuf::from(text))
    }
}

/// Spoken drive letter ("е", "e", "дэ") -> ASCII letter. Only Latin letters and the Russian
/// homoglyphs that voice recognition produces are accepted.
pub fn drive_letter(text: &str) -> Option<char> {
    let normalized = normalize(text);
    let word = normalized.split_whitespace().last()?;
    let letter = match word {
        "c" | "с" | "си" => 'C',
        "d" | "д" | "ди" | "дэ" => 'D',
        "e" | "е" | "и" | "э" => 'E',
        "f" | "ф" | "эф" => 'F',
        "g" | "г" | "джи" => 'G',
        _ => return None,
    };
    Some(letter)
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix("\\\\?\\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

fn lower(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn is_inside(path: &Path, root: &Path) -> bool {
    let path = lower(path);
    let root = lower(root);
    let root = root.trim_end_matches('\\');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

/// Rejects anything that is not a plain absolute drive path: UNC, device paths, `..`, alternate
/// data streams, NUL bytes and relative paths.
fn check_syntax(raw: &str) -> ControlResult<PathBuf> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(ControlError::validation("empty or malformed path"));
    }
    if trimmed.starts_with("\\\\") || trimmed.starts_with("//") {
        return Err(ControlError::forbidden(
            "UNC and device paths are not allowed",
        ));
    }
    let path = PathBuf::from(trimmed);
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(_)) => {}
        _ => {
            return Err(ControlError::validation(
                "an absolute path with a drive letter is required",
            ))
        }
    }
    for component in components {
        match component {
            Component::ParentDir => {
                return Err(ControlError::forbidden("\"..\" is not allowed in paths"))
            }
            Component::Normal(part) if part.to_string_lossy().contains(':') => {
                return Err(ControlError::forbidden(
                    "alternate data streams are not allowed",
                ))
            }
            _ => {}
        }
    }
    Ok(path)
}

/// Canonicalizes an existing path and checks it lies inside one of the allowed roots
/// (symlinks are resolved first, so a link pointing outside a root is rejected).
pub fn resolve_in_roots(raw: &str, roots: &[PathBuf]) -> ControlResult<PathBuf> {
    let path = check_syntax(raw)?;
    let canonical = fs::canonicalize(&path)
        .map(|resolved| strip_verbatim(&resolved))
        .map_err(|_| ControlError::not_found(format!("path does not exist: {}", path.display())))?;
    for root in roots {
        let root_canonical = fs::canonicalize(root)
            .map(|resolved| strip_verbatim(&resolved))
            .unwrap_or_else(|_| root.clone());
        if is_inside(&canonical, &root_canonical) {
            return Ok(canonical);
        }
    }
    Err(ControlError::forbidden(format!(
        "path is outside the allowed folders: {}",
        canonical.display()
    )))
}

/// Like [`resolve_in_roots`] but the path must be strictly inside a root (a root itself is refused).
pub fn resolve_strictly_inside(raw: &str, roots: &[PathBuf]) -> ControlResult<PathBuf> {
    let resolved = resolve_in_roots(raw, roots)?;
    for root in roots {
        let root_canonical = fs::canonicalize(root)
            .map(|path| strip_verbatim(&path))
            .unwrap_or_else(|_| root.clone());
        if lower(&resolved).trim_end_matches('\\') == lower(&root_canonical).trim_end_matches('\\')
        {
            return Err(ControlError::forbidden(
                "an allowed root itself cannot be deleted",
            ));
        }
    }
    Ok(resolved)
}

/// A drive root like `E:\` that exists on this machine.
pub fn drive_root(letter: char) -> ControlResult<PathBuf> {
    if !letter.is_ascii_alphabetic() {
        return Err(ControlError::validation("a drive letter is required"));
    }
    let root = PathBuf::from(format!("{}:\\", letter.to_ascii_uppercase()));
    if root.exists() {
        Ok(root)
    } else {
        Err(ControlError::not_found(format!(
            "drive {} does not exist",
            root.display()
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Directory listing: folders first, then files by name, at most [`LIST_LIMIT`] entries.
pub fn list_dir(path: &Path) -> ControlResult<(Vec<DirEntry>, usize)> {
    let read = fs::read_dir(path).map_err(|error| {
        ControlError::system(format!("cannot read {}: {error}", path.display()))
    })?;
    let mut entries: Vec<DirEntry> = read
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: metadata.is_dir(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let total = entries.len();
    entries.truncate(LIST_LIMIT);
    Ok((entries, total))
}

/// Opens a folder in Explorer.
pub fn open_in_explorer(path: &Path) -> ControlResult<()> {
    shell_open(&path.to_string_lossy(), None)
}

/// Moves a file or folder to the Recycle Bin. Never deletes permanently: if the shell cannot
/// recycle the item the operation fails.
pub fn delete_to_recycle_bin(path: &Path) -> ControlResult<()> {
    let _com = ComGuard::init();
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: standard IFileOperation usage on an initialized apartment.
    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        let flags = FILEOPERATION_FLAGS(
            FOF_ALLOWUNDO.0
                | FOFX_RECYCLEONDELETE.0
                | FOF_NOCONFIRMATION.0
                | FOF_NOERRORUI.0
                | FOF_SILENT.0,
        );
        operation.SetOperationFlags(flags)?;
        let item: IShellItem = SHCreateItemFromParsingName(&wide, None)?;
        operation.DeleteItem(&item, None)?;
        operation.PerformOperations()?;
        if operation.GetAnyOperationsAborted()?.as_bool() {
            return Err(ControlError::system(
                "the Recycle Bin operation was aborted",
            ));
        }
    }
    if path.exists() {
        return Err(ControlError::system(
            "the item is still present after the Recycle Bin operation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = vec![dir.path().to_path_buf()];
        (dir, roots)
    }

    #[test]
    fn known_folder_words() {
        assert_eq!(known_folder_key("загрузок"), Some("downloads"));
        assert_eq!(known_folder_key("папку Загрузки"), Some("downloads"));
        assert_eq!(known_folder_key("Documents"), Some("documents"));
        assert_eq!(known_folder_key("рабочий стол"), Some("desktop"));
        assert_eq!(known_folder_key("видео"), Some("videos"));
        assert_eq!(known_folder_key("секретная"), None);
    }

    #[test]
    fn drive_letters_from_speech() {
        assert_eq!(drive_letter("диск E"), Some('E'));
        assert_eq!(drive_letter("диск е"), Some('E'));
        assert_eq!(drive_letter("drive d"), Some('D'));
        assert_eq!(drive_letter("диск"), None);
    }

    #[test]
    fn accepts_paths_inside_root() {
        let (dir, roots) = temp_root();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x").expect("write");
        let resolved = resolve_in_roots(&file.to_string_lossy(), &roots).expect("inside");
        assert!(lower(&resolved).ends_with("a.txt"));
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        let (dir, roots) = temp_root();
        let sneaky = format!("{}\\sub\\..\\..\\secret.txt", dir.path().display());
        let error = resolve_in_roots(&sneaky, &roots).expect_err("traversal");
        assert_eq!(error.code, crate::error::ErrorCode::Forbidden);
    }

    #[test]
    fn rejects_other_drive_unc_and_relative() {
        let (_dir, roots) = temp_root();
        for bad in [
            "\\\\server\\share\\file.txt",
            "//server/share/file.txt",
            "\\\\?\\C:\\Windows\\notepad.exe",
            "relative\\file.txt",
            "",
        ] {
            assert!(resolve_in_roots(bad, &roots).is_err(), "{bad}");
        }
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let error = resolve_in_roots(&windows, &roots).expect_err("outside root");
        assert_eq!(error.code, crate::error::ErrorCode::Forbidden);
    }

    #[test]
    fn rejects_alternate_data_streams() {
        let (dir, roots) = temp_root();
        let ads = format!("{}\\a.txt:hidden", dir.path().display());
        assert!(resolve_in_roots(&ads, &roots).is_err());
    }

    #[test]
    fn root_itself_cannot_be_deleted() {
        let (dir, roots) = temp_root();
        let error = resolve_strictly_inside(&dir.path().to_string_lossy(), &roots)
            .expect_err("root refused");
        assert_eq!(error.code, crate::error::ErrorCode::Forbidden);
        let file = dir.path().join("x.txt");
        fs::write(&file, "x").expect("write");
        assert!(resolve_strictly_inside(&file.to_string_lossy(), &roots).is_ok());
    }

    #[test]
    fn lists_directories_first() {
        let (dir, _roots) = temp_root();
        fs::create_dir(dir.path().join("zdir")).expect("dir");
        fs::write(dir.path().join("a.txt"), "hello").expect("file");
        let (entries, total) = list_dir(dir.path()).expect("list");
        assert_eq!(total, 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "a.txt");
        assert_eq!(entries[1].size, 5);
    }

    #[test]
    fn sibling_prefix_is_not_inside() {
        assert!(is_inside(
            Path::new("C:\\Users\\me\\a"),
            Path::new("C:\\Users\\me")
        ));
        assert!(!is_inside(
            Path::new("C:\\Users\\mellow"),
            Path::new("C:\\Users\\me")
        ));
        assert!(is_inside(
            Path::new("c:\\users\\ME\\a"),
            Path::new("C:\\Users\\me\\")
        ));
    }
}
