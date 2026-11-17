//! Account settings: the newest full snapshot of the account's user settings (READY and
//! USER_SETTINGS_UPDATE), so the app can show an account's theme, message display and
//! privacy settings when it becomes the active one, without waiting for the next event.
//!
//! Fluxer's user settings carry no version. The gateway's order is the server's order,
//! and Fluxer's own client replaces its settings with every update, so this does the
//! same: the snapshots are taken in the order they arrive.

use inlet_protocol::models::GuildFolder;
use inlet_protocol::{Dispatch, Ready, UserSettings};
use serde_json::Map;

use crate::Changes;

/// Folders kept (Fluxer stores at most 100).
pub const MAX_FOLDERS: usize = 200;
/// Guild IDs kept in one folder entry.
pub const MAX_GUILDS_PER_FOLDER: usize = crate::limits::MAX_GUILDS;
/// Longest folder name kept, in characters.
const MAX_FOLDER_NAME_CHARS: usize = 100;

/// A folder with its size limits applied: IDs that are not snowflakes go, names and
/// icon names are cut.
fn capped_folder(mut f: GuildFolder) -> GuildFolder {
    f.guild_ids.retain(|id| crate::GuildId::parse(id).is_some());
    f.guild_ids.truncate(MAX_GUILDS_PER_FOLDER);
    if let Some(name) = &mut f.name {
        if name.chars().count() > MAX_FOLDER_NAME_CHARS {
            *name = name.chars().take(MAX_FOLDER_NAME_CHARS).collect();
        }
    }
    if f.icon.len() > 32 {
        f.icon = String::new();
    }
    f
}

fn capped_folders(list: &[GuildFolder]) -> Vec<GuildFolder> {
    list.iter()
        .take(MAX_FOLDERS)
        .cloned()
        .map(capped_folder)
        .collect()
}

/// The keys of `UserSettings::extra` the app reads. Everything else is dropped:
/// `synced_preferences` alone can be hundreds of KiB, and the preference pickers keep
/// their own copy of it ([`Display`] holds the two values of it the settings show).
const KEPT_EXTRA: [&str; 13] = [
    "friend_source_flags",
    "default_guilds_restricted",
    "bot_default_guilds_restricted",
    "restricted_guilds",
    "bot_restricted_guilds",
    "incoming_call_flags",
    "group_dm_add_permission_flags",
    "profile_privacy",
    "sensitive_content_friend_dm_filter",
    "sensitive_content_non_friend_dm_filter",
    "sensitive_content_guild_filter",
    "trusted_domains",
    "default_hide_muted_channels",
];

/// The display preferences kept in the account's synced preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Display {
    /// Message font size in pixels.
    pub font_size: Option<f64>,
    /// Space above the first message of a group in pixels (comfy display).
    pub message_group_spacing: Option<f64>,
    /// The same in the compact display.
    pub compact_message_group_spacing: Option<f64>,
    /// Avatars are shown in the compact display.
    pub show_avatars_in_compact_mode: Option<bool>,
    /// Text smileys become emoji when a message is sent (`chat_input`).
    pub convert_emoticons: Option<bool>,
}

/// The sound settings of the synced preferences (`SyncedPreferences.sound`).
pub type SoundSettings = inlet_protocol::proto::preferences::SoundSettings;

/// What the synced preferences of `settings` say that the app follows: the display
/// preferences and the sound settings. `None` when there is no readable snapshot.
fn read_preferences(settings: &UserSettings) -> Option<(Display, Option<SoundSettings>)> {
    let prefs = settings.synced_preferences()?.ok()?;
    Some((
        Display::of_preferences(prefs.get()),
        prefs.get().sound.clone(),
    ))
}

impl Display {
    /// What the synced preferences of `settings` say, if it carries a readable snapshot.
    pub fn of(settings: &UserSettings) -> Option<Display> {
        let prefs = settings.synced_preferences()?.ok()?;
        Some(Display::of_preferences(prefs.get()))
    }

    /// What the preferences say.
    pub fn of_preferences(prefs: &inlet_protocol::api::preferences::Preferences) -> Display {
        let a = prefs.accessibility.as_ref();
        Display {
            font_size: a.and_then(|a| a.font_size),
            message_group_spacing: a.and_then(|a| a.message_group_spacing),
            compact_message_group_spacing: a.and_then(|a| a.compact_message_group_spacing),
            show_avatars_in_compact_mode: a.and_then(|a| a.show_user_avatars_in_compact_mode),
            convert_emoticons: prefs.chat_input.as_ref().and_then(|c| c.convert_emoticons),
        }
    }
}

/// The settings of an account, as the server last said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    user: Option<Box<UserSettings>>,
    display: Display,
    /// The sound settings of the synced preferences, as the server holds them.
    sound: Option<SoundSettings>,
    /// The guild order and folders, from the same whole snapshots as the user settings
    /// (`trimmed` leaves them out of `user`: the rail reads them here, bounded).
    guild_folders: Vec<GuildFolder>,
}

/// Whether `settings` is a whole snapshot. Fluxer always sends the whole object, with a
/// theme; a partial one (a test's, or an old server's) would read as every switch off.
fn is_full(settings: &UserSettings) -> bool {
    !settings.theme.is_empty()
}

/// The snapshot without the parts that are big or that nobody here reads.
fn trimmed(settings: &UserSettings) -> Box<UserSettings> {
    let mut s = settings.clone();
    let mut extra = Map::new();
    for key in KEPT_EXTRA {
        if let Some(v) = s.extra.remove(key) {
            extra.insert(key.to_owned(), v);
        }
    }
    s.extra = extra;
    s.custom_status = None;
    s.guild_folders.clear();
    Box::new(s)
}

