// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings budget and the profile edit, against a mock Fluxer instance.

use std::time::Duration;

use inlet_protocol as p;
use serde_json::{json, Value};
use tokio::time::Instant;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::profile::{self, ImageChange, ProfileSave};
use super::write::{Change, Coalescer, Next, Wish, Writer, GATHER, MIN_GAP};
use crate::backend::{Event, Ui};
use crate::theme::Pref;

fn discovery(base: &str) -> Value {
    json!({
        "api_code_version": 1,
        "endpoints": {
            "api": format!("{base}/api"), "api_client": format!("{base}/api"),
            "api_public": format!("{base}/api"), "gateway": "ws://127.0.0.1:1/gateway",
            "media": format!("{base}/media"), "webapp": base,
            "invite": format!("{base}/invite")
        },
        "captcha": {"provider": "altcha"},
        "features": {"account_identity": "email"},
        "limits": {"version": 2, "rules": []},
        "app_public": {"branding": {"product_name": "Mock"}},
        "sso": {"enabled": false},
        "domain_migration": {"enabled": false}
    })
}

/// A mock instance and a client of its own token on it.
async fn fixture(token: &str) -> (MockServer, p::Client) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery(&server.uri())))
        .mount(&server)
        .await;
    let info = p::ClientInfo {
        installation_id: "00000000-0000-4000-a000-000000000000".into(),
        ..p::ClientInfo::default()
    };
    let instance = p::Instance::discover_with(&server.uri(), info)
        .await
        .expect("discovery");
    (server, p::Client::new(instance, p::Token::new(token)))
}

async fn patches(server: &MockServer, route: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "PATCH" && r.url.path() == route)
        .collect()
}

fn wish(seq: u64, changes: &[Change]) -> Wish {
    Wish::of(seq, changes)
}

/// The person clicks as fast as they can for half a minute, on every setting in turn:
/// the account settings are written at most five times in any ten seconds, and the
/// last click of each setting is what ends up on the server.
#[test]
fn clicking_as_fast_as_possible_stays_within_the_settings_budget() {
    let t0 = Instant::now();
    let mut c = Coalescer::default();
    let mut writes: Vec<(Duration, Wish)> = Vec::new();
    let themes = [Pref::Dark, Pref::Coal, Pref::Light, Pref::System];
    let mut seq = 0;
    let mut last = (Pref::Dark, false, 0u8);
    // A click every 20 ms for 30 s.
    for i in 0..1_500u64 {
        let at = Duration::from_millis(20 * i);
        let now = t0 + at;
        seq += 1;
        last = (themes[(i % 4) as usize], i % 3 == 0, (i % 3) as u8);
        c.add(
            wish(
                seq,
                &[
                    Change::Theme(last.0),
                    Change::Compact(last.1),
                    Change::RenderSpoilers(last.2),
                ],
            ),
            now,
        );
        while let Next::Write(w) = c.next(now) {
            writes.push((at, w));
        }
    }
    // What still waits goes out as the budget frees up.
    let mut at = Duration::from_secs(30);
    loop {
        match c.next(t0 + at) {
            Next::Idle => break,
            Next::Wait(d) => at += d,
            Next::Write(w) => writes.push((at, w)),
        }
    }
    assert!(!writes.is_empty());
    for (start, _) in &writes {
        let in_window = writes
            .iter()
            .filter(|(t, _)| *t >= *start && *t < *start + Duration::from_secs(10))
            .count();
        assert!(in_window <= 5, "{in_window} writes in ten seconds");
    }
    for pair in writes.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= MIN_GAP,
            "writes {pair:?} too close"
        );
    }
    let (_, final_write) = writes.last().unwrap();
    assert_eq!(
        final_write.through, seq,
        "the last click is in the last write"
    );
    let mut update = p::UserSettingsUpdate::default();
    Change::Theme(last.0).write(&mut update);
    Change::Compact(last.1).write(&mut update);
    Change::RenderSpoilers(last.2).write(&mut update);
    assert_eq!(final_write.update(), &update);
}

#[test]
fn one_change_waits_for_more_and_two_kinds_write_together() {
    let t0 = Instant::now();
    let mut c = Coalescer::default();
    assert_eq!(c.next(t0), Next::Idle);
    c.add(wish(1, &[Change::Theme(Pref::Light)]), t0);
    assert_eq!(c.next(t0), Next::Wait(GATHER));
    c.add(
        wish(2, &[Change::DeveloperMode(true)]),
        t0 + Duration::from_millis(200),
    );
    let Next::Write(w) = c.next(t0 + GATHER) else {
        panic!("due after the gathering time");
    };
    assert_eq!(w.through, 2);
    assert_eq!(w.update().theme.as_deref(), Some("light"));
    assert_eq!(w.update().developer_mode, Some(true));
    // A preference change is not an account change.
    assert!(wish(3, &[Change::FontSize(18)]).is_empty());
}

