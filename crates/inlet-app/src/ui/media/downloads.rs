// SPDX-License-Identifier: AGPL-3.0-or-later
//! The downloads panel (Fluxer's `NativeDownloads`): a button in the channel header once a
//! file has been saved from a message, and a list under it with each download's name,
//! progress and state, a button to cancel one that runs, one to remove a finished one
//! and "Clear list".
//!
//! A download always starts in the save dialog (the person picks where the file goes) and
//! is only ever written there: nothing is opened or run, now or later. "Show in folder"
//! asks the file manager to show the file, and only when it is clicked.
//!
//! Deviation: Fluxer also opens a finished file when its name is clicked. Inlet does not:
//! a file from a message can be a program, and the folder is one click away.
//!
//! The panel keeps the last [`MAX_ENTRIES`] downloads of this run; nothing about them is
//! stored.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{div, point, prelude::*, px, AnyElement, Context, FontWeight, SharedString, Task};

use super::{file_size, MediaIcon};
use crate::app::Inlet;
use crate::backend::media::SaveProgress;
use crate::theme;
use crate::ui::HEADER_HEIGHT;

/// Downloads the list keeps; the oldest finished ones go first.
pub const MAX_ENTRIES: usize = 50;
/// How often a running download redraws the panel.
const TICK: Duration = Duration::from_millis(250);
/// A click on the button right after a click outside closed the panel is that click.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
const PANEL_WIDTH: f32 = 360.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    Done,
    Cancelled,
    Failed(String),
}

pub struct Entry {
    pub id: u64,
    /// The file's name as saved, shortened for display.
    pub name: String,
    pub path: PathBuf,
    pub progress: Arc<SaveProgress>,
    pub status: Status,
    /// A finished file that is no longer where it was saved (checked when the panel opens).
    pub missing: bool,
}

#[derive(Default)]
pub struct Downloads {
    entries: Vec<Entry>,
    next: u64,
    open: bool,
    closed_by_click: Option<Instant>,
    ticker: Option<Task<()>>,
}

impl Downloads {
    /// A download starts: its entry, newest first. At the limit the oldest finished one
    /// goes; a list of only running ones stops at the limit by dropping the oldest.
    pub fn begin(&mut self, name: &str, path: PathBuf, progress: Arc<SaveProgress>) -> u64 {
        let id = self.next;
        self.next += 1;
        self.entries.insert(
            0,
            Entry {
                id,
                name: crate::model::clean_name(name),
                path,
                progress,
                status: Status::Active,
                missing: false,
            },
        );
        // Only a finished entry goes: a running download always keeps its place, so it can
        // still be cancelled (the list grows past the limit only by running ones).
        while self.entries.len() > MAX_ENTRIES {
            let Some(at) = self
                .entries
                .iter()
                .rposition(|e| e.status != Status::Active)
            else {
                break;
            };
            self.entries.remove(at);
        }
        id
    }

    /// A download ended.
    pub fn finish(&mut self, id: u64, status: Status) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.status = status;
        }
    }

    /// Stops a running download; its entry says "cancelled" when the task has stopped.
    pub fn cancel(&self, id: u64) {
        if let Some(e) = self
            .entries
            .iter()
            .find(|e| e.id == id && e.status == Status::Active)
        {
            e.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.entries
            .retain(|e| e.id != id || e.status == Status::Active);
    }

    /// Takes the finished ones out; running ones stay.
    pub fn clear_finished(&mut self) {
        self.entries.retain(|e| e.status == Status::Active);
    }

    pub fn active(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == Status::Active)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Demo scenes: three made-up downloads (one on its way, one done, one failed) and the
    /// panel open. Once.
    #[cfg(feature = "demo")]
    pub fn demo_fill(&mut self) {
        if !self.entries.is_empty() {
            return;
        }
        let progress = |received: u64, total: u64| {
            let p = Arc::new(SaveProgress::default());
            p.received.store(received, Ordering::Relaxed);
            p.total.store(total, Ordering::Relaxed);
            p
        };
        let failed = self.begin(
            "broken-archive.7z",
            PathBuf::from("broken-archive.7z"),
            progress(0, 0),
        );
        let done = self.begin(
            "meeting-notes.pdf",
            PathBuf::from("meeting-notes.pdf"),
            progress(1, 1),
        );
        let _running = self.begin(
            "holiday-photos.zip",
            PathBuf::from("holiday-photos.zip"),
            progress(3_500_000, 8_400_000),
        );
        self.finish(failed, Status::Failed("the connection was reset".into()));
        self.finish(done, Status::Done);
        self.open = true;
    }

    fn mark_missing(&mut self) {
        for e in &mut self.entries {
            if e.status == Status::Done {
                e.missing = !e.path.exists();
            }
        }
    }
}

