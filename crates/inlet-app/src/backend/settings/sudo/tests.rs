// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sudo part of the backend against a mock Fluxer instance, and (ignored) the whole
//! "Confirm it's you" flow against the private test instance.

use std::sync::Arc;

use inlet_protocol as p;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJ0eXBlIjoic3VkbyJ9.c2lnbmF0dXJl";
const KIT: &str = "/api/v1/users/@me/recovery-kit";

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
        "features": {"account_identity": "username"},
        "limits": {"version": 2, "rules": []},
        "app_public": {"branding": {"product_name": "Mock"}},
        "sso": {"enabled": false},
        "domain_migration": {"enabled": false}
    })
}

async fn fixture() -> (MockServer, p::Client) {
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
    (server, p::Client::new(instance, p::Token::new("tok-sudo")))
}

fn kit() -> Request {
    Request::new(p::Method::POST, "/users/@me/recovery-kit", None).unwrap()
}

fn password(text: &str) -> Option<Proof> {
    Some(Proof::Password(p::Secret::new(text)))
}

fn code(text: &str) -> Option<Proof> {
    Some(Proof::Code(p::Secret::new(text)))
}

fn needs_password() -> ResponseTemplate {
    ResponseTemplate::new(403).set_body_json(json!({
        "code": "SUDO_MODE_REQUIRED", "message": "Sudo mode required", "has_mfa": false,
        "methods": {"password": true}
    }))
}

fn invalid(field: &str, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "code": "INVALID_FORM_BODY", "message": "Invalid form body",
        "errors": [{"path": field, "code": code, "message": "no"}]
    }))
}

#[tokio::test]
async fn the_first_try_says_what_the_account_can_use() {
    let (server, client) = fixture().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(needs_password())
        .mount(&server)
        .await;
    let (ev, ended) = run(&client, 3, &kit(), None).await;
    assert!(ended.is_none());
    match ev {
        Ev::Needed { ticket, methods } => {
            assert_eq!(ticket, 3);
            assert!(methods.password && !methods.totp);
        }
        _ => panic!("expected Needed"),
    }
}

#[tokio::test]
async fn a_wrong_password_is_rejected_and_the_right_one_goes_through() {
    let (server, client) = fixture().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(json!({"password": "wrong"})))
        .respond_with(invalid("password", "INVALID_PASSWORD"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(json!({"password": "right-password"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"recovery_key": "k"})))
        .mount(&server)
        .await;
    let (ev, _) = run(&client, 1, &kit(), password("wrong")).await;
    assert!(matches!(
        ev,
        Ev::Rejected {
            ticket: 1,
            wrong: Wrong::Password
        }
    ));
    let (ev, _) = run(&client, 1, &kit(), password("right-password")).await;
    match ev {
        Ev::Done { ticket, response } => {
            assert_eq!(ticket, 1);
            assert_eq!(response.0["recovery_key"], "k");
            // The answer can hold a secret: it shows nothing.
            assert_eq!(format!("{response:?}"), "Response(<hidden>)");
        }
        _ => panic!("expected Done"),
    }
}

#[tokio::test]
async fn five_wrong_proofs_lock_the_dialog_and_the_sixth_is_not_sent() {
    let (server, client) = fixture().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(invalid("mfa_code", "INVALID_MFA_CODE"))
        .mount(&server)
        .await;
    for i in 0..p::sudo::MAX_WRONG_PROOFS {
        let (ev, _) = run(&client, 1, &kit(), code("000000")).await;
        assert!(
            matches!(
                ev,
                Ev::Rejected {
                    wrong: Wrong::Code,
                    ..
                }
            ),
            "try {i}"
        );
    }
    let (ev, _) = run(&client, 1, &kit(), code("000000")).await;
    match ev {
        Ev::Locked { wait_secs, .. } => assert!(wait_secs > 14 * 60),
        _ => panic!("expected Locked"),
    }
    let sent = server.received_requests().await.unwrap();
    let to_kit = sent.iter().filter(|r| r.url.path() == KIT).count();
    assert_eq!(to_kit, p::sudo::MAX_WRONG_PROOFS);
}

#[tokio::test]
async fn a_code_gives_a_token_and_the_next_action_needs_no_proof() {
    let (server, client) = fixture().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(
            json!({"mfa_method": "totp", "mfa_code": "123456"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(p::sudo::SUDO_HEADER, JWT)
                .set_body_json(json!({})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/mfa/backup-codes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"backup_codes": []})))
        .mount(&server)
        .await;
    let (ev, _) = run(&client, 1, &kit(), code("123456")).await;
    assert!(matches!(ev, Ev::Done { .. }));
    let codes = Request::new(p::Method::POST, "/users/@me/mfa/backup-codes", None).unwrap();
    let (ev, _) = run(&client, 2, &codes, None).await;
    assert!(matches!(ev, Ev::Done { ticket: 2, .. }));
    let sent = server.received_requests().await.unwrap();
    let with_token = sent
        .iter()
        .find(|r| r.url.path() == "/api/v1/users/@me/mfa/backup-codes")
        .unwrap();
    assert_eq!(
        with_token
            .headers
            .get("x-fluxer-sudo-mode-jwt")
            .and_then(|v| v.to_str().ok()),
        Some(JWT)
    );
}

