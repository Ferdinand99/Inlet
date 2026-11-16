// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings of the tabs as one gpui global, [`Store`]: what is shown, what changes
//! it (a click in a tab, the server's word), and what a change touches. A change shows
//! from the next frame without a restart: the colours and the message metrics are
//! switched here ([`crate::theme`]), and the views that keep state of their own (the
//! message list, the animation budget) follow when they see that the store's
//! generation moved.
//!
//! Local copies: the settings that decide how the first screen looks (theme, display
//! mode, font size, spacing) and the media switches are also kept in `settings.json`
//! (`"appearance"`, `"media"`), so the window draws right before the server has
//! answered. The server's snapshot replaces them when it comes.

use std::path::PathBuf;

use gpui::{App, Global};
use inlet_protocol as p;
use inlet_state::settings::Display;
use serde::{Deserialize, Serialize};

use super::sync::{Tracker, Values};
use crate::app::CurrentBackend;
use crate::backend::platform::prefs;
use crate::backend::settings::{write::Change, Cmd};
use crate::theme::{self, Pref};
use crate::ui::media::animation_budget::MotionPrefs;

/// The key of the local copy in `settings.json`.
const APPEARANCE_KEY: &str = "appearance";

/// The part of the settings that is kept locally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct LocalCopy {
    /// The server's name of the theme.
    theme: String,
    compact: bool,
    font_size: u8,
    group_spacing: u8,
}

impl Default for LocalCopy {
    fn default() -> Self {
        LocalCopy::of(&Values::default())
    }
}

impl LocalCopy {
    fn of(v: &Values) -> Self {
        LocalCopy {
            theme: v.theme.server_name().to_owned(),
            compact: v.compact,
            font_size: v.font_size,
            group_spacing: v.group_spacing,
        }
    }

    /// The defaults with what the file says, each value repaired if the file is odd.
    fn values(&self) -> Values {
        let mut v = Values::default();
        if let Some(pref) = Pref::from_server(&self.theme) {
            v.theme = pref;
        }
        v.compact = self.compact;
        v.font_size = self.font_size.clamp(8, 48);
        v.group_spacing = self.group_spacing.min(64);
        v
    }
}

/// The settings shown, and where they are kept.
pub struct Store {
    tracker: Tracker,
    /// `settings.json`; `None` in demo runs and tests, which write nothing.
    path: Option<PathBuf>,
    /// Goes up with every change of what is shown.
    generation: u64,
    /// What the colours and metrics were last switched to.
    applied: Option<Values>,
}

impl Global for Store {}

impl Store {
    fn new(values: Values, path: Option<PathBuf>) -> Self {
        Store {
            tracker: Tracker::new(values),
            path,
            generation: 1,
            applied: None,
        }
    }
}

/// Loads the local copy and switches the colours and metrics to it; call once at
/// start-up, before the window opens.
pub fn init(cx: &mut App) {
    let path = if cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some() {
        None
    } else {
        prefs::settings_path()
    };
    #[allow(unused_mut)]
    let mut values = path
        .as_deref()
        .map(|p| prefs::load_section::<LocalCopy>(p, APPEARANCE_KEY).values())
        .unwrap_or_default();
    // The demo's scene files name a theme.
    #[cfg(feature = "demo")]
    if let Some(pref) = std::env::var("INLET_DEMO_THEME")
        .ok()
        .and_then(|name| Pref::from_server(&name))
    {
        values.theme = pref;
    }
    theme::set_system_dark(theme::appearance_is_dark(cx.window_appearance()));
    // What was kept is what is shown: nothing to write back.
    switch_render_state(&values);
    let mut store = Store::new(values.clone(), path);
    store.applied = Some(values);
    cx.set_global(store);
}

/// The global, made with the defaults when `init` did not run (tests).
fn store(cx: &mut App) -> &mut Store {
    if !cx.has_global::<Store>() {
        cx.set_global(Store::new(Values::default(), None));
    }
    cx.global_mut::<Store>()
}

