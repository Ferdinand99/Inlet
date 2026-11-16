// SPDX-License-Identifier: AGPL-3.0-or-later
//! The result lists of `/gif`, `/sticker` and `/saved` (Fluxer's autocomplete for them):
//! the composer reports what is searched (`composer::media_list`), this module finds the
//! rows, draws the list above the composer and sends what is picked.
//!
//! - **`/gif`**: the instance's GIF search, as in the picker (the same 350 ms wait after
//!   the last key, one request per search, only the newest answer shown); with no text,
//!   trending. A pick sends the GIF's link as a message and tells the instance, as the
//!   picker does. Nothing here talks to the GIF provider: previews come from the media
//!   proxy only.
//! - **`/sticker`**: the stickers the account may use in the open channel, by name or tag.
//! - **`/saved`**: the account's saved media, by name or tag. A pick sends it as a message
//!   of its own, as the memes tab does.
//!
//! At most [`MAX_ROWS`] rows show, as in Fluxer. The list belongs to the account it was
//! asked for and goes with the account state like the rest of the pickers.

use std::time::Instant;

use gpui::{
    div, point, prelude::*, px, AnyElement, Context, FontWeight, ObjectFit, SharedString, Task,
};
use inlet_editor::MediaCommand;
use inlet_protocol as p;

use super::gif::{requests as gif_requests, sendable, GifState, SEARCH_DEBOUNCE};
use super::{meme, usable, COMPOSER_BOTTOM, COMPOSER_RIGHT, PICKER_WIDTH};
use crate::app::Inlet;
use crate::backend::composer::gif::{locale, Cmd};
use crate::backend::composer::Cmd as ComposerCmd;
use crate::model::Sticker;
use crate::theme;
use crate::ui::composer::autocomplete::{Candidate, Pick};
use crate::ui::composer::media_list::MediaEvent;
use crate::ui::media::animation_budget::Motion;
use crate::ui::media::image::{picture_of, placeholder};
use crate::ui::media::sticker::sticker_requests;

/// Rows a list shows at most (Fluxer's `MENTION_RESULT_LIMIT`).
pub const MAX_ROWS: usize = 10;
const ROW_H: f32 = 48.;
const THUMB: (f32, f32) = (72., 40.);

/// What a row is.
#[derive(Clone, Debug)]
pub(crate) enum Item {
    Gif(Box<p::Gif>),
    Sticker(Sticker),
    Meme(Box<p::Meme>),
}

/// Why a list has no rows, for the text in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Status {
    /// Nothing matched.
    #[default]
    Done,
    /// An answer is on its way.
    Loading,
    Failed(String),
    /// The instance has no GIF service.
    NoGifs,
}

/// The lists' state for the shown account.
#[derive(Default)]
pub(crate) struct MediaList {
    /// The GIF searches of the list (the picker's tab has its own).
    gifs: GifState,
    /// The command being typed and its search text.
    kind: Option<MediaCommand>,
    query: String,
    items: Vec<Item>,
    status: Status,
    /// Waits for the typing to stop before a GIF search goes out.
    timer: Option<Task<()>>,
}

impl MediaList {
    /// The command was left: searches on their way are not wanted.
    pub fn leave(&mut self) {
        self.kind = None;
        self.query.clear();
        self.items.clear();
        self.status = Status::Done;
        self.timer = None;
        self.gifs.cancel_search();
    }

    /// Whether `nonce` is a GIF search of this list.
    pub fn waits_for(&self, nonce: u64) -> bool {
        self.gifs.waits_for(nonce)
    }

    /// An answer to one of this list's GIF searches.
    pub fn gifs_answered(&mut self, nonce: u64, result: Result<Vec<p::Gif>, String>) {
        self.gifs.answered_gifs(nonce, result);
    }

    pub fn is_open(&self) -> bool {
        self.kind.is_some()
    }

