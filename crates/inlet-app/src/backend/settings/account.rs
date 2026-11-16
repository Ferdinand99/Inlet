// SPDX-License-Identifier: AGPL-3.0-or-later
//! The account's own settings and profile: the commands the tabs send and the events
//! that answer them. The writing is in `write.rs`, the profile in `profile.rs`.

use inlet_protocol as p;

use super::profile::{self, OwnProfile, ProfileError, ProfileSave};
use super::write::{self, Change, Wish};
use crate::backend::{report, Active, Backend};

/// Commands for the account's own settings and profile.
pub enum Cmd {
    /// Changes to settings. `seq` counts up with every command; the answer says how far
    /// it got ([`Ev::AccountSaved`], [`Ev::PrefsSaved`], [`Ev::SaveFailed`]).
    Save { seq: u64, changes: Vec<Change> },
    /// The account's own profile (account-wide), for the My profile tab.
    LoadProfile,
    /// Saves a profile edit, then reads the profile again.
    SaveProfile(Box<ProfileSave>),
}

/// Events of the account's own settings and profile.
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

pub(in crate::backend) fn handle(backend: &mut Backend, cmd: Cmd) {
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
