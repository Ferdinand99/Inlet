// SPDX-License-Identifier: AGPL-3.0-or-later
//! The windows of the rail and the channel list: the server menu, leaving a server, a
//! folder's settings, an invite, adding a server, the content warning before an
//! age-restricted channel, and the "coming soon" window of the entries that need
//! features Inlet does not have yet.
//!
//! They are one layer: only one is open at a time, it is the topmost layer while it is
//! open, and Escape closes it (see `desktop::Layer::Nav`).

use gpui::{div, prelude::*, px, AnyElement, Context, FocusHandle, FontWeight, Window};
use inlet_state::GuildId;

use super::gate::Gate;
use super::modal::modal;
use crate::app::Inlet;
use crate::theme;
use crate::ui::overlays::invite_accept::{AddServer, InviteView};
use crate::ui::rail::folder::FolderEdit;
use crate::ui::widgets::button;

/// What the "coming soon" window is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Soon {
    Discover,
    CreateServer,
    /// The members page of the server (Fluxer's `GuildMembersPage`).
    Members,
}

/// Which window is open.
pub enum NavDialog {
    /// The menu of a server: under its name in the channel list, or at the pointer.
    GuildMenu {
        guild: String,
        anchor: super::guild_menu::Anchor,
    },
    /// The menu of a channel row (favourites, copy ID).
    ChannelMenu {
        channel: String,
        at: gpui::Point<gpui::Pixels>,
    },
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
    /// What the signed-in instance's configuration says about the rail.
    pub instance: crate::backend::settings::guilds::InstanceNav,
}

impl Inlet {
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
                if let Some(key) = self.nav_account_key(cx) {
                    if let Some(mem) = self.sidebar.remembered.of_mut(&key) {
                        mem.open_folders.insert("1");
                    }
                }
            }
            Scene::GuildMenu => {
                self.nav.dialog = Some(NavDialog::GuildMenu {
                    guild: crate::demo::snowflake("g1"),
                    anchor: super::guild_menu::Anchor::Header,
                })
            }
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

    /// The window over the screen, if one is open.
    pub(crate) fn render_nav(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.nav.dialog.as_ref()?;
        let card = match dialog {
            NavDialog::GuildMenu { guild, anchor } => {
                let (guild, anchor) = (guild.clone(), *anchor);
                return Some(self.render_guild_menu(guild, anchor, cx));
            }
            NavDialog::ChannelMenu { channel, at } => {
                let (channel, at) = (channel.clone(), *at);
                return Some(self.render_channel_menu(channel, at, cx));
            }
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
            Soon::Discover => (t!("nav-discover"), t!("nav-soon-discover")),
            Soon::CreateServer => (t!("nav-create-server"), t!("nav-soon-create")),
            Soon::Members => (t!("sidebar-members"), t!("nav-soon-members")),
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
        // Not the account's own server, and not the instance's stock community.
        g.owner_id() != Some(state.me_id().to_string().as_str())
            && !self.nav.instance.is_stock(guild)
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

impl Inlet {
    /// The key of the shown account in what this computer remembers: `<origin>#<user>`.
    pub(crate) fn nav_account_key(&self, cx: &gpui::App) -> Option<String> {
        let state = self.account.as_ref()?.read(cx);
        (!self.instance.is_empty()).then(|| format!("{}#{}", self.instance, state.me_id()))
    }
}
