// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signing in inside the app: the identifier-first flow (look the account up, then the
//! password or an emailed code), the classic email-and-password form, the second factor
//! (an authenticator or backup code), and the wait for the new-IP email.
//!
//! The backend holds what must not reach the UI: the discovered instance (so its
//! rate limiter lives through the whole sign-in), and the tickets of the steps. The UI
//! gets steps to show ([`EntryStep`]) and errors in words ([`EntryError`]). Passwords
//! and codes arrive as [`p::Secret`]s and go straight into the request; the token a
//! finished sign-in returns goes through the same checks as a browser sign-in and is
//! stored in the keychain, nowhere else.
//!
//! Every request goes through the instance's anonymous limiter (10 a second, and the
//! waits the server asks for), and the UI sends one step at a time. The new-IP poll runs
//! every two seconds (the route allows 60 a minute) and stops at the ticket's lifetime.
//! A CAPTCHA is solved by the person's own machine, as Fluxer's client does, and never
//! skipped: when the instance does not accept the answer, the person is told.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use inlet_protocol as p;
use tokio::task::AbortHandle;

use crate::backend::session::{self, Credential, Failure};
use crate::backend::{Backend, Internal, Mode};
use crate::model;

/// The next flow number. Every start and every start-over takes a new one, so an answer
/// for an older flow is recognised and dropped.
fn next_flow() -> u64 {
    static FLOWS: AtomicU64 = AtomicU64::new(1);
    FLOWS.fetch_add(1, Ordering::Relaxed)
}

// Deviation: Fluxer keeps a `device_proof` from an earlier sign-in on the device and
// sends it with `identify`, so a known device is asked for a CAPTCHA later. Inlet keeps
// nothing but the keychain entry on disk, so it sends none and may be asked sooner.
// Deviation: a new address (sign-up), passkeys and single sign-on are not part of this
// form yet; the browser sign-in covers passkeys and single sign-on.

/// How often the new-IP wait asks whether the link was used.
const IP_POLL_EVERY: Duration = if cfg!(test) {
    Duration::from_millis(40)
} else {
    Duration::from_secs(2)
};
/// A new-IP ticket lives 15 minutes on the server.
const IP_TICKET_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// Failed polls in a row before the wait says the connection was lost.
const IP_POLL_MAX_FAILURES: u32 = 3;

/// What the instance offers for signing in, as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryInfo {
    /// The instance's origin (`https://chat.example`).
    pub origin: String,
    /// How the instance is named on screen (`chat.example`).
    pub host: String,
    /// What people sign in with.
    pub identity: p::Identity,
    /// The two-step entry (identifier, then password or code) rather than one form.
    pub identifier_first: bool,
    /// Every person has to use single sign-on, which Inlet cannot do yet.
    pub sso_enforced: bool,
}

/// A step the UI shows after a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryStep {
    /// The account is known and has a password.
    Password,
    /// A code was emailed.
    Code { resend_in_secs: u64 },
    /// A second factor is needed.
    Mfa {
        totp: bool,
        backup_codes: bool,
        passkey: bool,
    },
    /// Fluxer emailed a link to approve this network.
    IpAuthorization {
        email: Option<String>,
        resend_in_secs: u64,
    },
}

/// Where an error belongs on the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Instance,
    Identifier,
    Password,
    Code,
}

/// What went wrong, in words, and what the UI should do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryError {
    pub message: String,
    pub field: Field,
    /// The instance asked for a pause: how many seconds.
    pub wait_secs: Option<u64>,
    /// The ticket is gone: the sign-in has to start again from the first step.
    pub restart: bool,
}

impl EntryError {
    fn new(message: String, field: Field) -> Self {
        Self {
            message,
            field,
            wait_secs: None,
            restart: false,
        }
    }
}

