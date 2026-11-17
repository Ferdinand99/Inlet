// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the helper does about each callback of the web engine: a navigation, a page
//! load, a refused new window or download. No window and no engine in here, so the
//! decisions can be tested without a display.
//!
//! The rule for frames, the same on every platform: the window itself (a top-level
//! navigation) may only go to hosts in `allow`; a frame may go to `allow` and to `frames`;
//! a frame of any other host ends the flow like a refused navigation.
//!
//! How each engine gets there differs, because wry 0.57 reports a frame's navigation like
//! the window's on Linux and macOS, and not at all on Windows (where the helper hooks
//! the engine's frame callback and calls [`Gate::frame`]):
//!
//! - Windows: [`Gate::navigation`] sees the window only, [`Gate::frame`] sees the frames.
//! - Linux and macOS: [`Gate::navigation`] sees both and cannot tell them apart. A URL
//!   that only `frames` allows is let through, and remembered. When it was the window
//!   after all, the engine reports a page load for it ([`Gate::page_load`] sees window
//!   loads only), and the flow ends then. For that one case the request to a host of the
//!   flow's own `frames` list has started by then; nothing else is let through.

use std::cell::{Cell, RefCell};

use crate::policy::{BlockReason, Policy, Verdict};
use crate::protocol::{truncate_url, Event, MAX_BLOCKED_URL_LEN};

/// The most refused new windows and downloads reported before the page is taken to be
/// hostile and the flow ends.
pub const MAX_REFUSALS: u32 = 10;

/// What to do about a navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Let the engine go on.
    Load,
    /// Stop the navigation, report the event and close the window.
    Stop(Event),
}

/// What to do about a refused new window or download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Report it; the window stays.
    Report(Event),
    /// Too many: report it and close the window.
    EndFlow(Event),
}

/// The decisions of one window.
pub struct Gate {
    policy: Policy,
    /// Whether [`Gate::navigation`] is called for frames as well (Linux and macOS).
    frames_reported: bool,
    /// The last URL let through only because `frames` allows it.
    tentative: RefCell<Option<String>>,
    refusals: Cell<u32>,
}

impl Gate {
    pub fn new(policy: Policy, frames_reported: bool) -> Gate {
        Gate {
            policy,
            frames_reported,
            tentative: RefCell::new(None),
            refusals: Cell::new(0),
        }
    }

    /// The engine is about to navigate to `url`.
    pub fn navigation(&self, url: &str) -> Decision {
        // The engine's own empty page, which it may report before the first load. It
        // holds nothing and loads nothing.
        if url == "about:blank" {
            return Decision::Load;
        }
        match self.policy.check(url) {
            Verdict::Allow => Decision::Load,
            Verdict::Intercept => Decision::Stop(intercepted(url)),
            Verdict::Block(BlockReason::NotAllowed)
                if self.frames_reported && self.policy.check_frame(url) == Verdict::Allow =>
            {
                *self.tentative.borrow_mut() = Some(url.to_string());
                Decision::Load
            }
            Verdict::Block(_) => Decision::Stop(blocked(url)),
        }
    }

    /// A frame of the page is about to navigate to `url` (Windows only: the engine's
    /// frame callback).
    pub fn frame(&self, url: &str) -> Decision {
        if url == "about:blank" {
            return Decision::Load;
        }
        match self.policy.check_frame(url) {
            Verdict::Allow => Decision::Load,
            Verdict::Intercept => Decision::Stop(intercepted(url)),
            Verdict::Block(_) => Decision::Stop(blocked(url)),
        }
    }

    /// The window started or finished loading `url`. The engine reports the window's
    /// loads only, so only `allow` counts here. The second look at what [`Gate::navigation`]
    /// let through.
    pub fn page_load(&self, url: &str) -> Option<Event> {
        if url == "about:blank" {
            return None;
        }
        match self.policy.check(url) {
            Verdict::Allow => None,
            Verdict::Intercept => Some(intercepted(url)),
            Verdict::Block(_) => {
                self.tentative.borrow_mut().take();
                Some(blocked(url))
            }
        }
    }

    /// The last URL that was let through only as a possible frame.
    pub fn tentative(&self) -> Option<String> {
        self.tentative.borrow().clone()
    }

    /// A new window or a download was refused.
    pub fn refusal(&self, url: &str) -> Refusal {
        let count = self.refusals.get().saturating_add(1);
        self.refusals.set(count);
        let event = blocked(url);
        if count > MAX_REFUSALS {
            Refusal::EndFlow(event)
        } else {
            Refusal::Report(event)
        }
    }
}

fn intercepted(url: &str) -> Event {
    Event::Intercepted {
        url: url.to_string(),
    }
}