/// What the line under a download's name says.
pub fn meta(entry: &Entry) -> String {
    match &entry.status {
        Status::Done if entry.missing => t!("media-dl-moved"),
        Status::Done => t!("media-dl-done"),
        Status::Cancelled => t!("media-dl-cancelled"),
        Status::Failed(reason) => t!("media-dl-failed", reason = reason.as_str()),
        Status::Active => {
            let received = entry.progress.received.load(Ordering::Relaxed);
            let total = entry.progress.total.load(Ordering::Relaxed);
            match received.saturating_mul(100).checked_div(total) {
                Some(percent) => t!(
                    "media-dl-progress",
                    percent = percent.min(100).to_string(),
                    total = file_size(total)
                ),
                None if received > 0 => file_size(received),
                None => t!("media-dl-starting"),
            }
        }
    }
}

/// The fraction (0 to 1) of a running download, or `None` when its size is not known.
pub fn fraction(entry: &Entry) -> Option<f32> {
    let total = entry.progress.total.load(Ordering::Relaxed);
    (total > 0).then(|| {
        (entry.progress.received.load(Ordering::Relaxed) as f32 / total as f32).clamp(0., 1.)
    })
}

/// The panel's button in the channel header, once there is a download to show (see
/// `ui/chat/header_buttons.rs`).
pub fn header_button(this: &Inlet, cx: &mut Context<Inlet>) -> Option<AnyElement> {
    Inlet::downloads_button(this, cx)
}

impl Inlet {
    /// A save started: it gets an entry in the panel and a ticker that redraws it.
    pub(crate) fn download_started(
        &mut self,
        name: &str,
        path: PathBuf,
        progress: Arc<SaveProgress>,
        cx: &mut Context<Self>,
    ) -> u64 {
        let id = self.media.downloads.begin(name, path, progress);
        self.keep_download_ticker(cx);
        cx.notify();
        id
    }

    /// A save ended: the entry says how.
    pub(crate) fn download_ended(&mut self, id: u64, status: Status, cx: &mut Context<Self>) {
        self.media.downloads.finish(id, status);
        cx.notify();
    }

