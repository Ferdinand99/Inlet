// SPDX-License-Identifier: AGPL-3.0-or-later
//! A folder's look and its settings window: name, colour, icon, whether the icon shows
//! while the folder is closed, deleting the folder, and marking its servers as read.

use gpui::{
    div, prelude::*, px, rgb, AnyElement, Context, Entity, Focusable, FontWeight, Hsla,
    SharedString, Subscription, Window,
};
use inlet_protocol::api::folders::{folder_icon, guild_folder_flags};
use inlet_protocol::models::GuildFolder;
use inlet_state::GuildId;

use super::order;
use crate::app::Inlet;
use crate::assets::Icon;
use crate::text_input::{InputEvent, TextInput};
use crate::theme;
use crate::ui::notifications::unread::unread_targets;
use crate::ui::overlays::modal::modal;
use crate::ui::overlays::nav_dialog::{title_el, NavDialog};
use crate::ui::widgets::button;

/// The colours a folder can have, besides the default.
pub const PALETTE: [u32; 9] = [
    0xE74C3C, 0xE67E22, 0xF1C40F, 0x2ECC71, 0x1ABC9C, 0x3498DB, 0x9B59B6, 0xE91E63, 0x95A5A6,
];

/// The colour of a folder: its own, or the brand colour.
pub fn color_of(color: Option<u64>) -> Hsla {
    match color {
        Some(c) if c != 0 && c <= 0xFF_FFFF => rgb(c as u32).into(),
        _ => theme::brand(),
    }
}

/// The picture of a folder icon name (Fluxer's seven).
pub fn icon_of(name: &str) -> Icon {
    match name {
        folder_icon::STAR => Icon::Star,
        folder_icon::HEART => Icon::Heart,
        folder_icon::BOOKMARK => Icon::Bookmark,
        folder_icon::GAME_CONTROLLER => Icon::GameController,
        folder_icon::SHIELD => Icon::Shield,
        folder_icon::MUSIC_NOTE => Icon::MusicNote,
        _ => Icon::Folder,
    }
}

/// Whether a closed folder shows its icon instead of its servers' pictures.
pub fn shows_icon_when_closed(f: &GuildFolder) -> bool {
    f.flags & guild_folder_flags::SHOW_ICON_WHEN_COLLAPSED != 0
}

/// A folder being edited.
pub struct FolderEdit {
    pub id: i64,
    pub name: Entity<TextInput>,
    /// 0 for the default colour.
    pub color: u64,
    pub icon: String,
    pub show_icon: bool,
    _submit: Subscription,
}

impl Inlet {
    /// Opens the folder's window with the name field focused.
    pub(crate) fn open_folder_settings(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(folder) = self.rail_folders(cx).into_iter().find(|f| f.id == Some(id)) else {
            return;
        };
        let name = cx.new(|cx| {
            TextInput::new(t!("rail-folder-name"), cx)
                .max_chars(100)
                .with_text(folder.name.clone().unwrap_or_default())
        });
        let submit = cx.subscribe(&name, |this, _, _: &InputEvent, cx| this.save_folder(cx));
        let field = name.focus_handle(cx);
        self.focus_layer_field(&field, window, cx);
        self.nav_show(
            NavDialog::Folder(FolderEdit {
                id,
                name,
                color: folder.color.unwrap_or(0),
                icon: if folder.icon.is_empty() {
                    folder_icon::FOLDER.to_owned()
                } else {
                    folder.icon.clone()
                },
                show_icon: shows_icon_when_closed(&folder),
                _submit: submit,
            }),
            cx,
        );
    }

    fn save_folder(&mut self, cx: &mut Context<Self>) {
        let Some(NavDialog::Folder(edit)) = self.nav.dialog.as_ref() else {
            return;
        };
        let name = edit.name.read(cx).text().trim().to_owned();
        let flags = if edit.show_icon {
            guild_folder_flags::SHOW_ICON_WHEN_COLLAPSED
        } else {
            0
        };
        let list = order::edit_folder(
            &self.rail_folders(cx),
            edit.id,
            Some(name),
            Some(edit.color),
            &edit.icon,
            flags,
        );
        self.nav.dialog = None;
        self.rail_save(list, cx);
    }

    fn delete_folder(&mut self, cx: &mut Context<Self>) {
        let Some(NavDialog::Folder(edit)) = self.nav.dialog.as_ref() else {
            return;
        };
        let list = order::delete_folder(&self.rail_folders(cx), edit.id);
        self.nav.dialog = None;
        self.rail_save(list, cx);
    }

