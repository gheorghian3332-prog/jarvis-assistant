//! Finds `libvosk` (import library and DLLs) and copies the DLLs next to the built executables.
//!
//! The files come from `scripts/fetch-models.ps1` and live in `%LOCALAPPDATA%\Jarvis\models\libvosk`
//! (override the directory with `JARVIS_VOSK_LIB_DIR`).

use std::{env, fs, path::PathBuf};

fn vosk_dir() -> PathBuf {
    if let Some(dir) = env::var_os("JARVIS_VOSK_LIB_DIR") {
        return PathBuf::from(dir);
    }
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join("Jarvis").join("models").join("libvosk")
}

fn main() {
    println!("cargo:rerun-if-env-changed=JARVIS_VOSK_LIB_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    let dir = vosk_dir();
    let import_library = dir.join("libvosk.lib");
    if !import_library.is_file() {
        panic!(
            "libvosk.lib not found in {}. Run scripts/fetch-models.ps1 first \
             (or set JARVIS_VOSK_LIB_DIR to the folder with libvosk.lib and libvosk.dll).",
            dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", import_library.display());
    println!("cargo:rustc-link-search=native={}", dir.display());

    // OUT_DIR is target/<profile>/build/<crate>/<hash>/out (or .../build/<crate>-<hash>/out in older
    // layouts); executables land in target/<profile>, the parent of the `build` folder.
    let Some(out_dir) = env::var_os("OUT_DIR").map(PathBuf::from) else {
        return;
    };
    let Some(profile_dir) = out_dir
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "build"))
        .and_then(|build| build.parent())
        .map(PathBuf::from)
    else {
        return;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "dll") {
            println!("cargo:rerun-if-changed={}", path.display());
            let Some(name) = path.file_name() else {
                continue;
            };
            let deps = profile_dir.join("deps");
            for target in [profile_dir.clone(), deps] {
                if target.is_dir() {
                    if let Err(error) = fs::copy(&path, target.join(name)) {
                        println!("cargo:warning=could not copy {}: {error}", path.display());
                    }
                }
            }
        }
    }
}