#[tokio::test]
async fn a_refused_session_token_ends_the_session_and_other_failures_show_their_reason() {
    let (server, client) = fixture().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": "UNAUTHORIZED"})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "code": "ACCESS_DENIED", "message": "You may not do that"
        })))
        .mount(&server)
        .await;
    let (ev, ended) = run(&client, 1, &kit(), None).await;
    assert!(matches!(ev, Ev::Failed { .. }));
    assert!(ended.is_some(), "a 401 ends the session");
    let (ev, ended) = run(&client, 1, &kit(), None).await;
    match ev {
        Ev::Failed { reason, .. } => assert!(reason.contains("You may not do that"), "{reason}"),
        _ => panic!("expected Failed"),
    }
    assert!(ended.is_none());
}

#[test]
fn only_sudo_routes_can_be_asked_for() {
    assert!(Request::new(p::Method::POST, "/users/@me/delete", None).is_some());
    assert!(Request::new(p::Method::POST, "/channels/1/messages", None).is_none());
    assert!(Request::new(p::Method::GET, "/users/@me", None).is_none());
    let r = Request::new(
        p::Method::POST,
        "/users/@me/delete",
        Some(json!({"immediate": true})),
    );
    assert_eq!(r.unwrap().path(), "/users/@me/delete");
}

/// An action for an account that is not the active one (or when none is) is answered as
/// failed at once, and the proof never reaches any session.
#[tokio::test]
async fn an_action_without_its_account_is_answered_as_failed() {
    use futures::StreamExt;

    use crate::backend::accounts::{AccountKey, Accounts};
    use crate::backend::{Backend, Event, Ui};
    use crate::token_store::MemoryStore;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let store = Accounts::new(Arc::new(MemoryStore::default()), None, None);
    let mut backend = Backend::with_store(Ui::new(tx), store);
    handle(
        &mut backend,
        Cmd::Run {
            account: AccountKey::new("https://a.example", "1").unwrap(),
            ticket: 4,
            request: Arc::new(kit()),
            proof: password("typed-for-another-account"),
        },
    );
    match rx.next().await {
        Some(Event::Settings(super::super::Ev::Sudo(Ev::Failed { ticket, .. }))) => {
            assert_eq!(ticket, 4);
        }
        _ => panic!("expected Failed"),
    }
}

// ---------- the whole dialog flow on the private test instance ----------

