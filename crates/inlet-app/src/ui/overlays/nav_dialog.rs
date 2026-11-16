// SPDX-License-Identifier: AGPL-3.0-or-later
//! The windows of the rail and the channel list: the server menu, leaving a server, a
//! folder's settings, an invite, adding a server, the content warning before an
//! age-restricted channel, and the "coming soon" window of the entries that need
//! features Inlet does not have yet.
//!
//! They are one layer: only one is open at a time, it is the topmost layer while it is
//! open, and Escape closes it (see `desktop::Layer::Nav`).

use gpui::{div, prelude::*, px, AnyElement, Context, FocusHandle, FontWeight, Window};
use inlet_state::{AccountState, ChannelId, GuildId};

use super::modal::modal;
use crate::app::Inlet;
use crate::ui::overlays::invite_accept::{AddServer, InviteView};
use crate::ui::rail::folder::FolderEdit;
use crate::ui::widgets::button;
use crate::{model, theme};

/// What the "coming soon" window is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Soon {
    Favorites,
    Discover,
    CreateServer,
}

/// What a content warning is about, as Fluxer words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateScope {
    Channel,
    Category,
    Guild,
}

/// A content warning waiting for the person's answer. "Continue" remembers the answer
/// for `scope_id` and goes on with what was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    pub scope: GateScope,
    pub scope_id: String,
    /// The channel that was asked for.
    pub channel: String,
    /// The channel is a link channel (what would open is a web address).
    pub link: bool,
}

/// Which window is open.
pub enum NavDialog {
    /// The menu under the server name.
    GuildMenu,
    /// Leaving a server: the question, and its answer on its way.
    Leave {
        guild: String,
        name: String,
        busy: bool,
        error: Option<String>,
    },
    Folder(FolderEdit),
    Invite(InviteView),
    AddServer(AddServer),
    Gate(Gate),
    Soon(Soon),
}

/// The state of these windows.
#[derive(Default)]
pub struct NavState {
    pub dialog: Option<NavDialog>,
    /// The server a join asked for: opened when the guild list has it.
    pub join_target: Option<String>,
}

/// Whether a channel is age-restricted for this account, and what the warning is
/// about. Fluxer's rule: the channel's own setting, else its category's, else the
/// server's. `nsfw_override` is the setting; `nsfw` is what the server decided for the
/// channel itself.
pub fn gate_of(state: &AccountState, channel: ChannelId) -> Option<(GateScope, String)> {
    let c = state.channel(channel)?;
    let own = c.extra.get("nsfw_override").and_then(|v| v.as_bool());
    if own == Some(true) || (own.is_none() && c.nsfw && c.kind != 4) {
        return Some((GateScope::Channel, channel.to_string()));
    }
    if own == Some(false) {
        return None;
    }
    let parent = state
        .channels()
        .parent_of(channel)
        .and_then(|p| state.channel(p));
    if let Some(parent) = parent {
        match parent.extra.get("nsfw_override").and_then(|v| v.as_bool()) {
            Some(true) => return Some((GateScope::Category, parent.id.clone())),
            Some(false) => return None,
            None => {}
        }
    }
    let guild = state.channels().guild_of(channel)?;
    let properties = state.guild(guild)?.properties.as_ref()?;
    let nsfw = properties
        .extra
        .get("nsfw")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    nsfw.then(|| (GateScope::Guild, guild.to_string()))
}

impl Inlet {
    /// The gate in front of a channel, unless the person accepted it before.
    pub(crate) fn gate_for(&self, channel: &str, cx: &gpui::App) -> Option<Gate> {
        let account = self.account.as_ref()?;
        let state = account.read(cx);
        let id = ChannelId::parse(channel)?;
        let (scope, scope_id) = gate_of(state, id)?;
        if self.sidebar.remembered.agreed.contains(&scope_id) {
            return None;
        }
        let link = self
            .channels
            .iter()
            .any(|c| c.id == channel && c.kind == model::ChannelKind::Link);
        Some(Gate {
            scope,
            scope_id,
            channel: channel.to_owned(),
            link,
        })
    }

