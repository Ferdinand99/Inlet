// SPDX-License-Identifier: AGPL-3.0-or-later
//! The poller of the active account: one task with three clocks (the discovery document,
//! the experiments, the status page). See the module docs of `backend::instance` for
//! how often each is asked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use inlet_protocol as p;
use inlet_protocol::api::experiments::MAX_NOT_FOUND_STREAK;
use inlet_state::ExperimentState;
use tokio::sync::mpsc::UnboundedSender as TokioSender;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::{
    status_page, Ev, DISCOVERY_EVERY, DISCOVERY_MIN_GAP, DISCOVERY_RETRY, EXPERIMENTS_MIN_GAP,
    HIDDEN_FACTOR,
};
use crate::backend::session::{Failure, FailureKind};
use crate::backend::{Internal, Ui};

/// The poller of the active account.
pub(super) struct Poller {
    client: p::Client,
    events: Ui,
    internal: TokioSender<Internal>,
    gen: u64,
    wake: Arc<Notify>,
    window_active: Arc<AtomicBool>,
    /// Draws the jitter (-1 to 1); a fixed one in the tests.
    jitter: fn() -> f64,
    /// Every wait is divided by this (1 in the app; large in the tests).
    scale: u32,
}

/// When each of the three things is next due, and when it last was asked, so that a
/// wake (a reconnect, the window in use) brings forward only what the spacing allows.
pub(super) struct Clocks {
    pub(super) disc_due: Instant,
    pub(super) last_disc: Instant,
    pub(super) exp_due: Instant,
    pub(super) last_exp: Option<Instant>,
    pub(super) status_due: Instant,
    pub(super) last_status: Option<Instant>,
}

impl Clocks {
    pub(super) fn new(start: Instant, discovery_every: Duration) -> Self {
        Clocks {
            disc_due: start + discovery_every,
            last_disc: start,
            exp_due: start,
            last_exp: None,
            status_due: start,
            last_status: None,
        }
    }

    /// A reconnect or a focus change at `now`: what its minimum gap allows is due now.
    /// A burst of these changes nothing after the first.
    pub(super) fn wake(&mut self, now: Instant, experiments: bool, status: bool) {
        let gap = |last: Instant, min: Duration| now.saturating_duration_since(last) >= min;
        if gap(self.last_disc, DISCOVERY_MIN_GAP) {
            self.disc_due = now;
        }
        if experiments && self.last_exp.is_none_or(|l| gap(l, EXPERIMENTS_MIN_GAP)) {
            self.exp_due = now;
        }
        if status && self.last_status.is_none_or(|l| gap(l, EXPERIMENTS_MIN_GAP)) {
            self.status_due = now;
        }
    }
}

/// The experiments' polling state.
struct Experiments {
    current: p::Experiments,
    etag: Option<String>,
    failures: u32,
    not_found: u32,
    /// An answer has been taken in (so the first one is always reported).
    known: bool,
    polling: bool,
}

impl Poller {
    pub(super) fn new(
        client: p::Client,
        events: Ui,
        internal: TokioSender<Internal>,
        gen: u64,
        wake: Arc<Notify>,
        window_active: Arc<AtomicBool>,
    ) -> Self {
        Poller {
            client,
            events,
            internal,
            gen,
            wake,
            window_active,
            jitter: super::unit,
            scale: 1,
        }
    }

    /// For the tests: no jitter and every wait divided by `scale`.
    #[cfg(test)]
    pub(super) fn for_test(mut self, scale: u32) -> Self {
        self.jitter = || 0.0;
        self.scale = scale;
        self
    }

    fn scaled(&self, d: Duration) -> Duration {
        d / self.scale
    }

    /// The status page's address from the branding, when the instance says it is an
    /// Instatus page, and the address passes the checks.
    fn status_page(doc: &p::Discovery) -> Option<url::Url> {
        let b = doc.branding();
        if !b.status_page_instatus {
            return None;
        }
        status_page::base_url(&b.status_page_url?)
    }

