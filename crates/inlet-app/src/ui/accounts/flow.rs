// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where the in-app sign-in is: the step on screen, what the instance offers, the
//! countdowns, and the error to show. A plain value without any window or input in it, so
//! its rules can be tested on their own; `sign_in.rs` draws it and sends the commands.

use std::time::{Duration, Instant};

use crate::backend::accounts::entry::{EntryError, EntryInfo, EntryStep, Field};

/// How long a request may take before the screen says it is still working (Fluxer's
/// client shows the same hint when solving the CAPTCHA takes a moment).
pub const WORKING_HINT_AFTER: Duration = Duration::from_secs(2);

/// The steps of the sign-in screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Which instance, and how to sign in.
    Instance,
    /// Identifier-first: the email address or username.
    Identifier,
    /// The classic form: identifier and password together.
    Credentials,
    /// Identifier-first: the password of the account that was found.
    Password,
    /// The code that was emailed.
    Code,
    /// The authenticator or backup code.
    Mfa,
    /// Waiting for the link in the new-IP email.
    IpAuth,
}

/// What the account can use as a second factor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mfa {
    pub totp: bool,
    pub backup_codes: bool,
    /// A passkey or security key; Inlet cannot use it yet.
    pub passkey: bool,
}

impl Mfa {
    /// Whether a code can be typed at all.
    pub fn takes_a_code(&self) -> bool {
        self.totp || self.backup_codes
    }
}

/// The wait for the new-IP link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpWait {
    pub email: Option<String>,
    pub resend_at: Instant,
    /// The connection was lost while waiting.
    pub lost: bool,
    /// The email was already sent again (Fluxer allows it once).
    pub resent: bool,
}

/// An error and the field it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormError {
    pub message: String,
    pub field: Field,
}

/// The state of the sign-in screen's flow.
#[derive(Clone, Debug)]
pub struct Flow {
    pub step: Step,
    /// What the instance offers; `None` until it was found.
    pub info: Option<EntryInfo>,
    /// The identifier the flow is for (shown on the later steps).
    pub identifier: String,
    pub mfa: Mfa,
    /// When another email code can be asked for.
    pub code_resend_at: Option<Instant>,
    pub ip: Option<IpWait>,
    pub error: Option<FormError>,
    /// A line that is not an error (single sign-on is required, no account found).
    pub notice: Option<String>,
    /// A request is under way since this moment.
    pub busy_since: Option<Instant>,
    /// The instance asked for a pause: no new attempt before this moment.
    pub locked_until: Option<Instant>,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            step: Step::Instance,
            info: None,
            identifier: String::new(),
            mfa: Mfa::default(),
            code_resend_at: None,
            ip: None,
            error: None,
            notice: None,
            busy_since: None,
            locked_until: None,
        }
    }
}

impl Flow {
    /// The first step of the form the instance uses.
    pub fn first_step(&self) -> Step {
        match &self.info {
            Some(info) if info.identifier_first => Step::Identifier,
            Some(_) => Step::Credentials,
            None => Step::Instance,
        }
    }

    /// Back to the instance step, forgetting what was found.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Back to the first step of the form (the instance stays).
    pub fn restart(&mut self) {
        self.step = self.first_step();
        self.identifier.clear();
        self.mfa = Mfa::default();
        self.code_resend_at = None;
        self.ip = None;
        self.busy_since = None;
    }

    /// A request goes out. `false` when one is already under way or the instance asked
    /// for a pause that is not over.
    pub fn begin(&mut self, now: Instant) -> bool {
        if self.busy_since.is_some() || self.locked(now) {
            return false;
        }
        self.busy_since = Some(now);
        self.error = None;
        true
    }

    pub fn busy(&self) -> bool {
        self.busy_since.is_some()
    }

    /// Whether the screen should say that it is still working.
    pub fn working_hint(&self, now: Instant) -> bool {
        self.busy_since
            .is_some_and(|since| now.saturating_duration_since(since) >= WORKING_HINT_AFTER)
    }

    /// Whether a pause the instance asked for is still running.
    pub fn locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    /// The instance is found: show the form it uses.
    pub fn started(&mut self, info: EntryInfo) {
        self.busy_since = None;
        self.error = None;
        self.notice = None;
        self.identifier.clear();
        self.mfa = Mfa::default();
        self.code_resend_at = None;
        self.ip = None;
        let sso_only = info.sso_enforced;
        self.info = Some(info);
        // Single sign-on is not done here: only the browser is offered.
        self.step = if sso_only {
            Step::Instance
        } else {
            self.first_step()
        };
        if sso_only {
            self.notice = Some(t!("entry-sso-enforced"));
        }
    }

