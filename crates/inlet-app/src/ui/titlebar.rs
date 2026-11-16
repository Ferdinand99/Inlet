// SPDX-License-Identifier: AGPL-3.0-or-later
//! Inlet's own title bar, as Fluxer's desktop app has one: a bar of 32 px with the
//! name and mark on the left and, on Windows and Linux, minimize, maximize and close on
//! the right. On macOS the system's traffic lights sit over the bar's left end and the
//! name follows them. "Use native title bar" in the Desktop settings turns it off
//! (Windows and Linux; it takes a restart).
//!
//! - **Windows:** gpui hides the system's caption (`appears_transparent`) and asks the
//!   bar where the caption and the buttons are, so dragging, snapping, the snap layouts
//!   of the maximize button and double-click to maximize are the system's own. Only
//!   minimize is Inlet's, so "minimize to tray" can take it.
//! - **macOS:** the bar moves the window itself (`start_window_move`) and passes a
//!   double click on (`titlebar_double_click`), so the system's setting decides what it
//!   does.
//! - **Linux:** the window has no frame from the system (client side decorations):
//!   Inlet draws the buttons and a thin handle round the window for resizing.
//!
//! The drag area never contains the buttons: Windows answers "which part of the window
//! is this" with the first area it finds under the pointer.

use gpui::{
    div, prelude::*, px, svg, AnyElement, Context, CursorStyle, Decorations, Div, Hsla,
    MouseButton, ResizeEdge, Stateful, Window, WindowControlArea,
};

use crate::app::Inlet;
use crate::desktop::{self, Desktop};
use crate::theme;

/// The height of the bar.
pub const HEIGHT: f32 = 32.;
/// The width of a window button on Windows and Linux.
const BUTTON_WIDTH: f32 = 46.;
/// The room the macOS traffic lights take on the left.
const TRAFFIC_LIGHTS: f32 = 80.;
/// The thickness of the handles that resize a window without a frame (Linux).
const HANDLE: f32 = 5.;

const MARK: &[u8] = include_bytes!("../../../../assets/brand/inlet-mark-mono.svg");
const MINIMIZE: &[u8] = include_bytes!("../../assets/icons/minus.svg");
const MAXIMIZE: &[u8] = include_bytes!("../../assets/icons/square.svg");
const RESTORE: &[u8] = include_bytes!("../../assets/icons/copy.svg");
const CLOSE: &[u8] = include_bytes!("../../assets/icons/x.svg");

const MAC: bool = cfg!(target_os = "macos");
const WINDOWS: bool = cfg!(windows);

