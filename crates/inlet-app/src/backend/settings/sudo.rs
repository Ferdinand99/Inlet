// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sudo mode ("Confirm it's you") on the network side: sending an action that Fluxer
//! protects, and sending it again with the person's proof when the instance asks for one.
//! The protocol crate has the rules (`inlet_protocol::sudo`): the token, the routes it
//! goes to, the cap on wrong proofs. This is the bridge to the dialog.
//!
//! The flow, one command and four answers:
//!
//! 1. The UI sends [`Cmd::Run`] with the action and no proof. If the session holds a
//!    sudo token the action just goes through ([`Ev::Done`]). Otherwise the answer is
//!    [`Ev::Needed`] with what the account can use, and the UI opens the dialog.
//! 2. The person types a password or a code. The UI sends [`Cmd::Run`] again, the same
//!    action with the proof. It is [`Ev::Done`], [`Ev::Rejected`] (wrong password or
//!    code; the dialog stays open) or [`Ev::Needed`] again (the account cannot use that
//!    kind of proof, for example a password on an account with a second factor).
//! 3. [`Ev::Failed`] is any other failure, with its reason.
//!
//! The backend keeps no state between the two commands: the UI holds the action
//! ([`Request`]) while the dialog is open. The proof travels in the command, is wiped
//! after the request, and never comes back in an event. Nothing is written to disk.
//!
//! Request budget: one request per command, and the UI sends one at a time (the dialog
//! is busy while a request is out). The protocol refuses to send a proof after
//! [`p::sudo::MAX_WRONG_PROOFS`] wrong ones in 15 minutes ([`Ev::Locked`]), so a
//! dialog left open cannot be used to guess.

use std::fmt;
use std::sync::Arc;

use inlet_protocol as p;
use serde_json::Value;

use crate::backend::session::{Failure, FailureKind};
use crate::backend::{Active, Backend, Internal};

/// An action Fluxer protects with sudo mode: a method, a path below `/v1` and a JSON
/// object body (without the proof). Made only for the routes that take sudo mode
/// ([`p::is_sudo_route`]). The body can hold secrets (a new password), so `Debug` shows
/// only the method and the path.
#[derive(Clone)]
pub struct Request {
    method: p::Method,
    path: String,
    body: Option<Value>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Request({} {})", self.method, self.path)
    }
}

impl Request {
    /// The action `method path` with `body`, or `None` when the route does not take sudo
    /// mode or the body is not a JSON object.
    // No feature asks yet; the first one that needs sudo mode builds its action here.
    #[allow(dead_code)]
    pub fn new(method: p::Method, path: &str, body: Option<Value>) -> Option<Request> {
        let body_ok = matches!(body, None | Some(Value::Object(_)));
        (p::is_sudo_route(&method, path) && body_ok).then(|| Request {
            method,
            path: path.to_owned(),
            body,
        })
    }

    /// The recovery kit action: a stand-in for a real feature's action in the dialog's
    /// demo scenes and tests.
    #[cfg(any(test, feature = "demo"))]
    pub fn recovery_kit() -> Request {
        Request::new(p::Method::POST, "/users/@me/recovery-kit", None).expect("a sudo route")
    }

    #[cfg(test)]
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// What the person typed, or nothing (the first try).
pub enum Proof {
    /// The account password.
    Password(p::Secret),
    /// An authenticator app code or a backup code.
    Code(p::Secret),
}

impl Proof {
    fn into_protocol(self) -> p::SudoProof {
        match self {
            Proof::Password(s) => p::SudoProof::Password(s),
            Proof::Code(s) => p::SudoProof::Code(s),
        }
    }
}

pub enum Cmd {
    /// Sends the action. `ticket` names it in the answers (the UI counts up).
    Run {
        ticket: u64,
        request: Arc<Request>,
        proof: Option<Proof>,
    },
}

/// Why a proof was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrong {
    Password,
    /// The account has no password to give.
    NoPassword,
    Code,
    Other,
}