/// The state of a sign-in in progress.
pub(in crate::backend) struct Entry {
    instance: p::Instance,
    /// The text that was typed to find the instance.
    typed: String,
    /// Bumped when the flow starts over or ends; an answer for an older one is dropped.
    flow: u64,
    /// The ticket of `identify`, for the email code.
    identify: Option<p::Ticket>,
    /// The ticket of the second-factor step.
    mfa: Option<p::Ticket>,
    /// The ticket and poll of the new-IP wait.
    ip: Option<(p::Ticket, AbortHandle)>,
    /// Stops the CAPTCHA solve of this flow's requests when the person leaves it.
    cancel: p::altcha::SolveCancel,
}

impl Entry {
    fn stop_wait(&mut self) {
        if let Some((_, poll)) = self.ip.take() {
            poll.abort();
        }
    }

    fn start_over(&mut self) {
        self.flow = next_flow();
        // A solve that is running stops at once, so no CPU goes on working for a form
        // the person has left. The next request gets a fresh handle.
        self.cancel.cancel();
        self.cancel = p::altcha::SolveCancel::new();
        self.identify = None;
        self.mfa = None;
        self.stop_wait();
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.stop_wait();
    }
}

/// An answer from a task, with the flow it was asked in.
pub(in crate::backend) struct Done {
    flow: u64,
    /// The instance the request went to (none for the first discovery). A session that
    /// arrives after the person left is ended on it.
    instance: Option<p::Instance>,
    what: Outcome,
}

enum Outcome {
    /// Discovery of the instance that was typed (the text, and what was found).
    Started(String, Result<p::Instance, Failure>),
    /// The identifier was looked up; for a code account the code is sent as well.
    Identified(Result<Identified, EntryError>),
    /// A fresh code was sent.
    CodeSent(Result<u64, EntryError>),
    /// A password, code or second-factor attempt.
    SignedIn(Result<p::SignInResult, p::Error>, Field),
    /// The new-IP email was sent again.
    IpResent(Result<u64, EntryError>),
    /// The new-IP wait: the link was used.
    IpDone(p::SignedIn),
    /// The new-IP wait ended without a session.
    IpEnded(IpEnd),
}

enum IpEnd {
    /// Three polls failed in a row.
    Lost,
    /// The ticket ran out or the server refused it.
    Failed(EntryError),
}

enum Identified {
    /// Ask for the password.
    Password(p::Ticket),
    /// A code is on its way.
    Code(p::Ticket, u64),
    /// No account uses the identifier.
    NoAccount { closed: bool },
}

/// Commands of the in-app sign-in.
pub enum Cmd {
    /// Find the instance and tell what it offers.
    Start { instance: String },
    /// Identifier-first: look the identifier up.
    Identify { identifier: String },
    /// Sign in with a password (the one form, or the password step).
    Password {
        identifier: String,
        password: p::Secret,
    },
    /// Send the email code again.
    SendCode,
    /// Finish with the emailed code.
    Code { code: p::Secret },
    /// Finish with an authenticator or backup code.
    Mfa { code: p::Secret },
    /// Send the new-IP email again.
    IpResend,
    /// Poll for the new-IP link again after the connection was lost.
    IpRetry,
    /// Back to the first step (the discovered instance is kept).
    Back,
    /// The person left the sign-in screen or changed the instance: forget all of it.
    Leave,
    /// The sign-in starts from this invite code: the sign-in requests carry it, so the
    /// instance joins the server (`None`: no invite). Sent before `Start`.
    Invite(Option<String>),
}

/// What the UI hears from the in-app sign-in.
pub enum Ev {
    /// The instance is found: show the form it uses.
    Started(EntryInfo),
    /// Show this step.
    Step(EntryStep),
    /// The email code was sent again.
    CodeResent { resend_in_secs: u64 },
    /// The new-IP email was sent again.
    IpResent { resend_in_secs: u64 },
    /// The new-IP wait lost its connection; offer "Try again".
    IpLost,
    /// No account uses this identifier (and sign-up is not done here).
    NoAccount { registration_closed: bool },
    /// A step failed.
    Failed(EntryError),
    /// The backend no longer knows this sign-in (it was left, or the backend started
    /// over): start from the instance again.
    Lost,
}

