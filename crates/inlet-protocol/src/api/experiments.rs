//! Experiment assignments (`GET /experiments`): which staged features the
//! instance has switched on for the signed-in account, and how often to ask
//! again. Fluxer's own client revalidates with `If-None-Match` and stops after
//! three answers of 404 in a row; [`Experiments`] carries what it needs for that
//! ([`Experiments::next_poll`]).
//!
//! The document is read as Fluxer's schema reads it: a wrong shape makes the whole
//! answer [`ExperimentsAnswer::Malformed`] and the caller keeps what it had (the
//! failure count is reset), and an unknown assignment is ignored. The poll interval and jitter are clamped to
//! the limits of Fluxer's schema, so an instance cannot make the client poll
//! faster than every 30 seconds.

use std::time::Duration;

use reqwest::Method;
use serde_json::Value;

use crate::error::Result;
use crate::Client;

/// Shortest poll interval the schema allows, in seconds.
pub const MIN_POLL_INTERVAL_SECS: u64 = 60;
/// Longest poll interval the schema allows, in seconds.
pub const MAX_POLL_INTERVAL_SECS: u64 = 86_400;
/// The interval used when the document has none, in seconds.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 300;
/// Largest jitter the schema allows, in percent.
pub const MAX_POLL_JITTER_PERCENT: u8 = 50;
/// The jitter used when the document has none, in percent.
pub const DEFAULT_POLL_JITTER_PERCENT: u8 = 15;
/// The shortest wait between two polls whatever the jitter does (half the shortest
/// interval, as Fluxer's client).
pub const MIN_POLL_DELAY: Duration = Duration::from_secs(MIN_POLL_INTERVAL_SECS / 2);
/// Answers of 404 in a row after which the client stops asking (Fluxer's client).
pub const MAX_NOT_FOUND_STREAK: u32 = 3;

/// `assignments.channel_threads`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelThreads {
    /// Always `true` when the assignment is there.
    pub active: bool,
    /// Version of the instance's threads configuration.
    pub config_version: u64,
}

/// `assignments.voice_p2p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceP2p {
    /// Peer-to-peer calls are on for this account.
    pub enabled: bool,
    /// Most participants in a peer-to-peer call.
    pub max_participants: Option<u32>,
}

/// The assignments of one answer. `None` means the answer does not have it
/// (an older instance, or an entry that did not parse).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Assignments {
    /// Threads and forums.
    pub channel_threads: Option<ChannelThreads>,
    /// Visiting a community without joining it.
    pub guild_visits: Option<bool>,
    /// Peer-to-peer voice calls.
    pub voice_p2p: Option<VoiceP2p>,
    /// The new premium page.
    pub plutonium_page: Option<bool>,
    /// The move to a new domain.
    pub domain_migration: Option<bool>,
}

/// One answer of `GET /experiments`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Experiments {
    /// Seconds between two polls, clamped to the schema's limits.
    pub poll_interval_secs: u64,
    /// Jitter around the interval, in percent, clamped to 0-50.
    pub poll_jitter_percent: u8,
    /// What the instance assigned.
    pub assignments: Assignments,
}

impl Default for Experiments {
    fn default() -> Self {
        Experiments {
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            poll_jitter_percent: DEFAULT_POLL_JITTER_PERCENT,
            assignments: Assignments::default(),
        }
    }
}

/// An optional assignment: `Ok(None)` when it is not there, `Err` when it is there with
/// the wrong shape (the whole answer is then refused, as Fluxer's schema does).
fn entry<T>(
    assignments: &serde_json::Map<String, Value>,
    key: &str,
    read: impl FnOnce(&Value) -> Option<T>,
) -> Result<Option<T>, ()> {
    match assignments.get(key) {
        None => Ok(None),
        Some(v) => read(v).map(Some).ok_or(()),
    }
}

fn enabled(v: &Value) -> Option<bool> {
    v.get("enabled")?.as_bool()
}

