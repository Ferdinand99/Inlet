// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the "Confirm it's you" dialog is doing, as a plain value with no window: which
//! way the person confirms with, whether a request is out, and what went wrong. The
//! dialog (`../sudo_confirm.rs`) draws it and the tests drive it.

use std::sync::Arc;

use inlet_protocol as p;

use crate::backend::settings::sudo::{Ev, Request, Wrong};

/// A way to confirm that this client can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The account password.
    Password,
    /// A code from the authenticator app, or a backup code.
    Code,
}

/// Why the dialog shows an error under the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    WrongPassword,
    NoPassword,
    WrongCode,
    NotAccepted,
    /// Too many wrong tries: the minutes to wait.
    Locked {
        minutes: u64,
    },
    /// The action failed for another reason.
    Failed(String),
}

impl Problem {
    fn of(wrong: Wrong) -> Problem {
        match wrong {
            Wrong::Password => Problem::WrongPassword,
            Wrong::NoPassword => Problem::NoPassword,
            Wrong::Code => Problem::WrongCode,
            Wrong::Other => Problem::NotAccepted,
        }
    }
}

/// What an answer did to the dialog.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The dialog stays (it shows an error, or asks in another way).
    Stay,
    /// The action went through: close the dialog.
    Done,
}

/// The dialog's state.
pub struct State {
    /// The action the dialog confirms; the proof goes with it.
    pub request: Arc<Request>,
    /// Which action this is, so an answer for an old one is ignored.
    pub ticket: u64,
    pub methods: p::SudoMethods,
    pub mode: Mode,
    /// A request is out: nothing else is sent until it answers.
    pub busy: bool,
    pub problem: Option<Problem>,
}

impl State {
    /// The dialog for `request`, once the instance asked for sudo mode.
    pub fn new(ticket: u64, request: Arc<Request>, methods: p::SudoMethods) -> State {
        let mut state = State {
            request,
            ticket,
            methods,
            mode: Mode::Password,
            busy: false,
            problem: None,
        };
        state.mode = state.first_mode();
        state
    }

    /// The ways to confirm that are offered, password first (the way Fluxer's client
    /// lists them).
    pub fn modes(&self) -> Vec<Mode> {
        let mut modes = Vec::new();
        if self.methods.password {
            modes.push(Mode::Password);
        }
        if self.methods.takes_code() {
            modes.push(Mode::Code);
        }
        modes
    }

    fn first_mode(&self) -> Mode {
        self.modes().first().copied().unwrap_or(Mode::Password)
    }

    /// Whether this client can confirm in any way the account has. When it cannot (only
    /// a passkey, an e-mailed code or single sign-on), the dialog says so.
    pub fn can_confirm(&self) -> bool {
        !self.modes().is_empty()
    }

    /// Whether another way to confirm is offered next to the one in use.
    pub fn other_mode(&self) -> Option<Mode> {
        self.modes().into_iter().find(|m| *m != self.mode)
    }

    /// Changes the way to confirm (one the account has), and clears the error.
    pub fn use_mode(&mut self, mode: Mode) {
        if self.modes().contains(&mode) && !self.busy {
            self.mode = mode;
            self.problem = None;
        }
    }

    /// Whether the person may send now: a way to confirm, something typed, no request
    /// out, and no wait for too many wrong tries.
    pub fn can_submit(&self, typed: bool) -> bool {
        self.can_confirm()
            && typed
            && !self.busy
            && !matches!(self.problem, Some(Problem::Locked { .. }))
    }

    /// Marks a request as out. `false` when it must not be sent (one is out already).
    pub fn submit(&mut self) -> bool {
        if self.busy || !self.can_confirm() {
            return false;
        }
        self.busy = true;
        self.problem = None;
        true
    }

