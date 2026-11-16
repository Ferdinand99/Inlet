// SPDX-License-Identifier: AGPL-3.0-or-later
//! The menu of one item in a picker (Fluxer's `ExpressionContextMenuItems`, the emoji and
//! sticker menus, `CloneExpressionMenuItem` and the expression info card): a right
//! click on an emoji, a sticker or a GIF opens it.
//!
//! - **Favourite** an emoji or a GIF (or take it out), as the right click did alone before.
//! - **Copy ID** of a custom emoji or sticker, **Copy URL** of its picture (on the
//!   instance's media proxy) and **Open in browser** (through the link check).
//! - **Clone to…** copies a custom emoji or sticker into one of the account's own
//!   communities: the server does the copying (`POST /guilds/{id}/emojis/clone`, and the
//!   same for stickers). It lists only communities where the account may manage
//!   expressions, other than the source, and only when the source community allows
//!   cloning (its `CLONE_EMOJI_ENABLED` or `CLONE_STICKER_ENABLED` feature).
//! - **Info** says what the item is and where it is from.
//!
//! Deviations: Fluxer shows the clone entry only after an experimental switch in the
//! advanced settings and greys out a community whose slots are full; Inlet shows it
//! whenever cloning is allowed and lets the server refuse a full community. Stickers
//! cannot be favourites yet.

use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnyElement, ClipboardItem, Context, FontWeight,
    MouseDownEvent, Pixels, Point, SharedString,
};
use inlet_protocol as p;
use inlet_protocol::proto::preferences::FavoriteGifEntry;

use super::{prefs, Cmd, ComposerCmd, PrefEdit};
use crate::app::Inlet;
use crate::theme;

/// Width of the menu.
const MENU_WIDTH: f32 = 240.;
/// Communities the clone list shows.
const MAX_TARGETS: usize = 25;

/// What the menu is about.
#[derive(Clone, Debug)]
pub(super) enum Subject {
    /// A Unicode emoji: only the favourite entry.
    UnicodeEmoji { key: String },
    CustomEmoji {
        key: String,
        id: String,
        name: String,
        animated: bool,
        guild: String,
    },
    Sticker {
        id: String,
        name: String,
        animated: bool,
        guild: String,
    },
    Gif {
        entry: Box<FavoriteGifEntry>,
        /// The link that goes into a message, if it can be sent.
        link: Option<String>,
        /// The search result it came from, which can be saved to the saved media (a
        /// favourite stored by Fluxer has none).
        source: Option<Box<p::Gif>>,
    },
    /// A saved media item: it can be removed.
    Meme { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Main,
    Clone,
    Info,
}

/// The open menu.
pub(super) struct ItemMenu {
    subject: Subject,
    at: Point<Pixels>,
    view: View,
}

/// A community the account is in, as the clone list needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Community {
    pub id: String,
    pub name: String,
    /// The account may manage expressions there.
    pub manage: bool,
}

/// The communities an expression of `source` may be cloned into: not the source itself,
/// only where the account manages expressions, by name, at most [`MAX_TARGETS`]. None
/// when the source community does not allow cloning.
pub fn clone_targets(
    communities: &[Community],
    source: &str,
    source_allows: bool,
) -> Vec<Community> {
    if !source_allows {
        return Vec::new();
    }
    let mut out: Vec<Community> = communities
        .iter()
        .filter(|c| c.manage && c.id != source)
        .cloned()
        .collect();
    out.sort_by_key(|c| c.name.to_lowercase());
    out.truncate(MAX_TARGETS);
    out
}

/// The feature that lets others clone a community's emoji (`sticker` false) or stickers.
pub fn clone_feature(sticker: bool) -> &'static str {
    if sticker {
        crate::backend::composer::gif::CLONE_STICKER_FEATURE
    } else {
        crate::backend::composer::gif::CLONE_EMOJI_FEATURE
    }
}

impl Inlet {
    /// Opens the menu of an item at `at` (where the click was).
    pub(super) fn open_item_menu(&mut self, subject: Subject, at: Point<Pixels>) {
        self.composer.pickers.menu = Some(ItemMenu {
            subject,
            at,
            view: View::Main,
        });
    }

