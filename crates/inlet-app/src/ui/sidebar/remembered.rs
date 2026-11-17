// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the rail remembers on this computer between runs, kept under `"navigation"` in
//! `settings.json`, per account: the folders that are open, and the accepted content
//! warnings for forum and media channels.
//!
//! This is where Fluxer keeps them too: the open folders in `GuildFolderExpanded` and
//! the accepted warnings of thread-only channels in `localAgreedChannelIds`, both in
//! storage scoped to the account. Folder IDs are small numbers of one account's
//! settings, so account B must not open account A's folder 1: every entry is keyed by
//! `<origin>#<user id>`. (Folded categories and the accepted warnings of other channels
//! are settings of the account on the server: the guild settings and
//! `SyncedPreferences`.)
//!
//! Nothing here is secret and nothing here is a message. Every list is capped, and so is
//! the number of accounts, so a hand-edited or very old file cannot grow the app's
//! memory; when a list is full the oldest entries go first.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::backend::platform::prefs;

use super::history::Visit;

/// Key of the section in `settings.json`.
const SETTINGS_KEY: &str = "navigation";
/// Entries kept in each list.
pub const MAX_REMEMBERED: usize = 2_000;
/// Accounts kept.
pub const MAX_ACCOUNTS: usize = 32;
/// Longest ID kept (snowflakes are 20 digits at most).
const MAX_ID_LEN: usize = 20;
/// Longest account key kept.
const MAX_KEY_LEN: usize = 300;

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

/// What one account has.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountMemory {
    /// Folders that are open in the rail.
    pub open_folders: IdList,
    /// Forum and media channels whose content warning was accepted.
    pub local_agreed: IdList,
    /// Where the account was last, so the next start opens there (Fluxer's last
    /// location, kept per account).
    pub last_place: Option<Visit>,
}

/// The remembered navigation state of every account on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Remembered {
    accounts: BTreeMap<String, AccountMemory>,
    /// The width of the channel list in pixels, when it was dragged (the same for every
    /// account, as in Fluxer). `None` is the default width.
    sidebar_width: Option<u32>,
}

impl Remembered {
    fn checked(self) -> Self {
        let accounts = self
            .accounts
            .into_iter()
            .filter(|(k, _)| !k.is_empty() && k.len() <= MAX_KEY_LEN)
            .take(MAX_ACCOUNTS)
            .map(|(k, m)| {
                (
                    k,
                    AccountMemory {
                        open_folders: m.open_folders.checked(),
                        local_agreed: m.local_agreed.checked(),
                        last_place: m.last_place.filter(Visit::is_valid),
                    },
                )
            })
            .collect();
        Remembered {
            accounts,
            sidebar_width: self.sidebar_width.map(super::resize::clamp_width),
        }
    }

    /// The channel list's width: the one that was dragged, or the default.
    pub fn sidebar_width(&self) -> u32 {
        self.sidebar_width.unwrap_or(super::resize::DEFAULT_WIDTH)
    }

    /// Sets the channel list's width (clamped); the default width is stored as "none".
    pub fn set_sidebar_width(&mut self, width: u32) {
        let width = super::resize::clamp_width(width);
        self.sidebar_width = (width != super::resize::DEFAULT_WIDTH).then_some(width);
    }

    /// What `account` has (nothing for one not seen before).
    pub fn of(&self, account: &str) -> Option<&AccountMemory> {
        self.accounts.get(account)
    }

