// SPDX-License-Identifier: AGPL-3.0-or-later
//! Demo scenes for the downloads panel (`downloads-panel`) and the `/gif` result list
//! (`gif-command`). Nothing here talks to a network: the downloads are made up, and the GIF
//! answers are the demo's own (`pickers.rs`).

use gpui::{Context, Window};

use super::scenes::{current, Scene};
use crate::app::Inlet;

impl Inlet {
    /// Once per frame in a demo run: sets up the scene's state when the app is ready.
    pub(crate) fn demo_media_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match current() {
            Scene::DownloadsPanel => {
                if self.account.is_some() {
                    self.media.downloads.demo_fill();
                }
            }
            Scene::GifCommand => self.demo_gif_command(window, cx),
            _ => {}
        }
    }
}
