// SPDX-License-Identifier: AGPL-3.0-or-later
//! Demo data for the rail and channel list scenes: more servers, in folders; people in
//! a voice channel, a link channel and an age-restricted channel in `Inlet Dev`; and
//! the answers to invite and leave commands (there is no server to ask).

use std::collections::HashMap;

use serde_json::{json, Value};

use super::scenes::Scene;
use super::{channel, snowflake, user, user_id};
use crate::backend::settings::guilds::{Cmd, Ev, Kind, Preview};
use crate::backend::settings::Ev as SettingsEv;
use crate::backend::Ui;
use crate::model::{Channel, ChannelKind, Guild};

/// The scenes with this data.
pub fn is_nav_scene(scene: Scene) -> bool {
    matches!(
        scene,
        Scene::RailFolders | Scene::GuildBanner | Scene::GuildMenu | Scene::InviteAccept
    )
}

/// The servers the folders hold, besides the three every scene has.
pub fn extra_guilds() -> Vec<Guild> {
    [
        "Hedgerow Lab",
        "Tabletop Club",
        "Music Corner",
        "Night Owls",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| Guild {
        id: format!("g{}", i + 4),
        name: (*name).into(),
    })
    .collect()
}

/// Channel lists of the extra servers, and the link and age-restricted channels.
pub fn add_channels(channels: &mut HashMap<Option<String>, Vec<Channel>>) {
    for i in 4..=7 {
        channels.insert(
            Some(format!("g{i}")),
            vec![channel(
                &format!("n{i}-general"),
                "general",
                ChannelKind::Text,
                None,
                0,
            )],
        );
    }
    if let Some(list) = channels.get_mut(&Some("g1".to_string())) {
        list.push(channel(
            "c-link",
            "project-board",
            ChannelKind::Link,
            Some("c-community"),
            9,
        ));
        list.push(channel(
            "c-mature",
            "mature-chat",
            ChannelKind::Text,
            Some("c-community"),
            10,
        ));
    }
}

/// The account's folders in the rail scenes: an open folder, a closed one with its icon,
/// and loose servers between them.
pub fn user_settings(scene: Scene) -> Option<Value> {
    if !is_nav_scene(scene) {
        return None;
    }
    let id = |g: &str| snowflake(g);
    Some(json!({
        "status": "online",
        "theme": "dark",
        "custom_status": null,
        "guild_folders": [
            {"id": -1, "guild_ids": [id("g1")]},
            {"id": 1, "name": "Hedgerow", "color": 0x3498DB, "flags": 0, "icon": "folder",
             "guild_ids": [id("g2"), id("g3"), id("g4")]},
            {"id": -1, "guild_ids": [id("g5")]},
            {"id": 2, "name": "Free time", "color": 0xE67E22, "flags": 1, "icon": "game_controller",
             "guild_ids": [id("g6"), id("g7")]}
        ]
    }))
}

/// The guild settings of the navigation scenes: in `guild-banner`, two categories of
/// `Inlet Dev` are folded, so the voice channels and the link channel are in view.
pub fn guild_settings(scene: Scene) -> Option<Vec<Value>> {
    (scene == Scene::GuildBanner).then(|| {
        vec![json!({"guild_id": snowflake("g1"), "channel_overrides": [
            {"channel_id": snowflake("c-info"), "collapsed": true, "message_notifications": 3},
            {"channel_id": snowflake("c-text"), "collapsed": true, "message_notifications": 3},
        ]})]
    })
}

/// Fields the READY needs for the extra channels.
pub fn decorate_channel(channel_id: &str, v: &mut Value) {
    if channel_id == snowflake("c-link") {
        v["url"] = json!("https://example.com/project-board");
    }
    if channel_id == snowflake("c-mature") {
        v["nsfw"] = json!(true);
    }
}

/// Who is in the voice channels of `Inlet Dev`, and the members that names them.
pub fn voice(guild: &str) -> (Value, Vec<Value>) {
    if guild != snowflake("g1") {
        return (json!([]), Vec::new());
    }
    let state = |who: &str, channel: &str, extra: Value| {
        let mut v = json!({
            "guild_id": guild,
            "channel_id": snowflake(channel),
            "user_id": user_id(who),
            "connection_id": format!("conn-{who}"),
            "version": 1,
        });
        if let (Value::Object(v), Value::Object(e)) = (&mut v, extra) {
            v.extend(e);
        }
        v
    };
    let states = json!([
        state("ada", "c-lounge", json!({})),
        state("grace", "c-lounge", json!({"self_mute": true})),
        state(
            "linus",
            "c-lounge",
            json!({"self_deaf": true, "self_video": true})
        ),
        state("Sylo", "c-music", json!({"self_stream": true})),
    ]);
    let members = ["ada", "grace", "linus", "Sylo"]
        .iter()
        .map(
            |who| json!({"user": user(who), "roles": [], "joined_at": "2026-01-01T00:00:00+00:00"}),
        )
        .collect();
    (states, members)
}

/// What an invite shows in the scenes.
pub fn preview(code: &str) -> Preview {
    Preview {
        code: code.chars().take(256).collect(),
        kind: Kind::Guild,
        guild_id: Some(snowflake("g9")),
        guild_name: "Hedgerow Garden Club".into(),
        icon: None,
        channel_name: Some("welcome".into()),
        channel_id: None,
        inviter: Some("Ada".into()),
        members: Some(1_284),
        online: Some(213),
    }
}

/// The demo has no server: a preview shows the demo invite, joining and leaving say so.
pub fn answer(events: &Ui, cmd: Cmd) {
    let ev = match cmd {
        Cmd::PreviewInvite { code } => Ev::Preview {
            result: Ok(Box::new(preview(&code))),
            code,
        },
        Cmd::AcceptInvite { code } => Ev::Accepted {
            code,
            result: Err("this is the demo".into()),
        },
        Cmd::Leave { guild_id } => Ev::Left {
            guild_id,
            result: Err("this is the demo".into()),
        },
    };
    let _ = events.unbounded_send(SettingsEv::Guild(ev));
}