impl Experiments {
    /// Reads an answer the way Fluxer's schema does: the poll interval (an integer), the
    /// jitter (an integer from 0 to 50) and `assignments` (an object) must be there, and
    /// an assignment that is there must have its shape (`channel_threads.active` is
    /// `true`, a flag is a boolean, `max_participants` an integer from 2). Anything else
    /// gives `None`, and the caller keeps what it had. Unknown assignments and fields are
    /// ignored. The interval is clamped to 60 s to 24 h afterwards.
    pub fn parse(body: &Value) -> Option<Experiments> {
        let obj = body.as_object()?;
        let interval = obj.get("poll_interval_seconds")?.as_i64()?;
        let jitter = obj.get("poll_jitter_percent")?.as_i64()?;
        if !(0..=i64::from(MAX_POLL_JITTER_PERCENT)).contains(&jitter) {
            return None;
        }
        let a = obj.get("assignments")?.as_object()?;
        let channel_threads = entry(a, "channel_threads", |t| {
            // Only `active: true` exists on the wire.
            if t.get("active")?.as_bool()? {
                Some(ChannelThreads {
                    active: true,
                    config_version: t.get("config_version")?.as_u64()?,
                })
            } else {
                None
            }
        })
        .ok()?;
        let voice_p2p = entry(a, "voice_p2p", |v| {
            let max = v.get("max_participants")?.as_u64()?;
            (max >= 2).then_some(())?;
            Some(VoiceP2p {
                enabled: v.get("enabled")?.as_bool()?,
                max_participants: u32::try_from(max).ok(),
            })
        })
        .ok()?;
        Some(Experiments {
            poll_interval_secs: u64::try_from(interval)
                .unwrap_or(MIN_POLL_INTERVAL_SECS)
                .clamp(MIN_POLL_INTERVAL_SECS, MAX_POLL_INTERVAL_SECS),
            poll_jitter_percent: jitter as u8,
            assignments: Assignments {
                channel_threads,
                guild_visits: entry(a, "guild_visits", enabled).ok()?,
                voice_p2p,
                plutonium_page: entry(a, "plutonium_page", enabled).ok()?,
                domain_migration: entry(a, "domain_migration", enabled).ok()?,
            },
        })
    }

    /// How long to wait before the next poll: the interval with the jitter applied
    /// (`unit` is a number from -1 to 1, normally random), doubled for each
    /// failure in a row (at most twice, as Fluxer's client), and never shorter than
    /// [`MIN_POLL_DELAY`]. `hidden` multiplies the wait by four (a window that is not
    /// shown).
    pub fn next_poll(&self, failures: u32, hidden: bool, unit: f64) -> Duration {
        let unit = if unit.is_finite() {
            unit.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let base = self.poll_interval_secs as f64
            * if hidden { 4.0 } else { 1.0 }
            * f64::from(1u32 << failures.min(2));
        let jitter = base * f64::from(self.poll_jitter_percent) / 100.0 * unit;
        Duration::from_secs_f64((base + jitter).max(MIN_POLL_DELAY.as_secs_f64()))
    }
}

/// The answer of [`Client::experiments`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentsAnswer {
    /// A new document, with its `ETag`.
    Fresh(Experiments, Option<String>),
    /// The `ETag` sent still matches.
    NotModified,
    /// The instance has no such route (404): an older instance.
    NotFound,
    /// The body was not the document. Keep what is known and ask again later.
    Malformed,
}

