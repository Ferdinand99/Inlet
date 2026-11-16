// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Advanced tab: a searchable list of the settings Fluxer calls advanced, grouped by
//! category, each with its control; a setting that lives in another tab has an "Open"
//! button. It also holds the developer settings: developer mode, which shows "Copy ID" in
//! the message and user menus (as Fluxer does), "Copy diagnostics" and the account's own ID.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, ClipboardItem, Context, Entity, SharedString};

use super::super::controls::page;
use super::super::registry::{self, Control, Item, Sub};
use super::super::store;
use super::super::sync::Values;
use super::super::{with_app, SettingsTab, SettingsView};
use super::desktop::{heading, note, toggle};
use crate::text_input::TextInput;
use crate::theme;
use crate::ui::widgets::button;

#[derive(Default)]
pub struct AdvancedTab {
    /// The search field, made when the tab is first drawn.
    query: Option<Entity<TextInput>>,
    /// "Copy diagnostics" was pressed.
    copied_diagnostics: Rc<Cell<bool>>,
}

impl AdvancedTab {
    /// The words typed into the search field.
    fn typed(&self, cx: &gpui::App) -> String {
        self.query
            .as_ref()
            .map(|q| q.read(cx).text().to_owned())
            .unwrap_or_default()
    }

    /// Shows the setting `id`: its words go into the search field, so the list holds it.
    fn show(&mut self, id: &str, cx: &mut Context<SettingsView>) {
        let Some(item) = registry::ITEMS.iter().find(|i| i.id == id && i.advanced) else {
            return;
        };
        let label = (item.label)();
        let input = self.input(cx);
        input.update(cx, |i, cx| i.set_text(label, cx));
    }

    fn input(&mut self, cx: &mut Context<SettingsView>) -> Entity<TextInput> {
        if let Some(input) = &self.query {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::new(t!("settings-advanced-search"), cx));
        // The list follows what is typed.
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        self.query = Some(input.clone());
        input
    }
}

impl SettingsTab for AdvancedTab {
    fn id(&self) -> &'static str {
        "advanced"
    }

    fn title(&self) -> SharedString {
        t!("settings-advanced-title").into()
    }

    fn focus_item(&mut self, id: &str, cx: &mut Context<SettingsView>) {
        self.show(id, cx);
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let input = self.input(cx);
        let typed = self.typed(cx);
        let values = store::values(cx);
        let me = with_app(cx, |app, cx| {
            app.account.as_ref().map(|a| a.read(cx).me_id().to_string())
        })
        .flatten();

        let items = registry::search(&typed, true);
        let mut col = page(t!("settings-advanced-title")).child(div().py_3().child(input));
        if items.is_empty() {
            col = col.child(note(t!("settings-advanced-none")));
        }
        // A search lists the best match first; without one the rows keep their category.
        let searching = !typed.trim().is_empty();
        let mut last = None;
        for item in items {
            if !searching && last != Some(item.category) {
                col = col.child(heading(item.category.label().to_uppercase()));
                last = Some(item.category);
            }
            col = col.child(self.row(item, &values, me.as_deref(), cx));
        }
        col.into_any_element()
    }
}

impl AdvancedTab {
    /// One row: the label and what it does, and the control.
    fn row(
        &mut self,
        item: &'static Item,
        values: &Values,
        me: Option<&str>,
        cx: &mut Context<SettingsView>,
    ) -> AnyElement {
        let mut row = div().flex().flex_col().py_1();
        let description = (item.description)();
        match &item.control {
            Control::Switch { get, set } => {
                let on = get(values);
                let set = *set;
                row = row
                    .child(
                        toggle(item.id, (item.label)(), on, true).on_click(cx.listener(
                            move |_, _, _, cx| {
                                store::change(cx, set(!on));
                                cx.notify();
                            },
                        )),
                    )
                    .child(note(description));
            }
            Control::Group(subs) => {
                row = row
                    .child(
                        div()
                            .pt_2()
                            .text_color(theme::text_strong())
                            .child((item.label)()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(description),
                    );
                for sub in *subs {
                    row = row.child(self.sub_row(sub, values, cx));
                }
            }
            Control::CopyDiagnostics => {
                let copied = self.copied_diagnostics.get();
                let flag = Rc::clone(&self.copied_diagnostics);
                row = row.child(
                    self.action_row(
                        item,
                        description,
                        button(
                            "settings-copy-diagnostics",
                            if copied {
                                t!("settings-adv-diagnostics-copied")
                            } else {
                                (item.label)()
                            },
                            false,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                crate::crash::diagnostics(),
                            ));
                            flag.set(true);
                            cx.notify();
                        })),
                    ),
                );
            }
            Control::CopyId => {
                if let Some(id) = me {
                    let copy = id.to_owned();
                    row = row.child(
                        self.action_row(
                            item,
                            description,
                            button("settings-copy-id", t!("settings-adv-copy-id-button"), false)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                })),
                        ),
                    );
                } else {
                    return div().into_any_element();
                }
            }
            Control::Open => {
                let tab = item.tab;
                row = row.child(self.action_row(
                    item,
                    description,
                    button("settings-open", t!("settings-open"), false).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.select(tab);
                            cx.notify();
                        },
                    )),
                ));
            }
        }
        row.into_any_element()
    }

    /// A row with a button on its right.
    fn action_row(
        &self,
        item: &Item,
        description: String,
        action: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .py_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_color(theme::text_strong()).child((item.label)()))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(description),
                    ),
            )
            .child(div().flex_none().child(action))
    }

    /// One switch of a group, a little inward; greyed out when its group says so.
    fn sub_row(
        &self,
        sub: &'static Sub,
        values: &Values,
        cx: &mut Context<SettingsView>,
    ) -> impl IntoElement {
        let on = (sub.get)(values);
        let enabled = !(sub.disabled)(values);
        let set = sub.set;
        div().pl(px(16.)).child(
            toggle(sub.id, (sub.label)(), on && enabled, enabled).on_click(cx.listener(
                move |_, _, _, cx| {
                    if enabled {
                        store::change(cx, set(!on));
                        cx.notify();
                    }
                },
            )),
        )
    }
}
