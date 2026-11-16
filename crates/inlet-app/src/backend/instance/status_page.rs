// SPDX-License-Identifier: AGPL-3.0-or-later
//! The instance's status page: current incidents and planned maintenance, for the
//! banners. Fluxer's own client reads an Instatus page (`summary.json`, and
//! `components.json` when the page says it is under maintenance) when the instance's
//! discovery document says the page is one (`status_page_instatus`).
//!
//! The address comes from the instance, so it is treated as untrusted: only `https`
//! to a host name (no IP address, no `localhost`, no user name, only the default
//! port), no redirect to anything else, no cookie and no credential, a short timeout,
//! and an answer larger than [`MAX_BYTES`] is dropped. The text of the answer is cut
//! before it is shown. A page that is not an Instatus page (an error that is not a
//! passing one, or a document of another shape) is given up for good.
//!
//! This is a request to a third party: while the window has the focus, the person's
//! address goes to a host the instance names every minute or so. Fluxer's own client
//! does the same from the browser, and it is not the case of a link posted in a chat
//! (the media proxy rule), but it is worth knowing.
//!
//! Known limit: the host is a name, and the name may resolve to an address on the
//! person's network. Only a fixed path (`summary.json`, `components.json`) is asked, no
//! credential is sent and the answer is dropped unless it is an Instatus document, but a
//! resolver that refuses loopback, private, link-local and CGNAT addresses would close it
//! (about 30 lines with `reqwest::dns::Resolve`); not done yet.

use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::model::clean_name;

/// Largest answer read, in bytes.
pub const MAX_BYTES: usize = 512 * 1024;
/// Time allowed for one request.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest name kept, in characters.
const MAX_NAME_CHARS: usize = 200;
/// Longest address kept, in characters.
const MAX_URL_CHARS: usize = 2_048;
/// Longest maintenance accepted, in minutes (a year).
const MAX_MINUTES: u64 = 525_600;

/// How far planned maintenance is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceState {
    /// Planned, not started.
    Scheduled,
    /// Going on now.
    InProgress,
}

/// Planned or running maintenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maintenance {
    /// The status page's ID of it.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Where it stands.
    pub state: MaintenanceState,
    /// When it starts (Unix milliseconds), if the page says.
    pub start_ms: Option<i64>,
    /// How long it is expected to last.
    pub duration_minutes: u64,
    /// Its page, to open on request.
    pub url: Option<String>,
}

/// An ongoing incident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    /// The status page's ID of it.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its page, to open on request.
    pub url: Option<String>,
}

/// What the status page says now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// The maintenance to show: a running one before a planned one.
    pub maintenance: Option<Maintenance>,
    /// The incident to show.
    pub incident: Option<Incident>,
}

/// The status page's base address, checked: `https` to a host name on the default port,
/// without a user name, a query or a fragment, and without a trailing slash.
pub fn base_url(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    match url.host()? {
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            if d == "localhost" || d.ends_with(".localhost") || !d.contains('.') {
                return None;
            }
            // `.local`, `.internal` and the like are not the public internet.
            if ["local", "internal", "lan", "home", "corp", "intranet"]
                .iter()
                .any(|s| d.ends_with(&format!(".{s}")))
            {
                return None;
            }
        }
        // An address literal could point anywhere on the person's network.
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => return None,
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Some(url)
}

fn text(v: Option<&Value>, max: usize) -> Option<String> {
    let s = v?.as_str()?;
    (!s.trim().is_empty()).then(|| s.chars().take(max).collect())
}

/// An address from the page, kept only when it is an `https` (or `http`) one; the app
/// asks before opening it.
fn link(v: Option<&Value>) -> Option<String> {
    let s = text(v, MAX_URL_CHARS)?;
    let url = Url::parse(&s).ok()?;
    matches!(url.scheme(), "https" | "http").then_some(s)
}

fn maintenance_of(m: &Value) -> Option<Maintenance> {
    let state = match m.get("status")?.as_str()? {
        "NOTSTARTEDYET" => MaintenanceState::Scheduled,
        "INPROGRESS" => MaintenanceState::InProgress,
        // Finished maintenance is not a banner.
        _ => return None,
    };
    let duration = match m.get("duration")? {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => s.trim().parse::<u64>().ok()?,
        _ => return None,
    };
    if duration == 0 {
        return None;
    }
    let start_ms = m
        .get("start")
        .and_then(Value::as_str)
        .and_then(inlet_protocol::models::parse_timestamp_ms);
    // A page that gives no usable start has no maintenance to announce.
    start_ms?;
    Some(Maintenance {
        id: text(m.get("id"), MAX_NAME_CHARS)?,
        name: clean_name(&text(m.get("name"), MAX_NAME_CHARS)?),
        state,
        start_ms,
        duration_minutes: duration.min(MAX_MINUTES),
        url: link(m.get("url")),
    })
}

