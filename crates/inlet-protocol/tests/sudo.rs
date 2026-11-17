//! Sudo mode against a mock instance: the typed error, the proof in the body, the token
//! kept and attached only to the sudo routes, and the cap on wrong proofs.

mod common;

use inlet_protocol::sudo::{MAX_WRONG_PROOFS, SUDO_HEADER};
use inlet_protocol::{
    Client, Error, Instance, Method, SudoMethods, SudoProof, SudoRejection, Secret, Token,
};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJ0eXBlIjoic3VkbyJ9.c2lnbmF0dXJl";
const KIT: &str = "/api/v1/users/@me/recovery-kit";
const CODES: &str = "/api/v1/users/@me/mfa/backup-codes";

fn client(instance: &Instance) -> Client {
    Client::new(instance.clone(), Token::new("tok-sudo"))
}

fn required(methods: Value) -> ResponseTemplate {
    ResponseTemplate::new(403).set_body_json(json!({
        "code": "SUDO_MODE_REQUIRED", "message": "Sudo mode required",
        "has_mfa": methods["totp"].as_bool().unwrap_or(false), "methods": methods
    }))
}

fn invalid(field: &str, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "code": "INVALID_FORM_BODY", "message": "Invalid form body",
        "errors": [{"path": field, "code": code, "message": "no"}]
    }))
}

/// Every request to the API (the discovery request is left out).
async fn api_requests(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().starts_with("/api/"))
        .collect()
}

async fn requests_to(server: &MockServer, route: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == route)
        .collect()
}

fn sudo_header(r: &Request) -> Option<String> {
    r.headers
        .get(SUDO_HEADER.to_ascii_lowercase().as_str())
        .map(|v| v.to_str().unwrap().to_owned())
}

/// Mounts a route that answers 200 with the sudo token in its header.
async fn gives_token(server: &MockServer, route: &str) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).insert_header(SUDO_HEADER, JWT))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_sudo_route_without_a_token_says_what_the_account_can_use() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(required(json!({
            "totp": true, "webauthn": false, "backup_codes": true,
            "email_code": false, "password": false, "sso_reauth": false
        })))
        .mount(&server)
        .await;
    let c = client(&instance);
    let err = c
        .sudo_request(Method::POST, "/users/@me/recovery-kit", None, None)
        .await
        .unwrap_err();
    match err {
        Error::SudoRequired(r) => {
            assert!(r.has_mfa);
            assert_eq!(
                r.methods,
                SudoMethods {
                    totp: true,
                    backup_codes: true,
                    ..SudoMethods::default()
                }
            );
        }
        other => panic!("expected SudoRequired, got {other:?}"),
    }
    // The same typed error comes from the generic request, so any feature gets it.
    let err = c
        .request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SudoRequired(_)));
}

#[tokio::test]
async fn a_password_goes_in_the_body_and_an_account_without_a_second_factor_gets_no_token() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(json!({"password": "correct horse"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"recovery_key": "k"})))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&instance);
    let v = c
        .sudo_request(
            Method::POST,
            "/users/@me/recovery-kit",
            None,
            Some(&SudoProof::password("correct horse")),
        )
        .await
        .unwrap();
    assert_eq!(v["recovery_key"], "k");
    assert!(!c.has_sudo_token());
}

#[tokio::test]
async fn a_code_goes_as_totp_and_the_token_from_the_answer_is_used_next() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(
            json!({"mfa_method": "totp", "mfa_code": "123456"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(SUDO_HEADER, JWT)
                .set_body_json(json!({"recovery_key": "k"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(CODES))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"backup_codes": []})))
        .mount(&server)
        .await;
    let c = client(&instance);
    c.sudo_request(
        Method::POST,
        "/users/@me/recovery-kit",
        None,
        Some(&SudoProof::code("123456")),
    )
    .await
    .unwrap();
    assert!(c.has_sudo_token());

    // The next sudo route needs no proof: the token is attached.
    c.sudo_request(Method::POST, "/users/@me/mfa/backup-codes", None, None)
        .await
        .unwrap();
    let sent = requests_to(&server, CODES).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sudo_header(&sent[0]).as_deref(), Some(JWT));
    // The proof request itself carried no token (the proof stands in for it).
    let first = requests_to(&server, KIT).await;
    assert_eq!(sudo_header(&first[0]), None);
}