/// The dialog's state and the backend's `run`, driven the way the dialog drives them,
/// against a throwaway account on the private test instance: ask, wrong password, right
/// password, turn two-factor on, ask again (the password is no way in now), a code, a
/// backup code, the token, and the deletion of the account. Run with
/// `scripts/docker-build.sh fluxer-test -p inlet --lib -- --ignored the_confirm_dialog`.
#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn the_confirm_dialog_flow_on_the_test_instance() {
    use crate::ui::overlays::sudo_confirm::state::{Mode, Outcome, Problem, State};
    use p::test_support::{base32, http, random_hex, totp, totp_step_now, Throwaway};

    let origin = std::env::var("INLET_TEST_ORIGIN").unwrap_or_else(|_| "fluxer.test".into());
    let instance = p::Instance::discover(&origin).await.expect("discovery");
    let base = instance.api_base().to_string();
    let http = http();
    let account = Throwaway::register(&http, &base, "/auth/register", "inlet_f1a_app").await;
    let client = p::Client::new(instance, p::Token::new(account.token.clone()));
    client.me().await.expect("the new token works");

    let request = Arc::new(kit());
    let mut ticket = 0u64;
    let mut ask = |client: &p::Client, request: Arc<Request>, proof: Option<Proof>| {
        ticket += 1;
        let client = client.clone();
        let t = ticket;
        async move { (t, run(&client, t, &request, proof).await.0) }
    };

    // 1. The action goes out without a proof: the instance asks, the dialog opens.
    let (t, ev) = ask(&client, request.clone(), None).await;
    let Ev::Needed { methods, .. } = &ev else {
        panic!("expected Needed");
    };
    let mut dialog = State::new(t, request.clone(), *methods);
    assert_eq!(dialog.mode, Mode::Password);
    assert!(dialog.can_confirm() && dialog.other_mode().is_none());

    // 2. A wrong password: an error under the field, the dialog stays and is free.
    assert!(dialog.submit());
    let (_, ev) = ask(&client, request.clone(), password(&random_hex(10))).await;
    let ev = with_ticket(ev, dialog.ticket);
    assert_eq!(dialog.apply(&ev), Outcome::Stay);
    assert_eq!(dialog.problem, Some(Problem::WrongPassword));
    assert!(!dialog.busy);

    // 3. The right password: done.
    assert!(dialog.submit());
    let (_, ev) = ask(&client, request.clone(), password(&account.password)).await;
    assert_eq!(dialog.apply(&with_ticket(ev, dialog.ticket)), Outcome::Done);

    // 4. Turn two-factor on (the password is the proof).
    let secret: Vec<u8> = (0..20)
        .map(|i| u8::from_str_radix(&random_hex(20)[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    let step = totp_step_now();
    let enable = Arc::new(
        Request::new(
            p::Method::POST,
            "/users/@me/mfa/totp/enable",
            Some(json!({"secret": base32(&secret), "code": totp(&secret, step)})),
        )
        .unwrap(),
    );
    let (_, ev) = ask(&client, enable, password(&account.password)).await;
    let Ev::Done { response, .. } = ev else {
        panic!("enabling TOTP did not go through");
    };
    let backup = response.0["backup_codes"][0]["code"]
        .as_str()
        .or_else(|| response.0["backup_codes"][0].as_str())
        .expect("a backup code")
        .to_owned();

    // 5. Ask again: the account has a second factor now. The dialog starts in the code
    //    way; a password sent anyway makes the instance ask again, and the dialog stays
    //    on the code.
    let (t, ev) = ask(&client, request.clone(), None).await;
    let Ev::Needed { methods, .. } = &ev else {
        panic!("expected Needed");
    };
    assert!(methods.totp && !methods.password);
    let mut dialog = State::new(t, request.clone(), *methods);
    assert_eq!(dialog.mode, Mode::Code);
    let (_, ev) = ask(&client, request.clone(), password(&account.password)).await;
    assert!(
        matches!(ev, Ev::Needed { .. }),
        "a password is no way in now"
    );
    assert_eq!(dialog.apply(&with_ticket(ev, dialog.ticket)), Outcome::Stay);
    assert_eq!(dialog.mode, Mode::Code);

    // 6. A wrong code, then the right one (the next step, so it is not the code that
    //    turned two-factor on): done, and the session now holds a token.
    let (_, ev) = ask(&client, request.clone(), code("000000")).await;
    assert!(matches!(
        ev,
        Ev::Rejected {
            wrong: Wrong::Code,
            ..
        }
    ));
    let (_, ev) = ask(&client, request.clone(), code(&totp(&secret, step + 1))).await;
    assert!(matches!(ev, Ev::Done { .. }));
    assert!(client.has_sudo_token());
    // The next action needs no dialog at all.
    let (_, ev) = ask(&client, request.clone(), None).await;
    assert!(matches!(ev, Ev::Done { .. }));

    // 7. A backup code is a proof too, once.
    client.forget_sudo();
    let (_, ev) = ask(&client, request.clone(), code(&backup)).await;
    assert!(matches!(ev, Ev::Done { .. }));
    client.forget_sudo();
    let (_, ev) = ask(&client, request.clone(), code(&backup)).await;
    assert!(
        matches!(
            ev,
            Ev::Rejected {
                wrong: Wrong::Code,
                ..
            }
        ),
        "a spent backup code"
    );

    // 8. Clean up: delete the account with the session's token.
    client.forget_sudo();
    let delete = Arc::new(
        Request::new(
            p::Method::POST,
            "/users/@me/delete",
            Some(json!({"immediate": true})),
        )
        .unwrap(),
    );
    let (_, ev) = ask(&client, delete, code(&totp(&secret, unused_step(step)))).await;
    println!(
        "cleanup: {}",
        if matches!(ev, Ev::Done { .. }) {
            "deleted"
        } else {
            "left behind"
        }
    );
}

/// The same answer under another ticket (the dialog's own).
fn with_ticket(ev: Ev, ticket: u64) -> Ev {
    match ev {
        Ev::Needed { methods, .. } => Ev::Needed { ticket, methods },
        Ev::Rejected { wrong, .. } => Ev::Rejected { ticket, wrong },
        Ev::Locked { wait_secs, .. } => Ev::Locked { ticket, wait_secs },
        Ev::Done { response, .. } => Ev::Done { ticket, response },
        Ev::Failed { reason, .. } => Ev::Failed { ticket, reason },
    }
}

/// A TOTP step the instance accepts (within one step of now) that the test has not used
/// yet: the steps `used` and `used + 1` are spent.
fn unused_step(used: u64) -> u64 {
    let now = p::test_support::totp_step_now();
    [now + 1, now, now.saturating_sub(1)]
        .into_iter()
        .find(|s| *s != used && *s != used + 1)
        .expect("a free step")
}
