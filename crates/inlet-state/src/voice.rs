//! Who is in which voice channel of a guild, from the voice states in READY and
//! GUILD_CREATE and VOICE_STATE_UPDATE. Only the states are kept: joining a voice
//! channel is not done here.

use std::collections::BTreeMap;

use inlet_protocol::{Dispatch, VoiceState};

use crate::ids::{ChannelId, GuildId, UserId};
use crate::Changes;

/// Voice states kept for the account (Fluxer guilds are far below this).
pub const MAX_VOICE_STATES: usize = 20_000;
/// Longest connection ID kept.
const MAX_CONNECTION_LEN: usize = 64;

/// One person's connection to a voice channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub user: UserId,
    /// The connection (a person on two devices has two).
    pub connection: String,
    /// Muted by themselves or by a moderator.
    pub muted: bool,
    /// Deafened by themselves or by a moderator.
    pub deafened: bool,
    /// Sends video.
    pub video: bool,
    /// Shares the screen.
    pub streaming: bool,
    version: u64,
}

type Key = (UserId, String);

/// The voice states of one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Voice {
    by_channel: BTreeMap<ChannelId, BTreeMap<Key, Participant>>,
    /// Where each connection is now, so a move leaves the old channel.
    index: BTreeMap<Key, (GuildId, ChannelId)>,
}

impl Voice {
    /// The people in a voice channel, in the order they are listed (by user ID, so
    /// the list does not jump around when someone mutes).
    pub fn participants(&self, channel: ChannelId) -> Vec<&Participant> {
        self.by_channel
            .get(&channel)
            .map(|m| m.values().collect())
            .unwrap_or_default()
    }

    /// Whether no voice state is kept.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// How many connections are in a voice channel.
    pub fn count(&self, channel: ChannelId) -> usize {
        self.by_channel.get(&channel).map_or(0, BTreeMap::len)
    }

    /// Whether anyone is in a voice channel of the guild.
    pub fn any_in_guild(&self, guild: GuildId) -> bool {
        self.index.values().any(|(g, _)| *g == guild)
    }

    /// Replaces the states of a guild (READY, GUILD_CREATE): who is in voice now.
    pub(crate) fn replace_guild(&mut self, guild: GuildId, states: &[VoiceState]) {
        self.drop_guild(guild);
        for s in states {
            self.upsert(s, Some(guild));
        }
    }

    /// Forgets a guild's states (it was left or deleted).
    pub(crate) fn drop_guild(&mut self, guild: GuildId) {
        let gone: Vec<Key> = self
            .index
            .iter()
            .filter(|(_, (g, _))| *g == guild)
            .map(|(k, _)| k.clone())
            .collect();
        for key in gone {
            self.remove(&key);
        }
    }

    fn remove(&mut self, key: &Key) -> bool {
        let Some((_, channel)) = self.index.remove(key) else {
            return false;
        };
        if let Some(list) = self.by_channel.get_mut(&channel) {
            list.remove(key);
            if list.is_empty() {
                self.by_channel.remove(&channel);
            }
        }
        true
    }