#[tokio::test]
async fn the_token_goes_only_to_sudo_routes() {
    let (server, instance) = common::instance().await;
    gives_token(&server, KIT).await;
    for (m, p) in [
        ("GET", "/api/v1/users/@me"),
        ("POST", "/api/v1/channels/5/messages"),
        ("GET", "/api/v1/users/@me/guilds"),
        ("POST", "/api/v1/channels/5/typing"),
        ("PATCH", "/api/v1/users/@me"),
    ] {
        Mock::given(method(m))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
    }
    let c = client(&instance);
    c.request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap();
    assert!(c.has_sudo_token());

    for (m, p) in [
        (Method::GET, "/users/@me"),
        (Method::POST, "/channels/5/messages"),
        (Method::GET, "/users/@me/guilds"),
        (Method::POST, "/channels/5/typing"),
    ] {
        c.request_json(m, p, None).await.unwrap();
    }
    // PATCH /users/@me is a sudo route (it takes the token), GET /users/@me is not.
    c.request_json(Method::PATCH, "/users/@me", Some(&json!({"bio": "x"})))
        .await
        .unwrap();

    // The first request (which gave the token) is skipped; every later one carries the
    // header only if it is the PATCH to a sudo route.
    let all = api_requests(&server).await;
    assert_eq!(all.len(), 6);
    for r in &all[1..] {
        let on_sudo_route = r.method.as_str() == "PATCH" && r.url.path() == "/api/v1/users/@me";
        assert_eq!(
            sudo_header(r).is_some(),
            on_sudo_route,
            "{} {}",
            r.method,
            r.url
        );
    }
}

#[tokio::test]
async fn a_token_in_the_answer_of_another_route_is_not_kept() {
    let (server, instance) = common::instance().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/@me"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(SUDO_HEADER, JWT)
                .set_body_json(common::user_json("10", "ana")),
        )
        .mount(&server)
        .await;
    let c = client(&instance);
    c.me().await.unwrap();
    assert!(!c.has_sudo_token());
}

#[tokio::test]
async fn a_header_that_is_not_a_token_is_not_kept() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(ResponseTemplate::new(200).insert_header(SUDO_HEADER, "not a jwt"))
        .mount(&server)
        .await;
    let c = client(&instance);
    c.request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap();
    assert!(!c.has_sudo_token());
}

#[tokio::test]
async fn a_refused_token_is_dropped_and_the_error_comes_back() {
    let (server, instance) = common::instance().await;
    // The first answer gives a token; the second refuses (the instance forgot it).
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(ResponseTemplate::new(200).insert_header(SUDO_HEADER, JWT))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(required(json!({"totp": true})))
        .mount(&server)
        .await;
    let c = client(&instance);
    c.request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap();
    assert!(c.has_sudo_token());
    let err = c
        .request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SudoRequired(_)));
    assert!(!c.has_sudo_token());
}

#[tokio::test]
async fn forgetting_drops_the_token_for_every_clone() {
    let (server, instance) = common::instance().await;
    gives_token(&server, KIT).await;
    let c = client(&instance);
    let clone = c.clone();
    c.request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap();
    assert!(clone.has_sudo_token(), "clones share the session's token");
    clone.forget_sudo();
    assert!(!c.has_sudo_token());
}

#[tokio::test]
async fn a_wrong_proof_is_named_and_counted_and_five_stop_the_sending() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(json!({"password": "wrong-password"})))
        .respond_with(invalid("password", "INVALID_PASSWORD"))
        .mount(&server)
        .await;
    let c = client(&instance);
    let wrong = || SudoProof::password("wrong-password");
    for i in 0..MAX_WRONG_PROOFS {
        let err = c
            .sudo_request(
                Method::POST,
                "/users/@me/recovery-kit",
                None,
                Some(&wrong()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::SudoRejected(SudoRejection::WrongPassword)),
            "try {i}: {err:?}"
        );
    }
    // The sixth is not sent, and neither is a proof for another route.
    let err = c
        .sudo_request(
            Method::POST,
            "/users/@me/recovery-kit",
            None,
            Some(&wrong()),
        )
        .await
        .unwrap_err();
    match err {
        Error::RateLimited { retry_after } => assert!(retry_after.as_secs() > 14 * 60),
        other => panic!("expected RateLimited, got {other:?}"),
    }
    let err = c
        .sudo_request(
            Method::POST,
            "/users/@me/mfa/backup-codes",
            None,
            Some(&SudoProof::code("000000")),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::RateLimited { .. }));
    assert_eq!(
        requests_to(&server, KIT).await.len(),
        MAX_WRONG_PROOFS,
        "exactly five proofs were sent"
    );
    assert!(requests_to(&server, CODES).await.is_empty());
}

