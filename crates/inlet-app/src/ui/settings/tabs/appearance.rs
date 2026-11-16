// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Appearance tab: the theme (dark, coal, light, or the system's), how messages are
//! laid out (cozy or compact), the message font size, the space between groups of
//! messages and the zoom. A change shows at once and goes to the account, so Fluxer and
//! Inlet agree; the zoom is this computer's (the Desktop tab's setting).

use gpui::{div, prelude::*, px, rems, AnyElement, Context, SharedString};

use super::super::controls::{page, radio, stepper, theme_card};
use super::super::store;
use super::super::sync::{FONT_SIZES, GROUP_SPACINGS, SPACING_STEP};
use super::super::{SettingsTab, SettingsView};
use super::desktop::{heading, note};
use crate::backend::settings::write::Change;
use crate::desktop::{self, Desktop};
use crate::theme::{self, Pref};
use crate::ui::widgets::button;

pub struct AppearanceTab;

/// The id and name of a theme's card.
fn card(pref: Pref) -> (&'static str, String) {
    match pref {
        Pref::Dark => ("settings-theme-dark", t!("settings-appearance-theme-dark")),
        Pref::Coal => ("settings-theme-coal", t!("settings-appearance-theme-coal")),
        Pref::Light => (
            "settings-theme-light",
            t!("settings-appearance-theme-light"),
        ),
        Pref::System => (
            "settings-theme-system",
            t!("settings-appearance-theme-system"),
        ),
    }
}

impl SettingsTab for AppearanceTab {
    fn id(&self) -> &'static str {
        "appearance"
    }

    fn title(&self) -> SharedString {
        t!("settings-appearance-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let v = store::values(cx);
        let zoom = cx
            .try_global::<Desktop>()
            .map_or(100, |d| (d.prefs.zoom * 100.0).round() as u32);

        let cards = Pref::ALL.map(|pref| {
            let (id, label) = card(pref);
            theme_card(id, label, pref, v.theme == pref).on_click(cx.listener(
                move |_, _, _, cx| {
                    store::change(cx, Change::Theme(pref));
                    cx.notify();
                },
            ))
        });

        let font = v.font_size;
        let spacing = v.group_spacing;
        let compact = v.compact;
        let sample = div()
            .mt_2()
            .p_3()
            .rounded(px(8.))
            .bg(theme::bg_textarea())
            .text_color(theme::text())
            .text_size(rems(theme::message_font_px() / theme::DEFAULT_FONT_PX))
            .line_height(px(theme::message_line_px()))
            .child(t!("settings-appearance-sample"));

        page(t!("settings-appearance-title"))
            .child(heading(t!("settings-appearance-theme")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_4()
                    .py_2()
                    .children(cards),
            )
            .child(note(t!("settings-appearance-theme-note")).pl_0())
            .child(heading(t!("settings-appearance-display")))
            .child(
                radio(
                    "settings-display-cozy",
                    t!("settings-appearance-cozy"),
                    Some(t!("settings-appearance-cozy-hint")),
                    !compact,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    store::change(cx, Change::Compact(false));
                    cx.notify();
                })),
            )
            .child(
                radio(
                    "settings-display-compact",
                    t!("settings-appearance-compact"),
                    Some(t!("settings-appearance-compact-hint")),
                    compact,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    store::change(cx, Change::Compact(true));
                    cx.notify();
                })),
            )
            .child(heading(t!("settings-appearance-font-size")))
            .child(stepper(
                button("settings-font-smaller", "\u{2212}", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        let to = font.saturating_sub(1).max(*FONT_SIZES.start());
                        store::change(cx, Change::FontSize(to));
                        cx.notify();
                    },
                )),
                t!("settings-appearance-px", size = font),
                button("settings-font-larger", "+", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        let to = (font + 1).min(*FONT_SIZES.end());
                        store::change(cx, Change::FontSize(to));
                        cx.notify();
                    },
                )),
            ))
            .child(sample)
            .child(heading(t!("settings-appearance-spacing")))
            .child(stepper(
                button("settings-spacing-less", "\u{2212}", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        let to = spacing
                            .saturating_sub(SPACING_STEP)
                            .max(*GROUP_SPACINGS.start());
                        store::change(cx, Change::GroupSpacing(to));
                        cx.notify();
                    },
                )),
                t!("settings-appearance-px", size = spacing),
                button("settings-spacing-more", "+", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        let to = (spacing + SPACING_STEP).min(*GROUP_SPACINGS.end());
                        store::change(cx, Change::GroupSpacing(to));
                        cx.notify();
                    },
                )),
            ))
            .child(note(t!("settings-appearance-spacing-note")).pl_0())
            .child(heading(t!("settings-appearance-zoom")))
            .child(stepper(
                button("settings-appearance-zoom-out", "\u{2212}", false).on_click(cx.listener(
                    |_, _, _, cx| {
                        desktop::zoom_by(-1, cx);
                        cx.notify();
                    },
                )),
                t!("settings-desktop-zoom-value", percent = zoom),
                button("settings-appearance-zoom-in", "+", false).on_click(cx.listener(
                    |_, _, _, cx| {
                        desktop::zoom_by(1, cx);
                        cx.notify();
                    },
                )),
            ))
            .child(note(t!("settings-appearance-zoom-note")).pl_0())
            .into_any_element()
    }
}
