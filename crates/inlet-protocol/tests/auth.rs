//! Sign-in flows against a mock instance.

mod common;

use std::time::Duration;

use inlet_protocol::{
    altcha, poll_ip_authorization, sign_in_password, sign_in_totp, Handoff, HandoffStatus,
    IpAuthorization, MfaMethod, Secret, Session, SignInResult,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
use wiremock::{Mock, Request, Respond, ResponseTemplate};

fn token_response(token: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "token": token, "user_id": "10", "user": common::user_json("10", "ana")
    }))
}

#[tokio::test]
async fn password_sign_in_returns_token() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(
            json!({"login": "ana@example.org", "password": "correct horse"}),
        ))
        .and(header_exists("X-Fluxer-Client-Properties"))
        .respond_with(token_response("tok-pw"))
        .expect(1)
        .mount(&server)
        .await;
    match sign_in_password(&instance, "ana@example.org", &Secret::new("correct horse"))
        .await
        .unwrap()
    {
        SignInResult::Ok(s) => {
            assert_eq!(s.token.expose_secret(), "tok-pw");
            assert_eq!(s.user_id, "10");
            assert_eq!(s.user.unwrap().username, "ana");
            assert_eq!(format!("{:?}", s.token), "Token(<redacted>)");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn wrong_password_is_an_api_error() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "INVALID_FORM_BODY", "message": "Input Validation Error",
            "errors": [{"path": "password", "code": "INVALID_EMAIL_OR_PASSWORD", "message": "Invalid"}]
        })))
        .mount(&server)
        .await;
    match sign_in_password(&instance, "ana@example.org", &Secret::new("nope nope"))
        .await
        .unwrap()
    {
        SignInResult::Error(e) => assert_eq!(e.field_code(), Some("INVALID_EMAIL_OR_PASSWORD")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn mfa_then_totp() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mfa": true, "ticket": "mfa-ticket-1", "allowed_methods": ["totp", "webauthn", "backup_codes"],
            "totp": true, "webauthn": true, "backup_codes": true
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/login/mfa/totp"))
        .and(body_json(
            json!({"ticket": "mfa-ticket-1", "code": "123456"}),
        ))
        .respond_with(token_response("tok-mfa"))
        .expect(1)
        .mount(&server)
        .await;
    let (ticket, methods) =
        match sign_in_password(&instance, "ana@example.org", &Secret::new("pw-pw-pw-pw"))
            .await
            .unwrap()
        {
            SignInResult::NeedsMfa { ticket, methods } => (ticket, methods),
            other => panic!("{other:?}"),
        };
    assert_eq!(
        methods,
        vec![MfaMethod::Totp, MfaMethod::Webauthn, MfaMethod::BackupCodes]
    );
    assert_eq!(format!("{ticket:?}"), "Ticket(<redacted>)");
    match sign_in_totp(&instance, &ticket, &Secret::new(" 123456 "))
        .await
        .unwrap()
    {
        SignInResult::Ok(s) => assert_eq!(s.token.expose_secret(), "tok-mfa"),
        other => panic!("{other:?}"),
    }
}

/// Answers like Fluxer: 400 CAPTCHA_REQUIRED without a token, checks the token otherwise.
struct CaptchaGate {
    challenge: serde_json::Value,
}

impl Respond for CaptchaGate {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        match req
            .headers
            .get("X-Captcha-Token")
            .and_then(|v| v.to_str().ok())
        {
            None => ResponseTemplate::new(400).set_body_json(json!({
                "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
                "captcha_provider": "altcha", "altcha_challenge": self.challenge
            })),
            Some(t) if altcha::verify_test_token(t) => token_response("tok-captcha"),
            Some(_) => ResponseTemplate::new(400).set_body_json(json!({
                "code": "INVALID_CAPTCHA", "message": "bad", "captcha_provider": "altcha",
                "altcha_challenge": self.challenge
            })),
        }
    }
}

#[tokio::test]
async fn captcha_is_solved_and_retried() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(CaptchaGate {
            challenge: altcha::make_test_challenge(700, 100),
        })
        .expect(2)
        .mount(&server)
        .await;
    match sign_in_password(&instance, "ana@example.org", &Secret::new("correct horse"))
        .await
        .unwrap()
    {
        SignInResult::Ok(s) => assert_eq!(s.token.expose_secret(), "tok-captcha"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn rejected_captcha_reports_captcha() {
    let (server, instance) = common::instance().await;
    // The server always wants a new captcha: the client gives up after two solves.
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
            "captcha_provider": "altcha", "altcha_challenge": altcha::make_test_challenge(3, 10)
        })))
        .expect(3)
        .mount(&server)
        .await;
    let result = sign_in_password(&instance, "ana@example.org", &Secret::new("correct horse"))
        .await
        .unwrap();
    assert!(matches!(result, SignInResult::Captcha), "{result:?}");
}