/// What to tell about a failed request. `field` is where it belongs on the form.
pub fn explain(err: &p::Error, identity: p::Identity, field: Field) -> EntryError {
    match err {
        p::Error::Api(api) => explain_api(api, identity, field),
        p::Error::RateLimited { retry_after } => {
            let secs = retry_after.as_secs().max(1);
            EntryError {
                message: wait_text(secs),
                field,
                wait_secs: Some(secs),
                restart: false,
            }
        }
        p::Error::Captcha(_) => EntryError::new(t!("entry-error-captcha"), field),
        other => EntryError::new(session::describe(other), field),
    }
}

fn explain_api(api: &p::ApiError, identity: p::Identity, field: Field) -> EntryError {
    // Fluxer answers a wrong password and an unknown account the same way, and so does
    // Inlet: nothing here tells which of the two it was.
    let code = api.field_code().unwrap_or(&api.code);
    let (message, field, restart) = match code {
        "INVALID_EMAIL_OR_PASSWORD" | "INVALID_LOGIN_OR_PASSWORD" => (
            match identity {
                p::Identity::Email => t!("entry-error-credentials-email"),
                p::Identity::Username => t!("entry-error-credentials-username"),
            },
            Field::Password,
            false,
        ),
        "INVALID_CODE" | "INVALID_SIGN_IN_CODE" => {
            (t!("entry-error-code-wrong"), Field::Code, false)
        }
        "SIGN_IN_CODE_EXPIRED" => (t!("entry-error-code-expired"), Field::Code, false),
        "SESSION_TIMEOUT"
        | "UNKNOWN_SIGN_IN_TICKET"
        | "INVALID_OR_EXPIRED_AUTHORIZATION_TICKET" => (t!("entry-error-timed-out"), field, true),
        "EMAIL_CODES_UNAVAILABLE" => (t!("entry-error-no-codes"), field, false),
        "SSO_REQUIRED" => (t!("entry-error-sso-required"), field, false),
        "FEATURE_TEMPORARILY_DISABLED" => (t!("entry-error-feature-off"), field, false),
        "GLOBAL_IP_BANNED" | "GLOBAL_IP_TEMPORARILY_BANNED" => {
            (t!("entry-error-network-blocked"), field, false)
        }
        "IP_AUTHORIZATION_RESEND_LIMIT_EXCEEDED" => {
            (t!("entry-error-ip-resent-already"), field, false)
        }
        "EMAIL_CODE_RESEND_COOLDOWN" | "IP_AUTHORIZATION_RESEND_COOLDOWN" => {
            (t!("entry-error-resend-soon"), field, false)
        }
        _ if api.status == 429 => (t!("entry-error-rate-limited-now"), field, false),
        // Suspensions, closed accounts and other refusals: the instance's own words,
        // cleaned and cut.
        _ => (
            session::describe(&p::Error::Api(Box::new(api.clone()))),
            field,
            false,
        ),
    };
    EntryError {
        message,
        field,
        wait_secs: None,
        restart,
    }
}

/// "Try again in 12 s." or in minutes for a long wait.
fn wait_text(secs: u64) -> String {
    if secs > 120 {
        t!(
            "entry-error-rate-limited-minutes",
            minutes = secs.div_ceil(60)
        )
    } else {
        t!("entry-error-rate-limited-seconds", seconds = secs)
    }
}

