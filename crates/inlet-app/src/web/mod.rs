// SPDX-License-Identifier: AGPL-3.0-or-later
//! The embedded web view: how a flow that needs a real web page shows one.
//!
//! gpui has no web view, so the page runs in the `inlet-web` helper program (the
//! `inlet-web` crate), which ships next to `inlet`. [`open`] starts it, asks for one
//! window and returns a [`WebSession`]: a stream of what happens in it. The window opens
//! over Inlet's main window, always on top of it, and ends with the session.
//!
//! The helper is handed a URL and the rules for it, never a token or a cookie, and none
//! of Inlet's environment (see `inlet_web::client::helper_env`). A flow that needs the
//! signed-in user's token on the page does not exist: the page signs in by itself, and
//! what comes back is the URL it ends on ([`WebEvent::Intercepted`]).
//!
//! The rules (which hosts the window may go to, which URLs end the flow) are the flow's:
//! it names its hosts in [`WebFlow::allow`] and its end in [`WebFlow::intercept`].
//! Everything else is stopped, and so is HTTP. See `inlet_web::policy` for how the
//! patterns are read.
//!
//! Events carry URLs from a web page. They are untrusted data: parse them, do not
//! assume the host, and never open them.

use inlet_web::client::{ClientOptions, OpenError};
use inlet_web::protocol::{Bounds, OpenRequest, Parent, Size, MAX_WINDOW, MIN_WINDOW};

pub use inlet_web::client::Session as WebSession;
pub use inlet_web::protocol::{ErrorKind as WebErrorKind, Event as WebEvent};

/// What a flow asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebFlow {
    /// The first page. `https`, on one of the hosts of `allow`.
    pub url: String,
    /// The hosts the window may go to: `example.com`, `*.example.com`, optionally with a
    /// port (`example.com:8443`).
    pub allow: Vec<String>,
    /// URL prefixes that end the flow; the window reports the URL and closes.
    pub intercept: Vec<String>,
    /// Hosts that may appear as frames inside the page, and only as frames (a captcha, a
    /// payment form). Frames of any other host end the flow. Empty for a page that
    /// embeds nothing.
    pub frames: Vec<String>,
    /// The window's title, already translated. Inlet's name is added.
    pub title: String,
    /// The page area, in logical pixels.
    pub size: (u32, u32),
}

impl WebFlow {
    /// The request for the helper. The size is held to what the helper accepts.
    pub fn into_request(self, parent: Option<Parent>) -> OpenRequest {
        OpenRequest {
            url: self.url,
            allow: self.allow,
            intercept: self.intercept,
            frames: self.frames,
            title: window_title(&self.title),
            size: Size {
                width: self.size.0.clamp(MIN_WINDOW, MAX_WINDOW),
                height: self.size.1.clamp(MIN_WINDOW, MAX_WINDOW),
            },
            parent,
        }
    }
}

/// The title of a web view window: the flow's own, then Inlet, as in Inlet's other
/// windows.
pub fn window_title(title: &str) -> String {
    // The helper takes no control characters and a bounded length.
    let clean: String = title
        .chars()
        .filter(|c| !c.is_control())
        .take(inlet_web::protocol::MAX_TITLE_CHARS - 8)
        .collect();
    if clean.is_empty() {
        "Inlet".to_string()
    } else {
        format!("{clean} \u{2013} Inlet")
    }
}

/// Starts the helper and opens the window of `flow`.
///
/// Must run in a tokio runtime (the backend's). The session ends the window when it is
/// dropped.
pub async fn open(flow: WebFlow, parent: Option<Parent>) -> Result<WebSession, OpenError> {
    let options = ClientOptions::next_to_current_exe().map_err(OpenError::Spawn)?;
    WebSession::open(&options, flow.into_request(parent)).await
}

