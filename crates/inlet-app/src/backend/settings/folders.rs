// SPDX-License-Identifier: AGPL-3.0-or-later
//! Saving the guild order and folders (`guild_folders` in the account settings).
//!
//! The UI shows a change at once and sends the whole list. Request budget: one
//! `PATCH /users/@me/settings` at a time per account (through the protocol's limiter),
//! at least [`MIN_GAP`] between two of them, and changes that arrive meanwhile replace
//! each other, so a person dragging guilds around for ten seconds costs at most eleven
//! requests however many drops that was (Fluxer allows twenty in ten seconds). Signing
//! out aborts the worker with the account's other settings writes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use inlet_protocol as p;
use inlet_protocol::models::GuildFolder;

use super::super::session::{Failure, FailureKind};
use super::super::{Active, Backend, Internal};
use super::{Ev as SettingsEv, Ops};

/// Shortest time between two saves of one account.
pub const MIN_GAP: Duration = Duration::from_secs(1);

pub enum Cmd {
    /// Save this whole list as the guild order.
    Save { folders: Vec<GuildFolder> },
}

pub enum Ev {
    /// The server stored the list and answered with the settings.
    Saved(Box<p::UserSettings>),
    /// The list could not be saved (`reason` shows to the person).
    Failed(String),
}

/// The changes waiting for the worker, and whether the worker runs.
#[derive(Debug, Default)]
pub struct Queue {
    pending: Option<Vec<GuildFolder>>,
    running: bool,
}

impl Queue {
    /// A list to save. Replaces one that waits. Returns whether the caller must start
    /// the worker (none runs).
    pub fn offer(&mut self, folders: Vec<GuildFolder>) -> bool {
        self.pending = Some(folders);
        !std::mem::replace(&mut self.running, true)
    }

    /// The worker is gone (it ended, was aborted or panicked): nothing runs, and what
    /// waited belonged to a session that is over.
    pub fn stop(&mut self) {
        self.pending = None;
        self.running = false;
    }

    /// The worker asks for the next list. `None` ends the worker.
    pub fn next(&mut self) -> Option<Vec<GuildFolder>> {
        let next = self.pending.take();
        if next.is_none() {
            self.running = false;
        }
        next
    }
}

pub(super) fn handle(backend: &mut Backend, cmd: Cmd) {
    let Cmd::Save { folders } = cmd;
    let (client, gen, events, ops) = match &mut backend.active {
        #[cfg(feature = "demo")]
        Some(Active::Demo(_)) => {
            // No server: the list is the settings.
            let settings: p::UserSettings =
                serde_json::from_value(serde_json::json!({ "guild_folders": folders }))
                    .expect("demo settings");
            let _ = backend
                .events
                .unbounded_send(SettingsEv::Folders(Ev::Saved(Box::new(settings))));
            return;
        }
        Some(Active::Real(r)) => {
            let start = r
                .ops
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .folders
                .offer(folders);
            if !start {
                return;
            }
            (
                r.session.client().clone(),
                r.gen,
                r.events.clone(),
                r.ops.clone(),
            )
        }
        None => return,
    };
    let internal = backend.internal.clone();
    let task = backend.spawn_guarded(
        "guild folders",
        None,
        worker(client, gen, events, internal, ops),
    );
    if let Some(Active::Real(r)) = &mut backend.active {
        r.settings_writes.retain(|t| !t.is_finished());
        r.settings_writes.push(task);
    }
}

/// Marks the queue as having no worker when the worker's task ends, however it ends (a
/// finished queue, the session ending, an abort or a panic), so a later save starts a
/// new one.
struct WorkerGuard(Arc<Mutex<Ops>>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .folders
            .stop();
    }
}

async fn worker(
    client: p::Client,
    gen: u64,
    events: super::super::Ui,
    internal: tokio::sync::mpsc::UnboundedSender<Internal>,
    ops: Arc<Mutex<Ops>>,
) {
    let _guard = WorkerGuard(ops.clone());
    loop {
        let next = ops.lock().unwrap_or_else(|e| e.into_inner()).folders.next();
        let Some(folders) = next else {
            return;
        };
        match client.set_guild_folders(&folders).await {
            Ok(settings) => {
                let _ = events.unbounded_send(SettingsEv::Folders(Ev::Saved(Box::new(settings))));
            }
            Err(err) => {
                let failure = Failure::from(err);
                if failure.kind == FailureKind::SessionEnded {
                    let _ = internal.send(Internal::Ended {
                        gen,
                        message: failure.message,
                    });
                    return;
                }
                let _ = events.unbounded_send(SettingsEv::Folders(Ev::Failed(failure.message)));
            }
        }
        tokio::time::sleep(MIN_GAP).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: i64) -> Vec<GuildFolder> {
        vec![GuildFolder::new(n, None, vec![n.to_string()])]
    }

    /// A person drags guilds around for ten seconds, a drop every 100 ms: the worker
    /// saves at most once a second, and the last list is the one that is saved.
    #[test]
    fn dragging_guilds_around_costs_about_one_save_a_second() {
        let mut queue = Queue::default();
        let mut started = 0;
        let mut saved: Vec<Vec<GuildFolder>> = Vec::new();
        // The worker's clock: `ready_at` is when it asks for the next list.
        let mut ready_at = Some(0u64);
        for tick in 0..100u64 {
            let now = tick * 100;
            if queue.offer(list(tick as i64)) {
                started += 1;
                ready_at = Some(now);
            }
            if ready_at.is_some_and(|t| t <= now) {
                match queue.next() {
                    Some(next) => {
                        saved.push(next);
                        ready_at = Some(now + MIN_GAP.as_millis() as u64);
                    }
                    None => ready_at = None,
                }
            }
        }
        assert!(saved.len() <= 11, "{} saves", saved.len());
        assert!(started <= 2, "{started} workers");
        // After a quiet second the newest list goes out and the worker ends.
        let last = queue.next().map(|l| l[0].id);
        assert_eq!(last, Some(Some(99)));
        assert_eq!(queue.next(), None);
        // A new change starts a new worker.
        assert!(queue.offer(list(1)));
        assert!(!queue.offer(list(2)));
        // A worker that is gone (the session ended) leaves nothing wedged: what waited
        // is dropped, and the next change starts a worker again.
        queue.stop();
        assert_eq!(queue.next(), None);
        assert!(queue.offer(list(3)));
    }
}
