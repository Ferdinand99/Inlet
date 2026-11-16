// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the instance says about itself and about the account: its discovery document
//! (the flags, the notice, the branding), the account's experiment assignments, the
//! status page, and the two calls behind the account banners (the verification email,
//! the updated terms). The UI turns the answers into one `Capabilities` per account
//! (`inlet-state`) and into the banners.
//!
//! # Who asks, and how often
//!
//! Only the active account has a poller. A background account is announced once, from
//! the discovery document its session was built with, and makes no request until it
//! becomes the active one (the passive-account rule: no REST without the person).
//!
//! | What | When |
//! | --- | --- |
//! | Discovery (anonymous, `If-None-Match`) | every 24 hours, and on a new connection of the gateway, at most once per 5 minutes |
//! | `GET /experiments` (`If-None-Match`) | at once for a new active account, then at the instance's own cadence (60 s to 24 h, with its jitter, doubled after a failure up to four times), a forced one at most every 30 s; three answers of 404 in a row stop it for the session |
//! | The status page (`summary.json`) | only when the instance says it is an Instatus page: every 60 to 75 s while the window has the focus and the status is quiet, and after 1, 5, 15, 30 minutes after failures; nothing while the window is not in use |
//! | `POST /auth/verify/resend` | on a click, and at most once a minute |
//! | `POST /users/@me/terms-acceptance` | on a click |
//!
//! Every call goes through the account's own limiter (the status page has its own
//! client and takes no credential). A burst of reconnects, focus changes or clicks
//! collapses into the same few requests: see the tests.

mod poller;
pub mod status_page;
#[cfg(test)]
mod tests;

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use inlet_protocol as p;
use inlet_state::ExperimentState;
use tokio::sync::mpsc::UnboundedSender as TokioSender;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::session::{Failure, FailureKind};
use poller::Poller;

use super::{Active, Backend, Internal, Ui};

/// How often the discovery document is read again.
pub const DISCOVERY_EVERY: Duration = Duration::from_secs(24 * 3600);
/// The least time between two reads of the discovery document, whatever asks.
pub const DISCOVERY_MIN_GAP: Duration = Duration::from_secs(300);
/// The least time between two polls of the experiments, whatever asks.
pub const EXPERIMENTS_MIN_GAP: Duration = Duration::from_secs(30);
/// The least time between two verification emails.
pub const RESEND_GAP: Duration = Duration::from_secs(60);
/// The wait after a failed read of the discovery document, doubled up to an hour.
pub(super) const DISCOVERY_RETRY: Duration = Duration::from_secs(300);
/// How much longer the experiments are polled while the window is not in use.
pub(super) const HIDDEN_FACTOR: u64 = 4;

/// Commands of this domain, each for the active account.
pub enum Cmd {
    /// The window is in use (has the focus) or not. While it is not, nothing is polled
    /// more than rarely; when it comes back, what is due is asked at once (the poller
    /// keeps its own spacing, so any number of these makes at most one round).
    WindowActive(bool),
    /// Send the verification email again.
    ResendVerification,
    /// The account accepts the updated terms.
    AcceptTerms,
}

/// Events of this domain, tagged with their account.
pub enum Ev {
    /// The instance's discovery document: the session's own at the start, a newer one
    /// when it changed.
    Discovery(Box<p::Discovery>),
    /// The account's experiment assignments (or that the instance has none).
    Experiments(ExperimentState),
    /// What the status page says now.
    StatusPage(status_page::Status),
    /// The verification email went out, or why not.
    VerificationSent(Result<(), String>),
    /// The terms were accepted (the account as it is now), or why not.
    TermsAccepted(Result<Box<p::CurrentUser>, String>),
}

/// The backend's state of this domain.
pub struct State {
    poller: Option<AbortHandle>,
    wake: Arc<Notify>,
    window_active: Arc<AtomicBool>,
    last_resend: Mutex<Option<Instant>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            poller: None,
            wake: Arc::default(),
            window_active: Arc::new(AtomicBool::new(true)),
            last_resend: Mutex::new(None),
        }
    }
}

