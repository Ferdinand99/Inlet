// SPDX-License-Identifier: AGPL-3.0-or-later
//! The preview of a text or CSV attachment under its message (Fluxer's
//! `TextualAttachmentPreview`): the first lines of the file, or its first rows as a
//! table, with a button to show up to 100 of them.
//!
//! The file comes from the media proxy like every attachment, at most
//! [`text::MAX_BYTES`] of it (see `backend::media::text`), and is only ever drawn as plain
//! text: nothing in it is run, opened or read as markup, and lines and cells are cut. The
//! panel keeps one size while it loads and collapsed, so the message list does not jump;
//! it asks the list to measure the message again when it changes.
//!
//! Deviation: Fluxer colours code by language and lets the reader pick the language;
//! this shows plain text in the monospace font.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, Context, SharedString};

use super::attachment::FILE_CARD_WIDTH;
use crate::app::Inlet;
use crate::backend::media::text::{self, TextError};
use crate::model::Attachment;
use crate::theme;
use crate::ui::chat::messages::Row;
use crate::ui::markdown;

/// Lines (or rows) shown collapsed.
pub const COLLAPSED_LINES: usize = 6;
/// Lines (or rows) shown expanded, at most (Fluxer's `MAX_EXPANDED_PREVIEW_LINES`).
pub const EXPANDED_LINES: usize = 100;
/// Texts kept; the oldest is let go and loads again on a click.
const MAX_BODIES: usize = 48;
/// Files remembered as fetched (their small states), so none is fetched twice on its own.
const MAX_SEEN: usize = 4096;
const LINE_H: f32 = 16.;
const PADDING: f32 = 8.;
const FOOTER_H: f32 = 26.;
const PANEL_WIDTH: f32 = 520.;

/// What a preview shows: the first lines of a file, or its first rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Lines {
        /// At most [`EXPANDED_LINES`], each cut for display.
        lines: Vec<String>,
        /// Lines in the whole file.
        total: usize,
    },
    Table {
        rows: Vec<Vec<String>>,
        total: usize,
    },
}

impl Body {
    /// Reads the shown parts of a file's text.
    pub fn of(content: &str, csv: bool) -> Body {
        if csv {
            let all = text::parse_csv(content);
            let total = all.len();
            let rows = all
                .into_iter()
                .take(EXPANDED_LINES)
                .map(|r| {
                    r.iter()
                        .map(|c| text::cut(&c.replace(['\n', '\r'], " "), text::MAX_CELL_CHARS))
                        .collect()
                })
                .collect();
            Body::Table { rows, total }
        } else {
            let all = text::lines(content);
            let total = all.len();
            let lines = all
                .into_iter()
                .take(EXPANDED_LINES)
                .map(|l| text::cut(l, text::MAX_LINE_CHARS))
                .collect();
            Body::Lines { lines, total }
        }
    }

    /// Lines or rows in the whole file.
    pub fn total(&self) -> usize {
        match self {
            Body::Lines { total, .. } | Body::Table { total, .. } => *total,
        }
    }

    /// Lines or rows kept for showing.
    fn kept(&self) -> usize {
        match self {
            Body::Lines { lines, .. } => lines.len(),
            Body::Table { rows, .. } => rows.len(),
        }
    }

    /// Whether the button for more makes sense.
    pub fn can_expand(&self) -> bool {
        self.total() > COLLAPSED_LINES
    }

    /// How many lines or rows show, collapsed or expanded.
    pub fn shown(&self, expanded: bool) -> usize {
        self.kept().min(if expanded {
            EXPANDED_LINES
        } else {
            COLLAPSED_LINES
        })
    }
}

/// Why a preview is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failed {
    TooLarge,
    Unavailable,
}

#[derive(Clone, Debug)]
enum Entry {
    Loading,
    Ready(Arc<Body>),
    Failed(Failed),
    /// Fetched before, and its text was let go to keep memory down: a click fetches it
    /// again. It is never fetched again on its own, however often it is drawn.
    Dropped,
}

/// The previews of the session, by attachment ID, and which of them are expanded.
///
/// Every file is fetched at most once on its own: its ID stays in `entries` (a small
/// state, at most [`MAX_SEEN`] of them) after its text is let go, so drawing the same
/// message again and again, or drawing more files than the cache holds, never starts a
/// fetch. Only [`MAX_BODIES`] texts are kept; the oldest becomes [`Entry::Dropped`].
#[derive(Default)]
pub struct Previews {
    entries: std::cell::RefCell<HashMap<String, Entry>>,
    /// Every ID in `entries`, oldest first.
    seen: std::cell::RefCell<VecDeque<String>>,
    /// The IDs whose text is kept, oldest first.
    bodies: std::cell::RefCell<VecDeque<String>>,
    expanded: std::cell::RefCell<HashSet<String>>,
}

