// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Messages and media tab, with the options of Fluxer's own: animations (emoji, GIFs,
//! stickers), where media shows inline, link previews, reactions and spoilers. Each
//! switch is the account's (the same ones Fluxer has) and changes what the message list
//! shows at once.

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
        let (gif, emoji, stickers) = (v.gif_auto_play, v.animate_emoji, v.animate_stickers);
        let (embeds, reactions) = (v.render_embeds, v.render_reactions);
        let (inline_links, inline_uploads) = (v.inline_embed_media, v.inline_attachment_media);
        let spoilers = v.render_spoilers;
        let convert = v.convert_emoticons;

        let sticker = |id: &'static str, label: String, hint: String, value: u8| {
            radio(id, label, Some(hint), stickers == value).on_click(cx.listener(
                move |_, _, _, cx| {
                    store::change(cx, Change::AnimateStickers(value));
                    cx.notify();
                },
            ))
        };
        let spoiler = |id: &'static str, label: String, hint: String, value: u8| {
            radio(id, label, Some(hint), spoilers == value).on_click(cx.listener(
                move |_, _, _, cx| {
                    store::change(cx, Change::RenderSpoilers(value));
                    cx.notify();
                },
            ))
        };

        page(t!("settings-media-title"))
            .child(heading(t!("settings-media-animations")))
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
                toggle("settings-gif-autoplay", t!("settings-media-gif"), gif, true).on_click(
                    cx.listener(move |_, _, _, cx| {
                        store::change(cx, Change::GifAutoPlay(!gif));
                        cx.notify();
                    }),
                ),
            )
            .child(note(t!("settings-media-gif-note")))
            .child(heading(t!("settings-media-stickers")))
            .child(sticker(
                "settings-stickers-always",
                t!("settings-media-stickers-always"),
                t!("settings-media-stickers-always-hint"),
                0,
            ))
            .child(sticker(
                "settings-stickers-hover",
                t!("settings-media-stickers-hover"),
                t!("settings-media-stickers-hover-hint"),
                1,
            ))
            .child(sticker(
                "settings-stickers-never",
                t!("settings-media-stickers-never"),
                t!("settings-media-stickers-never-hint"),
                2,
            ))
            .child(heading(t!("settings-media-inline")))
            .child(
                toggle(
                    "settings-inline-links",
                    t!("settings-media-inline-links"),
                    inline_links,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::InlineEmbedMedia(!inline_links));
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "settings-inline-uploads",
                    t!("settings-media-inline-uploads"),
                    inline_uploads,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::InlineAttachmentMedia(!inline_uploads));
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
            .child(heading(t!("settings-media-reactions-heading")))
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
            .child(heading(t!("settings-media-input")))
            .child(
                toggle(
                    "settings-convert-emoticons",
                    t!("settings-media-emoticons"),
                    v.convert_emoticons,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::ConvertEmoticons(!convert));
                    cx.notify();
                })),
            )
            .child(heading(t!("settings-media-spoilers")))
            .child(spoiler(
                "settings-spoilers-click",
                t!("settings-media-spoilers-click"),
                t!("settings-media-spoilers-click-hint"),
                1,
            ))
            .child(spoiler(
                "settings-spoilers-moderator",
                t!("settings-media-spoilers-moderator"),
                t!("settings-media-spoilers-moderator-hint"),
                2,
            ))
            .child(spoiler(
                "settings-spoilers-always",
                t!("settings-media-spoilers-always"),
                t!("settings-media-spoilers-always-hint"),
                0,
            ))
            .into_any_element()
    }
}
