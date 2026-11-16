// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Appearance tab, with the options of Fluxer's own: the theme (dark, coal, light, or
//! the system's) and whether it follows the account, the message display (comfy or
//! dense) with its own spacing and avatars, the chat font size, the space between
//! message groups, and the zoom. The font size and the spacing offer exactly the values
//! Fluxer offers: Fluxer snaps any other value to one of them and writes it back. A
//! change shows at once and goes to the account, so Fluxer and Inlet agree; the zoom is
//! this computer's (the Desktop tab's setting).

use gpui::{div, prelude::*, px, rems, AnyElement, Context, SharedString};

use super::super::controls::{page, radio, stepper, theme_card};
use super::super::store;
use super::super::sync::{nearest, step, Values, FONT_SIZES, GROUP_SPACINGS};
use super::super::{SettingsTab, SettingsView};
use super::desktop::{heading, note, toggle};
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

/// The spacing change for the display in use: the compact display has its own.
fn spacing_change(compact: bool, px: u8) -> Change {
    if compact {
        Change::CompactGroupSpacing(px)
    } else {
        Change::GroupSpacing(px)
    }
}

/// The spacing the display in use shows.
fn spacing_of(v: &Values) -> u8 {
    if v.compact {
        v.compact_group_spacing
    } else {
        v.group_spacing
    }
}

/// Three sample messages as the display, font size and spacing make them look.
fn preview(v: &Values) -> gpui::Div {
    let compact = v.compact;
    let spacing = f32::from(spacing_of(v));
    let font = rems(f32::from(v.font_size) / theme::DEFAULT_FONT_PX);
    let line = px(f32::from(v.font_size) * 22. / 16.);
    let message = |first: bool, name: &str, time: String, text: String| {
        let name_el = div()
            .text_color(theme::text_strong())
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(name.to_owned());
        let body = div().text_color(theme::text()).child(text);
        if compact {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .pt(px(if first { spacing } else { 0. }))
                .child(
                    div()
                        .w(px(56.))
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(theme::timestamp())
                        .child(time),
                )
                .child(name_el)
                .child(body)
        } else {
            div()
                .flex()
                .flex_row()
                .gap_3()
                .pt(px(if first { spacing } else { 0. }))
                .child(
                    div()
                        .size(px(40.))
                        .flex_none()
                        .rounded_full()
                        .bg(theme::guild_item())
                        .when(!first, |d| d.opacity(0.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .when(first, |d| d.child(name_el))
                        .child(body),
                )
        }
    };
    div()
        .mt_2()
        .p_3()
        .rounded(px(8.))
        .bg(theme::bg_chat())
        .border_1()
        .border_color(theme::accent_border())
        .flex()
        .flex_col()
        .text_size(font)
        .line_height(line)
        .child(message(
            true,
            &t!("settings-appearance-preview-name"),
            t!("settings-appearance-preview-time-1"),
            t!("settings-appearance-preview-1"),
        ))
        .child(message(
            false,
            "",
            t!("settings-appearance-preview-time-2"),
            t!("settings-appearance-preview-2"),
        ))
        .child(message(
            true,
            &t!("settings-appearance-preview-name"),
            t!("settings-appearance-preview-time-3"),
            t!("settings-appearance-preview-3"),
        ))
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
        let sync = store::sync_theme(cx);
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

        let font = nearest(&FONT_SIZES, v.font_size);
        let compact = v.compact;
        let spacing = nearest(&GROUP_SPACINGS, spacing_of(&v));
        let default_spacing = if compact { 0 } else { 16 };

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
            .child(
                toggle(
                    "settings-theme-sync",
                    t!("settings-appearance-theme-sync"),
                    sync,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::set_sync_theme(cx, !sync);
                    cx.notify();
                })),
            )
            .child(note(t!("settings-appearance-theme-sync-note")))
            .child(heading(t!("settings-appearance-messages")))
            .child(preview(&v))
            .child(heading(t!("settings-appearance-font-size")))
            .child(
                stepper(
                    button("settings-font-smaller", "\u{2212}", false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            store::change(cx, Change::FontSize(step(&FONT_SIZES, font, -1)));
                            cx.notify();
                        },
                    )),
                    t!("settings-appearance-px", size = font),
                    button("settings-font-larger", "+", false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            store::change(cx, Change::FontSize(step(&FONT_SIZES, font, 1)));
                            cx.notify();
                        },
                    )),
                )
                .when(font != 16, |d| {
                    d.child(
                        button(
                            "settings-font-reset",
                            t!("settings-appearance-reset-font"),
                            false,
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            store::change(cx, Change::FontSize(16));
                            cx.notify();
                        })),
                    )
                }),
            )
            .child(heading(t!("settings-appearance-spacing")))
            .child(
                stepper(
                    button("settings-spacing-less", "\u{2212}", false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            let to = step(&GROUP_SPACINGS, spacing, -1);
                            store::change(cx, spacing_change(compact, to));
                            cx.notify();
                        },
                    )),
                    t!("settings-appearance-px", size = spacing),
                    button("settings-spacing-more", "+", false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            let to = step(&GROUP_SPACINGS, spacing, 1);
                            store::change(cx, spacing_change(compact, to));
                            cx.notify();
                        },
                    )),
                )
                .when(spacing != default_spacing, |d| {
                    d.child(
                        button(
                            "settings-spacing-reset",
                            t!("settings-appearance-reset-spacing"),
                            false,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            store::change(cx, spacing_change(compact, default_spacing));
                            cx.notify();
                        })),
                    )
                }),
            )
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
            .when(compact, |d| {
                // Fluxer's switch is "hide": avatars show when it is off.
                let hide = !v.compact_avatars;
                d.child(
                    toggle(
                        "settings-compact-avatars",
                        t!("settings-appearance-hide-avatars"),
                        hide,
                        true,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        store::change(cx, Change::CompactAvatars(hide));
                        cx.notify();
                    })),
                )
            })
            .child(heading(t!("settings-appearance-zoom")))
            .child(
                stepper(
                    button("settings-appearance-zoom-out", "\u{2212}", false).on_click(
                        cx.listener(|_, _, _, cx| {
                            desktop::zoom_by(-1, cx);
                            cx.notify();
                        }),
                    ),
                    t!("settings-desktop-zoom-value", percent = zoom),
                    button("settings-appearance-zoom-in", "+", false).on_click(cx.listener(
                        |_, _, _, cx| {
                            desktop::zoom_by(1, cx);
                            cx.notify();
                        },
                    )),
                )
                .when(zoom != 100, |d| {
                    d.child(
                        button(
                            "settings-appearance-zoom-reset",
                            t!("settings-appearance-reset-zoom"),
                            false,
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            desktop::set_zoom(1.0, cx);
                            cx.notify();
                        })),
                    )
                }),
            )
            .child(note(t!("settings-appearance-zoom-note")).pl_0())
            .into_any_element()
    }
}