    #[cfg(test)]
    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

/// The rows of a list for the composer: the title shown, and what a pick does. A GIF
/// goes in as its link; the others are sent by the app. Row `i` is item `i`.
pub(crate) fn candidates(items: &[Item]) -> Vec<Candidate> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let (title, text) = match item {
                Item::Gif(g) => (gif_title(g), sendable(&g.url)),
                Item::Sticker(s) => (s.name.clone(), None),
                Item::Meme(m) => (crate::model::clean_name(&m.name), None),
            };
            Candidate {
                title,
                detail: None,
                glyph: None,
                pick: Pick::Media { index, text },
            }
        })
        .collect()
}

/// A GIF's title for the list: its own, else the last part of its page address.
fn gif_title(g: &p::Gif) -> String {
    let title = crate::model::clean_name(&g.title);
    if !g.title.trim().is_empty() {
        return title;
    }
    url::Url::parse(&g.url)
        .ok()
        .and_then(|u| {
            u.path_segments()?
                .rfind(|s| !s.is_empty())
                .map(|s| crate::model::clean_name(&s.replace(['-', '_'], " ")))
        })
        .unwrap_or(title)
}

/// The GIFs a list shows: those with a link that can be sent, at most [`MAX_ROWS`].
pub(crate) fn gif_items(gifs: &[p::Gif]) -> Vec<Item> {
    gifs.iter()
        .filter(|g| sendable(&g.url).is_some())
        .take(MAX_ROWS)
        .map(|g| Item::Gif(Box::new(g.clone())))
        .collect()
}

/// The saved media matching `query` (name or tag), newest first, at most [`MAX_ROWS`].
pub(crate) fn meme_items(memes: &[p::Meme], query: &str) -> Vec<Item> {
    meme::matching(memes, query)
        .into_iter()
        .take(MAX_ROWS)
        .map(|m| Item::Meme(Box::new(m.clone())))
        .collect()
}

