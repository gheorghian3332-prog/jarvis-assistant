//! Action registry: name -> action, with switch-off support for individual actions.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::action::{Action, Risk};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionInfo {
    pub name: String,
    pub risk: Risk,
    pub description_ru: String,
    pub description_en: String,
    pub enabled: bool,
}

#[derive(Default)]
pub struct Registry {
    actions: BTreeMap<String, Arc<dyn Action>>,
    disabled: RwLock<HashSet<String>>,
}

impl Registry {
    pub fn new(disabled: HashSet<String>) -> Self {
        Self {
            actions: BTreeMap::new(),
            disabled: RwLock::new(disabled),
        }
    }

    pub fn register(&mut self, action: Arc<dyn Action>) {
        self.actions.insert(action.name().to_owned(), action);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Action>> {
        self.actions.get(name).cloned()
    }

    /// Replaces the set of switched-off actions (P6-B feeds this from settings).
    pub fn set_disabled(&self, disabled: HashSet<String>) {
        if let Ok(mut guard) = self.disabled.write() {
            *guard = disabled;
        }
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.disabled
            .read()
            .map(|disabled| !disabled.contains(name))
            .unwrap_or(false)
    }

    pub fn list(&self) -> Vec<ActionInfo> {
        self.actions
            .values()
            .map(|action| ActionInfo {
                name: action.name().to_owned(),
                risk: action.risk(),
                description_ru: action.description_ru().to_owned(),
                description_en: action.description_en().to_owned(),
                enabled: self.is_enabled(action.name()),
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}
