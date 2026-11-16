// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Keyboard shortcuts tab: the list of shortcuts, the same one Ctrl+/ opens. The
//! keys are Fluxer's defaults; setting your own comes later.

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString};

use super::super::{SettingsTab, SettingsView};
use crate::app::keymap;
use crate::theme;
use crate::ui::overlays::keybind_help;

pub struct KeybindsTab;

impl SettingsTab for KeybindsTab {
    fn id(&self) -> &'static str {
        "keybinds"
    }

    fn title(&self) -> SharedString {
        t!("settings-keybinds-title").into()
    }

    fn render(&mut self, _cx: &mut Context<SettingsView>) -> AnyElement {
        // The list is longer than a short window: it scrolls inside the tab.
        div()
            .id("settings-keybinds")
            .h_full()
            .max_w(px(640.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_strong())
                    .child(t!("settings-keybinds-title")),
            )
            .child(
                div()
                    .pt_1()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(t!("settings-keybinds-note")),
            )
            .children(keybind_help::list(keymap::rows()))
            .into_any_element()
    }
}
