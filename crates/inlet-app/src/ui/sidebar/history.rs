// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where the person has been: the history that Alt+Left and Alt+Right walk (Command+[
//! and Command+] on macOS), the last channel of each server, the server the last-server
//! key goes back to, the server slots (Ctrl+1 to Ctrl+9) and the place the next start
//! opens.
//!
//! Fluxer keeps the same in `SelectedChannel` (the viewed-channel history, at most 50
//! entries, in memory), `SelectedGuild` (the last server) and `Location` (the last
//! place, saved per account). The history is not saved: it starts empty at each start,
//! and the saved place is where the first entry of the next one is.

use std::collections::HashMap;

use gpui::{Context, Window};
use inlet_state::ChannelId;
use serde::{Deserialize, Serialize};

use crate::app::Inlet;

/// Entries the history keeps (Fluxer: 50).
pub const HISTORY_LIMIT: usize = 50;
/// Servers whose last channel is kept at once, so a very long session cannot grow it.
const LAST_CHANNELS_LIMIT: usize = 1_000;

/// A place: a channel, with the server it is in (`None` for a direct message).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    pub guild: Option<String>,
    pub channel: String,
}

fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

impl Visit {
    /// Whether both IDs have the shape of a snowflake (a hand-edited file may not).
    pub fn is_valid(&self) -> bool {
        is_id(&self.channel) && self.guild.as_deref().is_none_or(is_id)
    }
}

/// The history and the other places this session remembers.
#[derive(Default)]
pub struct History {
    entries: Vec<Visit>,
    /// The entry the person is at; `None` while the history is empty.
    at: Option<usize>,
    /// The channel last open in each server (`None` is the direct messages).
    last_channel: HashMap<Option<String>, String>,
    /// The server last open, for the key that toggles between it and the direct
    /// messages.
    last_guild: Option<String>,
    /// The account whose saved place was looked at (once per account and start).
    restored_for: Option<String>,
}

impl History {
    /// Records a channel the person opened. The entry the person is at is not recorded
    /// twice (also what a step back arrives at); anything ahead of it is dropped, like a
    /// browser's forward list after a new page.
    pub fn record(&mut self, visit: Visit) {
        self.last_channel
            .insert(visit.guild.clone(), visit.channel.clone());
        if self.last_channel.len() > LAST_CHANNELS_LIMIT {
            self.last_channel.clear();
            self.last_channel
                .insert(visit.guild.clone(), visit.channel.clone());
        }
        if visit.guild.is_some() {
            self.last_guild = visit.guild.clone();
        }
        if self.at.and_then(|i| self.entries.get(i)) == Some(&visit) {
            return;
        }
        if let Some(i) = self.at {
            self.entries.truncate(i + 1);
        }
        self.entries.push(visit);
        if self.entries.len() > HISTORY_LIMIT {
            let over = self.entries.len() - HISTORY_LIMIT;
            self.entries.drain(..over);
        }
        self.at = Some(self.entries.len() - 1);
    }

    /// Moves one entry back or forward past the entries that are no longer there
    /// (`usable` says), and returns the place arrived at. `None` (and no move) when
    /// there is none that way.
    pub fn step(&mut self, forward: bool, usable: impl Fn(&Visit) -> bool) -> Option<Visit> {
        let mut i = self.at?;
        loop {
            i = if forward { i + 1 } else { i.checked_sub(1)? };
            let entry = self.entries.get(i)?;
            if usable(entry) {
                self.at = Some(i);
                return Some(entry.clone());
            }
        }
    }

    /// The channel last open in a server.
    pub fn last_channel_of(&self, guild: &str) -> Option<&str> {
        self.last_channel
            .get(&Some(guild.to_owned()))
            .map(String::as_str)
    }

    /// The server last open.
    pub fn last_guild(&self) -> Option<&str> {
        self.last_guild.as_deref()
    }

    /// Takes the right to look at the saved place of `account`: true once per account.
    fn restore_once(&mut self, account: &str) -> bool {
        if self.restored_for.as_deref() == Some(account) {
            return false;
        }
        self.restored_for = Some(account.to_owned());
        true
    }

    /// Forgets everything (the account changed).
    pub fn clear(&mut self) {
        *self = History::default();
    }
}

/// The servers as the rail lists them, folders opened out, with the ones that are in
/// `known` only.
pub fn slot_order(rail: &[crate::ui::rail::order::Organized], known: &[String]) -> Vec<String> {
    use crate::ui::rail::order::Organized;
    let mut out: Vec<String> = Vec::new();
    for entry in rail {
        match entry {
            Organized::Guild(id) => out.push(id.clone()),
            Organized::Folder { guilds, .. } => out.extend(guilds.iter().cloned()),
        }
    }
    out.retain(|id| known.contains(id));
    out
}

