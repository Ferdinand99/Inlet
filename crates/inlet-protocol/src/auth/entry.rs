//! Identifier-first sign-in: look the account up, then sign in with its password or an
//! emailed code. `POST /auth/identify`, `/auth/email-code/send`, `/auth/email-code/verify`.
//!
//! These routes only work on an instance with `features.identifier_first` (otherwise
//! `identify` fails with 403 `FEATURE_TEMPORARILY_DISABLED`). They share the ALTCHA
//! handling of the password route: a gated request is solved and sent again. The
//! sign-up half of Fluxer's flow (a new address) is not done here; [`Identified`]
//! says an account is new and the caller stops.

use serde_json::{json, Value};

use super::{interpret_login, post_with_captcha, wipe, SignInResult, Ticket};
use crate::error::{Error, Result};
use crate::instance::{Identity, Instance};
use crate::secret::Secret;

/// How an existing account signs in, as `identify` says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignInMethod {
    /// The account has a password.
    Password,
    /// The account has no password: an emailed code.
    Code,
    /// Something newer.
    Other(String),
}

/// The answer of [`identify`].
#[derive(Debug)]
pub struct Identified {
    /// For the next step (valid 15 minutes).
    pub ticket: Ticket,
    /// Whether an account uses this identifier. `false`: it is new.
    pub existing: bool,
    /// How an existing account signs in.
    pub method: Option<SignInMethod>,
    /// For a new identifier: how the instance lets new accounts in (`open`,
    /// `approval` or `closed`).
    pub registration_mode: Option<String>,
}

/// Why a code was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePurpose {
    /// Signing in to an existing account.
    SignIn,
    /// Creating an account for a new address.
    SignUp,
}

/// The answer of [`send_email_code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSent {
    /// Sign-in or sign-up.
    pub purpose: CodePurpose,
    /// When the code stops working (ISO 8601), 10 minutes after sending.
    pub expires_at: String,
    /// When another code can be asked for (ISO 8601), 30 seconds after sending.
    pub resend_at: String,
}

fn api_failure(raw: &crate::rest::RawResponse) -> Error {
    Error::Api(Box::new(raw.api_error()))
}

fn gated(raw: crate::rest::RawResponse, captcha_failed: bool) -> Result<crate::rest::RawResponse> {
    if captcha_failed {
        return Err(Error::Captcha(
            "the instance did not accept the CAPTCHA answer".into(),
        ));
    }
    Ok(raw)
}

fn short(v: &Value, key: &str, max: usize) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .map(|s| s.chars().take(max).collect())
        .unwrap_or_default()
}

/// `POST /auth/identify`: looks up an email address (or a username on a username
/// instance). The identifier is sent as `email` or `login` to match the instance.
pub async fn identify(instance: &Instance, identifier: &str) -> Result<Identified> {
    let key = match instance.discovery().sign_in_options().identity {
        Identity::Email => "email",
        Identity::Username => "login",
    };
    let body = json!({ key: identifier.trim() });
    let (raw, captcha_failed) = post_with_captcha(instance, "/auth/identify", &body).await?;
    let raw = gated(raw, captcha_failed)?;
    if !raw.is_success() {
        return Err(api_failure(&raw));
    }
    let v = raw.value();
    let ticket = v
        .get("ticket")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty() && t.len() <= 256)
        .ok_or_else(|| Error::Decode("identify answered without a ticket".into()))?;
    let existing = match v.get("account").and_then(Value::as_str) {
        Some("existing") => true,
        Some("new") => false,
        _ => return Err(Error::Decode("identify answered without an account".into())),
    };
    let method = v
        .get("sign_in_method")
        .and_then(Value::as_str)
        .map(|m| match m {
            "password" => SignInMethod::Password,
            "code" => SignInMethod::Code,
            other => SignInMethod::Other(other.chars().take(32).collect()),
        });
    let registration_mode = v
        .get("registration")
        .filter(|r| !r.is_null())
        .map(|r| short(r, "mode", 16));
    Ok(Identified {
        ticket: Ticket(ticket.to_owned()),
        existing,
        method,
        registration_mode,
    })
}

/// `POST /auth/email-code/send`: emails a 6-digit code for an identify ticket. The
/// refusals are API errors: `EMAIL_CODES_UNAVAILABLE`, `UNKNOWN_SIGN_IN_TICKET`,
/// `PASSWORD_SIGN_IN_REQUIRED` (the account has a password) and, as
/// [`Error::RateLimited`], `EMAIL_CODE_RESEND_COOLDOWN`.
pub async fn send_email_code(instance: &Instance, ticket: &Ticket) -> Result<CodeSent> {
    let mut body = json!({ "ticket": ticket.expose_secret() });
    let sent = post_with_captcha(instance, "/auth/email-code/send", &body).await;
    wipe(&mut body, &["ticket"]);
    let (raw, captcha_failed) = sent?;
    let raw = gated(raw, captcha_failed)?;
    if !raw.is_success() {
        return Err(api_failure(&raw));
    }
    let v = raw.value();
    Ok(CodeSent {
        purpose: match v.get("purpose").and_then(Value::as_str) {
            Some("sign_up") => CodePurpose::SignUp,
            _ => CodePurpose::SignIn,
        },
        expires_at: short(&v, "code_expires_at", 40),
        resend_at: short(&v, "resend_available_at", 40),
    })
}

/// `POST /auth/email-code/verify`: checks the emailed code and signs in to the existing
/// account. Returns like a password sign-in: a token, a second-factor step, or a refusal
/// (`INVALID_SIGN_IN_CODE`, `SIGN_IN_CODE_EXPIRED`). A code allows 5 tries.
pub async fn verify_email_code(
    instance: &Instance,
    ticket: &Ticket,
    code: &Secret,
) -> Result<SignInResult> {
    verify_email_code_with_invite(instance, ticket, code, None).await
}

/// [`verify_email_code`] for a sign-in that starts from an invite.
pub async fn verify_email_code_with_invite(
    instance: &Instance,
    ticket: &Ticket,
    code: &Secret,
    invite_code: Option<&str>,
) -> Result<SignInResult> {
    // The server takes exactly six digits; a code pasted as "123 456" or "123-456" is
    // the same code (Fluxer's client keeps the digits too).
    let mut digits = String::with_capacity(6);
    digits.extend(
        code.expose_secret()
            .chars()
            .filter(char::is_ascii_digit)
            .take(6),
    );
    let mut body = json!({
        "ticket": ticket.expose_secret(),
        "code": digits,
        // Signing in only: a sign-up needs the terms, which this flow never accepts.
        "consent": false,
    });
    super::with_invite(&mut body, invite_code);
    let sent = post_with_captcha(instance, "/auth/email-code/verify", &body).await;
    wipe(&mut body, &["code", "ticket"]);
    let (raw, captcha_failed) = sent?;
    interpret_login(raw, captcha_failed)
}
