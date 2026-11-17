// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings the person can change, and how an edit and the server's word meet.
//!
//! An edit is applied at once and shown; it is also sent to the server. Fluxer's user
//! settings carry no version: the server's order is the order of the gateway, and
//! Fluxer's own client replaces its settings with every USER_SETTINGS_UPDATE. So does
//! this, in the order the snapshots arrive ([`Tracker::gateway`]); READY is the first.
//!
//! An edit stays on top of what the server last said until the server has settled it:
//!
//! - the answer to the write has come ([`Tracker::answered`]), and
//! - a snapshot from the gateway has arrived that holds the edit's value (the echo of
//!   the write: Fluxer sends one after every write, in the server's order, so if another
//!   client wrote after this one, its snapshot follows the echo).
//!
//! A snapshot that arrives while the edit waits and does not hold its value (the echo of
//! an earlier write, or a change another client made first) updates what the server is
//! known to say but never hides the edit. Once a snapshot has held the edit's value the
//! edit counts as echoed for good: if the person clicks back to the value the server
//! already has while an earlier write is in flight, the old value may show until that
//! write's own echo comes.
//! The answer itself gives no values: it may be older than a change another client made
//! after it. If the snapshot never comes, [`Tracker::expire`] lets the answer's values
//! stand after a short wait, so a lost echo cannot pin an edit.

use inlet_protocol as p;
use inlet_state::flags::{Flag, FlagSet};
use inlet_state::settings::Display;

use crate::backend::settings::write::Change;
use crate::theme::Pref;

/// Message font sizes Fluxer offers, in pixels. It snaps any other value to the nearest
/// of these and writes it back, so Inlet offers exactly these.
pub const FONT_SIZES: [u8; 7] = [12, 14, 15, 16, 18, 20, 24];
/// Spaces between message groups Fluxer offers, in pixels, with the same rule.
pub const GROUP_SPACINGS: [u8; 5] = [0, 4, 8, 16, 24];

/// The entry of `list` nearest to `value`.
pub fn nearest(list: &[u8], value: u8) -> u8 {
    list.iter()
        .copied()
        .min_by_key(|v| v.abs_diff(value))
        .unwrap_or(value)
}

