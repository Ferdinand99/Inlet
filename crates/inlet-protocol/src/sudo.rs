//! Sudo mode: "confirm it's you" before a dangerous action.
//!
//! Fluxer protects account and server actions (deleting the account, changing the
//! password, turning two-factor on or off, deleting a community, ...) with sudo mode.
//! There is no separate "verify" route. The flow, exactly as Fluxer's own client does it:
//!
//! 1. Send the action. If the account has no valid sudo token, the instance answers
//!    `403 SUDO_MODE_REQUIRED` with the ways the account can confirm itself. Here that
//!    is [`Error::SudoRequired`] ([`SudoRequired::methods`]); the UI asks the person.
//! 2. Send the same action again with the proof in the request body (`password`, or
//!    `mfa_method` + `mfa_code`; see [`SudoProof`]). Use [`Client::sudo_request`], which
//!    adds the proof. A wrong proof is [`Error::SudoRejected`].
//! 3. When the account has a second factor (or a passkey), the instance answers with a
//!    short-lived token in the `X-Fluxer-Sudo-Mode-JWT` response header. The client keeps
//!    it in memory ([`SudoToken`]) and attaches it to the sudo routes for the next
//!    4.5 minutes ([`SUDO_TOKEN_TTL`]; the token itself lives 5), so the person is not
//!    asked again for every action. An account with only a password gets no token and
//!    sends the password with every action.
//!
//! The token is attached **only** to the routes that use it ([`is_sudo_route`]), never to
//! the rest of the API, and it is read from a response only on those routes. It lives in
//! memory, is wiped when dropped, never appears in `Debug`, and is gone with the
//! [`Client`] (sign-out, switching accounts).
//!
//! # Request budget
//!
//! A wrong proof counts against [`MAX_WRONG_PROOFS`] per [`WRONG_PROOF_WINDOW`]; after
//! that [`Client::sudo_request`] refuses to send a proof and says how long to wait
//! ([`Error::RateLimited`]), so the person (or a bug) cannot guess a password or a
//! code at the instance's pace. Fluxer allows 10 wrong second-factor codes in 15
//! minutes; Inlet stops at 5. The e-mailed code is limited by the instance to 5 per 15
//! minutes ([`Client::send_sudo_email_code`]).
//!
//! # Not covered yet
//!
//! Passkeys (WebAuthn) as proof, and the single-sign-on re-authentication
//! (`sso_reauth`); both show up in [`SudoMethods`] so the UI can say what is missing.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::time::Instant;
use zeroize::Zeroize;

use crate::error::{Error, Result};
use crate::rest::{Client, RawResponse};
use crate::secret::Secret;

/// The request header Fluxer's sudo token travels in.
pub const SUDO_HEADER: &str = "X-Fluxer-Sudo-Mode-JWT";
/// How long a received token is used: Fluxer's token is valid for 5 minutes and its own
/// client stops using it after 4.5.
pub const SUDO_TOKEN_TTL: Duration = Duration::from_secs(270);
/// Wrong proofs that may be sent in [`WRONG_PROOF_WINDOW`].
pub const MAX_WRONG_PROOFS: usize = 5;
/// The window of [`MAX_WRONG_PROOFS`].
pub const WRONG_PROOF_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Longest password Fluxer accepts.
const MAX_PASSWORD_CHARS: usize = 256;
/// Longest second-factor or backup code Fluxer accepts.
const MAX_CODE_CHARS: usize = 32;
/// Longest e-mail code ticket Fluxer accepts.
const MAX_TICKET_CHARS: usize = 128;
/// A real token is a JWT of about 200 bytes; anything much larger is not one.
const MAX_TOKEN_BYTES: usize = 2048;

/// The short-lived token that proves the account was confirmed. Wiped when dropped,
/// redacted in `Debug`, no `Display`.
pub struct SudoToken(String);

impl SudoToken {
    /// The raw value, for the one place that sends it. Never log it.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// A token from a header value, if it has the shape of a JWT (three dot-separated
    /// parts of URL-safe characters, not too long). Anything else is not kept.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_TOKEN_BYTES {
            return None;
        }
        let parts: Vec<&str> = value.split('.').collect();
        let ok = parts.len() == 3
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            });
        ok.then(|| Self(value.to_owned()))
    }

    /// A token from a raw value, for tests.
    #[doc(hidden)]
    pub fn for_tests(value: &str) -> SudoToken {
        SudoToken(value.to_owned())
    }
}

