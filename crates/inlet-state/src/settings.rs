//! Account settings: the newest full snapshot of the account's user settings (READY and
//! USER_SETTINGS_UPDATE), so the app can show an account's theme, message display and
//! privacy settings when it becomes the active one, without waiting for the next event.

use inlet_protocol::{Dispatch, Ready, UserSettings};
use serde_json::Map;

use crate::Changes;

/// The keys of `UserSettings::extra` the app reads. Everything else is dropped:
/// `synced_preferences` alone can be hundreds of KiB, and the preference pickers keep
/// their own copy of it ([`Display`] holds the two values of it the settings show).
const KEPT_EXTRA: [&str; 3] = [
    "version",
    "friend_source_flags",
    "default_guilds_restricted",
];

/// The display preferences kept in the account's synced preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Display {
    /// Message font size in pixels.
    pub font_size: Option<f64>,
    /// Space above the first message of a group in pixels (cozy display).
    pub message_group_spacing: Option<f64>,
}

impl Display {
    /// What the synced preferences of `settings` say, if it carries a readable snapshot.
    pub fn of(settings: &UserSettings) -> Option<Display> {
        let prefs = settings.synced_preferences()?.ok()?;
        let a = prefs.get().accessibility.as_ref();
        Some(Display {
            font_size: a.and_then(|a| a.font_size),
            message_group_spacing: a.and_then(|a| a.message_group_spacing),
        })
    }
}

/// The settings of an account, as the server last said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    user: Option<Box<UserSettings>>,
    display: Display,
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
        let user = r.user_settings();
        Settings {
            display: user.as_ref().and_then(Display::of).unwrap_or_default(),
            user: user.as_ref().map(trimmed),
        }
    }

    /// Applies an event. A USER_SETTINGS_UPDATE replaces the snapshot when it is a
    /// full one (it has a `version`) that is not older than the one held. A partial
    /// update, as tests and old servers send, changes nothing here.
    pub fn apply(&mut self, event: &Dispatch) -> Changes {
        if let Dispatch::UserSettingsUpdate(update) = event {
            if let Some(version) = update.version() {
                let held = self.user.as_ref().and_then(|u| u.version());
                if held.is_none_or(|held| version >= held) {
                    if let Some(display) = Display::of(update) {
                        self.display = display;
                    }
                    self.user = Some(trimmed(update));
                }
            }
        }
        Changes::default()
    }

    /// The newest full snapshot, trimmed: `theme`, the media and message switches,
    /// `developer_mode` and the privacy fields (`version`, `friend_source_flags`,
    /// `default_guilds_restricted`). `None` before the first READY with settings.
    pub fn user(&self) -> Option<&UserSettings> {
        self.user.as_deref()
    }

    /// The font size and message spacing of the synced preferences.
    pub fn display(&self) -> Display {
        self.display
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(value: serde_json::Value) -> Box<UserSettings> {
        Box::new(serde_json::from_value(value).unwrap())
    }

    #[test]
    fn a_newer_snapshot_replaces_and_an_older_one_does_not() {
        let mut s = Settings::default();
        s.apply(&Dispatch::UserSettingsUpdate(settings(
            json!({"theme": "light", "version": 4}),
        )));
        assert_eq!(s.user().unwrap().theme, "light");
        s.apply(&Dispatch::UserSettingsUpdate(settings(
            json!({"theme": "coal", "version": 3}),
        )));
        assert_eq!(
            s.user().unwrap().theme,
            "light",
            "an older version is ignored"
        );
        s.apply(&Dispatch::UserSettingsUpdate(settings(
            json!({"theme": "coal", "version": 5}),
        )));
        assert_eq!(s.user().unwrap().theme, "coal");
    }

    #[test]
    fn a_partial_update_changes_nothing_and_big_fields_are_dropped() {
        let mut s = Settings::default();
        s.apply(&Dispatch::UserSettingsUpdate(settings(json!({
            "theme": "dark", "version": 1, "friend_source_flags": 3,
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