impl Client {
    /// `GET /experiments` with `If-None-Match` when `etag` is given.
    pub async fn experiments(&self, etag: Option<&str>) -> Result<ExperimentsAnswer> {
        let limiter = self.rate_limiter();
        let mut headers: Vec<(&'static str, String)> = Vec::new();
        if let Some(tag) = etag.filter(|t| !t.is_empty() && t.len() <= 256) {
            headers.push(("If-None-Match", tag.to_owned()));
        }
        let raw = crate::rest::send(
            self.instance(),
            &limiter,
            Some(self.token()),
            crate::rest::Request {
                method: Method::GET,
                path: "/experiments",
                body: None,
                headers: &headers,
                upload: None,
            },
        )
        .await?;
        match raw.status {
            304 => Ok(ExperimentsAnswer::NotModified),
            404 => Ok(ExperimentsAnswer::NotFound),
            s if (200..300).contains(&s) => Ok(match Experiments::parse(&raw.value()) {
                Some(e) => ExperimentsAnswer::Fresh(e, raw.etag),
                None => ExperimentsAnswer::Malformed,
            }),
            _ => Err(crate::error::Error::Api(Box::new(raw.api_error()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_answer_is_read() {
        let e = Experiments::parse(&json!({
            "poll_interval_seconds": 120,
            "poll_jitter_percent": 10,
            "assignments": {
                "domain_migration": {"enabled": true},
                "channel_threads": {"active": true, "config_version": 4},
                "plutonium_page": {"enabled": false},
                "voice_p2p": {"enabled": true, "max_participants": 3},
                "guild_visits": {"enabled": true},
                "something_new": {"enabled": true}
            }
        }))
        .unwrap();
        assert_eq!(e.poll_interval_secs, 120);
        assert_eq!(e.poll_jitter_percent, 10);
        let a = e.assignments;
        assert_eq!(a.domain_migration, Some(true));
        assert_eq!(
            a.channel_threads,
            Some(ChannelThreads {
                active: true,
                config_version: 4
            })
        );
        assert_eq!(a.plutonium_page, Some(false));
        assert_eq!(
            a.voice_p2p,
            Some(VoiceP2p {
                enabled: true,
                max_participants: Some(3)
            })
        );
        assert_eq!(a.guild_visits, Some(true));
    }

    #[test]
    fn one_broken_entry_refuses_the_whole_answer_as_fluxers_schema_does() {
        let ok = |a: Value| json!({"poll_interval_seconds": 300, "poll_jitter_percent": 15, "assignments": a});
        assert!(Experiments::parse(&ok(json!({"plutonium_page": {"enabled": true}}))).is_some());
        for broken in [
            json!({"guild_visits": "yes"}),
            json!({"channel_threads": {"config_version": 1}}),
            // `active` is `true` or the entry is not there.
            json!({"channel_threads": {"active": false, "config_version": 1}}),
            json!({"voice_p2p": {"enabled": true, "max_participants": 1}}),
            json!({"voice_p2p": {"enabled": true}}),
            json!({"domain_migration": {"enabled": 1}}),
        ] {
            assert!(
                Experiments::parse(&ok(broken.clone())).is_none(),
                "{broken}"
            );
        }
        // What is missing at the top is refused too.
        assert!(Experiments::parse(&json!({})).is_none());
        assert!(
            Experiments::parse(&json!({"poll_jitter_percent": 15, "assignments": {}})).is_none()
        );
        assert!(
            Experiments::parse(&json!({"poll_interval_seconds": 300, "assignments": {}})).is_none()
        );
        assert!(Experiments::parse(
            &json!({"poll_interval_seconds": 300, "poll_jitter_percent": 15})
        )
        .is_none());
        assert!(Experiments::parse(
            &json!({"poll_interval_seconds": 300, "poll_jitter_percent": 51, "assignments": {}})
        )
        .is_none());
        assert!(Experiments::parse(&json!([1, 2])).is_none());
        // No assignments at all is a valid answer.
        assert_eq!(
            Experiments::parse(&ok(json!({}))).unwrap().assignments,
            Assignments::default()
        );
    }

    #[test]
    fn the_poll_cadence_is_clamped() {
        let fast = Experiments::parse(&json!({
            "poll_interval_seconds": 1, "poll_jitter_percent": 50, "assignments": {}
        }))
        .unwrap();
        assert_eq!(fast.poll_interval_secs, MIN_POLL_INTERVAL_SECS);
        assert_eq!(fast.poll_jitter_percent, MAX_POLL_JITTER_PERCENT);
        let slow = Experiments::parse(&json!({
            "poll_interval_seconds": 9_999_999, "poll_jitter_percent": 0, "assignments": {}
        }))
        .unwrap();
        assert_eq!(slow.poll_interval_secs, MAX_POLL_INTERVAL_SECS);
        assert_eq!(slow.poll_jitter_percent, 0);
        // Whatever the jitter, the wait never drops below half the shortest interval.
        assert!(fast.next_poll(0, false, -1.0) >= MIN_POLL_DELAY);
    }

    #[test]
    fn the_wait_grows_with_failures_and_when_hidden() {
        let e = Experiments {
            poll_interval_secs: 300,
            poll_jitter_percent: 0,
            ..Experiments::default()
        };
        assert_eq!(e.next_poll(0, false, 0.5), Duration::from_secs(300));
        assert_eq!(e.next_poll(1, false, 0.5), Duration::from_secs(600));
        assert_eq!(e.next_poll(2, false, 0.5), Duration::from_secs(1200));
        assert_eq!(e.next_poll(9, false, 0.5), Duration::from_secs(1200));
        assert_eq!(e.next_poll(0, true, 0.5), Duration::from_secs(1200));
        let jittery = Experiments {
            poll_jitter_percent: 20,
            ..e
        };
        assert_eq!(jittery.next_poll(0, false, -1.0), Duration::from_secs(240));
        assert_eq!(jittery.next_poll(0, false, 1.0), Duration::from_secs(360));
        assert_eq!(
            jittery.next_poll(0, false, f64::NAN),
            Duration::from_secs(300)
        );
    }
}
