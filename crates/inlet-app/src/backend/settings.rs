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
//!   most once every [`write::MIN_GAP`]: at most 4 writes per 10 s, however fast the
//!   person clicks. With the preference writer (at most 10 per 10 s) and the presence
//!   writes (at most 5 per 20 s) the account stays under the 20 per 10 s.
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

pub mod account;
pub mod guilds;
pub mod profile;
pub mod sounds;
pub mod write;

use std::sync::{Arc, Mutex};

use super::{Backend, Command, Event};

/// What one account keeps for joining and leaving servers: the invite request gates and
/// the leaves in flight (see [`guilds`]). The guild order and folders are written with
/// the account settings ([`account::Cmd::SaveGuildFolders`]).
#[derive(Debug, Default)]
pub struct Ops {
    pub invites: guilds::InviteGate,
    pub accepts: guilds::InviteGate,
    pub leaving: guilds::LeaveGate,
}

/// The shared handle of an account's [`Ops`].
pub type SharedOps = Arc<Mutex<Ops>>;

/// Commands of this domain, by part: the account's own settings and profile, and (later)
/// the guild order and folders, each with its own `Cmd`.
pub enum Cmd {
    Account(account::Cmd),
    /// Look at an invite, join through it, leave a server.
    Guild(guilds::Cmd),
    Sounds(sounds::Cmd),
}

/// Events of this domain, by part.
pub enum Ev {
    Account(account::Ev),
    Guild(guilds::Ev),
}

impl From<account::Cmd> for Command {
    fn from(cmd: account::Cmd) -> Self {
        Command::Settings(Cmd::Account(cmd))
    }
}

impl From<sounds::Cmd> for Command {
    fn from(cmd: sounds::Cmd) -> Self {
        Command::Settings(Cmd::Sounds(cmd))
    }
}

impl From<account::Ev> for Event {
    fn from(ev: account::Ev) -> Self {
        Event::Settings(Ev::Account(ev))
    }
}

impl From<guilds::Cmd> for Command {
    fn from(cmd: guilds::Cmd) -> Self {
        Command::Settings(Cmd::Guild(cmd))
    }
}

impl From<guilds::Ev> for Event {
    fn from(ev: guilds::Ev) -> Self {
        Event::Settings(Ev::Guild(ev))
    }
}

pub(super) fn handle(backend: &mut Backend, cmd: Cmd) {
    match cmd {
        Cmd::Account(cmd) => account::handle(backend, cmd),
        Cmd::Guild(cmd) => guilds::handle(backend, cmd),
        Cmd::Sounds(cmd) => sounds::handle(backend, cmd),
    }
}

#[cfg(test)]
mod tests;
