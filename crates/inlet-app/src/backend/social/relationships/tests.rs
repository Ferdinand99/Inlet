// SPDX-License-Identifier: AGPL-3.0-or-later
//! The relationship and group DM budget, and the CAPTCHA and rate-limit answers, against
//! a mock instance.

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use super::*;
use crate::backend::accounts::testkit::{mock_instance, requests};

fn key(user: &str) -> AccountKey {
    AccountKey::new("https://mock.test", user).unwrap()
}

#[test]
fn the_worst_clicking_stays_within_the_budget() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    // 100 clicks on 100 people within 5 s: at most 10 calls start in any 10 s.
    let mut started = Vec::new();
    for i in 0..100u64 {
        let now = t0 + Duration::from_millis(50 * i);
        if g.admit(&format!("user:{i}"), Budget::Write, now).is_ok() {
            started.push(now);
            g.finish(&format!("user:{i}"));
        }
    }
    assert_eq!(started.len(), WRITES_PER_WINDOW);
    // The same person twice while the first call runs: one call.
    let later = t0 + WRITE_WINDOW * 2;
    assert!(g.admit("user:1", Budget::Write, later).is_ok());
    assert_eq!(g.admit("user:1", Budget::Write, later), Err(Refusal::Busy));
    g.finish("user:1");
    // Group DM creates: three an hour, whatever the clicks.
    let mut creates = 0;
    for i in 0..60u64 {
        let now = later + Duration::from_secs(60 * i);
        if g.admit("group-dm", Budget::GroupCreate, now).is_ok() {
            creates += 1;
            g.finish("group-dm");
        }
    }
    assert_eq!(creates, GROUP_CALLS_PER_HOUR);
    // Adds have their own three.
    let now = later + Duration::from_secs(10);
    for _ in 0..GROUP_CALLS_PER_HOUR {
        assert!(g.admit("group:5", Budget::GroupAdd(1), now).is_ok());
        g.finish("group:5");
    }
    match g.admit("group:5", Budget::GroupAdd(1), now) {
        Err(Refusal::GroupLimit { wait }) => assert!(wait > Duration::from_secs(3500)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn group_dm_create_handles_captcha_result() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    // A challenge Inlet cannot solve (an algorithm it does not know).
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/channels"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
            "captcha_provider": "altcha",
            "altcha_challenge": {"parameters": {
                "algorithm": "ARGON2ID", "cost": 1, "keyLength": 32, "keyPrefix": "a1b2c3",
                "nonce": "00112233445566778899aabbccddeeff",
                "salt": "00112233445566778899aabbccddeeff", "expiresAt": 4_102_444_800u64
            }, "signature": "x"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-captcha"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let account = key("1");
    let cmd = Cmd::CreateGroupDm {
        recipients: vec!["11".into(), "12".into()],
    };
    let ev = run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await;
    assert_eq!(
        ev,
        Ev::GroupDm {
            key: "group-dm".into(),
            result: GroupDm::Captcha
        }
    );
    // One request, and nothing is tried again on its own.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let posts = requests(&server)
        .await
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(posts, 1);

    // Over Fluxer's hourly limit: the real wait, and no more group calls until it ends.
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/channels"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "code": "RATE_LIMITED", "message": "slow", "global": false, "retry_after": 3000
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-limit"));
    let account = key("2");
    let ev = run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await;
    assert_eq!(
        ev,
        Ev::GroupDm {
            key: "group-dm".into(),
            result: GroupDm::RateLimited {
                retry_after: Duration::from_secs(3000)
            }
        }
    );
    assert_eq!(
        wait_text(Duration::from_secs(3000)),
        t!("social-wait-minutes", count = 50)
    );
    match with_gate(&account, |g| {
        g.admit("group-dm", Budget::GroupCreate, Instant::now())
    }) {
        Err(Refusal::GroupLimit { wait }) => assert!(wait > Duration::from_secs(2900)),
        other => panic!("{other:?}"),
    }
    // Other writes still go.
    assert!(with_gate(&account, |g| g.admit(
        "user:5",
        Budget::Write,
        Instant::now()
    ))
    .is_ok());
}

#[test]
fn a_call_that_never_answers_frees_its_target_and_fails() {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let ui = crate::backend::Ui::new(tx);
    let account = key("3");
    assert!(with_gate(&account, |g| g.admit(
        "user:9",
        Budget::Write,
        Instant::now()
    ))
    .is_ok());
    // The task panics or is aborted before it answers.
    drop(Call {
        account: account.clone(),
        key: "user:9".into(),
        events: ui,
        hint: None,
        answered: false,
    });
    assert!(with_gate(&account, |g| g.admit(
        "user:9",
        Budget::Write,
        Instant::now()
    ))
    .is_ok());
    match rx.try_recv() {
        Ok(crate::backend::Event::Social(super::super::Ev::Relationships(Ev::Failed {
            key,
            ..
        }))) => assert_eq!(key, "user:9"),
        _ => panic!("the UI is answered"),
    }
}

#[test]
fn a_huge_server_wait_is_capped() {
    let mut g = Gate::default();
    let now = Instant::now();
    g.group_rate_limited(now, Duration::MAX);
    match g.admit("group-dm", Budget::GroupCreate, now) {
        Err(Refusal::GroupLimit { wait }) => assert_eq!(wait, MAX_GROUP_WAIT),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn closing_or_editing_a_channel_that_is_not_a_dm_sends_nothing() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    let client = p::Client::new(instance, p::Token::new("tok-guard"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let account = key("4");
    // A guild text channel as the state knows it: the protocol's check refuses it.
    let text: p::Channel = serde_json::from_value(json!({
        "id": "900", "type": 0, "guild_id": "1", "name": "general"
    }))
    .unwrap();
    for cmd in [
        Cmd::CloseDm {
            channel: Box::new(text.clone()),
        },
        Cmd::EditGroupDm {
            channel: Box::new(text.clone()),
            change: GroupChange {
                name: Some(Some("renamed".into())),
                ..Default::default()
            },
        },
    ] {
        match run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await {
            Ev::Failed { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    let writes = requests(&server)
        .await
        .into_iter()
        .filter(|r| matches!(r.method.as_str(), "DELETE" | "PATCH"))
        .count();
    assert_eq!(writes, 0, "nothing reaches the server");
}

#[test]
fn friend_requests_stay_within_ten_a_minute() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    let sent = (0..30u64)
        .filter(|i| {
            let ok = g
                .admit(
                    "add-friend",
                    Budget::FriendRequest,
                    t0 + Duration::from_secs(2 * i),
                )
                .is_ok();
            g.finish("add-friend");
            ok
        })
        .count();
    assert_eq!(sent, FRIEND_REQUESTS_PER_MINUTE, "30 tries in a minute");
}

#[test]
fn adding_several_people_takes_that_many_of_the_hourly_slots_or_none() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    // Two people use two of three; a second call for two is refused whole.
    assert!(g.admit("group:7", Budget::GroupAdd(2), t0).is_ok());
    g.finish("group:7");
    match g.admit("group:7", Budget::GroupAdd(2), t0 + Duration::from_secs(60)) {
        Err(Refusal::GroupLimit { wait }) => {
            assert!(wait > Duration::from_secs(3000), "{wait:?}");
        }
        other => panic!("{other:?}"),
    }
    // The refusal took nothing: one more person still fits.
    assert!(g
        .admit("group:7", Budget::GroupAdd(1), t0 + Duration::from_secs(61))
        .is_ok());
    g.finish("group:7");
    // More people than the hour holds never fit, and the wait is at most an hour.
    match g.admit(
        "group:8",
        Budget::GroupAdd(GROUP_CALLS_PER_HOUR + 1),
        t0 + GROUP_WINDOW * 2,
    ) {
        Err(Refusal::GroupLimit { wait }) => assert!(wait <= GROUP_WINDOW),
        other => panic!("{other:?}"),
    }
    // After the hour the slots are back.
    assert!(g
        .admit("group:7", Budget::GroupAdd(3), t0 + GROUP_WINDOW * 3)
        .is_ok());
}

#[test]
fn only_static_pictures_are_group_icons() {
    // A GIF: header, 1x1 screen without a colour table, then image blocks.
    let gif = |frames: usize| {
        let mut b = b"GIF89a".to_vec();
        b.extend([1, 0, 1, 0, 0, 0, 0]);
        for _ in 0..frames {
            // A graphic control extension, then a 1x1 image with 1 byte of data.
            b.extend([0x21, 0xF9, 4, 0, 0, 0, 0, 0]);
            b.extend([0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
            b.extend([2, 1, 0x44, 0]);
        }
        b.push(0x3B);
        b
    };
    assert!(!is_animated(&gif(1), "image/gif"), "one frame is a picture");
    assert!(is_animated(&gif(2), "image/gif"));
    assert!(
        !is_animated(b"GIF89a", "image/gif"),
        "a cut file is not followed"
    );

    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend(kind);
        c.extend(data);
        c.extend([0, 0, 0, 0]);
        c
    };
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(chunk(b"IHDR", &[0; 13]));
    let still = [png.clone(), chunk(b"IDAT", &[1]), chunk(b"IEND", &[])].concat();
    let apng = [
        png.clone(),
        chunk(b"acTL", &[0; 8]),
        chunk(b"IDAT", &[1]),
        chunk(b"IEND", &[]),
    ]
    .concat();
    assert!(!is_animated(&still, "image/png"));
    assert!(is_animated(&apng, "image/png"));

    let webp = |chunks: &[(&[u8; 4], Vec<u8>)]| {
        let mut body = b"WEBP".to_vec();
        for (kind, data) in chunks {
            body.extend(*kind);
            body.extend((data.len() as u32).to_le_bytes());
            body.extend(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut b = b"RIFF".to_vec();
        b.extend((body.len() as u32).to_le_bytes());
        b.extend(body);
        b
    };
    assert!(!is_animated(
        &webp(&[
            (b"VP8X", vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (b"VP8 ", vec![1])
        ]),
        "image/webp"
    ));
    assert!(is_animated(
        &webp(&[(b"VP8X", vec![0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0])]),
        "image/webp"
    ));
    assert!(is_animated(&webp(&[(b"ANMF", vec![0; 4])]), "image/webp"));
    assert!(!is_animated(&[0xFF, 0xD8, 0xFF], "image/jpeg"));
}

#[test]
fn nicknames_are_trimmed_cleared_and_limited_to_32_characters() {
    assert_eq!(clean_nickname("  Cap  "), Ok(Some("Cap".into())));
    assert_eq!(clean_nickname("   "), Ok(None));
    assert_eq!(clean_nickname(""), Ok(None));
    assert!(clean_nickname(&"x".repeat(32)).is_ok());
    assert!(clean_nickname(&"x".repeat(33)).is_err());
    assert!(clean_nickname("a\u{7}b").is_err());
}

#[tokio::test]
async fn a_friend_nickname_is_sent_trimmed_and_cleared_when_blank() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/users/@me/relationships/11"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "11", "type": 1, "nickname": null,
            "user": {"id": "11", "username": "bo", "discriminator": "0001"}
        })))
        .expect(2)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-nick"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let account = key("5");
    for nickname in [Some("  Bo B ".to_owned()), Some("   ".to_owned())] {
        let cmd = Cmd::SetFriendNickname {
            user_id: "11".into(),
            nickname,
        };
        let ev = run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await;
        assert_eq!(
            ev,
            Ev::Done {
                key: "nick:11".into()
            }
        );
    }
    let bodies: Vec<serde_json::Value> = requests(&server)
        .await
        .into_iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0], json!({"nickname": "Bo B"}));
    assert_eq!(bodies[1], json!({"nickname": null}), "blank clears");

    // Too long: refused with a sentence, nothing sent.
    let cmd = Cmd::SetFriendNickname {
        user_id: "11".into(),
        nickname: Some("x".repeat(33)),
    };
    match run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await {
        Ev::Failed { reason, .. } => assert_eq!(reason, t!("social-nickname-invalid")),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        requests(&server)
            .await
            .into_iter()
            .filter(|r| r.method.as_str() == "PATCH")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_group_save_sends_nicknames_the_mature_switch_and_the_new_owner_in_one_patch() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/channels/500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "500", "type": 3, "recipients": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-group-save"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let account = key("6");
    let group: p::Channel =
        serde_json::from_value(json!({"id": "500", "type": 3, "owner_id": "1"})).unwrap();
    let cmd = Cmd::EditGroupDm {
        channel: Box::new(group),
        change: GroupChange {
            nsfw: Some(true),
            owner_id: Some("12".into()),
            nicks: vec![("11".into(), Some("Cap".into())), ("12".into(), None)],
            ..Default::default()
        },
    };
    let ev = run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await;
    assert_eq!(
        ev,
        Ev::Done {
            key: "group:500".into()
        }
    );
    let body: serde_json::Value = serde_json::from_slice(
        &requests(&server)
            .await
            .into_iter()
            .find(|r| r.method.as_str() == "PATCH")
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(
        body,
        json!({"owner_id": "12", "nicks": {"11": "Cap", "12": null}, "nsfw": true})
    );
}

#[tokio::test]
async fn group_invites_are_listed_made_with_a_day_and_revoked() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    let invite = |code: &str| {
        json!({
            "code": code, "type": 1, "member_count": 2, "temporary": false,
            "channel": {"id": "500", "name": null, "type": 3},
            "inviter": {"id": "10", "username": "ana", "discriminator": "0001", "global_name": "Ana"},
            "expires_at": "2026-11-17T08:00:00.000Z", "created_at": "2026-11-16T08:00:00.000Z",
            "uses": 1, "max_uses": 0, "max_age": 86400
        })
    };
    Mock::given(method("GET"))
        .and(path("/api/v1/channels/500/invites"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([invite("aaaa1111")])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/channels/500/invites"))
        .respond_with(ResponseTemplate::new(200).set_body_json(invite("bbbb2222")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/invites/aaaa1111"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-group-invites"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let account = key("7");
    let go = |cmd: Cmd| {
        let (client, internal, account) = (client.clone(), internal.clone(), account.clone());
        async move { run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await }
    };

    match go(Cmd::ListGroupInvites {
        channel_id: "500".into(),
    })
    .await
    {
        Ev::Invites { key, result, .. } => {
            assert_eq!(key, "invites:500");
            let list = result.unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].code, "aaaa1111");
            assert_eq!(list[0].inviter, Some(("10".into(), "Ana".into())));
            assert_eq!((list[0].uses, list[0].max_uses), (1, 0));
            assert!(list[0].url.ends_with("/invite/aaaa1111"), "{}", list[0].url);
            // 2026-11-16 08:00 UTC, and a day later.
            assert_eq!(list[0].created_ms, Some(1_794_816_000_000));
            assert_eq!(list[0].expires_ms, Some(1_794_902_400_000));
        }
        other => panic!("{other:?}"),
    }
    match go(Cmd::CreateGroupInvite {
        channel_id: "500".into(),
    })
    .await
    {
        Ev::InviteLink { key, url } => {
            assert_eq!(key, "invite-link:500");
            assert!(url.ends_with("/invite/bbbb2222"));
        }
        other => panic!("{other:?}"),
    }
    let made: serde_json::Value = serde_json::from_slice(
        &requests(&server)
            .await
            .into_iter()
            .find(|r| r.method.as_str() == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(
        made,
        json!({"max_age": 86400}),
        "the invite for a friend lasts a day"
    );
    assert_eq!(
        go(Cmd::RevokeInvite {
            channel_id: "500".into(),
            code: "aaaa1111".into()
        })
        .await,
        Ev::Done {
            key: "invite-revoke:500:aaaa1111".into()
        }
    );
}

#[tokio::test]
async fn an_invite_to_a_community_is_made_and_sent_in_the_dm() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/channels/30/invites"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "cccc3333", "type": 0, "temporary": false,
            "guild": {"id": "3", "name": "Garden"}, "channel": {"id": "30", "type": 0}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "600", "type": 1, "recipients": [{"id": "11", "username": "bo"}]
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/channels/600/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "700", "channel_id": "600", "type": 0, "content": "x",
            "author": {"id": "1", "username": "me"}, "timestamp": "2026-11-16T08:00:00.000Z"
        })))
        .expect(2)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-community"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let account = key("8");
    let invite_to = |vanity: Option<&str>| Cmd::InviteToCommunity {
        user_id: "11".into(),
        guild_id: "3".into(),
        channel_id: "30".into(),
        vanity: vanity.map(str::to_owned),
    };
    // A guild the account may invite to: a new invite is made. Otherwise the vanity link.
    for cmd in [invite_to(None), invite_to(Some("garden"))] {
        let ev = run(&client, &internal, 1, &account, &cmd.key(), cmd.clone()).await;
        assert_eq!(
            ev,
            Ev::Done {
                key: "community:11:3".into()
            }
        );
    }
    let sent: Vec<String> = requests(&server)
        .await
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/channels/600/messages")
        .map(|r| {
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["content"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(sent[0].ends_with("/invite/cccc3333"), "{sent:?}");
    assert!(sent[1].ends_with("/invite/garden"), "{sent:?}");
    let invites_made = requests(&server)
        .await
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/channels/30/invites")
        .count();
    assert_eq!(invites_made, 1, "the vanity link makes no invite");
}

#[tokio::test]
async fn people_who_cannot_be_added_come_back_with_the_ones_who_can() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/channels"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "GROUP_DM_RECIPIENTS_NOT_ADDABLE", "message": "no",
            "unaddable_recipients": [{"user_id": "12", "reason": "not_friends"}],
            "addable_recipients": ["11"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-unaddable"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let cmd = Cmd::CreateGroupDm {
        recipients: vec!["11".into(), "12".into()],
    };
    let ev = run(&client, &internal, 1, &key("9"), &cmd.key(), cmd.clone()).await;
    assert_eq!(
        ev,
        Ev::GroupDm {
            key: "group-dm".into(),
            result: GroupDm::NotAddable {
                unaddable: vec![("12".into(), "not_friends".into())],
                addable: vec!["11".into()],
            }
        }
    );
}

#[tokio::test]
async fn adding_people_stops_at_the_first_one_that_does_not_go_through() {
    let (server, instance) = mock_instance("ws://127.0.0.1:9", &[]).await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/channels/500/recipients/11"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/channels/500/recipients/12"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "MAX_GROUP_DM_RECIPIENTS", "message": "full"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = p::Client::new(instance, p::Token::new("tok-add-many"));
    let (internal, _irx) = tokio::sync::mpsc::unbounded_channel();
    let cmd = Cmd::AddRecipients {
        channel_id: "500".into(),
        user_ids: vec!["11".into(), "12".into(), "13".into()],
    };
    let ev = run(&client, &internal, 1, &key("10"), &cmd.key(), cmd.clone()).await;
    // The first person went in before the second one was refused: the answer says so.
    assert_eq!(
        ev,
        Ev::GroupDm {
            key: "group:500".into(),
            result: GroupDm::Partial {
                added: vec!["11".into()],
                rest: Box::new(GroupDm::Failed(t!("social-group-full"))),
            }
        }
    );
    let puts: Vec<String> = requests(&server)
        .await
        .into_iter()
        .filter(|r| r.method.as_str() == "PUT")
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(puts.len(), 2, "the third person is never asked: {puts:?}");
}

#[tokio::test(start_paused = true)]
async fn a_group_call_that_runs_long_says_it_is_still_working() {
    let account = key("11");
    assert!(with_gate(&account, |g| g.admit(
        "group-dm",
        Budget::GroupCreate,
        Instant::now()
    ))
    .is_ok());
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let ui = crate::backend::Ui::new(tx);
    let hint = tokio::spawn(slow_hint(ui, account.clone(), "group-dm".to_owned()));
    // Nothing before the moment is up.
    tokio::time::sleep(SLOW_HINT - Duration::from_millis(10)).await;
    assert!(rx.try_recv().is_err(), "not yet");
    tokio::time::sleep(Duration::from_millis(20)).await;
    hint.await.unwrap();
    match rx.try_recv() {
        Ok(crate::backend::Event::Social(super::super::Ev::Relationships(Ev::Working { key }))) => {
            assert_eq!(key, "group-dm")
        }
        _ => panic!("the window is told"),
    }
    // A call that has ended is not reported.
    with_gate(&account, |g| g.finish("group-dm"));
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    slow_hint(crate::backend::Ui::new(tx), account, "group-dm".to_owned()).await;
    assert!(rx.try_recv().is_err(), "done calls say nothing");
}

#[test]
fn a_revoke_and_a_list_of_the_same_group_do_not_block_each_other() {
    let list = Cmd::ListGroupInvites {
        channel_id: "500".into(),
    };
    let revoke = Cmd::RevokeInvite {
        channel_id: "500".into(),
        code: "aaaa1111".into(),
    };
    assert_ne!(list.key(), revoke.key());
    let now = Instant::now();
    let mut g = Gate::default();
    assert!(g.admit(&list.key(), list.budget(), now).is_ok());
    assert!(
        g.admit(&revoke.key(), revoke.budget(), now).is_ok(),
        "the revoke goes while the list is read"
    );
}

#[tokio::test(start_paused = true)]
async fn the_still_working_timer_ends_with_its_call() {
    let account = key("12");
    assert!(with_gate(&account, |g| g.admit(
        "group-dm",
        Budget::GroupCreate,
        Instant::now()
    ))
    .is_ok());
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let hint = tokio::spawn(slow_hint(
        crate::backend::Ui::new(tx.clone()),
        account.clone(),
        "group-dm".to_owned(),
    ));
    let mut call = Call {
        account: account.clone(),
        key: "group-dm".into(),
        events: crate::backend::Ui::new(tx),
        hint: Some(hint.abort_handle()),
        answered: false,
    };
    // The call answers at once; a new call with the same key starts at once too.
    call.answer(Ev::Done {
        key: "group-dm".into(),
    });
    assert!(with_gate(&account, |g| g.admit(
        "group-dm",
        Budget::GroupCreate,
        Instant::now()
    ))
    .is_ok());
    assert!(hint.await.unwrap_err().is_cancelled(), "the timer is gone");
    tokio::time::sleep(SLOW_HINT * 2).await;
    let mut said = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        said.push(ev);
    }
    assert_eq!(said.len(), 1, "only the call's own answer, no hint");
}