#[tokio::test]
async fn a_right_proof_clears_the_count() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(json!({"password": "wrong-password"})))
        .respond_with(invalid("password", "INVALID_PASSWORD"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .and(body_json(json!({"password": "right-password"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let c = client(&instance);
    let send = |pw: &'static str| {
        let c = c.clone();
        async move {
            c.sudo_request(
                Method::POST,
                "/users/@me/recovery-kit",
                None,
                Some(&SudoProof::password(pw)),
            )
            .await
        }
    };
    for _ in 0..MAX_WRONG_PROOFS - 1 {
        assert!(send("wrong-password").await.is_err());
    }
    send("right-password").await.unwrap();
    // Four more wrong ones fit again.
    for _ in 0..MAX_WRONG_PROOFS - 1 {
        assert!(matches!(
            send("wrong-password").await,
            Err(Error::SudoRejected(_))
        ));
    }
}

#[tokio::test]
async fn a_wrong_code_and_a_wrong_email_code_are_named() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path(CODES))
        .respond_with(invalid("mfa_code", "INVALID_MFA_CODE"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(KIT))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"code": "INVALID_SIGN_IN_CODE", "message": "no"})),
        )
        .mount(&server)
        .await;
    let c = client(&instance);
    let err = c
        .sudo_request(
            Method::POST,
            "/users/@me/mfa/backup-codes",
            None,
            Some(&SudoProof::code("000000")),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SudoRejected(SudoRejection::WrongCode)));
    let err = c
        .sudo_request(
            Method::POST,
            "/users/@me/recovery-kit",
            None,
            Some(&SudoProof::EmailCode {
                ticket: Secret::new("ticket"),
                code: Secret::new("123456"),
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::SudoRejected(SudoRejection::WrongEmailCode)
    ));
}

#[tokio::test]
async fn a_failure_that_is_not_about_the_proof_is_an_ordinary_error_and_not_counted() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/delete"))
        .respond_with(invalid("delete_messages", "INVALID_BOOLEAN"))
        .mount(&server)
        .await;
    let c = client(&instance);
    for _ in 0..MAX_WRONG_PROOFS + 2 {
        let err = c
            .sudo_request(
                Method::POST,
                "/users/@me/delete",
                Some(&json!({"delete_messages": "yes"})),
                Some(&SudoProof::password("right-password")),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Api(_)), "{err:?}");
    }
}

#[tokio::test]
async fn a_proof_is_never_sent_to_a_route_that_does_not_ask_for_it() {
    let (server, instance) = common::instance().await;
    let c = client(&instance);
    for (m, p) in [
        (Method::POST, "/channels/5/messages"),
        (Method::GET, "/users/@me"),
        (Method::POST, "/auth/login"),
        (Method::POST, "/users/@me/harvest"),
    ] {
        let err = c
            .sudo_request(m, p, None, Some(&SudoProof::password("pw-12345678")))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
    }
    assert!(api_requests(&server).await.is_empty());
}

#[tokio::test]
async fn a_proof_that_fluxer_would_refuse_is_not_sent_and_not_counted() {
    let (server, instance) = common::instance().await;
    let c = client(&instance);
    for proof in [
        SudoProof::password(""),
        SudoProof::password("x".repeat(300)),
        SudoProof::code("x".repeat(40)),
    ] {
        let err = c
            .sudo_request(Method::POST, "/users/@me/delete", None, Some(&proof))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }
    assert!(api_requests(&server).await.is_empty());
}

#[tokio::test]
async fn the_extra_fields_of_the_action_are_kept_next_to_the_proof() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/delete"))
        .and(body_json(
            json!({"delete_messages": true, "password": "right-password"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deletion_at": "t"})))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&instance);
    c.sudo_request(
        Method::POST,
        "/users/@me/delete",
        Some(&json!({"delete_messages": true})),
        Some(&SudoProof::password("right-password")),
    )
    .await
    .unwrap();
    // A body that is not an object cannot carry a proof.
    let err = c
        .sudo_request(
            Method::POST,
            "/users/@me/delete",
            Some(&json!([1, 2])),
            Some(&SudoProof::password("right-password")),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[tokio::test]
async fn the_methods_and_the_email_code_routes() {
    let (server, instance) = common::instance().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/@me/sudo/mfa-methods"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "totp": true, "webauthn": false, "backup_codes": true, "has_mfa": true,
            "password": false, "new_field": 1
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/@me/sudo/email-code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ticket": "tkt", "code_expires_at": "2026-11-17T09:00:00.000Z",
            "resend_available_at": "2026-11-17T08:50:30.000Z"
        })))
        .mount(&server)
        .await;
    let c = client(&instance);
    let methods = c.sudo_methods().await.unwrap();
    assert!(methods.totp && methods.backup_codes && !methods.password);
    let sent = c.send_sudo_email_code().await.unwrap();
    assert_eq!(sent.ticket.expose_secret(), "tkt");
    assert_eq!(
        sent.resend_available_at.as_deref(),
        Some("2026-11-17T08:50:30.000Z")
    );
    assert!(!format!("{sent:?}").contains("tkt"));
}

#[tokio::test]
async fn the_client_debug_shows_no_token() {
    let (server, instance) = common::instance().await;
    gives_token(&server, KIT).await;
    let c = client(&instance);
    c.request_json(Method::POST, "/users/@me/recovery-kit", None)
        .await
        .unwrap();
    let shown = format!("{c:?}");
    assert!(
        !shown.contains("eyJ") && !shown.contains("tok-sudo"),
        "{shown}"
    );
}
