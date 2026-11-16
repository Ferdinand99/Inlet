// SPDX-License-Identifier: AGPL-3.0-or-later
//! The messages between Inlet and the `inlet-web` helper: one JSON object per line, in
//! both directions. Inlet writes requests to the helper's standard input; the helper
//! writes events to its standard output.
//!
//! Every message carries the protocol version in `v`. A message with another version,
//! or with a field this version does not know, is refused: the two programs ship
//! together, so a mismatch is a broken installation and must not be guessed at.
//!
//! There is deliberately no field for a cookie, a header or a token: the helper gets a
//! URL and the rules for it, nothing else.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of this protocol.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest line either side reads. A longer line is a protocol error.
pub const MAX_LINE_LEN: usize = 64 * 1024;

/// The longest URL the helper opens or reports in full.
pub const MAX_URL_LEN: usize = 8 * 1024;

/// A blocked URL is cut to this many characters in the `blocked` event, so a page cannot
/// push a large value through the helper.
pub const MAX_BLOCKED_URL_LEN: usize = 2048;

/// The most host patterns and intercept prefixes one `open` may carry.
pub const MAX_RULES: usize = 32;

/// The longest window title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;

/// The smallest and the largest window size, in logical pixels.
pub const MIN_WINDOW: u32 = 320;
pub const MAX_WINDOW: u32 = 3840;

/// A request from Inlet to the helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Opens the window. Must be the first message, and the only `open`.
    Open(OpenRequest),
    /// Closes the window. The helper answers with `closed` and exits.
    Close,
}

/// What to open and which navigations to allow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    /// The first page. An `https` URL whose host matches `allow`.
    pub url: String,
    /// Host patterns the window may navigate to: `example.com`, `*.example.com`, with an
    /// optional port (`example.com:8443`, `example.com:*`; no port means 443).
    pub allow: Vec<String>,
    /// URL prefixes that end the flow: when the page navigates to one, the helper stops
    /// the navigation, reports `intercepted` with the URL and closes.
    #[serde(default)]
    pub intercept: Vec<String>,
    /// The window title.
    pub title: String,
    /// The size of the page area, in logical pixels.
    pub size: Size,
    /// Inlet's main window, which the helper window sits over.
    #[serde(default)]
    pub parent: Option<Parent>,
}

/// A size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// Where Inlet's main window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parent {
    /// The native window handle: the `HWND` on Windows (the helper window is tied to
    /// it, so it stays above it and follows its minimising). Not used elsewhere: a
    /// window handle means nothing in another process on macOS, and Wayland has no
    /// foreign parents.
    #[serde(default)]
    pub handle: Option<u64>,
    /// The main window's rectangle in logical screen coordinates. The helper window
    /// opens centred over it.
    #[serde(default)]
    pub bounds: Option<Bounds>,
}

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// An event from the helper to Inlet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The window is open and the first page is loading.
    Ready,
    /// The page tried to navigate to a URL that matches `intercept`. The navigation was
    /// stopped and the helper is closing. The URL comes from a web page: treat it as
    /// untrusted data.
    Intercepted { url: String },
    /// The helper refused a navigation, a new window or a download. A refused
    /// navigation closes the window; a refused new window or download does not. The URL
    /// is cut to [`MAX_BLOCKED_URL_LEN`] characters and comes from a web page.
    Blocked { url: String },
    /// The window is closed (by the user, by `close`, by an end of the flow, or because
    /// Inlet went away). The last event.
    Closed,
    /// The helper could not do what it was asked. The last event.
    Error { kind: ErrorKind },
}

impl Event {
    /// True for the events after which nothing more follows.
    pub fn is_last(&self) -> bool {
        matches!(self, Event::Closed | Event::Error { .. })
    }
}

/// Why the helper, or the client, gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The request had another protocol version.
    UnsupportedVersion,
    /// The request was malformed, or its URL or rules were refused.
    InvalidRequest,
    /// The system's web view is not installed (on Windows, the WebView2 runtime) or
    /// cannot start.
    RuntimeMissing,
    /// The window or the web view could not be created.
    WindowFailed,
    /// The helper broke a rule it keeps itself. Not sent by a healthy helper.
    Internal,
    /// Set by the client, never sent by the helper: the helper ended without a last event.
    HelperExited,
    /// Set by the client, never sent by the helper: the helper wrote something that is
    /// not this protocol.
    Protocol,
}

