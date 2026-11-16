// SPDX-License-Identifier: AGPL-3.0-or-later
//! The window before an age-restricted or content-warned channel, category or server.
//!
//! The rules are Fluxer's (`GuildMatureContentAgree`, `ContentWarningUtils`):
//!
//! - A channel is restricted by its own `nsfw_override`, else its category's, else the
//!   server's `nsfw`; a content warning (`content_warning_level` 1, with its text)
//!   resolves the same way. A thread is judged by its parent channel.
//! - An account the server does not allow mature content (`nsfw_allowed` false: under
//!   age, or not age-checked where that is required) is blocked: the window has no way
//!   through, and nothing is remembered.
//! - Anyone else answers once per scope (the channel, the category or the server,
//!   whichever the restriction comes from). The answer is stored in the account's synced
//!   preferences (`guild_nsfw_agreements`); only forum and media channels keep it on this
//!   computer, per account, as Fluxer does.
//! - **Fails closed.** A channel or a server the account does not know, or one whose
//!   record is missing, is blocked, never opened. The server's own effective `nsfw` of a
//!   channel counts too when the chain above says the channel is fine.

use gpui::{div, prelude::*, Context, Window};
use inlet_state::{AccountState, ChannelId};

use super::modal::modal;
use super::nav_dialog::{title_el, NavDialog};
use crate::app::Inlet;
use crate::backend::composer::gif::{NsfwScope, PrefEdit};
use crate::model::{self, ChannelKind};
use crate::theme;
use crate::ui::widgets::button;

/// Longest content warning text kept (Fluxer's limit is 200).
const MAX_WARNING_CHARS: usize = 200;

/// What a content warning covers, as Fluxer words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateScope {
    Channel,
    Category,
    Guild,
}

impl GateScope {
    fn synced(self) -> NsfwScope {
        match self {
            GateScope::Channel => NsfwScope::Channel,
            GateScope::Category => NsfwScope::Category,
            GateScope::Guild => NsfwScope::Guild,
        }
    }
}

/// What the rules say about a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateInfo {
    pub scope: GateScope,
    pub scope_id: String,
    /// Age-restricted: a blocked account cannot go on.
    pub restricted: bool,
    /// The content warning's own text, if it has one.
    pub warning: Option<String>,
    /// The channel or its server is not known: nothing opens.
    pub unknown: bool,
}

/// A window waiting for the person's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    pub info: GateInfo,
    /// The channel (or thread) that was asked for.
    pub channel: String,
    /// The channel is a link channel (what would open is a web address).
    pub link: bool,
    /// The account cannot go on: no "Continue".
    pub blocked: bool,
}

fn level_one(v: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    (v.get("content_warning_level").and_then(|l| l.as_i64()) == Some(1)).then(|| {
        v.get("content_warning_text")
            .and_then(|t| t.as_str())
            .map(|t| t.chars().take(MAX_WARNING_CHARS).collect::<String>())
            .and_then(|t| model::clean_text(&t))
            .unwrap_or_default()
    })
}

