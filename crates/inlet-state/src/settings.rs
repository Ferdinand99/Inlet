//! Account settings the sidebar needs: the guild order and folders
//! (`guild_folders` in READY `user_settings` and USER_SETTINGS_UPDATE).
//!
//! Fluxer's client replaces the whole list on every settings update, so does this.
//! Other settings (synced preferences, privacy, theme) are read where they are used.

use inlet_protocol::models::GuildFolder;
use inlet_protocol::{Dispatch, Ready};

use crate::Changes;

/// Folders kept (Fluxer stores at most 100).
pub const MAX_FOLDERS: usize = 200;
/// Guild IDs kept in one folder entry.
pub const MAX_GUILDS_PER_FOLDER: usize = crate::limits::MAX_GUILDS;
/// Longest folder name kept, in characters.
const MAX_NAME_CHARS: usize = 100;

/// The account's settings that the state keeps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    guild_folders: Vec<GuildFolder>,
}

/// A folder with its size limits applied.
fn capped(mut f: GuildFolder) -> GuildFolder {
    f.guild_ids.retain(|id| crate::GuildId::parse(id).is_some());
    f.guild_ids.truncate(MAX_GUILDS_PER_FOLDER);
    if let Some(name) = &mut f.name {
        if name.chars().count() > MAX_NAME_CHARS {
            *name = name.chars().take(MAX_NAME_CHARS).collect();
        }
    }
    if f.icon.len() > 32 {
        f.icon = String::new();
    }
    f
}

fn capped_list(list: &[GuildFolder]) -> Vec<GuildFolder> {
    list.iter().take(MAX_FOLDERS).cloned().map(capped).collect()
}

impl Settings {
    /// The settings in a READY.
    pub fn from_ready(r: &Ready) -> Self {
        Settings {
            guild_folders: r
                .user_settings()
                .map(|s| capped_list(&s.guild_folders))
                .unwrap_or_default(),
        }
    }

    /// The guild order and folders, as the server stored them.
    pub fn guild_folders(&self) -> &[GuildFolder] {
        &self.guild_folders
    }

    /// Applies an event.
    pub fn apply(&mut self, event: &Dispatch) -> Changes {
        let mut c = Changes::default();
        if let Dispatch::UserSettingsUpdate(settings) = event {
            let folders = capped_list(&settings.guild_folders);
            if folders != self.guild_folders {
                self.guild_folders = folders;
                c.folders = true;
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::UserSettings;
    use serde_json::json;

    fn update(v: serde_json::Value) -> Dispatch {
        let s: UserSettings = serde_json::from_value(v).unwrap();
        Dispatch::UserSettingsUpdate(Box::new(s))
    }

    #[test]
    fn folders_follow_the_settings_and_are_bounded() {
        let mut s = Settings::default();
        let folders = json!({"guild_folders": [
            {"id": 3, "name": "Games", "color": 255, "flags": 1, "icon": "star", "guild_ids": ["11", "12", "x"]},
            {"id": -1, "name": null, "icon": "folder", "guild_ids": ["10"]}
        ]});
        let c = s.apply(&update(folders.clone()));
        assert!(c.folders);
        let list = s.guild_folders();
        assert_eq!(list.len(), 2);
        // An ID that is not a snowflake is dropped.
        assert_eq!(list[0].guild_ids, ["11", "12"]);
        // The same settings again change nothing.
        assert!(!s.apply(&update(folders)).folders);
        // A list with very long names and too many entries is cut.
        let many: Vec<_> = (0..300)
            .map(|i| json!({"id": i, "name": "n".repeat(500), "guild_ids": []}))
            .collect();
        s.apply(&update(json!({ "guild_folders": many })));
        assert_eq!(s.guild_folders().len(), MAX_FOLDERS);
        assert_eq!(s.guild_folders()[0].name.as_deref().unwrap().len(), 100);
    }
}
