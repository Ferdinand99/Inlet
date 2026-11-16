// SPDX-License-Identifier: AGPL-3.0-or-later
//! The poller's request budget against a mock instance (a burst of reconnects makes one
//! round, three answers of 404 end the polling, the discovery document is read again
//! with its ETag), the clocks' spacing, the status page, and the commands without a
//! session.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedReceiver;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Notify;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::poller::{Clocks, Poller};
use super::*;
use crate::backend::accounts::Accounts;
use crate::backend::Event;
use crate::token_store::MemoryStore;

fn discovery(base: &str) -> Value {
    json!({
        "api_code_version": 1,
        "endpoints": {
            "api": format!("{base}/api"), "api_client": format!("{base}/api"),
            "api_public": format!("{base}/api"), "gateway": "ws://127.0.0.1:1/gateway",
            "media": format!("{base}/media"), "webapp": base, "invite": format!("{base}/invite")
        },
        "captcha": {"provider": "altcha"},
        "features": {"voice_enabled": true},
        "limits": {"version": 2, "rules": [{"id": "default", "overrides": {"max_message_length": 4000}}]},
        "app_public": {"branding": {"product_name": "Mock Social"}},
        "sso": {"enabled": false},
        "domain_migration": {"enabled": false}
    })
}

/// A client for a mock instance whose discovery has `ETag` "v1". Each fixture has its own
/// token, because the protocol shares the limiter between clients of one token.
async fn fixture() -> (MockServer, p::Client) {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"v1\"")
                .set_body_json(discovery(&server.uri())),
        )
        .mount(&server)
        .await;
    let info = p::ClientInfo {
        installation_id: "00000000-0000-4000-a000-000000000000".into(),
        ..p::ClientInfo::default()
    };
    let instance = p::Instance::discover_with(&server.uri(), info)
        .await
        .expect("discovery");
    let token = format!(
        "tok-instance-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    (server, p::Client::new(instance, p::Token::new(token)))
}

fn experiments_body(interval: u64) -> Value {
    json!({
        "poll_interval_seconds": interval, "poll_jitter_percent": 0,
        "assignments": {"channel_threads": {"active": true, "config_version": 1},
                        "guild_visits": {"enabled": true}}
    })
}

async fn count(server: &MockServer, route: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == route)
        .count()
}

struct Running {
    events: UnboundedReceiver<Event>,
    wake: Arc<Notify>,
    window: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn start(client: p::Client, scale: u32) -> Running {
    let (tx, events) = futures::channel::mpsc::unbounded();
    let (internal, _) = tokio::sync::mpsc::unbounded_channel();
    let wake = Arc::new(Notify::new());
    let window = Arc::new(AtomicBool::new(true));
    let poller = Poller::new(
        client,
        Ui::new(tx),
        internal,
        1,
        wake.clone(),
        window.clone(),
    )
    .for_test(scale);
    Running {
        events,
        wake,
        window,
        task: tokio::spawn(poller.run()),
    }
}

async fn next_event(r: &mut Running) -> Ev {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), r.events.next())
            .await
            .expect("an event within ten seconds")
            .expect("the poller is running");
        if let Event::Instance(ev) = event {
            return ev;
        }
    }
}

#[tokio::test]
async fn a_new_active_account_asks_its_experiments_once_and_reports_them() {
    let (server, client) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"e1\"")
                .set_body_json(experiments_body(300)),
        )
        .mount(&server)
        .await;
    let mut run = start(client, 1);
    match next_event(&mut run).await {
        Ev::Experiments(ExperimentState::Loaded(e)) => {
            assert_eq!(e.assignments.guild_visits, Some(true));
            assert_eq!(e.poll_interval_secs, 300);
        }
        _ => panic!("expected the experiments"),
    }
    // Nothing else is due for minutes: one request in all.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(&server, "/api/v1/experiments").await, 1);
}

/// The worst case for the budget: a reconnect storm, and the window gaining and losing
/// the focus at once, makes no second round.
#[tokio::test]
async fn a_burst_of_reconnects_and_focus_changes_makes_no_second_round() {
    let (server, client) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"e1\"")
                .set_body_json(experiments_body(300)),
        )
        .mount(&server)
        .await;
    let discovery_before = count(&server, "/.well-known/fluxer").await;
    let mut run = start(client, 1);
    assert!(matches!(next_event(&mut run).await, Ev::Experiments(_)));
    for i in 0..200 {
        run.wake.notify_one();
        if i % 2 == 0 {
            run.window
                .store(i % 4 == 0, std::sync::atomic::Ordering::Relaxed);
        }
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(count(&server, "/api/v1/experiments").await, 1);
    assert_eq!(
        count(&server, "/.well-known/fluxer").await,
        discovery_before,
        "the discovery document is not read again within five minutes"
    );
}

