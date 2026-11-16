// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server menu under the guild name: mark the server as read, mute it, its
//! notification settings, copy its ID and leave it.

use gpui::{
    div, prelude::*, px, AnyElement, ClipboardItem, Context, Div, FontWeight, SharedString,
    Stateful,
};
use inlet_state::GuildId;

use super::nav_dialog::NavDialog;
use crate::app::Inlet;
use crate::assets::Icon;
use crate::theme;
use crate::ui::notifications::settings::{Change, Target};
use crate::ui::rail::RAIL_WIDTH;
use crate::ui::HEADER_HEIGHT;

/// Width of the menu.
const WIDTH: f32 = 296.;

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
    /// Whether the open server is muted, as the account state has it now.
    fn open_guild_muted(&self, cx: &gpui::App) -> bool {
        let (Some(account), Some(guild)) = (
            self.account.as_ref(),
            self.selected_guild.as_deref().and_then(GuildId::parse),
        ) else {
            return false;
        };
        let state = account.read(cx);
        state
            .guild_settings()
            .guild_muted(Some(guild), state.now_ms())
    }

    /// The menu: a click catcher over the window and the card under the header. Every
    /// entry closes it first, so the window it opens is the only layer left.
    pub(super) fn render_guild_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(guild) = self.selected_guild.clone() else {
            self.nav.dialog = None;
            return div().into_any_element();
        };
        let muted = self.open_guild_muted(cx);
        let can_leave = self.can_leave_guild(&guild, cx);
        let (mute_id, copy_id, leave_id) = (guild.clone(), guild.clone(), guild.clone());
        let card = div()
            .id("guild-menu")
            .debug_selector(|| "guild-menu".into())
            .occlude()
            .absolute()
            .left(px(RAIL_WIDTH + 8.))
            .top(px(HEADER_HEIGHT - 4.))
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
                        this.mark_guild_read(cx);
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
                .on_click(cx.listener(|this, _, _, cx| {
                    this.nav_close(cx);
                    this.open_notification_settings_for(None, cx);
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
