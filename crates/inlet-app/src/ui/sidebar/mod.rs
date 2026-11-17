// SPDX-License-Identifier: AGPL-3.0-or-later
//! The column next to the guild rail: guild header, channel or DM list, user panel.

mod category;
pub mod channel_list;
pub mod community_badge;
pub mod dm_list;
pub mod favorites;
pub mod floating;
pub mod guild_header;
pub mod history;
mod remembered;
pub mod resize;
mod user_panel;

pub use remembered::Remembered;

use gpui::{div, prelude::*, px, AnyElement, Context, Window};

use crate::app::Inlet;
use crate::backend::messages;
use crate::model::{self, Channel, Guild};
use crate::theme;

/// Channel list / DM list width before it is set (see `resize`).
pub const SIDEBAR_WIDTH: f32 = resize::DEFAULT_WIDTH as f32;

pub struct SidebarState {
    /// What is remembered between runs: the categories folded away in the channel list,
    /// the folders open in the rail and the content warnings accepted.
    pub remembered: Remembered,
    /// Where the person has been: the history the keys walk, the last channel of each
    /// server.
    pub history: history::History,
    /// The list's width in pixels.
    pub width: u32,
    /// The write of the width that waits for the drag to stop.
    pub width_saver: Option<gpui::Task<()>>,
    /// The scroll of the channel list, which the floating unread indicators read.
    pub scroll: gpui::ScrollHandle,
    /// The rows and view height the indicators were last made for (see `floating`).
    pub shown: (Vec<(usize, floating::Severity)>, i32),
}

impl Default for SidebarState {
    fn default() -> Self {
        let remembered = Remembered::current();
        SidebarState {
            width: remembered.sidebar_width(),
            remembered,
            history: history::History::default(),
            width_saver: None,
            scroll: gpui::ScrollHandle::new(),
            shown: (Vec::new(), 0),
        }
    }
}

impl Inlet {
    pub(crate) fn on_guilds(&mut self, guilds: Vec<Guild>, cx: &mut Context<Self>) {
        self.guilds = guilds;
        if let Some(selected) = &self.selected_guild {
            if !self.guilds.iter().any(|g| &g.id == selected) {
                self.chat.notice = Some(t!("sidebar-guild-gone"));
                self.select_guild(None, cx);
            }
        }
        // A server the person just joined opens as soon as the list has it.
        self.nav_on_guilds(cx);
        // The first time: the place the account was at when the app last ran.
        self.restore_last_place(cx);
    }

    pub(crate) fn on_channels(
        &mut self,
        guild_id: Option<String>,
        channels: Vec<Channel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if guild_id == self.selected_guild {
            self.channels = model::sidebar_order(&channels);
            self.social.dm_rows = None;
            // The DM list keeps the personal notes channel (it is not in the REST list).
            self.keep_personal_notes(cx);
            let open_still_exists = self
                .selected_channel
                .as_ref()
                .is_some_and(|id| self.channels.iter().any(|c| &c.id == id));
            if self.selected_channel.is_some() && !open_still_exists {
                // A DM this client closed goes quietly, as in Fluxer.
                let closed_here = self
                    .selected_channel
                    .as_ref()
                    .is_some_and(|id| self.social.closing.remove(id));
                if !closed_here {
                    self.chat.notice = Some(t!("sidebar-channel-deleted"));
                }
                self.selected_channel = None;
            } else if let Some(open) = self.selected_channel.clone() {
                // The open channel may have changed (for example its slowmode).
                self.composer_open_channel(&open, window, cx);
            }
            // Back to an account: the channel it had open.
            if let Some(wanted) = self.switcher.take_wanted_channel() {
                if self.channels.iter().any(|c| c.id == wanted) {
                    self.select_channel(wanted, window, cx);
                }
            }
            #[cfg(feature = "demo")]
            if let Some(wanted) = self.demo_channel.take() {
                if self.channels.iter().any(|c| c.id == wanted) {
                    self.select_channel(wanted, window, cx);
                }
            }
            // A channel a confirmed link asked for (see `desktop`).
            self.take_link_channel(window, cx);
            // A guild opens its first channel; the DM list opens the friends page, as
            // in Fluxer.
            if self.selected_channel.is_none() {
                // The DM list opens the DM open last (Fluxer's home button), else Friends.
                let last_dm = self
                    .social
                    .last_dm
                    .clone()
                    .filter(|id| self.channels.iter().any(|c| &c.id == id));
                // A guild opens the channel that was open in it last, else its first
                // channel that asks no content warning.
                let wanted = guild_id
                    .as_deref()
                    .and_then(|g| self.remembered_channel_of(g));
                let opens = |c: &&Channel| c.is_messageable() && self.gate_for(&c.id, cx).is_none();
                let first = wanted
                    .and_then(|w| self.channels.iter().find(|c| c.id == w && opens(c)))
                    .or_else(|| self.channels.iter().find(opens))
                    .cloned();
                match first {
                    Some(first) if guild_id.is_some() => self.select_channel(first.id, window, cx),
                    _ => match last_dm {
                        Some(id) if guild_id.is_none() => self.select_channel(id, window, cx),
                        _ => self.clear_messages(),
                    },
                }
            }
        }
    }

    pub(crate) fn select_guild(&mut self, guild_id: Option<String>, cx: &mut Context<Self>) {
        self.threads_leave_channel(None, cx);
        self.selected_guild = guild_id.clone();
        self.selected_channel = None;
        self.channels.clear();
        self.clear_messages();
        self.backend.send(messages::Cmd::OpenGuild { guild_id });
        cx.notify();
    }

    pub(crate) fn select_channel(
        &mut self,
        channel_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An age-restricted channel asks first, whichever way it is opened.
        if let Some(gate) = self.gate_for(&channel_id, cx) {
            self.show_gate(gate, cx);
            return;
        }
        self.selected_channel = Some(channel_id.clone());
        if self.selected_guild.is_none() {
            self.social.last_dm = Some(channel_id.clone());
        }
        self.note_channel_visit(&channel_id);
        self.record_visit(&channel_id, cx);
        // A forum opens the forum view; any other channel closes it.
        if self.open_forum(&channel_id, window, cx) {
            cx.notify();
            return;
        }
        self.composer_open_channel(&channel_id, window, cx);
        self.open_channel_messages(cx);
        cx.notify();
    }

    pub(crate) fn render_channel_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Only a server's channel list has the floating unread indicators.
        let (rows, unread): (Vec<AnyElement>, Vec<(usize, floating::Severity)>) =
            if self.rail.favorites {
                (self.favorite_rows(cx), Vec::new())
            } else {
                match self.selected_guild {
                    None => (self.dm_rows(cx), Vec::new()),
                    Some(_) => self.channel_rows(cx),
                }
            };
        let pills = self.floating_unread(&unread, cx);
        let header = if self.rail.favorites {
            self.render_favorites_header()
        } else {
            self.render_guild_header(cx).into_any_element()
        };
        div()
            .debug_selector(|| "sidebar".into())
            .w(px(self.sidebar_width()))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::bg_app())
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("channels")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.sidebar.scroll)
                            .pt_2()
                            .pr_2()
                            .pb_2()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .children(rows),
                    )
                    .children(pills),
            )
    }
}
