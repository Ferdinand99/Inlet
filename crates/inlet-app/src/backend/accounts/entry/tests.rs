// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of the in-app sign-in against a mock instance: the classic form, identifier
//! first with a password or an emailed code, the second factor, the new-IP wait, the
//! CAPTCHA, the pauses the instance asks for, and what is dropped when the person leaves.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedReceiver;
use inlet_protocol as p;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver as InternalRx;
use tokio::time::timeout;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;
use crate::backend::accounts::testkit::{discovery, requests};
use crate::backend::accounts::{AccountKey, Accounts, Cmd as AccountsCmd, Ev as AccountsEv};
use crate::backend::Event;
use crate::token_store::MemoryStore;

fn backend() -> (
    Backend,
    UnboundedReceiver<Event>,
    InternalRx<Internal>,
    Arc<MemoryStore>,
) {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let store = Arc::new(MemoryStore::default());
    let accounts = Accounts::new(store.clone(), None, None);
    let mut b = Backend::with_store(crate::backend::Ui::new(tx), accounts);
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    b.internal = itx;
    (b, rx, irx, store)
}

/// A mock instance with the given `features`, `me` for the token `tok-new` (user 77) and
/// the lists a new session loads.
async fn instance(features: Value) -> MockServer {
    let server = MockServer::start().await;
    let mut doc = discovery(&server.uri(), "ws://127.0.0.1:1/gateway");
    doc["features"] = features;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"v1\"")
                .set_body_json(doc),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/@me"))
        .and(header("authorization", "tok-new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "77", "username": "ana", "global_name": "Ana", "discriminator": "0001",
            "avatar": null, "flags": 0
        })))
        .mount(&server)
        .await;
    for route in ["/api/v1/users/@me/guilds", "/api/v1/users/@me/channels"] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
    }
    server
}

fn token_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "token": "tok-new", "user_id": "77",
        "user": {"id": "77", "username": "ana", "discriminator": "0001", "global_name": "Ana",
                 "avatar": null, "flags": 0}
    }))
}

fn classic() -> Value {
    json!({"account_identity": "email", "emails_enabled": true})
}

fn identifier_first() -> Value {
    json!({"account_identity": "email", "identifier_first": true, "email_codes": true,
           "emails_enabled": true})
}

/// Feeds the backend its internal messages until an event matching `want` reaches the UI.
/// `seen` is told about every event on the way.
async fn pump(
    b: &mut Backend,
    irx: &mut InternalRx<Internal>,
    rx: &mut UnboundedReceiver<Event>,
    mut want: impl FnMut(&Event) -> bool,
) -> Event {
    loop {
        while let Ok(ev) = rx.try_recv() {
            if want(&ev) {
                return ev;
            }
        }
        match timeout(Duration::from_secs(15), irx.recv()).await {
            Ok(Some(msg)) => b.internal(msg),
            other => panic!("nothing happened: {:?}", other.is_ok()),
        }
    }
}

/// The next in-app sign-in event.
async fn next_entry(
    b: &mut Backend,
    irx: &mut InternalRx<Internal>,
    rx: &mut UnboundedReceiver<Event>,
) -> Ev {
    match pump(b, irx, rx, |e| {
        matches!(e, Event::Accounts(AccountsEv::Entry(_)))
    })
    .await
    {
        Event::Accounts(AccountsEv::Entry(ev)) => ev,
        _ => unreachable!(),
    }
}

fn start(b: &mut Backend, server: &MockServer) {
    handle(
        b,
        Cmd::Start {
            instance: server.uri(),
        },
    );
}

fn password(b: &mut Backend, identifier: &str, secret: &str) {
    handle(
        b,
        Cmd::Password {
            identifier: identifier.into(),
            password: p::Secret::new(secret),
        },
    );
}

/// Runs until the account is signed in; fails when a confirm screen is asked for.
async fn until_signed_in(
    b: &mut Backend,
    irx: &mut InternalRx<Internal>,
    rx: &mut UnboundedReceiver<Event>,
) {
    let confirmed = AtomicBool::new(false);
    pump(b, irx, rx, |e| {
        if matches!(e, Event::Accounts(AccountsEv::ConfirmAccount { .. })) {
            confirmed.store(true, Ordering::SeqCst);
        }
        matches!(e, Event::Accounts(AccountsEv::SignedIn { .. }))
    })
    .await;
    assert!(
        !confirmed.load(Ordering::SeqCst),
        "a sign-in typed in the app is not confirmed a second time"
    );
}

