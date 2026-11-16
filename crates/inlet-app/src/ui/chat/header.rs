// SPDX-License-Identifier: AGPL-3.0-or-later
//! The channel header: icon, name, topic, the header buttons and the status line. In a
//! short header (the search column open at 1280 px) the topic is cut first, then the
//! channel name; the status line and the buttons keep their size.

use gpui::{div, prelude::*, px, Context, FontWeight};

use super::header_buttons;
use crate::app::Inlet;
use crate::theme;
use crate::ui::sidebar::channel_list::channel_icon;
use crate::ui::HEADER_HEIGHT;

/// The narrowest the header squeezes the channel's icon and name to (the icon and a few
/// letters, then an ellipsis). There is no upper limit, as in Fluxer.
const TITLE_MIN_WIDTH: f32 = 96.;

impl Inlet {
    pub(crate) fn render_chat_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self
            .selected_channel
            .as_ref()
            .and_then(|id| self.channels.iter().find(|c| &c.id == id));
        let title = open.map(|c| c.name.clone()).unwrap_or_default();
        let title_icon = open.map(|c| channel_icon(c.kind));
        let topic = self
            .selected_channel
            .as_ref()
            .and_then(|id| self.channels.iter().find(|c| &c.id == id))
            .and_then(|c| c.topic.clone());
        let buttons = header_buttons::render(self, cx);
        div()
            .debug_selector(|| "chat-header".into())
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme::divider())
            // The channel's icon and name come first and keep their width; when the
            // header is short, the topic is cut first (it takes only what is left),
            // then the name, down to a minimum.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_shrink(1.)
                    .min_w(px(TITLE_MIN_WIDTH))
                    .items_center()
                    .gap_2()
                    .children(title_icon.map(|i| i.el(px(24.), theme::text_muted())))
                    .child(
                        div()
                            .debug_selector(|| "chat-header-title".into())
                            .min_w_0()
                            .flex_shrink(1.)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::MEDIUM)
                            .line_height(px(24.))
                            .text_color(theme::text_strong())
                            .child(title),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .children(topic.map(|t| {
                        div()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(20.))
                                    .flex_none()
                                    .bg(theme::accent_border()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(13.))
                                    .line_height(px(18.))
                                    .text_color(theme::text_tertiary())
                                    .child(t),
                            )
                    })),
            )
            // Nothing yet: each button's domain fills its slot.
            .children(buttons)
            .child(
                div()
                    .debug_selector(|| "chat-header-status".into())
                    .flex_none()
                    .whitespace_nowrap()
                    .pl_3()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(self.chat.status.clone()),
            )
    }
}