/// The entry of `list` one step from `value` (which is first snapped to the list),
/// stopping at the ends. `direction` is -1 or 1.
pub fn step(list: &[u8], value: u8, direction: i32) -> u8 {
    let at = list
        .iter()
        .position(|v| *v == nearest(list, value))
        .unwrap_or(0) as i32;
    let to = (at + direction.signum()).clamp(0, list.len() as i32 - 1);
    list[to as usize]
}

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
    pub inline_embed_media: bool,
    pub inline_attachment_media: bool,
    pub bot_dms_from_members: bool,
    pub call_flags: u8,
    pub group_dm_flags: u8,
    pub profile_privacy: u8,
    pub sensitive_friends: u8,
    pub sensitive_others: u8,
    pub sensitive_servers: u8,
    pub font_size: u8,
    pub group_spacing: u8,
    pub compact_group_spacing: u8,
    pub compact_avatars: bool,
    pub convert_emoticons: bool,
    /// The on/off switches of [`inlet_state::flags`], synced and local.
    pub flags: FlagSet,
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
            inline_embed_media: true,
            inline_attachment_media: true,
            bot_dms_from_members: true,
            call_flags: 8,
            group_dm_flags: 8,
            profile_privacy: 0,
            sensitive_friends: 0,
            sensitive_others: 2,
            sensitive_servers: 1,
            font_size: 16,
            group_spacing: 16,
            compact_group_spacing: 0,
            compact_avatars: false,
            convert_emoticons: false,
            flags: FlagSet::default(),
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
            Change::InlineEmbedMedia(v) => self.inline_embed_media = v,
            Change::InlineAttachmentMedia(v) => self.inline_attachment_media = v,
            Change::BotDmsFromMembers(v) => self.bot_dms_from_members = v,
            Change::IncomingCalls(v) => self.call_flags = v & 63,
            Change::GroupDmAdd(v) => self.group_dm_flags = v & 31,
            Change::ProfilePrivacy(v) => self.profile_privacy = v.min(2),
            Change::SensitiveFriends(v) => self.sensitive_friends = v.min(2),
            Change::SensitiveOthers(v) => self.sensitive_others = v.min(2),
            Change::SensitiveServers(v) => self.sensitive_servers = v.min(2),
            Change::FontSize(v) => self.font_size = v.clamp(8, 48),
            Change::GroupSpacing(v) => self.group_spacing = v.min(64),
            Change::CompactGroupSpacing(v) => self.compact_group_spacing = v.min(64),
            Change::CompactAvatars(v) => self.compact_avatars = v,
            Change::ConvertEmoticons(v) => self.convert_emoticons = v,
            Change::Flag(flag, on) => self.flags = self.flags.with(flag, on),
        }
    }

    /// Takes the values of the synced preferences.
    pub fn apply_display(&mut self, d: &Display) {
        let px = |v: Option<f64>, max: f64| {
            v.filter(|px| px.is_finite())
                .map(|px| px.round().clamp(0., max) as u8)
        };
        if let Some(n) = px(d.font_size, 48.) {
            self.font_size = n.max(8);
        }
        if let Some(n) = px(d.message_group_spacing, 64.) {
            self.group_spacing = n;
        }
        if let Some(n) = px(d.compact_message_group_spacing, 64.) {
            self.compact_group_spacing = n;
        }
        if let Some(show) = d.show_avatars_in_compact_mode {
            self.compact_avatars = show;
        }
        if let Some(on) = d.convert_emoticons {
            self.convert_emoticons = on;
        }
        self.flags = self.flags.with_synced(&d.flags);
    }

    /// The state of one on/off switch.
    pub fn flag(&self, flag: Flag) -> bool {
        self.flags.get(flag)
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
        v.inline_embed_media = s.inline_embed_media;
        v.inline_attachment_media = s.inline_attachment_media;
        if let Some(restricted) = s.switch("bot_default_guilds_restricted") {
            v.bot_dms_from_members = !restricted;
        }
        let number = |key: &str, max: u64| s.number(key).map(|n| n.min(max) as u8);
        if let Some(n) = number("incoming_call_flags", 63) {
            v.call_flags = n;
        }
        if let Some(n) = number("group_dm_add_permission_flags", 31) {
            v.group_dm_flags = n;
        }
        if let Some(n) = number("profile_privacy", 2) {
            v.profile_privacy = n;
        }
        if let Some(n) = number("sensitive_content_friend_dm_filter", 2) {
            v.sensitive_friends = n;
        }
        if let Some(n) = number("sensitive_content_non_friend_dm_filter", 2) {
            v.sensitive_others = n;
        }
        if let Some(n) = number("sensitive_content_guild_filter", 2) {
            v.sensitive_servers = n;
        }
        if let Some(d) = display {
            v.apply_display(&d);
        }
        v
    }
}

/// An edit that waits for the server.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    change: Change,
    /// The number of the command that carried it.
    seq: u64,
    /// The server has answered for the write.
    answered: bool,
    /// A snapshot from the gateway has arrived that holds the edit's value.
    echoed: bool,
}

/// What an edit and the server's snapshots make of the settings.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// What the server last said (or, before it has said anything, the copy kept from
    /// last time).
    confirmed: Values,
    /// The newest edit of each kind that the server has not settled.
    pending: Vec<Pending>,
    seq: u64,
    /// What the newest answer to a write said, if no snapshot has come since.
    answer: Option<Values>,
}

impl Tracker {
    pub fn new(values: Values) -> Self {
        Tracker {
            confirmed: values,
            ..Tracker::default()
        }
    }

    /// The settings to show: what the server said, with the edits it has not settled
    /// on top.
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