    /// The communities of this account, for the clone list.
    fn communities(&self, cx: &gpui::App) -> Vec<Community> {
        let Some(account) = self.account.as_ref() else {
            return Vec::new();
        };
        let state = account.read(cx);
        state
            .guilds()
            .map(|g| Community {
                id: g.id.to_string(),
                name: crate::model::clean_name(g.name()),
                manage: state
                    .guild_permissions(g.id)
                    .has(p::permissions::MANAGE_EXPRESSIONS),
            })
            .collect()
    }

    /// Whether `guild`'s features include `feature`.
    fn community_has(&self, guild: &str, feature: &str, cx: &gpui::App) -> bool {
        let (Some(account), Some(id)) = (self.account.as_ref(), inlet_state::GuildId::parse(guild))
        else {
            return false;
        };
        account
            .read(cx)
            .guild(id)
            .and_then(|g| g.properties.as_ref())
            .is_some_and(|g| g.features.iter().any(|f| f == feature))
    }

    /// The community's name, when the account is in it.
    fn community_name(&self, guild: &str, cx: &gpui::App) -> Option<String> {
        let account = self.account.as_ref()?;
        let id = inlet_state::GuildId::parse(guild)?;
        let state = account.read(cx);
        let g = state.guild(id)?;
        Some(crate::model::clean_name(g.name()))
    }

    /// The URL of the item's picture on the media proxy.
    fn expression_url(&self, subject: &Subject) -> Option<String> {
        let proxy = &self.media.setup.as_ref()?.proxy;
        match subject {
            Subject::CustomEmoji { id, animated, .. } => proxy.emoji(id, *animated, 128),
            Subject::Sticker { id, animated, .. } => proxy.sticker(id, *animated, 320),
            _ => None,
        }
    }

