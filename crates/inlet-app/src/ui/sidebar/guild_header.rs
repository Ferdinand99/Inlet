// SPDX-License-Identifier: AGPL-3.0-or-later
//! The header above the channel list: the guild's banner when it has one, with the
//! guild name over it, or just the name. A click opens the server menu. In the direct
//! messages the header is the quick switcher bar (see `dm_list`).

use gpui::{div, prelude::*, px, Context, FontWeight};

use crate::app::Inlet;
use crate::assets::Icon;
use crate::theme;
use crate::ui::overlays::guild_menu::Anchor;
use crate::ui::HEADER_HEIGHT;

impl Inlet {
    pub(super) fn render_guild_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The DM list's header is the quick switcher bar (see `dm_list`).
        if self.selected_guild.is_none() {
            return div().flex_none().child(self.render_dm_header(cx));
        }
        let title = match &self.selected_guild {
            None => t!("sidebar-direct-messages"),
            Some(id) => self
                .guilds
                .iter()
                .find(|g| &g.id == id)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
        };
        let banner = self.guild_banner(super::SIDEBAR_WIDTH, cx);
        let with_banner = banner.is_some();
        // The name, a button for the server menu. Over a banner it lies on the picture,
        // on a shade that fades out downwards; the text stays white.
        // Deviation: no text shadow (gpui has none); the shade keeps the name readable.
        let name = div()
            .id("guild-header")
            .debug_selector(|| "guild-header".into())
            .h(px(HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text_strong())
            .when(!with_banner, |d| {
                d.border_b_1()
                    .border_color(theme::divider())
                    .hover(|s| s.bg(theme::hover_bg()))
            })
            .when(with_banner, |d| {
                d.absolute().top_0().left_0().bg(gpui::linear_gradient(
                    180.,
                    gpui::linear_color_stop(gpui::black().opacity(0.45), 0.),
                    gpui::linear_color_stop(gpui::black().opacity(0.), 1.),
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(title),
            )
            .child(Icon::CaretDown.el(px(16.), theme::text_strong()))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(guild) = this.selected_guild.clone() {
                    this.open_guild_menu(guild, Anchor::Header, cx);
                }
            }));
        match banner {
            // The banner is 16:9 at the sidebar's width, and at most 30 % of the window.
            Some(banner) => div()
                .relative()
                .flex_none()
                .w_full()
                .h(px(crate::ui::media::avatar::BANNER_HEIGHT))
                .max_h(gpui::relative(0.3))
                .overflow_hidden()
                .border_b_1()
                .border_color(theme::divider())
                .child(banner)
                .child(name),
            None => div().flex_none().flex().flex_col().child(name),
        }
    }
}