#[tokio::test]
async fn three_answers_of_404_end_the_polling_and_say_so_once() {
    let (server, client) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": "UNKNOWN_ROUTE"})))
        .mount(&server)
        .await;
    // Waits are 100000 times shorter than real ones, so the three rounds take a moment.
    let mut run = start(client, 100_000);
    assert!(matches!(
        next_event(&mut run).await,
        Ev::Experiments(ExperimentState::Unavailable)
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(&server, "/api/v1/experiments").await, 3);
    // And no more events.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), run.events.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_discovery_document_is_read_again_with_its_etag_and_only_a_change_is_reported() {
    let (server, client) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/experiments"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    // The first read again gets 304; the second a changed document (with a notice); the
    // third, which carries the new ETag, 304 again.
    server_discovery_script(&server).await;
    // 24 hours divided by a million: about 86 ms.
    let mut run = start(client, 1_000_000);
    // The experiments route is gone with the reset, so an `Unavailable` may come first.
    let doc = loop {
        if let Ev::Discovery(doc) = next_event(&mut run).await {
            break doc;
        }
    };
    assert_eq!(doc.notice().unwrap().message, "Planned maintenance");
    tokio::time::sleep(Duration::from_millis(400)).await;
    // No event for a 304, and each read carried the ETag of the document held then.
    let tags: Vec<String> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/.well-known/fluxer")
        .map(|r| {
            r.headers
                .get("if-none-match")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(tags.len() >= 3, "{tags:?}");
    assert_eq!(tags[0], "\"v1\"");
    assert_eq!(tags[1], "\"v1\"");
    assert!(tags[2..].iter().all(|t| t == "\"v2\""), "{tags:?}");
    // Only the one change was reported.
    assert!(tokio::time::timeout(Duration::from_millis(50), async {
        loop {
            if let Some(Event::Instance(Ev::Discovery(_))) = run.events.next().await {
                break;
            }
        }
    })
    .await
    .is_err());
}

async fn server_discovery_script(server: &MockServer) {
    server.reset().await;
    let mut changed = discovery(&server.uri());
    changed["notice"] = json!({"id": "n1", "message": "Planned maintenance", "url": null,
                                "starts_at": null, "ends_at": null});
    // Matched in order of mounting: the first `v1` answer is 304, later ones the change.
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .and(header("If-None-Match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .and(header("If-None-Match", "\"v1\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"v2\"")
                .set_body_json(changed),
        )
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/fluxer"))
        .and(header("If-None-Match", "\"v2\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(server)
        .await;
}

#[test]
fn the_clocks_bring_forward_only_what_the_gaps_allow() {
    let t0 = Instant::now();
    let mut c = Clocks::new(t0, Duration::from_secs(24 * 3600));
    // Start-up: the experiments are due at once, the discovery in 24 hours.
    assert_eq!(c.exp_due, t0);
    assert_eq!(c.disc_due, t0 + Duration::from_secs(24 * 3600));
    c.last_exp = Some(t0);
    c.exp_due = t0 + Duration::from_secs(300);
    // 50 wakes in the first 20 seconds change nothing.
    for s in 1..=20u64 {
        c.wake(t0 + Duration::from_secs(s), true, false);
        c.wake(t0 + Duration::from_secs(s), true, false);
    }
    assert_eq!(c.exp_due, t0 + Duration::from_secs(300));
    assert_eq!(c.disc_due, t0 + Duration::from_secs(24 * 3600));
    // After 30 seconds the experiments may be brought forward, the discovery not yet.
    c.wake(t0 + Duration::from_secs(31), true, false);
    assert_eq!(c.exp_due, t0 + Duration::from_secs(31));
    assert_eq!(c.disc_due, t0 + Duration::from_secs(24 * 3600));
    // After five minutes the discovery too.
    c.wake(t0 + Duration::from_secs(301), true, false);
    assert_eq!(c.disc_due, t0 + Duration::from_secs(301));
    // Polling that was given up is not woken, and neither is a status page there is none of.
    c.exp_due = t0 + Duration::from_secs(1000);
    c.status_due = t0 + Duration::from_secs(1000);
    c.wake(t0 + Duration::from_secs(400), false, false);
    assert_eq!(c.exp_due, t0 + Duration::from_secs(1000));
    assert_eq!(c.status_due, t0 + Duration::from_secs(1000));
    c.wake(t0 + Duration::from_secs(400), true, true);
    assert_eq!(c.status_due, t0 + Duration::from_secs(400));
}

#[test]
fn the_verification_email_is_sent_at_most_once_a_minute() {
    let state = State::default();
    let t0 = Instant::now();
    let mut asks = 0;
    for s in 0..180u64 {
        if state.resend_allowed(t0 + Duration::from_secs(s)) {
            asks += 1;
        }
    }
    // One at 0 s, one at 60 s, one at 120 s, however often the button is pressed.
    assert_eq!(asks, 3);
}

async fn backend_without_account() -> (Backend, UnboundedReceiver<Event>) {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let store = Accounts::new(Arc::new(MemoryStore::default()), None, None);
    (Backend::with_store(Ui::new(tx), store), rx)
}

/// Commands with no account signed in are answered at once, so a button never waits.
#[tokio::test]
async fn the_commands_without_an_account_are_answered_as_failed() {
    let (mut b, mut rx) = backend_without_account().await;
    handle(&mut b, Cmd::ResendVerification);
    assert!(matches!(
        rx.next().await,
        Some(Event::Instance(Ev::VerificationSent(Err(_))))
    ));
    handle(&mut b, Cmd::AcceptTerms);
    assert!(matches!(
        rx.next().await,
        Some(Event::Instance(Ev::TermsAccepted(Err(_))))
    ));
    // The window in use or not changes nothing and makes no request.
    handle(&mut b, Cmd::WindowActive(false));
    handle(&mut b, Cmd::WindowActive(true));
    assert!(rx.try_recv().is_err(), "no answer, no event");
}

#[test]
fn a_background_account_is_announced_from_its_own_document() {
    let doc: p::Discovery = serde_json::from_value(discovery("https://x.example")).unwrap();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    announce_document(&Ui::new(tx), &doc);
    match rx.try_recv() {
        Ok(Event::Instance(Ev::Discovery(sent))) => assert_eq!(*sent, doc),
        _ => panic!("expected the document"),
    }
}

mod status {
    use super::*;
    use crate::backend::instance::status_page::{fetch, Fetched, MaintenanceState};

    fn base(server: &MockServer) -> url::Url {
        url::Url::parse(&server.uri()).unwrap()
    }

    fn http() -> reqwest::Client {
        crate::backend::instance::status_page::client().unwrap()
    }

    async fn serve(server: &MockServer, route: &str, template: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(template)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_summary_with_maintenance_is_read_and_a_page_under_maintenance_asks_its_components() {
        let server = MockServer::start().await;
        serve(
            &server,
            "/summary.json",
            ResponseTemplate::new(200).set_body_json(json!({
                "page": {"status": "UNDERMAINTENANCE"}, "activeIncidents": [], "activeMaintenances": []
            })),
        )
        .await;
        serve(
            &server,
            "/components.json",
            ResponseTemplate::new(200).set_body_json(json!([
                {"activeMaintenances": [{"id": "c1", "name": "API", "status": "INPROGRESS",
                  "start": "2026-11-20T09:00:00.000Z", "duration": "45", "url": "https://s.example/c1"}]}
            ])),
        )
        .await;
        match fetch(&http(), &base(&server)).await {
            Fetched::Status(s) => {
                let m = s.maintenance.unwrap();
                assert_eq!(
                    (m.id.as_str(), m.state),
                    ("c1", MaintenanceState::InProgress)
                );
            }
            other => panic!("expected a status, got {other:?}"),
        }
        assert_eq!(count(&server, "/components.json").await, 1);
    }

    #[tokio::test]
    async fn failures_are_told_apart_passing_ones_from_a_page_that_is_not_instatus() {
        let cases: [(ResponseTemplate, bool); 6] = [
            (ResponseTemplate::new(500), true),
            (ResponseTemplate::new(429), true),
            (ResponseTemplate::new(408), true),
            (ResponseTemplate::new(404), false),
            (
                ResponseTemplate::new(200).set_body_string("<html>status</html>"),
                false,
            ),
            (
                ResponseTemplate::new(200).set_body_json(json!({"hello": "world"})),
                false,
            ),
        ];
        for (template, passing) in cases {
            let server = MockServer::start().await;
            serve(&server, "/summary.json", template).await;
            let got = fetch(&http(), &base(&server)).await;
            assert_eq!(matches!(got, Fetched::Failed), passing, "{got:?}");
            assert!(passing || matches!(got, Fetched::Unsupported), "{got:?}");
        }
    }

    #[tokio::test]
    async fn an_answer_that_is_too_large_is_dropped() {
        let server = MockServer::start().await;
        let huge = format!(
            "{{\"page\": {{}}, \"pad\": \"{}\"}}",
            "x".repeat(super::super::status_page::MAX_BYTES + 10)
        );
        serve(
            &server,
            "/summary.json",
            ResponseTemplate::new(200).set_body_string(huge),
        )
        .await;
        assert!(matches!(
            fetch(&http(), &base(&server)).await,
            Fetched::Unsupported
        ));
    }

    #[tokio::test]
    async fn no_credential_and_no_cookie_goes_to_a_status_page() {
        let server = MockServer::start().await;
        serve(
            &server,
            "/summary.json",
            ResponseTemplate::new(200)
                .insert_header("Set-Cookie", "a=b")
                .set_body_json(json!({"page": {"status": "UP"}, "activeIncidents": [], "activeMaintenances": []})),
        )
        .await;
        let http = http();
        for _ in 0..2 {
            assert!(matches!(
                fetch(&http, &base(&server)).await,
                Fetched::Status(_)
            ));
        }
        let seen: Vec<Request> = server.received_requests().await.unwrap_or_default();
        assert_eq!(seen.len(), 2);
        for r in seen {
            assert!(r.headers.get("authorization").is_none());
            assert!(r.headers.get("cookie").is_none(), "no cookie jar");
            assert!(r.headers.get("x-fluxer-features").is_none());
        }
    }
}