async fn count(server: &MockServer, route: &str) -> usize {
    requests(server)
        .await
        .iter()
        .filter(|r| r.url.path() == route)
        .count()
}

#[tokio::test]
async fn a_password_sign_in_goes_straight_to_the_account_and_the_keychain() {
    let server = instance(classic()).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(
            json!({"login": "ana@example.org", "password": "correct horse"}),
        ))
        .respond_with(token_response())
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, store) = backend();
    start(&mut b, &server);
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Started(info) => {
            assert!(!info.identifier_first);
            assert_eq!(info.identity, p::Identity::Email);
        }
        _ => panic!("the instance was not reported"),
    }
    password(&mut b, "ana@example.org", "correct horse");
    until_signed_in(&mut b, &mut irx, &mut rx).await;
    if let Some(tail) = b.store_tail.take() {
        let _ = tail.await;
    }
    // The token is in the keychain entry of the account, and the account is listed.
    let key = AccountKey::new(&server.uri(), "77").unwrap();
    assert!(store.entries.lock().unwrap().contains_key(&key.entry()));
    assert!(b.store.lock().await.list().iter().any(|a| a.key == key));
    assert!(b.entry.is_none(), "the sign-in is over");
}

#[tokio::test]
async fn identifier_first_asks_for_the_password_of_an_account_that_has_one() {
    let server = instance(identifier_first()).await;
    Mock::given(path("/api/v1/auth/identify"))
        .and(body_json(json!({"email": "ana@example.org"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ticket": "identify-1", "account": "existing", "sign_in_method": "password",
            "registration": null
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/email-code/send"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(token_response())
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::Started(info) if info.identifier_first
    ));
    handle(
        &mut b,
        Cmd::Identify {
            identifier: "ana@example.org".into(),
        },
    );
    assert_eq!(
        match next_entry(&mut b, &mut irx, &mut rx).await {
            Ev::Step(step) => step,
            _ => panic!("no step"),
        },
        EntryStep::Password
    );
    password(&mut b, "ana@example.org", "correct horse");
    until_signed_in(&mut b, &mut irx, &mut rx).await;
}

#[tokio::test]
async fn an_account_without_a_password_gets_an_email_code_and_signs_in_with_it() {
    let server = instance(identifier_first()).await;
    Mock::given(path("/api/v1/auth/identify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ticket": "identify-2", "account": "existing", "sign_in_method": "code",
            "registration": null
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/email-code/send"))
        .and(body_json(json!({"ticket": "identify-2"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "purpose": "sign_in",
            "code_expires_at": "2099-01-01T00:10:00.000Z",
            "resend_available_at": "2000-01-01T00:00:30.000Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/email-code/verify"))
        .and(body_json(
            json!({"ticket": "identify-2", "code": "123456", "consent": false}),
        ))
        .respond_with(token_response())
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(
        &mut b,
        Cmd::Identify {
            identifier: "ana@example.org".into(),
        },
    );
    match next_entry(&mut b, &mut irx, &mut rx).await {
        // The time the instance gave has passed: the code can be asked for again at once.
        Ev::Step(EntryStep::Code { resend_in_secs }) => assert_eq!(resend_in_secs, 0),
        _ => panic!("no code step"),
    }
    handle(
        &mut b,
        Cmd::Code {
            code: p::Secret::new("123456"),
        },
    );
    until_signed_in(&mut b, &mut irx, &mut rx).await;
}

#[tokio::test]
async fn a_new_address_is_reported_and_nothing_is_registered() {
    let server = instance(identifier_first()).await;
    Mock::given(path("/api/v1/auth/identify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ticket": "identify-3", "account": "new", "sign_in_method": null,
            "registration": {"mode": "closed", "consent_required": false, "requires_password": false}
        })))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(
        &mut b,
        Cmd::Identify {
            identifier: "nobody@example.org".into(),
        },
    );
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::NoAccount {
            registration_closed: true
        }
    ));
    for route in ["/api/v1/auth/register", "/api/v1/auth/email-code/send"] {
        assert_eq!(count(&server, route).await, 0, "{route} was called");
    }
}

#[tokio::test]
async fn a_second_factor_finishes_with_a_backup_code_typed_with_spaces() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mfa": true, "ticket": "mfa-1", "allowed_methods": ["totp", "backup_codes"],
            "totp": true, "webauthn": false, "backup_codes": true
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/login/mfa/totp"))
        .and(body_json(json!({"ticket": "mfa-1", "code": "abcdefgh"})))
        .respond_with(token_response())
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Step(EntryStep::Mfa {
            totp,
            backup_codes,
            passkey,
        }) => assert_eq!((totp, backup_codes, passkey), (true, true, false)),
        _ => panic!("no second factor step"),
    }
    handle(
        &mut b,
        Cmd::Mfa {
            code: p::Secret::new(" abcd efgh "),
        },
    );
    until_signed_in(&mut b, &mut irx, &mut rx).await;
}

