// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server menu: under the server name, and on a right-click on a server in the rail.
//! Mark as read, mute, notification settings, hide muted channels, privacy settings,
//! copy the ID, and leave (not for a server the account cannot leave).
//!
//! Left out because Fluxer needs more than Inlet has: "Delete my messages" (it asks for
//! sudo mode, which the protocol crate does not offer), and the entries of the guild
//! settings, inviting and creating channels (later tasks).

use gpui::{
    div, prelude::*, px, AnyElement, ClipboardItem, Context, Div, FontWeight, Pixels, Point,
    SharedString, Stateful,
};
use inlet_state::GuildId;

use super::nav_dialog::NavDialog;
use crate::app::Inlet;
use crate::assets::Icon;
use crate::theme;
use crate::ui::notifications::settings::{Change, Target};
use crate::ui::notifications::unread::unread_targets;
use crate::ui::rail::RAIL_WIDTH;
use crate::ui::HEADER_HEIGHT;

/// Width of the menu.
const WIDTH: f32 = 296.;

/// Where the menu stands: under the server name, or at the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Header,
    At(Point<Pixels>),
}

fn row(id: &'static str, icon: Icon, label: String, danger: bool) -> Stateful<Div> {
    let color = if danger {
        theme::danger()
    } else {
        theme::text()
    };
    div()
        .id(SharedString::from(id))
        .h(px(32.))
        .px(px(8.))
        .rounded(px(6.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .hover(|s| s.bg(theme::floating_hover()))
        .child(icon.el(px(18.), color))
        .child(div().flex_1().min_w_0().child(label))
}

impl Inlet {
    /// Opens the menu of a server.
    pub(crate) fn open_guild_menu(
        &mut self,
        guild: String,
        anchor: Anchor,
        cx: &mut Context<Self>,
    ) {
        self.nav_show(NavDialog::GuildMenu { guild, anchor }, cx);
    }

    fn guild_muted(&self, guild: &str, cx: &gpui::App) -> bool {
        let (Some(account), Some(id)) = (self.account.as_ref(), GuildId::parse(guild)) else {
            return false;
        };
        let state = account.read(cx);
        state.guild_settings().guild_muted(Some(id), state.now_ms())
    }

    fn guild_hides_muted(&self, guild: &str, cx: &gpui::App) -> bool {
        let (Some(account), Some(id)) = (self.account.as_ref(), GuildId::parse(guild)) else {
            return false;
        };
        account
            .read(cx)
            .guild_settings()
            .get(Some(id))
            .hide_muted_channels
    }

    /// The menu of a channel row: add it to the favourites or take it out, copy its ID.
    pub(super) fn render_channel_menu(
        &mut self,
        channel: String,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let favorite = self.channel_favorite_state(&channel);
        let (fav_id, copy_id) = (channel.clone(), channel);
        let mut card = div()
            .id("channel-menu")
            .occlude()
            .absolute()
            .left(at.x.min(px(900.)))
            .top(at.y.min(px(700.)))
            .w(px(240.))
            .p(px(8.))
            .rounded(px(8.))
            .bg(theme::floating_bg())
            .shadow(vec![gpui::BoxShadow {
                color: theme::floating_shadow(),
                offset: gpui::point(px(0.), px(4.)),
                blur_radius: px(16.),
                spread_radius: px(0.),
                inset: false,
            }])
            .flex()
            .flex_col()
            .gap(px(2.));
        if let Some(on) = favorite {
            card = card.child(
                row(
                    "channel-menu-favorite",
                    Icon::Star,
                    if on {
                        t!("nav-menu-unfavorite")
                    } else {
                        t!("nav-menu-favorite")
                    },
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.nav_close(cx);
                    this.toggle_favorite(&fav_id, cx);
                })),
            );
        }
        card = card.child(
            row(
                "channel-menu-copy",
                Icon::Copy,
                t!("nav-menu-copy-channel-id"),
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_id.clone()));
                this.nav_close(cx);
            })),
        );
        self.layer_keys(
            div()
                .id("channel-menu-catcher")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx)))
                .child(card),
            cx,
        )
        .into_any_element()
    }

    /// "Mark as read" for every channel of one server (not only the open one).
    fn mark_guild_read_of(&mut self, guild: &str, cx: &mut Context<Self>) {
        let (Some(account), Some(id)) = (self.account.clone(), GuildId::parse(guild)) else {
            return;
        };
        let targets = unread_targets(account.read(cx), Some(id));
        self.mark_listed_read(targets, cx);
    }

    /// The menu: a click catcher over the window and the card. Every entry closes it
    /// first, so the window it opens is the only layer left.
    pub(super) fn render_guild_menu(
        &mut self,
        guild: String,
        anchor: Anchor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = self.guild_muted(&guild, cx);
        let hides_muted = self.guild_hides_muted(&guild, cx);
        let can_leave = self.can_leave_guild(&guild, cx);
        let (read_id, mute_id, hide_id, notify_id, copy_id, leave_id) = (
            guild.clone(),
            guild.clone(),
            guild.clone(),
            guild.clone(),
            guild.clone(),
            guild.clone(),
        );
        let (left, top) = match anchor {
            Anchor::Header => (px(RAIL_WIDTH + 8.), px(HEADER_HEIGHT - 4.)),
            // Kept inside the window: the menu is about 290 px high.
            Anchor::At(p) => (p.x, p.y.min(px(540.))),
        };
        let card = div()
            .id("guild-menu")
            .debug_selector(|| "guild-menu".into())
            .occlude()
            .absolute()
            .left(left)
            .top(top)
            .w(px(WIDTH))
            .p(px(8.))
            .rounded(px(8.))
            .bg(theme::floating_bg())
            .shadow(vec![gpui::BoxShadow {
                color: theme::floating_shadow(),
                offset: gpui::point(px(0.), px(4.)),
                blur_radius: px(16.),
                spread_radius: px(0.),
                inset: false,
            }])
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                row("guild-menu-read", Icon::Check, t!("nav-menu-read"), false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.nav_close(cx);
                        this.mark_guild_read_of(&read_id, cx);
                    }),
                ),
            )
            .child(
                row(
                    "guild-menu-mute",
                    if muted { Icon::Bell } else { Icon::BellSlash },
                    if muted {
                        t!("nav-menu-unmute")
                    } else {
                        t!("nav-menu-mute")
                    },
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.nav_close(cx);
                    let change = if muted {
                        Change::Unmute
                    } else {
                        Change::Mute { for_ms: None }
                    };
                    this.notification_change(Target::Guild(Some(mute_id.clone())), change, cx);
                })),
            )
            .child(
                row(
                    "guild-menu-notifications",
                    Icon::Bell,
                    t!("nav-menu-notifications"),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.nav_close(cx);
                    this.open_guild_notification_settings(notify_id.clone(), cx);
                })),
            )
            .child(
                row(
                    "guild-menu-hide-muted",
                    if hides_muted {
                        Icon::CheckSquare
                    } else {
                        Icon::Hash
                    },
                    t!("nav-menu-hide-muted"),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.nav_close(cx);
                    this.notification_change(
                        Target::Guild(Some(hide_id.clone())),
                        Change::HideMuted(!hides_muted),
                        cx,
                    );
                })),
            )
            .child(
                row(
                    "guild-menu-privacy",
                    Icon::Shield,
                    t!("nav-menu-privacy"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.nav_close(cx);
                    this.open_settings(Some("privacy"), cx);
                })),
            )
            .child(
                row("guild-menu-copy", Icon::Copy, t!("nav-menu-copy-id"), false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_id.clone()));
                        this.nav_close(cx);
                    }),
                ),
            )
            .when(can_leave, |d| {
                d.child(div().h(px(1.)).mx(px(4.)).my(px(4.)).bg(theme::divider()))
                    .child(
                        row(
                            "guild-menu-leave",
                            Icon::SignOut,
                            t!("nav-menu-leave"),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let name = this
                                .guilds
                                .iter()
                                .find(|g| g.id == leave_id)
                                .map(|g| g.name.clone())
                                .unwrap_or_default();
                            this.nav_show(
                                NavDialog::Leave {
                                    guild: leave_id.clone(),
                                    name,
                                    busy: false,
                                    error: None,
                                },
                                cx,
                            );
                        })),
                    )
            });
        // The catcher: a click anywhere else closes the menu.
        self.layer_keys(
            div()
                .id("guild-menu-catcher")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx)))
                .child(card),
            cx,
        )
        .into_any_element()
    }
}