impl Clone for SudoToken {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Drop for SudoToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SudoToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SudoToken(<redacted>)")
    }
}

impl PartialEq for SudoToken {
    fn eq(&self, other: &Self) -> bool {
        // Same length and bytes, compared without stopping at the first difference.
        self.0.len() == other.0.len()
            && self
                .0
                .bytes()
                .zip(other.0.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }
}

impl Eq for SudoToken {}

/// What the person typed to confirm it is them.
#[derive(Debug)]
#[non_exhaustive]
pub enum SudoProof {
    /// The account password. Fluxer takes it only when the account has no second factor.
    Password(Secret),
    /// A code from the authenticator app, or an unused backup code. Fluxer takes both as
    /// the same method (`totp`).
    Code(Secret),
    /// The code e-mailed after [`Client::send_sudo_email_code`], for accounts with a
    /// confirmed address and no password or second factor.
    EmailCode {
        /// The ticket that came with the e-mail.
        ticket: Secret,
        /// The six digits from the e-mail.
        code: Secret,
    },
}

impl SudoProof {
    /// A password proof.
    pub fn password(value: impl Into<String>) -> Self {
        SudoProof::Password(Secret::new(value))
    }

    /// An authenticator or backup code proof.
    pub fn code(value: impl Into<String>) -> Self {
        SudoProof::Code(Secret::new(value))
    }

    /// Checks the lengths Fluxer accepts, so a value it would refuse is not sent (and
    /// not counted as a wrong proof).
    fn check(&self) -> Result<()> {
        let bad = |what: &str| Err(Error::InvalidInput(format!("sudo proof: {what}")));
        match self {
            SudoProof::Password(p) => {
                if p.expose_secret().chars().count() == 0
                    || p.expose_secret().chars().count() > MAX_PASSWORD_CHARS {
                    return bad("the password is empty or too long");
                }
            }
            SudoProof::Code(c) => {
                if c.expose_secret().chars().count() == 0
                    || c.expose_secret().chars().count() > MAX_CODE_CHARS {
                    return bad("the code is empty or too long");
                }
            }
            SudoProof::EmailCode { ticket, code } => {
                let code = code.expose_secret();
                let digits = code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit());
                let ticket_chars = ticket.expose_secret().chars().count();
                if ticket_chars == 0 || ticket_chars > MAX_TICKET_CHARS || !digits {
                    return bad("the e-mail code is not six digits");
                }
            }
        }
        Ok(())
    }

    /// Adds the proof to a request body.
    fn put_into(&self, body: &mut Map<String, Value>) {
        match self {
            SudoProof::Password(p) => {
                body.insert("password".into(), Value::String(p.expose_secret().to_owned()));
            }
            SudoProof::Code(c) => {
                body.insert("mfa_method".into(), json!("totp"));
                body.insert("mfa_code".into(), Value::String(c.expose_secret().to_owned()));
            }
            SudoProof::EmailCode { ticket, code } => {
                body.insert("mfa_method".into(), json!("email_code"));
                body.insert("email_code_ticket".into(), Value::String(ticket.expose_secret().to_owned()));
                body.insert("email_code".into(), Value::String(code.expose_secret().to_owned()));
            }
        }
    }
}

/// The ways an account can confirm itself, as the instance reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct SudoMethods {
    /// A code from the authenticator app.
    pub totp: bool,
    /// A passkey or security key (not supported by this crate yet).
    pub webauthn: bool,
    /// An unused backup code (typed like an authenticator code).
    pub backup_codes: bool,
    /// A code sent to the confirmed e-mail address.
    pub email_code: bool,
    /// The account password.
    pub password: bool,
    /// Signing in again with the instance's single sign-on (not supported yet).
    pub sso_reauth: bool,
}

impl SudoMethods {
    /// Whether this crate can send a proof for at least one of the methods
    /// ([`SudoProof`]): a password, an authenticator or backup code, or an e-mailed code.
    pub fn any_supported(&self) -> bool {
        self.password || self.totp || self.backup_codes || self.email_code
    }