    /// The identifier is not an account.
    pub fn no_account(&mut self, registration_closed: bool) {
        self.busy_since = None;
        self.error = Some(FormError {
            message: if registration_closed {
                t!("entry-no-account-closed")
            } else {
                t!("entry-no-account")
            },
            field: Field::Identifier,
        });
    }

    /// The backend names the next step.
    pub fn step_to(&mut self, step: EntryStep, now: Instant) {
        self.busy_since = None;
        self.error = None;
        self.notice = None;
        match step {
            EntryStep::Password => self.step = Step::Password,
            EntryStep::Code { resend_in_secs } => {
                self.step = Step::Code;
                self.code_resend_at = Some(now + Duration::from_secs(resend_in_secs));
            }
            EntryStep::Mfa {
                totp,
                backup_codes,
                passkey,
            } => {
                self.step = Step::Mfa;
                self.mfa = Mfa {
                    totp,
                    backup_codes,
                    passkey,
                };
            }
            EntryStep::IpAuthorization {
                email,
                resend_in_secs,
            } => {
                self.step = Step::IpAuth;
                self.ip = Some(IpWait {
                    email,
                    resend_at: now + Duration::from_secs(resend_in_secs),
                    lost: false,
                    resent: false,
                });
            }
        }
    }

    /// A step failed. A ticket that ran out takes the person back to the first step.
    pub fn failed(&mut self, err: EntryError, now: Instant) {
        self.busy_since = None;
        if let Some(secs) = err.wait_secs {
            self.locked_until = Some(now + Duration::from_secs(secs));
        }
        if err.restart {
            self.restart();
        }
        self.error = Some(FormError {
            message: err.message,
            field: err.field,
        });
    }

    pub fn code_resent(&mut self, resend_in_secs: u64, now: Instant) {
        self.busy_since = None;
        self.error = None;
        self.code_resend_at = Some(now + Duration::from_secs(resend_in_secs));
    }

    pub fn ip_resent(&mut self, resend_in_secs: u64, now: Instant) {
        self.busy_since = None;
        self.error = None;
        if let Some(ip) = &mut self.ip {
            ip.resend_at = now + Duration::from_secs(resend_in_secs);
            ip.resent = true;
        }
    }

    pub fn ip_lost(&mut self) {
        self.busy_since = None;
        if let Some(ip) = &mut self.ip {
            ip.lost = true;
        }
    }

    /// "Try again" after the connection was lost.
    pub fn ip_retrying(&mut self) {
        if let Some(ip) = &mut self.ip {
            ip.lost = false;
        }
    }

    /// Whole seconds until `at`, rounded up; 0 when it has passed.
    pub fn seconds_until(at: Option<Instant>, now: Instant) -> u64 {
        at.map_or(0, |at| {
            let left = at.saturating_duration_since(now);
            left.as_secs() + u64::from(left.subsec_nanos() > 0)
        })
    }