impl Previews {
    fn get(&self, id: &str) -> Option<Entry> {
        self.entries.borrow().get(id).cloned()
    }

    /// Whether this call made the entry: only then is the file fetched.
    fn start(&self, id: &str) -> bool {
        let mut entries = self.entries.borrow_mut();
        if entries.contains_key(id) {
            return false;
        }
        entries.insert(id.to_owned(), Entry::Loading);
        let mut seen = self.seen.borrow_mut();
        seen.push_back(id.to_owned());
        while seen.len() > MAX_SEEN {
            if let Some(old) = seen.pop_front() {
                entries.remove(&old);
                self.expanded.borrow_mut().remove(&old);
                self.bodies.borrow_mut().retain(|b| *b != old);
            }
        }
        true
    }

    /// A click on a dropped preview: it loads again. `false` when it is not dropped.
    fn restart(&self, id: &str) -> bool {
        let mut entries = self.entries.borrow_mut();
        match entries.get_mut(id) {
            Some(entry @ Entry::Dropped) => {
                *entry = Entry::Loading;
                true
            }
            _ => false,
        }
    }

    fn finish(&self, id: &str, result: Result<Body, Failed>) {
        let mut entries = self.entries.borrow_mut();
        // Forgotten while it loaded (past the limit of remembered files): nothing to fill.
        let Some(entry) = entries.get_mut(id) else {
            return;
        };
        *entry = match result {
            Ok(body) => {
                let mut bodies = self.bodies.borrow_mut();
                bodies.push_back(id.to_owned());
                Entry::Ready(Arc::new(body))
            }
            Err(why) => Entry::Failed(why),
        };
        // Past the kept texts, the oldest is let go (and says so with a click to load it).
        let mut bodies = self.bodies.borrow_mut();
        while bodies.len() > MAX_BODIES {
            if let Some(old) = bodies.pop_front() {
                if let Some(e) = entries.get_mut(&old) {
                    *e = Entry::Dropped;
                }
                self.expanded.borrow_mut().remove(&old);
            }
        }
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.borrow().contains(id)
    }

    fn toggle(&self, id: &str) {
        let mut set = self.expanded.borrow_mut();
        if !set.remove(id) {
            set.insert(id.to_owned());
        }
    }

    /// Forgets everything (the session ended).
    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
        self.seen.borrow_mut().clear();
        self.bodies.borrow_mut().clear();
        self.expanded.borrow_mut().clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    #[cfg(test)]
    fn kept_bodies(&self) -> usize {
        self.bodies.borrow().len()
    }
}

/// The height of a panel in a state: the same while loading and collapsed, so the
/// message does not change size when the file arrives.
pub fn panel_height(body: Option<&Body>, expanded: bool, line_h: f32) -> f32 {
    let lines = match body {
        Some(b) if expanded => b.shown(true).max(1),
        _ => COLLAPSED_LINES,
    };
    2. * PADDING + lines as f32 * line_h + FOOTER_H
}

