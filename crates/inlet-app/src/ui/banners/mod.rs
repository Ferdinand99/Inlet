// SPDX-License-Identifier: AGPL-3.0-or-later
//! The banners over the app, as Fluxer's "nagbars", and what feeds them: the instance's
//! capabilities, the account's own flags and the status page.
//!
//! Which banner shows follows Fluxer's rule: of the banners that apply, the one with the
//! lowest priority number among those the person cannot dismiss, and the one among the
//! dismissible ones, at most two, in order of priority ([`select`]).
//!
//! | Banner | Shows when | Dismissible |
//! | --- | --- | --- |
//! | Connection | the connection to the instance has been down for 3 seconds, or stopped | no |
//! | New terms | the account is behind on the terms or the privacy policy | no |
//! | Email | the instance wants the address confirmed (or it bounces) | no |
//! | Limited account | messaging is paused on the account | no |
//! | Maintenance | the status page has running or planned maintenance | yes, per maintenance and stage |
//! | Instance notice | the administrator's notice is live | yes, per text |
//! | Domain moved | the instance moves to a new domain and reaches this account | yes, for 7 days |
//!
//! The capabilities live in each account's [`inlet_state::AccountState`]; the answers of
//! the backend (`backend::instance`) are kept per account here until the state has them
//! ([`Inlet::sync_instance_info`]), so a discovery document that arrives before READY, or
//! for an account in the background, is not lost.
//!
//! Dismissals are kept in the `"banners"` section of `settings.json`, capped, so a notice
//! stays away after a restart.

mod view;

use std::collections::{BTreeMap, HashMap};

use gpui::{Context, EntityId};
use inlet_protocol as p;
use inlet_state::{Cap, Capabilities, ExperimentState};
use serde::{Deserialize, Serialize};

use crate::app::{Inlet, Screen};
use crate::backend::accounts::{AccountKey, Cmd};
use crate::backend::instance::{status_page, Ev};

/// How long the connection must be down before the banner shows.
pub const CONNECTION_GRACE_MS: u64 = 3_000;
/// How long a dismissed "domain moved" banner stays away.
pub const DOMAIN_MOVED_DISMISS_MS: i64 = 7 * 24 * 3_600_000;
/// Most dismissals remembered.
const MAX_DISMISSED: usize = 200;
/// Longest wait before the clock is looked at again for a notice's start or end.
const LONGEST_TIMER_MS: i64 = 24 * 3_600_000;
const SETTINGS_KEY: &str = "banners";

/// A banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Banner {
    /// The connection to the instance is down.
    Connection,
    /// Updated terms or privacy policy.
    Terms,
    /// Planned or running maintenance.
    Maintenance,
    /// The instance's notice.
    InstanceNotice,
    /// The address needs to be confirmed, or bounces.
    Email,
    /// Messaging is paused on the account.
    AccountLimited,
    /// The instance moved to a new domain.
    DomainMoved,
}

impl Banner {
    /// Lower comes first (Fluxer's `priority`).
    pub fn priority(self) -> f32 {
        match self {
            Banner::Connection => -100.,
            Banner::Terms => -5.,
            Banner::AccountLimited => -3.75,
            Banner::Email => -3.,
            Banner::InstanceNotice => -1.5,
            Banner::Maintenance => -1.,
            Banner::DomainMoved => 3.,
        }
    }

    /// Whether the person can dismiss it.
    pub fn dismissible(self) -> bool {
        matches!(
            self,
            Banner::Maintenance | Banner::InstanceNotice | Banner::DomainMoved
        )
    }
}

/// Fluxer's `selectVisibleNagbars`: of the banners that apply, one the person cannot
/// dismiss (the lowest priority number) and one they can, in order of priority.
pub fn select(applying: &[Banner]) -> Vec<Banner> {
    let mut sorted = applying.to_vec();
    sorted.sort_by(|a, b| a.priority().total_cmp(&b.priority()));
    sorted.dedup();
    let firm = sorted.iter().copied().find(|b| !b.dismissible());
    let light = sorted.iter().copied().find(|b| b.dismissible());
    let mut out: Vec<Banner> = firm.into_iter().chain(light).collect();
    out.sort_by(|a, b| a.priority().total_cmp(&b.priority()));
    out
}

