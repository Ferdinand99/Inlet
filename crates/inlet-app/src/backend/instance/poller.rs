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
    status_page, Ev, Latest, LatestMap, DISCOVERY_EVERY, DISCOVERY_MIN_GAP, DISCOVERY_RETRY,
    EXPERIMENTS_MIN_GAP, HIDDEN_FACTOR,
};
use crate::backend::accounts::AccountKey;
use crate::backend::session::{Failure, FailureKind};
use crate::backend::{Internal, Ui};

/// How often the discovery document is read again on a self-hosted instance while the
/// window has the focus, so an administrator's notice shows soon (Fluxer's client does
/// the same: `InstanceNoticeState`).
pub(super) const SELF_HOSTED_EVERY: Duration = Duration::from_secs(300);

/// The poller of the active account.
pub(super) struct Poller {
    client: p::Client,
    events: Ui,
    internal: TokioSender<Internal>,
    gen: u64,
    wake: Arc<Notify>,
    /// Set when the wake may also bring the experiments forward.
    wake_experiments: Arc<AtomicBool>,
    window_active: Arc<AtomicBool>,
    key: AccountKey,
    latest: LatestMap,
    seed: Latest,
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
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        client: p::Client,
        events: Ui,
        internal: TokioSender<Internal>,
        gen: u64,
        wake: Arc<Notify>,
        window_active: Arc<AtomicBool>,
        wake_experiments: Arc<AtomicBool>,
        key: AccountKey,
        latest: LatestMap,
        seed: Latest,
    ) -> Self {
        Poller {
            client,
            events,
            internal,
            gen,
            wake,
            wake_experiments,
            window_active,
            key,
            latest,
            seed,
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

    /// Notes the newest document, so a later activation of this account continues from it.
    fn note(&self, doc: &p::Discovery, etag: &Option<String>, at: Instant) {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                self.key.clone(),
                Latest {
                    doc: doc.clone(),
                    etag: etag.clone(),
                    at,
                },
            );
    }

    pub(super) async fn run(self) {
        let instance = self.client.instance().clone();
        // Where the account's discovery document stands: its newest copy, not the one the
        // session was built with. One that is more than the minimum gap old is read again
        // at once (with its ETag: usually a 304).
        let mut doc = self.seed.doc.clone();
        let mut disc_etag = self.seed.etag.clone();
        let mut self_hosted = doc.instance_features().self_hosted;
        let mut disc_retry = DISCOVERY_RETRY;
        let start = Instant::now();
        let every = |self_hosted: bool| {
            self.scaled(if self_hosted {
                SELF_HOSTED_EVERY
            } else {
                DISCOVERY_EVERY
            })
        };
        let mut clocks = Clocks::new(start, every(self_hosted));
        clocks.last_disc = self.seed.at;
        clocks.disc_due = if start.saturating_duration_since(self.seed.at) >= DISCOVERY_MIN_GAP {
            start
        } else {
            self.seed.at + every(self_hosted)
        };
        let mut exp = Experiments {
            current: p::Experiments::default(),
            etag: None,
            failures: 0,
            not_found: 0,
            known: false,
            polling: true,
        };
        // Started while the window is not in use: the first poll waits its turn, as in
        // Fluxer's client.
        if !self.window_active.load(Ordering::Relaxed) {
            clocks.exp_due = start
                + self.scaled(Duration::from_secs(
                    exp.current.poll_interval_secs * HIDDEN_FACTOR,
                ));
        }
        let mut status_url = Self::status_page(&doc);
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
                () = self.wake.notified() => {
                    // Only the window coming back brings the experiments forward.
                    let experiments = self.wake_experiments.swap(false, Ordering::Relaxed);
                    clocks.wake(Instant::now(), exp.polling && experiments, status_on);
                }
            }
            let now = Instant::now();
            if now >= clocks.disc_due {
                if self_hosted && !self.window_active.load(Ordering::Relaxed) {
                    // The quick refresh is for a window in use; asked again when it is.
                    clocks.disc_due = now + every(true);
                } else {
                    clocks.last_disc = now;
                    match instance.fetch_discovery(disc_etag.as_deref()).await {
                        Ok(None) => {
                            disc_retry = DISCOVERY_RETRY;
                            clocks.disc_due = now + every(self_hosted);
                            self.note(&doc, &disc_etag, now);
                        }
                        Ok(Some((fresh, etag))) => {
                            disc_retry = DISCOVERY_RETRY;
                            self_hosted = fresh.instance_features().self_hosted;
                            clocks.disc_due = now + every(self_hosted);
                            disc_etag = etag;
                            doc = fresh;
                            self.note(&doc, &disc_etag, now);
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
                            let _ = self
                                .events
                                .unbounded_send(Ev::Discovery(Box::new(doc.clone())));
                        }
                        Err(err) => {
                            log::warn!("discovery could not be read again: {err}");
                            clocks.disc_due = now + self.scaled(disc_retry);
                            disc_retry = (disc_retry * 2).min(Duration::from_secs(3600));
                        }
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
            Ok(p::ExperimentsAnswer::Malformed) => {
                exp.failures = 0;
                exp.not_found = 0;
            }
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
                // Only 404 in a row count towards giving up (as in Fluxer's client).
                exp.not_found = 0;
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