fn blocked(url: &str) -> Event {
    Event::Blocked {
        url: truncate_url(url, MAX_BLOCKED_URL_LEN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyOptions;

    fn gate(frames_reported: bool) -> Gate {
        let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let policy = Policy::new(
            &strings(&["id.example"]),
            &strings(&["https://id.example/done"]),
            PolicyOptions::default(),
        )
        .unwrap()
        .with_frames(&strings(&["captcha.example"]))
        .unwrap();
        Gate::new(policy, frames_reported)
    }

    fn stop_blocked(url: &str) -> Decision {
        Decision::Stop(blocked(url))
    }

    #[test]
    fn the_window_goes_to_allowed_hosts_only_on_every_platform() {
        for frames_reported in [false, true] {
            let g = gate(frames_reported);
            assert_eq!(g.navigation("https://id.example/a"), Decision::Load);
            assert_eq!(
                g.navigation("https://evil.example/"),
                stop_blocked("https://evil.example/")
            );
            assert_eq!(
                g.navigation("data:text/html,x"),
                stop_blocked("data:text/html,x")
            );
            assert_eq!(
                g.navigation("https://id.example/done?code=1"),
                Decision::Stop(intercepted("https://id.example/done?code=1"))
            );
        }
    }

    #[test]
    fn windows_the_window_may_not_go_to_a_frame_host_and_frames_use_their_own_callback() {
        let g = gate(false);
        assert_eq!(
            g.navigation("https://captcha.example/"),
            stop_blocked("https://captcha.example/")
        );
        assert_eq!(g.frame("https://captcha.example/widget"), Decision::Load);
        assert_eq!(g.frame("https://id.example/frame"), Decision::Load);
        assert_eq!(
            g.frame("https://evil.example/"),
            stop_blocked("https://evil.example/")
        );
        assert_eq!(
            g.frame("https://id.example/done?code=1"),
            Decision::Stop(intercepted("https://id.example/done?code=1"))
        );
    }

    #[test]
    fn linux_and_macos_let_a_frame_host_through_and_catch_a_window_load_of_it() {
        let g = gate(true);
        // A frame of a listed host: the navigation callback lets it through.
        assert_eq!(
            g.navigation("https://captcha.example/widget"),
            Decision::Load
        );
        assert_eq!(
            g.tentative().as_deref(),
            Some("https://captcha.example/widget")
        );
        // Frames never produce a page load, so nothing more happens.
        // A frame of any other host ends the flow.
        assert_eq!(
            g.navigation("https://evil.example/frame"),
            stop_blocked("https://evil.example/frame")
        );
        // The window itself went there after all: its page load ends the flow.
        assert_eq!(
            g.page_load("https://captcha.example/widget"),
            Some(blocked("https://captcha.example/widget"))
        );
        assert_eq!(g.tentative(), None);
    }

    #[test]
    fn a_page_load_of_an_allowed_host_is_fine_and_other_loads_end_the_flow() {
        let g = gate(true);
        assert_eq!(g.page_load("https://id.example/login"), None);
        assert_eq!(g.page_load("about:blank"), None);
        assert_eq!(
            g.page_load("https://id.example/done?code=1"),
            Some(intercepted("https://id.example/done?code=1"))
        );
        assert_eq!(
            g.page_load("https://evil.example/"),
            Some(blocked("https://evil.example/"))
        );
        assert_eq!(
            g.page_load("javascript:alert(1)"),
            Some(blocked("javascript:alert(1)"))
        );
    }

    #[test]
    fn the_empty_page_is_let_through() {
        for frames_reported in [false, true] {
            let g = gate(frames_reported);
            assert_eq!(g.navigation("about:blank"), Decision::Load);
            assert_eq!(g.frame("about:blank"), Decision::Load);
        }
        // But `about:srcdoc` and the like are not.
        assert_eq!(
            gate(true).navigation("about:srcdoc"),
            stop_blocked("about:srcdoc")
        );
    }

    #[test]
    fn a_long_blocked_url_is_cut_in_the_event() {
        let url = format!("https://evil.example/{}", "a".repeat(5000));
        match gate(true).navigation(&url) {
            Decision::Stop(Event::Blocked { url }) => {
                assert_eq!(url.chars().count(), MAX_BLOCKED_URL_LEN);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refusals_are_reported_a_few_times_and_then_end_the_flow() {
        let g = gate(true);
        for i in 0..MAX_REFUSALS {
            assert_eq!(
                g.refusal(&format!("https://id.example/popup{i}")),
                Refusal::Report(blocked(&format!("https://id.example/popup{i}"))),
                "refusal {i}"
            );
        }
        assert_eq!(
            g.refusal("https://id.example/popup"),
            Refusal::EndFlow(blocked("https://id.example/popup"))
        );
        // And it stays ended.
        assert!(matches!(
            g.refusal("https://id.example/x"),
            Refusal::EndFlow(_)
        ));
    }
}
