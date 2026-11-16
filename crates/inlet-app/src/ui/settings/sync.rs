// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings the person can change, and how an edit and the server's word meet.
//!
//! An edit is applied at once and shown; it is also sent to the server. Until the
//! server has answered for it, the edit stays on top of what the server last said
//! ([`Tracker::values`]). The server's settings carry a `version` that goes up with
//! every change, and that settles two clients that change the same setting:
//!
//! - a snapshot that is not newer than the newest one seen (an echo that came late) is
//!   ignored, so an old echo can never undo a later click;
//! - a snapshot that is newer replaces what the server is known to say. An edit still
//!   waiting for its answer stays on top until the answer comes: the answer carries the
//!   version of this client's own write, and if another client's snapshot has a higher
//!   one, that client wrote last and its value is what is shown. If this client's write
//!   has the higher version, it wrote last and its value stays.
//!
//! The synced preferences (font size, message spacing) have no version of their own;
//! they travel inside the same user settings object, and their answers are taken as
//! they come.

use inlet_protocol as p;
use inlet_state::settings::Display;

use crate::backend::settings::write::Change;
use crate::theme::Pref;

/// Message font sizes the settings offer, in pixels.
pub const FONT_SIZES: std::ops::RangeInclusive<u8> = 12..=24;
/// Group spacings the settings offer, in pixels, in steps of [`SPACING_STEP`].
pub const GROUP_SPACINGS: std::ops::RangeInclusive<u8> = 0..=32;
pub const SPACING_STEP: u8 = 4;

/// Who may send a friend request: the flags of Fluxer's `friend_source_flags`.
pub mod friend_sources {
    pub const MUTUAL_FRIENDS: u8 = 1;
    pub const MUTUAL_SERVERS: u8 = 2;
    pub const ANYONE: u8 = 4;
}

/// Every setting of the tabs, with Fluxer's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    pub theme: Pref,
    pub compact: bool,
    pub gif_auto_play: bool,
    pub animate_emoji: bool,
    /// 0 always, 1 on interaction, 2 never.
    pub animate_stickers: u8,
    pub render_embeds: bool,
    pub render_reactions: bool,
    /// 0 always shown, 1 on click, 2 if moderator.
    pub render_spoilers: u8,
    pub developer_mode: bool,
    pub dms_from_members: bool,
    pub friend_sources: u8,
    pub font_size: u8,
    pub group_spacing: u8,
}

impl Default for Values {
    fn default() -> Self {
        Values {
            theme: Pref::Dark,
            compact: false,
            gif_auto_play: true,
            animate_emoji: true,
            animate_stickers: 0,
            render_embeds: true,
            render_reactions: true,
            render_spoilers: 1,
            developer_mode: false,
            dms_from_members: true,
            friend_sources: friend_sources::MUTUAL_FRIENDS | friend_sources::MUTUAL_SERVERS,
            font_size: 16,
            group_spacing: 16,
        }
    }
}

impl Values {
    /// Puts one change on top.
    pub fn apply(&mut self, c: &Change) {
        match *c {
            Change::Theme(v) => self.theme = v,
            Change::Compact(v) => self.compact = v,
            Change::GifAutoPlay(v) => self.gif_auto_play = v,
            Change::AnimateEmoji(v) => self.animate_emoji = v,
            Change::AnimateStickers(v) => self.animate_stickers = v.min(2),
            Change::RenderEmbeds(v) => self.render_embeds = v,
            Change::RenderReactions(v) => self.render_reactions = v,
            Change::RenderSpoilers(v) => self.render_spoilers = v.min(2),
            Change::DeveloperMode(v) => self.developer_mode = v,
            Change::DmsFromMembers(v) => self.dms_from_members = v,
            Change::FriendSources(v) => self.friend_sources = v & 7,
            Change::FontSize(v) => self.font_size = v.clamp(8, 48),
            Change::GroupSpacing(v) => self.group_spacing = v.min(64),
        }
    }