    /// "Mark as read" for every server of the folder.
    fn mark_folder_read(&mut self, cx: &mut Context<Self>) {
        let Some(NavDialog::Folder(edit)) = self.nav.dialog.as_ref() else {
            return;
        };
        let id = edit.id;
        let guilds: Vec<String> = self
            .rail_folders(cx)
            .into_iter()
            .find(|f| f.id == Some(id))
            .map(|f| f.guild_ids)
            .unwrap_or_default();
        let Some(account) = self.account.clone() else {
            return;
        };
        let targets = {
            let state = account.read(cx);
            guilds
                .iter()
                .filter_map(|g| GuildId::parse(g))
                .flat_map(|g| unread_targets(state, Some(g)))
                .collect::<Vec<_>>()
        };
        self.nav.dialog = None;
        self.mark_listed_read(targets, cx);
    }

    pub(crate) fn render_folder_dialog(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(NavDialog::Folder(edit)) = self.nav.dialog.as_ref() else {
            return modal("folder", 480., []);
        };
        let (name, color, icon, show_icon) = (
            edit.name.clone(),
            edit.color,
            edit.icon.clone(),
            edit.show_icon,
        );
        let label = |text: String| -> AnyElement {
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_muted())
                .child(text.to_uppercase())
                .into_any_element()
        };
        // Colours: the default (the brand colour) and nine more.
        let mut swatches = div().flex().flex_row().flex_wrap().gap(px(8.));
        for c in std::iter::once(0u32).chain(PALETTE) {
            let selected = u64::from(c) == color;
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("folder-color-{c}")))
                    .size(px(28.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(color_of(Some(u64::from(c))))
                    .border_2()
                    .border_color(if selected {
                        theme::text_strong()
                    } else {
                        theme::bg_chat()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(NavDialog::Folder(e)) = this.nav.dialog.as_mut() {
                            e.color = u64::from(c);
                        }
                        cx.notify();
                    })),
            );
        }
        let mut icons = div().flex().flex_row().flex_wrap().gap(px(8.));
        for name_of in folder_icon::ALL {
            let selected = *name_of == icon;
            icons = icons.child(
                div()
                    .id(SharedString::from(format!("folder-icon-{name_of}")))
                    .size(px(36.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(if selected {
                        theme::selected_bg()
                    } else {
                        theme::bg_textarea()
                    })
                    .hover(|s| s.bg(theme::hover_bg()))
                    .child(icon_of(name_of).el(px(20.), color_of(Some(color))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(NavDialog::Folder(e)) = this.nav.dialog.as_mut() {
                            e.icon = (*name_of).to_owned();
                        }
                        cx.notify();
                    })),
            );
        }
        let toggle = div()
            .id("folder-show-icon")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_color(theme::text())
            .child(
                div()
                    .size(px(20.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if show_icon {
                        theme::brand()
                    } else {
                        theme::bg_textarea()
                    })
                    .when(show_icon, |d| {
                        d.child(Icon::Check.el(px(14.), theme::text_strong()))
                    }),
            )
            .child(t!("rail-folder-show-icon"))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(NavDialog::Folder(e)) = this.nav.dialog.as_mut() {
                    e.show_icon = !e.show_icon;
                }
                cx.notify();
            }));
        modal(
            "folder-settings",
            480.,
            [
                title_el(t!("rail-folder-title")),
                label(t!("rail-folder-name")),
                name.into_any_element(),
                label(t!("rail-folder-color")),
                swatches.into_any_element(),
                label(t!("rail-folder-icon")),
                icons.into_any_element(),
                toggle.into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        button("folder-read", t!("rail-folder-read"), false)
                            .on_click(cx.listener(|this, _, _, cx| this.mark_folder_read(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        button("folder-delete", t!("rail-folder-delete"), false)
                            .text_color(theme::danger())
                            .on_click(cx.listener(|this, _, _, cx| this.delete_folder(cx))),
                    )
                    .child(
                        button("folder-cancel", t!("common-cancel"), false)
                            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
                    )
                    .child(
                        button("folder-save", t!("rail-folder-save"), true)
                            .on_click(cx.listener(|this, _, _, cx| this.save_folder(cx))),
                    )
                    .into_any_element(),
            ],
        )
    }
}