impl State {
    /// Whether a verification email may go out at `now`; when it may, the time is taken.
    pub(super) fn resend_allowed(&self, now: Instant) -> bool {
        let mut last = self.last_resend.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| now.saturating_duration_since(at) < RESEND_GAP) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// A number from -1 to 1, not predictable (jitter between polls).
pub(super) fn unit() -> f64 {
    let bits = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (bits as f64 / u64::MAX as f64) * 2.0 - 1.0
}

/// An account became the active one: it is announced, and its poller starts. The
/// previous poller (of the account it replaces) stops first.
pub(super) fn activated(b: &mut Backend, session: &p::Session, gen: u64) {
    stop(b);
    let events = b.events.clone();
    announce(&events, session);
    let wake = Arc::new(Notify::new());
    b.instance.wake = wake.clone();
    let poller = Poller::new(
        session.client().clone(),
        events,
        b.internal.clone(),
        gen,
        wake,
        b.instance.window_active.clone(),
    );
    b.instance.poller = Some(b.spawn_guarded("instance poller", None, poller.run()));
}

/// The account goes to the background: its poller stops.
pub(super) fn stop(b: &mut Backend) {
    if let Some(task) = b.instance.poller.take() {
        task.abort();
    }
}

/// A background account's discovery document, from the session it was built with. No
/// request.
pub(super) fn announce(events: &Ui, session: &p::Session) {
    announce_document(events, session.instance().discovery());
}

fn announce_document(events: &Ui, doc: &p::Discovery) {
    let _ = events.unbounded_send(Ev::Discovery(Box::new(doc.clone())));
}

/// The active account's gateway connected (again): ask what is due.
pub(super) fn reconnected(b: &Backend) {
    b.instance.wake.notify_one();
}

pub(super) fn handle(b: &mut Backend, cmd: Cmd) {
    match cmd {
        Cmd::WindowActive(active) => {
            let was = b.instance.window_active.swap(active, Ordering::Relaxed);
            if active && !was {
                b.instance.wake.notify_one();
            }
        }
        Cmd::ResendVerification => resend_verification(b),
        Cmd::AcceptTerms => accept_terms(b),
    }
}

/// The client and generation of the active account, if it is a real one.
fn active_client(b: &Backend) -> Option<(p::Client, u64)> {
    match &b.active {
        Some(Active::Real(r)) => Some((r.session.client().clone(), r.gen)),
        _ => None,
    }
}

fn resend_verification(b: &mut Backend) {
    #[cfg(feature = "demo")]
    if matches!(b.active, Some(Active::Demo(_))) {
        b.emit(Ev::VerificationSent(Ok(())));
        return;
    }
    let Some((client, gen)) = active_client(b) else {
        b.emit(Ev::VerificationSent(Err(t!("banner-not-connected"))));
        return;
    };
    if !b.instance.resend_allowed(Instant::now()) {
        b.emit(Ev::VerificationSent(Err(t!("banner-verify-wait"))));
        return;
    }
    let events = b.events.clone();
    let internal = b.internal.clone();
    b.spawn_guarded("verification email", None, async move {
        let result = match client.resend_verification_email().await {
            Ok(()) => Ok(()),
            Err(p::Error::RateLimited { .. }) => Err(t!("banner-verify-wait")),
            Err(err) => Err(failure_text(&internal, gen, err)),
        };
        let _ = events.unbounded_send(Ev::VerificationSent(result));
    });
}

fn accept_terms(b: &mut Backend) {
    let Some((client, gen)) = active_client(b) else {
        b.emit(Ev::TermsAccepted(Err(t!("banner-not-connected"))));
        return;
    };
    let events = b.events.clone();
    let internal = b.internal.clone();
    b.spawn_guarded("terms acceptance", None, async move {
        let result = client
            .accept_terms()
            .await
            .map(Box::new)
            .map_err(|err| failure_text(&internal, gen, err));
        let _ = events.unbounded_send(Ev::TermsAccepted(result));
    });
}

/// The sentence for a failed call; a rejected token ends the session instead.
fn failure_text(internal: &TokioSender<Internal>, gen: u64, err: p::Error) -> String {
    let failure = Failure::from(err);
    if failure.kind == FailureKind::SessionEnded {
        let _ = internal.send(Internal::Ended {
            gen,
            message: failure.message.clone(),
        });
    }
    failure.message
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
