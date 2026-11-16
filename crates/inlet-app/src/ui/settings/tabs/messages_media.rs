// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Messages and media tab: GIFs and animated emoji and stickers, embeds and link
//! previews, reactions and spoilers. Each switch is the account's (the same ones Fluxer
//! has) and changes what the message list shows at once.

use gpui::{prelude::*, AnyElement, Context, SharedString};

use super::super::controls::{page, radio};
use super::super::store;
use super::super::{SettingsTab, SettingsView};
use super::desktop::{heading, note, toggle};
use crate::backend::settings::write::Change;

pub struct MessagesMediaTab;

impl SettingsTab for MessagesMediaTab {
    fn id(&self) -> &'static str {
        "messages-media"
    }

    fn title(&self) -> SharedString {
        t!("settings-media-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let v = store::values(cx);
        let (gif, emoji, stickers) = (v.gif_auto_play, v.animate_emoji, v.animate_stickers != 2);
        let (embeds, reactions) = (v.render_embeds, v.render_reactions);
        let always = v.render_spoilers == 0;

        page(t!("settings-media-title"))
            .child(heading(t!("settings-media-animations")))
            .child(
                toggle("settings-gif-autoplay", t!("settings-media-gif"), gif, true).on_click(
                    cx.listener(move |_, _, _, cx| {
                        store::change(cx, Change::GifAutoPlay(!gif));
                        cx.notify();
                    }),
                ),
            )
            .child(note(t!("settings-media-gif-note")))
            .child(
                toggle(
                    "settings-animate-emoji",
                    t!("settings-media-emoji"),
                    emoji,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::AnimateEmoji(!emoji));
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "settings-animate-stickers",
                    t!("settings-media-stickers"),
                    stickers,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::AnimateStickers(if stickers { 2 } else { 0 }));
                    cx.notify();
                })),
            )
            .child(heading(t!("settings-media-links")))
            .child(
                toggle(
                    "settings-render-embeds",
                    t!("settings-media-embeds"),
                    embeds,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::RenderEmbeds(!embeds));
                    cx.notify();
                })),
            )
            .child(note(t!("settings-media-embeds-note")))
            .child(
                toggle(
                    "settings-render-reactions",
                    t!("settings-media-reactions"),
                    reactions,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::RenderReactions(!reactions));
                    cx.notify();
                })),
            )
            .child(heading(t!("settings-media-spoilers")))
            .child(
                radio(
                    "settings-spoilers-click",
                    t!("settings-media-spoilers-click"),
                    None,
                    !always,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    store::change(cx, Change::RenderSpoilers(1));
                    cx.notify();
                })),
            )
            .child(
                radio(
                    "settings-spoilers-always",
                    t!("settings-media-spoilers-always"),
                    None,
                    always,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    store::change(cx, Change::RenderSpoilers(0));
                    cx.notify();
                })),
            )
            .into_any_element()
    }
}