/// Where Inlet's main window is, for the helper's window to open over: its rectangle on
/// the screen, and on Windows its handle (the helper's window is tied to it, so it
/// stays above it and goes with it when it is minimised).
pub fn parent_of(window: &gpui::Window) -> Parent {
    let bounds = window.bounds();
    Parent {
        handle: native_handle(window),
        bounds: Some(Bounds {
            x: f32::from(bounds.origin.x) as i32,
            y: f32::from(bounds.origin.y) as i32,
            width: f32::from(bounds.size.width).max(0.0) as u32,
            height: f32::from(bounds.size.height).max(0.0) as u32,
        }),
    }
}

#[cfg(windows)]
fn native_handle(window: &gpui::Window) -> Option<u64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get() as u64),
        _ => None,
    }
}

/// A window handle of this process means nothing in the helper's on macOS, and Wayland
/// has no foreign parents: the window is only placed over the main window and kept on top.
#[cfg(not(windows))]
fn native_handle(_window: &gpui::Window) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use inlet_web::protocol::{encode_request, Request};

    use super::*;

    fn flow() -> WebFlow {
        WebFlow {
            url: "https://id.example/login".into(),
            allow: vec!["id.example".into()],
            intercept: vec!["https://id.example/done".into()],
            frames: vec![],
            title: "Sign in".into(),
            size: (480, 640),
        }
    }

    #[test]
    fn the_title_is_the_flows_own_then_inlets_name() {
        assert_eq!(window_title("Sign in"), "Sign in \u{2013} Inlet");
        assert_eq!(window_title(""), "Inlet");
        assert_eq!(window_title("a\nb\u{7}c"), "abc \u{2013} Inlet");
        // Long titles are cut, so the helper never refuses one.
        let long = window_title(&"x".repeat(1000));
        assert!(long.chars().count() <= inlet_web::protocol::MAX_TITLE_CHARS);
    }

    #[test]
    fn a_flow_becomes_a_request_the_helper_accepts() {
        let request = flow().into_request(None);
        assert_eq!(request.url, "https://id.example/login");
        assert_eq!(request.title, "Sign in \u{2013} Inlet");
        assert_eq!(
            request.size,
            Size {
                width: 480,
                height: 640
            }
        );
        // What the client would check before it starts anything.
        assert!(inlet_web::policy::Policy::for_request(
            &request,
            inlet_web::policy::PolicyOptions::default()
        )
        .is_ok());
    }

    #[test]
    fn the_size_is_held_to_what_the_helper_accepts() {
        let mut small = flow();
        small.size = (10, 0);
        let request = small.into_request(None);
        assert_eq!(
            (request.size.width, request.size.height),
            (MIN_WINDOW, MIN_WINDOW)
        );
        let mut big = flow();
        big.size = (u32::MAX, 100_000);
        let request = big.into_request(None);
        assert_eq!(
            (request.size.width, request.size.height),
            (MAX_WINDOW, MAX_WINDOW)
        );
    }

    #[test]
    fn the_request_carries_no_token_cookie_or_header() {
        // The whole wire message is a URL, the rules, a title, a size and where the
        // window goes. A field for anything else would show up in this list.
        let parent = Parent {
            handle: Some(7),
            bounds: Some(Bounds {
                x: 1,
                y: 2,
                width: 300,
                height: 400,
            }),
        };
        let line = encode_request(&Request::Open(flow().into_request(Some(parent))));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        let keys: BTreeSet<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "type",
                "v",
                "url",
                "allow",
                "intercept",
                "frames",
                "title",
                "size",
                "parent"
            ])
        );
        let parent_keys: BTreeSet<&str> = value["parent"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(parent_keys, BTreeSet::from(["handle", "bounds"]));
    }

    #[tokio::test]
    async fn without_a_helper_the_flow_fails_with_a_clear_error() {
        // The test binary has no `inlet-web` beside it.
        match open(flow(), None).await {
            Err(OpenError::HelperMissing(path)) => {
                assert_eq!(path.file_name().unwrap(), inlet_web::client::HELPER_FILE);
            }
            Err(other) => panic!("another error: {other}"),
            Ok(_) => panic!("a helper was found"),
        }
    }
}