/// What the backend last said about one account's instance.
#[derive(Default)]
pub struct Info {
    discovery: Option<Box<p::Discovery>>,
    experiments: ExperimentState,
    /// Counts the changes, so the account state is brought up to date only when needed.
    generation: u64,
}

/// What is kept of the dismissals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Dismissed notices and maintenance, as `kind|instance|id`.
    pub dismissed: Vec<String>,
    /// When the "domain moved" banner was dismissed, per instance (Unix milliseconds).
    pub moved_dismissed_at: BTreeMap<String, i64>,
}

impl Prefs {
    /// The saved dismissals; none in tests and demo runs.
    fn current() -> Self {
        let demo = cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some();
        if cfg!(test) || demo {
            return Self::default();
        }
        crate::backend::platform::prefs::settings_path()
            .map(|p| crate::backend::platform::prefs::load_section(&p, SETTINGS_KEY))
            .unwrap_or_default()
    }

    fn is_dismissed(&self, id: &str) -> bool {
        self.dismissed.iter().any(|d| d == id)
    }

    /// Remembers a dismissal, keeping the newest [`MAX_DISMISSED`].
    fn dismiss(&mut self, id: String) {
        self.dismissed.retain(|d| *d != id);
        self.dismissed.push(id);
        let extra = self.dismissed.len().saturating_sub(MAX_DISMISSED);
        self.dismissed.drain(..extra);
    }
}

/// The view state of this domain.
#[derive(Default)]
pub struct State {
    info: HashMap<AccountKey, Info>,
    /// Which state entity and which generation of an account's info it has been given.
    applied: HashMap<AccountKey, (EntityId, u64)>,
    prefs: Option<Prefs>,
    /// What the status page says, for the account that asked.
    status: Option<(AccountKey, status_page::Status)>,
    /// The connection status text last seen and since when.
    connection_seen: Option<(String, std::time::Instant)>,
    /// Re-renders after the grace time, or at the next start or end of a notice.
    timer: Option<(i64, gpui::Task<()>)>,
    /// The terms window is open.
    pub terms_open: bool,
    /// Accepting the terms is on its way.
    pub terms_busy: bool,
    /// Why accepting the terms failed.
    pub terms_error: Option<String>,
    /// The "domain moved" steps window is open.
    pub steps_open: bool,
    /// Sending the verification email is on its way.
    pub verify_busy: bool,
    /// The demo's terms scene has been set up.
    #[cfg(feature = "demo")]
    demo_terms_done: bool,
}

impl State {
    /// The windows are closed (another account, signing out).
    pub fn close_windows(&mut self) {
        self.terms_open = false;
        self.terms_busy = false;
        self.terms_error = None;
        self.steps_open = false;
        self.verify_busy = false;
    }

    fn prefs(&mut self) -> &mut Prefs {
        self.prefs.get_or_insert_with(Prefs::current)
    }
}

/// The official instances' new domains (Fluxer's `DOMAIN_MIGRATION_SOURCE_TO_TARGET`),
/// by the host of the instance's address or of its web app.
pub fn moved_target(host: &str) -> Option<&'static str> {
    match host.to_ascii_lowercase().as_str() {
        "fluxer.app" | "web.fluxer.app" => Some("fluxer.com"),
        "canary.fluxer.app" | "web.canary.fluxer.app" => Some("canary.fluxer.com"),
        _ => None,
    }
}

/// The text of a maintenance banner's duration: "90 minutes" becomes "1 hour"-style
/// units only when it divides evenly, as Fluxer's.
pub fn duration_text(minutes: u64) -> String {
    let minutes = minutes.max(1);
    if minutes.is_multiple_of(24 * 60) {
        t!("banner-duration-days", count = minutes / (24 * 60))
    } else if minutes.is_multiple_of(60) {
        t!("banner-duration-hours", count = minutes / 60)
    } else {
        t!("banner-duration-minutes", count = minutes)
    }
}

/// Text from the instance made safe to show: line breaks become spaces, control
/// characters, bidirectional overrides and zero-width characters go, and it is cut to
/// `max` characters (with an ellipsis when cut).
pub fn tidy(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in text.chars() {
        let c = if matches!(c, '\n' | '\r' | '\t') {
            ' '
        } else {
            c
        };
        let hidden = c.is_control()
            || matches!(c,
                '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{061C}');
        if hidden {
            continue;
        }
        if count == max {
            out.push('\u{2026}');
            break;
        }
        out.push(c);
        count += 1;
    }
    out.trim().to_string()
}

