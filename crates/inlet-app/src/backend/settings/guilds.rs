// SPDX-License-Identifier: AGPL-3.0-or-later
//! Joining and leaving guilds: look at an invite before joining it, join through it, and
//! leave a guild.
//!
//! An invite code is untrusted input (it comes from a link in a message, a deep link or
//! a field). It is checked before any request (`invite_code`: letters, digits, `-` and
//! `_`, 1-256 characters) and every request goes through the protocol's limiter.
//! Looking at an invite is `GET /invites/{code}` and uses nothing up; joining is a
//! separate command that only the window's button sends. Request budget: at most one
//! invite request a second per account ([`InviteGate`]) and one leave at a time, each
//! started by a click.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use inlet_protocol as p;
use inlet_protocol::api::invites::{invite_code, invite_type};

use super::super::session::{Failure, FailureKind};
use super::super::{Active, Backend, Internal, Ui};
use super::Ev as SettingsEv;
use crate::model::clean_name;

/// Shortest time between two invite requests of one account.
pub const MIN_INVITE_GAP: Duration = Duration::from_secs(1);

pub enum Cmd {
    /// Look at an invite without joining it.
    PreviewInvite { code: String },
    /// Join through an invite the person has seen.
    AcceptInvite { code: String },
    /// Leave a guild.
    Leave { guild_id: String },
}

/// What an invite leads to, with the names cleaned and cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub code: String,
    /// A guild invite or a group DM invite.
    pub kind: Kind,
    pub guild_id: Option<String>,
    pub guild_name: String,
    /// The guild's icon hash.
    pub icon: Option<String>,
    pub channel_name: Option<String>,
    /// The channel the invite points at (a group DM invite opens it after joining).
    pub channel_id: Option<String>,
    pub inviter: Option<String>,
    pub members: Option<u64>,
    pub online: Option<u64>,
}

/// What the instance's configuration says about the rail: Fluxer's `RuntimeConfig`
/// (`community.single_community`, `community.single_community_guild_id`,
/// `features.discovery_enabled`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceNav {
    /// The instance is one community every account joins: no Add and no Explore, and
    /// the stock community cannot be left.
    pub single_community: bool,
    pub stock_guild: Option<String>,
    /// Discovery is on (unless the instance says it is not).
    pub discovery_off: bool,
}

impl InstanceNav {
    /// Reads the discovery document's raw parts.
    pub fn of(
        community: Option<&serde_json::Value>,
        features: &serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        let single = community
            .and_then(|c| c.get("single_community"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let stock = community
            .and_then(|c| c.get("single_community_guild_id"))
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned);
        InstanceNav {
            single_community: single,
            stock_guild: stock,
            discovery_off: features.get("discovery_enabled").and_then(|v| v.as_bool())
                == Some(false),
        }
    }

    /// Fluxer's `isDiscoveryAvailable`: Explore shows only then.
    pub fn explore(&self) -> bool {
        !self.single_community && !self.discovery_off
    }

    /// Add (create or join a community) shows unless the instance is a single community.
    pub fn add(&self) -> bool {
        !self.single_community
    }

    /// Fluxer's `isStockCommunityGuild`: the stock community cannot be left.
    pub fn is_stock(&self, guild: &str) -> bool {
        self.single_community && self.stock_guild.as_deref() == Some(guild)
    }
}

/// Tells the UI what the signed-in instance's configuration says about the rail.
pub(in crate::backend) fn instance_known(backend: &Backend, session: &p::Session) {
    let d = session.instance().discovery();
    let nav = InstanceNav::of(d.extra.get("community"), &d.features);
    let _ = backend
        .events
        .unbounded_send(SettingsEv::Guild(Ev::Instance(Box::new(nav))));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Guild,
    GroupDm,
}

pub enum Ev {
    /// The answer to `PreviewInvite`.
    Preview {
        code: String,
        result: Result<Box<Preview>, String>,
    },
    /// The answer to `AcceptInvite`: where the invite led (the guild, or the group DM).
    Accepted {
        code: String,
        result: Result<Box<Preview>, String>,
    },
    /// What the signed-in instance's configuration says (sent when an account becomes
    /// the shown one).
    Instance(Box<InstanceNav>),
    /// The answer to `Leave`.
    Left {
        guild_id: String,
        result: Result<(), String>,
    },
}

/// At most one invite request per [`MIN_INVITE_GAP`]. Looking at an invite and joining it have
/// a gate each, so a click on "Join" right after the preview is not turned away.
#[derive(Debug, Default)]
pub struct InviteGate {
    last: Option<Instant>,
}

impl InviteGate {
    /// Whether a request may go now; records it when it may.
    pub fn admit(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < MIN_INVITE_GAP)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// The guilds being left right now: one request per guild at a time, and few at once.
#[derive(Debug, Default)]
pub struct LeaveGate {
    open: HashSet<String>,
}

impl LeaveGate {
    /// Most leaves in flight at once.
    pub const MAX: usize = 4;