#[tokio::test]
async fn a_wrong_second_factor_code_keeps_the_step_and_says_so() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mfa": true, "ticket": "mfa-2", "allowed_methods": ["totp"],
            "totp": true, "webauthn": false, "backup_codes": false
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/login/mfa/totp"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "INVALID_FORM_BODY", "message": "Input Validation Error",
            "errors": [{"path": "code", "code": "INVALID_CODE", "message": "Invalid code"}]
        })))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(
        &mut b,
        Cmd::Mfa {
            code: p::Secret::new("000000"),
        },
    );
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Failed(err) => {
            assert_eq!(err.field, Field::Code);
            assert!(!err.restart, "the ticket allows more tries");
            assert_eq!(err.message, t!("entry-error-code-wrong"));
        }
        _ => panic!("no error"),
    }
    assert!(b.entry.as_ref().is_some_and(|e| e.mfa.is_some()));
}

fn ip_required() -> ResponseTemplate {
    ResponseTemplate::new(403).set_body_json(json!({
        "code": "IP_AUTHORIZATION_REQUIRED", "message": "Check your email",
        "ip_authorization_required": true, "ticket": "ip-1", "email": "ana@example.org",
        "resend_available_in": 30
    }))
}

#[tokio::test]
async fn a_new_network_waits_for_the_email_link_and_then_signs_in() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ip_required())
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed": false})))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "completed": true, "token": "tok-new", "user_id": "77",
            "user": {"id": "77", "username": "ana", "discriminator": "0001", "global_name": "Ana",
                     "avatar": null, "flags": 0}
        })))
        .with_priority(2)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Step(EntryStep::IpAuthorization {
            email,
            resend_in_secs,
        }) => {
            assert_eq!(email.as_deref(), Some("ana@example.org"));
            assert_eq!(resend_in_secs, 30);
        }
        _ => panic!("no new network step"),
    }
    until_signed_in(&mut b, &mut irx, &mut rx).await;
    assert!(count(&server, "/api/v1/auth/ip-authorization/poll").await >= 3);
}

#[tokio::test]
async fn the_new_network_email_can_be_sent_again() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ip_required())
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed": false})))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/resend"))
        .and(body_json(json!({"ticket": "ip-1"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(&mut b, Cmd::IpResend);
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::IpResent { resend_in_secs: 30 }
    ));
}

#[tokio::test]
async fn three_failed_polls_in_a_row_say_the_connection_was_lost() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ip_required())
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::IpLost
    ));
    assert_eq!(
        count(&server, "/api/v1/auth/ip-authorization/poll").await,
        3
    );
}

#[tokio::test]
async fn an_expired_new_network_ticket_sends_the_person_back_to_the_start() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ip_required())
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "INVALID_FORM_BODY", "message": "Gone",
            "errors": [{"path": "ticket", "code": "INVALID_OR_EXPIRED_AUTHORIZATION_TICKET", "message": "Gone"}]
        })))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Failed(err) => assert!(err.restart),
        _ => panic!("no error"),
    }
    assert_eq!(
        count(&server, "/api/v1/auth/ip-authorization/poll").await,
        1
    );
}