impl Inlet {
    /// Whether `cap` is on for the account on screen. Without an account (sign-in, demo
    /// before READY) the defaults of [`Capabilities::new`] count.
    pub(crate) fn has_cap(&self, cx: &gpui::App, cap: Cap) -> bool {
        match &self.account {
            Some(a) => a.read(cx).capabilities().has(cap),
            None => Capabilities::new().has(cap),
        }
    }

    /// An event of the instance domain. `account` is the account whose tasks sent it
    /// (`None` for an untagged one).
    pub(crate) fn on_instance_event(
        &mut self,
        account: Option<AccountKey>,
        ev: Ev,
        cx: &mut Context<Self>,
    ) {
        let key = account.or_else(|| self.switcher.active.clone());
        let Some(key) = key else {
            return;
        };
        let active = self.switcher.is_active(&key);
        match ev {
            Ev::Discovery(doc) => {
                let info = self.banners.info.entry(key).or_default();
                if info.discovery.as_deref() != Some(&*doc) {
                    // The accent follows a change of the instance's colour (the first
                    // document's was applied when the account became the active one).
                    let accent = |d: &p::Discovery| d.branding().theme_color;
                    let changed = info
                        .discovery
                        .as_deref()
                        .is_some_and(|old| accent(old) != accent(&doc));
                    if active && changed {
                        crate::theme::set_accent(
                            accent(&doc).as_deref().and_then(crate::theme::parse_hex),
                        );
                    }
                    info.discovery = Some(doc);
                    info.generation += 1;
                }
            }
            Ev::Experiments(state) => {
                let info = self.banners.info.entry(key).or_default();
                if info.experiments != state {
                    info.experiments = state;
                    info.generation += 1;
                }
            }
            Ev::StatusPage(status) if active => self.banners.status = Some((key, status)),
            Ev::VerificationSent(result) if active => {
                self.banners.verify_busy = false;
                match result {
                    Ok(()) => self.chat.notice = Some(t!("banner-verify-sent")),
                    Err(why) => self.chat.error = Some(why),
                }
            }
            Ev::TermsAccepted(result) if active => {
                self.banners.terms_busy = false;
                match result {
                    Ok(user) => {
                        self.banners.terms_open = false;
                        self.banners.terms_error = None;
                        if let Some(account) = self.account.clone() {
                            let update = p::Dispatch::UserUpdate(user);
                            account.update(cx, |s, _| s.apply(&update));
                        }
                    }
                    Err(why) => self.banners.terms_error = Some(why),
                }
            }
            // Another account's status page, mail or terms: not on screen.
            Ev::StatusPage(_) | Ev::VerificationSent(_) | Ev::TermsAccepted(_) => {}
        }
        self.sync_instance_info(cx);
        cx.notify();
    }