    /// Once per frame: the demo scenes open their window.
    pub(crate) fn nav_frame(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "demo")]
        self.demo_nav(cx);
        let _ = cx;
    }

    /// Demo screenshots (CI): the rail and invite scenes open what they show once the
    /// account is in.
    #[cfg(feature = "demo")]
    fn demo_nav(&mut self, cx: &mut Context<Self>) {
        use crate::app::Screen;
        use crate::demo::scenes::{current, Scene};
        use std::sync::atomic::{AtomicBool, Ordering};
        static DONE: AtomicBool = AtomicBool::new(false);
        let scene = current();
        if std::env::var_os("INLET_DEMO").is_none()
            || !crate::demo::nav::is_nav_scene(scene)
            || self.account.is_none()
            || self.screen != Screen::Main
            || self.guilds.is_empty()
            || DONE.swap(true, Ordering::Relaxed)
        {
            return;
        }
        match scene {
            Scene::RailFolders => {
                self.sidebar.remembered.open_folders.insert("1");
            }
            Scene::GuildMenu => self.nav.dialog = Some(NavDialog::GuildMenu),
            Scene::InviteAccept => {
                self.nav.dialog = Some(NavDialog::Invite(InviteView {
                    code: "hedgerow".into(),
                    stage: crate::ui::overlays::invite_accept::Stage::Loaded(Box::new(
                        crate::demo::nav::preview("hedgerow"),
                    )),
                }));
            }
            _ => {}
        }
        cx.notify();
    }

    /// Whether a window of this layer is open.
    pub(crate) fn nav_open(&self) -> bool {
        self.nav.dialog.is_some()
    }

    pub(crate) fn nav_show(&mut self, dialog: NavDialog, cx: &mut Context<Self>) {
        self.nav.dialog = Some(dialog);
        cx.notify();
    }

    pub(crate) fn nav_close(&mut self, cx: &mut Context<Self>) {
        if self.nav.dialog.take().is_some() {
            cx.notify();
        }
    }

    /// The text field of the open window, if it has one.
    pub(crate) fn nav_field(&self, cx: &gpui::App) -> Option<FocusHandle> {
        use gpui::Focusable;
        match self.nav.dialog.as_ref()? {
            NavDialog::Folder(f) => Some(f.name.focus_handle(cx)),
            NavDialog::AddServer(a) => Some(a.input.focus_handle(cx)),
            _ => None,
        }
    }

    pub(crate) fn nav_field_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.nav_field(cx).is_some_and(|h| h.is_focused(window))
    }

    /// The accepted content warning: remembered, and the channel opens.
    fn gate_accepted(&mut self, gate: Gate, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.remembered.agreed.insert(&gate.scope_id);
        self.sidebar.remembered.save_in_background(cx);
        self.nav_close(cx);
        self.open_channel_row(gate.channel, window, cx);
    }

    /// The window over the screen, if one is open.
    pub(crate) fn render_nav(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.nav.dialog.as_ref()?;
        let card = match dialog {
            NavDialog::GuildMenu => return Some(self.render_guild_menu(cx)),
            NavDialog::Leave {
                guild,
                name,
                busy,
                error,
            } => {
                let (guild, name, busy, error) =
                    (guild.clone(), name.clone(), *busy, error.clone());
                self.render_leave(guild, name, busy, error, cx)
            }
            NavDialog::Folder(_) => self.render_folder_dialog(cx),
            NavDialog::Invite(_) => self.render_invite(cx),
            NavDialog::AddServer(_) => self.render_add_server(cx),
            NavDialog::Gate(gate) => {
                let gate = gate.clone();
                self.render_gate(gate, cx)
            }
            NavDialog::Soon(soon) => {
                let soon = *soon;
                self.render_soon(soon, cx)
            }
        };
        Some(self.layer_keys(card, cx).into_any_element())
    }

    fn render_soon(&self, soon: Soon, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let (title, body) = match soon {
            Soon::Favorites => (t!("nav-favorites"), t!("nav-soon-favorites")),
            Soon::Discover => (t!("nav-discover"), t!("nav-soon-discover")),
            Soon::CreateServer => (t!("nav-create-server"), t!("nav-soon-create")),
        };
        modal(
            "nav-soon",
            440.,
            [
                title_el(title),
                div()
                    .text_color(theme::text_muted())
                    .child(body)
                    .into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(
                        button("nav-soon-ok", t!("nav-understood"), true)
                            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
                    )
                    .into_any_element(),
            ],
        )
    }

    fn render_gate(&self, gate: Gate, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let body = match (gate.scope, gate.link) {
            (_, true) => t!("nav-gate-link"),
            (GateScope::Guild, _) => t!("nav-gate-guild"),
            (GateScope::Category, _) => t!("nav-gate-category"),
            (GateScope::Channel, _) => t!("nav-gate-channel"),
        };
        let go = gate.clone();
        modal(
            "nav-gate",
            480.,
            [
                title_el(t!("nav-gate-title")),
                div()
                    .text_color(theme::text_muted())
                    .child(body)
                    .into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("nav-gate-cancel", t!("common-cancel"), false)
                            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
                    )
                    .child(
                        button("nav-gate-continue", t!("nav-gate-continue"), true).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.gate_accepted(go.clone(), window, cx)
                            }),
                        ),
                    )
                    .into_any_element(),
            ],
        )
    }

    fn render_leave(
        &self,
        guild: String,
        name: String,
        busy: bool,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let leave = guild.clone();
        modal(
            "nav-leave",
            480.,
            [
                title_el(t!("nav-leave-title", name = name.as_str())),
                div()
                    .text_color(theme::text_muted())
                    .child(t!("nav-leave-body", name = name.as_str()))
                    .into_any_element(),
                div()
                    .children(error.map(|e| {
                        div()
                            .text_size(px(14.))
                            .text_color(theme::danger())
                            .child(e)
                    }))
                    .into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("nav-leave-cancel", t!("common-cancel"), false)
                            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
                    )
                    .child(
                        button(
                            "nav-leave-ok",
                            if busy {
                                t!("nav-leaving")
                            } else {
                                t!("nav-leave")
                            },
                            true,
                        )
                        .bg(theme::danger())
                        .when(busy, |b| b.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.leave_guild_confirmed(leave.clone(), cx)
                        })),
                    )
                    .into_any_element(),
            ],
        )
    }

    /// The server a join opened: select it as soon as the list has it.
    pub(crate) fn nav_on_guilds(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.nav.join_target.clone() else {
            return;
        };
        if self.guilds.iter().any(|g| g.id == target) {
            self.nav.join_target = None;
            self.select_guild(Some(target), cx);
        }
    }

    /// Whether this guild belongs to the account (the menu shows "Leave" only then, and
    /// never to the account the server belongs to, which cannot leave it).
    pub(crate) fn can_leave_guild(&self, guild: &str, cx: &gpui::App) -> bool {
        let (Some(account), Some(id)) = (self.account.as_ref(), GuildId::parse(guild)) else {
            return false;
        };
        let state = account.read(cx);
        let Some(g) = state.guild(id) else {
            return false;
        };
        g.owner_id() != Some(state.me_id().to_string().as_str())
    }
}

