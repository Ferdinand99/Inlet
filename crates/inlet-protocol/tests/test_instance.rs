//! Against the private test instance `fluxer.test` (LAN only, private test CA).
//! Needs the feature `test-extra-ca`, `INLET_TEST_EXTRA_CA_FILE` pointing at
//! the instance's `ca.crt`, and a name mapping (`--add-host
//! fluxer.test:10.0.3.196` in Docker). Ignored by default; run with
//! `cargo test -p inlet-protocol --features test-extra-ca --test test_instance -- --ignored --nocapture`.
//!
//! Two requests: discovery, and one gateway connection that reads Hello. No
//! sign-in.
#![cfg(feature = "test-extra-ca")]

use std::time::Duration;

use futures_util::StreamExt;
use inlet_protocol::gateway::{codec::ZstdStream, gateway_url, tls_connector};
use inlet_protocol::Instance;
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn origin() -> String {
    std::env::var("INLET_TEST_ORIGIN").unwrap_or_else(|_| "fluxer.test".into())
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn test_instance_discovery_and_gateway_hello() {
    assert!(
        std::env::var_os(inlet_protocol::test_ca::ENV).is_some(),
        "set {} to the test CA",
        inlet_protocol::test_ca::ENV
    );
    let instance = Instance::discover(&origin()).await.expect("discovery");
    println!("api base     {}", instance.api_base());
    println!("gateway      {}", instance.gateway_url());
    assert!(instance.api_base().starts_with("https://"));
    assert!(instance.gateway_url().starts_with("wss://"));
    println!(
        "media        {:?}",
        instance.media().map(|m| m.base().to_owned())
    );

    let url = gateway_url(instance.gateway_url(), true).unwrap();
    let req = url.as_str().into_client_request().unwrap();
    let (mut ws, _) =
        tokio_tungstenite::connect_async_tls_with_config(req, None, true, tls_connector())
            .await
            .expect("gateway connect with the extra CA");
    let mut codec = ZstdStream::new().unwrap();
    let hello: Value = loop {
        let msg = tokio::time::timeout(Duration::from_secs(15), ws.next())
            .await
            .expect("Hello in time")
            .expect("socket open")
            .expect("frame");
        match msg {
            WsMessage::Binary(b) => {
                break serde_json::from_slice(&codec.decompress(&b).unwrap()).unwrap()
            }
            WsMessage::Text(t) => break serde_json::from_str(t.as_str()).unwrap(),
            WsMessage::Close(f) => panic!("closed: {f:?}"),
            _ => continue,
        }
    };
    println!("gateway      Hello: {hello}");
    assert_eq!(hello["op"], 10);
    let _ = ws.close(None).await;
}

/// The group DM, nickname and invite routes exist on the instance, and each one needs a
/// token: a made-up token (never a real account's) is refused with 401, not 404.
#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn test_instance_group_invite_and_nickname_routes_need_a_token() {
    use inlet_protocol::api::invites::InviteParams;
    use inlet_protocol::{Channel, Client, Error, GroupDmEdit, Token};

    let instance = Instance::discover(&origin()).await.expect("discovery");
    let client = Client::new(instance.clone(), Token::new("not-a-real-token"));
    let group: Channel =
        serde_json::from_value(serde_json::json!({"id": "100", "type": 3})).unwrap();
    let edit = GroupDmEdit {
        nsfw: Some(false),
        ..Default::default()
    };
    let refused = |what: &str, result: Result<(), Error>| match result {
        Err(Error::Unauthorized) => println!("{what}: 401 as it should"),
        other => panic!("{what}: {other:?}"),
    };
    refused(
        "create invite",
        client
            .create_invite("100", InviteParams::default())
            .await
            .map(drop),
    );
    refused(
        "list invites",
        client.channel_invites("100").await.map(drop),
    );
    refused("revoke invite", client.delete_invite("aB3dEf7h").await);
    refused(
        "friend nickname",
        client.set_friend_nickname("100", Some("x")).await.map(drop),
    );
    refused(
        "edit group",
        client.edit_group_dm(&group, &edit).await.map(drop),
    );
    refused("empty group", client.create_group_dm(&[]).await.map(|_| ()));
    println!(
        "invite link: {:?}",
        inlet_protocol::api::invites::invite_url(&instance, "aB3dEf7h")
    );
}