#[tokio::test]
async fn leaving_the_sign_in_stops_the_wait_for_the_email_link() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ip_required())
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed": false})))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(&mut b, Cmd::Back);
    // A poll may be in flight; after it settles, no more come.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let polled = count(&server, "/api/v1/auth/ip-authorization/poll").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        count(&server, "/api/v1/auth/ip-authorization/poll").await,
        polled
    );
}

#[tokio::test]
async fn leaving_the_screen_stops_the_wait_for_the_email_link_and_forgets_the_instance() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ip_required())
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed": false})))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(&mut b, Cmd::Leave);
    assert!(b.entry.is_none());
    // A poll may be in flight; after it settles, no more come.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let polled = count(&server, "/api/v1/auth/ip-authorization/poll").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        count(&server, "/api/v1/auth/ip-authorization/poll").await,
        polled
    );
}

/// 400 `CAPTCHA_REQUIRED` until the request carries a solved challenge.
struct Gate(Value);

impl Respond for Gate {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        match req
            .headers
            .get("X-Captcha-Token")
            .and_then(|v| v.to_str().ok())
        {
            Some(t) if p::altcha::verify_test_token(t) => token_response(),
            _ => ResponseTemplate::new(400).set_body_json(json!({
                "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
                "captcha_provider": "altcha", "altcha_challenge": self.0
            })),
        }
    }
}

#[tokio::test]
async fn the_captcha_is_solved_on_this_machine_before_the_password_goes_again() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(Gate(p::altcha::make_test_challenge(500, 50)))
        .expect(2)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    until_signed_in(&mut b, &mut irx, &mut rx).await;
}

#[tokio::test]
async fn a_captcha_the_instance_does_not_accept_says_so_and_offers_the_browser() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
            "captcha_provider": "altcha", "altcha_challenge": p::altcha::make_test_challenge(3, 10)
        })))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Failed(err) => assert_eq!(err.message, t!("entry-error-captcha")),
        _ => panic!("no error"),
    }
}

#[tokio::test]
async fn a_pause_the_instance_asks_for_is_reported_and_not_retried() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "code": "RATE_LIMITED", "message": "Slow down", "retry_after": 45
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Failed(err) => {
            assert_eq!(err.wait_secs, Some(45));
            assert!(err.message.contains("45"), "{}", err.message);
        }
        _ => panic!("no error"),
    }
    assert_eq!(count(&server, "/api/v1/auth/login").await, 1);
}

#[tokio::test]
async fn the_same_instance_again_is_not_looked_up_again() {
    let server = instance(classic()).await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    start(&mut b, &server);
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::Started(_)
    ));
    assert_eq!(
        count(&server, "/.well-known/fluxer").await,
        1,
        "its limiter and answer are kept for the whole sign-in"
    );
}

#[tokio::test]
async fn an_answer_for_a_sign_in_that_was_left_is_dropped() {
    let server = instance(identifier_first()).await;
    Mock::given(path("/api/v1/auth/identify"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "ticket": "identify-late", "account": "existing", "sign_in_method": "password",
                    "registration": null
                }))
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(
        &mut b,
        Cmd::Identify {
            identifier: "ana@example.org".into(),
        },
    );
    handle(&mut b, Cmd::Back);
    let mut heard = false;
    while let Ok(Some(msg)) = timeout(Duration::from_millis(800), irx.recv()).await {
        b.internal(msg);
    }
    while let Ok(ev) = rx.try_recv() {
        heard |= matches!(ev, Event::Accounts(AccountsEv::Entry(_)));
    }
    assert!(!heard, "a late answer reached a form that had moved on");
    assert!(b.entry.as_ref().is_some_and(|e| e.identify.is_none()));
}

#[tokio::test]
async fn leaving_the_screen_forgets_the_instance_and_every_ticket() {
    let server = instance(classic()).await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    assert!(b.entry.is_some());
    handle(&mut b, Cmd::Leave);
    assert!(b.entry.is_none());
    // The browser sign-in's cancel is not the end of this one.
    handle(
        &mut b,
        Cmd::Start {
            instance: server.uri(),
        },
    );
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    crate::backend::accounts::handle(&mut b, AccountsCmd::CancelSignIn);
    assert!(
        b.entry.is_some(),
        "cancelling the browser sign-in keeps the form's state"
    );
}