    /// An edit: shown at once, and kept on top until the server settles it. Returns the
    /// number of the command that carries it to the server.
    pub fn edit(&mut self, change: Change) -> u64 {
        self.seq += 1;
        let kind = change.kind();
        self.pending.retain(|p| p.change.kind() != kind);
        self.pending.push(Pending {
            change,
            seq: self.seq,
            answered: false,
            echoed: false,
        });
        self.seq
    }

    /// A change that is only for this computer: nothing is sent, so nothing waits for the
    /// server. It is what this computer holds from now on.
    pub fn set_local(&mut self, change: Change) {
        self.pending.retain(|p| p.change.kind() != change.kind());
        self.confirmed.apply(&change);
        // The answer kept for [`Tracker::expire`] must not take it back.
        if let Some(answer) = &mut self.answer {
            answer.apply(&change);
        }
    }

    /// Whether an edit of this kind waits for the server.
    #[cfg(test)]
    pub fn is_waiting(&self, kind: crate::backend::settings::write::Kind) -> bool {
        self.pending.iter().any(|p| p.change.kind() == kind)
    }

    /// A snapshot from the gateway (READY, or an update). It is what the server says now.
    /// Returns whether what is shown may have changed.
    pub fn gateway(&mut self, values: Values) -> bool {
        let before = self.values();
        self.confirmed = values;
        self.answer = None;
        for p in &mut self.pending {
            let mut with_edit = self.confirmed.clone();
            with_edit.apply(&p.change);
            if with_edit == self.confirmed {
                p.echoed = true;
            }
        }
        self.pending.retain(|p| !(p.answered && p.echoed));
        self.values() != before
    }

    /// The server has answered for the writes up to command `through`: of the account
    /// settings, or (`preferences`) of the synced preferences. `values` are what the
    /// answer said; they are kept for [`Tracker::expire`] only.
    pub fn answered(&mut self, values: Option<Values>, through: u64, preferences: bool) -> bool {
        let before = self.values();
        for p in &mut self.pending {
            if p.change.is_preference() == preferences && p.seq <= through {
                p.answered = true;
            }
        }
        if values.is_some() {
            self.answer = values;
        }
        self.pending.retain(|p| !(p.answered && p.echoed));
        self.values() != before
    }

