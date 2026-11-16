// SPDX-License-Identifier: AGPL-3.0-or-later
//! Favourites: channels from any server in one list, kept in the account's synced
//! preferences (`favorites`: channels, the person's own categories, which of them are
//! folded, and whether muted channels are left out), so they are the same in the Fluxer
//! app. The star in the rail (shown unless `show_favorites` is off) opens the list in
//! place of a server's channels; a channel is added or removed with the star button in
//! its header or from its right-click menu.
//!
//! A favourite belongs to a channel the account has; one it no longer knows is left out
//! of the list (it stays in the preferences, as Fluxer's client does).

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, MouseDownEvent, SharedString};
use inlet_protocol::proto::preferences::{FavoriteCategory, FavoriteChannel};
use inlet_state::ChannelId;

use crate::app::Inlet;
use crate::assets::Icon;
use crate::backend::composer::gif::PrefEdit;
use crate::model::ChannelKind;
use crate::theme;
use crate::ui::notifications::badge::mention_badge;
use crate::ui::sidebar::channel_list::{channel_icon, ROW_HEIGHT, ROW_RADIUS};
use crate::ui::HEADER_HEIGHT;

/// The channel kind of a protocol channel type, for its icon.
fn kind_of(kind: i64) -> ChannelKind {
    use inlet_protocol::models::channel_type::*;
    match kind {
        GUILD_VOICE => ChannelKind::Voice,
        GUILD_ANNOUNCEMENT => ChannelKind::Announcement,
        GUILD_FORUM => ChannelKind::Forum,
        GUILD_MEDIA => ChannelKind::Media,
        GUILD_LINK => ChannelKind::Link,
        _ => ChannelKind::Text,
    }
}

/// A channel as Fluxer stores it as a favourite: the server's category is not the
/// favourites category, so `parent_id` is empty (it holds the ID of one of the person's
/// own favourites categories, or nothing). A parent copied from the server would point at
/// a category the favourites list does not have.
pub fn new_favorite(channel: &str, guild: &str, count: usize) -> FavoriteChannel {
    FavoriteChannel {
        channel_id: channel.to_owned(),
        guild_id: guild.to_owned(),
        parent_id: None,
        position: i32::try_from(count).unwrap_or(i32::MAX),
        nickname: None,
    }
}

/// A favourite channel with what the list shows of it.
struct Shown {
    fav: FavoriteChannel,
    name: String,
    guild_name: String,
    kind: ChannelKind,
}

impl Inlet {
    /// Whether the rail shows the favourites star.
    pub(crate) fn favorites_star_shown(&self) -> bool {
        self.composer.pickers.prefs.show_favourites()
    }

    /// Opens the favourites list in place of a server's channels.
    pub(crate) fn open_favorites(&mut self, cx: &mut Context<Self>) {
        self.rail.favorites = true;
        cx.notify();
    }

    fn is_favorite(&self, channel: &str) -> bool {
        self.composer
            .pickers
            .prefs
            .favourites_state()
            .is_some_and(|f| f.channels.iter().any(|c| c.channel_id == channel))
    }

    /// Adds a channel of the open server to the favourites, or takes it out.
    pub(crate) fn toggle_favorite(&mut self, channel: &str, cx: &mut Context<Self>) {
        let on = !self.is_favorite(channel);
        let Some(guild) = self.selected_guild.clone() else {
            return;
        };
        if !self.channels.iter().any(|c| c.id == channel) {
            return;
        }
        let count = self
            .composer
            .pickers
            .prefs
            .favourites_state()
            .map_or(0, |f| f.channels.len());
        let fav = new_favorite(channel, &guild, count);
        self.edit_prefs(PrefEdit::FavouriteChannel { channel: fav, on });
        cx.notify();
    }

    /// Opens a favourite: its server, then the channel (the list stays up).
    fn open_favorite_channel(&mut self, guild: String, channel: String, cx: &mut Context<Self>) {
        if self.selected_guild.as_deref() == Some(guild.as_str()) {
            self.chat.pending_channel_jump = Some(channel);
        } else {
            self.overlays.link_channel = Some(channel);
            self.select_guild(Some(guild), cx);
        }
        cx.notify();
    }

    /// Adds a category of the person's own to the favourites.
    fn new_favorite_category(&mut self, cx: &mut Context<Self>) {
        let count = self
            .composer
            .pickers
            .prefs
            .favourites_state()
            .map_or(0, |f| f.categories.len());
        let category = FavoriteCategory {
            id: inlet_protocol::new_nonce(),
            name: t!("favorites-category-name", number = count + 1),
            position: i32::try_from(count).unwrap_or(i32::MAX),
        };
        self.edit_prefs(PrefEdit::FavouriteCategory { category, on: true });
        cx.notify();
    }