#[test]
fn a_wrong_password_and_an_unknown_account_read_the_same() {
    let api = |code: &str, field: bool| {
        p::Error::Api(Box::new(p::ApiError {
            status: 400,
            code: if field { "INVALID_FORM_BODY" } else { code }.into(),
            message: "Input Validation Error".into(),
            errors: field.then(|| json!([{"path": "password", "code": code, "message": "x"}])),
        }))
    };
    let email = |e: &p::Error| explain(e, p::Identity::Email, Field::Password).message;
    // Fluxer sends the same field code for both; so does Inlet say the same thing.
    assert_eq!(
        email(&api("INVALID_EMAIL_OR_PASSWORD", true)),
        email(&api("INVALID_EMAIL_OR_PASSWORD", false))
    );
    assert_eq!(
        explain(
            &api("INVALID_LOGIN_OR_PASSWORD", true),
            p::Identity::Username,
            Field::Password
        )
        .message,
        t!("entry-error-credentials-username")
    );
    assert!(!email(&api("INVALID_EMAIL_OR_PASSWORD", true)).contains("exist"));
}

#[test]
fn errors_name_the_step_to_start_again_for_a_ticket_that_is_gone() {
    for code in [
        "SESSION_TIMEOUT",
        "UNKNOWN_SIGN_IN_TICKET",
        "INVALID_OR_EXPIRED_AUTHORIZATION_TICKET",
    ] {
        let err = p::Error::Api(Box::new(p::ApiError {
            status: 400,
            code: code.into(),
            message: "x".into(),
            errors: None,
        }));
        assert!(
            explain(&err, p::Identity::Email, Field::Code).restart,
            "{code}"
        );
    }
}

#[test]
fn a_long_pause_is_told_in_minutes() {
    let err = p::Error::RateLimited {
        retry_after: Duration::from_secs(200),
    };
    let told = explain(&err, p::Identity::Email, Field::Password);
    assert_eq!(told.wait_secs, Some(200));
    assert!(told.message.contains("4 min"), "{}", told.message);
}

#[test]
fn the_time_to_resend_follows_the_instance_and_falls_back_to_thirty_seconds() {
    let sent = |at: &str| p::CodeSent {
        purpose: p::CodePurpose::SignIn,
        expires_at: String::new(),
        resend_at: at.into(),
    };
    assert_eq!(resend_wait(&sent("not a time")), 30);
    assert_eq!(resend_wait(&sent("2000-01-01T00:00:00Z")), 0);
    let soon = (chrono::Utc::now() + chrono::Duration::seconds(20)).to_rfc3339();
    assert!((19..=20).contains(&resend_wait(&sent(&soon))));
}

/// Against the private test instance (username sign-in, no CAPTCHA, no email). Needs no
/// account. Run with `scripts/docker-build.sh fluxer-test -p inlet --lib -- --ignored
/// on_the_test_instance`.
#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn a_wrong_password_on_the_test_instance_reads_as_a_wrong_username_or_password() {
    let origin = std::env::var("INLET_TEST_ORIGIN").unwrap_or_else(|_| "fluxer.test".into());
    let (mut b, mut rx, mut irx, _store) = backend();
    handle(&mut b, Cmd::Start { instance: origin });
    let info = match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Started(info) => info,
        _ => panic!("the test instance was not found"),
    };
    assert_eq!(info.identity, p::Identity::Username);
    password(&mut b, "inlet_nobody_has_this_name", "not the password");
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Failed(err) => {
            println!("{err:?}");
            assert_eq!(err.field, Field::Password);
            assert_eq!(err.message, t!("entry-error-credentials-username"));
        }
        _ => panic!("no refusal"),
    }
}