    /// What `account` has, made if it is new. `None` for a key that is too long or when
    /// the number of accounts is full.
    pub fn of_mut(&mut self, account: &str) -> Option<&mut AccountMemory> {
        if account.is_empty() || account.len() > MAX_KEY_LEN {
            return None;
        }
        if !self.accounts.contains_key(account) && self.accounts.len() >= MAX_ACCOUNTS {
            return None;
        }
        Some(self.accounts.entry(account.to_owned()).or_default())
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

    /// Saves the state off the UI thread (a failed write only loses the choice). Writes
    /// are numbered: a snapshot that a newer one has already replaced is not written, so
    /// two quick changes never land out of order.
    pub fn save_in_background(&self, cx: &gpui::App) {
        static ASKED: AtomicU64 = AtomicU64::new(0);
        static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
        let demo = cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some();
        if cfg!(test) || demo {
            return;
        }
        let this = self.clone();
        let number = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        cx.background_executor()
            .spawn(async move {
                let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
                if number < *written {
                    return;
                }
                *written = number;
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
    fn open_folders_are_kept_per_account_and_survive_a_restart() {
        let path = temp_file("persist");
        std::fs::write(&path, r#"{"desktop": {"zoom": 1.25}}"#).unwrap();
        let mut r = Remembered::load(&path);
        assert_eq!(r, Remembered::default());
        let a = "https://chat.example#10";
        let b = "https://chat.example#20";
        let mem = r.of_mut(a).unwrap();
        assert!(mem.open_folders.insert("1"));
        assert!(!mem.open_folders.insert("1"), "once");
        assert!(mem.local_agreed.insert("900"));
        r.save(&path).unwrap();

        let again = Remembered::load(&path);
        assert_eq!(again, r);
        // Account B does not see account A's open folder 1.
        assert!(again.of(a).unwrap().open_folders.contains("1"));
        assert!(again.of(b).is_none());
        let mut r = again;
        assert!(!r.of_mut(b).unwrap().open_folders.contains("1"));
        assert!(r.of_mut(a).unwrap().open_folders.remove("1"));
        r.save(&path).unwrap();
        assert!(!Remembered::load(&path)
            .of(a)
            .unwrap()
            .open_folders
            .contains("1"));
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
        let accounts: serde_json::Map<String, serde_json::Value> = (0..MAX_ACCOUNTS + 10)
            .map(|i| (format!("a{i}"), serde_json::json!({"open_folders": ["1"]})))
            .collect();
        let doc = serde_json::json!({"navigation": {"accounts": accounts}});
        std::fs::write(&path, doc.to_string()).unwrap();
        let mut r = Remembered::load(&path);
        assert_eq!(r.accounts.len(), MAX_ACCOUNTS);
        assert!(r.of_mut("one-too-many").is_none());
        // Lists: the newest are kept, bad values and duplicates go.
        let doc = serde_json::json!({"navigation": {"accounts": {"a": {
            "open_folders": many, "local_agreed": ["x", "5", "5", "../1"]}}}});
        std::fs::write(&path, doc.to_string()).unwrap();
        let r = Remembered::load(&path);
        let mem = r.of("a").unwrap();
        assert_eq!(mem.open_folders.0.len(), MAX_REMEMBERED);
        assert!(mem
            .open_folders
            .contains(&(MAX_REMEMBERED + 500).to_string()));
        assert!(!mem.open_folders.contains("1"));
        assert_eq!(mem.local_agreed.0.len(), 1);
        let mut list = mem.open_folders.clone();
        assert!(list.insert("99999999"));
        assert_eq!(list.0.len(), MAX_REMEMBERED);
        assert!(!list.contains("501"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_last_place_and_the_sidebar_width_survive_a_restart() {
        let path = temp_file("place");
        let mut r = Remembered::default();
        assert_eq!(r.sidebar_width(), 320, "the default");
        r.set_sidebar_width(400);
        r.of_mut("https://chat.example#10").unwrap().last_place = Some(Visit {
            guild: Some("5".into()),
            channel: "77".into(),
        });
        r.save(&path).unwrap();
        let again = Remembered::load(&path);
        assert_eq!(again, r);
        assert_eq!(again.sidebar_width(), 400);
        assert_eq!(
            again
                .of("https://chat.example#10")
                .unwrap()
                .last_place
                .as_ref()
                .map(|v| v.channel.as_str()),
            Some("77")
        );
        // The default width is not written; the width stays between 200 and 480.
        r.set_sidebar_width(320);
        assert_eq!(r.sidebar_width(), 320);
        r.set_sidebar_width(10_000);
        assert_eq!(r.sidebar_width(), 480);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_hand_edited_place_and_width_are_checked() {
        let path = temp_file("place-bad");
        let doc = serde_json::json!({"navigation": {"sidebar_width": 9999, "accounts": {
            "a": {"last_place": {"guild": "../1", "channel": "5"}},
            "b": {"last_place": {"guild": null, "channel": "5"}}}}});
        std::fs::write(&path, doc.to_string()).unwrap();
        let r = Remembered::load(&path);
        assert_eq!(r.sidebar_width(), 480);
        assert!(r.of("a").unwrap().last_place.is_none(), "odd server ID");
        assert!(r.of("b").unwrap().last_place.is_some());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