/// The stickers matching `query` (name or tag) that may be used in a channel of
/// `channel_guild`, the open community's first, at most [`MAX_ROWS`].
pub(crate) fn sticker_items(
    sets: &[super::sticker::GuildStickers],
    query: &str,
    channel_guild: Option<&str>,
    external: bool,
    global: bool,
) -> Vec<Item> {
    let q = query.trim().to_lowercase();
    sets.iter()
        .filter(|set| usable(&set.guild, channel_guild, external, global))
        .flat_map(|set| set.stickers.iter())
        .filter(|(s, tags)| {
            q.is_empty()
                || s.name.to_lowercase().contains(&q)
                || tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
        .take(MAX_ROWS)
        .map(|(s, _)| Item::Sticker(s.clone()))
        .collect()
}

impl Inlet {
    /// What the composer says about a media command.
    pub(crate) fn on_media_list_event(&mut self, event: MediaEvent, cx: &mut Context<Self>) {
        self.pickers_follow_account(cx);
        match event {
            MediaEvent::Query { kind, query } => {
                self.pickers_setup(cx);
                let list = &mut self.composer.pickers.commands;
                // A new command starts from an empty list.
                if list.kind != Some(kind) {
                    list.items.clear();
                }
                list.kind = Some(kind);
                list.query = query;
                self.media_refresh(true, cx);
            }
            MediaEvent::Picked {
                kind,
                index,
                channel_id,
            } => self.media_picked(kind, index, channel_id),
            MediaEvent::Left => self.composer.pickers.commands.leave(),
        }
        cx.notify();
    }

    /// Something a list waits for arrived (GIFs, the saved media): the rows again.
    pub(crate) fn media_rows_changed(&mut self, cx: &mut Context<Self>) {
        if self.composer.pickers.commands.is_open() {
            self.media_refresh(false, cx);
        }
    }

    /// Finds the rows for the open command and gives them to the composer. `typed` says
    /// the search text just changed (a GIF search is then scheduled).
    fn media_refresh(&mut self, typed: bool, cx: &mut Context<Self>) {
        let Some(kind) = self.composer.pickers.commands.kind else {
            return;
        };
        let query = self.composer.pickers.commands.query.clone();
        let (items, status) = match kind {
            MediaCommand::Saved => self.saved_items(&query),
            MediaCommand::Sticker => (self.sticker_rows(&query, cx), Status::Done),
            MediaCommand::Gif => self.gif_rows(&query, typed, cx),
        };
        let rows = candidates(&items);
        let list = &mut self.composer.pickers.commands;
        list.items = items;
        list.status = status;
        let view = self.composer.view.clone();
        view.update(cx, |v, cx| v.set_media_rows(kind, &query, rows, cx));
    }

    fn saved_items(&mut self, query: &str) -> (Vec<Item>, Status) {
        let pickers = &mut self.composer.pickers;
        if pickers.memes.is_none() {
            if !pickers.memes_asked {
                pickers.memes_error = None;
                pickers.memes_asked = true;
                self.backend.send(ComposerCmd::Pickers(Cmd::LoadMemes));
            }
            return (
                Vec::new(),
                match &pickers.memes_error {
                    Some(e) => Status::Failed(e.clone()),
                    None => Status::Loading,
                },
            );
        }
        let memes = pickers.memes.as_deref().unwrap_or_default();
        (meme_items(memes, query), Status::Done)
    }

    /// The open channel's community and the account's rights to use other communities'
    /// stickers there.
    fn media_channel(&self, cx: &gpui::App) -> (Option<String>, bool) {
        let ch = self
            .selected_channel
            .as_deref()
            .and_then(inlet_state::ChannelId::parse);
        let (Some(account), Some(ch)) = (self.account.as_ref(), ch) else {
            return (None, false);
        };
        let state = account.read(cx);
        let Some(g) = state.channels().guild_of(ch) else {
            return (None, false);
        };
        let perms = state.permissions(g, ch);
        (
            Some(g.to_string()),
            perms.has(p::permissions::USE_EXTERNAL_STICKERS),
        )
    }

    fn sticker_rows(&self, query: &str, cx: &gpui::App) -> Vec<Item> {
        let (guild, external) = self.media_channel(cx);
        let global = self
            .composer
            .pickers
            .setup
            .as_ref()
            .is_some_and(|s| s.global_expressions);
        let sets = self.picker_guild_stickers(cx);
        sticker_items(&sets, query, guild.as_deref(), external, global)
    }

    fn gif_rows(
        &mut self,
        query: &str,
        typed: bool,
        cx: &mut Context<Self>,
    ) -> (Vec<Item>, Status) {
        match self.composer.pickers.setup.as_ref() {
            // The setup is on its way (asked when the list opened).
            None => return (Vec::new(), Status::Loading),
            Some(s) if !s.gif_enabled => return (Vec::new(), Status::NoGifs),
            Some(_) => {}
        }
        let text = query.trim();
        if text.is_empty() {
            // Nothing typed yet: what is trending.
            self.composer.pickers.commands.timer = None;
            self.composer.pickers.commands.gifs.cancel_search();
            self.gif_trending_needed();
            return match self.composer.pickers.gifs.trending_result() {
                Some(Ok(g)) => (gif_items(g), Status::Done),
                Some(Err(e)) => (Vec::new(), Status::Failed(e.clone())),
                None => (Vec::new(), Status::Loading),
            };
        }
        if typed {
            let list = &mut self.composer.pickers.commands;
            if list.gifs.query_changed(text, Instant::now()) {
                list.timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                    let _ = this.update(cx, |this, cx| this.media_gif_due(cx));
                }));
            }
        }
        match self.composer.pickers.commands.gifs.search_for(text) {
            Some(Ok(g)) => (gif_items(g), Status::Done),
            Some(Err(e)) => (Vec::new(), Status::Failed(e.clone())),
            None => (Vec::new(), Status::Loading),
        }
    }

    /// The typing stopped: the newest text is searched for.
    fn media_gif_due(&mut self, cx: &mut Context<Self>) {
        let Some((nonce, query)) = self.composer.pickers.commands.gifs.take_due(Instant::now())
        else {
            return;
        };
        self.backend.send(ComposerCmd::Pickers(Cmd::SearchGifs {
            query,
            locale: locale(),
            nonce,
        }));
        cx.notify();
    }

    /// A row was picked: a GIF is told to the instance (its link goes out through the
    /// composer), a sticker or saved item is sent here.
    fn media_picked(&mut self, kind: MediaCommand, index: usize, channel_id: String) {
        let list = &mut self.composer.pickers.commands;
        let query = list.query.trim().to_owned();
        let item = list.items.get(index).cloned();
        list.leave();
        match (kind, item) {
            (MediaCommand::Gif, Some(Item::Gif(gif))) => {
                self.backend.send(ComposerCmd::Pickers(Cmd::SharedGif {
                    gif,
                    query: (!query.is_empty()).then_some(query),
                    locale: locale(),
                }));
            }
            (MediaCommand::Sticker, Some(Item::Sticker(s))) => {
                self.backend.send(ComposerCmd::Pickers(Cmd::SendSticker {
                    channel_id,
                    sticker_id: s.id,
                    nonce: p::new_nonce(),
                }));
            }
            (MediaCommand::Saved, Some(Item::Meme(m))) => {
                self.backend.send(ComposerCmd::Pickers(Cmd::SendMeme {
                    channel_id,
                    meme_id: m.id.clone(),
                    nonce: p::new_nonce(),
                }));
            }
            _ => {}
        }
    }

    /// The list above the composer while a media command is typed.
    pub(crate) fn render_media_list(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.pickers_follow_account(cx);
        let view = self.composer.view.clone();
        let (kind, selected, _) = view.read(cx).media_list()?;
        let heading = match kind {
            MediaCommand::Gif => t!("composer-ac-gifs"),
            MediaCommand::Sticker => t!("composer-ac-stickers"),
            MediaCommand::Saved => t!("composer-ac-saved"),
        };
        let items = self.composer.pickers.commands.items.clone();
        let mut body = div().flex().flex_col();
        if items.is_empty() {
            let text = match &self.composer.pickers.commands.status {
                Status::Loading => t!("media-list-loading"),
                Status::Failed(e) => t!("media-list-failed", error = e.as_str()),
                Status::NoGifs => t!("media-list-gifs-off"),
                Status::Done => match kind {
                    MediaCommand::Gif => t!("media-list-none-gifs"),
                    MediaCommand::Sticker => t!("media-list-none-stickers"),
                    MediaCommand::Saved => t!("media-list-none-saved"),
                },
            };
            body = body.child(
                div()
                    .px(px(8.))
                    .py(px(10.))
                    .text_size(px(14.))
                    .text_color(theme::text_muted())
                    .child(text),
            );
        }
        for (i, item) in items.iter().enumerate() {
            body = body.child(self.render_media_row(i, item, i == selected, &view, cx));
        }
        let frame = div()
            .id("media-list")
            .occlude()
            .w(px(PICKER_WIDTH))
            .flex()
            .flex_col()
            .p(px(8.))
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
            .child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_tertiary())
                    .child(heading),
            )
            .child(body);
        // The same place as the pickers: above the composer, left of the member list.
        Some(
            div()
                .absolute()
                .right(px(COMPOSER_RIGHT
                    + self
                        .member_list_shown()
                        .map_or(0., |_| crate::ui::members::WIDTH)))
                .bottom(px(COMPOSER_BOTTOM))
                .child(frame)
                .into_any_element(),
        )
    }

    fn render_media_row(
        &self,
        ix: usize,
        item: &Item,
        selected: bool,
        view: &gpui::Entity<crate::ui::composer::ComposerView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (thumb, title) = self.media_thumb(item, cx);
        let view = view.clone();
        div()
            .id(SharedString::from(format!("media-row-{ix}")))
            .h(px(ROW_H))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::selected_bg()))
            .hover(|s| s.bg(theme::floating_hover()))
            .child(thumb)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_strong())
                    .child(title),
            )
            .on_click(cx.listener(move |_, _, window, cx| {
                view.update(cx, |v, cx| v.pick(ix, window, cx));
            }))
            .into_any_element()
    }

    /// A row's picture (a still, from the media proxy) and its title.
    fn media_thumb(&self, item: &Item, cx: &mut Context<Self>) -> (AnyElement, String) {
        let Some(setup) = self.media.setup.as_ref() else {
            return (
                placeholder(THUMB, 4., false),
                match item {
                    Item::Gif(g) => gif_title(g),
                    Item::Sticker(s) => s.name.clone(),
                    Item::Meme(m) => crate::model::clean_name(&m.name),
                },
            );
        };
        let proxy = setup.proxy.clone();
        let scale = self.media.scale;
        match item {
            Item::Gif(g) => {
                let g = g.clone();
                let (_, still) = gif_requests(|t| g.preview(&proxy, t), scale);
                let shown = self.moving(None, still.as_ref(), cx);
                let anim = self.media.images.animator(Motion::Gif);
                (
                    picture_of(shown, THUMB, 4., ObjectFit::Cover, &anim),
                    gif_title(&g),
                )
            }
            Item::Sticker(s) => {
                let prefs = self.media.images.animations().borrow().prefs;
                let (_, still) = sticker_requests(&proxy, s, &prefs, scale);
                let shown = self.moving(None, still.as_ref(), cx);
                let anim = self.media.images.animator(Motion::Sticker);
                (
                    picture_of(shown, (THUMB.1, THUMB.1), 0., ObjectFit::Contain, &anim),
                    s.name.clone(),
                )
            }
            Item::Meme(m) => {
                let req = meme::preview(&proxy, m, scale);
                let shown = self.moving(None, req.as_ref(), cx);
                let anim = self.media.images.animator(Motion::Gif);
                (
                    picture_of(shown, THUMB, 4., ObjectFit::Cover, &anim),
                    crate::model::clean_name(&m.name),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::composer::pickers::sticker::GuildStickers;
    use serde_json::json;

    fn gif(id: &str, title: &str, url: &str) -> p::Gif {
        serde_json::from_value(json!({
            "id": id, "title": title, "url": url, "slug": id,
            "provider": "klipy",
            "proxy_src": format!("https://media.example.org/external/{id}")
        }))
        .unwrap()
    }

    fn meme(id: &str, name: &str, tags: &[&str]) -> p::Meme {
        serde_json::from_value(json!({
            "id": id, "name": name, "tags": tags,
            "url": format!("https://media.example.org/attachments/1/{id}/m.png")
        }))
        .unwrap()
    }

    fn set(guild: &str, names: &[(&str, &[&str])]) -> GuildStickers {
        GuildStickers {
            guild: guild.into(),
            name: format!("Guild {guild}"),
            stickers: names
                .iter()
                .enumerate()
                .map(|(i, (n, tags))| {
                    (
                        Sticker {
                            id: format!("{guild}0{i}"),
                            name: (*n).into(),
                            animated: false,
                        },
                        tags.iter().map(|t| t.to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn a_gif_row_sends_its_link_and_skips_gifs_without_one() {
        let gifs = vec![
            gif("a", "Dancing cat", "https://klipy.example/gifs/dancing-cat"),
            gif("b", "", "javascript:alert(1)"),
            gif("c", "No link", ""),
            gif("d", "Has space", "https://klipy.example/a b"),
        ];
        let items = gif_items(&gifs);
        assert_eq!(items.len(), 1, "only a plain http(s) link can be sent");
        let rows = candidates(&items);
        assert_eq!(rows[0].title, "Dancing cat");
        assert_eq!(
            rows[0].pick,
            Pick::Media {
                index: 0,
                text: Some("https://klipy.example/gifs/dancing-cat".into())
            }
        );
    }

    #[test]
    fn a_gif_without_a_title_is_named_by_its_address() {
        let g = gif("a", "", "https://klipy.example/gifs/happy-dog_jumps");
        assert_eq!(gif_title(&g), "happy dog jumps");
    }

    #[test]
    fn the_lists_show_at_most_ten_rows() {
        let gifs: Vec<p::Gif> = (0..25)
            .map(|i| gif(&i.to_string(), "g", &format!("https://klipy.example/g/{i}")))
            .collect();
        assert_eq!(gif_items(&gifs).len(), MAX_ROWS);
        let memes: Vec<p::Meme> = (0..25).map(|i| meme(&i.to_string(), "m", &[])).collect();
        assert_eq!(meme_items(&memes, "").len(), MAX_ROWS);
    }

    #[test]
    fn saved_media_is_found_by_name_or_tag_and_sent_by_id() {
        let memes = vec![meme("1", "Cat", &["funny"]), meme("2", "Dog", &["cute"])];
        let items = meme_items(&memes, "cute");
        let rows = candidates(&items);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Dog");
        assert_eq!(
            rows[0].pick,
            Pick::Media {
                index: 0,
                text: None
            }
        );
        assert!(matches!(&items[0], Item::Meme(m) if m.id == "2"));
    }

    #[test]
    fn stickers_the_account_may_not_use_are_not_offered() {
        let sets = vec![
            set("100", &[("Wave", &["hello"]), ("Party", &[])]),
            set("200", &[("Wave2", &[])]),
        ];
        let names = |items: &[Item]| -> Vec<String> {
            items
                .iter()
                .filter_map(|i| match i {
                    Item::Sticker(s) => Some(s.name.clone()),
                    _ => None,
                })
                .collect()
        };
        // In guild 100 without the right to use others': only its own.
        let own = sticker_items(&sets, "wave", Some("100"), false, false);
        assert_eq!(names(&own), ["Wave"]);
        // With the right and the instance's global expressions: both.
        let both = sticker_items(&sets, "wave", Some("100"), true, true);
        assert_eq!(names(&both), ["Wave", "Wave2"]);
        // Tags count.
        let tagged = sticker_items(&sets, "hello", Some("100"), false, false);
        assert_eq!(names(&tagged), ["Wave"]);
        // In a direct message only global expressions work.
        assert!(sticker_items(&sets, "", None, false, false).is_empty());
        assert_eq!(sticker_items(&sets, "", None, false, true).len(), 3);
    }

    #[test]
    fn leaving_the_command_drops_the_rows_and_the_waiting_search() {
        let mut list = MediaList {
            kind: Some(MediaCommand::Gif),
            query: "cats".into(),
            items: gif_items(&[gif("a", "x", "https://klipy.example/g/a")]),
            ..MediaList::default()
        };
        let (nonce, _) = list.gifs.start_search("cats".into());
        assert!(list.waits_for(nonce));
        list.leave();
        assert!(!list.is_open());
        assert!(list.items().is_empty());
        assert!(!list.waits_for(nonce), "an answer after leaving is dropped");
    }

    #[test]
    fn only_the_newest_search_of_the_list_is_shown() {
        let mut list = MediaList::default();
        let (first, _) = list.gifs.start_search("ca".into());
        let (second, _) = list.gifs.start_search("cat".into());
        // The first answer arrives after the second search started: dropped.
        list.gifs_answered(
            first,
            Ok(vec![gif("x", "old", "https://klipy.example/g/x")]),
        );
        assert!(list.gifs.search_for("ca").is_none());
        list.gifs_answered(
            second,
            Ok(vec![gif("y", "new", "https://klipy.example/g/y")]),
        );
        assert!(list.gifs.search_for("cat").is_some());
        assert!(list.gifs.search_for("ca").is_none());
    }
}