#[tokio::test]
async fn a_step_for_a_sign_in_the_backend_no_longer_knows_says_it_is_lost() {
    let (mut b, mut rx, mut irx, _store) = backend();
    password(&mut b, "ana@example.org", "correct horse");
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::Lost
    ));
    handle(
        &mut b,
        Cmd::Code {
            code: p::Secret::new("123456"),
        },
    );
    assert!(matches!(
        next_entry(&mut b, &mut irx, &mut rx).await,
        Ev::Lost
    ));
}

// ---------- a positive sign-in against the private test instance ----------

/// Throwaway values from the operating system's random seed (no extra dependency).
fn random_hex(bytes: usize) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    while out.len() < bytes * 2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(out.len() as u64);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(bytes * 2);
    out
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, b) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let pad = |x: u8| -> Vec<u8> { block.iter().map(|b| b ^ x).collect() };
    let mut inner = pad(0x36);
    inner.extend_from_slice(msg);
    let mut outer = pad(0x5c);
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// The six-digit TOTP code of `secret` for 30-second step number `step`.
fn totp(secret: &[u8], step: u64) -> String {
    let mac = hmac_sha1(secret, &step.to_be_bytes());
    let o = (mac[19] & 0xf) as usize;
    let n = u32::from_be_bytes([mac[o] & 0x7f, mac[o + 1], mac[o + 2], mac[o + 3]]);
    format!("{:06}", n % 1_000_000)
}