/// What stands in front of a channel, or `None` for a channel that needs nothing.
pub fn gate_of(state: &AccountState, channel: ChannelId) -> Option<GateInfo> {
    let unknown = |id: ChannelId| GateInfo {
        scope: GateScope::Channel,
        scope_id: id.to_string(),
        restricted: true,
        warning: None,
        unknown: true,
    };
    let Some(first) = state.channel(channel) else {
        return Some(unknown(channel));
    };
    // A thread is judged by the channel it belongs to.
    let (id, c) = if first.is_thread() {
        let parent = state.channels().parent_of(channel);
        match parent.and_then(|p| state.channel(p).map(|c| (p, c))) {
            Some(found) => found,
            None => return Some(unknown(channel)),
        }
    } else {
        (channel, first)
    };
    // Direct messages and personal notes have no server and no gate.
    let guild = state.channels().guild_of(id)?;
    let Some(properties) = state.guild(guild).and_then(|g| g.properties.as_ref()) else {
        return Some(unknown(id));
    };
    let category = state
        .channels()
        .parent_of(id)
        .and_then(|p| state.channel(p).map(|c| (p, c)))
        .filter(|(_, c)| c.kind == model_category());
    let own = |c: &inlet_protocol::Channel| c.extra.get("nsfw_override").and_then(|v| v.as_bool());

    // The restriction and where it comes from: the channel, its category, the server.
    let (mut restricted, mut scope, mut scope_id) = if c.kind == model_category() {
        match own(c) {
            Some(b) => (b, GateScope::Category, id.to_string()),
            None => (
                properties.extra.get("nsfw").and_then(|v| v.as_bool()) == Some(true),
                GateScope::Guild,
                guild.to_string(),
            ),
        }
    } else if let Some(b) = own(c) {
        (b, GateScope::Channel, id.to_string())
    } else if let Some(b) = category.as_ref().and_then(|(_, cat)| own(cat)) {
        let cat_id = category
            .as_ref()
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        (b, GateScope::Category, cat_id)
    } else {
        (
            properties.extra.get("nsfw").and_then(|v| v.as_bool()) == Some(true),
            GateScope::Guild,
            guild.to_string(),
        )
    };
    // The server says the channel is restricted but the chain does not: believe the
    // server, with the channel as the scope.
    if !restricted && c.nsfw {
        restricted = true;
        scope = GateScope::Channel;
        scope_id = id.to_string();
    }

    // The content warning, resolved the same way.
    let warning = level_one(&c.extra)
        .map(|t| (t, GateScope::Channel, id.to_string()))
        .or_else(|| {
            category.as_ref().and_then(|(p, cat)| {
                level_one(&cat.extra).map(|t| (t, GateScope::Category, p.to_string()))
            })
        })
        .or_else(|| level_one(&properties.extra).map(|t| (t, GateScope::Guild, guild.to_string())));
    if !restricted && warning.is_none() {
        return None;
    }
    let mut text = None;
    if let Some((t, w_scope, w_id)) = warning {
        if !t.is_empty() {
            text = Some(t);
        }
        if !restricted {
            scope = w_scope;
            scope_id = w_id;
        }
    }
    Some(GateInfo {
        scope,
        scope_id,
        restricted,
        warning: text,
        unknown: false,
    })
}

/// Whether the account has no way through: the channel or server is not known, or it is
/// age-restricted and the server does not allow this account mature content.
pub fn blocked(info: &GateInfo, nsfw_allowed: bool) -> bool {
    info.unknown || (info.restricted && !nsfw_allowed)
}

/// The channel type of a category (`4`).
fn model_category() -> i64 {
    inlet_protocol::models::channel_type::GUILD_CATEGORY
}

impl Inlet {
    /// Whether the person already accepted the warning of `scope_id`: in the account's
    /// synced preferences, or (forum and media channels only) on this computer.
    fn gate_agreed(&self, info: &GateInfo, thread_only: bool, cx: &gpui::App) -> bool {
        if thread_only && info.scope == GateScope::Channel {
            return self
                .nav_account_key(cx)
                .and_then(|key| {
                    self.sidebar
                        .remembered
                        .of(&key)
                        .map(|m| m.local_agreed.contains(&info.scope_id))
                })
                .unwrap_or(false);
        }
        self.composer
            .pickers
            .prefs
            .agreed(info.scope.synced(), &info.scope_id)
    }