    /// The menu over the picker, if one is open.
    pub(crate) fn render_item_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.composer.pickers.menu.as_ref()?;
        // Gone with the picker.
        if !self.composer.pickers.is_open() {
            self.composer.pickers.menu = None;
            return None;
        }
        let (subject, at, view) = (menu.subject.clone(), menu.at, menu.view);
        let body = match view {
            View::Main => self.menu_main(&subject, cx),
            View::Clone => self.menu_clone(&subject, cx),
            View::Info => self.menu_info(&subject, cx),
        };
        let frame = div()
            .id("picker-item-menu")
            .occlude()
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .p(px(6.))
            .rounded(px(8.))
            .bg(theme::floating_bg())
            .border_1()
            .border_color(theme::accent_border())
            .shadow(vec![gpui::BoxShadow {
                color: theme::floating_shadow(),
                offset: point(px(0.), px(6.)),
                blur_radius: px(14.),
                spread_radius: px(0.),
                inset: false,
            }])
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.composer.pickers.menu = None;
                cx.notify();
            }))
            .children(body);
        Some(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.))
                    .child(frame),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    fn menu_item(
        &self,
        id: &'static str,
        label: String,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Inlet, &mut Context<Inlet>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px(px(8.))
            .py(px(6.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(14.))
            .text_color(theme::text())
            .hover(|s| s.bg(theme::floating_hover()))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                action(this, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn close_menu(&mut self) {
        self.composer.pickers.menu = None;
    }

    fn menu_main(&self, subject: &Subject, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut items: Vec<AnyElement> = Vec::new();
        let favourite = |this: &Inlet, key: &str| this.composer.pickers.prefs.is_favourite(key);
        match subject {
            Subject::UnicodeEmoji { key } | Subject::CustomEmoji { key, .. } => {
                let on = !favourite(self, key);
                let key = key.clone();
                items.push(self.menu_item(
                    "item-menu-favourite",
                    if on {
                        t!("item-menu-fav-emoji")
                    } else {
                        t!("item-menu-unfav-emoji")
                    },
                    cx,
                    move |this, _| {
                        this.edit_prefs(PrefEdit::FavouriteEmoji {
                            key: key.clone(),
                            on,
                        });
                        this.close_menu();
                    },
                ));
            }
            Subject::Gif {
                entry,
                link,
                source,
            } => {
                let on = !self.composer.pickers.prefs.is_favourite_gif(&entry.url);
                let entry = entry.clone();
                items.push(self.menu_item(
                    "item-menu-favourite",
                    if on {
                        t!("item-menu-fav-gif")
                    } else {
                        t!("item-menu-unfav-gif")
                    },
                    cx,
                    move |this, _| {
                        this.edit_prefs(PrefEdit::FavouriteGif {
                            entry: entry.clone(),
                            on,
                        });
                        this.close_menu();
                    },
                ));
                if let Some(link) = link.clone() {
                    items.push(self.menu_item(
                        "item-menu-copy-link",
                        t!("item-menu-copy-gif"),
                        cx,
                        move |this, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                            this.chat.notice = Some(t!("media-link-copied"));
                            this.close_menu();
                        },
                    ));
                }
                if let Some(gif) = source.clone() {
                    items.push(self.menu_item(
                        "item-menu-save-gif",
                        t!("item-menu-save-gif"),
                        cx,
                        move |this, _| {
                            this.backend.send(ComposerCmd::Pickers(Cmd::SaveGifAsMeme {
                                gif: gif.clone(),
                            }));
                            this.close_menu();
                        },
                    ));
                }
            }
            Subject::Meme { id } => {
                let id = id.clone();
                items.push(self.menu_item(
                    "item-menu-remove-meme",
                    t!("item-menu-remove-meme"),
                    cx,
                    move |this, _| {
                        this.backend.send(ComposerCmd::Pickers(Cmd::DeleteMeme {
                            meme_id: id.clone(),
                        }));
                        this.close_menu();
                    },
                ));
            }
            Subject::Sticker { .. } => {}
        }
        // Custom emoji and stickers: ID, clone, URL, browser and info.
        let expression = match subject {
            Subject::CustomEmoji { id, guild, .. } => Some((id.clone(), guild.clone(), false)),
            Subject::Sticker { id, guild, .. } => Some((id.clone(), guild.clone(), true)),
            _ => None,
        };
        if let Some((id, guild, sticker)) = expression {
            let word = |emoji: String, sticker_text: String| {
                if sticker {
                    sticker_text
                } else {
                    emoji
                }
            };
            let copy = id.clone();
            items.push(self.menu_item(
                "item-menu-copy-id",
                word(
                    t!("item-menu-copy-emoji-id"),
                    t!("item-menu-copy-sticker-id"),
                ),
                cx,
                move |this, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    this.close_menu();
                },
            ));
            let allowed = self.community_has(&guild, clone_feature(sticker), cx);
            if !clone_targets(&self.communities(cx), &guild, allowed).is_empty() {
                items.push(self.menu_item(
                    "item-menu-clone",
                    word(t!("item-menu-clone-emoji"), t!("item-menu-clone-sticker")),
                    cx,
                    |this, _| {
                        if let Some(m) = &mut this.composer.pickers.menu {
                            m.view = View::Clone;
                        }
                    },
                ));
            }
            if let Some(url) = self.expression_url(subject) {
                let (copy, open) = (url.clone(), url);
                items.push(self.menu_item(
                    "item-menu-copy-url",
                    word(
                        t!("item-menu-copy-emoji-url"),
                        t!("item-menu-copy-sticker-url"),
                    ),
                    cx,
                    move |this, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        this.chat.notice = Some(t!("media-link-copied"));
                        this.close_menu();
                    },
                ));
                items.push(self.menu_item(
                    "item-menu-open",
                    word(t!("item-menu-open-emoji"), t!("item-menu-open-sticker")),
                    cx,
                    move |this, cx| {
                        this.close_menu();
                        this.open_link(&open, false, cx);
                    },
                ));
            }
            items.push(self.menu_item(
                "item-menu-info",
                word(t!("item-menu-emoji-info"), t!("item-menu-sticker-info")),
                cx,
                |this, _| {
                    if let Some(m) = &mut this.composer.pickers.menu {
                        m.view = View::Info;
                    }
                },
            ));
        }
        items
    }

    fn menu_clone(&self, subject: &Subject, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (sticker, source_id, name, guild) = match subject {
            Subject::CustomEmoji {
                id, name, guild, ..
            } => (false, id.clone(), name.clone(), guild.clone()),
            Subject::Sticker {
                id, name, guild, ..
            } => (true, id.clone(), name.clone(), guild.clone()),
            _ => return Vec::new(),
        };
        let allowed = self.community_has(&guild, clone_feature(sticker), cx);
        let targets = clone_targets(&self.communities(cx), &guild, allowed);
        let mut items = vec![div()
            .px(px(8.))
            .pb(px(4.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text_tertiary())
            .child(if sticker {
                t!("item-menu-clone-sticker")
            } else {
                t!("item-menu-clone-emoji")
            })
            .into_any_element()];
        for (ix, target) in targets.into_iter().enumerate() {
            let (source_id, name) = (source_id.clone(), name.clone());
            let id = SharedString::from(format!("item-menu-target-{ix}"));
            let label = target.name.clone();
            items.push(
                div()
                    .id(id)
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(14.))
                    .text_color(theme::text())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .hover(|s| s.bg(theme::floating_hover()))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_menu();
                        this.backend
                            .send(ComposerCmd::Pickers(Cmd::CloneExpression {
                                sticker,
                                guild_id: target.id.clone(),
                                source_id: source_id.clone(),
                                name: name.clone(),
                                target_name: target.name.clone(),
                            }));
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        items
    }

    fn menu_info(&self, subject: &Subject, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (sticker, name, guild) = match subject {
            Subject::CustomEmoji { name, guild, .. } => (false, format!(":{name}:"), guild.clone()),
            Subject::Sticker { name, guild, .. } => (true, name.clone(), guild.clone()),
            _ => return Vec::new(),
        };
        let from = self.community_name(&guild, cx);
        let line = |text: String, strong: bool| {
            div()
                .px(px(8.))
                .py(px(2.))
                .text_size(px(if strong { 15. } else { 13. }))
                .when(strong, |d| {
                    d.font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_strong())
                })
                .when(!strong, |d| d.text_color(theme::text_muted()))
                .child(text_or_dash(text))
                .into_any_element()
        };
        vec![
            line(name, true),
            line(
                if sticker {
                    t!("item-info-sticker")
                } else {
                    t!("item-info-emoji")
                },
                false,
            ),
            line(
                match from {
                    Some(community) => t!("item-info-from", community = community.as_str()),
                    None => t!("item-info-from-other"),
                },
                false,
            ),
        ]
    }
}