    /// Whether the screen has something that changes with time: a countdown, a pause, or
    /// a request whose hint is still to come.
    pub fn needs_clock(&self, now: Instant) -> bool {
        let counting = |at: Option<Instant>| at.is_some_and(|at| now < at);
        counting(self.code_resend_at)
            || counting(self.ip.as_ref().map(|ip| ip.resend_at))
            || counting(self.locked_until)
            || self.busy_since.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::Identity;

    fn info(identifier_first: bool, sso_enforced: bool) -> EntryInfo {
        EntryInfo {
            origin: "https://chat.example".into(),
            host: "chat.example".into(),
            identity: Identity::Email,
            identifier_first,
            sso_enforced,
        }
    }

    fn error(field: Field, wait: Option<u64>, restart: bool) -> EntryError {
        EntryError {
            message: "No".into(),
            field,
            wait_secs: wait,
            restart,
        }
    }

    #[test]
    fn an_identifier_first_instance_starts_at_the_identifier_step() {
        let mut flow = Flow::default();
        flow.started(info(true, false));
        assert_eq!(flow.step, Step::Identifier);
    }

    #[test]
    fn a_classic_instance_starts_at_the_one_form() {
        let mut flow = Flow::default();
        flow.started(info(false, false));
        assert_eq!(flow.step, Step::Credentials);
    }

    #[test]
    fn an_sso_only_instance_offers_only_the_browser() {
        let mut flow = Flow::default();
        flow.started(info(true, true));
        assert_eq!(flow.step, Step::Instance);
        assert!(flow.notice.is_some());
    }

    #[test]
    fn only_one_request_goes_out_at_a_time() {
        let now = Instant::now();
        let mut flow = Flow::default();
        assert!(flow.begin(now));
        assert!(
            !flow.begin(now),
            "a second click while waiting does nothing"
        );
        flow.failed(error(Field::Password, None, false), now);
        assert!(flow.begin(now));
    }

    #[test]
    fn a_wrong_password_stays_on_the_step_with_its_error() {
        let now = Instant::now();
        let mut flow = Flow::default();
        flow.started(info(true, false));
        flow.step_to(EntryStep::Password, now);
        assert!(flow.begin(now));
        flow.failed(error(Field::Password, None, false), now);
        assert_eq!(flow.step, Step::Password);
        assert_eq!(flow.error.as_ref().map(|e| e.field), Some(Field::Password));
        assert!(!flow.busy());
    }

    #[test]
    fn a_ticket_that_ran_out_returns_to_the_first_step() {
        let now = Instant::now();
        let mut flow = Flow::default();
        flow.started(info(true, false));
        flow.identifier = "ana@example.org".into();
        flow.step_to(
            EntryStep::Mfa {
                totp: true,
                backup_codes: false,
                passkey: false,
            },
            now,
        );
        flow.failed(error(Field::Identifier, None, true), now);
        assert_eq!(flow.step, Step::Identifier);
        assert!(flow.identifier.is_empty());
        assert!(flow.error.is_some());
    }

    #[test]
    fn a_pause_from_the_instance_blocks_new_attempts_until_it_ends() {
        let now = Instant::now();
        let mut flow = Flow::default();
        flow.failed(error(Field::Password, Some(30), false), now);
        assert!(flow.locked(now + Duration::from_secs(29)));
        assert!(!flow.begin(now + Duration::from_secs(29)));
        assert!(flow.begin(now + Duration::from_secs(31)));
    }

    #[test]
    fn the_resend_countdown_counts_down_to_zero() {
        let now = Instant::now();
        let mut flow = Flow::default();
        flow.step_to(EntryStep::Code { resend_in_secs: 30 }, now);
        assert_eq!(Flow::seconds_until(flow.code_resend_at, now), 30);
        assert_eq!(
            Flow::seconds_until(flow.code_resend_at, now + Duration::from_millis(29_500)),
            1
        );
        assert_eq!(
            Flow::seconds_until(flow.code_resend_at, now + Duration::from_secs(31)),
            0
        );
        assert!(flow.needs_clock(now));
        assert!(!flow.needs_clock(now + Duration::from_secs(31)));
    }

    #[test]
    fn the_screen_says_it_is_still_working_after_two_seconds() {
        let now = Instant::now();
        let mut flow = Flow::default();
        assert!(flow.begin(now));
        assert!(!flow.working_hint(now + Duration::from_millis(1_900)));
        assert!(flow.working_hint(now + Duration::from_millis(2_000)));
    }

    #[test]
    fn the_new_ip_wait_can_lose_its_connection_and_try_again() {
        let now = Instant::now();
        let mut flow = Flow::default();
        flow.step_to(
            EntryStep::IpAuthorization {
                email: Some("ana@example.org".into()),
                resend_in_secs: 30,
            },
            now,
        );
        flow.ip_lost();
        assert!(flow.ip.as_ref().unwrap().lost);
        flow.ip_retrying();
        assert!(!flow.ip.as_ref().unwrap().lost);
    }

    #[test]
    fn the_new_ip_email_can_be_marked_as_sent_again() {
        let now = Instant::now();
        let mut flow = Flow::default();
        flow.step_to(
            EntryStep::IpAuthorization {
                email: None,
                resend_in_secs: 0,
            },
            now,
        );
        flow.ip_resent(30, now);
        let ip = flow.ip.as_ref().unwrap();
        assert!(ip.resent);
        assert_eq!(Flow::seconds_until(Some(ip.resend_at), now), 30);
    }

    #[test]
    fn a_missing_account_is_reported_on_the_identifier_field() {
        let mut flow = Flow::default();
        flow.started(info(true, false));
        flow.no_account(true);
        let err = flow.error.as_ref().unwrap();
        assert_eq!(err.field, Field::Identifier);
        assert_ne!(
            err.message,
            {
                let mut open = Flow::default();
                open.no_account(false);
                open.error.unwrap().message
            },
            "a closed instance says so"
        );
    }
}
