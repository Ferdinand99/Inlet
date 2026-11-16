// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings: account settings on the server, `SyncedPreferences` and the local settings file. The guild
//! order and folders, and joining and leaving guilds, live in their own submodules.

use std::sync::{Arc, Mutex};

use super::Backend;

pub mod folders;
pub mod guilds;

/// What one account keeps for the guild order and membership commands.
#[derive(Debug, Default)]
pub struct Ops {
    pub folders: folders::Queue,
    pub invites: guilds::InviteGate,
}

/// The shared handle of an account's [`Ops`].
pub type SharedOps = Arc<Mutex<Ops>>;

/// Commands of this domain.
pub enum Cmd {
    /// Save the guild order and folders.
    Folders(folders::Cmd),
    /// Look at an invite, join through it, leave a guild.
    Guild(guilds::Cmd),
}

impl From<folders::Cmd> for Cmd {
    fn from(c: folders::Cmd) -> Self {
        Cmd::Folders(c)
    }
}

impl From<guilds::Cmd> for Cmd {
    fn from(c: guilds::Cmd) -> Self {
        Cmd::Guild(c)
    }
}

/// Events of this domain.
pub enum Ev {
    Folders(folders::Ev),
    Guild(guilds::Ev),
}

impl From<folders::Ev> for Ev {
    fn from(e: folders::Ev) -> Self {
        Ev::Folders(e)
    }
}

impl From<guilds::Ev> for Ev {
    fn from(e: guilds::Ev) -> Self {
        Ev::Guild(e)
    }
}

pub(super) fn handle(backend: &mut Backend, cmd: Cmd) {
    match cmd {
        Cmd::Folders(cmd) => folders::handle(backend, cmd),
        Cmd::Guild(cmd) => guilds::handle(backend, cmd),
    }
}