fn text_or_dash(text: String) -> String {
    if text.trim().is_empty() {
        "\u{2013}".to_owned()
    } else {
        text
    }
}

/// The subject of a right click on an emoji cell.
pub(super) fn emoji_subject(cell: &super::emoji::Cell) -> Subject {
    match cell {
        super::emoji::Cell::Unicode(e) => Subject::UnicodeEmoji {
            key: prefs::unicode_key(e),
        },
        super::emoji::Cell::Custom(c) => Subject::CustomEmoji {
            key: prefs::custom_key(&c.guild, &c.id),
            id: c.id.clone(),
            name: c.name.clone(),
            animated: c.animated,
            guild: c.guild.clone(),
        },
    }
}

/// Where a right click was.
pub(super) fn click_at(e: &MouseDownEvent) -> Point<Pixels> {
    e.position
}

#[cfg(test)]
mod tests {
    use super::*;

    fn community(id: &str, name: &str, manage: bool) -> Community {
        Community {
            id: id.into(),
            name: name.into(),
            manage,
        }
    }

    #[test]
    fn a_clone_goes_to_other_communities_the_account_manages() {
        let all = vec![
            community("1", "Zebras", true),
            community("2", "apes", true),
            community("3", "Bears", false),
            community("4", "Source", true),
        ];
        let names: Vec<String> = clone_targets(&all, "4", true)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            ["apes", "Zebras"],
            "sorted, not the source, not read-only"
        );
    }

    #[test]
    fn nothing_to_clone_when_the_source_does_not_allow_it() {
        let all = vec![community("1", "A", true)];
        assert!(clone_targets(&all, "9", false).is_empty());
        assert!(clone_targets(&[], "9", true).is_empty());
        // The only community is the source.
        assert!(clone_targets(&all, "1", true).is_empty());
    }

    #[test]
    fn the_clone_list_is_capped() {
        let all: Vec<Community> = (0..100)
            .map(|i| community(&i.to_string(), &format!("c{i:03}"), true))
            .collect();
        assert_eq!(clone_targets(&all, "x", true).len(), MAX_TARGETS);
    }

    #[test]
    fn emoji_and_stickers_have_their_own_clone_feature() {
        assert_eq!(
            clone_feature(false),
            crate::backend::composer::gif::CLONE_EMOJI_FEATURE
        );
        assert_eq!(
            clone_feature(true),
            crate::backend::composer::gif::CLONE_STICKER_FEATURE
        );
    }

    #[test]
    fn an_empty_name_shows_a_dash() {
        assert_eq!(text_or_dash("  ".into()), "\u{2013}");
        assert_eq!(text_or_dash("x".into()), "x");
    }
}
