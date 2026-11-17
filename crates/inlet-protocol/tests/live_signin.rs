//! The sign-in routes against the private test instance `fluxer.test` (LAN only, private
//! test CA). It needs no account: every request is one that must be refused, so the
//! shapes of the refusals the app relies on are checked against a real Fluxer.
//!
//! Needs the feature `test-extra-ca`, `INLET_TEST_EXTRA_CA_FILE` pointing at the
//! instance's `ca.crt`, and a name mapping. Ignored by default; run with
//! `scripts/docker-build.sh fluxer-test -p inlet-protocol --features test-extra-ca --test live_signin -- --ignored --nocapture`.
//!
//! The instance signs in with a username, has no email and no CAPTCHA, and its
//! registration is closed. A different test instance may differ; the checks that depend
//! on that read the instance's own discovery document first.
#![cfg(feature = "test-extra-ca")]

use inlet_protocol::{
    identify, poll_ip_authorization, resend_ip_authorization, send_email_code, sign_in_password,
    sign_in_totp, verify_email_code, Error, Identity, Instance, Secret, SignInResult, Ticket,
};

fn origin() -> String {
    std::env::var("INLET_TEST_ORIGIN").unwrap_or_else(|_| "fluxer.test".into())
}

async fn instance() -> Instance {
    assert!(
        std::env::var_os(inlet_protocol::test_ca::ENV).is_some(),
        "set {} to the test CA",
        inlet_protocol::test_ca::ENV
    );
    Instance::discover(&origin()).await.expect("discovery")
}

fn api_code(err: &Error) -> (u16, String) {
    match err {
        Error::Api(api) => (
            api.status,
            api.field_code().unwrap_or(&api.code).to_string(),
        ),
        other => panic!("expected an API refusal, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn the_test_instance_says_how_people_sign_in() {
    let options = instance().await.discovery().sign_in_options();
    println!("{options:?}");
    assert_eq!(options.identity, Identity::Username);
    assert!(!options.sso_enforced);
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn a_wrong_password_for_a_name_nobody_has_is_refused_like_a_wrong_password() {
    let instance = instance().await;
    let result = sign_in_password(
        &instance,
        "inlet_nobody_has_this_name",
        &Secret::new("not the password"),
    )
    .await
    .expect("the request went through");
    match result {
        SignInResult::Error(api) => {
            println!("{} {} {:?}", api.status, api.code, api.field_code());
            assert_eq!(api.status, 400);
            let code = api.field_code().unwrap_or(&api.code).to_string();
            assert!(
                code == "INVALID_LOGIN_OR_PASSWORD" || code == "INVALID_EMAIL_OR_PASSWORD",
                "{code}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn identify_answers_with_a_refusal_when_the_instance_is_not_identifier_first() {
    let instance = instance().await;
    if instance.discovery().sign_in_options().identifier_first {
        return;
    }
    let err = identify(&instance, "inlet_nobody_has_this_name")
        .await
        .unwrap_err();
    let (status, code) = api_code(&err);
    println!("{status} {code}");
    assert_eq!(status, 403);
    assert_eq!(code, "FEATURE_TEMPORARILY_DISABLED");
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn a_second_factor_ticket_that_does_not_exist_is_refused() {
    let instance = instance().await;
    let result = sign_in_totp(
        &instance,
        &Ticket::for_tests("a-ticket-nobody-issued"),
        &Secret::new("123456"),
    )
    .await
    .expect("the request went through");
    match result {
        SignInResult::Error(api) => {
            println!("{} {} {:?}", api.status, api.code, api.field_code());
            assert!((400..500).contains(&api.status));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn an_email_code_ticket_that_does_not_exist_is_refused() {
    let instance = instance().await;
    let ticket = Ticket::for_tests("a-ticket-nobody-issued");
    let sent = send_email_code(&instance, &ticket).await.unwrap_err();
    let (status, code) = api_code(&sent);
    println!("send: {status} {code}");
    assert!((400..500).contains(&status));
    match verify_email_code(&instance, &ticket, &Secret::new("123456"))
        .await
        .expect("the request went through")
    {
        SignInResult::Error(api) => {
            println!("verify: {} {} {:?}", api.status, api.code, api.field_code());
            assert!((400..500).contains(&api.status));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn a_new_network_ticket_that_does_not_exist_is_refused_for_the_poll_and_the_resend() {
    let instance = instance().await;
    let ticket = Ticket::for_tests("a-ticket-nobody-issued");
    let polled = poll_ip_authorization(&instance, &ticket).await.unwrap_err();
    let (status, code) = api_code(&polled);
    println!("poll: {status} {code}");
    assert_eq!(status, 400);
    assert_eq!(code, "INVALID_OR_EXPIRED_AUTHORIZATION_TICKET");
    let resent = resend_ip_authorization(&instance, &ticket)
        .await
        .unwrap_err();
    let (status, code) = api_code(&resent);
    println!("resend: {status} {code}");
    assert!((400..500).contains(&status));
}