fn glyph(data: &'static [u8], size: f32, color: Hsla) -> impl IntoElement {
    svg()
        .data(data)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A window button: a picture on a square that lights up under the pointer.
fn caption_button(id: &'static str, picture: &'static [u8], red: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(BUTTON_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_default()
        .text_color(theme::text_strong())
        .hover(move |s| {
            if red {
                s.bg(theme::danger())
            } else {
                s.bg(theme::hover_bg())
            }
        })
        .child(glyph(picture, 12., theme::text_strong()))
}

impl Inlet {
    /// Whether Inlet draws the title bar this run.
    pub(crate) fn has_own_title_bar(&self, cx: &Context<Self>) -> bool {
        cx.try_global::<Desktop>()
            .is_some_and(|d| d.custom_titlebar)
    }

    /// The bar, when Inlet draws its own.
    pub(crate) fn render_titlebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.has_own_title_bar(cx) {
            return None;
        }
        let maximized = window.is_maximized();
        let brand = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .child(glyph(MARK, 16., theme::text_strong()))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme::text_strong())
                    .child(t!("titlebar-name")),
            );

        // The part that moves the window: the name and the empty space.
        let mut drag = div()
            .id("titlebar-drag")
            .h_full()
            .flex_1()
            .flex()
            .flex_row()
            .items_center();
        if MAC {
            drag = drag.child(div().w(px(TRAFFIC_LIGHTS)).h_full().flex_none());
        }
        drag = drag.child(brand);
        if WINDOWS {
            // The system moves the window and maximizes it on a double click.
            drag = drag.window_control_area(WindowControlArea::Drag);
        } else {
            drag = drag.on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, event: &gpui::MouseDownEvent, window, _| {
                    if event.click_count >= 2 {
                        if MAC {
                            window.titlebar_double_click();
                        } else {
                            window.zoom_window();
                        }
                    } else {
                        window.start_window_move();
                    }
                }),
            );
        }

        let mut bar = div()
            .id("titlebar")
            .w_full()
            .h(px(HEIGHT))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .bg(theme::bg_app())
            .child(drag);
        if !MAC {
            // Minimize is always Inlet's own, so "minimize to tray" can take it.
            bar = bar.child(
                caption_button("titlebar-minimize", MINIMIZE, false).on_click(
                    cx.listener(|_, _, window, cx| desktop::minimize_requested(window, cx)),
                ),
            );
            let max = caption_button(
                "titlebar-maximize",
                if maximized { RESTORE } else { MAXIMIZE },
                false,
            );
            let close = caption_button("titlebar-close", CLOSE, true);
            if WINDOWS {
                bar = bar
                    .child(max.window_control_area(WindowControlArea::Max))
                    .child(close.window_control_area(WindowControlArea::Close));
            } else {
                bar = bar
                    .child(max.on_click(cx.listener(|_, _, window, _| window.zoom_window())))
                    .child(close.on_click(cx.listener(|_, _, window, cx| {
                        desktop::close_requested(window, cx);
                    })));
            }
        }
        Some(bar.into_any_element())
    }

    /// The handles round a window the system gives no frame (Linux with client side
    /// decorations), so it can be resized from its edges. Empty otherwise, and while the
    /// window is maximized.
    pub(crate) fn render_resize_handles(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        if !self.has_own_title_bar(cx)
            || !matches!(window.window_decorations(), Decorations::Client { .. })
            || window.is_maximized()
            || window.is_fullscreen()
        {
            return Vec::new();
        }
        resize_handles()
    }
}

/// The eight strips: four edges and four corners.
fn resize_handles() -> Vec<AnyElement> {
    fn edge(
        id: &'static str,
        which: ResizeEdge,
        cursor: CursorStyle,
        place: impl FnOnce(Div) -> Div,
    ) -> AnyElement {
        place(div().absolute().cursor(cursor))
            .id(id)
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                window.start_window_resize(which);
            })
            .into_any_element()
    }
    let corner = px(HANDLE * 2.);
    vec![
        edge(
            "resize-top",
            ResizeEdge::Top,
            CursorStyle::ResizeUpDown,
            |d| d.top_0().left(corner).right(corner).h(px(HANDLE)),
        ),
        edge(
            "resize-bottom",
            ResizeEdge::Bottom,
            CursorStyle::ResizeUpDown,
            |d| d.bottom_0().left(corner).right(corner).h(px(HANDLE)),
        ),
        edge(
            "resize-left",
            ResizeEdge::Left,
            CursorStyle::ResizeLeftRight,
            |d| d.left_0().top(corner).bottom(corner).w(px(HANDLE)),
        ),
        edge(
            "resize-right",
            ResizeEdge::Right,
            CursorStyle::ResizeLeftRight,
            |d| d.right_0().top(corner).bottom(corner).w(px(HANDLE)),
        ),
        edge(
            "resize-top-left",
            ResizeEdge::TopLeft,
            CursorStyle::ResizeUpLeftDownRight,
            |d| d.top_0().left_0().size(px(HANDLE * 2.)),
        ),
        edge(
            "resize-top-right",
            ResizeEdge::TopRight,
            CursorStyle::ResizeUpRightDownLeft,
            |d| d.top_0().right_0().size(px(HANDLE * 2.)),
        ),
        edge(
            "resize-bottom-left",
            ResizeEdge::BottomLeft,
            CursorStyle::ResizeUpRightDownLeft,
            |d| d.bottom_0().left_0().size(px(HANDLE * 2.)),
        ),
        edge(
            "resize-bottom-right",
            ResizeEdge::BottomRight,
            CursorStyle::ResizeUpLeftDownRight,
            |d| d.bottom_0().right_0().size(px(HANDLE * 2.)),
        ),
    ]
}
