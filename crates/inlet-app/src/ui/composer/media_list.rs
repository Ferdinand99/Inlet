// SPDX-License-Identifier: AGPL-3.0-or-later
//! `/gif`, `/sticker` and `/saved` in the composer: the text after the command is a
//! search, and a list of results opens above the box (Fluxer's autocomplete for them). The
//! composer only watches the text and the keys; the app (`pickers::commands`) owns the
//! results and their pictures, because it holds the GIF answers, the saved media, the
//! stickers and the images.
//!
//! - The composer tells the app what is searched ([`MediaEvent::Query`]) and gets the
//!   rows back ([`ComposerView::set_media_rows`]); Up, Down, Enter, Tab and Escape work as in
//!   the other lists.
//! - A GIF becomes the message (its link, sent like any message: reply, slowmode and
//!   rights apply). A sticker or a saved item is sent by the app as a message of its own,
//!   and the command text goes either way.
//! - Only the main composer has the lists (the thread panel's has no picker buttons).

use gpui::{Context, Focusable, Window};
use inlet_editor::MediaCommand;

use super::autocomplete::{Candidate, Section};
use super::editor::{AutocompleteState, ComposerEvent, ComposerView};

/// What the composer says about a media command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaEvent {
    /// The command's search text changed (or the command was just typed).
    Query { kind: MediaCommand, query: String },
    /// Row `index` was picked: the app sends the item unless it is a GIF, whose link the
    /// composer sends itself.
    Picked {
        kind: MediaCommand,
        index: usize,
        channel_id: String,
    },
    /// The command is no longer being typed: searches on their way are not wanted.
    Left,
}

impl ComposerView {
    /// The text is `/gif`, `/sticker` or `/saved` with a search text: the app is told
    /// when the search changes, and the list shows the rows it has given so far.
    pub(super) fn update_media_trigger(
        &mut self,
        kind: MediaCommand,
        query: String,
        start: usize,
        cx: &mut Context<Self>,
    ) {
        // Only the main composer has them.
        if self.pickers.is_none() {
            self.autocomplete = None;
            return;
        }
        let same = self.autocomplete.is_some()
            && self.media_query.as_ref() == Some(&(kind, query.clone()));
        if !same {
            self.media_query = Some((kind, query.clone()));
            cx.emit(ComposerEvent::Media(MediaEvent::Query { kind, query }));
        }
        let (rows, selected) = match &self.autocomplete {
            Some(open) if open.section == Section::Media(kind) => {
                (open.rows.clone(), open.selected)
            }
            _ => (Vec::new(), 0),
        };
        self.autocomplete = Some(AutocompleteState {
            section: Section::Media(kind),
            selected: selected.min(rows.len().saturating_sub(1)),
            rows,
            start,
        });
    }

    /// The text stopped being a media command.
    pub(super) fn leave_media_trigger(&mut self, cx: &mut Context<Self>) {
        if self.media_query.take().is_some() {
            cx.emit(ComposerEvent::Media(MediaEvent::Left));
        }
    }

    /// The app's rows for the search `query` of `kind`. Rows for a search the text has
    /// left behind are dropped.
    pub fn set_media_rows(
        &mut self,
        kind: MediaCommand,
        query: &str,
        rows: Vec<Candidate>,
        cx: &mut Context<Self>,
    ) {
        if self.media_query.as_ref() != Some(&(kind, query.to_owned())) {
            return;
        }
        let Some(open) = &mut self.autocomplete else {
            return;
        };
        if open.section != Section::Media(kind) {
            return;
        }
        open.selected = open.selected.min(rows.len().saturating_sub(1));
        open.rows = rows;
        cx.notify();
    }

    /// The open media list: the command, the selected row and how many rows it has.
    pub fn media_list(&self) -> Option<(MediaCommand, usize, usize)> {
        let open = self.autocomplete.as_ref()?;
        match open.section {
            Section::Media(kind) => Some((kind, open.selected, open.rows.len())),
            _ => None,
        }
    }

    /// Row `index` of the media list was picked.
    pub(super) fn pick_media(
        &mut self,
        section: Section,
        index: usize,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Section::Media(kind), Some(channel)) = (section, self.channel.clone()) else {
            return;
        };
        self.media_query = None;
        cx.emit(ComposerEvent::Media(MediaEvent::Picked {
            kind,
            index,
            channel_id: channel.channel_id,
        }));
        match text {
            Some(link) => {
                // The message becomes the link and goes out like any message.
                let put = self.editor.update(cx, |e, cx| {
                    e.clear(window, cx);
                    e.insert_text(&link, window, cx).is_ok()
                });
                if put {
                    self.submit(window, cx);
                } else {
                    self.error = Some(t!("composer-too-long", max = self.max_len.to_string()));
                }
            }
            None => self.sent(window, cx),
        }
        window.focus(&self.editor.focus_handle(cx), cx);
        cx.notify();
    }
}