/// Fluxer's choice: a running maintenance before a planned one.
fn pick_maintenance(list: &Value) -> Option<Maintenance> {
    let mut chosen: Option<Maintenance> = None;
    for m in list.as_array()?.iter().take(50).filter_map(maintenance_of) {
        let better = match &chosen {
            None => true,
            Some(c) => {
                c.state == MaintenanceState::Scheduled && m.state == MaintenanceState::InProgress
            }
        };
        if better {
            chosen = Some(m);
        }
    }
    chosen
}

/// Whether `doc` has the shape of an Instatus summary.
pub fn is_summary(doc: &Value) -> bool {
    doc.is_object()
        && (doc.get("page").is_some_and(Value::is_object)
            || doc.get("activeIncidents").is_some_and(Value::is_array)
            || doc.get("activeMaintenances").is_some_and(Value::is_array))
}

/// Reads `summary.json`. `components` is the `components.json` fetched when the page
/// is under maintenance without naming one (`None` when not fetched). Returns `None`
/// for a document that is not an Instatus summary.
pub fn parse_summary(summary: &Value, components: Option<&Value>) -> Option<Status> {
    if !is_summary(summary) {
        return None;
    }
    let incident = summary
        .get("activeIncidents")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter().take(50).find(|i| {
                i.get("status").and_then(Value::as_str) != Some("RESOLVED")
                    && i.get("resolved").is_none_or(Value::is_null)
            })
        })
        .and_then(|i| {
            Some(Incident {
                id: text(i.get("id"), MAX_NAME_CHARS)?,
                name: clean_name(&text(i.get("name"), MAX_NAME_CHARS)?),
                url: link(i.get("url")),
            })
        });
    let mut maintenance = summary.get("activeMaintenances").and_then(pick_maintenance);
    if maintenance.is_none() && needs_components(summary) {
        maintenance = components.and_then(component_maintenance);
    }
    Some(Status {
        maintenance,
        incident,
    })
}

/// The page says it is under maintenance but names none: Fluxer asks `components.json`.
pub fn needs_components(summary: &Value) -> bool {
    summary
        .get("page")
        .and_then(|p| p.get("status"))
        .and_then(Value::as_str)
        == Some("UNDERMAINTENANCE")
}

fn component_maintenance(doc: &Value) -> Option<Maintenance> {
    let list = doc
        .as_array()
        .or_else(|| doc.get("components").and_then(Value::as_array))?;
    let all: Vec<Value> = list
        .iter()
        .take(200)
        .filter_map(|c| c.get("activeMaintenances").and_then(Value::as_array))
        .flatten()
        .take(50)
        .cloned()
        .collect();
    pick_maintenance(&Value::Array(all))
}

/// What one round of asking gave.
#[derive(Debug)]
pub enum Fetched {
    /// The page's state now.
    Status(Status),
    /// A passing failure (network, 5xx, 408, 429): ask again later, with a longer wait.
    Failed,
    /// Not an Instatus page: do not ask this address again.
    Unsupported,
}

/// Whether a redirect is followed: at most three hops, and every hop passes the checks
/// of the first address ([`base_url`]), so a status page cannot send the client on to an
/// address literal, `localhost` or a name of the local network.
pub fn redirect_allowed(previous_hops: usize, to: &Url) -> bool {
    previous_hops < 3 && base_url(to.as_str()).is_some()
}

/// The HTTP client for status pages: no cookies, https only through redirects, a short
/// timeout.
pub fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!(
            "Inlet/{} (+https://github.com/Ferdinand99/Inlet)",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if redirect_allowed(attempt.previous().len(), attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .ok()
}

/// Why a document was not read.
enum Why {
    /// A passing failure: ask again later.
    Failed,
    /// Not an Instatus page.
    Unsupported,
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value, Why> {
    let resp = http
        .get(url)
        .header("Accept", "application/json")
        .header("Cache-Control", "no-store")
        .send()
        .await
        .map_err(|_| Why::Failed)?;
    let status = resp.status();
    if !status.is_success() {
        let passing = !status.is_client_error() || matches!(status.as_u16(), 408 | 429);
        return Err(if passing {
            Why::Failed
        } else {
            Why::Unsupported
        });
    }
    if resp.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(Why::Unsupported);
    }
    let mut resp = resp;
    let mut body = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_BYTES {
                    return Err(Why::Unsupported);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Err(Why::Failed),
        }
    }
    serde_json::from_slice(&body).map_err(|_| Why::Unsupported)
}