impl Inlet {
    /// The preview under a file's card, if the file gets one. Starts the fetch the first
    /// time it is drawn.
    pub(super) fn text_preview(
        &self,
        message_id: &str,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let setup = self.media.setup.as_ref()?;
        // A spoiler file is covered by `file_cover` until it is revealed, so a preview that is
        // asked for here is for a visible file.
        if a.expired || !text::previewable(a.content_type.as_deref(), &a.filename, a.size) {
            return None;
        }
        let url = super::requests::attachment_original(&setup.proxy, a)?;
        let csv = text::is_csv(a.content_type.as_deref(), &a.filename);
        let previews = &self.media.texts;
        if previews.start(&a.id) {
            self.fetch_text_preview(a.id.clone(), message_id.to_owned(), url.clone(), csv, cx);
        }
        let entry = previews.get(&a.id)?;
        let width = self
            .fit_column((PANEL_WIDTH.max(FILE_CARD_WIDTH), 0.), 0.)
            .0;
        let expanded = previews.is_expanded(&a.id);
        let panel = div()
            .id(SharedString::from(format!(
                "text-preview-{message_id}-{}",
                a.id
            )))
            .w(px(width))
            .max_w_full()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_color(theme::accent_border())
            .bg(theme::code_bg())
            .overflow_hidden();
        Some(match entry {
            Entry::Loading => panel
                .h(px(panel_height(None, false, LINE_H)))
                .p(px(PADDING))
                .text_size(px(12.))
                .text_color(theme::text_muted())
                .child(t!("media-text-loading"))
                .into_any_element(),
            Entry::Dropped => {
                let (id, mid, url) = (a.id.clone(), message_id.to_owned(), url.clone());
                panel
                    .h(px(panel_height(None, false, LINE_H)))
                    .p(px(PADDING))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(theme::link())
                    .child(t!("media-text-show"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.media.texts.restart(&id) {
                            this.fetch_text_preview(id.clone(), mid.clone(), url.clone(), csv, cx);
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            }
            Entry::Failed(why) => panel
                .p(px(PADDING))
                .text_size(px(12.))
                .text_color(theme::text_muted())
                .child(match why {
                    Failed::TooLarge => t!("media-text-too-large"),
                    Failed::Unavailable => t!("media-text-unavailable"),
                })
                .into_any_element(),
            Entry::Ready(body) => {
                let height = panel_height(Some(&body), expanded, LINE_H);
                let shown = body.shown(expanded);
                let content = match &*body {
                    Body::Lines { lines, .. } => lines_view(&lines[..shown]),
                    Body::Table { rows, .. } => table_view(&rows[..shown]),
                };
                // Only a longer file has more to show.
                let toggle = body.can_expand().then(|| {
                    let (id, mid) = (a.id.clone(), message_id.to_owned());
                    div()
                        .id(SharedString::from(format!("text-preview-more-{mid}-{id}")))
                        .cursor_pointer()
                        .text_color(theme::link())
                        .child(if expanded {
                            t!("media-text-less")
                        } else {
                            t!("media-text-more")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.media.texts.toggle(&id);
                            this.remeasure_message(&mid);
                            cx.notify();
                        }))
                });
                let count = match &*body {
                    Body::Lines { total, .. } => t!("media-text-lines", count = *total),
                    Body::Table { total, .. } => t!("media-text-rows", count = *total),
                };
                panel
                    .h(px(height))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .p(px(PADDING))
                            .child(content),
                    )
                    .child(
                        div()
                            .h(px(FOOTER_H))
                            .px(px(PADDING))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .border_color(theme::accent_border())
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(count)
                            .children(toggle),
                    )
                    .into_any_element()
            }
        })
    }

    /// Fetches a file's text from the media proxy in the background; the answer fills its
    /// entry and the message is measured again.
    fn fetch_text_preview(
        &self,
        id: String,
        message_id: String,
        url: url::Url,
        csv: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(setup) = self.media.setup.as_ref() else {
            return;
        };
        let (fetcher, runtime) = (setup.fetcher.clone(), setup.runtime.clone());
        cx.spawn(async move |this, cx| {
            let result = runtime.spawn(text::fetch_text(fetcher, url)).await;
            let result = match result {
                Ok(Ok(content)) => Ok(Body::of(&content, csv)),
                Ok(Err(TextError::TooLarge)) => Err(Failed::TooLarge),
                Ok(Err(TextError::Fetch(e))) => {
                    log::debug!("a text preview could not be fetched: {e}");
                    Err(Failed::Unavailable)
                }
                Err(_) => Err(Failed::Unavailable),
            };
            let _ = this.update(cx, |this, cx| {
                this.media.texts.finish(&id, result);
                this.remeasure_message(&message_id);
                cx.notify();
            });
        })
        .detach();
    }

    /// The message's row is measured again (its preview changed size).
    pub(crate) fn remeasure_message(&mut self, message_id: &str) {
        let Some(id) = inlet_state::MessageId::parse(message_id) else {
            return;
        };
        let at = self
            .chat
            .view
            .rows
            .iter()
            .position(|r| matches!(r, Row::Message(m) if m.id == id));
        if let Some(ix) = at {
            self.chat.message_list.splice(ix..ix + 1, 1);
        }
    }
}

/// Plain lines in the monospace font, each cut off at the box's edge.
fn lines_view(lines: &[String]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .font_family(markdown::mono_font())
        .text_size(px(12.))
        .text_color(theme::text())
        .children(lines.iter().map(|l| {
            div()
                .h(px(LINE_H))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(if l.is_empty() {
                    SharedString::from(" ")
                } else {
                    SharedString::from(l.clone())
                })
        }))
        .into_any_element()
}

/// Rows of equal columns; the first row is the header.
fn table_view(rows: &[Vec<String>]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .text_size(px(12.))
        .text_color(theme::text())
        .children(rows.iter().enumerate().map(|(i, row)| {
            div()
                .h(px(LINE_H))
                .flex()
                .flex_row()
                .gap(px(12.))
                .when(i == 0, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .children(row.iter().map(|cell| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(SharedString::from(cell.clone()))
                }))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_file_keeps_its_first_hundred_lines_cut_to_length() {
        let content: String = (0..250)
            .map(|i| {
                if i == 3 {
                    "x".repeat(2_000)
                } else {
                    format!("line {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let body = Body::of(&content, false);
        let Body::Lines { lines, total } = &body else {
            panic!("lines")
        };
        assert_eq!(*total, 250);
        assert_eq!(lines.len(), EXPANDED_LINES);
        assert!(lines[3].chars().count() <= text::MAX_LINE_CHARS + 1);
        assert_eq!(body.shown(false), COLLAPSED_LINES);
        assert_eq!(body.shown(true), EXPANDED_LINES);
        assert!(body.can_expand());
    }

    #[test]
    fn a_short_file_has_nothing_more_to_show() {
        let body = Body::of("a\nb\nc", false);
        assert!(!body.can_expand());
        assert_eq!(body.shown(true), 3);
        assert_eq!(body.total(), 3);
    }

    #[test]
    fn a_csv_file_is_a_table_of_cut_cells() {
        let cell = "z".repeat(1_000);
        let content = format!("name,note\nAda,\"{cell}\"\nGrace,\"two\nlines\"");
        let Body::Table { rows, total } = Body::of(&content, true) else {
            panic!("table")
        };
        assert_eq!(total, 3);
        assert_eq!(rows[0], ["name", "note"]);
        assert!(rows[1][1].chars().count() <= text::MAX_CELL_CHARS + 1);
        assert_eq!(rows[2][1], "two lines", "a line break in a cell is a space");
    }

    #[test]
    fn a_panel_keeps_its_size_from_loading_to_collapsed() {
        let long = Body::of(&"l\n".repeat(50), false);
        let short = Body::of("one", false);
        let loading = panel_height(None, false, LINE_H);
        assert_eq!(panel_height(Some(&long), false, LINE_H), loading);
        assert_eq!(panel_height(Some(&short), false, LINE_H), loading);
        assert!(panel_height(Some(&long), true, LINE_H) > loading);
        // Expanded, a long file is capped at a hundred lines.
        let huge = Body::of(&"l\n".repeat(500), false);
        assert_eq!(
            panel_height(Some(&huge), true, LINE_H),
            2. * PADDING + EXPANDED_LINES as f32 * LINE_H + FOOTER_H
        );
    }

    #[test]
    fn a_preview_is_fetched_once_and_never_again_on_its_own() {
        let p = Previews::default();
        assert!(p.start("a"));
        assert!(!p.start("a"), "asked again: no second fetch");
        p.finish("a", Ok(Body::of("hi", false)));
        assert!(matches!(p.get("a"), Some(Entry::Ready(_))));
        p.toggle("a");
        assert!(p.is_expanded("a"));
        // More files than texts are kept: the oldest text is let go, its entry stays.
        for i in 0..MAX_BODIES {
            assert!(p.start(&format!("f{i}")));
            p.finish(&format!("f{i}"), Ok(Body::of("x", false)));
        }
        assert_eq!(p.kept_bodies(), MAX_BODIES);
        assert!(matches!(p.get("a"), Some(Entry::Dropped)));
        assert!(!p.is_expanded("a"));
        assert!(
            !p.start("a"),
            "a dropped preview is not fetched again by drawing it"
        );
        // Only a click loads it again, once.
        assert!(p.restart("a"));
        assert!(!p.restart("a"));
        p.finish("a", Ok(Body::of("again", false)));
        assert!(matches!(p.get("a"), Some(Entry::Ready(_))));
        p.clear();
        assert_eq!(p.len(), 0);
    }

    /// Drawing more previewable files than the cache keeps, again and again, makes one
    /// fetch per file and no more.
    #[test]
    fn drawing_sixty_files_twice_fetches_each_at_most_once() {
        let p = Previews::default();
        let mut fetches = 0;
        for _frame in 0..2 {
            for i in 0..60 {
                let id = format!("file{i}");
                if p.start(&id) {
                    fetches += 1;
                    p.finish(&id, Ok(Body::of("text", false)));
                }
            }
        }
        assert_eq!(fetches, 60);
        assert!(p.kept_bodies() <= MAX_BODIES);
    }

    #[test]
    fn a_failed_preview_says_why_and_is_not_asked_again() {
        let p = Previews::default();
        assert!(p.start("x"));
        p.finish("x", Err(Failed::TooLarge));
        assert!(matches!(p.get("x"), Some(Entry::Failed(Failed::TooLarge))));
        assert!(!p.start("x"));
    }
}