impl From<p::SudoRejection> for Wrong {
    fn from(r: p::SudoRejection) -> Wrong {
        match r {
            p::SudoRejection::WrongPassword => Wrong::Password,
            p::SudoRejection::NoPassword => Wrong::NoPassword,
            p::SudoRejection::WrongCode | p::SudoRejection::WrongEmailCode => Wrong::Code,
            _ => Wrong::Other,
        }
    }
}

/// The answer of an action, for the feature that asked. It can hold secrets (a recovery
/// key, backup codes), so `Debug` shows nothing of it.
pub struct Response(#[allow(dead_code)] pub Value);

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Response(<hidden>)")
    }
}

pub enum Ev {
    /// The instance wants the person to confirm it is them: what the account can use.
    Needed {
        ticket: u64,
        methods: p::SudoMethods,
    },
    /// The proof was wrong. The dialog stays open.
    Rejected { ticket: u64, wrong: Wrong },
    /// Too many wrong proofs; none is sent for `wait_secs` seconds.
    Locked { ticket: u64, wait_secs: u64 },
    /// The action went through.
    Done {
        ticket: u64,
        #[allow(dead_code)] // The feature that asked reads it.
        response: Response,
    },
    /// The action failed for another reason (shown as it is).
    Failed { ticket: u64, reason: String },
}

pub(in crate::backend) fn handle(backend: &mut Backend, cmd: Cmd) {
    let Cmd::Run {
        ticket,
        request,
        proof,
    } = cmd;
    #[cfg(feature = "demo")]
    if let Some(Active::Demo(_)) = &backend.active {
        // The demo has no server.
        backend.emit(Ev::Failed {
            ticket,
            reason: t!("settings-not-connected"),
        });
        return;
    }
    let Some(Active::Real(r)) = &backend.active else {
        // Not signed in, or the command was for an account that is not the active one.
        backend.emit(Ev::Failed {
            ticket,
            reason: t!("settings-not-connected"),
        });
        return;
    };
    let client = r.session.client().clone();
    let events = r.events.clone();
    let internal = backend.internal.clone();
    let gen = r.gen;
    backend.spawn_guarded("sudo request", None, async move {
        let (event, ended) = run(&client, ticket, &request, proof).await;
        if let Some(message) = ended {
            // A token that is gone ends the session: the loop signs out and the dialog
            // closes with it.
            let _ = internal.send(Internal::Ended { gen, message });
        }
        let _ = events.unbounded_send(event);
    });
}

/// Runs one command to its answer. The second value is the reason when the session
/// itself ended (the token was refused).
pub(super) async fn run(
    client: &p::Client,
    ticket: u64,
    request: &Request,
    proof: Option<Proof>,
) -> (Ev, Option<String>) {
    let proof = proof.map(Proof::into_protocol);
    let sent_proof = proof.is_some();
    let result = client
        .sudo_request(
            request.method.clone(),
            &request.path,
            request.body.as_ref(),
            proof.as_ref(),
        )
        .await;
    // The proof is wiped here, before anything else happens.
    drop(proof);
    let event = match result {
        Ok(value) => Ev::Done {
            ticket,
            response: Response(value),
        },
        Err(p::Error::SudoRequired(required)) => Ev::Needed {
            ticket,
            methods: required.methods,
        },
        Err(p::Error::SudoRejected(reason)) => Ev::Rejected {
            ticket,
            wrong: reason.into(),
        },
        Err(p::Error::RateLimited { retry_after }) if sent_proof => Ev::Locked {
            ticket,
            wait_secs: retry_after.as_secs().max(1),
        },
        Err(err) => {
            // Every reason but a refused token is shown in the dialog.
            let failure = Failure::from(err);
            let ended =
                (failure.kind == FailureKind::SessionEnded).then(|| failure.message.clone());
            let event = Ev::Failed {
                ticket,
                reason: failure.message,
            };
            return (event, ended);
        }
    };
    (event, None)
}

#[cfg(test)]
mod tests;