    pub(super) async fn run(self) {
        let instance = self.client.instance().clone();
        let mut disc_etag = instance.etag().map(str::to_owned);
        let mut disc_retry = DISCOVERY_RETRY;
        let mut clocks = Clocks::new(Instant::now(), self.scaled(DISCOVERY_EVERY));
        let mut exp = Experiments {
            current: p::Experiments::default(),
            etag: None,
            failures: 0,
            not_found: 0,
            known: false,
            polling: true,
        };
        let mut status_url = Self::status_page(instance.discovery());
        let mut status_http = status_url.as_ref().and_then(|_| status_page::client());
        let mut status_failures = 0u32;
        let mut shown_status = status_page::Status::default();

        loop {
            let status_on = status_url.is_some() && status_http.is_some();
            let mut next = clocks.disc_due;
            if exp.polling {
                next = next.min(clocks.exp_due);
            }
            if status_on {
                next = next.min(clocks.status_due);
            }
            tokio::select! {
                () = tokio::time::sleep_until(next) => {}
                () = self.wake.notified() => clocks.wake(Instant::now(), exp.polling, status_on),
            }
            let now = Instant::now();
            if now >= clocks.disc_due {
                clocks.last_disc = now;
                match instance.fetch_discovery(disc_etag.as_deref()).await {
                    Ok(None) => {
                        disc_retry = DISCOVERY_RETRY;
                        clocks.disc_due = now + self.scaled(DISCOVERY_EVERY);
                    }
                    Ok(Some((doc, etag))) => {
                        disc_retry = DISCOVERY_RETRY;
                        clocks.disc_due = now + self.scaled(DISCOVERY_EVERY);
                        disc_etag = etag;
                        let url = Self::status_page(&doc);
                        if url != status_url {
                            // The instance moved or dropped its status page.
                            status_http = url
                                .as_ref()
                                .and_then(|_| status_http.take().or_else(status_page::client));
                            status_url = url;
                            clocks.status_due = now;
                            clocks.last_status = None;
                            status_failures = 0;
                            shown_status = status_page::Status::default();
                            let _ = self
                                .events
                                .unbounded_send(Ev::StatusPage(shown_status.clone()));
                        }
                        let _ = self.events.unbounded_send(Ev::Discovery(Box::new(doc)));
                    }
                    Err(err) => {
                        log::warn!("discovery could not be read again: {err}");
                        clocks.disc_due = now + self.scaled(disc_retry);
                        disc_retry = (disc_retry * 2).min(Duration::from_secs(3600));
                    }
                }
            }
            if exp.polling && now >= clocks.exp_due {
                self.poll_experiments(&mut exp, &mut clocks).await;
            }
            let (Some(url), Some(http)) = (&status_url, &status_http) else {
                continue;
            };
            if now < clocks.status_due {
                continue;
            }
            if !self.window_active.load(Ordering::Relaxed) {
                // Nothing while the window is not in use; asked again when it is.
                clocks.status_due = now + self.scaled(Duration::from_secs(300));
                continue;
            }
            clocks.last_status = Some(now);
            match status_page::fetch(http, url).await {
                status_page::Fetched::Status(s) => {
                    status_failures = 0;
                    if s != shown_status {
                        shown_status = s.clone();
                        let _ = self.events.unbounded_send(Ev::StatusPage(s));
                    }
                }
                status_page::Fetched::Failed => status_failures += 1,
                status_page::Fetched::Unsupported => {
                    // Not an Instatus page: never again for this address.
                    status_url = None;
                    status_http = None;
                    if shown_status != status_page::Status::default() {
                        shown_status = status_page::Status::default();
                        let _ = self
                            .events
                            .unbounded_send(Ev::StatusPage(shown_status.clone()));
                    }
                    continue;
                }
            }
            let wait = status_page::next_delay(
                &shown_status,
                status_failures,
                super::now_ms(),
                (self.jitter)().abs(),
            );
            clocks.status_due = Instant::now() + self.scaled(wait);
        }
    }

    async fn poll_experiments(&self, exp: &mut Experiments, clocks: &mut Clocks) {
        clocks.last_exp = Some(Instant::now());
        match self.client.experiments(exp.etag.as_deref()).await {
            Ok(p::ExperimentsAnswer::Fresh(e, etag)) => {
                exp.failures = 0;
                exp.not_found = 0;
                exp.etag = etag;
                let changed = exp.current != e || !exp.known;
                exp.current = e;
                exp.known = true;
                if changed {
                    let _ = self
                        .events
                        .unbounded_send(Ev::Experiments(ExperimentState::Loaded(e)));
                }
            }
            Ok(p::ExperimentsAnswer::NotModified) => {
                exp.failures = 0;
                exp.not_found = 0;
            }
            Ok(p::ExperimentsAnswer::Malformed) => exp.failures = 0,
            Ok(p::ExperimentsAnswer::NotFound) => {
                exp.not_found += 1;
                exp.failures += 1;
                if exp.not_found >= MAX_NOT_FOUND_STREAK {
                    log::warn!("the instance has no experiments route; not asking again");
                    exp.polling = false;
                    let _ = self
                        .events
                        .unbounded_send(Ev::Experiments(ExperimentState::Unavailable));
                    return;
                }
            }
            Err(err) => {
                let failure = Failure::from(err);
                if failure.kind == FailureKind::SessionEnded {
                    let _ = self.internal.send(Internal::Ended {
                        gen: self.gen,
                        message: failure.message,
                    });
                    exp.polling = false;
                    return;
                }
                log::warn!("the experiments could not be read: {}", failure.message);
                exp.failures += 1;
            }
        }
        let hidden = !self.window_active.load(Ordering::Relaxed);
        let mut wait = exp.current.next_poll(exp.failures, false, (self.jitter)());
        if hidden {
            wait = wait.max(Duration::from_secs(
                exp.current.poll_interval_secs * HIDDEN_FACTOR,
            ));
        }
        clocks.exp_due = Instant::now() + self.scaled(wait);
    }
}