impl Inlet {
    /// A channel was opened by the person (or by a step through the history, which
    /// arrives at the entry it is already at): remember it in the history, as the last
    /// channel of its server and as the place the next start opens.
    pub(crate) fn record_visit(&mut self, channel: &str, cx: &mut Context<Self>) {
        let visit = Visit {
            guild: self.selected_guild.clone(),
            channel: channel.to_owned(),
        };
        if !visit.is_valid() {
            return;
        }
        self.sidebar.history.record(visit.clone());
        let Some(key) = self.nav_account_key(cx) else {
            return;
        };
        if let Some(mem) = self.sidebar.remembered.of_mut(&key) {
            if mem.last_place.as_ref() != Some(&visit) {
                mem.last_place = Some(visit);
                self.sidebar.remembered.save_in_background(cx);
            }
        }
    }

    /// Whether a place in the history can be opened still: the channel is there and
    /// can be seen, and the server is one of the account's.
    fn visit_usable(&self, visit: &Visit, cx: &gpui::App) -> bool {
        let (Some(account), Some(ch)) = (self.account.as_ref(), ChannelId::parse(&visit.channel))
        else {
            return false;
        };
        account.read(cx).channel_permissions(ch).can_view()
            && visit
                .guild
                .as_ref()
                .is_none_or(|g| self.guilds.iter().any(|x| &x.id == g))
    }

    /// Opens a place: the channel itself when its server is open, else the server first.
    fn open_visit(&mut self, visit: Visit, window: &mut Window, cx: &mut Context<Self>) {
        self.rail.favorites = false;
        if self.selected_guild == visit.guild {
            self.select_channel(visit.channel, window, cx);
        } else {
            self.follow_channel_link(visit.guild, visit.channel, window, cx);
        }
    }

    /// Alt+Left and Alt+Right (Command+[ and Command+]): one place back or forward.
    pub(crate) fn step_history(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.shortcuts_allowed() {
            return;
        }
        let mut history = std::mem::take(&mut self.sidebar.history);
        let visit = history.step(forward, |v| self.visit_usable(v, cx));
        self.sidebar.history = history;
        if let Some(visit) = visit {
            self.open_visit(visit, window, cx);
        }
    }

    /// The servers in the order of the rail.
    fn slot_guilds(&self, cx: &gpui::App) -> Vec<String> {
        let known: Vec<String> = self.guilds.iter().map(|g| g.id.clone()).collect();
        slot_order(&self.rail_organized(cx), &known)
    }

    /// Ctrl+1 to Ctrl+9: the direct messages, then the first eight servers of the rail.
    /// A server opens at the channel it had open last.
    pub(crate) fn go_to_slot(&mut self, slot: usize, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        self.rail.favorites = false;
        if slot == 0 {
            if self.selected_guild.is_some() {
                self.select_guild(None, cx);
            }
            return;
        }
        let Some(guild) = self.slot_guilds(cx).get(slot - 1).cloned() else {
            return;
        };
        if self.selected_guild.as_deref() != Some(guild.as_str()) {
            self.select_guild(Some(guild), cx);
        }
    }

    /// Ctrl+Alt+Right: from a server to the direct messages, and from there back to the
    /// server that was open last.
    pub(crate) fn toggle_last_server(&mut self, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        self.rail.favorites = false;
        if self.selected_guild.is_some() {
            self.select_guild(None, cx);
        } else if let Some(guild) = self
            .sidebar
            .history
            .last_guild()
            .filter(|g| self.guilds.iter().any(|x| x.id == *g))
            .map(str::to_owned)
        {
            self.select_guild(Some(guild), cx);
        }
    }

    /// The channel a server opens at: the one last open in it when it is still there
    /// and can be opened, else `None` (the first channel opens).
    pub(crate) fn remembered_channel_of(&self, guild: &str) -> Option<String> {
        let wanted = self.sidebar.history.last_channel_of(guild)?;
        self.channels
            .iter()
            .any(|c| c.id == wanted && c.is_messageable())
            .then(|| wanted.to_owned())
    }