/// Why a line could not be read as a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("the line is longer than {MAX_LINE_LEN} bytes")]
    TooLong,
    #[error("the line is not a JSON object")]
    NotJson,
    #[error("the message has protocol version {0:?}, this build speaks {PROTOCOL_VERSION}")]
    UnsupportedVersion(Option<u64>),
    #[error("the message is not a message of this protocol version")]
    Invalid,
}

/// The request as one line, with `v` and a trailing newline.
pub fn encode_request(request: &Request) -> String {
    encode(request)
}

/// The event as one line, with `v` and a trailing newline.
pub fn encode_event(event: &Event) -> String {
    encode(event)
}

fn encode<T: Serialize>(message: &T) -> String {
    let mut value = serde_json::to_value(message).expect("a protocol message is JSON");
    if let Value::Object(map) = &mut value {
        map.insert("v".into(), PROTOCOL_VERSION.into());
    }
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Reads one request line.
pub fn decode_request(line: &str) -> Result<Request, ProtocolError> {
    decode(line)
}

/// Reads one event line.
pub fn decode_event(line: &str) -> Result<Event, ProtocolError> {
    decode(line)
}

fn decode<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, ProtocolError> {
    let line = line.trim_end_matches(['\n', '\r']);
    if line.len() > MAX_LINE_LEN {
        return Err(ProtocolError::TooLong);
    }
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(line) else {
        return Err(ProtocolError::NotJson);
    };
    match map.remove("v").and_then(|v| v.as_u64()) {
        Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
        other => return Err(ProtocolError::UnsupportedVersion(other)),
    }
    serde_json::from_value(Value::Object(map)).map_err(|_| ProtocolError::Invalid)
}

/// Cuts `url` to at most `max` characters, on a character boundary.
pub fn truncate_url(url: &str, max: usize) -> String {
    match url.char_indices().nth(max) {
        Some((end, _)) => url[..end].to_string(),
        None => url.to_string(),
    }
}

/// Why an [`OpenRequest`] was refused before anything opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    #[error("the request has no host pattern in `allow`")]
    NoAllow,
    #[error("more than {MAX_RULES} patterns or prefixes")]
    TooManyRules,
    #[error("`allow` pattern {0} is not accepted")]
    BadPattern(usize),
    #[error("`intercept` prefix {0} is not accepted")]
    BadPrefix(usize),
    #[error("the start URL is not accepted")]
    BadUrl,
    #[error("the title or the size is not accepted")]
    BadWindow,
    #[error("the request is longer than {MAX_LINE_LEN} bytes")]
    TooLarge,
}