    /// The gate in front of a channel or thread, unless it needs none or the person
    /// accepted it before.
    pub(crate) fn gate_for(&self, channel: &str, cx: &gpui::App) -> Option<Gate> {
        let Some(account) = self.account.as_ref() else {
            // Nothing is known: nothing opens.
            return Some(Gate {
                info: GateInfo {
                    scope: GateScope::Channel,
                    scope_id: channel.to_owned(),
                    restricted: true,
                    warning: None,
                    unknown: true,
                },
                channel: channel.to_owned(),
                link: false,
                blocked: true,
            });
        };
        let state = account.read(cx);
        let id = ChannelId::parse(channel)?;
        let info = gate_of(state, id)?;
        let allowed = state.me().nsfw_allowed();
        let blocked = blocked(&info, allowed);
        let thread_only = state.channel(id).is_some_and(|c| matches!(c.kind, 15 | 16));
        if !blocked && self.gate_agreed(&info, thread_only, cx) {
            return None;
        }
        let link = self
            .channels
            .iter()
            .any(|c| c.id == channel && c.kind == ChannelKind::Link);
        Some(Gate {
            info,
            channel: channel.to_owned(),
            link,
            blocked,
        })
    }

    /// The accepted content warning: remembered, and what was asked for opens.
    pub(super) fn gate_accepted(
        &mut self,
        gate: Gate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A blocked account has no way through.
        if gate.blocked {
            return;
        }
        let thread_only = ChannelId::parse(&gate.channel)
            .and_then(|id| self.account.as_ref().and_then(|a| a.read(cx).channel(id)))
            .is_some_and(|c| matches!(c.kind, 15 | 16));
        if thread_only && gate.info.scope == GateScope::Channel {
            if let Some(key) = self.nav_account_key(cx) {
                if let Some(mem) = self.sidebar.remembered.of_mut(&key) {
                    mem.local_agreed.insert(&gate.info.scope_id);
                    self.sidebar.remembered.save_in_background(cx);
                }
            }
        } else {
            self.edit_prefs(PrefEdit::NsfwAgree {
                scope: gate.info.scope.synced(),
                id: gate.info.scope_id.clone(),
            });
        }
        self.nav_close(cx);
        let is_thread = ChannelId::parse(&gate.channel)
            .and_then(|id| self.account.as_ref().and_then(|a| a.read(cx).channel(id)))
            .is_some_and(|c| c.is_thread());
        match ChannelId::parse(&gate.channel) {
            Some(id) if is_thread => self.open_thread(id, window, cx),
            _ => self.open_channel_row(gate.channel, window, cx),
        }
    }

    pub(super) fn render_gate(
        &self,
        gate: Gate,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let body = if gate.blocked {
            t!("nav-gate-blocked")
        } else if let Some(text) = gate.info.warning.clone() {
            text
        } else {
            match (gate.info.scope, gate.link) {
                (_, true) => t!("nav-gate-link"),
                (GateScope::Guild, _) => t!("nav-gate-guild"),
                (GateScope::Category, _) => t!("nav-gate-category"),
                (GateScope::Channel, _) => t!("nav-gate-channel"),
            }
        };
        let title = if gate.info.restricted || gate.blocked {
            t!("nav-gate-title")
        } else {
            t!("nav-gate-warning-title")
        };
        let go = gate.clone();
        let mut buttons = div().flex().flex_row().justify_end().gap_2().child(
            button(
                "nav-gate-cancel",
                if gate.blocked {
                    t!("nav-understood")
                } else {
                    t!("common-cancel")
                },
                gate.blocked,
            )
            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
        );
        if !gate.blocked {
            buttons = buttons.child(
                button("nav-gate-continue", t!("nav-gate-continue"), true).on_click(cx.listener(
                    move |this, _, window, cx| this.gate_accepted(go.clone(), window, cx),
                )),
            );
        }
        modal(
            "nav-gate",
            480.,
            [
                title_el(title),
                div()
                    .text_color(theme::text_muted())
                    .child(body)
                    .into_any_element(),
                buttons.into_any_element(),
            ],
        )
    }

