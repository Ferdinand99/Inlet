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
    /// The on/off switches the preferences name (see [`crate::flags`]).
    pub flags: crate::flags::Read,
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
            flags: crate::flags::Read::of(prefs),
        }
    }
}

/// The settings of an account, as the server last said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    user: Option<Box<UserSettings>>,
    display: Display,
    /// The guild order and folders. Kept apart from the user settings above: the server
    /// list fills this in (it is empty here).
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
        Settings {
            display: user.as_ref().and_then(Display::of).unwrap_or_default(),
            user: user.as_ref().map(trimmed),
            guild_folders: Vec::new(),
        }
    }

    /// Applies an event: a USER_SETTINGS_UPDATE that is a whole snapshot replaces the
    /// one held. A partial update changes nothing here.
    pub fn apply(&mut self, event: &Dispatch) -> Changes {
        if let Dispatch::UserSettingsUpdate(update) = event {
            if is_full(update) {
                if let Some(display) = Display::of(update) {
                    self.display = display;
                }
                self.user = Some(trimmed(update));
            }
        }
        Changes::default()
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
