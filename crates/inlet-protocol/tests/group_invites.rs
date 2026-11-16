//! Group DM nicknames, the mature switch, ownership, invites (create, list,
//! revoke), the invite link and a friend's private nickname: paths, bodies and
//! answers against a mock instance.

mod common;

use std::collections::BTreeMap;

use inlet_protocol::api::invites::{invite_url, InviteParams, MAX_INVITE_AGE_SECONDS};
use inlet_protocol::{Channel, Client, Error, GroupDmEdit, Instance, Token};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn channel(id: &str, kind: i64) -> Channel {
    serde_json::from_value(json!({"id": id, "type": kind})).unwrap()
}

fn client(instance: &Instance, token: &str) -> Client {
    Client::new(instance.clone(), Token::new(token))
}

async fn mount(
    server: &MockServer,
    m: &str,
    p: &str,
    body: Option<Value>,
    reply: ResponseTemplate,
) {
    let mut builder = Mock::given(method(m)).and(path(p));
    if let Some(b) = body {
        builder = builder.and(body_json(b));
    }
    builder.respond_with(reply).expect(1).mount(server).await;
}

fn count(requests: &[Request], p: &str) -> usize {
    requests.iter().filter(|r| r.url.path() == p).count()
}

fn invite(code: &str, uses: u64) -> Value {
    json!({
        "code": code, "type": 1,
        "channel": {"id": "500", "name": null, "type": 3},
        "inviter": common::user_json("10", "ana"),
        "member_count": 3, "expires_at": "2026-11-17T08:00:00.000Z", "temporary": false,
        "created_at": "2026-11-16T08:00:00.000Z", "uses": uses, "max_uses": 0, "max_age": 86400
    })
}