    /// Redraws while a download runs, so its percentage moves. It stops by itself.
    fn keep_download_ticker(&mut self, cx: &mut Context<Self>) {
        if self.media.downloads.ticker.is_some() {
            return;
        }
        self.media.downloads.ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let more = this
                .update(cx, |this, cx| {
                    cx.notify();
                    let more = this.media.downloads.active() > 0;
                    if !more {
                        this.media.downloads.ticker = None;
                    }
                    more
                })
                .unwrap_or(false);
            if !more {
                break;
            }
        }));
    }

    /// The button as a function of the window (see `header_button`).
    fn downloads_button(this: &Inlet, cx: &mut Context<Inlet>) -> Option<AnyElement> {
        let downloads = &this.media.downloads;
        if downloads.is_empty() {
            return None;
        }
        let active = downloads.active();
        let open = downloads.open;
        Some(
            div()
                .id("header-downloads")
                .relative()
                .flex_none()
                .p(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(open, |d| d.bg(theme::selected_bg()))
                .hover(|s| s.bg(theme::hover_bg()))
                .child(MediaIcon::Download.el(
                    px(24.),
                    if open {
                        theme::text_strong()
                    } else {
                        theme::text_muted()
                    },
                ))
                .when(active > 0, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .min_w(px(14.))
                            .h(px(14.))
                            .px(px(3.))
                            .rounded_full()
                            .bg(theme::brand())
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_strong())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(active.to_string()),
                    )
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    let d = &mut this.media.downloads;
                    // The click outside that just closed the panel was this button's own.
                    if d.closed_by_click
                        .take()
                        .is_some_and(|at| at.elapsed() < REOPEN_GUARD)
                    {
                        return;
                    }
                    d.open = !d.open;
                    if d.open {
                        d.mark_missing();
                    }
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The list under the header button while it is open.
    pub(crate) fn render_downloads_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let downloads = &self.media.downloads;
        if !downloads.open || downloads.is_empty() {
            return None;
        }
        let rows: Vec<AnyElement> = downloads
            .entries()
            .iter()
            .map(|e| render_entry(e, cx))
            .collect();
        let panel = div()
            .id("downloads-panel")
            .occlude()
            .w(px(PANEL_WIDTH))
            .max_h(px(420.))
            .flex()
            .flex_col()
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
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                let d = &mut this.media.downloads;
                d.open = false;
                d.closed_by_click = Some(Instant::now());
                cx.notify();
            }))
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_strong())
                            .child(t!("media-dl-title")),
                    )
                    .child(
                        div()
                            .id("downloads-clear")
                            .cursor_pointer()
                            .text_size(px(13.))
                            .text_color(theme::link())
                            .child(t!("media-dl-clear"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.media.downloads.clear_finished();
                                if this.media.downloads.is_empty() {
                                    this.media.downloads.open = false;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("downloads-list")
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .children(rows),
            );
        Some(
            div()
                .absolute()
                .top(px(HEADER_HEIGHT))
                .right(px(16.))
                .child(panel)
                .into_any_element(),
        )
    }
}

fn render_entry(entry: &Entry, cx: &mut Context<Inlet>) -> AnyElement {
    let id = entry.id;
    let active = entry.status == Status::Active;
    let can_reveal = entry.status == Status::Done && !entry.missing;
    let path = entry.path.clone();
    let button = |name: String, label: String| {
        div()
            .id(SharedString::from(name))
            .flex_none()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(theme::text_muted())
            .hover(|s| s.bg(theme::floating_hover()))
            .child(label)
    };
    div()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(theme::text_strong())
                .child(entry.name.clone()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme::text_muted())
                .child(meta(entry)),
        )
        .children(active.then(|| {
            // A bar for a download whose size is known.
            let frac = fraction(entry);
            div()
                .h(px(4.))
                .w_full()
                .rounded_full()
                .bg(theme::bg_textarea())
                .children(frac.map(|f| {
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(theme::brand())
                        .w(gpui::relative(f))
                }))
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(4.))
                .children(can_reveal.then(|| {
                    button(format!("download-reveal-{id}"), t!("media-dl-reveal")).on_click(
                        cx.listener(move |_, _, _, cx| {
                            // Only now, on this click: the file manager shows the file.
                            cx.reveal_path(&path);
                        }),
                    )
                }))
                .child(if active {
                    button(format!("download-cancel-{id}"), t!("media-dl-cancel")).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.media.downloads.cancel(id);
                            cx.notify();
                        }),
                    )
                } else {
                    button(format!("download-remove-{id}"), t!("media-dl-remove")).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.media.downloads.remove(id);
                            if this.media.downloads.is_empty() {
                                this.media.downloads.open = false;
                            }
                            cx.notify();
                        }),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(received: u64, total: u64) -> Arc<SaveProgress> {
        let p = Arc::new(SaveProgress::default());
        p.received.store(received, Ordering::Relaxed);
        p.total.store(total, Ordering::Relaxed);
        p
    }

    fn start(d: &mut Downloads, name: &str) -> u64 {
        d.begin(name, PathBuf::from(format!("/tmp/{name}")), progress(0, 0))
    }

    #[test]
    fn a_download_shows_progress_then_its_end() {
        let mut d = Downloads::default();
        let id = start(&mut d, "a.zip");
        assert_eq!(d.active(), 1);
        assert_eq!(meta(&d.entries()[0]), t!("media-dl-starting"));
        d.entries[0].progress.received.store(500, Ordering::Relaxed);
        assert_eq!(meta(&d.entries()[0]), file_size(500));
        d.entries[0].progress.total.store(2_000, Ordering::Relaxed);
        assert_eq!(
            meta(&d.entries()[0]),
            t!(
                "media-dl-progress",
                percent = "25".to_string(),
                total = file_size(2_000)
            )
        );
        assert_eq!(fraction(&d.entries()[0]), Some(0.25));
        d.finish(id, Status::Done);
        assert_eq!(d.active(), 0);
        assert_eq!(meta(&d.entries()[0]), t!("media-dl-done"));
        d.entries[0].missing = true;
        assert_eq!(meta(&d.entries()[0]), t!("media-dl-moved"));
    }

    #[test]
    fn cancelling_sets_the_flag_only_for_a_running_download() {
        let mut d = Downloads::default();
        let running = start(&mut d, "run");
        let done = start(&mut d, "done");
        d.finish(done, Status::Done);
        d.cancel(done);
        assert!(!d.entries()[0].progress.cancel.load(Ordering::Relaxed));
        d.cancel(running);
        assert!(d.entries()[1].progress.cancel.load(Ordering::Relaxed));
        d.finish(running, Status::Cancelled);
        assert_eq!(meta(&d.entries()[1]), t!("media-dl-cancelled"));
    }

    #[test]
    fn clearing_the_list_keeps_what_still_runs() {
        let mut d = Downloads::default();
        let a = start(&mut d, "a");
        let b = start(&mut d, "b");
        let c = start(&mut d, "c");
        d.finish(a, Status::Done);
        d.finish(c, Status::Failed("no space".into()));
        d.clear_finished();
        assert_eq!(d.entries().len(), 1);
        assert_eq!(d.entries()[0].id, b);
        // A running one cannot be removed, only cancelled.
        d.remove(b);
        assert_eq!(d.entries().len(), 1);
        d.finish(b, Status::Done);
        d.remove(b);
        assert!(d.is_empty());
    }

    #[test]
    fn the_list_is_capped_and_finished_ones_go_first() {
        let mut d = Downloads::default();
        let first = start(&mut d, "first");
        d.finish(first, Status::Done);
        let running = start(&mut d, "running");
        for i in 0..MAX_ENTRIES {
            let id = start(&mut d, &format!("f{i}"));
            d.finish(id, Status::Done);
        }
        assert_eq!(d.entries().len(), MAX_ENTRIES);
        assert!(
            d.entries().iter().all(|e| e.id != first),
            "oldest finished went"
        );
        assert!(
            d.entries().iter().any(|e| e.id == running),
            "a running one stays"
        );
        // A list of only running downloads is never cut: each can still be cancelled.
        let mut busy = Downloads::default();
        for i in 0..MAX_ENTRIES + 5 {
            start(&mut busy, &format!("r{i}"));
        }
        assert_eq!(busy.active(), MAX_ENTRIES + 5);
        // The newest is on top.
        assert_eq!(d.entries()[0].name, format!("f{}", MAX_ENTRIES - 1));
    }

    #[test]
    fn a_finished_file_that_is_gone_is_marked() {
        let dir = std::env::temp_dir().join(format!("inlet-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let present = dir.join("here.bin");
        std::fs::write(&present, b"x").unwrap();
        let mut d = Downloads::default();
        let a = d.begin("here.bin", present, progress(1, 1));
        let b = d.begin("gone.bin", dir.join("gone.bin"), progress(1, 1));
        d.finish(a, Status::Done);
        d.finish(b, Status::Done);
        d.mark_missing();
        let gone = d.entries().iter().find(|e| e.id == b).unwrap();
        let here = d.entries().iter().find(|e| e.id == a).unwrap();
        assert!(gone.missing && !here.missing);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