    /// The settings as the server's user settings say, with `base` for what they do
    /// not say (an older server, or a field this crate does not know yet).
    pub fn from_server(s: &p::UserSettings, display: Option<Display>, base: &Values) -> Values {
        let mut v = base.clone();
        if let Some(theme) = Pref::from_server(&s.theme) {
            v.theme = theme;
        }
        v.compact = s.message_display_compact;
        v.gif_auto_play = s.gif_auto_play;
        v.animate_emoji = s.animate_emoji;
        v.animate_stickers = s.animate_stickers.min(2) as u8;
        v.render_embeds = s.render_embeds;
        v.render_reactions = s.render_reactions;
        v.render_spoilers = s.render_spoilers.min(2) as u8;
        v.developer_mode = s.developer_mode;
        if let Some(restricted) = s.default_guilds_restricted() {
            v.dms_from_members = !restricted;
        }
        if let Some(flags) = s.friend_source_flags() {
            v.friend_sources = (flags & 7) as u8;
        }
        if let Some(d) = display {
            if let Some(px) = d.font_size.filter(|px| px.is_finite()) {
                v.font_size = px.round().clamp(8., 48.) as u8;
            }
            if let Some(px) = d.message_group_spacing.filter(|px| px.is_finite()) {
                v.group_spacing = px.round().clamp(0., 64.) as u8;
            }
        }
        v
    }
}

/// An edit that waits for the server's answer.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    change: Change,
    /// The number of the command that carried it.
    seq: u64,
}

/// What an edit and the server's snapshots make of the settings.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// What the server last said (or, before it has said anything, the copy kept from
    /// last time).
    confirmed: Values,
    /// The newest version seen.
    version: Option<i64>,
    /// The newest edit of each kind that the server has not answered for.
    pending: Vec<Pending>,
    seq: u64,
}

impl Tracker {
    pub fn new(values: Values) -> Self {
        Tracker {
            confirmed: values,
            ..Tracker::default()
        }
    }

    /// The settings to show: what the server said, with the edits it has not answered
    /// for on top.
    pub fn values(&self) -> Values {
        let mut v = self.confirmed.clone();
        for p in &self.pending {
            v.apply(&p.change);
        }
        v
    }

    /// What the server last said, without the edits waiting.
    pub fn confirmed(&self) -> &Values {
        &self.confirmed
    }

    /// An edit: shown at once, and kept on top until the server answers. Returns the
    /// number of the command that carries it to the server.
    pub fn edit(&mut self, change: Change) -> u64 {
        self.seq += 1;
        let kind = change.kind();
        self.pending.retain(|p| p.change.kind() != kind);
        self.pending.push(Pending {
            change,
            seq: self.seq,
        });
        self.seq
    }

    /// Whether an edit of this kind waits for the server.
    #[cfg(test)]
    pub fn is_waiting(&self, kind: crate::backend::settings::write::Kind) -> bool {
        self.pending.iter().any(|p| p.change.kind() == kind)
    }

    /// A snapshot of the account settings: from the gateway (READY, an update) or the
    /// answer to a write when `answer_through` names the command it answers for.
    /// `version` is the snapshot's version, `None` for a server that sends none (it is
    /// then taken as the newest). Returns whether what is shown may have changed.
    pub fn server(&mut self, version: Option<i64>, values: Values, answer: Option<u64>) -> bool {
        let before = self.values();
        let newer = match (version, self.version) {
            (Some(v), Some(known)) => v > known,
            _ => true,
        };
        if newer {
            self.version = version.or(self.version);
            self.confirmed = values;
        }
        if let Some(through) = answer {
            // The server has answered for these edits, whether or not its snapshot was
            // the newest: if another client wrote later, its value is already shown.
            // Preference edits are answered for by their own write.
            self.pending
                .retain(|p| p.change.is_preference() || p.seq > through);
        }
        self.values() != before
    }