#[tokio::test]
async fn group_nicknames_the_mature_switch_and_ownership_are_one_patch() {
    let (server, instance) = common::instance().await;
    let c = client(&instance, "tok-group-edit");
    let group = channel("500", 3);
    let updated = json!({"id": "500", "type": 3, "name": null, "owner_id": "11", "nsfw": true,
        "nicks": {"10": "Cap"}, "recipients": []});
    mount(
        &server,
        "PATCH",
        "/api/v1/channels/500",
        Some(json!({"nicks": {"10": "Cap", "12": null}, "nsfw": true, "owner_id": "11"})),
        ResponseTemplate::new(200).set_body_json(updated),
    )
    .await;
    let edit = GroupDmEdit {
        nicks: BTreeMap::from([
            ("10".to_owned(), Some(" Cap ".to_owned())),
            ("12".to_owned(), Some("  ".to_owned())),
        ]),
        nsfw: Some(true),
        owner_id: Some("11".into()),
        ..Default::default()
    };
    let channel = c.edit_group_dm(&group, &edit).await.unwrap();
    assert_eq!(channel.group_owner_id(), Some("11"));
    assert_eq!(channel.group_nick("10"), Some("Cap"));
    assert!(channel.group_mature());

    // A bad nickname never reaches the server.
    let too_long = GroupDmEdit {
        nicks: BTreeMap::from([("10".to_owned(), Some("x".repeat(33)))]),
        ..Default::default()
    };
    assert!(matches!(
        c.edit_group_dm(&group, &too_long).await,
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        count(
            &server.received_requests().await.unwrap(),
            "/api/v1/channels/500"
        ),
        1
    );
}

#[tokio::test]
async fn a_friends_private_nickname_is_set_and_cleared() {
    let (server, instance) = common::instance().await;
    let c = client(&instance, "tok-friend-nick");
    let friend = |nick: Value| json!({"id": "11", "type": 1, "user": common::user_json("11", "bo"), "nickname": nick});
    mount(
        &server,
        "PATCH",
        "/api/v1/users/@me/relationships/11",
        Some(json!({"nickname": "Bo B"})),
        ResponseTemplate::new(200).set_body_json(friend(json!("Bo B"))),
    )
    .await;
    assert_eq!(
        c.set_friend_nickname("11", Some("Bo B"))
            .await
            .unwrap()
            .nickname
            .as_deref(),
        Some("Bo B")
    );
    server.reset().await;
    mount(
        &server,
        "PATCH",
        "/api/v1/users/@me/relationships/11",
        Some(json!({"nickname": null})),
        ResponseTemplate::new(200).set_body_json(friend(Value::Null)),
    )
    .await;
    assert_eq!(
        c.set_friend_nickname("11", None).await.unwrap().nickname,
        None
    );
}

#[tokio::test]
async fn group_invites_are_created_listed_and_revoked() {
    let (server, instance) = common::instance().await;
    let c = client(&instance, "tok-group-invites");
    mount(
        &server,
        "POST",
        "/api/v1/channels/500/invites",
        Some(json!({"max_age": 86_400})),
        ResponseTemplate::new(200).set_body_json(invite("aB3dEf7h", 0)),
    )
    .await;
    mount(
        &server,
        "GET",
        "/api/v1/channels/500/invites",
        None,
        ResponseTemplate::new(200).set_body_json(json!([invite("aB3dEf7h", 2), {"nope": 1}])),
    )
    .await;
    mount(
        &server,
        "DELETE",
        "/api/v1/invites/aB3dEf7h",
        None,
        ResponseTemplate::new(204),
    )
    .await;

    let made = c
        .create_invite(
            "500",
            InviteParams {
                max_age: Some(86_400),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(made.code, "aB3dEf7h");
    assert_eq!(made.max_age, Some(86_400));
    assert_eq!(
        invite_url(&instance, &made.code).unwrap(),
        format!("{}/invite/aB3dEf7h", server.uri())
    );

    let list = c.channel_invites("500").await.unwrap();
    assert_eq!(list.len(), 1, "an entry that is no invite is skipped");
    assert_eq!(list[0].uses, Some(2));
    assert_eq!(list[0].max_uses, Some(0));
    assert_eq!(
        list[0].created_at.as_deref(),
        Some("2026-11-16T08:00:00.000Z")
    );
    assert_eq!(list[0].inviter.as_ref().unwrap().id, "10");

    c.delete_invite("aB3dEf7h").await.unwrap();
}

#[tokio::test]
async fn invite_calls_check_their_input_before_any_request() {
    let (server, instance) = common::instance().await;
    let c = client(&instance, "tok-invite-input");
    assert!(matches!(
        c.create_invite("../x", InviteParams::default()).await,
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        c.create_invite(
            "500",
            InviteParams {
                max_age: Some(MAX_INVITE_AGE_SECONDS + 1),
                ..Default::default()
            }
        )
        .await,
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        c.create_invite(
            "500",
            InviteParams {
                max_uses: Some(101),
                ..Default::default()
            }
        )
        .await,
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        c.channel_invites("a/b").await,
        Err(Error::InvalidInput(_))
    ));
    for bad in ["", "../users/@me", "a?b", "a b"] {
        assert!(matches!(
            c.delete_invite(bad).await,
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(invite_url(&instance, bad), None, "{bad:?}");
    }
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| r.url.path() == "/.well-known/fluxer"),
        "nothing but discovery was sent"
    );
}

#[tokio::test]
async fn an_empty_group_is_created_without_recipients() {
    let (server, instance) = common::instance().await;
    mount(
        &server,
        "POST",
        "/api/v1/users/@me/channels",
        Some(json!({"recipients": []})),
        ResponseTemplate::new(200).set_body_json(json!({"id": "501", "type": 3, "recipients": []})),
    )
    .await;
    let c = client(&instance, "tok-empty-group");
    match c.create_group_dm(&[]).await.unwrap() {
        inlet_protocol::GroupDmOutcome::Done(channel) => assert_eq!(channel.id, "501"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_instance_without_an_invite_base_has_no_invite_links() {
    let server = MockServer::start().await;
    let mut discovery = common::discovery_json(&server.uri(), "ws://127.0.0.1:1/gateway");
    discovery["endpoints"]
        .as_object_mut()
        .unwrap()
        .remove("invite");
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
        .mount(&server)
        .await;
    let instance = Instance::discover_with(&server.uri(), inlet_protocol::ClientInfo::default())
        .await
        .unwrap();
    assert_eq!(invite_url(&instance, "aB3dEf7h"), None);
}