    /// The first time the servers are known for an account: opens the place it was at
    /// when the app last ran, unless the person already went somewhere or an account
    /// switch is on its way to a channel.
    pub(crate) fn restore_last_place(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.nav_account_key(cx) else {
            return;
        };
        if !self.sidebar.history.restore_once(&key) {
            return;
        }
        if self.selected_guild.is_some()
            || self.selected_channel.is_some()
            || self.switcher.has_wanted_channel()
            || self.overlays.link_channel.is_some()
            || self.nav.join_target.is_some()
        {
            return;
        }
        let Some(visit) = self
            .sidebar
            .remembered
            .of(&key)
            .and_then(|m| m.last_place.clone())
        else {
            return;
        };
        match visit.guild {
            Some(guild) if self.guilds.iter().any(|g| g.id == guild) => {
                self.overlays.link_channel = Some(visit.channel);
                self.select_guild(Some(guild), cx);
            }
            Some(_) => {}
            None => {
                // The direct messages are the list that is open: its channel opens as
                // soon as the list has it.
                self.social.last_dm = Some(visit.channel.clone());
                if self.channels.iter().any(|c| c.id == visit.channel) {
                    self.chat.pending_channel_jump = Some(visit.channel);
                }
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(guild: Option<&str>, channel: &str) -> Visit {
        Visit {
            guild: guild.map(str::to_owned),
            channel: channel.to_owned(),
        }
    }

    #[test]
    fn history_walks_back_and_forward_and_drops_what_is_ahead() {
        let mut h = History::default();
        for ch in ["1", "2", "3"] {
            h.record(v(Some("9"), ch));
        }
        assert_eq!(h.step(false, |_| true), Some(v(Some("9"), "2")));
        assert_eq!(h.step(false, |_| true), Some(v(Some("9"), "1")));
        assert_eq!(h.step(false, |_| true), None, "nothing before the first");
        assert_eq!(h.step(true, |_| true), Some(v(Some("9"), "2")));
        // Arriving at the entry the step moved to adds nothing.
        h.record(v(Some("9"), "2"));
        assert_eq!(h.step(true, |_| true), Some(v(Some("9"), "3")));
        assert_eq!(h.step(true, |_| true), None, "nothing after the last");
        // A new place after a step back cuts the forward list.
        assert_eq!(h.step(false, |_| true), Some(v(Some("9"), "2")));
        h.record(v(None, "7"));
        assert_eq!(h.step(true, |_| true), None);
        assert_eq!(h.step(false, |_| true), Some(v(Some("9"), "2")));
    }

    #[test]
    fn history_skips_places_that_are_gone() {
        let mut h = History::default();
        for ch in ["1", "2", "3", "4"] {
            h.record(v(None, ch));
        }
        // 3 and 2 are gone: back goes straight to 1.
        assert_eq!(
            h.step(false, |p| p.channel == "1" || p.channel == "4"),
            Some(v(None, "1"))
        );
        // Nothing usable ahead but the entries are all gone: no move.
        assert_eq!(h.step(true, |p| p.channel == "2"), Some(v(None, "2")));
        assert_eq!(h.step(true, |_| false), None);
        assert_eq!(h.step(false, |p| p.channel == "1"), Some(v(None, "1")));
    }

    #[test]
    fn history_keeps_fifty_places() {
        let mut h = History::default();
        for n in 0..(HISTORY_LIMIT + 20) {
            h.record(v(None, &n.to_string()));
        }
        assert_eq!(h.entries.len(), HISTORY_LIMIT);
        assert_eq!(h.entries[0].channel, "20");
        let mut steps = 0;
        while h.step(false, |_| true).is_some() {
            steps += 1;
        }
        assert_eq!(steps, HISTORY_LIMIT - 1);
    }

    #[test]
    fn a_server_remembers_its_last_channel_and_the_last_server() {
        let mut h = History::default();
        h.record(v(Some("1"), "11"));
        h.record(v(Some("1"), "12"));
        h.record(v(None, "50"));
        h.record(v(Some("2"), "21"));
        h.record(v(None, "51"));
        assert_eq!(h.last_channel_of("1"), Some("12"));
        assert_eq!(h.last_channel_of("2"), Some("21"));
        assert_eq!(h.last_channel_of("3"), None);
        assert_eq!(
            h.last_guild(),
            Some("2"),
            "direct messages are not a server"
        );
    }

    #[test]
    fn the_last_place_is_looked_at_once_per_account() {
        let mut h = History::default();
        assert!(h.restore_once("a#1"));
        assert!(!h.restore_once("a#1"));
        assert!(h.restore_once("a#2"));
        h.clear();
        assert!(h.restore_once("a#2"));
    }

    #[test]
    fn slots_follow_the_rail_with_folders_opened_out() {
        use crate::ui::rail::order::Organized;
        use inlet_protocol::models::GuildFolder;
        let folder: GuildFolder =
            serde_json::from_value(serde_json::json!({"id": 1, "guild_ids": ["2", "3"]})).unwrap();
        let rail = vec![
            Organized::Guild("1".into()),
            Organized::Folder {
                folder,
                guilds: vec!["2".into(), "3".into()],
            },
            Organized::Guild("4".into()),
        ];
        let known: Vec<String> = ["1", "2", "4"].map(String::from).to_vec();
        assert_eq!(slot_order(&rail, &known), ["1", "2", "4"]);
    }

    #[test]
    fn a_hand_edited_place_with_odd_ids_is_not_valid() {
        assert!(v(Some("123"), "456").is_valid());
        assert!(v(None, "456").is_valid());
        assert!(!v(Some("../1"), "456").is_valid());
        assert!(!v(None, "").is_valid());
        assert!(!v(None, &"9".repeat(21)).is_valid());
    }
}