impl OpenRequest {
    /// Checks what does not need the URL rules: the title and the size. The rules
    /// themselves are checked by [`crate::policy::Policy::for_request`].
    pub fn check_window(&self) -> Result<(), RequestError> {
        let title_ok = !self.title.is_empty()
            && self.title.chars().count() <= MAX_TITLE_CHARS
            && !self.title.chars().any(char::is_control);
        let size_ok = (MIN_WINDOW..=MAX_WINDOW).contains(&self.size.width)
            && (MIN_WINDOW..=MAX_WINDOW).contains(&self.size.height);
        if title_ok && size_ok {
            Ok(())
        } else {
            Err(RequestError::BadWindow)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> OpenRequest {
        OpenRequest {
            url: "https://id.example/login".into(),
            allow: vec!["id.example".into(), "*.id.example".into()],
            intercept: vec!["https://id.example/done".into()],
            title: "Sign in".into(),
            size: Size {
                width: 480,
                height: 640,
            },
            parent: Some(Parent {
                handle: Some(42),
                bounds: Some(Bounds {
                    x: 10,
                    y: 20,
                    width: 1280,
                    height: 800,
                }),
            }),
        }
    }

    #[test]
    fn a_request_round_trips_and_carries_the_version() {
        for request in [Request::Open(open()), Request::Close] {
            let line = encode_request(&request);
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
            let value: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["v"], PROTOCOL_VERSION);
            assert_eq!(decode_request(&line), Ok(request));
        }
    }

    #[test]
    fn every_event_round_trips() {
        let events = [
            Event::Ready,
            Event::Intercepted {
                url: "https://id.example/done?code=1".into(),
            },
            Event::Blocked {
                url: "https://evil.example/".into(),
            },
            Event::Closed,
            Event::Error {
                kind: ErrorKind::RuntimeMissing,
            },
        ];
        for event in events {
            let line = encode_event(&event);
            assert_eq!(decode_event(&line), Ok(event));
        }
    }

    #[test]
    fn the_wire_format_is_the_documented_one() {
        // Compared as values: the key order depends on which serde_json features the
        // workspace turns on.
        let wire = |line: String| serde_json::from_str::<Value>(&line).unwrap();
        assert_eq!(
            wire(encode_event(&Event::Blocked { url: "u".into() })),
            serde_json::json!({"type": "blocked", "url": "u", "v": 1})
        );
        assert_eq!(
            wire(encode_event(&Event::Error {
                kind: ErrorKind::WindowFailed
            })),
            serde_json::json!({"type": "error", "kind": "window_failed", "v": 1})
        );
        assert_eq!(
            wire(encode_request(&Request::Close)),
            serde_json::json!({"type": "close", "v": 1})
        );
    }

    #[test]
    fn another_version_or_none_is_refused() {
        assert_eq!(
            decode_request(r#"{"type":"close","v":2}"#),
            Err(ProtocolError::UnsupportedVersion(Some(2)))
        );
        assert_eq!(
            decode_request(r#"{"type":"close"}"#),
            Err(ProtocolError::UnsupportedVersion(None))
        );
        assert_eq!(
            decode_request(r#"{"type":"close","v":"1"}"#),
            Err(ProtocolError::UnsupportedVersion(None))
        );
    }

    #[test]
    fn unknown_fields_and_types_are_refused() {
        // A cookie or a header has no place in the protocol, so a request that carries
        // one is refused rather than passed on.
        let with_cookies =
            r#"{"type":"open","v":1,"url":"https://a.example/","allow":["a.example"],
            "title":"t","size":{"width":400,"height":400},"cookies":[{"name":"a","value":"b"}]}"#
                .replace('\n', "");
        assert_eq!(decode_request(&with_cookies), Err(ProtocolError::Invalid));
        let with_headers =
            r#"{"type":"open","v":1,"url":"https://a.example/","allow":["a.example"],
            "title":"t","size":{"width":400,"height":400},"headers":{"authorization":"x"}}"#
                .replace('\n', "");
        assert_eq!(decode_request(&with_headers), Err(ProtocolError::Invalid));
        assert_eq!(
            decode_request(r#"{"type":"eval","v":1,"script":"1"}"#),
            Err(ProtocolError::Invalid)
        );
        assert_eq!(
            decode_request(r#"{"type":"open","v":1,"url":"https://a.example/"}"#),
            Err(ProtocolError::Invalid)
        );
    }

    #[test]
    fn garbage_is_not_json() {
        for line in ["", "not json", "[1]", "42", "\"close\"", "{"] {
            assert_eq!(
                decode_request(line),
                Err(ProtocolError::NotJson),
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_long_line_is_refused() {
        let line = format!(
            r#"{{"type":"blocked","v":1,"url":"{}"}}"#,
            "a".repeat(MAX_LINE_LEN)
        );
        assert_eq!(decode_event(&line), Err(ProtocolError::TooLong));
    }

    #[test]
    fn urls_are_cut_on_a_character_boundary() {
        assert_eq!(truncate_url("abcdef", 3), "abc");
        assert_eq!(truncate_url("abc", 3), "abc");
        assert_eq!(truncate_url("æøå€", 2), "æø");
        assert_eq!(truncate_url("", 5), "");
    }

    #[test]
    fn the_window_needs_a_clean_title_and_a_sane_size() {
        assert_eq!(open().check_window(), Ok(()));
        for mutate in [
            (|r: &mut OpenRequest| r.title.clear()) as fn(&mut OpenRequest),
            |r| r.title = "line\nbreak".into(),
            |r| r.title = "x".repeat(MAX_TITLE_CHARS + 1),
            |r| r.size.width = MIN_WINDOW - 1,
            |r| r.size.height = MAX_WINDOW + 1,
        ] {
            let mut request = open();
            mutate(&mut request);
            assert_eq!(request.check_window(), Err(RequestError::BadWindow));
        }
    }

    #[test]
    fn only_closed_and_error_are_last() {
        assert!(Event::Closed.is_last());
        assert!(Event::Error {
            kind: ErrorKind::Internal
        }
        .is_last());
        assert!(!Event::Ready.is_last());
        assert!(!Event::Blocked { url: "u".into() }.is_last());
        assert!(!Event::Intercepted { url: "u".into() }.is_last());
    }
}
