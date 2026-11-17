// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs the real helper with a real window on every platform, against a local server over
//! plain http (the `test-loopback` build accepts that on loopback only).
//!
//! The most important property of the helper is that the URL the identity provider
//! sends the user back to is stopped before a request for it leaves, and the redirect case
//! (`302 Location: <redirect_uri>?code=...`) is the real one. The CI jobs of all three
//! platforms run this file. It needs a display and the system's web engine, so the tests
//! are ignored in a plain `cargo test`:
//!
//! ```text
//! cargo test -p inlet-web --features test-loopback --test loopback -- --include-ignored --test-threads=1
//! ```
//!
//! (Linux: under `xvfb-run`.) The same scenarios over TLS, with the Linux-only extras
//! (downloads, new windows, frames, the environment of the helper), are in `helper.rs`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use inlet_web::client::{ClientOptions, Session};
use inlet_web::protocol::{Event, OpenRequest, Size};
use inlet_web::testkit::PageServer;

const HELPER: &str = env!("CARGO_BIN_EXE_inlet-web");
const WAIT: Duration = Duration::from_secs(60);

/// The settings a headless run in a container needs; nothing elsewhere.
fn extra_env() -> Vec<(OsString, OsString)> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    [
        ("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1"),
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
        ("LIBGL_ALWAYS_SOFTWARE", "1"),
        ("GDK_BACKEND", "x11"),
        ("NO_AT_BRIDGE", "1"),
    ]
    .into_iter()
    .map(|(k, v)| (OsString::from(k), OsString::from(v)))
    .collect()
}

fn options() -> ClientOptions {
    ClientOptions {
        helper: PathBuf::from(HELPER),
        extra_env: extra_env(),
    }
}

fn request(server: &PageServer, path: &str) -> OpenRequest {
    OpenRequest {
        url: format!("http://{}{path}", server.host_ip()),
        allow: vec![server.host_ip()],
        intercept: vec![format!("http://{}/cb", server.host_ip())],
        frames: vec![],
        title: "Inlet test".into(),
        size: Size {
            width: 480,
            height: 360,
        },
        parent: None,
    }
}

async fn open(server: &PageServer, path: &str) -> Session {
    Session::open(&options(), request(server, path))
        .await
        .expect("open the helper")
}

async fn all_events(session: &mut Session) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(WAIT, session.next_event()).await {
            Ok(Some(event)) => events.push(event),
            Ok(None) => return events,
            Err(_) => panic!("no event within the time limit; so far {events:?}"),
        }
    }
}

fn blocked_url(events: &[Event]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        Event::Blocked { url } => Some(url.as_str()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn an_allowed_navigation_loads_and_the_window_closes_on_request() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/auto/next").await;
    let first = tokio::time::timeout(WAIT, session.next_event()).await;
    assert_eq!(first.expect("an event"), Some(Event::Ready));
    let deadline = Instant::now() + WAIT;
    while !server.saw(&server.host_ip(), "/next") {
        assert!(Instant::now() < deadline, "the page never went to /next");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    session.close().await;
    assert_eq!(all_events(&mut session).await, vec![Event::Closed]);
    session.finished().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn a_script_that_goes_to_the_intercept_url_is_stopped_before_the_request() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/auto/intercept").await;
    let events = all_events(&mut session).await;
    assert_eq!(
        events,
        vec![
            Event::Ready,
            Event::Intercepted {
                url: format!("http://{}/cb?code=abc", server.host_ip())
            },
            Event::Closed,
        ]
    );
    session.finished().await;
    assert!(!server.saw(&server.host_ip(), "/cb"), "{:?}", server.hits());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn a_redirect_to_the_intercept_url_is_stopped_before_the_request() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/redirect-cb").await;
    let events = all_events(&mut session).await;
    assert_eq!(
        events,
        vec![
            Event::Ready,
            Event::Intercepted {
                url: format!("http://{}/cb?code=abc", server.host_ip())
            },
            Event::Closed,
        ]
    );
    session.finished().await;
    assert!(server.saw(&server.host_ip(), "/redirect-cb"));
    assert!(!server.saw(&server.host_ip(), "/cb"), "{:?}", server.hits());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn another_host_is_blocked_and_the_window_closes() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/auto/blocked").await;
    let events = all_events(&mut session).await;
    assert_eq!(events.first(), Some(&Event::Ready), "{events:?}");
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    assert_eq!(
        blocked_url(&events),
        Some(format!("http://{}/blocked", server.host_name()).as_str())
    );
    session.finished().await;
    assert!(
        !server.saw(&server.host_name(), "/blocked"),
        "{:?}",
        server.hits()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn a_redirect_to_another_host_is_blocked_before_the_request() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/redirect").await;
    let events = all_events(&mut session).await;
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    assert_eq!(
        blocked_url(&events),
        Some(format!("http://{}/redir-target", server.host_name()).as_str())
    );
    session.finished().await;
    assert!(
        !server.saw(&server.host_name(), "/redir-target"),
        "{:?}",
        server.hits()
    );
}

// WebView2 and WKWebView refuse a script's navigation to a data: URL themselves, without
// asking the helper; WebKitGTK asks, and the helper blocks it.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn a_data_url_is_blocked() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/auto/data").await;
    let events = all_events(&mut session).await;
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    assert!(
        blocked_url(&events).is_some_and(|u| u.starts_with("data:")),
        "{events:?}"
    );
    session.finished().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn a_frame_of_a_listed_host_loads() {
    let server = PageServer::plain().expect("start the server");
    let mut open = request(&server, "/auto/frame");
    open.frames = vec![server.host_name()];
    let mut session = Session::open(&options(), open)
        .await
        .expect("open the helper");
    let first = tokio::time::timeout(WAIT, session.next_event()).await;
    assert_eq!(first.expect("an event"), Some(Event::Ready));
    let deadline = Instant::now() + WAIT;
    while !server.saw(&server.host_name(), "/frame") {
        assert!(
            Instant::now() < deadline,
            "the frame never loaded: {:?}",
            server.hits()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    session.close().await;
    assert_eq!(all_events(&mut session).await, vec![Event::Closed]);
    session.finished().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and the system web engine"]
async fn a_frame_of_another_host_ends_the_flow() {
    let server = PageServer::plain().expect("start the server");
    let mut session = open(&server, "/auto/frame").await;
    let events = all_events(&mut session).await;
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    assert_eq!(
        blocked_url(&events),
        Some(format!("http://{}/frame", server.host_name()).as_str())
    );
    session.finished().await;
    assert!(
        !server.saw(&server.host_name(), "/frame"),
        "{:?}",
        server.hits()
    );
}
