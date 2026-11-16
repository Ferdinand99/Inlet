// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs the real helper, with a real WebKitGTK window, against a local HTTPS server.
//!
//! Needs a display and WebKitGTK, so the tests are ignored in a plain `cargo test`. Run
//! them under Xvfb, one at a time:
//!
//! ```text
//! xvfb-run -a cargo test -p inlet-web --features test-loopback --test helper -- --include-ignored --test-threads=1
//! ```
//!
//! The server's certificate is self-signed; a build with the `test-loopback` feature (a
//! debug build only) trusts any certificate, so the page loads. Nothing here touches the
//! network beyond loopback.
#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inlet_web::client::{ClientOptions, Session};
use inlet_web::protocol::{
    decode_event, encode_request, ErrorKind, Event, OpenRequest, Request, Size,
};
use inlet_web::testkit::{Conn, PageServer};

const HELPER: &str = env!("CARGO_BIN_EXE_inlet-web");
const WAIT: Duration = Duration::from_secs(40);

/// The server over TLS with a fresh self-signed certificate for `localhost` and
/// `127.0.0.1`.
fn https_server() -> PageServer {
    let key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("make a certificate");
    let cert = key.cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()),
    );
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert], key_der)
    .expect("certificate and key");
    let config = Arc::new(config);
    PageServer::start(
        "https",
        Arc::new(move |tcp| {
            let conn = rustls::ServerConnection::new(config.clone()).ok()?;
            Some(Box::new(rustls::StreamOwned::new(conn, tcp)) as Box<dyn Conn>)
        }),
    )
    .expect("start the server")
}

/// A folder of the test's own to use as the helper's temporary folder.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("inlet-web-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }

    /// The folders the helper made in it.
    fn data_dirs(&self) -> Vec<String> {
        std::fs::read_dir(&self.0)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("inlet-web-"))
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The settings a headless run in a container needs. Only the tests pass them.
fn headless_env(tmp: &Path) -> Vec<(OsString, OsString)> {
    [
        ("TMPDIR", tmp.as_os_str()),
        ("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1".as_ref()),
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1".as_ref()),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "1".as_ref()),
        ("LIBGL_ALWAYS_SOFTWARE", "1".as_ref()),
        ("GDK_BACKEND", "x11".as_ref()),
        ("NO_AT_BRIDGE", "1".as_ref()),
    ]
    .into_iter()
    .map(|(k, v)| (OsString::from(k), v.to_os_string()))
    .collect()
}

fn options(tmp: &Path) -> ClientOptions {
    ClientOptions {
        helper: PathBuf::from(HELPER),
        extra_env: headless_env(tmp),
    }
}

fn request(server: &PageServer, path: &str) -> OpenRequest {
    OpenRequest {
        url: format!("https://{}{path}", server.host_ip()),
        allow: vec![server.host_ip()],
        intercept: vec![format!("https://{}/cb", server.host_ip())],
        title: "Inlet test".into(),
        size: Size {
            width: 480,
            height: 360,
        },
        parent: None,
    }
}

async fn next(session: &mut Session) -> Option<Event> {
    tokio::time::timeout(WAIT, session.next_event())
        .await
        .expect("an event within the time limit")
}

/// Everything up to the end of the stream.
async fn all_events(session: &mut Session) -> Vec<Event> {
    let mut events = Vec::new();
    while let Some(event) = next(session).await {
        events.push(event);
    }
    events
}

/// Events up to and including the first one that satisfies `stop`.
async fn until(session: &mut Session, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut events = Vec::new();
    while let Some(event) = next(session).await {
        let done = stop(&event);
        events.push(event);
        if done {
            return events;
        }
    }
    panic!("the stream ended before the awaited event: {events:?}");
}