    /// Whether a typed code (authenticator or backup) is a way in.
    pub fn takes_code(&self) -> bool {
        self.totp || self.backup_codes
    }
}

/// The instance asked for sudo mode (`403 SUDO_MODE_REQUIRED`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SudoRequired {
    /// Whether the account has a second factor.
    pub has_mfa: bool,
    /// What the account can use to confirm itself.
    pub methods: SudoMethods,
}

impl SudoRequired {
    /// Reads the error body: `has_mfa` and `methods` sit at the top level (Fluxer's own
    /// client reads them there); a copy under `data` is accepted too.
    pub(crate) fn from_body(body: &Value) -> SudoRequired {
        let read = |v: &Value| -> Option<SudoRequired> {
            let methods: SudoMethods = serde_json::from_value(v.get("methods")?.clone()).ok()?;
            Some(SudoRequired {
                has_mfa: v.get("has_mfa").and_then(Value::as_bool).unwrap_or(false),
                methods,
            })
        };
        read(body)
            .or_else(|| body.get("data").and_then(read))
            .unwrap_or(SudoRequired {
                has_mfa: false,
                methods: SudoMethods::default(),
            })
    }
}

/// Why a proof was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoRejection {
    /// The password is wrong (`INVALID_PASSWORD`).
    WrongPassword,
    /// The account has no password (`PASSWORD_NOT_SET`).
    NoPassword,
    /// The authenticator or backup code is wrong, or too many were tried
    /// (`INVALID_MFA_CODE`).
    WrongCode,
    /// The e-mailed code is wrong, expired or unknown.
    WrongEmailCode,
    /// Refused for another reason in the proof's fields.
    Other,
}

/// The e-mail ticket and the times that come with it (`POST /users/@me/sudo/email-code`).
#[derive(Debug)]
pub struct SudoEmailCode {
    /// Send it back with the code ([`SudoProof::EmailCode`]).
    pub ticket: Secret,
    /// When the code stops working (RFC 3339), when the instance said.
    pub code_expires_at: Option<String>,
    /// When another e-mail may be asked for (RFC 3339), when the instance said.
    pub resend_available_at: Option<String>,
}

/// One route that takes sudo mode: a method and a path below `/v1`, where `:` marks a
/// segment that can be anything (an ID).
struct SudoRoute {
    method: Method,
    pattern: &'static str,
}

fn routes() -> &'static [SudoRoute] {
    use std::sync::OnceLock;
    static ROUTES: OnceLock<Vec<SudoRoute>> = OnceLock::new();
    ROUTES.get_or_init(|| {
        let table: &[(Method, &'static str)] = &[
            // The account and its security.
            (Method::PATCH, "/users/@me"),
            (Method::POST, "/users/@me/password"),
            (Method::POST, "/users/@me/password-change/complete"),
            (Method::POST, "/users/@me/email-change/apply"),
            (Method::POST, "/users/@me/disable"),
            (Method::POST, "/users/@me/delete"),
            (Method::POST, "/users/@me/messages/delete"),
            (Method::POST, "/users/@me/messages/bulk-delete-mine"),
            (Method::POST, "/users/@me/mfa/totp/enable"),
            (Method::POST, "/users/@me/mfa/totp/disable"),
            (Method::POST, "/users/@me/mfa/backup-codes"),
            (Method::POST, "/users/@me/recovery-kit"),
            (Method::POST, "/users/@me/sso/link/start"),
            (Method::DELETE, "/users/@me/sso"),
            (Method::DELETE, "/users/@me/authorized-ips"),
            (
                Method::POST,
                "/users/@me/mfa/webauthn/credentials/registration-options",
            ),
            (
                Method::PATCH,
                "/users/@me/mfa/webauthn/credentials/:credential_id",
            ),
            (
                Method::DELETE,
                "/users/@me/mfa/webauthn/credentials/:credential_id",
            ),
            (Method::PUT, "/users/@me/mfa/webauthn/two-factor"),
            (Method::POST, "/auth/sessions/logout"),
            // Communities and channels.
            (Method::DELETE, "/users/@me/guilds/:guild_id"),
            (
                Method::POST,
                "/users/@me/guilds/:guild_id/messages/bulk-delete-mine",
            ),
            (Method::PATCH, "/guilds/:guild_id"),
            (Method::POST, "/guilds/:guild_id/delete"),
            (Method::POST, "/guilds/:guild_id/transfer-ownership"),
            (Method::DELETE, "/channels/:channel_id"),
            (Method::DELETE, "/channels/:channel_id/recipients/:user_id"),
            (
                Method::POST,
                "/channels/:channel_id/messages/bulk-delete-mine",
            ),
            // Applications and purchases.
            (Method::DELETE, "/oauth2/applications/:id"),
            (Method::POST, "/oauth2/applications/:id/bot/reset-token"),
            (Method::POST, "/oauth2/applications/:id/client-secret/reset"),
            (Method::DELETE, "/premium/store/purchases/:purchase_id"),
        ];
        table
            .iter()
            .map(|(method, pattern)| SudoRoute {
                method: method.clone(),
                pattern,
            })
            .collect()
    })
}

