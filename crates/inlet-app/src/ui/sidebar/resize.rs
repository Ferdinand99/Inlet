// SPDX-License-Identifier: AGPL-3.0-or-later
//! The handle between the channel list and the chat that sets the list's width.
//!
//! Fluxer's `SidebarResizeHandle`: a 6 px strip over the border, dragged with the
//! pointer; the width stays between 200 and 480 px and is 320 px until it is set; a
//! double click puts the default back. The width is the same for every account and is
//! kept on this computer (see `remembered`). The handle lies over the border, so the
//! layout does not move when it is there.

use std::time::Duration;

use gpui::{div, prelude::*, px, ClickEvent, Context, DragMoveEvent, EmptyView, MouseButton};

use crate::app::Inlet;
use crate::theme;
use crate::ui::rail::RAIL_WIDTH;

/// The narrowest list.
pub const MIN_WIDTH: u32 = 200;
/// The widest list.
pub const MAX_WIDTH: u32 = 480;
/// The width before it is set.
pub const DEFAULT_WIDTH: u32 = 320;
/// The handle's width (Fluxer: 0.375 rem).
const HANDLE_WIDTH: f32 = 6.;
/// The height of the user panel under the list, which the handle does not cover.
const USER_PANEL_HEIGHT: f32 = 58.;
/// How long the width stays put before it is written (the drag changes it all the time).
const SAVE_AFTER: Duration = Duration::from_millis(500);

/// The width kept between the least and the most.
pub fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The list's width when the pointer is at `x` (window pixels): the list starts where
/// the rail ends.
pub fn width_at(x: f32) -> u32 {
    clamp_width((x - RAIL_WIDTH).round().max(0.) as u32)
}

/// What is dragged: the handle. The drag shows nothing of its own.
#[derive(Clone)]
pub struct ResizeDrag;

impl Inlet {
    /// The channel list's width now.
    pub(crate) fn sidebar_width(&self) -> f32 {
        self.sidebar.width as f32
    }

    /// Sets the width and writes it once it has stayed put.
    pub(crate) fn set_sidebar_width(&mut self, width: u32, cx: &mut Context<Self>) {
        let width = clamp_width(width);
        if width == self.sidebar.width {
            return;
        }
        self.sidebar.width = width;
        self.sidebar.remembered.set_sidebar_width(width);
        self.sidebar.width_saver = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                this.sidebar.remembered.save_in_background(cx)
            });
        }));
        cx.notify();
    }

    /// The strip over the border between the list and the chat.
    pub(crate) fn render_resize_handle(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let left = RAIL_WIDTH + self.sidebar_width() - HANDLE_WIDTH / 2.;
        div()
            .id("sidebar-resize")
            .debug_selector(|| "sidebar-resize".into())
            .absolute()
            .top_0()
            .bottom(px(USER_PANEL_HEIGHT))
            .left(px(left))
            .w(px(HANDLE_WIDTH))
            .cursor_col_resize()
            .flex()
            .justify_center()
            .on_drag(ResizeDrag, |_, _, _, cx| cx.new(|_| EmptyView))
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.set_sidebar_width(DEFAULT_WIDTH, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h_full()
                    .w(px(2.))
                    .rounded_full()
                    .hover(|s| s.bg(theme::divider())),
            )
    }

    /// The pointer moves while the handle is dragged.
    pub(crate) fn resize_to(&mut self, event: &DragMoveEvent<ResizeDrag>, cx: &mut Context<Self>) {
        self.set_sidebar_width(width_at(f32::from(event.event.position.x)), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_width_stays_between_200_and_480() {
        assert_eq!(clamp_width(0), 200);
        assert_eq!(clamp_width(199), 200);
        assert_eq!(clamp_width(320), 320);
        assert_eq!(clamp_width(480), 480);
        assert_eq!(clamp_width(5_000), 480);
        // The pointer's place: the list starts where the 72 px rail ends.
        assert_eq!(width_at(72. + 300.), 300);
        assert_eq!(width_at(10.), 200);
        assert_eq!(width_at(2_000.), 480);
    }
}
