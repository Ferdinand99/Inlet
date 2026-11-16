// SPDX-License-Identifier: AGPL-3.0-or-later
//! The list of keyboard shortcuts (Ctrl+/, Cmd+/ on macOS): a window over the chat with
//! every shortcut in its group and the keys as this platform writes them.
//!
//! It is a layer (`desktop::Layer::KeybindHelp`): it has the keyboard while it is the
//! topmost one, Escape and Ctrl+/ close it, and the shortcuts that act on the chat do
//! nothing behind it. The rows come from `app::keymap::rows`, the same list the settings
//! tab shows ([`list`] draws both).

use gpui::{div, prelude::*, px, rgba, AnyElement, Context, FontWeight};

use crate::app::keymap::{self, Row};
use crate::app::Inlet;
use crate::desktop::Layer;
use crate::theme;
use crate::ui::widgets::button;

/// Width of the window.
pub const WIDTH: f32 = 560.;
/// Tallest the window gets; a shorter application window makes it shorter, and the
/// list scrolls.
pub const MAX_HEIGHT: f32 = 640.;

/// The key caps of one shortcut, and after "or" those of its second key.
fn caps(keys: Vec<String>, also: Option<Vec<String>>) -> AnyElement {
    let one = |keys: Vec<String>| {
        div()
            .flex_none()
            .flex()
            .flex_row()
            .gap(px(4.))
            .children(keys.into_iter().map(|key| {
                div()
                    .min_w(px(24.))
                    .h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(theme::bg_app())
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_secondary())
                    .child(key)
            }))
    };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(one(keys))
        .children(also.map(|also| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(t!("keys-or")),
                )
                .child(one(also))
        }))
        .into_any_element()
}

/// The shortcuts in `rows`, one heading per group and one line per shortcut, to put in a
/// column.
pub fn list(rows: Vec<Row>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for group in keymap::Group::ALL {
        let mut in_group = rows.iter().filter(|r| r.group == group).peekable();
        if in_group.peek().is_none() {
            continue;
        }
        out.push(
            div()
                .pt_4()
                .pb_1()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_muted())
                .child(group.title())
                .into_any_element(),
        );
        for row in in_group {
            out.push(
                div()
                    .py(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme::text_strong())
                            .child(row.label.clone()),
                    )
                    .child(caps(row.keys.clone(), row.also.clone()))
                    .into_any_element(),
            );
        }
    }
    out
}

impl Inlet {
    /// The window with the shortcuts, over everything below the prompts.
    pub(crate) fn render_keybind_help(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = div()
            .id("keybind-help")
            .debug_selector(|| "keybind-help".into())
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .p(px(24.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0));
        // The keyboard goes to the window while it is the topmost layer; Escape closes it.
        let backdrop = if self.is_top_layer(Layer::KeybindHelp) {
            self.layer_keys(backdrop, cx)
        } else {
            backdrop
        };
        backdrop
            .child(
                div()
                    .w(px(WIDTH))
                    .max_h(px(MAX_HEIGHT))
                    .min_h_0()
                    .p_6()
                    .rounded_lg()
                    .bg(theme::bg_chat())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_strong())
                                    .child(t!("keys-title")),
                            )
                            .child(
                                button("keybind-help-close", t!("keys-close"), false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.overlays.keybind_help = false;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("keybind-help-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(list(keymap::rows())),
                    ),
            )
            .into_any_element()
    }
}
