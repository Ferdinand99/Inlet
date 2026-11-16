//! The guild folders and the voice states of the account.

mod common;

use common::*;
use inlet_state::AccountState;
use serde_json::json;

#[test]
fn folders_come_from_ready_and_follow_settings_updates() {
    let mut v = ready_json(
        guild_ready(default_roles(), default_channels(), &[MOD_ROLE]),
        json!([]),
        json!([]),
    );
    v["user_settings"] = json!({"guild_folders": [
        {"id": 1, "name": "Work", "color": 16711680, "icon": "shield", "guild_ids": [GUILD]}
    ]});
    let mut s = AccountState::from_ready_at(&ready_from(v), JOINED_MS as i64);
    assert_eq!(s.settings().guild_folders().len(), 1);
    assert_eq!(
        s.settings().guild_folders()[0].name.as_deref(),
        Some("Work")
    );

    let c = s.apply(&d(
        "USER_SETTINGS_UPDATE",
        json!({"guild_folders": [{"id": -1, "guild_ids": [GUILD]}]}),
    ));
    assert!(c.folders);
    assert!(s.settings().guild_folders()[0].is_uncategorized());
    // A new READY brings the folders again.
    let c = s.apply(&d(
        "READY",
        ready_json(
            guild_ready(default_roles(), default_channels(), &[MOD_ROLE]),
            json!([]),
            json!([]),
        ),
    ));
    assert!(c.reset);
    assert!(s.settings().guild_folders().is_empty());
}

#[test]
fn voice_states_follow_the_guild_and_name_the_person() {
    let mut guild = guild_ready(default_roles(), default_channels(), &[MOD_ROLE]);
    guild["voice_states"] = json!([
        {"guild_id": GUILD, "channel_id": VOICE, "user_id": OTHER, "connection_id": "c1",
         "version": 1, "self_deaf": true}
    ]);
    let mut s = AccountState::from_ready_at(
        &ready_from(ready_json(guild, json!([]), json!([]))),
        JOINED_MS as i64,
    );
    let list = s.voice().participants(ch(VOICE));
    assert_eq!(list.len(), 1);
    assert!(list[0].deafened && !list[0].video);
    // Someone joins with a member object: the guild knows them afterwards.
    let c = s.apply(&d(
        "VOICE_STATE_UPDATE",
        json!({"guild_id": GUILD, "channel_id": VOICE, "user_id": "30", "connection_id": "c2",
               "version": 1,
               "member": {"user": {"id": "30", "username": "bo"}, "roles": [],
                          "joined_at": "2026-01-01T00:00:00Z"}}),
    ));
    assert!(c.voice);
    assert_eq!(s.voice().count(ch(VOICE)), 2);
    assert!(s.member(g(), uid("30")).is_some());
    // The guild goes: so do its voice states.
    s.apply(&d("GUILD_DELETE", json!({"id": GUILD})));
    assert_eq!(s.voice().count(ch(VOICE)), 0);
}