/// Whether `method path` is a route that takes sudo mode (a path below `/v1`, with or
/// without a query). Only these routes get the sudo token, a proof in the body, or have
/// the token read from their answer. The list is the routes of Fluxer's API that call
/// its sudo check; a route Fluxer adds later is not in it until it is added here, which
/// fails closed (no token is sent there).
pub fn is_sudo_route(method: &Method, path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    let given: Vec<&str> = rest.split('/').collect();
    routes().iter().any(|route| {
        if route.method != *method {
            return false;
        }
        let wanted: Vec<&str> = route.pattern[1..].split('/').collect();
        wanted.len() == given.len()
            && wanted.iter().zip(&given).all(|(w, g)| {
                if w.starts_with(':') {
                    !g.is_empty() && *g != "." && *g != ".." && !g.contains('%')
                } else {
                    w == g
                }
            })
    })
}

/// The token and the wrong-proof count of one session. Shared by the clones of a
/// [`Client`].
#[derive(Default)]
pub(crate) struct SudoStore {
    inner: Mutex<StoreInner>,
}

#[derive(Default)]
struct StoreInner {
    token: Option<(SudoToken, Instant)>,
    wrong: Vec<Instant>,
}

impl SudoStore {
    /// The token, while it is still valid (an expired one is dropped).
    pub(crate) fn token(&self) -> Option<SudoToken> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &inner.token {
            Some((token, expires)) if Instant::now() < *expires => Some(token.clone()),
            Some(_) => {
                inner.token = None;
                None
            }
            None => None,
        }
    }

    pub(crate) fn set(&self, token: SudoToken) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.token = Some((token, Instant::now() + SUDO_TOKEN_TTL));
    }

    pub(crate) fn clear(&self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).token = None;
    }

    /// How long until another wrong proof may be sent, or `None` when one may.
    fn proof_wait(&self) -> Option<Duration> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        inner
            .wrong
            .retain(|at| now.saturating_duration_since(*at) < WRONG_PROOF_WINDOW);
        if inner.wrong.len() < MAX_WRONG_PROOFS {
            return None;
        }
        let oldest = inner.wrong.iter().min().copied()?;
        Some(WRONG_PROOF_WINDOW.saturating_sub(now.saturating_duration_since(oldest)))
    }

    fn record_wrong(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wrong
            .push(Instant::now());
    }

    fn reset_wrong(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wrong
            .clear();
    }
}

/// Overwrites every string in a JSON value with zeros (the proof was copied into it).
fn wipe(value: &mut Value) {
    match value {
        Value::String(s) => s.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe),
        Value::Object(map) => map.values_mut().for_each(wipe),
        _ => {}
    }
}