    /// Brings the account on screen up to date with what the backend said about its
    /// instance. Cheap when nothing changed; also catches a state made after the
    /// answer arrived (the first READY) and an account switched to.
    pub(crate) fn sync_instance_info(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(account)) = (self.switcher.active.clone(), self.account.clone())
        else {
            return;
        };
        let Some(info) = self.banners.info.get(&key) else {
            return;
        };
        let marker = (account.entity_id(), info.generation);
        if self.banners.applied.get(&key) == Some(&marker) {
            return;
        }
        let (doc, experiments) = (info.discovery.clone(), info.experiments);
        self.banners.applied.insert(key, marker);
        account.update(cx, |s, _| {
            if let Some(doc) = &doc {
                s.set_discovery(doc);
            }
            s.set_experiments(experiments);
        });
        // The demo's terms scene: the instance changed its terms; the window is open.
        #[cfg(feature = "demo")]
        if crate::demo::instance::terms_scene() && !self.banners.demo_terms_done {
            self.banners.demo_terms_done = true;
            self.banners.terms_open = true;
            account.update(cx, |s, _| {
                s.set_policy_dates(inlet_state::PolicyDates {
                    terms_updated_ms: Some(1),
                    privacy_updated_ms: None,
                })
            });
        }
    }

    /// The window has the focus or not: the backend polls less while it does not.
    pub(crate) fn banners_window_active(&mut self, active: bool) {
        self.backend
            .send(crate::backend::instance::Cmd::WindowActive(active));
    }

    /// What the About view says about the instance the active account is on: its name
    /// from its branding, its address and its status page.
    pub(crate) fn about_instance(&self, cx: &gpui::App) -> Option<crate::about::InstanceLine> {
        let key = self.switcher.active.as_ref()?;
        let account = self.account.as_ref()?;
        let branding = account.read(cx).capabilities().branding();
        let host = url::Url::parse(&key.origin.to_string())
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let name = match &branding.product_name {
            Some(n) => tidy(n, 60),
            None => host.clone(),
        };
        // Only an address the status page poller would also ask is shown as a link.
        let status_page = branding
            .status_page_url
            .filter(|u| crate::backend::instance::status_page::base_url(u).is_some());
        Some(crate::about::InstanceLine {
            name,
            host: tidy(&host, 100),
            status_page,
        })
    }

    /// The dismissal id of a notice or maintenance, by the instance it came from.
    fn dismissal_id(&self, kind: &str, id: &str) -> String {
        let origin = self
            .switcher
            .active
            .as_ref()
            .map(|k| k.origin.to_string())
            .unwrap_or_default();
        format!("{kind}|{origin}|{id}")
    }

    /// Dismisses a banner, and remembers it.
    pub(crate) fn dismiss_banner(&mut self, banner: Banner, now_ms: i64, cx: &mut Context<Self>) {
        let origin = self
            .switcher
            .active
            .as_ref()
            .map(|k| k.origin.to_string())
            .unwrap_or_default();
        let id = match banner {
            Banner::InstanceNotice => self
                .account
                .as_ref()
                .and_then(|a| a.read(cx).capabilities().notice())
                .map(|n| self.dismissal_id("notice", &n.id)),
            Banner::Maintenance => self.banners.status.as_ref().and_then(|(_, s)| {
                s.maintenance
                    .as_ref()
                    .map(|m| self.dismissal_id("maintenance", &format!("{}:{:?}", m.id, m.state)))
            }),
            Banner::DomainMoved => {
                self.banners
                    .prefs()
                    .moved_dismissed_at
                    .insert(origin, now_ms);
                None
            }
            _ => return,
        };
        if let Some(id) = id {
            self.banners.prefs().dismiss(id);
        }
        self.save_banner_prefs(cx);
        cx.notify();
    }

    fn save_banner_prefs(&mut self, cx: &mut Context<Self>) {
        let prefs = self.banners.prefs().clone();
        // Saved off the UI thread; a failed write only loses the dismissal.
        if cfg!(test) || (cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some()) {
            return;
        }
        cx.background_executor()
            .spawn(async move {
                if let Some(path) = crate::backend::platform::prefs::settings_path() {
                    if let Err(e) =
                        crate::backend::platform::prefs::save_section(&path, SETTINGS_KEY, &prefs)
                    {
                        log::warn!("could not save the dismissed banners: {e}");
                    }
                }
            })
            .detach();
    }

    /// Whether the connection to the instance is down, after the grace time. Notes the
    /// status text it sees, and sets a timer that renders again when the grace is over.
    fn connection_lost(&mut self, cx: &mut Context<Self>) -> bool {
        if self.chat.gateway_down.is_some() {
            return true;
        }
        let text = self.chat.status.clone();
        let connected = text.is_empty() || text == t!("status-connected") || is_demo_status(&text);
        if connected {
            self.banners.connection_seen = None;
            return false;
        }
        let since = match &self.banners.connection_seen {
            Some((seen, at)) if *seen == text => *at,
            // Another text of "not connected" (counting down, trying again) is the same
            // outage: the clock keeps running.
            Some((_, at)) => *at,
            None => std::time::Instant::now(),
        };
        self.banners.connection_seen = Some((text, since));
        let waited = since.elapsed().as_millis() as u64;
        if waited >= CONNECTION_GRACE_MS {
            return true;
        }
        self.schedule_render_in(CONNECTION_GRACE_MS - waited, cx);
        false
    }

    /// Renders again after `ms`, once (a newer timer replaces an older one).
    fn schedule_render_in(&mut self, ms: u64, cx: &mut Context<Self>) {
        let due = now_ms() + ms as i64;
        if self
            .banners
            .timer
            .as_ref()
            .is_some_and(|(at, _)| *at <= due + 50 && *at >= due - 50)
        {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(ms))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        self.banners.timer = Some((due, task));
    }

    /// The banners that apply now, before [`select`] picks.
    fn applying_banners(&mut self, cx: &mut Context<Self>) -> Vec<Banner> {
        let Some(account) = self.account.clone() else {
            return Vec::new();
        };
        let now = now_ms();
        let (notices, caps, origin_host) = {
            let state = account.read(cx);
            (
                state.notices(),
                state.capabilities().clone(),
                self.switcher.active.as_ref().map(|k| k.origin.to_string()),
            )
        };
        let mut out = Vec::new();
        if self.connection_lost(cx) {
            out.push(Banner::Connection);
        }
        if notices.terms != inlet_state::TermsUpdate::None {
            out.push(Banner::Terms);
        }
        if notices.confirm_email || notices.email_bounced {
            out.push(Banner::Email);
        }
        if notices.limited {
            out.push(Banner::AccountLimited);
        }
        if let Some(n) = caps.live_notice(now) {
            let id = self.dismissal_id("notice", &n.id);
            if !self.banners.prefs().is_dismissed(&id) {
                out.push(Banner::InstanceNotice);
            }
            // The next start or end of the notice changes what shows.
            let next = [n.starts_at_ms, n.ends_at_ms]
                .into_iter()
                .flatten()
                .filter(|t| *t > now)
                .min();
            if let Some(next) = next {
                self.schedule_render_in((next - now).min(LONGEST_TIMER_MS) as u64, cx);
            }
        } else if let Some(n) = caps.notice() {
            if n.starts_at_ms.is_some_and(|s| s > now) {
                self.schedule_render_in(
                    (n.starts_at_ms.unwrap_or(now) - now).min(LONGEST_TIMER_MS) as u64,
                    cx,
                );
            }
        }
        let maintenance = self
            .banners
            .status
            .as_ref()
            .filter(|(k, _)| self.switcher.is_active(k))
            .and_then(|(_, s)| s.maintenance.clone());
        if let Some(m) = maintenance {
            let id = self.dismissal_id("maintenance", &format!("{}:{:?}", m.id, m.state));
            if !self.banners.prefs().is_dismissed(&id) {
                out.push(Banner::Maintenance);
            }
        }
        if let Some(origin) = origin_host {
            let host = url::Url::parse(&origin)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default();
            if caps.domain_moved() && moved_target(&host).is_some() {
                let dismissed = self
                    .banners
                    .prefs()
                    .moved_dismissed_at
                    .get(&origin)
                    .is_some_and(|at| now - *at < DOMAIN_MOVED_DISMISS_MS);
                if !dismissed {
                    out.push(Banner::DomainMoved);
                }
            }
        }
        out
    }

    /// The banners to draw now (see [`select`]), on the main screen only.
    pub(crate) fn visible_banners(&mut self, cx: &mut Context<Self>) -> Vec<Banner> {
        if self.screen != Screen::Main {
            return Vec::new();
        }
        self.sync_instance_info(cx);
        let applying = self.applying_banners(cx);
        select(&applying)
    }

    /// Sends the verification email again.
    pub(crate) fn resend_verification(&mut self, cx: &mut Context<Self>) {
        if self.banners.verify_busy {
            return;
        }
        self.banners.verify_busy = true;
        self.backend
            .send(crate::backend::instance::Cmd::ResendVerification);
        cx.notify();
    }

    /// Accepts the updated terms.
    pub(crate) fn accept_terms(&mut self, cx: &mut Context<Self>) {
        if self.banners.terms_busy {
            return;
        }
        self.banners.terms_busy = true;
        self.banners.terms_error = None;
        self.backend
            .send(crate::backend::instance::Cmd::AcceptTerms);
        cx.notify();
    }

    /// Tries to connect again after the gateway stopped.
    pub(crate) fn retry_connection(&mut self, cx: &mut Context<Self>) {
        self.chat.gateway_down = None;
        self.chat.status = t!("status-reconnecting");
        self.backend
            .send(Cmd::Reconnect(self.switcher.active.clone()));
        cx.notify();
    }
}

/// The status line the demo gateway shows: connected, as far as banners go.
fn is_demo_status(text: &str) -> bool {
    matches!(text, "Demo mode")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests;