    fn favorite_rows_data(&self, cx: &gpui::App) -> Vec<Shown> {
        let Some(account) = self.account.as_ref() else {
            return Vec::new();
        };
        let state = account.read(cx);
        let Some(favs) = self.composer.pickers.prefs.favourites_state() else {
            return Vec::new();
        };
        let open = self.selected_channel.as_deref();
        favs.channels
            .iter()
            .filter_map(|f| {
                let id = ChannelId::parse(&f.channel_id)?;
                let c = state.channel(id)?;
                // Muted channels are left out when the person asked for that.
                if favs.hide_muted_channels
                    && Some(f.channel_id.as_str()) != open
                    && self.badge(&f.channel_id).dimmed
                    && self.badge(&f.channel_id).mentions == 0
                {
                    return None;
                }
                let guild = inlet_state::GuildId::parse(&f.guild_id)?;
                let name = f
                    .nickname
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .or_else(|| c.name.clone())
                    .map(|n| crate::model::clean_name(&n))
                    .unwrap_or_default();
                Some(Shown {
                    fav: f.clone(),
                    name,
                    guild_name: crate::model::clean_name(state.guild(guild)?.name()),
                    kind: kind_of(c.kind),
                })
            })
            .collect()
    }

    /// The favourites list: channels without a category first, then the person's
    /// categories (which fold), each with its channels.
    pub(super) fn favorite_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let shown = self.favorite_rows_data(cx);
        let (categories, folded, hide_muted) = match self.composer.pickers.prefs.favourites_state()
        {
            Some(f) => (
                f.categories.clone(),
                f.collapsed_category_ids.clone(),
                f.hide_muted_channels,
            ),
            None => (Vec::new(), Vec::new(), false),
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        // A new category of the person's own (Fluxer names it by asking; Inlet gives it a
        // name that can be told apart, and a category can be taken away again).
        rows.push(
            div()
                .id("favorites-new-category")
                .flex_none()
                .h(px(ROW_HEIGHT))
                .ml_2()
                .px_2()
                .rounded(px(ROW_RADIUS))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.))
                .text_color(theme::text_muted())
                .hover(|s| s.bg(theme::hover_bg()))
                .child(Icon::Plus.el(px(18.), theme::text_muted()))
                .child(t!("favorites-new-category"))
                .on_click(cx.listener(|this, _, _, cx| this.new_favorite_category(cx)))
                .into_any_element(),
        );
        // "Hide muted channels", a setting of the list.
        rows.push(
            div()
                .id("favorites-hide-muted")
                .flex_none()
                .h(px(ROW_HEIGHT))
                .ml_2()
                .px_2()
                .rounded(px(ROW_RADIUS))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.))
                .text_color(theme::text_muted())
                .hover(|s| s.bg(theme::hover_bg()))
                .child(
                    if hide_muted {
                        Icon::CheckSquare
                    } else {
                        Icon::Hash
                    }
                    .el(px(18.), theme::text_muted()),
                )
                .child(t!("favorites-hide-muted"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.edit_prefs(PrefEdit::FavouriteHideMuted(!hide_muted));
                    cx.notify();
                }))
                .into_any_element(),
        );
        if shown.is_empty() {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(t!("favorites-empty"))
                    .into_any_element(),
            );
            return rows;
        }
        let known_category = |c: &Shown| {
            c.fav
                .parent_id
                .as_ref()
                .is_some_and(|p| categories.iter().any(|cat| &cat.id == p))
        };
        let mut loose: Vec<&Shown> = shown.iter().filter(|c| !known_category(c)).collect();
        loose.sort_by_key(|c| c.fav.position);
        for c in loose {
            rows.push(self.favorite_row(c, cx));
        }
        let mut cats: Vec<&FavoriteCategory> = categories.iter().collect();
        cats.sort_by_key(|c| c.position);
        for cat in cats {
            let is_folded = folded.contains(&cat.id);
            let id = cat.id.clone();
            rows.push(
                div()
                    .id(SharedString::from(format!("fav-cat-{}", cat.id)))
                    .flex_none()
                    .h(px(ROW_HEIGHT))
                    .ml_2()
                    .px_2()
                    .mt_4()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_channel())
                    .hover(|s| s.text_color(theme::text_strong()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(crate::model::clean_name(&cat.name)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("fav-cat-del-{}", cat.id)))
                            .flex_none()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(|s| s.bg(theme::hover_bg()))
                            .child(Icon::X.el(px(12.), theme::text_channel()))
                            .on_click(cx.listener({
                                let category = cat.clone();
                                move |this, _, _, cx| {
                                    this.edit_prefs(PrefEdit::FavouriteCategory {
                                        category: category.clone(),
                                        on: false,
                                    });
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        if is_folded {
                            Icon::CaretRight
                        } else {
                            Icon::CaretDown
                        }
                        .el(px(12.), theme::text_channel()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_prefs(PrefEdit::FavouriteFolded {
                            id: id.clone(),
                            on: !is_folded,
                        });
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if is_folded {
                continue;
            }
            let mut inside: Vec<&Shown> = shown
                .iter()
                .filter(|c| c.fav.parent_id.as_deref() == Some(cat.id.as_str()))
                .collect();
            inside.sort_by_key(|c| c.fav.position);
            for c in inside {
                rows.push(self.favorite_row(c, cx));
            }
        }
        rows
    }

    fn favorite_row(&self, c: &Shown, cx: &mut Context<Self>) -> AnyElement {
        let (guild, channel) = (c.fav.guild_id.clone(), c.fav.channel_id.clone());
        let remove = c.fav.clone();
        let active = self.selected_channel.as_deref() == Some(c.fav.channel_id.as_str());
        let badge = self.badge(&c.fav.channel_id);
        let color = if active {
            theme::text_strong()
        } else {
            theme::text_channel()
        };
        let group = SharedString::from(format!("fav-{}", c.fav.channel_id));
        div()
            .id(SharedString::from(format!("fav-row-{}", c.fav.channel_id)))
            .group(group.clone())
            .debug_selector(|| "favorite-row".into())
            .flex_none()
            .h(px(ROW_HEIGHT))
            .ml_2()
            .px_2()
            .rounded(px(ROW_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .when(active, |d| d.bg(theme::selected_bg()))
            .when(!active, |d| {
                d.hover(|s| s.bg(theme::hover_bg()).text_color(theme::text()))
            })
            .when(badge.dimmed, |d| d.opacity(0.5))
            .child(channel_icon(c.kind).el(px(20.), color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(c.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(100.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(c.guild_name.clone()),
            )
            .when(badge.mentions > 0, |d| {
                d.child(mention_badge(badge.mentions))
            })
            .child(
                div()
                    .id(SharedString::from(format!(
                        "fav-remove-{}",
                        c.fav.channel_id
                    )))
                    .flex_none()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .hover(|s| s.bg(theme::hover_bg()))
                    .child(Icon::Star.el(px(14.), theme::brand()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_prefs(PrefEdit::FavouriteChannel {
                            channel: remove.clone(),
                            on: false,
                        });
                        cx.notify();
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_favorite_channel(guild.clone(), channel.clone(), cx)
            }))
            .into_any_element()
    }

    /// The header of the favourites list, as tall as a server's.
    pub(super) fn render_favorites_header(&self) -> AnyElement {
        div()
            .debug_selector(|| "guild-header".into())
            .flex_none()
            .w_full()
            .h(px(HEADER_HEIGHT))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::divider())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text_strong())
            .child(t!("nav-favorites"))
            .into_any_element()
    }

    /// A right-click on a channel row: its menu (favourites, copy ID).
    pub(crate) fn channel_context_menu(
        &mut self,
        channel: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::overlays::nav_dialog::NavDialog;
        self.nav_show(
            NavDialog::ChannelMenu {
                channel,
                at: event.position,
            },
            cx,
        );
    }

    /// Whether the channel is a favourite, for a channel of the open server that can be
    /// one (a server channel, not a category); `None` for any other.
    pub(crate) fn channel_favorite_state(&self, channel: &str) -> Option<bool> {
        let favorable = self.selected_guild.is_some()
            && self
                .channels
                .iter()
                .any(|c| c.id == channel && c.kind != ChannelKind::Category);
        favorable.then(|| self.is_favorite(channel))
    }
}

/// The star in the channel header: add the open channel to the favourites or take it out.
pub fn header_button(this: &Inlet, cx: &mut Context<Inlet>) -> Option<AnyElement> {
    // With the search column open in a window that is not wide, the chat is narrow and the
    // channel's name keeps its room (the right-click menu still has the entry).
    if this.search_panel_open() && this.members.window_width < 1500. {
        return None;
    }
    let channel = this.selected_channel.clone()?;
    let on = this.channel_favorite_state(&channel)?;
    let color = if on {
        theme::brand()
    } else {
        theme::text_muted()
    };
    Some(
        div()
            .id("favorite-button")
            .flex_none()
            .p(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover_bg()))
            .child(Icon::Star.el(px(24.), color))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_favorite(&channel, cx)))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_favorite_added_here_has_no_parent_so_fluxer_shows_it() {
        let f = new_favorite("301", "100", 2);
        assert_eq!(f.parent_id, None);
        assert_eq!(
            (f.channel_id.as_str(), f.guild_id.as_str(), f.position),
            ("301", "100", 2)
        );
        // It survives the synced preferences: stored, read back, parent still empty.
        let mut prefs = inlet_protocol::api::preferences::Preferences::default();
        PrefEdit::FavouriteChannel {
            channel: f.clone(),
            on: true,
        }
        .apply(&mut prefs);
        let stored = &prefs.favorites.as_ref().unwrap().channels;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].parent_id, None);
    }
}
