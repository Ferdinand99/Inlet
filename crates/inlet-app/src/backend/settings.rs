// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings: the account's settings on the server (`PATCH /users/@me/settings`), the
//! synced preferences (the font size and message spacing) and the account's own
//! profile. The settings the person changes are first applied by the UI, which then
//! asks for them to be written here ([`Cmd::Save`]).
//!
//! Where a setting lives (as in Fluxer): theme, message display, media switches,
//! spoilers, developer mode and the privacy switches are fields of the account's user
//! settings; the font size and the group spacing are fields of the synced preferences;
//! the zoom, the window and the desktop switches are local (`settings.json`).
//!
//! Request budget (all REST goes through the protocol's limiter; Fluxer allows 20
//! `PATCH /users/@me/settings` per 10 s per account, and the presence writes use up to
//! 5 per 20 s of them):
//! - **Account settings** ([`write`]) are written by one task per account that keeps only
//!   the newest wish per field, waits [`write::GATHER`] for more changes and writes at
//!   most once every [`write::MIN_GAP`]: at most 5 writes per 10 s, however fast the
//!   person clicks.
//! - **Synced preferences** go through the protocol's single writer per account (one
//!   `PATCH` per batch of edits, two at least 1 s apart), after one `GET` for the base
//!   snapshot the first time a session writes them.
//! - **Profile** ([`profile`]): one `GET` when the My profile tab opens, and one `PATCH`
//!   and one `GET` for each press of Save (the UI allows one save at a time).
//!
//! A write that fails is reported once (the notice line) and answered with
//! [`Ev::SaveFailed`], so the UI shows what the server holds again. Nothing is retried
//! by itself. An account in the background writes nothing: a command for an account
//! that is not the active one is dropped by the backend.

pub mod profile;
pub mod write;

use inlet_protocol as p;

use self::profile::{OwnProfile, ProfileError, ProfileSave};
use self::write::{Change, Wish};
use super::{report, Active, Backend};

/// Commands of this domain.
pub enum Cmd {
    /// Changes to settings. `seq` counts up with every command; the answer says how far
    /// it got ([`Ev::AccountSaved`], [`Ev::PrefsSaved`], [`Ev::SaveFailed`]).
    Save { seq: u64, changes: Vec<Change> },
    /// The account's own profile (account-wide), for the My profile tab.
    LoadProfile,
    /// Saves a profile edit, then reads the profile again.
    SaveProfile(Box<ProfileSave>),
}

/// Events of this domain.
pub enum Ev {
    /// The account settings after the newest wish was written; `through` is the highest
    /// `seq` it holds.
    AccountSaved {
        through: u64,
        settings: Box<p::UserSettings>,
    },
    /// The synced preferences after a write of the preferences up to `seq`.
    PrefsSaved {
        through: u64,
        prefs: Box<p::SyncedPreferences>,
    },
    /// A write failed (the reason is in the notice line). The UI shows what the server
    /// holds for the settings of that kind again.
    SaveFailed {
        through: u64,
        preferences: bool,
    },
    /// The account's own profile.
    Profile(Box<OwnProfile>),
    ProfileFailed(String),
    /// The profile was saved; the profile as it is now.
    ProfileSaved(Box<OwnProfile>),
    ProfileSaveFailed(String),
}

pub(super) fn handle(backend: &mut Backend, cmd: Cmd) {
    let internal = backend.internal.clone();
    #[cfg(feature = "demo")]
    if let Some(Active::Demo(_)) = &backend.active {
        // The demo has no server: the UI already shows a change to the settings.
        match cmd {
            Cmd::LoadProfile => backend.emit(Ev::Profile(Box::new(profile::demo_profile()))),
            Cmd::SaveProfile(save) => {
                backend.emit(Ev::ProfileSaved(Box::new(profile::demo_saved(*save))));
            }
            Cmd::Save { .. } => {}
        }
        return;
    }
    let Some(Active::Real(r)) = &mut backend.active else {
        // Not signed in, or the command was for an account that is not the active one.
        match cmd {
            // Each kind of setting in the command is answered, so none stays waiting.
            Cmd::Save { seq, changes } => {
                for preferences in [false, true] {
                    if changes.iter().any(|c| c.is_preference() == preferences) {
                        backend.emit(Ev::SaveFailed {
                            through: seq,
                            preferences,
                        });
                    }
                }
            }
            Cmd::LoadProfile => backend.emit(Ev::ProfileFailed(t!("settings-not-connected"))),
            Cmd::SaveProfile(_) => {
                backend.emit(Ev::ProfileSaveFailed(t!("settings-not-connected")));
            }
        }
        return;
    };
    let client = r.session.client().clone();
    let events = r.events.clone();
    let gen = r.gen;
    match cmd {
        Cmd::Save { seq, changes } => {
            let account = Wish::of(seq, &changes);
            if !account.is_empty() {
                let tx = r.settings.sender(&client, &events, &internal, gen);
                let _ = tx.send(account);
            }
            let prefs: Vec<Change> = changes.into_iter().filter(Change::is_preference).collect();
            if !prefs.is_empty() {
                let base = r.settings.preferences_base();
                backend.spawn_guarded("save preferences", None, async move {
                    match write::write_preferences(&client, &base, prefs).await {
                        Ok(saved) => {
                            let _ = events.unbounded_send(Ev::PrefsSaved {
                                through: seq,
                                prefs: Box::new(saved),
                            });
                        }
                        Err(err) => {
                            report(
                                &events,
                                &internal,
                                gen,
                                |reason| t!("settings-save-failed", reason = reason),
                                err,
                            );
                            let _ = events.unbounded_send(Ev::SaveFailed {
                                through: seq,
                                preferences: true,
                            });
                        }
                    }
                });
            }
        }
        Cmd::LoadProfile => {
            backend.spawn_guarded("load profile", None, async move {
                match profile::load(&client).await {
                    Ok(own) => {
                        let _ = events.unbounded_send(Ev::Profile(Box::new(own)));
                    }
                    Err(err) => {
                        if let Some(reason) = profile::reason(&internal, gen, err) {
                            let _ = events.unbounded_send(Ev::ProfileFailed(reason));
                        }
                    }
                }
            });
        }
        Cmd::SaveProfile(save) => {
            backend.spawn_guarded("save profile", None, async move {
                let result: Result<OwnProfile, ProfileError> = profile::save(&client, *save).await;
                match result {
                    Ok(own) => {
                        let _ = events.unbounded_send(Ev::ProfileSaved(Box::new(own)));
                    }
                    Err(err) => {
                        if let Some(reason) = profile::reason(&internal, gen, err) {
                            let _ = events.unbounded_send(Ev::ProfileSaveFailed(reason));
                        }
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
