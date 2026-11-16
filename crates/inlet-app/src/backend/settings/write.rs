// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing the account's settings: the changes the UI asks for, the coalescer that keeps
//! the newest wish per field, the task that writes them within the budget, and the
//! synced preferences write.

use std::sync::Arc;
use std::time::Duration;

use inlet_protocol as p;
use inlet_protocol::api::preferences::Preferences;
use inlet_protocol::proto::preferences::AccessibilitySettings;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender as TokioSender};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::Ev;
use crate::backend::{report, Internal, Ui};
use crate::theme::Pref;

/// How long a write waits for more changes before it goes out.
pub const GATHER: Duration = Duration::from_millis(600);
/// Shortest time between two writes of the account settings.
pub const MIN_GAP: Duration = Duration::from_secs(2);

/// One change to a setting. The UI keeps the newest per kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    // Account settings on the server.
    Theme(Pref),
    /// Compact message display.
    Compact(bool),
    GifAutoPlay(bool),
    AnimateEmoji(bool),
    /// 0 always, 1 on interaction, 2 never.
    AnimateStickers(u8),
    RenderEmbeds(bool),
    RenderReactions(bool),
    /// 0 always shown, 1 on click, 2 if moderator.
    RenderSpoilers(u8),
    DeveloperMode(bool),
    /// DMs from members of servers, by default (the reverse of `default_guilds_restricted`).
    DmsFromMembers(bool),
    /// Who may send a friend request: 1 mutual friends, 2 mutual servers, 4 anyone.
    FriendSources(u8),
    // Synced preferences.
    /// Message font size in pixels.
    FontSize(u8),
    /// Space above the first message of a group, in pixels.
    GroupSpacing(u8),
}

/// What kind of setting a change is (the newest change of a kind replaces the older).
pub type Kind = std::mem::Discriminant<Change>;

impl Change {
    pub fn kind(&self) -> Kind {
        std::mem::discriminant(self)
    }

    /// Whether the change goes into the synced preferences (else the user settings).
    pub fn is_preference(&self) -> bool {
        matches!(self, Change::FontSize(_) | Change::GroupSpacing(_))
    }

    /// Puts the change into a settings update.
    pub(super) fn write(&self, u: &mut p::UserSettingsUpdate) {
        match *self {
            Change::Theme(pref) => u.theme = Some(pref.server_name().to_owned()),
            Change::Compact(on) => u.message_display_compact = Some(on),
            Change::GifAutoPlay(on) => u.gif_auto_play = Some(on),
            Change::AnimateEmoji(on) => u.animate_emoji = Some(on),
            Change::AnimateStickers(n) => u.animate_stickers = Some(u64::from(n.min(2))),
            Change::RenderEmbeds(on) => u.render_embeds = Some(on),
            Change::RenderReactions(on) => u.render_reactions = Some(on),
            Change::RenderSpoilers(n) => u.render_spoilers = Some(u64::from(n.min(2))),
            Change::DeveloperMode(on) => u.developer_mode = Some(on),
            Change::DmsFromMembers(on) => u.default_guilds_restricted = Some(!on),
            Change::FriendSources(flags) => u.friend_source_flags = Some(u64::from(flags & 7)),
            Change::FontSize(_) | Change::GroupSpacing(_) => {}
        }
    }

    /// Puts the change into the synced preferences. Sets values, never toggles, so the
    /// protocol may apply an edit again after a conflict.
    pub(super) fn set_preference(&self, prefs: &mut Preferences) {
        match *self {
            Change::FontSize(px) => accessibility(prefs).font_size = Some(f64::from(px)),
            Change::GroupSpacing(px) => {
                accessibility(prefs).message_group_spacing = Some(f64::from(px));
            }
            _ => {}
        }
    }
}

/// The accessibility part of the preferences, made when there is none yet.
fn accessibility(prefs: &mut Preferences) -> &mut AccessibilitySettings {
    prefs.accessibility.get_or_insert_with(Default::default)
}

/// The newest wishes for the account settings, with the number of the newest command
/// they hold. Fields not asked for stay `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wish {
    update: p::UserSettingsUpdate,
    /// The number of the newest command the wish holds.
    pub through: u64,
}

impl Wish {
    /// The account settings among `changes` (the preferences go another way).
    pub fn of(through: u64, changes: &[Change]) -> Wish {
        let mut update = p::UserSettingsUpdate::default();
        for c in changes.iter().filter(|c| !c.is_preference()) {
            c.write(&mut update);
        }
        Wish { update, through }
    }

    pub fn is_empty(&self) -> bool {
        self.update == p::UserSettingsUpdate::default()
    }

    /// The update that would be sent.
    #[cfg(test)]
    pub fn update(&self) -> &p::UserSettingsUpdate {
        &self.update
    }