#[tokio::test]
async fn new_ip_authorization_polls_to_token() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "code": "IP_AUTHORIZATION_REQUIRED", "message": "Check your email", "ticket": "ip-ticket", "email": "a***@example.org"
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .and(query_param("ticket", "ip-ticket"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed": false})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "completed": true, "token": "tok-ip", "user_id": "10", "user": common::user_json("10", "ana")
        })))
        .with_priority(2)
        .mount(&server)
        .await;
    let ticket = match sign_in_password(&instance, "ana@example.org", &Secret::new("correct horse"))
        .await
        .unwrap()
    {
        SignInResult::IpAuthorizationRequired {
            ticket,
            email,
            resend_available_in,
        } => {
            assert_eq!(email.as_deref(), Some("a***@example.org"));
            assert_eq!(resend_available_in, 30);
            ticket
        }
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        poll_ip_authorization(&instance, &ticket).await.unwrap(),
        IpAuthorization::Pending
    ));
    match poll_ip_authorization(&instance, &ticket).await.unwrap() {
        IpAuthorization::Completed(s) => assert_eq!(s.token.expose_secret(), "tok-ip"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn browser_handoff_flow() {
    let (server, instance) = common::instance().await;
    let web_origin = server.uri();
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/handoff/initiate"))
        .and(header("Origin", web_origin.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "ABCDEFGHJKLM", "expires_at": "2026-11-10T09:05:00Z",
            "poll_secret": "secret_poll-1", "return_method": "code"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/handoff/ABCDEFGHJKLM/status"))
        .and(body_json(json!({"poll_secret": "secret_poll-1"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "pending"})))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/handoff/ABCDEFGHJKLM/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "completed", "token": "tok-handoff", "user_id": "10", "user": common::user_json("10", "ana")
        })))
        .with_priority(2)
        .mount(&server)
        .await;

    let handoff = Handoff::start(&instance).await.unwrap();
    assert_eq!(handoff.display_code(), "ABCDEF-GHJKLM");
    assert_eq!(
        handoff.browser_url(),
        format!("{}/login?handoff=1", server.uri())
    );
    assert!(!format!("{handoff:?}").contains("secret_poll-1"));
    let status = handoff.wait(Duration::from_millis(1)).await.unwrap();
    let signed_in = match status {
        HandoffStatus::Completed(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(signed_in.token.expose_secret(), "tok-handoff");

    // The token makes a working session.
    Mock::given(path("/api/v1/users/@me"))
        .and(header("Authorization", "tok-handoff"))
        .respond_with(ResponseTemplate::new(200).set_body_json(common::user_json("10", "ana")))
        .mount(&server)
        .await;
    let session = Session::from_signed_in(instance, signed_in);
    assert_eq!(session.client().me().await.unwrap().id, "10");
}

#[tokio::test]
async fn handoff_denied_and_cancel() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/handoff/initiate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "QWERTYUIOPAS", "expires_at": "2026-11-10T09:05:00Z", "poll_secret": "s2"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/handoff/QWERTYUIOPAS/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "denied"})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/auth/handoff/QWERTYUIOPAS"))
        .and(body_json(json!({"poll_secret": "s2"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let handoff = Handoff::start(&instance).await.unwrap();
    assert!(matches!(
        handoff.poll().await.unwrap(),
        HandoffStatus::Denied
    ));
    handoff.cancel().await.unwrap();
}

// ---------- identifier-first sign-in, email codes and the second factor ----------

fn identify_json(
    account: &str,
    method: Option<&str>,
    registration: serde_json::Value,
) -> serde_json::Value {
    json!({
        "ticket": "identify-ticket-1", "account": account,
        "sign_in_method": method, "registration": registration
    })
}

#[tokio::test]
async fn identify_sends_the_email_and_reads_how_the_account_signs_in() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/identify"))
        .and(body_json(json!({"email": "ana@example.org"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(identify_json(
            "existing",
            Some("password"),
            json!(null),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let found = inlet_protocol::identify(&instance, " ana@example.org ")
        .await
        .unwrap();
    assert!(found.existing);
    assert_eq!(found.method, Some(inlet_protocol::SignInMethod::Password));
    assert_eq!(format!("{:?}", found.ticket), "Ticket(<redacted>)");
}

#[tokio::test]
async fn identify_sends_the_username_on_a_username_instance() {
    let (server, instance) =
        common::instance_with_features(Some(json!({"account_identity": "username"}))).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/identify"))
        .and(body_json(json!({"login": "ana"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(identify_json(
            "new",
            None,
            json!({"mode": "closed", "consent_required": false, "requires_password": true}),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let found = inlet_protocol::identify(&instance, "ana").await.unwrap();
    assert!(!found.existing);
    assert_eq!(found.method, None);
    assert_eq!(found.registration_mode.as_deref(), Some("closed"));
}

#[tokio::test]
async fn identify_is_refused_when_the_instance_is_not_identifier_first() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/identify"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "code": "FEATURE_TEMPORARILY_DISABLED", "message": "This feature is turned off"
        })))
        .mount(&server)
        .await;
    match inlet_protocol::identify(&instance, "ana@example.org")
        .await
        .unwrap_err()
    {
        inlet_protocol::Error::Api(e) => {
            assert_eq!(e.status, 403);
            assert_eq!(e.code, "FEATURE_TEMPORARILY_DISABLED");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn identify_solves_a_captcha_after_heavy_use() {
    let (server, instance) = common::instance().await;
    struct Gate(serde_json::Value);
    impl Respond for Gate {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            match req
                .headers
                .get("X-Captcha-Token")
                .and_then(|v| v.to_str().ok())
            {
                Some(t) if altcha::verify_test_token(t) => ResponseTemplate::new(200)
                    .set_body_json(identify_json("existing", Some("code"), json!(null))),
                _ => ResponseTemplate::new(400).set_body_json(json!({
                    "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
                    "captcha_provider": "altcha", "altcha_challenge": self.0
                })),
            }
        }
    }
    Mock::given(path("/api/v1/auth/identify"))
        .respond_with(Gate(altcha::make_test_challenge(300, 50)))
        .expect(2)
        .mount(&server)
        .await;
    let found = inlet_protocol::identify(&instance, "ana@example.org")
        .await
        .unwrap();
    assert_eq!(found.method, Some(inlet_protocol::SignInMethod::Code));
}

#[tokio::test]
async fn an_unaccepted_captcha_on_identify_is_a_captcha_error() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/identify"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "CAPTCHA_REQUIRED", "message": "Captcha required",
            "captcha_provider": "altcha", "altcha_challenge": altcha::make_test_challenge(3, 10)
        })))
        .mount(&server)
        .await;
    let err = inlet_protocol::identify(&instance, "ana@example.org")
        .await
        .unwrap_err();
    assert!(matches!(err, inlet_protocol::Error::Captcha(_)), "{err:?}");
}

#[tokio::test]
async fn an_email_code_is_sent_for_the_ticket_and_verified() {
    let (server, instance) = common::instance().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/identify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(identify_json(
            "existing",
            Some("code"),
            json!(null),
        )))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/email-code/send"))
        .and(body_json(json!({"ticket": "identify-ticket-1"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "purpose": "sign_in", "code_expires_at": "2026-11-16T10:10:00.000Z",
            "resend_available_at": "2026-11-16T10:00:30.000Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/email-code/verify"))
        .and(body_json(
            json!({"ticket": "identify-ticket-1", "code": "123456", "consent": false}),
        ))
        .respond_with(token_response("tok-code"))
        .expect(1)
        .mount(&server)
        .await;
    let found = inlet_protocol::identify(&instance, "ana@example.org")
        .await
        .unwrap();
    let sent = inlet_protocol::send_email_code(&instance, &found.ticket)
        .await
        .unwrap();
    assert_eq!(sent.purpose, inlet_protocol::CodePurpose::SignIn);
    assert_eq!(sent.resend_at, "2026-11-16T10:00:30.000Z");
    match inlet_protocol::verify_email_code(&instance, &found.ticket, &Secret::new(" 123456 "))
        .await
        .unwrap()
    {
        SignInResult::Ok(s) => assert_eq!(s.token.expose_secret(), "tok-code"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_wrong_email_code_is_an_api_refusal() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/email-code/verify"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "INVALID_SIGN_IN_CODE", "message": "That code is not right"
        })))
        .mount(&server)
        .await;
    match inlet_protocol::verify_email_code(
        &instance,
        &inlet_protocol::Ticket::for_tests("t"),
        &Secret::new("000000"),
    )
    .await
    .unwrap()
    {
        SignInResult::Error(e) => assert_eq!(e.code, "INVALID_SIGN_IN_CODE"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_email_code_for_an_account_with_two_factor_asks_for_the_second_factor() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/email-code/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mfa": true, "ticket": "mfa-ticket-2", "allowed_methods": ["totp"],
            "totp": true, "webauthn": false, "backup_codes": false
        })))
        .mount(&server)
        .await;
    match inlet_protocol::verify_email_code(
        &instance,
        &inlet_protocol::Ticket::for_tests("t"),
        &Secret::new("123456"),
    )
    .await
    .unwrap()
    {
        SignInResult::NeedsMfa { methods, .. } => assert_eq!(methods, vec![MfaMethod::Totp]),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_email_code_cooldown_is_a_rate_limit_with_its_wait() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/email-code/send"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "code": "EMAIL_CODE_RESEND_COOLDOWN", "message": "Wait", "retry_after": 28
        })))
        .expect(1)
        .mount(&server)
        .await;
    match inlet_protocol::send_email_code(&instance, &inlet_protocol::Ticket::for_tests("t"))
        .await
        .unwrap_err()
    {
        inlet_protocol::Error::RateLimited { retry_after } => {
            assert_eq!(retry_after, Duration::from_secs(28));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_account_with_a_password_cannot_get_an_email_code() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/email-code/send"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "PASSWORD_SIGN_IN_REQUIRED", "message": "Use your password"
        })))
        .mount(&server)
        .await;
    match inlet_protocol::send_email_code(&instance, &inlet_protocol::Ticket::for_tests("t"))
        .await
        .unwrap_err()
    {
        inlet_protocol::Error::Api(e) => assert_eq!(e.code, "PASSWORD_SIGN_IN_REQUIRED"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_backup_code_goes_to_the_same_route_as_an_authenticator_code() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login/mfa/totp"))
        .and(body_json(
            json!({"ticket": "mfa-ticket-3", "code": "abcd-efgh"}),
        ))
        .respond_with(token_response("tok-backup"))
        .expect(1)
        .mount(&server)
        .await;
    let ticket = inlet_protocol::Ticket::for_tests("mfa-ticket-3");
    match sign_in_totp(&instance, &ticket, &Secret::new("abcd-efgh"))
        .await
        .unwrap()
    {
        SignInResult::Ok(s) => assert_eq!(s.token.expose_secret(), "tok-backup"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_wrong_authenticator_code_keeps_its_field_code() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login/mfa/totp"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "INVALID_FORM_BODY", "message": "Input Validation Error",
            "errors": [{"path": "code", "code": "INVALID_CODE", "message": "Invalid code"}]
        })))
        .mount(&server)
        .await;
    let ticket = inlet_protocol::Ticket::for_tests("t");
    match sign_in_totp(&instance, &ticket, &Secret::new("123456"))
        .await
        .unwrap()
    {
        SignInResult::Error(e) => assert_eq!(e.field_code(), Some("INVALID_CODE")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_new_ip_challenge_may_carry_its_fields_under_details() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "code": "IP_AUTHORIZATION_REQUIRED", "message": "Check your email",
            "details": {"context": {
                "ticket": "ip-ticket-2", "email": "ana@example.org", "resend_available_in": 12
            }}
        })))
        .mount(&server)
        .await;
    match sign_in_password(&instance, "ana", &Secret::new("correct horse"))
        .await
        .unwrap()
    {
        SignInResult::IpAuthorizationRequired {
            ticket,
            email,
            resend_available_in,
        } => {
            assert_eq!(ticket.expose_secret(), "ip-ticket-2");
            assert_eq!(email.as_deref(), Some("ana@example.org"));
            assert_eq!(resend_available_in, 12);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_new_ip_email_can_be_sent_again_once() {
    let (server, instance) = common::instance().await;
    Mock::given(path("/api/v1/auth/ip-authorization/resend"))
        .and(body_json(json!({"ticket": "ip-ticket"})))
        .respond_with(ResponseTemplate::new(204))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/ip-authorization/resend"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": "IP_AUTHORIZATION_RESEND_LIMIT_EXCEEDED", "message": "Already resent"
        })))
        .with_priority(2)
        .mount(&server)
        .await;
    let ticket = inlet_protocol::Ticket::for_tests("ip-ticket");
    inlet_protocol::resend_ip_authorization(&instance, &ticket)
        .await
        .unwrap();
    match inlet_protocol::resend_ip_authorization(&instance, &ticket)
        .await
        .unwrap_err()
    {
        inlet_protocol::Error::Api(e) => {
            assert_eq!(e.code, "IP_AUTHORIZATION_RESEND_LIMIT_EXCEEDED")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn sign_in_options_follow_the_discovery_document() {
    let (_server, instance) = common::instance_with_features(Some(json!({
        "account_identity": "username", "identifier_first": true, "email_codes": true,
        "emails_enabled": true
    })))
    .await;
    let o = instance.discovery().sign_in_options();
    assert_eq!(o.identity, inlet_protocol::Identity::Username);
    assert!(o.identifier_first && o.email_codes && o.emails_enabled && o.captcha);
    assert!(!o.sso_enabled && !o.sso_enforced);
    // A document without the flags means email, classic form, no codes.
    let (_server, plain) = common::instance_with_features(Some(json!({}))).await;
    let o = plain.discovery().sign_in_options();
    assert_eq!(o.identity, inlet_protocol::Identity::Email);
    assert!(!o.identifier_first && !o.email_codes);
}