    /// Applies one state. `guild` is the guild the state is known to belong to (the
    /// event's own `guild_id` when `None`). Whether the list changed.
    fn upsert(&mut self, s: &VoiceState, guild: Option<GuildId>) -> bool {
        let Some(user) = s.user_id.as_deref().and_then(UserId::parse) else {
            return false;
        };
        let Some(guild) = guild.or_else(|| s.guild_id.as_deref().and_then(GuildId::parse)) else {
            // A call in a direct message: not shown in a channel list.
            return false;
        };
        let connection: String = s
            .connection_id
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(MAX_CONNECTION_LEN)
            .collect();
        let key = (user, connection.clone());
        let Some(channel) = s.channel_id.as_deref().and_then(ChannelId::parse) else {
            // The participant left.
            return self.remove(&key);
        };
        // An older state of the same connection changes nothing.
        if let Some((_, at)) = self.index.get(&key) {
            if let Some(old) = self.by_channel.get(at).and_then(|m| m.get(&key)) {
                if s.version != 0 && old.version != 0 && s.version < old.version {
                    return false;
                }
            }
        }
        if !self.index.contains_key(&key) && self.index.len() >= MAX_VOICE_STATES {
            return false;
        }
        let new = Participant {
            user,
            connection,
            muted: s.mute || s.self_mute,
            deafened: s.deaf || s.self_deaf,
            video: s.self_video,
            streaming: s.self_stream,
            version: s.version,
        };
        let moved = self.index.get(&key).map(|(_, c)| *c);
        if let Some(from) = moved.filter(|c| *c != channel) {
            if let Some(list) = self.by_channel.get_mut(&from) {
                list.remove(&key);
                if list.is_empty() {
                    self.by_channel.remove(&from);
                }
            }
        }
        self.index.insert(key.clone(), (guild, channel));
        let list = self.by_channel.entry(channel).or_default();
        let changed = list.get(&key) != Some(&new) || moved != Some(channel);
        list.insert(key, new);
        changed
    }

    /// Applies an event.
    pub fn apply(&mut self, event: &Dispatch) -> Changes {
        let mut c = Changes::default();
        if let Dispatch::VoiceStateUpdate(s) = event {
            if self.upsert(s, None) {
                c.voice = true;
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(user: &str, channel: Option<&str>, conn: &str, version: u64) -> VoiceState {
        serde_json::from_value(json!({
            "guild_id": "100", "channel_id": channel, "user_id": user,
            "connection_id": conn, "version": version, "self_mute": true
        }))
        .unwrap()
    }

    fn update(s: VoiceState) -> Dispatch {
        Dispatch::VoiceStateUpdate(Box::new(s))
    }

    #[test]
    fn people_join_move_and_leave_voice_channels() {
        let mut v = Voice::default();
        let (a, b) = (ChannelId(300), ChannelId(301));
        assert!(v.apply(&update(state("10", Some("300"), "c1", 1))).voice);
        assert!(v.apply(&update(state("11", Some("300"), "c2", 1))).voice);
        assert_eq!(v.count(a), 2);
        assert!(v.participants(a)[0].muted);
        // The same state again changes nothing.
        assert!(!v.apply(&update(state("10", Some("300"), "c1", 1))).voice);
        // A move leaves the old channel.
        v.apply(&update(state("10", Some("301"), "c1", 2)));
        assert_eq!((v.count(a), v.count(b)), (1, 1));
        // An older state is ignored.
        assert!(!v.apply(&update(state("10", Some("300"), "c1", 1))).voice);
        assert_eq!((v.count(a), v.count(b)), (1, 1));
        // Leaving.
        v.apply(&update(state("10", None, "c1", 3)));
        assert_eq!(v.count(b), 0);
        assert!(v.any_in_guild(GuildId(100)));
        v.drop_guild(GuildId(100));
        assert!(!v.any_in_guild(GuildId(100)));
        assert_eq!(v.count(a), 0);
    }

    #[test]
    fn calls_and_bad_ids_are_not_kept_and_the_total_is_bounded() {
        let mut v = Voice::default();
        let call: VoiceState = serde_json::from_value(
            json!({"channel_id": "300", "user_id": "10", "connection_id": "c"}),
        )
        .unwrap();
        assert!(!v.apply(&update(call)).voice);
        assert!(!v.apply(&update(state("x", Some("300"), "c", 1))).voice);
        let states: Vec<VoiceState> = (0..MAX_VOICE_STATES as u64 + 50)
            .map(|i| state(&(i + 1).to_string(), Some("300"), "c", 1))
            .collect();
        v.replace_guild(GuildId(100), &states);
        assert_eq!(v.count(ChannelId(300)), MAX_VOICE_STATES);
    }
}
