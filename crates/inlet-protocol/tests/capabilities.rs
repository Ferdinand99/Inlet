//! The calls behind the capability and banner features against a mock instance:
//! experiments with `ETag`, discovery revalidation without a new instance, the
//! verification email and the terms.

mod common;

use inlet_protocol::{Client, Error, ExperimentsAnswer, Token};
use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, ResponseTemplate};

fn assignments() -> serde_json::Value {
    json!({
        "poll_interval_seconds": 300,
        "poll_jitter_percent": 15,
        "assignments": {
            "channel_threads": {"active": true, "config_version": 2},
            "guild_visits": {"enabled": true},
            "domain_migration": {"enabled": false}
        }
    })
}

#[tokio::test]
async fn experiments_are_fetched_and_revalidated_with_their_etag() {
    let (server, instance) = common::instance().await;
    let c = Client::new(instance, Token::new("tok-experiments"));
    // A request that carries the ETag gets 304; the first one gets the document.
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .and(header("If-None-Match", "\"e1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .and(header_exists("Authorization"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"e1\"")
                .set_body_json(assignments()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let first = c.experiments(None).await.unwrap();
    let ExperimentsAnswer::Fresh(e, etag) = first else {
        panic!("expected a document, got {first:?}");
    };
    assert_eq!(etag.as_deref(), Some("\"e1\""));
    assert_eq!(e.assignments.guild_visits, Some(true));
    assert_eq!(e.assignments.channel_threads.unwrap().config_version, 2);
    assert_eq!(
        c.experiments(Some("\"e1\"")).await.unwrap(),
        ExperimentsAnswer::NotModified
    );
}

#[tokio::test]
async fn an_instance_without_the_route_and_a_broken_body_are_told_apart() {
    let (server, instance) = common::instance().await;
    let c = Client::new(instance, Token::new("tok-experiments-404"));
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": "UNKNOWN_ROUTE"})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert_eq!(
        c.experiments(None).await.unwrap(),
        ExperimentsAnswer::NotFound
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!("not an object")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert_eq!(
        c.experiments(None).await.unwrap(),
        ExperimentsAnswer::Malformed
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"code": "INTERNAL"})))
        .mount(&server)
        .await;
    assert!(matches!(c.experiments(None).await, Err(Error::Api(_))));
}

#[tokio::test]
async fn discovery_is_revalidated_without_a_new_instance() {
    let (server, instance) = common::instance().await;
    let pins = instance.session_pins();
    assert_eq!(instance.etag(), Some("\"v1\""));
    // The mock answers 304 to the ETag it gave and a changed document otherwise.
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .and(header("If-None-Match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    assert!(instance
        .fetch_discovery(Some("\"v1\""))
        .await
        .unwrap()
        .is_none());

    let mut changed = common::discovery_json(&server.uri(), "ws://127.0.0.1:1/gateway");
    changed["notice"] = json!({"id": "n1", "message": "Planned maintenance", "url": null,
                                "starts_at": null, "ends_at": null});
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"v2\"")
                .set_body_json(changed),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (doc, etag) = instance
        .fetch_discovery(Some("\"v1\""))
        .await
        .unwrap()
        .expect("a new document");
    assert_eq!(etag.as_deref(), Some("\"v2\""));
    assert_eq!(doc.notice().unwrap().message, "Planned maintenance");
    // The instance the session holds is unchanged: same hosts, same ETag.
    assert_eq!(instance.session_pins(), pins);
    assert_eq!(instance.etag(), Some("\"v1\""));
}

#[tokio::test]
async fn an_instance_that_needs_a_newer_client_is_refused() {
    let (server, instance) = common::instance().await;
    let mut doc = common::discovery_json(&server.uri(), "ws://127.0.0.1:1/gateway");
    doc["api_code_version"] = json!(99);
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .respond_with(ResponseTemplate::new(200).set_body_json(doc))
        .mount(&server)
        .await;
    assert!(matches!(
        instance.fetch_discovery(None).await,
        Err(Error::UnsupportedApiVersion(99))
    ));
}

#[tokio::test]
async fn the_verification_email_and_the_terms_use_their_routes() {
    let (server, instance) = common::instance().await;
    let c = Client::new(instance, Token::new("tok-verify"));
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/verify/resend"))
        .respond_with(ResponseTemplate::new(204))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    c.resend_verification_email().await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/terms-acceptance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "41", "username": "me", "discriminator": "0001", "global_name": null,
            "avatar": null, "flags": 0, "terms_agreed_at": "2026-11-16T12:00:00.000Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let me = c.accept_terms().await.unwrap();
    assert!(me.terms_agreed_at_ms().is_some());
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/verify/resend"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "3600")
                .set_body_json(
                    json!({"code": "RATE_LIMITED", "retry_after": 3600.0, "global": false}),
                ),
        )
        .mount(&server)
        .await;
    let err = c.resend_verification_email().await.unwrap_err();
    assert!(matches!(err, Error::RateLimited { .. }), "{err:?}");
}