    /// Takes an answer of the backend. An answer for another action is ignored.
    pub fn apply(&mut self, ev: &Ev) -> Outcome {
        let ticket = match ev {
            Ev::Needed { ticket, .. }
            | Ev::Rejected { ticket, .. }
            | Ev::Locked { ticket, .. }
            | Ev::Done { ticket, .. }
            | Ev::Failed { ticket, .. } => *ticket,
        };
        if ticket != self.ticket {
            return Outcome::Stay;
        }
        self.busy = false;
        match ev {
            Ev::Done { .. } => return Outcome::Done,
            Ev::Needed { methods, .. } => {
                // Asked again: the proof was not one the account takes (a password on an
                // account with a second factor). Show what it does take.
                self.methods = *methods;
                if !self.modes().contains(&self.mode) {
                    self.mode = self.first_mode();
                }
            }
            Ev::Rejected { wrong, .. } => self.problem = Some(Problem::of(*wrong)),
            Ev::Locked { wait_secs, .. } => {
                self.problem = Some(Problem::Locked {
                    minutes: wait_secs.div_ceil(60).max(1),
                });
            }
            Ev::Failed { reason, .. } => self.problem = Some(Problem::Failed(reason.clone())),
        }
        Outcome::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::settings::sudo::Response;
    use serde_json::json;

    fn request() -> Arc<Request> {
        Arc::new(Request::recovery_kit())
    }

    fn methods(password: bool, totp: bool, backup: bool) -> p::SudoMethods {
        p::SudoMethods {
            password,
            totp,
            backup_codes: backup,
            ..p::SudoMethods::default()
        }
    }

    #[test]
    fn only_sudo_routes_make_a_request() {
        assert!(Request::new(p::Method::POST, "/users/@me/delete", None).is_some());
        assert!(Request::new(p::Method::POST, "/channels/1/messages", None).is_none());
        assert!(Request::new(p::Method::GET, "/users/@me", None).is_none());
        // A body must be an object (the proof is added to it).
        assert!(Request::new(p::Method::POST, "/users/@me/delete", Some(json!([1]))).is_none());
        assert!(Request::new(
            p::Method::POST,
            "/users/@me/delete",
            Some(json!({"immediate": true}))
        )
        .is_some());
    }

    #[test]
    fn the_first_way_is_the_password_then_a_code() {
        let s = State::new(1, request(), methods(true, true, true));
        assert_eq!(s.modes(), vec![Mode::Password, Mode::Code]);
        assert_eq!(s.mode, Mode::Password);
        assert_eq!(s.other_mode(), Some(Mode::Code));
        let s = State::new(1, request(), methods(false, true, true));
        assert_eq!(s.modes(), vec![Mode::Code]);
        assert_eq!(s.mode, Mode::Code);
        assert_eq!(s.other_mode(), None);
        // Only backup codes left: still a code.
        let s = State::new(1, request(), methods(false, false, true));
        assert_eq!(s.mode, Mode::Code);
    }

    #[test]
    fn an_account_with_only_a_passkey_cannot_confirm_here() {
        let only_passkey = p::SudoMethods {
            webauthn: true,
            ..p::SudoMethods::default()
        };
        let mut s = State::new(1, request(), only_passkey);
        assert!(!s.can_confirm());
        assert!(!s.can_submit(true));
        assert!(!s.submit());
        assert!(!s.busy);
        // An e-mailed code and single sign-on exist in the protocol but not in the
        // dialog yet.
        let sso_or_email = p::SudoMethods {
            email_code: true,
            sso_reauth: true,
            ..p::SudoMethods::default()
        };
        assert!(!State::new(1, request(), sso_or_email).can_confirm());
    }

    #[test]
    fn one_request_at_a_time() {
        let mut s = State::new(1, request(), methods(true, false, false));
        assert!(s.can_submit(true));
        assert!(!s.can_submit(false), "nothing typed");
        assert!(s.submit());
        assert!(!s.can_submit(true), "busy");
        assert!(!s.submit(), "a second submit does nothing");
        // Switching way is refused while a request is out.
        let mut both = State::new(1, request(), methods(true, true, false));
        assert!(both.submit());
        both.use_mode(Mode::Code);
        assert_eq!(both.mode, Mode::Password);
    }

    #[test]
    fn a_wrong_proof_shows_an_error_and_frees_the_dialog() {
        let mut s = State::new(7, request(), methods(true, false, false));
        s.submit();
        let out = s.apply(&Ev::Rejected {
            ticket: 7,
            wrong: Wrong::Password,
        });
        assert_eq!(out, Outcome::Stay);
        assert!(!s.busy);
        assert_eq!(s.problem, Some(Problem::WrongPassword));
        // The error goes when the person tries again.
        assert!(s.submit());
        assert_eq!(s.problem, None);
    }

    #[test]
    fn answers_for_another_action_are_ignored() {
        let mut s = State::new(7, request(), methods(true, false, false));
        s.submit();
        let out = s.apply(&Ev::Done {
            ticket: 6,
            response: Response(json!({})),
        });
        assert_eq!(out, Outcome::Stay);
        assert!(s.busy, "still waiting for its own answer");
        assert_eq!(
            s.apply(&Ev::Done {
                ticket: 7,
                response: Response(json!({}))
            }),
            Outcome::Done
        );
    }

    #[test]
    fn too_many_tries_block_sending_and_say_how_long() {
        let mut s = State::new(1, request(), methods(true, false, false));
        s.submit();
        s.apply(&Ev::Locked {
            ticket: 1,
            wait_secs: 14 * 60 + 5,
        });
        assert_eq!(s.problem, Some(Problem::Locked { minutes: 15 }));
        assert!(!s.can_submit(true));
    }

    #[test]
    fn asked_again_shows_what_the_account_takes() {
        // A password was sent, the account has a second factor: the instance asks again.
        let mut s = State::new(1, request(), methods(true, false, false));
        s.submit();
        s.apply(&Ev::Needed {
            ticket: 1,
            methods: methods(false, true, true),
        });
        assert_eq!(s.mode, Mode::Code);
        assert_eq!(s.modes(), vec![Mode::Code]);
        assert!(!s.busy);
        assert!(s.can_submit(true));
    }

    #[test]
    fn another_failure_is_shown_as_it_is() {
        let mut s = State::new(1, request(), methods(true, false, false));
        s.submit();
        s.apply(&Ev::Failed {
            ticket: 1,
            reason: "refused-for-now".into(),
        });
        assert_eq!(s.problem, Some(Problem::Failed("refused-for-now".into())));
    }
}