    /// The synced preferences as the server stored them after a write of them up to
    /// command `through`.
    pub fn preferences(&mut self, display: Display, through: u64) -> bool {
        let before = self.values();
        if let Some(px) = display.font_size.filter(|px| px.is_finite()) {
            self.confirmed.font_size = px.round().clamp(8., 48.) as u8;
        }
        if let Some(px) = display.message_group_spacing.filter(|px| px.is_finite()) {
            self.confirmed.group_spacing = px.round().clamp(0., 64.) as u8;
        }
        self.pending
            .retain(|p| !(p.change.is_preference() && p.seq <= through));
        self.values() != before
    }

    /// A write up to command `through` failed: the edits it carried are dropped, and
    /// the settings show what the server holds. Returns whether that changed anything.
    pub fn failed(&mut self, through: u64, preferences: bool) -> bool {
        let before = self.values();
        self.pending
            .retain(|p| !(p.change.is_preference() == preferences && p.seq <= through));
        self.values() != before
    }

    /// Another account is shown: its edits and its versions are not this one's. The
    /// values stay until its own snapshot comes.
    pub fn reset_account(&mut self) {
        self.confirmed = self.values();
        self.pending.clear();
        self.version = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn server_values(theme: &str, version: i64) -> (Option<i64>, Values) {
        let s: p::UserSettings = serde_json::from_value(json!({
            "theme": theme, "version": version, "message_display_compact": false,
            "gif_auto_play": true, "animate_emoji": true, "render_embeds": true,
            "render_reactions": true, "render_spoilers": 1,
        }))
        .unwrap();
        (
            s.version(),
            Values::from_server(&s, None, &Values::default()),
        )
    }

    fn shown(t: &Tracker) -> Pref {
        t.values().theme
    }

    /// Two clients change the theme: whichever the server applied last is shown, in
    /// whatever order the two answers reach this client.
    #[test]
    fn server_setting_conflict_takes_the_newest() {
        // The other client wrote first (version 9), this client's write won (10).
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("dark", 8);
        t.server(v, vals, None);
        let seq = t.edit(Change::Theme(Pref::Light));
        assert_eq!(shown(&t), Pref::Light, "the click shows at once");
        let (v, vals) = server_values("coal", 9);
        t.server(v, vals, None);
        assert_eq!(
            shown(&t),
            Pref::Light,
            "the click stays on top while it waits"
        );
        let (v, vals) = server_values("light", 10);
        t.server(v, vals, Some(seq));
        assert_eq!(shown(&t), Pref::Light, "this client wrote last");
        assert!(!t.is_waiting(Change::Theme(Pref::Dark).kind()));

        // The other client wrote last (version 11 against this client's 10).
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("dark", 8);
        t.server(v, vals, None);
        let seq = t.edit(Change::Theme(Pref::Light));
        let (v, vals) = server_values("coal", 11);
        t.server(v, vals, None);
        let (v, vals) = server_values("light", 10);
        t.server(v, vals, Some(seq));
        assert_eq!(shown(&t), Pref::Coal, "the newer version is the newest");
    }

    #[test]
    fn an_older_snapshot_is_ignored_and_a_newer_one_replaces() {
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("light", 5);
        assert!(t.server(v, vals, None));
        assert_eq!(shown(&t), Pref::Light);
        let (v, vals) = server_values("coal", 4);
        assert!(!t.server(v, vals, None), "an old echo changes nothing");
        assert_eq!(shown(&t), Pref::Light);
        let (v, vals) = server_values("coal", 6);
        assert!(t.server(v, vals, None));
        assert_eq!(
            shown(&t),
            Pref::Coal,
            "another client's change shows at once"
        );
    }

    #[test]
    fn the_echo_of_an_earlier_write_does_not_undo_a_later_click() {
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("dark", 1);
        t.server(v, vals, None);
        t.edit(Change::Theme(Pref::Light));
        let seq2 = t.edit(Change::Theme(Pref::Coal));
        // The gateway echoes the first write.
        let (v, vals) = server_values("light", 2);
        t.server(v, vals, None);
        assert_eq!(shown(&t), Pref::Coal);
        let (v, vals) = server_values("coal", 3);
        t.server(v, vals, Some(seq2));
        assert_eq!(shown(&t), Pref::Coal);
        assert!(!t.is_waiting(Change::Theme(Pref::Dark).kind()));
    }

    #[test]
    fn a_failed_write_goes_back_to_what_the_server_holds() {
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("dark", 1);
        t.server(v, vals, None);
        let seq = t.edit(Change::Theme(Pref::Light));
        let font = t.edit(Change::FontSize(20));
        assert!(t.failed(seq, false));
        assert_eq!(shown(&t), Pref::Dark);
        assert_eq!(
            t.values().font_size,
            20,
            "a preference write is another write"
        );
        assert!(t.failed(font, true));
        assert_eq!(t.values().font_size, 16);
    }

    #[test]
    fn an_edit_after_the_answer_started_stays_on_top() {
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("dark", 1);
        t.server(v, vals, None);
        let first = t.edit(Change::Theme(Pref::Light));
        let second = t.edit(Change::Theme(Pref::Coal));
        assert!(second > first);
        // The answer is for the first command only; the second still waits.
        let (v, vals) = server_values("light", 2);
        t.server(v, vals, Some(first));
        assert_eq!(shown(&t), Pref::Coal);
        assert!(t.is_waiting(Change::Theme(Pref::Dark).kind()));
    }

    #[test]
    fn preferences_come_from_the_synced_preferences() {
        let mut t = Tracker::new(Values::default());
        let seq = t.edit(Change::GroupSpacing(8));
        assert_eq!(t.values().group_spacing, 8);
        t.preferences(
            Display {
                font_size: Some(18.),
                message_group_spacing: Some(8.),
            },
            seq,
        );
        assert_eq!(t.values().font_size, 18);
        assert_eq!(t.values().group_spacing, 8);
        assert!(!t.is_waiting(Change::GroupSpacing(0).kind()));
    }

    #[test]
    fn values_of_the_server_fill_in_what_it_does_not_say() {
        let s: p::UserSettings = serde_json::from_value(json!({
            "theme": "dark_legacy", "version": 3, "render_spoilers": 9,
            "friend_source_flags": 15, "default_guilds_restricted": true,
            "animate_stickers": 2, "developer_mode": true
        }))
        .unwrap();
        let v = Values::from_server(
            &s,
            Some(Display {
                font_size: Some(500.),
                message_group_spacing: Some(f64::NAN),
            }),
            &Values::default(),
        );
        assert_eq!(v.theme, Pref::Dark, "the legacy palette draws as dark");
        assert_eq!(v.render_spoilers, 2, "out of range values are cut");
        assert_eq!(v.friend_sources, 7);
        assert!(!v.dms_from_members);
        assert_eq!(v.animate_stickers, 2);
        assert!(v.developer_mode);
        assert_eq!(v.font_size, 48);
        assert_eq!(v.group_spacing, 16, "not a number: kept");
        // A theme the server names that this app does not know keeps the one shown.
        let s: p::UserSettings = serde_json::from_value(json!({"theme": "neon"})).unwrap();
        let base = Values {
            theme: Pref::Coal,
            ..Values::default()
        };
        assert_eq!(Values::from_server(&s, None, &base).theme, Pref::Coal);
    }

    #[test]
    fn another_account_starts_without_the_old_ones_versions() {
        let mut t = Tracker::new(Values::default());
        let (v, vals) = server_values("light", 50);
        t.server(v, vals, None);
        t.edit(Change::Theme(Pref::Coal));
        t.reset_account();
        // The other account's version is lower, and still taken.
        let (v, vals) = server_values("dark", 3);
        assert!(t.server(v, vals, None));
        assert_eq!(shown(&t), Pref::Dark);
    }
}