    /// The echo of the write up to command `through` has not come: what the answer said
    /// stands, and the edits it settled stop waiting.
    pub fn expire(&mut self, through: u64, preferences: bool) -> bool {
        let before = self.values();
        if let Some(values) = self.answer.take() {
            self.confirmed = values;
        }
        self.pending.retain(|p| {
            !(p.answered && p.change.is_preference() == preferences && p.seq <= through)
        });
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

    /// Another account is shown: its edits are not this one's. The values stay until
    /// its own snapshot comes.
    pub fn reset_account(&mut self) {
        self.confirmed = self.values();
        self.pending.clear();
        self.answer = None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// The settings object as Fluxer sends it: the whole object, no version.
    pub(crate) fn fluxer(theme: &str, friend_flags: u64) -> p::UserSettings {
        serde_json::from_value(json!({
            "status": "online", "theme": theme, "locale": "en-US",
            "message_display_compact": false, "gif_auto_play": true,
            "animate_emoji": true, "animate_stickers": 0, "render_embeds": true,
            "render_reactions": true, "render_spoilers": 1, "developer_mode": false,
            "inline_attachment_media": true, "inline_embed_media": true,
            "friend_source_flags": friend_flags, "default_guilds_restricted": false,
            "incoming_call_flags": 8, "group_dm_add_permission_flags": 8,
            "guild_folders": [], "afk_timeout": 600, "time_format": 0
        }))
        .unwrap()
    }

    fn snapshot(theme: &str) -> Values {
        Values::from_server(&fluxer(theme, 3), None, &Values::default())
    }

    fn shown(t: &Tracker) -> Pref {
        t.values().theme
    }

    fn account_kind() -> crate::backend::settings::write::Kind {
        Change::Theme(Pref::Dark).kind()
    }

    /// Two clients change the theme: whichever the server applied last is shown, whatever
    /// order the answer and the gateway's snapshots arrive in (the gateway's order is the
    /// server's: this client's echo, then the other client's update if it wrote later).
    #[test]
    fn server_setting_conflict_takes_the_newest() {
        // The other client wrote first, this client's write won.
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
        let seq = t.edit(Change::Theme(Pref::Light));
        assert_eq!(shown(&t), Pref::Light, "the click shows at once");
        t.gateway(snapshot("coal"));
        assert_eq!(
            shown(&t),
            Pref::Light,
            "the click stays on top while it waits"
        );
        t.answered(Some(snapshot("light")), seq, false);
        assert_eq!(shown(&t), Pref::Light, "still waiting for the echo");
        t.gateway(snapshot("light"));
        assert_eq!(shown(&t), Pref::Light, "this client wrote last");
        assert!(!t.is_waiting(account_kind()));

        // This client wrote first, the other client's update came after the echo and
        // before the answer.
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
        let seq = t.edit(Change::Theme(Pref::Light));
        t.gateway(snapshot("light"));
        t.gateway(snapshot("coal"));
        assert_eq!(shown(&t), Pref::Light, "not settled before the answer");
        t.answered(Some(snapshot("light")), seq, false);
        assert_eq!(
            shown(&t),
            Pref::Coal,
            "the update after the write is the newest"
        );

        // The same, with the answer first.
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
        let seq = t.edit(Change::Theme(Pref::Light));
        t.answered(Some(snapshot("light")), seq, false);
        assert_eq!(shown(&t), Pref::Light);
        t.gateway(snapshot("light"));
        t.gateway(snapshot("coal"));
        assert_eq!(shown(&t), Pref::Coal);
    }

    #[test]
    fn another_device_changes_the_theme_without_any_version() {
        let mut t = Tracker::new(Values::default());
        assert!(t.gateway(snapshot("light")));
        assert_eq!(shown(&t), Pref::Light);
        assert!(t.gateway(snapshot("coal")));
        assert_eq!(shown(&t), Pref::Coal, "the next update shows at once");
        assert!(
            !t.gateway(snapshot("coal")),
            "the same again changes nothing"
        );
    }

    #[test]
    fn a_ready_without_a_version_sets_what_the_privacy_tab_shows() {
        let ready = fluxer("dark", 0);
        let v = Values::from_server(&ready, None, &Values::default());
        assert_eq!(v.friend_sources, 0, "a flag of 0 is shown as 0");
        let mut t = Tracker::new(Values::default());
        t.gateway(v);
        assert_eq!(t.values().friend_sources, 0);
        let ready: p::UserSettings = serde_json::from_value(json!({
            "theme": "dark", "default_guilds_restricted": true
        }))
        .unwrap();
        let v = Values::from_server(&ready, None, &Values::default());
        assert!(!v.dms_from_members);
        assert_eq!(v.friend_sources, 3, "not said: the default stays");
    }

    #[test]
    fn the_echo_of_an_earlier_write_does_not_undo_a_later_click() {
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
        t.edit(Change::Theme(Pref::Light));
        let seq2 = t.edit(Change::Theme(Pref::Coal));
        // The gateway echoes the first write.
        t.gateway(snapshot("light"));
        assert_eq!(shown(&t), Pref::Coal);
        t.answered(Some(snapshot("coal")), seq2, false);
        assert_eq!(shown(&t), Pref::Coal);
        t.gateway(snapshot("coal"));
        assert_eq!(shown(&t), Pref::Coal);
        assert!(!t.is_waiting(account_kind()));
    }

    #[test]
    fn a_lost_echo_cannot_pin_an_edit() {
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
        let seq = t.edit(Change::Theme(Pref::Light));
        t.answered(Some(snapshot("light")), seq, false);
        assert!(t.is_waiting(account_kind()));
        t.expire(seq, false);
        assert!(!t.is_waiting(account_kind()));
        assert_eq!(shown(&t), Pref::Light, "what the answer said stands");
    }

    #[test]
    fn a_failed_write_goes_back_to_what_the_server_holds() {
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
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
        t.gateway(snapshot("dark"));
        let first = t.edit(Change::Theme(Pref::Light));
        let second = t.edit(Change::Theme(Pref::Coal));
        assert!(second > first);
        // The answer is for the first command only; the second still waits.
        t.answered(Some(snapshot("light")), first, false);
        t.gateway(snapshot("light"));
        assert_eq!(shown(&t), Pref::Coal);
        assert!(t.is_waiting(account_kind()));
    }

    #[test]
    fn preferences_are_settled_by_their_own_answer_and_the_next_snapshot() {
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("dark"));
        let seq = t.edit(Change::GroupSpacing(8));
        assert_eq!(t.values().group_spacing, 8);
        // An answer to an account write does not settle a preference edit.
        t.answered(None, seq, false);
        t.gateway(snapshot("dark"));
        assert_eq!(t.values().group_spacing, 8);
        t.answered(None, seq, true);
        assert_eq!(
            t.values().group_spacing,
            8,
            "answered, the echo is still to come"
        );
        assert!(t.is_waiting(Change::GroupSpacing(0).kind()));
        let display = Display {
            font_size: Some(18.),
            message_group_spacing: Some(8.),
            ..Display::default()
        };
        t.gateway(Values::from_server(
            &fluxer("dark", 3),
            Some(display),
            &Values::default(),
        ));
        assert_eq!(t.values().font_size, 18);
        assert_eq!(t.values().group_spacing, 8);
        assert!(
            !t.is_waiting(Change::GroupSpacing(0).kind()),
            "settled by the echo"
        );
    }

    #[test]
    fn values_of_the_server_fill_in_what_it_does_not_say() {
        let s: p::UserSettings = serde_json::from_value(json!({
            "theme": "dark_legacy", "render_spoilers": 9,
            "friend_source_flags": 15, "default_guilds_restricted": true,
            "animate_stickers": 2, "developer_mode": true
        }))
        .unwrap();
        let v = Values::from_server(
            &s,
            Some(Display {
                font_size: Some(500.),
                message_group_spacing: Some(f64::NAN),
                ..Display::default()
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
    fn the_sizes_are_fluxers_lists_and_other_values_snap_to_them() {
        assert_eq!(FONT_SIZES, [12, 14, 15, 16, 18, 20, 24]);
        assert_eq!(GROUP_SPACINGS, [0, 4, 8, 16, 24]);
        // A 13 px value written by another client shows as the nearest size.
        assert_eq!(nearest(&FONT_SIZES, 13), 12);
        assert_eq!(nearest(&FONT_SIZES, 17), 16);
        assert_eq!(nearest(&FONT_SIZES, 40), 24);
        assert_eq!(step(&FONT_SIZES, 16, 1), 18);
        assert_eq!(step(&FONT_SIZES, 16, -1), 15);
        assert_eq!(step(&FONT_SIZES, 24, 1), 24, "stops at the end");
        assert_eq!(step(&GROUP_SPACINGS, 0, -1), 0);
        assert_eq!(
            step(&GROUP_SPACINGS, 10, 1),
            16,
            "10 snaps to 8 first, then steps"
        );
    }

    #[test]
    fn the_compact_display_has_its_own_spacing_and_avatars() {
        let mut v = Values::default();
        assert_eq!((v.group_spacing, v.compact_group_spacing), (16, 0));
        v.apply(&Change::CompactGroupSpacing(8));
        v.apply(&Change::CompactAvatars(true));
        assert_eq!((v.group_spacing, v.compact_group_spacing), (16, 8));
        assert!(v.compact_avatars);
        let d = Display {
            compact_message_group_spacing: Some(4.),
            show_avatars_in_compact_mode: Some(false),
            ..Display::default()
        };
        v.apply_display(&d);
        assert_eq!(v.compact_group_spacing, 4);
        assert!(!v.compact_avatars);
    }

    #[test]
    fn another_account_starts_without_the_old_ones_edits() {
        let mut t = Tracker::new(Values::default());
        t.gateway(snapshot("light"));
        t.edit(Change::Theme(Pref::Coal));
        t.reset_account();
        assert!(t.gateway(snapshot("dark")));
        assert_eq!(shown(&t), Pref::Dark);
    }

    /// The display part of a snapshot of the synced preferences with the action bar set.
    fn action_bar_echo(on: bool) -> Display {
        let mut prefs = inlet_protocol::api::preferences::Preferences::default();
        Flag::ActionBar.write(&mut prefs, on);
        Display::of_preferences(&prefs)
    }

    #[test]
    fn a_switch_edit_stays_on_top_until_the_synced_preferences_echo_it() {
        let mut t = Tracker::new(Values::default());
        let seq = t.edit(Change::Flag(Flag::ActionBar, false));
        assert!(!t.values().flag(Flag::ActionBar), "shown at once");
        let kind = Change::Flag(Flag::ActionBar, false).kind();

        // The echo of an earlier write still holds the old value: the edit stays.
        let old = Values::from_server(
            &fluxer("dark", 3),
            Some(action_bar_echo(true)),
            t.confirmed(),
        );
        t.gateway(old);
        assert!(!t.values().flag(Flag::ActionBar));
        assert!(t.is_waiting(kind));
        // The answer alone does not settle it either.
        t.answered(None, seq, true);
        assert!(t.is_waiting(kind));
        // The snapshot that holds the edit does.
        let echo = Values::from_server(
            &fluxer("dark", 3),
            Some(action_bar_echo(false)),
            t.confirmed(),
        );
        t.gateway(echo);
        assert!(!t.is_waiting(kind));
        assert!(!t.values().flag(Flag::ActionBar));

        // Another client turns it back on: that wins.
        let other = Values::from_server(
            &fluxer("dark", 3),
            Some(action_bar_echo(true)),
            t.confirmed(),
        );
        t.gateway(other);
        assert!(t.values().flag(Flag::ActionBar));
    }

    #[test]
    fn a_local_switch_is_held_at_once_and_survives_the_servers_snapshots() {
        let mut t = Tracker::new(Values::default());
        t.set_local(Change::Flag(Flag::HideBlockedMessages, true));
        assert!(t.values().flag(Flag::HideBlockedMessages));
        assert!(!t.is_waiting(Change::Flag(Flag::HideBlockedMessages, true).kind()));
        // A snapshot says nothing about it (it is not in the preferences).
        let snap = Values::from_server(
            &fluxer("dark", 3),
            Some(action_bar_echo(true)),
            t.confirmed(),
        );
        t.gateway(snap);
        assert!(t.values().flag(Flag::HideBlockedMessages));
    }

    #[test]
    fn a_local_switch_flipped_before_an_echo_expires_is_not_taken_back() {
        let mut t = Tracker::new(Values::default());
        let seq = t.edit(Change::Theme(Pref::Coal));
        // The write is answered with the values from then; the echo has not come.
        let answer = Values::from_server(&fluxer("coal", 3), None, t.confirmed());
        t.answered(Some(answer), seq, false);
        // In the wait, the person flips a switch that is only for this computer.
        t.set_local(Change::Flag(Flag::HideBlockedMessages, true));
        // The echo never comes: the answer's values stand, and the switch keeps its value.
        t.expire(seq, false);
        assert!(t.values().flag(Flag::HideBlockedMessages));
        assert_eq!(shown(&t), Pref::Coal);
    }

    #[test]
    fn switches_of_a_new_account_without_preferences_have_fluxers_defaults() {
        // An account that never wrote its preferences has none: every switch is at its
        // default, whatever the last account had.
        let mut off = Values::default();
        off.apply(&Change::Flag(Flag::ActionBar, false));
        let v = Values::from_server(&fluxer("dark", 3), Some(Display::default()), &off);
        assert!(v.flag(Flag::ActionBar));
        assert!(!v.flag(Flag::DoubleClickActions));
    }
}