/// A window's title line.
pub(crate) fn title_el(text: String) -> AnyElement {
    div()
        .text_size(px(20.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text_strong())
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::models::Ready;
    use serde_json::json;

    fn state(guild_nsfw: bool) -> AccountState {
        let ready: Ready = serde_json::from_value(json!({
            "session_id": "s", "user": {"id": "10", "username": "me"},
            "private_channels": [], "users": [], "read_states": [],
            "user_guild_settings": [], "presences": [],
            "guilds": [{
                "id": "100",
                "properties": {"id": "100", "name": "G", "owner_id": "1", "nsfw": guild_nsfw},
                "roles": [{"id": "100", "name": "@everyone", "permissions": "3072", "position": 0}],
                "channels": [
                    {"id": "300", "type": 4, "name": "mature", "position": 0, "nsfw_override": true},
                    {"id": "301", "type": 0, "name": "in-mature", "parent_id": "300", "position": 1},
                    {"id": "302", "type": 0, "name": "own", "nsfw": true, "position": 2},
                    {"id": "303", "type": 0, "name": "safe", "position": 3, "nsfw_override": false},
                    {"id": "304", "type": 0, "name": "plain", "position": 4}
                ],
                "members": [{"user": {"id": "10", "username": "me"}, "roles": [], "joined_at": "2026-01-01T00:00:00Z"}]
            }]
        }))
        .unwrap();
        AccountState::from_ready_at(&ready, 1)
    }

    #[test]
    fn age_restricted_channels_ask_before_opening() {
        let s = state(false);
        let ch = |id: u64| ChannelId(id);
        // The channel's own setting, its category's, and what the server decided.
        assert_eq!(
            gate_of(&s, ch(302)),
            Some((GateScope::Channel, "302".into()))
        );
        assert_eq!(
            gate_of(&s, ch(301)),
            Some((GateScope::Category, "300".into()))
        );
        assert_eq!(gate_of(&s, ch(304)), None);
        // An explicit "not age-restricted" wins over the server.
        assert_eq!(gate_of(&s, ch(303)), None);
        // A restricted server gates every channel that does not say otherwise.
        let s = state(true);
        assert_eq!(gate_of(&s, ch(304)), Some((GateScope::Guild, "100".into())));
        assert_eq!(gate_of(&s, ch(303)), None);
        assert_eq!(gate_of(&s, ch(999)), None);
    }
}