#[test]
fn a_change_goes_into_the_field_the_server_knows() {
    let mut u = p::UserSettingsUpdate::default();
    for c in [
        Change::Theme(Pref::System),
        Change::Compact(true),
        Change::AnimateStickers(9),
        Change::RenderSpoilers(0),
        Change::DmsFromMembers(false),
        Change::FriendSources(0b1111),
        Change::FontSize(18),
    ] {
        c.write(&mut u);
    }
    assert_eq!(u.theme.as_deref(), Some("system"));
    assert_eq!(u.message_display_compact, Some(true));
    assert_eq!(u.animate_stickers, Some(2), "cut to what the server takes");
    assert_eq!(u.render_spoilers, Some(0));
    assert_eq!(
        u.default_guilds_restricted,
        Some(true),
        "allowing DMs is the reverse of restricting them"
    );
    assert_eq!(u.friend_source_flags, Some(7));
    // The font size is a synced preference, not a field of the update.
    let mut only_font = p::UserSettingsUpdate::default();
    Change::FontSize(18).write(&mut only_font);
    assert_eq!(only_font, p::UserSettingsUpdate::default());

    let mut prefs = p::api::preferences::Preferences::default();
    Change::FontSize(18).set_preference(&mut prefs);
    Change::GroupSpacing(8).set_preference(&mut prefs);
    Change::Theme(Pref::Coal).set_preference(&mut prefs);
    let a = prefs.accessibility.unwrap();
    assert_eq!(a.font_size, Some(18.));
    assert_eq!(a.message_group_spacing, Some(8.));
}

/// Twenty clicks in a burst are one `PATCH` with the newest values; the next burst
/// waits for the gap.
#[tokio::test]
async fn a_burst_of_changes_is_one_write_and_the_next_waits_for_the_gap() {
    let (server, client) = fixture("tok-settings-burst").await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/users/@me/settings"))
        .respond_with(|r: &Request| {
            let mut body: Value = serde_json::from_slice(&r.body).unwrap();
            body["version"] = json!(9);
            ResponseTemplate::new(200).set_body_json(body)
        })
        .mount(&server)
        .await;
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let events = Ui::new(tx);
    let (internal, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut writer = Writer::default();
    let wishes = writer.sender(&client, &events, &internal, 1);

    let started = Instant::now();
    for seq in 1..=20u64 {
        let theme = [Pref::Light, Pref::Coal][(seq % 2) as usize];
        wishes
            .send(wish(
                seq,
                &[Change::Theme(theme), Change::RenderEmbeds(seq % 2 == 0)],
            ))
            .unwrap();
    }
    tokio::time::sleep(GATHER + Duration::from_millis(500)).await;
    let first = patches(&server, "/api/v1/users/@me/settings").await;
    assert_eq!(first.len(), 1, "one write for the whole burst");
    let body: Value = serde_json::from_slice(&first[0].body).unwrap();
    assert_eq!(body["theme"], "light", "the newest value, from command 20");
    assert_eq!(body["render_embeds"], true);

    // Another change at once: it waits for the minimum gap since the write.
    wishes.send(wish(21, &[Change::Compact(true)])).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        patches(&server, "/api/v1/users/@me/settings").await.len(),
        1,
        "still inside the gap"
    );
    let until = started + GATHER + MIN_GAP + Duration::from_millis(900);
    tokio::time::sleep_until(until).await;
    let all = patches(&server, "/api/v1/users/@me/settings").await;
    assert_eq!(all.len(), 2);
    let body: Value = serde_json::from_slice(&all[1].body).unwrap();
    assert_eq!(body, json!({"message_display_compact": true}));

    // The UI is told how far each write got.
    let mut through = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let Event::Settings(super::Ev::AccountSaved { through: t, .. }) = event {
            through.push(t);
        }
    }
    assert_eq!(through, vec![20, 21]);
}