impl Settings {
    /// The settings READY carries, if it carries any.
    pub fn from_ready(r: &Ready) -> Self {
        let user = r.user_settings().filter(is_full);
        let (display, sound) = user.as_ref().and_then(read_preferences).unwrap_or_default();
        Settings {
            display,
            sound,
            guild_folders: user
                .as_ref()
                .map(|u| capped_folders(&u.guild_folders))
                .unwrap_or_default(),
            user: user.as_ref().map(trimmed),
        }
    }

    /// Applies an event: a USER_SETTINGS_UPDATE that is a whole snapshot replaces the
    /// one held. A partial update changes nothing here.
    pub fn apply(&mut self, event: &Dispatch) -> Changes {
        let mut changes = Changes::default();
        if let Dispatch::UserSettingsUpdate(update) = event {
            if is_full(update) {
                if let Some((display, sound)) = read_preferences(update) {
                    self.display = display;
                    self.sound = sound;
                }
                self.user = Some(trimmed(update));
                // The folders only from a whole snapshot: a partial update has none and
                // would read as an account without folders.
                let folders = capped_folders(&update.guild_folders);
                if folders != self.guild_folders {
                    self.guild_folders = folders;
                    changes.folders = true;
                }
            }
        }
        changes
    }

    /// The newest full snapshot, trimmed: `theme`, the media and message switches,
    /// `developer_mode` and the privacy fields (see `KEPT_EXTRA`). `None` before the
    /// first READY with settings.
    pub fn user(&self) -> Option<&UserSettings> {
        self.user.as_deref()
    }

    /// The font size and message spacing of the synced preferences.
    pub fn display(&self) -> Display {
        self.display
    }

    /// The sound settings of the synced preferences, if the account has any.
    pub fn sound(&self) -> Option<&SoundSettings> {
        self.sound.as_ref()
    }

    /// The guild order and folders (see the field).
    pub fn guild_folders(&self) -> &[GuildFolder] {
        &self.guild_folders
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(value: serde_json::Value) -> Box<UserSettings> {
        Box::new(serde_json::from_value(value).unwrap())
    }

    /// A snapshot shaped like Fluxer's: no version, the whole object.
    fn fluxer(theme: &str, flags: u64) -> Box<UserSettings> {
        settings(json!({
            "status": "online", "theme": theme, "locale": "en-US",
            "message_display_compact": false, "gif_auto_play": true,
            "animate_emoji": true, "animate_stickers": 0, "render_embeds": true,
            "render_reactions": true, "render_spoilers": 1, "developer_mode": false,
            "friend_source_flags": flags, "default_guilds_restricted": false,
            "incoming_call_flags": 8, "group_dm_add_permission_flags": 8,
            "guild_folders": [], "afk_timeout": 600, "time_format": 0
        }))
    }

    #[test]
    fn folders_follow_the_settings_and_are_bounded() {
        let with = |folders: serde_json::Value| {
            let mut s = fluxer("dark", 0);
            s.guild_folders = serde_json::from_value(folders).unwrap();
            Dispatch::UserSettingsUpdate(s)
        };
        let mut s = Settings::default();
        let list = json!([
            {"id": 3, "name": "Games", "color": 255, "flags": 1, "icon": "star", "guild_ids": ["11", "12", "x"]},
            {"id": -1, "name": null, "icon": "folder", "guild_ids": ["10"]}
        ]);
        let c = s.apply(&with(list.clone()));
        assert!(c.folders);
        assert_eq!(s.guild_folders().len(), 2);
        // An ID that is not a snowflake is dropped.
        assert_eq!(s.guild_folders()[0].guild_ids, ["11", "12"]);
        // The same settings again change nothing.
        assert!(!s.apply(&with(list)).folders);
        // A partial update (no theme) leaves the folders alone.
        let partial: UserSettings = serde_json::from_value(json!({"status": "online"})).unwrap();
        assert!(
            !s.apply(&Dispatch::UserSettingsUpdate(Box::new(partial)))
                .folders
        );
        assert_eq!(s.guild_folders().len(), 2);
        // Very long names and too many entries are cut.
        let many: Vec<_> = (0..300)
            .map(|i| json!({"id": i, "name": "n".repeat(500), "guild_ids": []}))
            .collect();
        s.apply(&with(json!(many)));
        assert_eq!(s.guild_folders().len(), MAX_FOLDERS);
        assert_eq!(s.guild_folders()[0].name.as_deref().unwrap().len(), 100);
    }

    #[test]
    fn snapshots_are_taken_in_the_order_they_arrive() {
        let mut s = Settings::default();
        s.apply(&Dispatch::UserSettingsUpdate(fluxer("light", 3)));
        assert_eq!(s.user().unwrap().theme, "light");
        s.apply(&Dispatch::UserSettingsUpdate(fluxer("coal", 0)));
        assert_eq!(s.user().unwrap().theme, "coal");
        assert_eq!(
            s.user().unwrap().friend_source_flags(),
            Some(0),
            "a flag of 0 is a value, not a missing one"
        );
    }

    #[test]
    fn a_partial_update_changes_nothing_and_big_fields_are_dropped() {
        let mut s = Settings::default();
        s.apply(&Dispatch::UserSettingsUpdate(settings(json!({
            "theme": "dark", "friend_source_flags": 3,
            "synced_preferences": "AAAA", "something_else": 1
        }))));
        let kept = s.user().unwrap();
        assert_eq!(kept.friend_source_flags(), Some(3));
        assert!(!kept.extra.contains_key("synced_preferences"));
        assert!(!kept.extra.contains_key("something_else"));
        s.apply(&Dispatch::UserSettingsUpdate(settings(
            json!({"status": "dnd"}),
        )));
        assert_eq!(s.user().unwrap().theme, "dark");
    }
}