    /// Whether leaving `guild` may start now; records it when it may.
    pub fn begin(&mut self, guild: &str) -> bool {
        self.open.len() < Self::MAX && self.open.insert(guild.to_owned())
    }

    pub fn end(&mut self, guild: &str) {
        self.open.remove(guild);
    }
}

/// Frees a guild's leave when its task ends, however it ends.
struct LeaveGuard {
    ops: Arc<Mutex<super::Ops>>,
    guild: String,
}

impl Drop for LeaveGuard {
    fn drop(&mut self) {
        self.ops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .leaving
            .end(&self.guild);
    }
}

/// The preview of an invite, from what the server sent.
pub fn preview_of(invite: &p::Invite) -> Preview {
    let name = |s: &str| clean_name(&s.chars().take(100).collect::<String>());
    let kind = if invite.kind == invite_type::GROUP_DM {
        Kind::GroupDm
    } else {
        Kind::Guild
    };
    let channel_name = invite
        .channel
        .as_ref()
        .and_then(|c| c.name.as_deref())
        .map(name)
        .filter(|n| !n.is_empty());
    let guild_name = invite
        .guild
        .as_ref()
        .and_then(|g| g.name.as_deref())
        .map(name)
        .or_else(|| channel_name.clone())
        .unwrap_or_default();
    Preview {
        code: invite.code.chars().take(256).collect(),
        kind,
        guild_id: invite
            .guild
            .as_ref()
            .map(|g| g.id.clone())
            .filter(|id| id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())),
        guild_name,
        icon: invite
            .guild
            .as_ref()
            .and_then(|g| g.icon.clone())
            .filter(|h| h.len() <= 64 && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')),
        channel_id: invite
            .channel
            .as_ref()
            .map(|c| c.id.clone())
            .filter(|id| id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())),
        channel_name,
        inviter: invite.inviter.as_ref().map(|u| name(u.display_name())),
        members: invite.member_count,
        online: invite.presence_count,
    }
}

fn reason(
    internal: &tokio::sync::mpsc::UnboundedSender<Internal>,
    gen: u64,
    err: p::Error,
) -> Option<String> {
    if matches!(err, p::Error::RateLimited { .. }) {
        return Some(t!("invite-too-fast"));
    }
    let failure = Failure::from(err);
    if failure.kind == FailureKind::SessionEnded {
        let _ = internal.send(Internal::Ended {
            gen,
            message: failure.message,
        });
        return None;
    }
    Some(failure.message)
}

pub(super) fn handle(backend: &mut Backend, cmd: Cmd) {
    // The answer to a refused command: the window must not wait for it.
    let refuse = |events: &Ui, cmd: &Cmd, why: String| {
        let ev = match cmd {
            Cmd::PreviewInvite { code } => Ev::Preview {
                code: code.clone(),
                result: Err(why),
            },
            Cmd::AcceptInvite { code } => Ev::Accepted {
                code: code.clone(),
                result: Err(why),
            },
            Cmd::Leave { guild_id } => Ev::Left {
                guild_id: guild_id.clone(),
                result: Err(why),
            },
        };
        let _ = events.unbounded_send(SettingsEv::Guild(ev));
    };
    // Checked before anything is sent.
    let valid = match &cmd {
        Cmd::PreviewInvite { code } | Cmd::AcceptInvite { code } => invite_code(code).is_ok(),
        Cmd::Leave { guild_id } => {
            !guild_id.is_empty()
                && guild_id.len() <= 20
                && guild_id.bytes().all(|b| b.is_ascii_digit())
        }
    };
    if !valid {
        return refuse(&backend.events, &cmd, t!("invite-not-valid"));
    }
    let (client, gen, events, ops) = match &mut backend.active {
        #[cfg(feature = "demo")]
        Some(Active::Demo(m)) => return m.guild_command(cmd),
        Some(Active::Real(r)) => (
            r.session.client().clone(),
            r.gen,
            r.events.clone(),
            r.ops.clone(),
        ),
        None => return refuse(&backend.events, &cmd, t!("invite-not-connected")),
    };
    // Invites: one request a second. Leaving: one at a time for a guild.
    let leave_guard = match &cmd {
        Cmd::Leave { guild_id } => {
            let began = ops
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .leaving
                .begin(guild_id);
            if !began {
                return refuse(&events, &cmd, t!("nav-leave-busy"));
            }
            Some(LeaveGuard {
                ops: ops.clone(),
                guild: guild_id.clone(),
            })
        }
        _ => {
            let mut ops = ops.lock().unwrap_or_else(|e| e.into_inner());
            let admitted = match &cmd {
                Cmd::AcceptInvite { .. } => ops.accepts.admit(Instant::now()),
                _ => ops.invites.admit(Instant::now()),
            };
            if !admitted {
                return refuse(&events, &cmd, t!("invite-too-fast"));
            }
            None
        }
    };
    let internal = backend.internal.clone();
    let task = backend.spawn_guarded("guild membership", None, async move {
        let _leave = leave_guard;
        match cmd {
            Cmd::PreviewInvite { code } => {
                let result = match client.invite(&code).await {
                    Ok(invite) => Ok(Box::new(preview_of(&invite))),
                    Err(err) => match reason(&internal, gen, err) {
                        Some(why) => Err(why),
                        None => return,
                    },
                };
                let _ = events.unbounded_send(SettingsEv::Guild(Ev::Preview { code, result }));
            }
            Cmd::AcceptInvite { code } => {
                let result = match client.accept_invite(&code).await {
                    Ok(invite) => Ok(Box::new(preview_of(&invite))),
                    Err(err) => match reason(&internal, gen, err) {
                        Some(why) => Err(why),
                        None => return,
                    },
                };
                let _ = events.unbounded_send(SettingsEv::Guild(Ev::Accepted { code, result }));
            }
            Cmd::Leave { guild_id } => {
                let path = format!("/users/@me/guilds/{guild_id}");
                let result = match client.request_json(p::Method::DELETE, &path, None).await {
                    Ok(_) => Ok(()),
                    Err(err) => match reason(&internal, gen, err) {
                        Some(why) => Err(why),
                        None => return,
                    },
                };
                let _ = events.unbounded_send(SettingsEv::Guild(Ev::Left { guild_id, result }));
            }
        }
    });
    if let Some(Active::Real(r)) = &mut backend.active {
        r.settings_writes.retain(|t| !t.is_finished());
        r.settings_writes.push(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invite_requests_are_one_a_second() {
        let mut gate = InviteGate::default();
        let start = Instant::now();
        let mut sent = 0;
        // A click every 50 ms for ten seconds.
        for i in 0..200u64 {
            if gate.admit(start + Duration::from_millis(i * 50)) {
                sent += 1;
            }
        }
        assert!((10..=11).contains(&sent), "{sent} requests");
    }

    #[test]
    fn a_guild_is_left_once_at_a_time() {
        let mut gate = LeaveGate::default();
        assert!(gate.begin("1"));
        assert!(!gate.begin("1"), "the same guild twice");
        for id in ["2", "3", "4"] {
            assert!(gate.begin(id));
        }
        assert!(!gate.begin("5"), "no more than {} at once", LeaveGate::MAX);
        gate.end("1");
        assert!(gate.begin("5"));
        assert!(!gate.begin("1"), "still four open");
    }

    #[test]
    fn a_preview_shows_cleaned_names_and_keeps_only_safe_ids() {
        let invite: p::Invite = serde_json::from_value(json!({
            "code": "abc123", "type": 0,
            "guild": {"id": "55", "name": "Evil\u{202E}Guild  ", "icon": "a_12ab"},
            "channel": {"id": "7", "name": "general", "type": 0},
            "inviter": {"id": "9", "username": "ada"},
            "member_count": 120, "presence_count": 14
        }))
        .unwrap();
        let preview = preview_of(&invite);
        assert_eq!(preview.kind, Kind::Guild);
        assert!(!preview.guild_name.contains('\u{202E}'));
        assert_eq!(preview.guild_id.as_deref(), Some("55"));
        assert_eq!(preview.icon.as_deref(), Some("a_12ab"));
        assert_eq!((preview.members, preview.online), (Some(120), Some(14)));
        assert_eq!(preview.inviter.as_deref(), Some("ada"));

        // An icon or ID that is not a plain hash or number is dropped.
        let odd: p::Invite = serde_json::from_value(json!({
            "code": "x", "type": 1,
            "guild": {"id": "../x", "name": "G", "icon": "../../a.png"},
            "channel": {"id": "7", "name": "party", "type": 3}
        }))
        .unwrap();
        let preview = preview_of(&odd);
        assert_eq!(preview.kind, Kind::GroupDm);
        assert_eq!((preview.guild_id, preview.icon), (None, None));
    }
}
