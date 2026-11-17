// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing the sound settings into the account's synced preferences
//! (`SyncedPreferences.sound`), so they follow the person between Inlet and Fluxer.
//!
//! Request budget: the UI hands a change over only after the person has stopped
//! changing sliders and switches for a moment (`SoundHub::take_sync`, polled once a
//! second), so one gesture is one write. The write goes through the protocol's single
//! preferences writer (one `PATCH` per batch of edits, two at least a second apart,
//! after one `GET` for the base snapshot the first time). The edit sets whole values,
//! never toggles, so the protocol may apply it again after a conflict.

use crate::backend::sounds::SoundSettings;
use crate::backend::{report, Active, Backend};

/// Commands for the sound settings.
pub enum Cmd {
    /// Writes these settings into the account's preferences.
    Save(Box<SoundSettings>),
}

pub(in crate::backend) fn handle(backend: &mut Backend, cmd: Cmd) {
    let Cmd::Save(settings) = cmd;
    // The demo has no server; another account than the active one writes nothing.
    let Some(Active::Real(r)) = &mut backend.active else {
        return;
    };
    let client = r.session.client().clone();
    let events = r.events.clone();
    let gen = r.gen;
    let base = r.settings.preferences_base();
    let internal = backend.internal.clone();
    backend.spawn_guarded("save sound settings", None, async move {
        let known = base.lock().await.clone();
        let result = async {
            let known = match known {
                Some(k) => k,
                None => client.synced_preferences().await?,
            };
            let wanted = *settings;
            client
                .update_synced_preferences(&known, move |prefs| wanted.write_proto(prefs))
                .await
        }
        .await;
        match result {
            Ok(stored) => *base.lock().await = Some(stored),
            Err(err) => report(
                &events,
                &internal,
                gen,
                |reason| t!("settings-save-failed", reason = reason),
                err,
            ),
        }
    });
}