    /// Shows the gate window.
    pub(crate) fn show_gate(&mut self, gate: Gate, cx: &mut Context<Self>) {
        self.nav_show(NavDialog::Gate(gate), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::models::Ready;
    use serde_json::json;

    fn state(guild: serde_json::Value, channels: serde_json::Value) -> AccountState {
        let mut props = json!({"id": "100", "name": "G", "owner_id": "1"});
        props
            .as_object_mut()
            .unwrap()
            .extend(guild.as_object().unwrap().clone());
        let ready: Ready = serde_json::from_value(json!({
            "session_id": "s", "user": {"id": "10", "username": "me"},
            "private_channels": [], "users": [], "read_states": [],
            "user_guild_settings": [], "presences": [],
            "guilds": [{
                "id": "100",
                "properties": props,
                "roles": [{"id": "100", "name": "@everyone", "permissions": "3072", "position": 0}],
                "channels": channels,
                "members": [{"user": {"id": "10", "username": "me"}, "roles": [], "joined_at": "2026-01-01T00:00:00Z"}]
            }]
        }))
        .unwrap();
        AccountState::from_ready_at(&ready, 1)
    }

    fn channels() -> serde_json::Value {
        json!([
            {"id": "300", "type": 4, "name": "mature", "position": 0, "nsfw_override": true},
            {"id": "301", "type": 0, "name": "in-mature", "parent_id": "300", "position": 1},
            {"id": "302", "type": 0, "name": "own", "nsfw_override": true, "position": 2},
            {"id": "303", "type": 0, "name": "safe", "position": 3, "nsfw_override": false},
            {"id": "304", "type": 0, "name": "plain", "position": 4},
            {"id": "305", "type": 0, "name": "warned", "position": 5,
             "content_warning_level": 1, "content_warning_text": "Spoilers ahead"},
            {"id": "306", "type": 0, "name": "server-said", "position": 6, "nsfw": true},
            {"id": "307", "type": 11, "name": "thread", "parent_id": "301", "position": 7}
        ])
    }

    fn scope(s: &AccountState, id: u64) -> Option<(GateScope, String)> {
        gate_of(s, ChannelId(id)).map(|i| (i.scope, i.scope_id))
    }

    #[test]
    fn age_restricted_channels_ask_before_opening() {
        let s = state(json!({"nsfw": false}), channels());
        // The channel's own setting, its category's, and what the server decided.
        assert_eq!(scope(&s, 302), Some((GateScope::Channel, "302".into())));
        assert_eq!(scope(&s, 301), Some((GateScope::Category, "300".into())));
        assert_eq!(scope(&s, 300), Some((GateScope::Category, "300".into())));
        assert_eq!(scope(&s, 306), Some((GateScope::Channel, "306".into())));
        assert_eq!(scope(&s, 304), None);
        // An explicit "not age-restricted" wins over the server.
        assert_eq!(scope(&s, 303), None);
        // A restricted server gates every channel that does not say otherwise, once for
        // the whole server.
        let s = state(json!({"nsfw": true}), channels());
        assert_eq!(scope(&s, 304), Some((GateScope::Guild, "100".into())));
        assert_eq!(scope(&s, 303), None);
        assert_eq!(scope(&s, 301), Some((GateScope::Category, "300".into())));
    }

    #[test]
    fn a_content_warning_alone_asks_and_shows_its_text() {
        let s = state(json!({"nsfw": false}), channels());
        let info = gate_of(&s, ChannelId(305)).unwrap();
        assert!(!info.restricted);
        assert_eq!(info.warning.as_deref(), Some("Spoilers ahead"));
        assert_eq!(
            (info.scope, info.scope_id.as_str()),
            (GateScope::Channel, "305")
        );
        // A warning on the server covers its channels, once.
        let s = state(
            json!({"content_warning_level": 1, "content_warning_text": "Be kind"}),
            channels(),
        );
        let info = gate_of(&s, ChannelId(304)).unwrap();
        assert_eq!(
            (info.scope, info.warning.as_deref()),
            (GateScope::Guild, Some("Be kind"))
        );
    }

    #[test]
    fn a_thread_under_a_gated_channel_asks_first() {
        let s = state(json!({"nsfw": false}), channels());
        // Thread 307 sits under 301, which is under the restricted category.
        let info = gate_of(&s, ChannelId(307)).unwrap();
        assert_eq!(
            (info.scope, info.scope_id.as_str()),
            (GateScope::Category, "300")
        );
        assert!(info.restricted && !info.unknown);
    }

    #[test]
    fn missing_data_is_blocked_not_opened() {
        let s = state(json!({"nsfw": false}), channels());
        // A channel the account does not know.
        let info = gate_of(&s, ChannelId(999)).unwrap();
        assert!(info.unknown);
        // A thread whose parent is not known.
        let s2 = state(
            json!({}),
            json!([{"id": "400", "type": 11, "name": "orphan", "parent_id": "401", "position": 0}]),
        );
        assert!(gate_of(&s2, ChannelId(400)).is_some_and(|i| i.unknown));
    }

    /// Every way into a channel (a click on its row, a jump from a link, a search result
    /// or the inbox, a favourite) ends in `select_channel`, so the window comes first and
    /// the channel is not opened; the demo account is not allowed mature content, so it
    /// is blocked and has no way through.
    #[cfg(feature = "demo")]
    #[gpui::test]
    fn every_way_into_a_gated_channel_asks_first(cx: &mut gpui::TestAppContext) {
        use crate::demo::{scenes::Scene, snowflake};
        use crate::layout_probe::Probe;
        let (probe, cx) = cx.add_window_view(|w, cx| Probe::in_scene(Scene::GuildBanner, w, cx));
        cx.run_until_parked();
        let mature = snowflake("c-mature");
        probe.update_in(cx, |probe, window, cx| {
            probe.inlet().clone().update(cx, |inlet, cx| {
                let was = inlet.selected_channel.clone();
                assert_ne!(was.as_deref(), Some(mature.as_str()));
                let asked = |inlet: &Inlet| match &inlet.nav.dialog {
                    Some(NavDialog::Gate(g)) => g.blocked && g.channel == mature,
                    _ => false,
                };
                // A click on its row.
                inlet.open_channel_row(mature.clone(), window, cx);
                assert!(asked(inlet), "the row");
                assert_eq!(inlet.selected_channel, was);
                // A jump from a link, a search result or the inbox.
                inlet.nav.dialog = None;
                inlet.chat.pending_channel_jump = Some(mature.clone());
                inlet.take_channel_jump(window, cx);
                assert!(asked(inlet), "a jump");
                assert_eq!(inlet.selected_channel, was);
                // A blocked window has no way through.
                let gate = match &inlet.nav.dialog {
                    Some(NavDialog::Gate(g)) => Some(g.clone()),
                    _ => None,
                };
                if let Some(g) = gate {
                    inlet.gate_accepted(g, window, cx);
                }
                assert_eq!(inlet.selected_channel, was);
                assert!(matches!(inlet.nav.dialog, Some(NavDialog::Gate(_))));
                // A channel that needs nothing opens as before.
                inlet.nav.dialog = None;
                inlet.select_channel(snowflake("c-dev"), window, cx);
                assert_eq!(inlet.selected_channel, Some(snowflake("c-dev")));
                assert!(inlet.nav.dialog.is_none());
            });
        });
    }

    #[test]
    fn a_blocked_account_cannot_continue() {
        // `nsfw_allowed` is missing or false: the gate is blocked, and `gate_accepted`
        // refuses to remember or open anything for a blocked gate.
        let s = state(json!({"nsfw": false}), channels());
        assert!(!s.me().nsfw_allowed());
        let info = gate_of(&s, ChannelId(302)).unwrap();
        assert!(blocked(&info, s.me().nsfw_allowed()));
        assert!(!blocked(&info, true), "an allowed account may accept");
        // A plain content warning is not an age restriction: it can be accepted.
        let info = gate_of(&s, ChannelId(305)).unwrap();
        assert!(!blocked(&info, false));
        // Unknown data is blocked for everyone.
        let info = gate_of(&s, ChannelId(999)).unwrap();
        assert!(blocked(&info, true));
    }
}