    /// Takes `newer` on top: its fields replace this one's.
    fn merge(&mut self, newer: Wish) {
        macro_rules! take {
            ($($f:ident),*) => {$(
                if newer.update.$f.is_some() {
                    self.update.$f = newer.update.$f;
                }
            )*};
        }
        take!(
            theme,
            message_display_compact,
            gif_auto_play,
            animate_emoji,
            animate_stickers,
            render_embeds,
            render_reactions,
            render_spoilers,
            developer_mode,
            default_guilds_restricted,
            friend_source_flags
        );
        self.through = self.through.max(newer.through);
    }
}

/// What the write task does next.
#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Next {
    /// Nothing waits.
    Idle,
    /// A write waits this long (for more changes or for the minimum gap).
    Wait(Duration),
    /// Write this now.
    Write(Wish),
}

/// Keeps the newest wish per field and says when to write: [`GATHER`] after the first
/// change since the last write, and never sooner than [`MIN_GAP`] after it.
#[derive(Default)]
pub struct Coalescer {
    wish: Wish,
    first_at: Option<Instant>,
    last_write: Option<Instant>,
}

impl Coalescer {
    pub fn add(&mut self, wish: Wish, now: Instant) {
        if wish.is_empty() {
            return;
        }
        self.first_at.get_or_insert(now);
        self.wish.merge(wish);
    }

    pub fn next(&mut self, now: Instant) -> Next {
        if self.wish.is_empty() {
            return Next::Idle;
        }
        let gathered = self.first_at.map_or(now, |t| t + GATHER);
        let spaced = self.last_write.map_or(now, |t| t + MIN_GAP);
        let due = gathered.max(spaced);
        if now < due {
            return Next::Wait(due - now);
        }
        self.last_write = Some(now);
        self.first_at = None;
        Next::Write(std::mem::take(&mut self.wish))
    }
}

/// One account's settings writer: the task that writes its account settings, and the
/// base snapshot of its synced preferences.
#[derive(Default)]
pub struct Writer {
    wishes: Option<TokioSender<Wish>>,
    task: Option<AbortHandle>,
    /// The synced preferences as this session last knew them, for the next write.
    prefs: Arc<tokio::sync::Mutex<Option<p::SyncedPreferences>>>,
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Writer {
    /// Where wishes go; the write task starts with the first one.
    pub(in crate::backend) fn sender(
        &mut self,
        client: &p::Client,
        events: &Ui,
        internal: &TokioSender<Internal>,
        gen: u64,
    ) -> TokioSender<Wish> {
        if let Some(tx) = &self.wishes {
            return tx.clone();
        }
        let (tx, rx) = unbounded_channel();
        let task = tokio::spawn(write_task(
            client.clone(),
            events.clone(),
            internal.clone(),
            gen,
            rx,
        ));
        self.task = Some(task.abort_handle());
        self.wishes = Some(tx.clone());
        tx
    }

    /// The snapshot the next preference write builds on.
    pub fn preferences_base(&self) -> Arc<tokio::sync::Mutex<Option<p::SyncedPreferences>>> {
        Arc::clone(&self.prefs)
    }
}

/// The write task of one account.
async fn write_task(
    client: p::Client,
    events: Ui,
    internal: TokioSender<Internal>,
    gen: u64,
    mut rx: UnboundedReceiver<Wish>,
) {
    let mut c = Coalescer::default();
    loop {
        match c.next(Instant::now()) {
            Next::Idle => match rx.recv().await {
                Some(w) => c.add(w, Instant::now()),
                None => return,
            },
            Next::Wait(wait) => {
                tokio::select! {
                    w = rx.recv() => match w {
                        Some(w) => c.add(w, Instant::now()),
                        None => return,
                    },
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            Next::Write(wish) => match client.update_user_settings(&wish.update).await {
                Ok(settings) => {
                    let _ = events.unbounded_send(Ev::AccountSaved {
                        through: wish.through,
                        settings: Box::new(settings),
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
                        through: wish.through,
                        preferences: false,
                    });
                }
            },
        }
    }
}

/// Writes preference changes on top of the session's base snapshot (read first when the
/// session has none). The protocol batches them and keeps the writes apart.
pub async fn write_preferences(
    client: &p::Client,
    base: &tokio::sync::Mutex<Option<p::SyncedPreferences>>,
    changes: Vec<Change>,
) -> p::Result<p::SyncedPreferences> {
    let known = base.lock().await.clone();
    let known = match known {
        Some(k) => k,
        None => client.synced_preferences().await?,
    };
    let stored = client
        .update_synced_preferences(&known, move |prefs| {
            for c in &changes {
                c.set_preference(prefs);
            }
        })
        .await?;
    *base.lock().await = Some(stored.clone());
    Ok(stored)
}
