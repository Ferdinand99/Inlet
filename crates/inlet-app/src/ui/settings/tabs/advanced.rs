// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Advanced tab: developer mode, which shows "Copy ID" in the message and user
//! menus (as Fluxer does), and the account's own ID to copy.

use gpui::{div, prelude::*, px, AnyElement, ClipboardItem, Context, SharedString};

use super::super::controls::page;
use super::super::store;
use super::super::{with_app, SettingsTab, SettingsView};
use super::desktop::{heading, note, toggle};
use crate::backend::settings::write::Change;
use crate::theme;
use crate::ui::widgets::button;

pub struct AdvancedTab;

impl SettingsTab for AdvancedTab {
    fn id(&self) -> &'static str {
        "advanced"
    }

    fn title(&self) -> SharedString {
        t!("settings-advanced-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let developer = store::values(cx).developer_mode;
        let me = with_app(cx, |app, cx| {
            app.account.as_ref().map(|a| a.read(cx).me_id().to_string())
        })
        .flatten();

        let col = page(t!("settings-advanced-title"))
            .child(heading(t!("settings-advanced-developer")))
            .child(
                toggle(
                    "settings-developer-mode",
                    t!("settings-advanced-developer-mode"),
                    developer,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::DeveloperMode(!developer));
                    cx.notify();
                })),
            )
            .child(note(t!("settings-advanced-developer-note")));

        // Fluxer's experimental switch for "Clone to…" in the emoji and sticker menus. It is
        // a setting of this installation, as in Fluxer, and off by default.
        let clone = crate::desktop::expression_clone_shortcuts(cx);
        let col = col
            .child(heading(t!("settings-advanced-experimental")))
            .child(
                toggle(
                    "settings-clone-shortcuts",
                    t!("settings-advanced-clone-shortcuts"),
                    clone,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    crate::desktop::set_expression_clone_shortcuts(!clone, cx);
                    cx.notify();
                })),
            )
            .child(note(t!("settings-advanced-clone-shortcuts-note")));
        let mut col = col;

        if let Some(id) = me {
            let copy = id.clone();
            col = col.child(heading(t!("settings-advanced-id"))).child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .child(
                        div()
                            .px_3()
                            .py(px(6.))
                            .rounded(px(6.))
                            .bg(theme::bg_textarea())
                            .text_color(theme::text())
                            .child(id),
                    )
                    .child(
                        button("settings-copy-id", t!("settings-advanced-copy-id"), false)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            })),
                    ),
            );
        }
        col.into_any_element()
    }
}