/// The settings to show.
pub fn values(cx: &App) -> Values {
    cx.try_global::<Store>()
        .map(|s| s.tracker.values())
        .unwrap_or_default()
}

/// Goes up with every change of what is shown; views that keep state of their own
/// compare it with the one they last followed.
pub fn generation(cx: &App) -> u64 {
    cx.try_global::<Store>().map_or(0, |s| s.generation)
}

/// A change from a tab: shown at once, kept, and sent to the server.
pub fn change(cx: &mut App, change: Change) {
    let seq = store(cx).tracker.edit(change);
    apply(cx);
    if let Some(backend) = cx.try_global::<CurrentBackend>().map(|b| b.0.clone()) {
        backend.send(Cmd::Save {
            seq,
            changes: vec![change],
        });
    }
}

/// The account settings as the server says them: READY, an update, or the answer to a
/// write (`answer`: the command it answers for).
pub fn server_settings(
    cx: &mut App,
    settings: &p::UserSettings,
    display: Option<Display>,
    answer: Option<u64>,
) {
    let s = store(cx);
    let base = s.tracker.confirmed().clone();
    let values = Values::from_server(settings, display, &base);
    let before = s.tracker.values();
    s.tracker.server(settings.version(), values, answer);
    if s.tracker.values() != before {
        apply(cx);
    }
}

/// The synced preferences as stored after a write of them up to command `through`.
pub fn preferences(cx: &mut App, display: Display, through: u64) {
    if store(cx).tracker.preferences(display, through) {
        apply(cx);
    }
}

/// A write failed: the settings show what the server holds again.
pub fn failed(cx: &mut App, through: u64, preferences: bool) {
    if store(cx).tracker.failed(through, preferences) {
        apply(cx);
    }
}

/// Another account is shown.
pub fn reset_account(cx: &mut App) {
    store(cx).tracker.reset_account();
}

/// The operating system's light or dark appearance changed.
pub fn system_appearance_changed(cx: &mut App, dark: bool) {
    if theme::system_is_dark() != dark {
        theme::set_system_dark(dark);
        // Only the choice "system" draws differently, but the check is cheap.
        let s = store(cx);
        theme::apply(s.tracker.values().theme);
        s.generation += 1;
        cx.refresh_windows();
    }
}

/// Switches the colours and the message metrics to `v`.
fn switch_render_state(v: &Values) {
    theme::apply(v.theme);
    theme::set_message_font_px(f32::from(v.font_size));
    theme::set_group_spacing(f32::from(v.group_spacing));
}