fn blocked_url(events: &[Event]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        Event::Blocked { url } => Some(url.as_str()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn an_allowed_navigation_loads() {
    let server = https_server();
    let tmp = Scratch::new("allowed");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/auto/next"))
        .await
        .expect("open");
    assert_eq!(next(&mut session).await, Some(Event::Ready));
    // The first page redirects itself to /next, on the same host.
    let deadline = Instant::now() + WAIT;
    while !server.saw(&server.host_ip(), "/next") {
        assert!(Instant::now() < deadline, "the page never went to /next");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The window is still open; Inlet closes it.
    session.close().await;
    assert_eq!(all_events(&mut session).await, vec![Event::Closed]);
    session.finished().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn an_intercept_url_is_reported_and_closes_the_window() {
    let server = https_server();
    let tmp = Scratch::new("intercept");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/auto/intercept"))
        .await
        .expect("open");
    let events = all_events(&mut session).await;
    assert_eq!(
        events,
        vec![
            Event::Ready,
            Event::Intercepted {
                url: format!("https://{}/cb?code=abc", server.host_ip())
            },
            Event::Closed,
        ]
    );
    session.finished().await;
    // The navigation was stopped before a request left the window.
    assert!(!server.saw(&server.host_ip(), "/cb"), "{:?}", server.hits());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn a_host_that_is_not_allowed_is_blocked_and_closes_the_window() {
    let server = https_server();
    let tmp = Scratch::new("blocked");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/auto/blocked"))
        .await
        .expect("open");
    let events = all_events(&mut session).await;
    assert_eq!(events.first(), Some(&Event::Ready), "{events:?}");
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    let url = blocked_url(&events).expect("a blocked event");
    assert_eq!(url, format!("https://{}/blocked", server.host_name()));
    session.finished().await;
    // Nothing was fetched from the other host.
    assert!(
        !server.saw(&server.host_name(), "/blocked"),
        "{:?}",
        server.hits()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn a_redirect_to_another_host_is_blocked() {
    let server = https_server();
    let tmp = Scratch::new("redirect");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/redirect"))
        .await
        .expect("open");
    let events = all_events(&mut session).await;
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    let url = blocked_url(&events).expect("a blocked event");
    assert_eq!(url, format!("https://{}/redir-target", server.host_name()));
    session.finished().await;
    assert!(
        !server.saw(&server.host_name(), "/redir-target"),
        "{:?}",
        server.hits()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn a_data_url_is_blocked() {
    let server = https_server();
    let tmp = Scratch::new("data");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/auto/data"))
        .await
        .expect("open");
    let events = all_events(&mut session).await;
    assert_eq!(events.last(), Some(&Event::Closed), "{events:?}");
    let url = blocked_url(&events).expect("a blocked event");
    assert!(url.starts_with("data:"), "{url}");
    session.finished().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn a_download_is_blocked_and_the_window_stays() {
    let server = https_server();
    let tmp = Scratch::new("download");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/auto/download"))
        .await
        .expect("open");
    let events = until(&mut session, |e| matches!(e, Event::Blocked { .. })).await;
    assert_eq!(
        blocked_url(&events),
        Some(format!("https://{}/file", server.host_ip()).as_str())
    );
    // The window is open still: Inlet's close is what ends it.
    session.close().await;
    assert_eq!(all_events(&mut session).await, vec![Event::Closed]);
    session.finished().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn a_new_window_is_blocked_and_the_window_stays() {
    let server = https_server();
    let tmp = Scratch::new("window");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/auto/window"))
        .await
        .expect("open");
    let events = until(&mut session, |e| matches!(e, Event::Blocked { .. })).await;
    assert_eq!(
        blocked_url(&events),
        Some(format!("https://{}/popup", server.host_ip()).as_str())
    );
    session.close().await;
    assert_eq!(all_events(&mut session).await, vec![Event::Closed]);
    session.finished().await;
    assert!(
        !server.saw(&server.host_ip(), "/popup"),
        "{:?}",
        server.hits()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn the_data_folder_exists_while_the_window_is_open_and_is_removed_after() {
    let server = https_server();
    let tmp = Scratch::new("datadir");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/next"))
        .await
        .expect("open");
    assert_eq!(next(&mut session).await, Some(Event::Ready));
    assert_eq!(tmp.data_dirs().len(), 1, "{:?}", tmp.data_dirs());
    session.close().await;
    assert_eq!(all_events(&mut session).await, vec![Event::Closed]);
    session.finished().await;
    assert_eq!(tmp.data_dirs(), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
async fn the_helper_has_none_of_inlets_environment_and_no_arguments() {
    let server = https_server();
    let tmp = Scratch::new("env");
    // Variables that stand for what Inlet must not hand over.
    std::env::set_var("INLET_TEST_TOKEN", "super-secret-token-value");
    std::env::set_var("FLUXER_TEST_COOKIE", "cookie-secret-value");
    let mut session = Session::open(&options(&tmp.0), request(&server, "/next"))
        .await
        .expect("open");
    assert_eq!(next(&mut session).await, Some(Event::Ready));
    let pid = session.pid().expect("the helper's process id");
    let environ = std::fs::read(format!("/proc/{pid}/environ")).expect("read the environment");
    let environ = String::from_utf8_lossy(&environ);
    assert!(!environ.contains("INLET_TEST_TOKEN"), "{environ}");
    assert!(!environ.contains("super-secret-token-value"));
    assert!(!environ.contains("FLUXER_TEST_COOKIE"));
    assert!(!environ.contains("cookie-secret-value"));
    // What the window needs is there.
    assert!(environ.contains("DISPLAY="), "{environ}");
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).expect("read the command line");
    let args: Vec<&[u8]> = cmdline
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .collect();
    assert_eq!(args.len(), 1, "the helper has arguments: {args:?}");
    session.close().await;
    all_events(&mut session).await;
    session.finished().await;
    std::env::remove_var("INLET_TEST_TOKEN");
    std::env::remove_var("FLUXER_TEST_COOKIE");
}

/// Starts the helper by hand, as a program that is not the client would.
fn spawn_raw(tmp: &Path) -> std::process::Child {
    let mut command = Command::new(HELPER);
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if matches!(&*name, "DISPLAY" | "XAUTHORITY" | "PATH" | "HOME") || name.starts_with("XDG_")
        {
            command.env(key, value);
        }
    }
    command.envs(headless_env(tmp));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the helper")
}

fn wait_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = child.try_wait().expect("wait for the helper") {
            return status;
        }
        assert!(Instant::now() < deadline, "the helper did not exit");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
fn the_helper_exits_when_its_input_closes() {
    let server = https_server();
    let tmp = Scratch::new("stdin");
    let mut child = spawn_raw(&tmp.0);
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    stdin
        .write_all(encode_request(&Request::Open(request(&server, "/next"))).as_bytes())
        .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(decode_event(&line), Ok(Event::Ready));
    // Inlet goes away.
    drop(stdin);
    line.clear();
    out.read_line(&mut line).unwrap();
    assert_eq!(decode_event(&line), Ok(Event::Closed));
    assert_eq!(wait_exit(&mut child).code(), Some(0));
    assert_eq!(tmp.data_dirs(), Vec::<String>::new());
}

#[test]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
fn a_request_of_another_version_is_refused() {
    let tmp = Scratch::new("version");
    let mut child = spawn_raw(&tmp.0);
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    stdin.write_all(b"{\"type\":\"close\",\"v\":99}\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(
        decode_event(&line),
        Ok(Event::Error {
            kind: ErrorKind::UnsupportedVersion
        })
    );
    assert_eq!(wait_exit(&mut child).code(), Some(1));
}

#[test]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
fn the_helper_refuses_a_url_the_client_would_have_refused() {
    // A program that skips the client's checks still meets the helper's own.
    let tmp = Scratch::new("badurl");
    let server = https_server();
    for url in [
        "javascript:alert(1)".to_string(),
        "file:///etc/passwd".to_string(),
        "data:text/html,hi".to_string(),
        format!("https://localhost:{}/", server.port()),
    ] {
        let mut child = spawn_raw(&tmp.0);
        let mut stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut open = request(&server, "/");
        open.url = url.clone();
        stdin
            .write_all(encode_request(&Request::Open(open)).as_bytes())
            .unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(
            decode_event(&line),
            Ok(Event::Error {
                kind: ErrorKind::InvalidRequest
            }),
            "{url}"
        );
        assert_eq!(wait_exit(&mut child).code(), Some(1));
    }
}

#[test]
#[ignore = "needs a display and WebKitGTK; run under xvfb-run"]
fn a_request_with_a_cookie_field_is_refused() {
    let tmp = Scratch::new("cookie");
    let mut child = spawn_raw(&tmp.0);
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let line = r#"{"type":"open","v":1,"url":"https://127.0.0.1:1/","allow":["127.0.0.1:1"],"title":"t","size":{"width":400,"height":400},"cookies":["a=b"]}"#;
    writeln!(stdin, "{line}").unwrap();
    stdin.flush().unwrap();
    let mut answer = String::new();
    out.read_line(&mut answer).unwrap();
    assert_eq!(
        decode_event(&answer),
        Ok(Event::Error {
            kind: ErrorKind::InvalidRequest
        })
    );
    assert_eq!(wait_exit(&mut child).code(), Some(1));
}
