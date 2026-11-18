//! Runtime configuration passed in from outside (settings are owned by P6-B).

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct ControlConfig {
    /// Roots that file actions may touch. Empty = defaults (see [`default_allowed_roots`]).
    pub allowed_roots: Vec<PathBuf>,
    /// User aliases: spoken name -> launch target (path, URI or shell: moniker).
    pub app_aliases: HashMap<String, String>,
    /// Extra Steam installation folders, in addition to the autodetected ones.
    pub steam_roots: Vec<PathBuf>,
    /// Action names that are switched off.
    pub disabled_actions: HashSet<String>,
}

impl ControlConfig {
    pub fn effective_roots(&self) -> Vec<PathBuf> {
        if self.allowed_roots.is_empty() {
            default_allowed_roots()
        } else {
            self.allowed_roots.clone()
        }
    }
}

/// User profile plus non-system fixed drives `D:\` and `E:\` (only when they exist).
pub fn default_allowed_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(profile) = env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(profile));
    }
    for drive in ["D:\\", "E:\\"] {
        let path = PathBuf::from(drive);
        if path.exists() {
            roots.push(path);
        }
    }
    roots
}