/// The seconds from now to an ISO 8601 time, 0 when it has passed or cannot be read.
fn seconds_until(iso: &str) -> Option<u64> {
    let at = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(
        (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .num_seconds()
            .clamp(0, 3600) as u64,
    )
}

/// How long until another email code can be asked for. Fluxer says 30 seconds; the
/// instance's own time is used when it can be read.
fn resend_wait(sent: &p::CodeSent) -> u64 {
    seconds_until(&sent.resend_at).unwrap_or(30)
}

pub(super) fn handle(b: &mut Backend, cmd: Cmd) {
    match cmd {
        Cmd::Start { instance } => b.entry_start(instance),
        Cmd::Identify { identifier } => b.entry_identify(identifier),
        Cmd::Password {
            identifier,
            password,
        } => b.entry_password(identifier, password),
        Cmd::SendCode => b.entry_send_code(),
        Cmd::Code { code } => b.entry_code(code),
        Cmd::Mfa { code } => b.entry_mfa(code),
        Cmd::IpResend => b.entry_ip_resend(),
        Cmd::IpRetry => b.entry_ip_retry(),
        Cmd::Back => {
            if let Some(entry) = &mut b.entry {
                entry.start_over();
            }
        }
        Cmd::Leave => b.entry_leave(),
        Cmd::Invite(code) => b.entry_invite = code.filter(|c| !c.is_empty()),
    }
}

impl Backend {
    fn entry_say(&self, ev: Ev) {
        self.tell(super::Ev::Entry(ev));
    }

    /// Ends the in-app sign-in (the person left the screen, or it finished).
    pub(super) fn entry_leave(&mut self) {
        self.entry = None;
        self.entry_starting = None;
        self.entry_invite = None;
    }

    fn identity(&self) -> p::Identity {
        self.entry
            .as_ref()
            .map(|e| e.instance.discovery().sign_in_options().identity)
            .unwrap_or_default()
    }

    /// Runs `work` for the current flow; its answer comes back as an [`Outcome`].
    fn entry_task<F, Fut>(&mut self, what: &'static str, work: F)
    where
        F: FnOnce(p::Instance) -> Fut,
        Fut: std::future::Future<Output = Outcome> + Send + 'static,
    {
        let Some(entry) = &self.entry else {
            return self.entry_say(Ev::Lost);
        };
        let (flow, instance) = (entry.flow, entry.instance.clone());
        let fut = p::altcha::with_solve_cancel(entry.cancel.clone(), work(instance.clone()));
        let internal = self.internal.clone();
        self.spawn_guarded(what, None, async move {
            let what = fut.await;
            let _ = internal.send(Internal::Entry(Done {
                flow,
                instance: Some(instance),
                what,
            }));
        });
    }

    fn entry_start(&mut self, typed: String) {
        let typed = typed.trim().to_string();
        // The same instance again keeps its limiter and what it said.
        if let Some(entry) = &mut self.entry {
            if entry.typed == typed {
                entry.start_over();
                let info = info_of(&entry.instance);
                self.entry_say(Ev::Started(info));
                return;
            }
        }
        self.entry = None;
        let internal = self.internal.clone();
        let flow = next_flow();
        self.entry_starting = Some(flow);
        self.spawn_guarded("entry discovery", None, async move {
            let result = session::discover(&typed).await;
            let _ = internal.send(Internal::Entry(Done {
                flow,
                instance: None,
                what: Outcome::Started(typed, result),
            }));
        });
    }

    fn entry_identify(&mut self, identifier: String) {
        let identity = self.identity();
        if let Some(entry) = &mut self.entry {
            entry.identify = None;
        }
        self.entry_task("entry identify", move |instance| async move {
            let found = match p::identify(&instance, &identifier).await {
                Ok(found) => found,
                Err(err) => {
                    return Outcome::Identified(Err(explain(&err, identity, Field::Identifier)))
                }
            };
            if !found.existing {
                let closed = found.registration_mode.as_deref() == Some("closed");
                return Outcome::Identified(Ok(Identified::NoAccount { closed }));
            }
            let options = instance.discovery().sign_in_options();
            let by_code = options.email_codes && found.method == Some(p::SignInMethod::Code);
            if !by_code {
                return Outcome::Identified(Ok(Identified::Password(found.ticket)));
            }
            match p::send_email_code(&instance, &found.ticket).await {
                Ok(sent) => {
                    Outcome::Identified(Ok(Identified::Code(found.ticket, resend_wait(&sent))))
                }
                // The account has a password after all.
                Err(p::Error::Api(api)) if api.code == "PASSWORD_SIGN_IN_REQUIRED" => {
                    Outcome::Identified(Ok(Identified::Password(found.ticket)))
                }
                Err(err) => Outcome::Identified(Err(explain(&err, identity, Field::Identifier))),
            }
        });
    }

    fn entry_password(&mut self, identifier: String, password: p::Secret) {
        let invite = self.entry_invite.clone();
        self.entry_task("entry password", move |instance| async move {
            let result = p::sign_in_password_with_invite(
                &instance,
                &identifier,
                &password,
                invite.as_deref(),
            )
            .await;
            drop(password);
            Outcome::SignedIn(result, Field::Password)
        });
    }

    fn entry_send_code(&mut self) {
        let identity = self.identity();
        let Some(ticket) = self.entry.as_ref().and_then(|e| e.identify.clone()) else {
            return self.entry_say(Ev::Lost);
        };
        self.entry_task("entry send code", move |instance| async move {
            match p::send_email_code(&instance, &ticket).await {
                Ok(sent) => Outcome::CodeSent(Ok(resend_wait(&sent))),
                Err(err) => Outcome::CodeSent(Err(explain(&err, identity, Field::Code))),
            }
        });
    }

    fn entry_code(&mut self, code: p::Secret) {
        let Some(ticket) = self.entry.as_ref().and_then(|e| e.identify.clone()) else {
            return self.entry_say(Ev::Lost);
        };
        let invite = self.entry_invite.clone();
        self.entry_task("entry code", move |instance| async move {
            let result =
                p::verify_email_code_with_invite(&instance, &ticket, &code, invite.as_deref())
                    .await;
            drop(code);
            Outcome::SignedIn(result, Field::Code)
        });
    }

    fn entry_mfa(&mut self, code: p::Secret) {
        let Some(ticket) = self.entry.as_ref().and_then(|e| e.mfa.clone()) else {
            return self.entry_say(Ev::Lost);
        };
        // Authenticator apps show "123 456"; backup codes have no spaces.
        // One allocation of the right size, so no half-built copy is left behind.
        let mut kept = String::with_capacity(code.expose_secret().len());
        kept.extend(code.expose_secret().chars().filter(|c| !c.is_whitespace()));
        let cleaned = p::Secret::new(kept);
        drop(code);
        let invite = self.entry_invite.clone();
        self.entry_task("entry mfa", move |instance| async move {
            let result =
                p::sign_in_totp_with_invite(&instance, &ticket, &cleaned, invite.as_deref()).await;
            drop(cleaned);
            Outcome::SignedIn(result, Field::Code)
        });
    }

    fn entry_ip_resend(&mut self) {
        let identity = self.identity();
        let Some(ticket) = self
            .entry
            .as_ref()
            .and_then(|e| e.ip.as_ref().map(|(t, _)| t.clone()))
        else {
            return;
        };
        self.entry_task("entry ip resend", move |instance| async move {
            match p::resend_ip_authorization(&instance, &ticket).await {
                Ok(()) => Outcome::IpResent(Ok(30)),
                Err(err) => Outcome::IpResent(Err(explain(&err, identity, Field::Code))),
            }
        });
    }

    fn entry_ip_retry(&mut self) {
        let Some(entry) = &mut self.entry else {
            return;
        };
        let Some((ticket, poll)) = entry.ip.take() else {
            return;
        };
        poll.abort();
        self.start_ip_wait(ticket);
    }

    /// Starts asking whether the new-IP link was used.
    fn start_ip_wait(&mut self, ticket: p::Ticket) {
        let Some(entry) = &self.entry else {
            return;
        };
        let (flow, instance) = (entry.flow, entry.instance.clone());
        let identity = instance.discovery().sign_in_options().identity;
        let internal = self.internal.clone();
        let polled = ticket.clone();
        let poll = self.spawn_guarded("entry ip wait", None, async move {
            let deadline = tokio::time::Instant::now() + IP_TICKET_LIFETIME;
            let mut failures = 0;
            let end = loop {
                if tokio::time::Instant::now() >= deadline {
                    break Outcome::IpEnded(IpEnd::Failed(EntryError {
                        restart: true,
                        ..EntryError::new(t!("entry-error-timed-out"), Field::Identifier)
                    }));
                }
                match p::poll_ip_authorization(&instance, &polled).await {
                    Ok(p::IpAuthorization::Completed(signed_in)) => {
                        break Outcome::IpDone(signed_in)
                    }
                    Ok(_) => failures = 0,
                    // The server refused the ticket (gone or used): not a lost connection.
                    Err(p::Error::Api(api))
                        if (400..500).contains(&api.status) && api.status != 429 =>
                    {
                        let err = explain(&p::Error::Api(api), identity, Field::Identifier);
                        break Outcome::IpEnded(IpEnd::Failed(EntryError {
                            restart: true,
                            ..err
                        }));
                    }
                    Err(p::Error::RateLimited { retry_after }) => {
                        tokio::time::sleep(retry_after.min(Duration::from_secs(30))).await;
                        continue;
                    }
                    Err(_) => {
                        failures += 1;
                        if failures >= IP_POLL_MAX_FAILURES {
                            break Outcome::IpEnded(IpEnd::Lost);
                        }
                    }
                }
                tokio::time::sleep(IP_POLL_EVERY).await;
            };
            let _ = internal.send(Internal::Entry(Done {
                flow,
                instance: Some(instance),
                what: end,
            }));
        });
        if let Some(entry) = &mut self.entry {
            entry.ip = Some((ticket, poll));
        }
    }

    /// An answer from one of the tasks above.
    pub(super) fn entry_done(&mut self, done: Done) {
        // Discovery comes first: no entry exists yet.
        if let Outcome::Started(typed, result) = done.what {
            // Only the latest start counts.
            if self.entry_starting != Some(done.flow) {
                return;
            }
            self.entry_starting = None;
            match result {
                Ok(instance) => {
                    let info = info_of(&instance);
                    self.entry = Some(Entry {
                        instance,
                        typed,
                        flow: done.flow,
                        identify: None,
                        mfa: None,
                        ip: None,
                        cancel: p::altcha::SolveCancel::new(),
                    });
                    self.entry_say(Ev::Started(info));
                }
                Err(failure) => self.entry_say(Ev::Failed(EntryError::new(
                    failure.message,
                    Field::Instance,
                ))),
            }
            return;
        }
        let identity = self.identity();
        match &self.entry {
            Some(entry) if entry.flow == done.flow => {}
            _ => return self.entry_discard(done),
        }
        match done.what {
            Outcome::Started(..) => {}
            Outcome::Identified(Err(err)) => self.entry_say(Ev::Failed(err)),
            Outcome::Identified(Ok(Identified::NoAccount { closed })) => {
                self.entry_say(Ev::NoAccount {
                    registration_closed: closed,
                })
            }
            Outcome::Identified(Ok(Identified::Password(ticket))) => {
                if let Some(entry) = &mut self.entry {
                    entry.identify = Some(ticket);
                }
                self.entry_say(Ev::Step(EntryStep::Password));
            }
            Outcome::Identified(Ok(Identified::Code(ticket, resend_in_secs))) => {
                if let Some(entry) = &mut self.entry {
                    entry.identify = Some(ticket);
                }
                self.entry_say(Ev::Step(EntryStep::Code { resend_in_secs }));
            }
            Outcome::CodeSent(Ok(resend_in_secs)) => {
                self.entry_say(Ev::CodeResent { resend_in_secs })
            }
            Outcome::CodeSent(Err(err)) | Outcome::IpResent(Err(err)) => {
                self.entry_say(Ev::Failed(err))
            }
            Outcome::IpResent(Ok(resend_in_secs)) => {
                self.entry_say(Ev::IpResent { resend_in_secs })
            }
            Outcome::SignedIn(result, field) => self.entry_signed_in(result, field, identity),
            Outcome::IpDone(signed_in) => self.entry_finish(signed_in),
            Outcome::IpEnded(IpEnd::Lost) => self.entry_say(Ev::IpLost),
            Outcome::IpEnded(IpEnd::Failed(err)) => self.entry_say(Ev::Failed(err)),
        }
    }

    /// The result of a password, email code or second-factor attempt.
    fn entry_signed_in(
        &mut self,
        result: Result<p::SignInResult, p::Error>,
        field: Field,
        identity: p::Identity,
    ) {
        let result = match result {
            Ok(result) => result,
            Err(err) => return self.entry_say(Ev::Failed(explain(&err, identity, field))),
        };
        match result {
            p::SignInResult::Ok(signed_in) => self.entry_finish(signed_in),
            p::SignInResult::NeedsMfa { ticket, methods } => {
                let has = |m: &p::MfaMethod| methods.contains(m);
                let step = EntryStep::Mfa {
                    totp: has(&p::MfaMethod::Totp),
                    backup_codes: has(&p::MfaMethod::BackupCodes),
                    passkey: has(&p::MfaMethod::Webauthn),
                };
                if let Some(entry) = &mut self.entry {
                    entry.mfa = Some(ticket);
                }
                self.entry_say(Ev::Step(step));
            }
            p::SignInResult::IpAuthorizationRequired {
                ticket,
                email,
                resend_available_in,
            } => {
                self.start_ip_wait(ticket);
                // The address is the instance's text: cleaned like every name from a server.
                let email = email
                    .filter(|e| !e.trim().is_empty())
                    .map(|e| model::clean_name(&e));
                self.entry_say(Ev::Step(EntryStep::IpAuthorization {
                    email,
                    resend_in_secs: resend_available_in,
                }));
            }
            p::SignInResult::Captcha => self.entry_say(Ev::Failed(explain(
                &p::Error::Captcha(String::new()),
                identity,
                field,
            ))),
            p::SignInResult::Error(api) => self.entry_say(Ev::Failed(explain(
                &p::Error::Api(Box::new(api)),
                identity,
                field,
            ))),
            // A newer kind of step this Inlet does not know.
            _ => self.entry_say(Ev::Failed(EntryError::new(
                t!("entry-error-unknown-step"),
                field,
            ))),
        }
    }

    /// An answer for a sign-in the person has left (Back, another instance, the screen
    /// closed). A session it carries is ended on the instance, as the browser sign-in does
    /// on cancel, so no session stays alive that Inlet does not hold.
    fn entry_discard(&mut self, done: Done) {
        let signed_in = match done.what {
            Outcome::SignedIn(Ok(p::SignInResult::Ok(signed_in)), _) => signed_in,
            Outcome::IpDone(signed_in) => signed_in,
            _ => return,
        };
        let Some(instance) = done.instance else {
            return;
        };
        self.spawn_guarded("revoke a left sign-in", None, async move {
            session::revoke(&p::Session::from_signed_in(instance, signed_in)).await;
        });
    }

    /// A sign-in worked: the token goes through the same checks as a browser sign-in
    /// (it is used once to read the account), and then to the keychain.
    fn entry_finish(&mut self, signed_in: p::SignedIn) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        let instance = entry.instance.clone();
        drop(entry);
        self.offline = None;
        let origin = instance.origin().to_string();
        self.start_connect(
            origin,
            Some(instance),
            Credential::Entered(signed_in),
            Mode::Fresh,
            None,
        );
    }
}

fn info_of(instance: &p::Instance) -> EntryInfo {
    let options = instance.discovery().sign_in_options();
    let origin = instance.origin().to_string();
    let host = url::Url::parse(&origin)
        .ok()
        .and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(port) => format!("{h}:{port}"),
                None => h.to_string(),
            })
        })
        .map(|h| model::clean_name(&h))
        .unwrap_or_else(|| model::clean_name(&origin));
    EntryInfo {
        origin,
        host,
        identity: options.identity,
        identifier_first: options.identifier_first,
        sso_enforced: options.sso_enforced,
    }
}

#[cfg(test)]
mod tests;
