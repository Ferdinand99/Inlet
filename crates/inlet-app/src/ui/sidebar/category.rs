// SPDX-License-Identifier: AGPL-3.0-or-later
//! A category row of the channel list: its name, the chevron that folds it and the bell.
//! A folded category is remembered between runs (see `remembered`).

use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, SharedString,
};

use crate::app::Inlet;
use crate::assets::Icon;
use crate::model::Channel;
use crate::theme;

impl Inlet {
    /// Whether a category is folded away.
    pub(crate) fn category_folded(&self, id: &str) -> bool {
        self.sidebar.remembered.collapsed.contains(id)
    }

    /// Folds a category away, or opens it again, and remembers the choice.
    pub(crate) fn toggle_category(&mut self, id: &str, cx: &mut Context<Self>) {
        let remembered = &mut self.sidebar.remembered;
        if !remembered.collapsed.remove(id) {
            remembered.collapsed.insert(id);
        }
        remembered.save_in_background(cx);
        cx.notify();
    }

    /// One category row. `spaced` leaves room above it (it follows channels).
    pub(super) fn category_row(
        &self,
        c: &Channel,
        spaced: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.category_folded(&c.id);
        let cat = c.id.clone();
        // A muted category is drawn at half strength, as a muted channel is.
        let dimmed = self.badge(&c.id).dimmed;
        let bell = if dimmed { Icon::BellSlash } else { Icon::Bell };
        let group = SharedString::from(format!("cat-{}", c.id));
        let (for_menu, for_bell) = (c.id.clone(), c.id.clone());
        // The fold row and the bell are siblings, so a click on the bell never folds the
        // category. Right-click on the row opens the same window.
        div()
            .id(SharedString::from(format!("cat-frame-{}", c.id)))
            .group(group.clone())
            .relative()
            .flex_none()
            .when(spaced, |d| d.mt_4())
            .when(dimmed, |d| d.opacity(0.5))
            .child(
                div()
                    .id(SharedString::from(format!("ch-{}", c.id)))
                    .h(px(32.))
                    .ml_2()
                    .px_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_channel())
                    .hover(|s| s.text_color(theme::text_strong()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            // Room for the bell, so it never covers the end of a long name.
                            .mr(px(22.))
                            .child(c.name.clone()),
                    )
                    .child(
                        if folded {
                            Icon::CaretRight
                        } else {
                            Icon::CaretDown
                        }
                        .el(px(12.), theme::text_channel()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_category(&cat, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.open_notification_settings_for(Some(for_menu.clone()), cx)
                        }),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("cat-bell-{}", c.id)))
                    .absolute()
                    .right(px(26.))
                    .top(px(6.))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .hover(|s| s.bg(theme::hover_bg()))
                    .child(bell.el(px(14.), theme::text_muted()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_notification_settings_for(Some(for_bell.clone()), cx)
                    })),
            )
            .into_any_element()
    }
}