/// The reason a `400` answer to a request with a proof gives, if it is about the proof.
pub(crate) fn rejection_of(raw: &RawResponse, proof: &SudoProof) -> Option<SudoRejection> {
    if raw.status != 400 {
        return None;
    }
    let err = raw.api_error();
    if matches!(
        err.code.as_str(),
        "INVALID_SIGN_IN_CODE" | "SIGN_IN_CODE_EXPIRED" | "UNKNOWN_SIGN_IN_TICKET"
    ) {
        return matches!(proof, SudoProof::EmailCode { .. })
            .then_some(SudoRejection::WrongEmailCode);
    }
    if err.code != "INVALID_FORM_BODY" {
        return None;
    }
    let errors = err.errors.as_ref()?.as_array()?;
    let mut found = None;
    for e in errors {
        let path = e
            .get("path")
            .or_else(|| e.get("field"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let code = e.get("code").and_then(Value::as_str).unwrap_or("");
        let reason = match (path, code) {
            ("password", "INVALID_PASSWORD") => SudoRejection::WrongPassword,
            ("password", "PASSWORD_NOT_SET") => SudoRejection::NoPassword,
            ("mfa_code" | "mfa_method", _) => SudoRejection::WrongCode,
            ("password" | "webauthn_response" | "webauthn_challenge", _) => SudoRejection::Other,
            _ => continue,
        };
        found = Some(reason);
        break;
    }
    found
}

impl Client {
    /// The sudo token this session holds, while it is valid. Mostly for tests and for a
    /// UI that wants to know whether the next action will ask.
    pub fn has_sudo_token(&self) -> bool {
        self.sudo_store().token().is_some()
    }

    /// Forgets the sudo token now (for example when the person locks the app).
    pub fn forget_sudo(&self) {
        self.sudo_store().clear();
    }

    /// Sends a request to a sudo route ([`is_sudo_route`]), with the proof in the body
    /// when there is one. Without a proof the session's sudo token is used if it has
    /// one. Returns the parsed JSON (or `Value::Null` for 204).
    ///
    /// - [`Error::SudoRequired`]: the instance wants a proof; ask the person and call
    ///   again with it.
    /// - [`Error::SudoRejected`]: the proof was wrong. After [`MAX_WRONG_PROOFS`] wrong
    ///   ones in [`WRONG_PROOF_WINDOW`], no further proof is sent ([`Error::RateLimited`]).
    ///
    /// A token from the answer is kept for the next requests. `body` must be a JSON
    /// object (or none); the proof is added to a copy that is wiped after the request.
    pub async fn sudo_request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        proof: Option<&SudoProof>,
    ) -> Result<Value> {
        if !is_sudo_route(&method, path) {
            // A password must never go to a route that does not ask for it.
            return Err(Error::InvalidInput(format!(
                "{method} {path} is not a sudo route"
            )));
        }
        let Some(proof) = proof else {
            return self.request_json(method, path, body).await;
        };
        proof.check()?;
        if let Some(wait) = self.sudo_store().proof_wait() {
            return Err(Error::RateLimited { retry_after: wait });
        }
        let mut map = match body {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => {
                return Err(Error::InvalidInput(
                    "a sudo request body must be a JSON object".into(),
                ))
            }
        };
        proof.put_into(&mut map);
        let mut full = Value::Object(map);
        // The proof stands in for the token: it is what the person just confirmed.
        let result = self.sudo_send(method, path, &full, proof).await;
        wipe(&mut full);
        let raw = result?;
        if raw.body.is_empty() {
            return Ok(Value::Null);
        }
        raw.json()
    }

    async fn sudo_send(
        &self,
        method: Method,
        path: &str,
        body: &Value,
        proof: &SudoProof,
    ) -> Result<RawResponse> {
        let raw = self.send_for_sudo(method, path, Some(body), false).await?;
        if raw.is_success() {
            self.sudo_store().reset_wrong();
            return Ok(raw);
        }
        if let Some(reason) = rejection_of(&raw, proof) {
            self.sudo_store().record_wrong();
            return Err(Error::SudoRejected(reason));
        }
        Err(self.error_of(&raw))
    }

    /// `GET /users/@me/sudo/mfa-methods`: what the account can use to confirm itself.
    pub async fn sudo_methods(&self) -> Result<SudoMethods> {
        let raw = self
            .raw(Method::GET, "/users/@me/sudo/mfa-methods", None)
            .await?;
        raw.json()
    }

    /// `POST /users/@me/sudo/email-code`: e-mails a six-digit code to the account's
    /// confirmed address. Only for accounts without a second factor; the instance allows
    /// 5 of these in 15 minutes, so the UI must not offer it again at once (the answer's
    /// `resend_available_at` says when).
    pub async fn send_sudo_email_code(&self) -> Result<SudoEmailCode> {
        let v: Value = self
            .raw(Method::POST, "/users/@me/sudo/email-code", None)
            .await?
            .json()?;
        let ticket = v
            .get("ticket")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TICKET_CHARS)
            .ok_or_else(|| Error::Decode("sudo e-mail code answer without a ticket".into()))?;
        let text = |key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 64)
                .map(str::to_owned)
        };
        Ok(SudoEmailCode {
            ticket: Secret::new(ticket),
            code_expires_at: text("code_expires_at"),
            resend_available_at: text("resend_available_at"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJ0eXBlIjoic3VkbyJ9.c2ln-_x";

    #[test]
    fn secrets_are_redacted() {
        let secret = Secret::new("hunter2-very-secret");
        let token = SudoToken::parse(JWT).unwrap();
        for shown in [format!("{secret:?}"), format!("{token:?}")] {
            assert!(!shown.contains("hunter2"), "{shown}");
            assert!(!shown.contains("eyJ"), "{shown}");
            assert!(shown.contains("<redacted>"));
        }
        let proof = SudoProof::password("hunter2-very-secret");
        assert!(!format!("{proof:?}").contains("hunter2"));
        let proof = SudoProof::code("123456");
        assert!(!format!("{proof:?}").contains("123456"));
        let proof = SudoProof::EmailCode {
            ticket: Secret::new("tick-et-value"),
            code: Secret::new("123456"),
        };
        let shown = format!("{proof:?}");
        assert!(!shown.contains("tick-et-value") && !shown.contains("123456"));
    }

    #[test]
    fn a_token_must_look_like_a_jwt() {
        assert!(SudoToken::parse(JWT).is_some());
        assert!(SudoToken::parse(" eyJa.eyJb.c2ln ").is_some());
        for bad in [
            "",
            "plain",
            "a.b",
            "a.b.c.d",
            "a..c",
            "a.b.c d",
            "a.b.c\nX: y",
            "a.b.é",
        ] {
            assert!(SudoToken::parse(bad).is_none(), "{bad:?}");
        }
        let long = format!("a.b.{}", "c".repeat(MAX_TOKEN_BYTES));
        assert!(SudoToken::parse(&long).is_none());
    }

    #[test]
    fn tokens_compare_by_value() {
        let a = SudoToken::parse(JWT).unwrap();
        assert!(a == a.clone());
        assert!(a != SudoToken::parse("eyJa.eyJb.c2ln").unwrap());
    }

    #[test]
    fn the_token_is_wiped_on_drop() {
        // Drop calls `zeroize` on the string; run the same call by hand and look at the
        // buffer it leaves (the capacity is kept, so the pointer stays valid).
        let mut token = SudoToken::parse(JWT).unwrap();
        let ptr = token.0.as_ptr();
        token.0.zeroize();
        assert_eq!(token.0.len(), 0);
        let bytes = unsafe { std::slice::from_raw_parts(ptr, JWT.len()) };
        assert!(bytes.iter().all(|b| *b == 0));
    }

    #[test]
    fn only_sudo_routes_match() {
        let yes = [
            (Method::POST, "/users/@me/delete"),
            (Method::POST, "/users/@me/recovery-kit"),
            (Method::PATCH, "/users/@me"),
            (Method::PATCH, "/users/@me?x=1"),
            (Method::DELETE, "/channels/123"),
            (Method::DELETE, "/channels/123/recipients/456"),
            (Method::POST, "/guilds/9/transfer-ownership"),
            (Method::POST, "/guilds/9/delete"),
            (Method::PATCH, "/guilds/9"),
            (Method::DELETE, "/users/@me/guilds/9"),
            (Method::PUT, "/users/@me/mfa/webauthn/two-factor"),
        ];
        for (m, p) in yes {
            assert!(is_sudo_route(&m, p), "{m} {p}");
        }
        let no = [
            (Method::GET, "/users/@me"),
            (Method::POST, "/users/@me"),
            (Method::GET, "/users/@me/delete"),
            (Method::POST, "/channels/123/messages"),
            (Method::DELETE, "/channels/123/messages/5"),
            (Method::POST, "/channels/123/typing"),
            (Method::DELETE, "/channels/"),
            (Method::DELETE, "/channels/../x"),
            (Method::DELETE, "/channels/%2e%2e"),
            (Method::GET, "/guilds/9"),
            (Method::POST, "/auth/login"),
            (Method::GET, "/users/@me/sudo/mfa-methods"),
            (Method::POST, "/users/@me/sudo/email-code"),
            (Method::POST, "/users/@me/harvest"),
            (Method::POST, "users/@me/delete"),
            (Method::POST, "/users/@me/delete/extra"),
            (Method::POST, "//users/@me/delete"),
        ];
        for (m, p) in no {
            assert!(!is_sudo_route(&m, p), "{m} {p}");
        }
    }

    #[test]
    fn the_required_error_reads_methods_at_the_top_level_or_under_data() {
        let top = json!({"code": "SUDO_MODE_REQUIRED", "has_mfa": true,
            "methods": {"totp": true, "backup_codes": true, "password": false}});
        let r = SudoRequired::from_body(&top);
        assert!(r.has_mfa && r.methods.totp && r.methods.backup_codes && !r.methods.password);
        assert!(r.methods.any_supported() && r.methods.takes_code());
        let nested = json!({"code": "SUDO_MODE_REQUIRED", "data": {"has_mfa": false,
            "methods": {"password": true}}});
        let r = SudoRequired::from_body(&nested);
        assert!(!r.has_mfa && r.methods.password && !r.methods.takes_code());
        let none = SudoRequired::from_body(&json!({"code": "SUDO_MODE_REQUIRED"}));
        assert!(!none.methods.any_supported());
    }

    #[test]
    fn a_proof_goes_into_the_body_the_way_fluxer_takes_it() {
        let mut m = Map::new();
        SudoProof::password("pw-123456").put_into(&mut m);
        assert_eq!(Value::Object(m), json!({"password": "pw-123456"}));
        let mut m = Map::new();
        SudoProof::code("123456").put_into(&mut m);
        assert_eq!(
            Value::Object(m),
            json!({"mfa_method": "totp", "mfa_code": "123456"})
        );
        let mut m = Map::new();
        SudoProof::EmailCode {
            ticket: Secret::new("t"),
            code: Secret::new("123456"),
        }
        .put_into(&mut m);
        assert_eq!(
            Value::Object(m),
            json!({"mfa_method": "email_code", "email_code_ticket": "t", "email_code": "123456"})
        );
    }

    #[test]
    fn proofs_fluxer_would_refuse_are_not_sent() {
        assert!(SudoProof::password("").check().is_err());
        assert!(SudoProof::password("x".repeat(257)).check().is_err());
        assert!(SudoProof::password("x".repeat(256)).check().is_ok());
        assert!(SudoProof::code("").check().is_err());
        assert!(SudoProof::code("x".repeat(33)).check().is_err());
        let email = |code: &str| SudoProof::EmailCode {
            ticket: Secret::new("t"),
            code: Secret::new(code),
        };
        assert!(email("12345").check().is_err());
        assert!(email("12345a").check().is_err());
        assert!(email("123456").check().is_ok());
    }

    #[test]
    fn wiping_clears_every_string_in_a_value() {
        let mut v = json!({"password": "secret", "list": ["a", {"b": "c"}], "n": 1});
        wipe(&mut v);
        assert!(!v.to_string().contains("secret"));
        assert_eq!(v["password"].as_str(), Some(""));
        assert_eq!(v["list"][1]["b"].as_str(), Some(""));
        assert_eq!(v["n"], json!(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_expires_after_four_and_a_half_minutes() {
        let store = SudoStore::default();
        assert!(store.token().is_none());
        store.set(SudoToken::parse(JWT).unwrap());
        tokio::time::advance(SUDO_TOKEN_TTL - Duration::from_secs(1)).await;
        assert!(store.token().is_some());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(store.token().is_none());
        // Gone for good, not only hidden.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(store.token().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_proofs_are_capped_and_forgiven_after_the_window() {
        let store = SudoStore::default();
        for _ in 0..MAX_WRONG_PROOFS - 1 {
            store.record_wrong();
            assert!(store.proof_wait().is_none());
        }
        store.record_wrong();
        let wait = store.proof_wait().expect("blocked after the fifth");
        assert!(wait <= WRONG_PROOF_WINDOW && wait > WRONG_PROOF_WINDOW - Duration::from_secs(1));
        tokio::time::advance(WRONG_PROOF_WINDOW + Duration::from_secs(1)).await;
        assert!(store.proof_wait().is_none());
        store.record_wrong();
        store.reset_wrong();
        assert!(store.proof_wait().is_none());
    }
}