#[tokio::test]
async fn a_failed_write_is_reported_to_the_ui() {
    let (server, client) = fixture("tok-settings-fail").await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/users/@me/settings"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "INVALID_FORM_BODY", "message": "no"
        })))
        .mount(&server)
        .await;
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let events = Ui::new(tx);
    let (internal, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut writer = Writer::default();
    let wishes = writer.sender(&client, &events, &internal, 1);
    wishes
        .send(wish(4, &[Change::DeveloperMode(true)]))
        .unwrap();
    let mut failed = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while failed.is_none() && Instant::now() < deadline {
        if let Ok(Event::Settings(super::Ev::SaveFailed {
            through,
            preferences,
        })) = rx.try_recv()
        {
            failed = Some((through, preferences));
        } else {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    assert_eq!(failed, Some((4, false)));
}

#[test]
fn pictures_are_known_by_their_first_bytes() {
    assert_eq!(
        profile::sniff_image(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]),
        Some("image/png")
    );
    assert_eq!(
        profile::sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]),
        Some("image/jpeg")
    );
    assert_eq!(profile::sniff_image(b"GIF89a.."), Some("image/gif"));
    assert_eq!(
        profile::sniff_image(b"RIFF\0\0\0\0WEBPVP8 "),
        Some("image/webp")
    );
    assert_eq!(profile::sniff_image(b"<svg xmlns='x'/>"), None, "never SVG");
    assert_eq!(
        profile::sniff_image(b"MZ\x90\0"),
        None,
        "a program is no picture"
    );
    assert_eq!(profile::sniff_image(b""), None);
}

#[tokio::test]
async fn a_profile_picture_is_read_only_when_it_fits_and_is_a_picture() {
    let dir = std::env::temp_dir().join(format!("inlet-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("me.png");
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&[0; 32]);
    std::fs::write(&png, &bytes).unwrap();
    assert!(profile::read_image(png.clone()).await.is_ok());

    // A program renamed to .png is refused.
    let fake = dir.join("fake.png");
    std::fs::write(&fake, b"MZ\x90\0\0\0").unwrap();
    assert!(matches!(
        profile::read_image(fake).await,
        Err(profile::ProfileError::Text(_))
    ));

    // One byte over the limit is refused without being read whole.
    let big = dir.join("big.png");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(profile::MAX_IMAGE_BYTES + 1).unwrap();
    assert!(matches!(
        profile::read_image(big).await,
        Err(profile::ProfileError::Text(_))
    ));
    assert!(matches!(
        profile::read_image(dir.join("missing.png")).await,
        Err(profile::ProfileError::Text(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_profile_edit_sends_only_what_changed() {
    let edit = profile::edit_of(ProfileSave {
        name: Some("  ".into()),
        bio: Some("hello".into()),
        avatar: ImageChange::Remove,
        ..ProfileSave::default()
    })
    .await
    .ok()
    .unwrap();
    assert_eq!(edit.global_name, Some(None), "an empty name clears it");
    assert_eq!(edit.bio, Some(Some("hello".into())));
    assert_eq!(edit.avatar, Some(None));
    assert_eq!(edit.pronouns, None, "left as it is");
    assert_eq!(edit.banner, None);

    let long = profile::edit_of(ProfileSave {
        name: Some("n".repeat(100)),
        pronouns: Some("p".repeat(100)),
        ..ProfileSave::default()
    })
    .await
    .ok()
    .unwrap();
    assert_eq!(long.global_name.unwrap().unwrap().chars().count(), 32);
    assert_eq!(long.pronouns.unwrap().unwrap().chars().count(), 40);
}

#[tokio::test]
async fn the_own_profile_is_read_with_its_texts_cleaned() {
    let (server, client) = fixture("tok-settings-profile").await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "7", "username": "ada", "discriminator": "0"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/7/profile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user": {"id": "7", "username": "ada", "global_name": "Ada\u{202E}\u{7}",
                     "avatar": "abc123"},
            "user_profile": {"bio": "line one\nline two", "pronouns": "she/her",
                             "banner": "bannerhash"}
        })))
        .mount(&server)
        .await;
    let own = profile::load(&client).await.ok().unwrap();
    assert_eq!(own.name, "Ada", "control and direction characters go");
    // Joiners stay, so an emoji sequence or a Persian name is not rewritten on save.
    let joined = profile::own_profile(
        &serde_json::from_value(json!({
            "user": {"id": "7", "username": "ada",
                     "global_name": "\u{1F469}\u{200D}\u{1F4BB}\u{200C}\nx"},
            "user_profile": {"bio": "a\u{200B}b\nc", "pronouns": "x\ny"}
        }))
        .unwrap(),
    );
    assert_eq!(
        joined.name, "\u{1F469}\u{200D}\u{1F4BB}\u{200C}x",
        "no line break in a name"
    );
    assert_eq!(joined.bio, "ab\nc", "a bio keeps its line breaks");
    assert_eq!(joined.pronouns, "xy");
    assert_eq!(own.bio, "line one\nline two", "a bio keeps its line breaks");
    assert_eq!(own.pronouns, "she/her");
    assert_eq!(own.banner.as_deref(), Some("bannerhash"));
    assert_eq!(own.avatar.as_ref().map(|a| a.hash.as_str()), Some("abc123"));
}
