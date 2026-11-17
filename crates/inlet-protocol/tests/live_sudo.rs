//! Sudo mode against the private test instance `fluxer.test` (LAN only, private test CA).
//! Needs the feature `test-extra-ca` and the variables `scripts/docker-build.sh
//! fluxer-test` sets. Ignored by default; run with
//! `scripts/docker-build.sh fluxer-test -p inlet-protocol --test live_sudo`.
//!
//! The test makes its own throwaway account (random name and password, kept in memory;
//! registration is open on the test instance) and deletes it at the end. It reads no
//! file and uses no real account.
#![cfg(feature = "test-extra-ca")]

use inlet_protocol::test_support::{base32, http, random_hex, totp, totp_step_now, Throwaway};
use inlet_protocol::{Client, Error, Instance, Method, SudoProof, SudoRejection, Token};
use serde_json::{json, Value};

fn origin() -> String {
    std::env::var("INLET_TEST_ORIGIN").unwrap_or_else(|_| "fluxer.test".into())
}

const KIT: &str = "/users/@me/recovery-kit";

#[tokio::test]
#[ignore = "talks to the private test instance"]
async fn sudo_mode_with_a_password_then_a_code_then_a_backup_code() {
    let instance = Instance::discover(&origin()).await.expect("discovery");
    let base = instance.api_base().to_string();
    let http = http();
    let account = Throwaway::register(&http, &base, "/auth/register", "inlet_f1a").await;
    let client = Client::new(instance.clone(), Token::new(account.token.clone()));
    client.me().await.expect("the new token works");

    // 1. A password-only account: the action asks, a wrong password is named, the right
    //    one works and no token comes back (Fluxer gives one only with a second factor).
    let err = client
        .sudo_request(Method::POST, KIT, None, None)
        .await
        .unwrap_err();
    let Error::SudoRequired(required) = err else {
        panic!("expected SudoRequired, got {err:?}");
    };
    println!("password-only account: {required:?}");
    assert!(required.methods.password && !required.methods.totp && !required.has_mfa);

    let err = client
        .sudo_request(
            Method::POST,
            KIT,
            None,
            Some(&SudoProof::password(random_hex(10))),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::SudoRejected(SudoRejection::WrongPassword)),
        "{err:?}"
    );
    let kit = client
        .sudo_request(
            Method::POST,
            KIT,
            None,
            Some(&SudoProof::password(account.password.clone())),
        )
        .await
        .expect("the right password");
    assert!(kit.is_object(), "{kit}");
    assert!(!client.has_sudo_token(), "no token without a second factor");

    // 2. Turn TOTP on (the password is the proof), and keep a backup code.
    let secret: Vec<u8> = (0..20)
        .map(|i| u8::from_str_radix(&random_hex(20)[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    let step = totp_step_now();
    let enabled = client
        .sudo_request(
            Method::POST,
            "/users/@me/mfa/totp/enable",
            Some(&json!({"secret": base32(&secret), "code": totp(&secret, step)})),
            Some(&SudoProof::password(account.password.clone())),
        )
        .await
        .expect("enabling TOTP");
    let backup = backup_code(&enabled);

    // 3. Now the account has a second factor: the password is not a way in any more, and
    //    the instance says what is.
    let err = client
        .sudo_request(
            Method::POST,
            KIT,
            None,
            Some(&SudoProof::password(account.password.clone())),
        )
        .await
        .unwrap_err();
    let Error::SudoRequired(required) = err else {
        panic!("expected SudoRequired, got {err:?}");
    };
    println!("account with TOTP: {required:?}");
    assert!(required.has_mfa && required.methods.totp && !required.methods.password);

    // 4. A code gives a token; the next action needs no proof.
    let err = client
        .sudo_request(Method::POST, KIT, None, Some(&SudoProof::code("000000")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::SudoRejected(SudoRejection::WrongCode)),
        "{err:?}"
    );
    client
        .sudo_request(
            Method::POST,
            KIT,
            None,
            Some(&SudoProof::code(totp(&secret, step + 1))),
        )
        .await
        .expect("a TOTP code");
    assert!(client.has_sudo_token(), "a token after a code");
    client
        .sudo_request(Method::POST, KIT, None, None)
        .await
        .expect("the token is attached by itself");
    // The same token does nothing on routes that are not sudo routes.
    client.me().await.expect("an ordinary route");

    // 5. A backup code works as a proof too (it is spent).
    client.forget_sudo();
    assert!(!client.has_sudo_token());
    let err = client
        .sudo_request(Method::POST, KIT, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SudoRequired(_)));
    client
        .sudo_request(
            Method::POST,
            KIT,
            None,
            Some(&SudoProof::code(backup.clone())),
        )
        .await
        .expect("a backup code");
    assert!(client.has_sudo_token());
    client.forget_sudo();
    let err = client
        .sudo_request(Method::POST, KIT, None, Some(&SudoProof::code(backup)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::SudoRejected(SudoRejection::WrongCode)),
        "a spent backup code: {err:?}"
    );

    // 6. The routes that tell what the account can use.
    let methods = client.sudo_methods().await.expect("mfa-methods");
    assert!(methods.totp);

    // 7. Clean up: delete the account with a code (the instance deletes it at once).
    let gone = client
        .sudo_request(
            Method::POST,
            "/users/@me/delete",
            Some(&json!({"immediate": true})),
            Some(&SudoProof::code(totp(&secret, step.saturating_sub(1)))),
        )
        .await;
    println!("cleanup: {gone:?}");
    if gone.is_err() {
        let status = account
            .delete(
                &http,
                &base,
                "/users/@me/delete",
                json!({"mfa_method": "totp", "mfa_code": totp(&secret, step + 1)}),
            )
            .await;
        println!("cleanup (plain request): {status}");
    }
}

/// The first backup code in the answer of the TOTP enable route.
fn backup_code(enabled: &Value) -> String {
    let codes = enabled["backup_codes"].as_array().expect("backup codes");
    let first = &codes[0];
    first
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| first.as_str())
        .expect("a backup code")
        .to_owned()
}