fn base32(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut buf, mut bits) = (String::new(), 0u32, 0);
    for b in data {
        buf = (buf << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            out.push(A[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
        buf &= 0xff;
    }
    if bits > 0 {
        out.push(A[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[test]
fn the_totp_helpers_match_the_rfc_test_vectors() {
    // RFC 6238 appendix B (SHA-1, secret "12345678901234567890"): T = 59 s is step 1.
    assert_eq!(totp(b"12345678901234567890", 1), "287082");
    assert_eq!(totp(b"12345678901234567890", 37_037_036), "081804");
    assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
}

/// One JSON request to the test instance's API, with the test CA trusted.
async fn call(
    http: &reqwest::Client,
    base: &str,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let mut req = http
        .request(method, format!("{base}{path}"))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if let Some(token) = token {
        req = req.header("Authorization", token);
    }
    let resp = req.send().await.expect("the test instance answers");
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn now_step() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30
}

/// The throwaway account lives only in this test: a random name and password kept in
/// memory, a token that goes to a memory store, and a deletion at the end (the account
/// is left behind if that is refused). Run with `scripts/docker-build.sh fluxer-test -p
/// inlet --lib -- --ignored a_throwaway_account`.
#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn a_throwaway_account_signs_in_with_its_password_and_then_with_a_totp_code() {
    let origin = std::env::var("INLET_TEST_ORIGIN").unwrap_or_else(|_| "fluxer.test".into());
    let instance = p::Instance::discover(&origin).await.expect("discovery");
    let base = instance.api_base().to_string();
    let http = p::test_ca::add_to_http(reqwest::Client::builder())
        .build()
        .expect("http client");
    let username = format!("inlet_t34_{}", random_hex(6));
    let pw = random_hex(16);

    // Register the account (the instance takes a username, no email, no CAPTCHA).
    let (status, registered) = call(
        &http,
        &base,
        reqwest::Method::POST,
        "/auth/register",
        None,
        json!({"username": username, "password": pw, "global_name": username,
               "date_of_birth": "1990-01-01", "consent": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "registration: {} {}",
        registered["code"], registered["message"]
    );
    let first_token = registered["token"].as_str().expect("a token").to_string();
    let user_id = registered["user_id"]
        .as_str()
        .expect("a user id")
        .to_string();
    let key = AccountKey::new(instance.origin(), &user_id).unwrap();

    // 1. The password path, through the app's own flow.
    {
        let (mut b, mut rx, mut irx, store) = backend();
        handle(
            &mut b,
            Cmd::Start {
                instance: origin.clone(),
            },
        );
        let _ = next_entry(&mut b, &mut irx, &mut rx).await;
        password(&mut b, &username, &pw);
        until_signed_in(&mut b, &mut irx, &mut rx).await;
        if let Some(tail) = b.store_tail.take() {
            let _ = tail.await;
        }
        let entry = store.entries.lock().unwrap().get(&key.entry()).cloned();
        let stored: Value = serde_json::from_str(&entry.expect("the keychain entry")).unwrap();
        let token = stored["token"].as_str().unwrap_or_default();
        assert!(
            !token.is_empty() && token != first_token,
            "a new session token"
        );
        assert!(b.store.lock().await.list().iter().any(|a| a.key == key));
        println!("password path: signed in, token stored under the account's entry");
    }

    // 2. Turn TOTP on through the API, then sign in with the password and a code.
    let secret: Vec<u8> = (0..20)
        .map(|i| u8::from_str_radix(&random_hex(20)[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    let step = now_step();
    let (status, enabled) = call(
        &http,
        &base,
        reqwest::Method::POST,
        "/users/@me/mfa/totp/enable",
        Some(&first_token),
        json!({"secret": base32(&secret), "code": totp(&secret, step), "password": pw}),
    )
    .await;
    assert_eq!(
        status, 200,
        "enabling TOTP: {} {}",
        enabled["code"], enabled["message"]
    );
    {
        let (mut b, mut rx, mut irx, store) = backend();
        handle(
            &mut b,
            Cmd::Start {
                instance: origin.clone(),
            },
        );
        let _ = next_entry(&mut b, &mut irx, &mut rx).await;
        password(&mut b, &username, &pw);
        match next_entry(&mut b, &mut irx, &mut rx).await {
            Ev::Step(EntryStep::Mfa { totp, .. }) => assert!(totp),
            _ => panic!("no second factor step"),
        }
        // The next step is inside the instance's one-step skew, and is not the code
        // that enabled TOTP.
        handle(
            &mut b,
            Cmd::Mfa {
                code: p::Secret::new(totp(&secret, step + 1)),
            },
        );
        until_signed_in(&mut b, &mut irx, &mut rx).await;
        if let Some(tail) = b.store_tail.take() {
            let _ = tail.await;
        }
        assert!(store.entries.lock().unwrap().contains_key(&key.entry()));
        println!("totp path: signed in with a code, token stored under the account's entry");
    }

    // 3. Clean up: delete the account now; if the instance refuses, it stays behind.
    let (status, gone) = call(
        &http,
        &base,
        reqwest::Method::POST,
        "/users/@me/delete",
        Some(&first_token),
        json!({"password": pw, "mfa_method": "totp",
               "mfa_code": totp(&secret, step.saturating_sub(1)), "immediate": true}),
    )
    .await;
    println!("cleanup: {status} {}", gone["code"]);
}

// ---------- a session that arrives after the person left is ended ----------

fn logout_route() -> Mock {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(header("authorization", "tok-new"))
        .respond_with(ResponseTemplate::new(204))
}

async fn wait_for_requests(server: &MockServer, route: &str, wanted: usize) {
    for _ in 0..100 {
        if count(server, route).await >= wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_session_that_arrives_after_back_is_ended_on_the_server_and_never_stored() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(token_response().set_delay(Duration::from_millis(300)))
        .mount(&server)
        .await;
    logout_route().expect(1).mount(&server).await;
    let (mut b, mut rx, mut irx, store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    // The person goes back while the instance is still answering.
    handle(&mut b, Cmd::Back);
    let mut signed_in = false;
    while let Ok(Some(msg)) = timeout(Duration::from_millis(800), irx.recv()).await {
        b.internal(msg);
    }
    while let Ok(ev) = rx.try_recv() {
        signed_in |= matches!(ev, Event::Accounts(AccountsEv::SignedIn { .. }));
    }
    wait_for_requests(&server, "/api/v1/auth/logout", 1).await;
    assert!(!signed_in, "a left sign-in must not sign the account in");
    assert_eq!(count(&server, "/api/v1/auth/logout").await, 1);
    assert!(store.entries.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_new_network_session_that_arrives_after_leaving_is_ended_too() {
    let server = instance(classic()).await;
    logout_route().expect(1).mount(&server).await;
    let (mut b, _rx, _irx, _store) = backend();
    let instance = p::Instance::discover(&server.uri()).await.unwrap();
    // The poll's answer was already on its way when the person left.
    b.entry_done(Done {
        flow: 1,
        instance: Some(instance),
        what: Outcome::IpDone(p::SignedIn {
            token: p::Token::new("tok-new"),
            user_id: "77".into(),
            user: None,
        }),
    });
    wait_for_requests(&server, "/api/v1/auth/logout", 1).await;
    assert_eq!(count(&server, "/api/v1/auth/logout").await, 1);
}

#[tokio::test]
async fn the_address_of_the_new_network_email_is_cleaned_before_it_is_shown() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "code": "IP_AUTHORIZATION_REQUIRED", "message": "Check your email",
            "ticket": "ip-9", "email": "ana@exa\u{202e}mple.org", "resend_available_in": 30
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed": false})))
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    match next_entry(&mut b, &mut irx, &mut rx).await {
        Ev::Step(EntryStep::IpAuthorization { email, .. }) => {
            assert_eq!(email.as_deref(), Some("ana@example.org"));
        }
        _ => panic!("no new network step"),
    }
}

// ---------- an invite, and stopping the CAPTCHA work ----------

#[tokio::test]
async fn a_sign_in_that_starts_from_an_invite_carries_the_code_with_every_step() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .and(body_json(
            json!({"login": "ana@example.org", "password": "correct horse",
                   "invite_code": "abc123"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mfa": true, "ticket": "mfa-inv", "allowed_methods": ["totp"],
            "totp": true, "webauthn": false, "backup_codes": false
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/login/mfa/totp"))
        .and(body_json(
            json!({"ticket": "mfa-inv", "code": "123456", "invite_code": "abc123"}),
        ))
        .respond_with(token_response())
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    handle(&mut b, Cmd::Invite(Some("abc123".into())));
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    handle(
        &mut b,
        Cmd::Mfa {
            code: p::Secret::new("123456"),
        },
    );
    until_signed_in(&mut b, &mut irx, &mut rx).await;
}

#[tokio::test]
async fn a_sign_in_without_an_invite_sends_no_invite_code() {
    let server = instance(classic()).await;
    Mock::given(path("/api/v1/auth/login"))
        .and(body_json(
            json!({"login": "ana@example.org", "password": "correct horse"}),
        ))
        .respond_with(token_response())
        .expect(1)
        .mount(&server)
        .await;
    let (mut b, mut rx, mut irx, _store) = backend();
    // An invite that was left behind by an earlier sign-in is forgotten with it.
    handle(&mut b, Cmd::Invite(Some("old".into())));
    handle(&mut b, Cmd::Leave);
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    until_signed_in(&mut b, &mut irx, &mut rx).await;
}

#[tokio::test]
async fn leaving_while_the_captcha_is_solved_stops_the_work_and_frees_the_solver() {
    let server = instance(classic()).await;
    let mut challenge = p::altcha::make_test_challenge(1, 1);
    challenge["parameters"]["keyPrefix"] = json!("0123456789abcdef0123456789abcdef");
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
            "captcha_provider": "altcha", "altcha_challenge": challenge
        })))
        .expect(1)
        .mount(&server)
        .await;
    logout_route().expect(0).mount(&server).await;
    let (mut b, mut rx, mut irx, _store) = backend();
    start(&mut b, &server);
    let _ = next_entry(&mut b, &mut irx, &mut rx).await;
    password(&mut b, "ana@example.org", "correct horse");
    // The solve is running (the challenge came back once); the person goes back.
    wait_for_requests(&server, "/api/v1/auth/login", 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle(&mut b, Cmd::Back);
    // The workers stop, so the one solver slot is free again within moments.
    let token = p::altcha::solve(
        p::altcha::make_test_challenge(40, 10),
        Duration::from_secs(10),
    )
    .await
    .expect("the solver is free again");
    assert!(p::altcha::verify_test_token(&token));
    while let Ok(Some(msg)) = timeout(Duration::from_millis(300), irx.recv()).await {
        b.internal(msg);
    }
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(
                ev,
                Event::Accounts(AccountsEv::SignedIn { .. } | AccountsEv::Entry(_))
            ),
            "a cancelled sign-in said something"
        );
    }
    assert_eq!(count(&server, "/api/v1/auth/login").await, 1);
}
