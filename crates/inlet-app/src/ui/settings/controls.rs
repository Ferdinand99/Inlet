// SPDX-License-Identifier: AGPL-3.0-or-later
//! The controls the settings tabs share: a page, a choice row, a theme card, and the
//! row that holds a value between a minus and a plus button. The check box row, the
//! heading and the note are the Desktop tab's (`tabs::desktop`).

use gpui::{div, prelude::*, px, Div, ElementId, FontWeight, SharedString, Stateful};

use crate::theme::{self, Pref, Theme};

/// The widest a tab's content gets.
pub const PAGE_WIDTH: f32 = 640.;

/// A tab's page: its title, and a column to put the rows in.
pub fn page(title: String) -> Div {
    div().flex().flex_col().max_w(px(PAGE_WIDTH)).pb_8().child(
        div()
            .text_size(px(20.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme::text_strong())
            .child(title),
    )
}

/// A row with a round choice mark: one of several that exclude each other.
pub fn radio(
    id: &'static str,
    label: String,
    hint: Option<String>,
    selected: bool,
) -> Stateful<Div> {
    let mark = div()
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_2()
        .border_color(if selected {
            theme::brand()
        } else {
            theme::text_muted()
        })
        .when(selected, |d| {
            d.child(div().size(px(10.)).rounded_full().bg(theme::brand()))
        });
    let text = div()
        .flex()
        .flex_col()
        .child(div().text_color(theme::text_strong()).child(label))
        .children(hint.map(|h| {
            div()
                .text_size(px(13.))
                .text_color(theme::text_muted())
                .child(h)
        }));
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .py_2()
        .cursor_pointer()
        .child(mark)
        .child(text)
}

/// A small picture of a theme: the shell on the left, the chat with two lines of text
/// and the brand colour. "System" shows dark and light side by side.
fn theme_picture(pref: Pref) -> Div {
    let one = |theme: Theme| {
        let p = theme::preview(theme);
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(div().w(px(22.)).h_full().flex_none().bg(p.shell))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .bg(p.chat)
                    .p(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(div().w(px(46.)).h(px(5.)).rounded(px(2.)).bg(p.text))
                    .child(div().w(px(70.)).h(px(5.)).rounded(px(2.)).bg(p.text_muted))
                    .child(div().w(px(18.)).h(px(8.)).rounded(px(3.)).bg(p.brand)),
            )
    };
    let frame = div()
        .w_full()
        .h(px(76.))
        .rounded(px(8.))
        .overflow_hidden()
        .flex()
        .flex_row();
    match pref {
        Pref::Dark => frame.child(one(Theme::Dark)),
        Pref::Coal => frame.child(one(Theme::Coal)),
        Pref::Light => frame.child(one(Theme::Light)),
        Pref::System => frame
            .child(
                div()
                    .w(px(66.))
                    .h_full()
                    .flex_none()
                    .overflow_hidden()
                    .child(one(Theme::Dark)),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(
                        div()
                            .w(px(66.))
                            .h_full()
                            .flex_none()
                            .child(one(Theme::Light)),
                    ),
            ),
    }
}

/// A theme to pick: its picture and its name, with a ring when it is the one in use.
pub fn theme_card(id: &'static str, label: String, pref: Pref, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(132.))
        .flex()
        .flex_col()
        .gap_2()
        .cursor_pointer()
        .child(
            div()
                .p(px(3.))
                .rounded(px(11.))
                .border_2()
                .border_color(if selected {
                    theme::brand()
                } else {
                    theme::accent_border()
                })
                .child(theme_picture(pref)),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_1()
                .text_color(if selected {
                    theme::text_strong()
                } else {
                    theme::text_muted()
                })
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
}

/// The line that holds a value between its buttons: `[−] 16 px [+]`. The buttons are
/// the caller's, so they can act on the caller's view.
pub fn stepper(minus: Stateful<Div>, value: String, plus: Stateful<Div>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .py_2()
        .child(minus)
        .child(
            div()
                .w(px(72.))
                .flex()
                .justify_center()
                .text_color(theme::text_strong())
                .child(value),
        )
        .child(plus)
}

/// A plain button whose element ID is made at run time (one per row of a list).
pub fn button_with_id(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py_2()
        .rounded_md()
        .flex()
        .justify_center()
        .cursor_pointer()
        .text_color(theme::text_strong())
        .bg(theme::bg_textarea())
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}

/// One of a few buttons side by side, of which one is selected.
pub fn segment(id: &'static str, label: String, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py(px(6.))
        .rounded_md()
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
            theme::brand()
        } else {
            theme::accent_border()
        })
        .bg(if selected {
            theme::selected_bg()
        } else {
            theme::bg_textarea()
        })
        .text_color(if selected {
            theme::text_strong()
        } else {
            theme::text_muted()
        })
        .hover(|s| s.opacity(0.85))
        .child(label)
}
