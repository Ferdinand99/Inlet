// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sound effects on the UI side: the hub as a gpui global, the account's sound
//! settings coming in and the person's changes going out, the pick of a custom sound
//! file, and the slider the settings use. The settings section itself is
//! `ui/settings/tabs/sounds.rs`; the rules and the player are `backend::sounds`.
//!
//! A change goes to the hub (shown and kept at once). Once the person has stopped
//! changing things, [`init`]'s ticker hands the settings to the backend, which writes
//! them into the account's synced preferences. The account's settings (READY, and every
//! update) come in through [`follow_account`].

pub mod slider;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, Global, PathPromptOptions, SharedString};

use crate::app::CurrentBackend;
use crate::backend::settings::sounds::Cmd;
use crate::backend::sounds::{Invalid, SoundHub, SoundKind, SoundSettings};
use inlet_state::settings::SoundSettings as ServerSounds;

/// How often changes are looked for to hand to the account.
const TICK: Duration = Duration::from_secs(1);

/// What the settings section remembers between two draws.
#[derive(Default)]
pub struct SoundsUi {
    /// The rare sound effects are shown too.
    pub show_all: bool,
    /// The per-sound volume list is open. `None` until the section was drawn once; it
    /// then opens by itself when some sound has a volume of its own.
    pub overrides_open: Option<bool>,
    /// The last thing that went wrong with a custom sound, for the section.
    pub message: Option<SharedString>,
}

impl Global for SoundsUi {}

/// Demo runs write nothing.
fn demo_mode() -> bool {
    cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some()
}

/// Makes the hub, from `settings.json`; call once at start-up, after the notification
/// settings are loaded (they hold the old on/off switch of the sound).
pub fn init(cx: &mut App) {
    let path = if demo_mode() {
        None
    } else {
        crate::backend::platform::prefs::settings_path()
    };
    let hub = SoundHub::new(path);
    // Before there was a list of sounds, one switch turned the notification sound off.
    // The switch is gone; an Inlet that had it off keeps all sounds off.
    if cx.has_global::<crate::ui::notifications::alerts::Alerts>()
        && !crate::ui::notifications::alerts::sound_switch(cx)
    {
        hub.edit(|s| s.all_disabled = true);
        crate::ui::notifications::alerts::change_prefs(cx, |p| p.sound = true);
    }
    cx.set_global(hub.clone());
    cx.set_global(SoundsUi::default());
    let ticker = hub;
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(TICK).await;
        if let Some(settings) = ticker.take_sync() {
            cx.update(|cx| send(cx, settings));
        }
    })
    .detach();
}

/// The hub, once [`init`] has run.
pub fn hub(cx: &App) -> Option<SoundHub> {
    cx.try_global::<SoundHub>().cloned()
}

/// Plays a sound effect, if it is on.
#[cfg_attr(test, allow(dead_code))]
pub fn play(cx: &App, kind: SoundKind) {
    if let Some(hub) = hub(cx) {
        hub.play(kind);
    }
}

/// Hands the settings to the backend, which writes them into the account.
fn send(cx: &App, settings: SoundSettings) {
    if let Some(backend) = cx.try_global::<CurrentBackend>().map(|b| b.0.clone()) {
        backend.send(Cmd::Save(Box::new(settings)));
    }
}

/// The account's sound settings, as its state holds them (`None`: it has none).
pub fn follow_account(cx: &App, sound: Option<&ServerSounds>) {
    if let Some(hub) = hub(cx) {
        hub.adopt_remote(sound.map(SoundSettings::from_proto));
    }
}

/// Another account is shown: its sound settings come in as new.
pub fn forget_account(cx: &App) {
    if let Some(hub) = hub(cx) {
        hub.forget_remote();
    }
}

/// Asks which file to use for `kind`, then reads it off the UI thread.
pub fn pick_custom(kind: SoundKind, cx: &mut App) {
    let Some(hub) = hub(cx) else {
        return;
    };
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(SharedString::from(t!("settings-sounds-pick-prompt"))),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = chosen.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        let result = import(cx, hub, kind, path).await;
        cx.update(|cx| {
            cx.global_mut::<SoundsUi>().message = match result {
                Ok(()) => None,
                Err(why) => Some(SharedString::from(why.text())),
            };
            cx.refresh_windows();
        });
    })
    .detach();
}

async fn import(
    cx: &gpui::AsyncApp,
    hub: SoundHub,
    kind: SoundKind,
    path: PathBuf,
) -> Result<(), Invalid> {
    cx.background_executor()
        .spawn(async move { hub.set_custom(kind, &path) })
        .await
}
