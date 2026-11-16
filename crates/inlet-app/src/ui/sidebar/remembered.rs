// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the sidebar and the rail remember between runs, kept under `"navigation"` in
//! `settings.json`: the categories folded away, the folders that are open, and the
//! channels, categories and servers whose content warning was accepted.
//!
//! Nothing here is secret and nothing here is a message. Every list is capped, so a
//! hand-edited or very old file cannot grow the app's memory; when a list is full the
//! oldest entries go first.
// Deviation: Fluxer keeps the open folders and the accepted content warnings in the
// account's synced preferences, and the folded categories in the account's notification
// settings; Inlet keeps them on this computer. They do not follow the account to
// another device or another Fluxer client.

use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::backend::platform::prefs;

/// Key of the section in `settings.json`.
const SETTINGS_KEY: &str = "navigation";
/// Entries kept in each list.
pub const MAX_REMEMBERED: usize = 2_000;
/// Longest ID kept (snowflakes are 20 digits at most).
const MAX_ID_LEN: usize = 20;

/// A list of IDs that keeps the newest [`MAX_REMEMBERED`] and no duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdList(VecDeque<String>);

fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

impl IdList {
    pub fn contains(&self, id: &str) -> bool {
        self.0.iter().any(|i| i == id)
    }

    /// Adds an ID (a value that is not a snowflake is ignored). Whether the list changed.
    pub fn insert(&mut self, id: &str) -> bool {
        if !is_id(id) || self.contains(id) {
            return false;
        }
        if self.0.len() >= MAX_REMEMBERED {
            self.0.pop_front();
        }
        self.0.push_back(id.to_owned());
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|i| i != id);
        self.0.len() != before
    }

    /// Drops what a hand-edited file may carry: other values than snowflakes,
    /// duplicates, and everything past the cap.
    fn checked(mut self) -> Self {
        let mut seen = std::collections::HashSet::new();
        self.0.retain(|i| is_id(i) && seen.insert(i.clone()));
        while self.0.len() > MAX_REMEMBERED {
            self.0.pop_front();
        }
        self
    }
}

/// The remembered navigation state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Remembered {
    /// Categories folded away in the channel list.
    pub collapsed: IdList,
    /// Folders that are open in the rail. Folder IDs are small numbers, kept as text.
    pub open_folders: IdList,
    /// Channels, categories and servers whose content warning was accepted.
    pub agreed: IdList,
}

impl Remembered {
    fn checked(self) -> Self {
        Remembered {
            collapsed: self.collapsed.checked(),
            open_folders: self.open_folders.checked(),
            agreed: self.agreed.checked(),
        }
    }

    /// Reads the section from `path`; defaults for a missing or broken file.
    pub fn load(path: &Path) -> Self {
        prefs::load_section::<Remembered>(path, SETTINGS_KEY).checked()
    }

    /// Writes the section into `path`, keeping the file's other keys.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        prefs::save_section(path, SETTINGS_KEY, self)
    }

    /// The saved state; empty in tests and demo runs, so they never read or write the
    /// person's file.
    pub fn current() -> Self {
        let demo = cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some();
        if cfg!(test) || demo {
            return Self::default();
        }
        prefs::settings_path()
            .map(|p| Self::load(&p))
            .unwrap_or_default()
    }

    /// Saves the state off the UI thread (a failed write only loses the choice).
    pub fn save_in_background(&self, cx: &gpui::App) {
        let demo = cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some();
        if cfg!(test) || demo {
            return;
        }
        let this = self.clone();
        cx.background_executor()
            .spawn(async move {
                if let Some(path) = prefs::settings_path() {
                    if let Err(e) = this.save(&path) {
                        log::warn!("could not save the navigation settings: {e}");
                    }
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("inlet-remembered-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn collapsed_categories_persist() {
        let path = temp_file("persist");
        std::fs::write(&path, r#"{"desktop": {"zoom": 1.25}}"#).unwrap();
        let mut r = Remembered::load(&path);
        assert_eq!(r, Remembered::default());
        assert!(r.collapsed.insert("301"));
        assert!(!r.collapsed.insert("301"), "once");
        assert!(r.collapsed.insert("302"));
        assert!(r.open_folders.insert("3"));
        assert!(r.agreed.insert("900"));
        r.save(&path).unwrap();

        // A new run finds the same folded categories, open folders and agreements.
        let again = Remembered::load(&path);
        assert_eq!(again, r);
        assert!(again.collapsed.contains("301") && again.collapsed.contains("302"));
        // Opening a category again forgets it.
        let mut r = again;
        assert!(r.collapsed.remove("301"));
        r.save(&path).unwrap();
        assert!(!Remembered::load(&path).collapsed.contains("301"));
        // The other sections of the file stay.
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["desktop"]["zoom"], 1.25);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_hand_edited_file_cannot_grow_the_lists() {
        let path = temp_file("bounded");
        let many: Vec<String> = (1..=MAX_REMEMBERED + 500).map(|i| i.to_string()).collect();
        let doc = serde_json::json!({"navigation": {"collapsed": many, "agreed": ["x", "5", "5", "../1"]}});
        std::fs::write(&path, doc.to_string()).unwrap();
        let r = Remembered::load(&path);
        assert_eq!(r.collapsed.0.len(), MAX_REMEMBERED);
        // The newest are kept.
        assert!(r.collapsed.contains(&(MAX_REMEMBERED + 500).to_string()));
        assert!(!r.collapsed.contains("1"));
        assert_eq!(r.agreed.0.len(), 1);
        // A full list drops its oldest entry for a new one.
        let mut list = r.collapsed;
        assert!(list.insert("99999999"));
        assert_eq!(list.0.len(), MAX_REMEMBERED);
        assert!(!list.contains("501"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