/// Switches the colours and metrics to what is shown, keeps the local copy, and makes
/// every window draw again.
fn apply(cx: &mut App) {
    let s = store(cx);
    let now = s.tracker.values();
    if s.applied.as_ref() == Some(&now) {
        return;
    }
    let before = s.applied.replace(now.clone());
    switch_render_state(&now);
    s.generation += 1;
    if let Some(path) = s.path.clone() {
        let local = LocalCopy::of(&now);
        if before.as_ref().map(LocalCopy::of) != Some(local.clone()) {
            if let Err(err) = prefs::save_section(&path, APPEARANCE_KEY, &local) {
                log::warn!("could not write the appearance settings: {err}");
            }
        }
        let motion = MotionPrefs::of_settings(&now);
        if before.as_ref().map(MotionPrefs::of_settings) != Some(motion) {
            if let Err(err) = motion.save(&path) {
                log::warn!("could not write the media settings: {err}");
            }
        }
    }
    cx.refresh_windows();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::theme::Theme;
    use gpui::{div, prelude::*, Context, IntoElement, Render, TestAppContext, Window};
    use serde_json::json;

    /// A view that draws the way the chat does: the chat colour, and it keeps what it
    /// drew last.
    struct Probe {
        drawn: Vec<(Theme, u32)>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let c: gpui::Rgba = theme::bg_chat().into();
            self.drawn
                .push((theme::current(), (c.r * 255.0).round() as u32));
            div().bg(theme::bg_chat())
        }
    }

    /// The probe's newest and the one before.
    fn drawn(cx: &mut TestAppContext, view: &gpui::Entity<Probe>) -> (Theme, u32) {
        cx.read_entity(view, |p, _| *p.drawn.last().expect("drawn"))
    }

    /// A theme, font size, display mode and group spacing change shows from the next
    /// frame on, in a window that stays open: nothing restarts.
    #[gpui::test]
    fn appearance_change_applies_without_restart(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Probe { drawn: Vec::new() });
        cx.update(|_, cx| cx.set_global(Store::new(Values::default(), None)));
        cx.run_until_parked();
        assert_eq!(drawn(cx, &view), (Theme::Dark, 0x1e));
        assert_eq!(theme::message_font_px(), 16.);

        cx.update(|_, cx| change(cx, Change::Theme(Pref::Light)));
        cx.run_until_parked();
        assert_eq!(
            drawn(cx, &view),
            (Theme::Light, 0xf0),
            "the open window draws with the light chat colour"
        );
        assert_eq!(cx.update(|_, cx| values(cx).theme), Pref::Light);

        cx.update(|_, cx| change(cx, Change::Theme(Pref::Coal)));
        cx.run_until_parked();
        assert_eq!(drawn(cx, &view), (Theme::Coal, 0x02));

        let before = cx.update(|_, cx| generation(cx));
        cx.update(|_, cx| {
            change(cx, Change::FontSize(20));
            change(cx, Change::GroupSpacing(8));
            change(cx, Change::Compact(true));
        });
        assert_eq!(theme::message_font_px(), 20.);
        assert_eq!(theme::message_line_px(), 28.);
        assert_eq!(theme::group_spacing(), 8.);
        cx.update(|_, cx| {
            let v = values(cx);
            assert!(v.compact && v.font_size == 20 && v.group_spacing == 8);
            assert!(
                generation(cx) > before,
                "views that follow it see the change"
            );
        });

        // "System" follows the operating system, also while the app runs.
        cx.update(|_, cx| {
            change(cx, Change::Theme(Pref::System));
            system_appearance_changed(cx, false);
        });
        cx.run_until_parked();
        assert_eq!(drawn(cx, &view).0, Theme::Light);
        cx.update(|_, cx| system_appearance_changed(cx, true));
        cx.run_until_parked();
        assert_eq!(drawn(cx, &view).0, Theme::Dark);

        // The server's word shows the same way: another client picked coal.
        let coal: p::UserSettings = serde_json::from_value(json!({
            "theme": "coal", "version": 99, "message_display_compact": true,
            "gif_auto_play": true, "animate_emoji": true, "render_embeds": true,
            "render_reactions": true, "render_spoilers": 1
        }))
        .unwrap();
        // The pending click on "system" is answered for first, as the writer would.
        cx.update(|_, cx| server_settings(cx, &coal, None, Some(10)));
        cx.run_until_parked();
        assert_eq!(drawn(cx, &view), (Theme::Coal, 0x02));
        cx.update(|_, _| {
            theme::set_theme(Theme::Dark);
            theme::set_message_font_px(16.);
            theme::set_group_spacing(16.);
        });
    }

    #[test]
    fn the_local_copy_repairs_odd_values() {
        let odd = LocalCopy {
            theme: "neon".into(),
            compact: true,
            font_size: 200,
            group_spacing: 250,
        };
        let v = odd.values();
        assert_eq!(v.theme, Pref::Dark);
        assert!(v.compact);
        assert_eq!(v.font_size, 48);
        assert_eq!(v.group_spacing, 64);
        assert_eq!(LocalCopy::default().values(), Values::default());
    }

    #[test]
    fn the_local_copy_round_trips_through_the_settings_file() {
        let dir = std::env::temp_dir().join(format!("inlet-appearance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let v = Values {
            theme: Pref::Coal,
            compact: true,
            font_size: 18,
            group_spacing: 8,
            ..Values::default()
        };
        prefs::save_section(&path, APPEARANCE_KEY, &LocalCopy::of(&v)).unwrap();
        let back: LocalCopy = prefs::load_section(&path, APPEARANCE_KEY);
        assert_eq!(back.values(), v);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