/// Asks the status page at `base` (from [`base_url`]) for its state.
pub async fn fetch(http: &reqwest::Client, base: &Url) -> Fetched {
    let base = base.as_str().trim_end_matches('/');
    let summary = match get_json(http, &format!("{base}/summary.json")).await {
        Ok(v) => v,
        Err(Why::Failed) => return Fetched::Failed,
        Err(Why::Unsupported) => return Fetched::Unsupported,
    };
    if !is_summary(&summary) {
        return Fetched::Unsupported;
    }
    let components = if needs_components(&summary)
        && summary
            .get("activeMaintenances")
            .and_then(pick_maintenance)
            .is_none()
    {
        get_json(http, &format!("{base}/components.json"))
            .await
            .ok()
    } else {
        None
    };
    match parse_summary(&summary, components.as_ref()) {
        Some(status) => Fetched::Status(status),
        None => Fetched::Unsupported,
    }
}

/// Seconds to wait before the next round, after Fluxer's client: a minute and a quarter
/// at most while all is well, sooner when a planned maintenance is about to start, and
/// 1, 5, 15 then 30 minutes after failures.
pub fn next_delay(status: &Status, failures: u32, now_ms: i64, unit: f64) -> Duration {
    const RETRY: [u64; 4] = [60, 300, 900, 1800];
    if failures > 0 {
        let i = (failures as usize).min(RETRY.len()) - 1;
        return Duration::from_secs(RETRY[i]);
    }
    let unit = if unit.is_finite() {
        unit.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let normal = Duration::from_secs_f64(60.0 + 15.0 * unit);
    match &status.maintenance {
        Some(m) if m.state == MaintenanceState::InProgress => normal,
        Some(m) if m.state == MaintenanceState::Scheduled => match m.start_ms {
            Some(start) => {
                let until = start - now_ms;
                if until <= 0 {
                    Duration::from_secs(10)
                } else if until < 60_000 {
                    Duration::from_millis(until as u64 + 5_000).max(Duration::from_secs(10))
                } else {
                    normal
                }
            }
            None => normal,
        },
        _ => normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(extra: Value) -> Value {
        let mut doc =
            json!({"page": {"status": "UP"}, "activeIncidents": [], "activeMaintenances": []});
        for (k, v) in extra.as_object().unwrap() {
            doc[k] = v.clone();
        }
        doc
    }

    #[test]
    fn only_a_public_https_host_name_is_asked() {
        for ok in [
            "https://status.fluxer.app",
            "https://status.example.org/",
            "https://status.example.org/page/",
        ] {
            assert!(base_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "http://status.example.org",
            "https://127.0.0.1",
            "https://[::1]/",
            "https://192.168.1.10/status",
            "https://localhost",
            "https://printer",
            "https://nas.local",
            "https://wiki.internal",
            "https://user:pw@status.example.org",
            "https://status.example.org:8443",
            "ftp://status.example.org",
            "javascript:alert(1)",
            "not a url",
            "",
        ] {
            assert!(base_url(bad).is_none(), "{bad}");
        }
        let u = base_url("https://status.example.org/p/?q=1#f").unwrap();
        assert_eq!(u.as_str(), "https://status.example.org/p");
    }

    #[test]
    fn a_redirect_is_followed_only_to_an_address_the_first_hop_could_have_been() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert!(redirect_allowed(
            0,
            &url("https://status.example.org/other/path?x=1")
        ));
        assert!(redirect_allowed(2, &url("https://eu.status.example.org/")));
        assert!(!redirect_allowed(3, &url("https://status.example.org/")));
        for to in [
            "http://status.example.org/",
            "https://127.0.0.1/summary.json",
            "https://192.168.1.1/any?x",
            "https://[::1]/",
            "https://localhost/",
            "https://nas.local/",
            "https://printer/",
            "https://status.example.org:8443/",
            "https://user:pw@status.example.org/",
        ] {
            assert!(!redirect_allowed(0, &url(to)), "{to}");
        }
    }

    #[test]
    fn a_running_maintenance_wins_over_a_planned_one_and_finished_ones_are_ignored() {
        let doc = summary(json!({"activeMaintenances": [
            {"id": "m3", "name": "Done", "status": "COMPLETED", "start": "2026-11-20T10:00:00.000Z",
             "duration": "30", "url": "https://s.example/m3"},
            {"id": "m1", "name": "Database upgrade", "status": "NOTSTARTEDYET",
             "start": "2026-11-21T10:00:00.000Z", "duration": "90", "url": "https://s.example/m1"},
            {"id": "m2", "name": "Network work", "status": "INPROGRESS",
             "start": "2026-11-20T09:00:00.000Z", "duration": 60, "url": "https://s.example/m2"}
        ]}));
        let s = parse_summary(&doc, None).unwrap();
        let m = s.maintenance.unwrap();
        assert_eq!(
            (m.id.as_str(), m.state, m.duration_minutes),
            ("m2", MaintenanceState::InProgress, 60)
        );
        let planned = summary(json!({"activeMaintenances": [
            {"id": "m1", "name": "Database upgrade", "status": "NOTSTARTEDYET",
             "start": "2026-11-21T10:00:00.000Z", "duration": "90", "url": "https://s.example/m1"}]}));
        let m = parse_summary(&planned, None).unwrap().maintenance.unwrap();
        assert_eq!(
            (m.state, m.duration_minutes),
            (MaintenanceState::Scheduled, 90)
        );
        assert!(m.start_ms.is_some());
        assert_eq!(m.url.as_deref(), Some("https://s.example/m1"));
    }

    #[test]
    fn an_unresolved_incident_is_shown_and_a_resolved_one_is_not() {
        let doc = summary(json!({"activeIncidents": [
            {"id": "i1", "name": "Old", "status": "RESOLVED", "url": "https://s.example/i1"},
            {"id": "i2", "name": "Gateway trouble", "status": "INVESTIGATING", "url": "https://s.example/i2"}
        ]}));
        let i = parse_summary(&doc, None).unwrap().incident.unwrap();
        assert_eq!(i.id, "i2");
        let none = summary(json!({"activeIncidents": [
            {"id": "i1", "name": "Old", "status": "MONITORING", "resolved": "2026-11-01T00:00:00.000Z"}]}));
        assert!(parse_summary(&none, None).unwrap().incident.is_none());
    }

    #[test]
    fn a_page_under_maintenance_without_one_asks_its_components() {
        let doc = summary(json!({"page": {"status": "UNDERMAINTENANCE"}}));
        assert!(needs_components(&doc));
        let components = json!({"components": [
            {"activeMaintenances": [{"id": "c1", "name": "API", "status": "INPROGRESS",
              "start": "2026-11-20T09:00:00.000Z", "duration": "45", "url": "https://s.example/c1"}]},
            {"activeMaintenances": []}
        ]});
        let m = parse_summary(&doc, Some(&components))
            .unwrap()
            .maintenance
            .unwrap();
        assert_eq!(m.id, "c1");
        // The same as a bare list.
        let bare = components["components"].clone();
        assert!(parse_summary(&doc, Some(&bare))
            .unwrap()
            .maintenance
            .is_some());
        assert!(parse_summary(&doc, None).unwrap().maintenance.is_none());
    }

    #[test]
    fn text_from_the_page_is_cleaned_and_cut_and_odd_documents_are_refused() {
        let doc = summary(json!({"activeIncidents": [
            {"id": "i", "name": format!("bad\u{202E}name{}", "x".repeat(1000)), "status": "INVESTIGATING",
             "url": "javascript:alert(1)"}]}));
        let i = parse_summary(&doc, None).unwrap().incident.unwrap();
        assert!(!i.name.contains('\u{202E}') && i.name.chars().count() <= 100);
        assert!(i.url.is_none(), "only http(s) links are kept");
        assert!(parse_summary(&json!({"hello": 1}), None).is_none());
        assert!(parse_summary(&json!([1, 2]), None).is_none());
        // A maintenance with no usable time or length is no maintenance.
        let odd = summary(json!({"activeMaintenances": [
            {"id": "m", "name": "x", "status": "INPROGRESS", "start": "soon", "duration": "5"},
            {"id": "n", "name": "x", "status": "INPROGRESS", "start": "2026-11-20T09:00:00.000Z", "duration": "0"}]}));
        assert!(parse_summary(&odd, None).unwrap().maintenance.is_none());
    }

    #[test]
    fn the_wait_follows_fluxers_cadence() {
        let idle = Status::default();
        assert_eq!(next_delay(&idle, 0, 0, 0.0), Duration::from_secs(60));
        assert_eq!(next_delay(&idle, 0, 0, 1.0), Duration::from_secs(75));
        for (failures, secs) in [(1, 60), (2, 300), (3, 900), (4, 1800), (9, 1800)] {
            assert_eq!(
                next_delay(&idle, failures, 0, 0.5),
                Duration::from_secs(secs)
            );
        }
        let soon = Status {
            maintenance: Some(Maintenance {
                id: "m".into(),
                name: "n".into(),
                state: MaintenanceState::Scheduled,
                start_ms: Some(30_000),
                duration_minutes: 10,
                url: None,
            }),
            incident: None,
        };
        // Starts in 30 s: ask when it has started.
        assert_eq!(next_delay(&soon, 0, 0, 0.0), Duration::from_secs(35));
        // Already past its time: ask again in ten seconds.
        assert_eq!(next_delay(&soon, 0, 60_000, 0.0), Duration::from_secs(10));
        // Far away: the usual cadence.
        assert_eq!(
            next_delay(&soon, 0, -3_600_000, 0.0),
            Duration::from_secs(60)
        );
    }
}
