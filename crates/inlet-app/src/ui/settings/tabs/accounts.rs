// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Accounts tab: the accounts Inlet is signed in to (up to ten, on any instance),
//! which one is shown, and switching, signing out and adding another. It does what the
//! account switcher in the user panel does, from here.

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString};

use super::super::controls::{button_with_id, page};
use super::super::{close_later, with_app, SettingsTab, SettingsView};
use super::desktop::{heading, note};
use crate::backend::accounts::{AccountKey, Cmd, MAX_ACCOUNTS};
use crate::theme;
use crate::ui::widgets::button;

pub struct AccountsTab;

/// One account of the list, as the tab shows it.
struct Row {
    key: AccountKey,
    name: String,
    detail: String,
    active: bool,
}

impl SettingsTab for AccountsTab {
    fn id(&self) -> &'static str {
        "accounts"
    }

    fn title(&self) -> SharedString {
        t!("settings-accounts-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let rows: Vec<Row> = with_app(cx, |app, _| {
            app.switcher
                .accounts
                .iter()
                .map(|a| {
                    let handle = if a.handle.is_empty() {
                        String::new()
                    } else {
                        t!(
                            "accounts-handle-at",
                            handle = a.handle.clone(),
                            instance = a.key.origin.host().to_string()
                        )
                    };
                    Row {
                        key: a.key.clone(),
                        name: if a.name.is_empty() {
                            a.handle.clone()
                        } else {
                            a.name.clone()
                        },
                        detail: if handle.is_empty() {
                            a.key.origin.host().to_string()
                        } else {
                            handle
                        },
                        active: app.switcher.is_active(&a.key),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
        let full = rows.len() >= MAX_ACCOUNTS;

        let list = rows.into_iter().enumerate().map(|(ix, row)| {
            let switch_key = row.key.clone();
            let remove_key = row.key;
            div()
                .id(SharedString::from(format!("settings-account-{ix}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .py_2()
                .px_3()
                .rounded(px(8.))
                .bg(theme::bg_textarea())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::text_strong())
                                .overflow_hidden()
                                .child(row.name),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::text_muted())
                                .overflow_hidden()
                                .child(row.detail),
                        ),
                )
                .child(if row.active {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::brand())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("settings-accounts-shown"))
                        .into_any_element()
                } else {
                    button_with_id(
                        SharedString::from(format!("settings-account-switch-{ix}")),
                        t!("settings-accounts-switch"),
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let key = switch_key.clone();
                        with_app(cx, |app, cx| app.switch_account(key, window, cx));
                        close_later(cx);
                    }))
                    .into_any_element()
                })
                .child(
                    button_with_id(
                        SharedString::from(format!("settings-account-remove-{ix}")),
                        t!("settings-accounts-sign-out"),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let key = remove_key.clone();
                        with_app(cx, |app, cx| {
                            app.backend.send(Cmd::RemoveAccount(key));
                            cx.notify();
                        });
                    })),
                )
        });

        page(t!("settings-accounts-title"))
            .child(heading(t!("settings-accounts-signed-in")))
            .child(div().flex().flex_col().gap_2().children(list))
            .child(
                div().flex().flex_row().pt_4().child(
                    button(
                        "settings-account-add",
                        if full {
                            t!("accounts-full", max = MAX_ACCOUNTS)
                        } else {
                            t!("accounts-add")
                        },
                        false,
                    )
                    .when(full, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        if !full {
                            with_app(cx, |app, cx| app.begin_add_account(window, cx));
                            close_later(cx);
                        }
                    })),
                ),
            )
            .child(note(t!("settings-accounts-note")).pl_0().pt_2())
            .into_any_element()
    }
}
