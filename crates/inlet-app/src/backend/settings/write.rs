// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing the account's settings: the changes the UI asks for, the coalescer that keeps
//! the newest wish per field, the task that writes them within the budget, and the
//! synced preferences write.

use std::sync::Arc;
use std::time::Duration;

use inlet_protocol as p;
use inlet_protocol::api::preferences::Preferences;
use inlet_protocol::proto::preferences::AccessibilitySettings;
use inlet_state::flags::Flag;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender as TokioSender};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::account::Ev;
use crate::backend::{report, Internal, Ui};
use crate::theme::Pref;

/// How long a write waits for more changes before it goes out.
pub const GATHER: Duration = Duration::from_millis(600);
/// Shortest time between two writes of the account settings.
pub const MIN_GAP: Duration = Duration::from_secs(3);

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
    /// Show media of links inline (`inline_embed_media`).
    InlineEmbedMedia(bool),
    /// Show uploaded media inline (`inline_attachment_media`).
    InlineAttachmentMedia(bool),
    /// DMs from bots of servers, by default (the reverse of `bot_default_guilds_restricted`).
    BotDmsFromMembers(bool),
    /// Who may call: flags (1 friends of friends, 2 server members, 4 everyone, 8 friends
    /// only, 16 nobody, 32 silent for everyone).
    IncomingCalls(u8),
    /// Who may add to group DMs: flags as for calls, without the silent one.
    GroupDmAdd(u8),
    /// Who sees the full profile: 0 all servers, 1 small servers, 2 friends only.
    ProfilePrivacy(u8),
    /// Sensitive media in DMs from friends: 0 show, 1 blur, 2 block.
    SensitiveFriends(u8),
    /// Sensitive media in DMs from others: 0 show, 1 blur, 2 block.
    SensitiveOthers(u8),
    /// Sensitive media in server channels: 0 show, 1 blur, 2 block.
    SensitiveServers(u8),
    // Synced preferences.
    /// Message font size in pixels.
    FontSize(u8),
    /// Space above the first message of a group, in pixels.
    GroupSpacing(u8),
    /// The same in the compact display.
    CompactGroupSpacing(u8),
    /// Avatars in the compact display.
    CompactAvatars(bool),
    /// Text smileys become emoji when a message is sent.
    ConvertEmoticons(bool),
    /// One of the on/off switches of [`inlet_state::flags`]: synced, or only on this
    /// computer.
    Flag(Flag, bool),
}

/// What kind of setting a change is (the newest change of a kind replaces the older): the
/// variant, and for a switch which one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Kind {
    tag: std::mem::Discriminant<Change>,
    switch: Option<Flag>,
}

impl Change {
    pub fn kind(&self) -> Kind {
        Kind {
            tag: std::mem::discriminant(self),
            switch: match self {
                Change::Flag(f, _) => Some(*f),
                _ => None,
            },
        }
    }

    /// Whether the change goes into the synced preferences (else the user settings, or
    /// nowhere: see [`Change::is_local`]).
    pub fn is_preference(&self) -> bool {
        match self {
            Change::FontSize(_)
            | Change::GroupSpacing(_)
            | Change::CompactGroupSpacing(_)
            | Change::CompactAvatars(_)
            | Change::ConvertEmoticons(_) => true,
            Change::Flag(f, _) => f.is_synced(),
            _ => false,
        }
    }

    /// The change is only for this computer: it is kept in `settings.json` and never sent.
    pub fn is_local(&self) -> bool {
        matches!(self, Change::Flag(f, _) if !f.is_synced())
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
            Change::InlineEmbedMedia(on) => u.inline_embed_media = Some(on),
            Change::InlineAttachmentMedia(on) => u.inline_attachment_media = Some(on),
            Change::BotDmsFromMembers(on) => u.bot_default_guilds_restricted = Some(!on),
            Change::IncomingCalls(flags) => u.incoming_call_flags = Some(u64::from(flags & 63)),
            Change::GroupDmAdd(flags) => {
                u.group_dm_add_permission_flags = Some(u64::from(flags & 31));
            }
            Change::ProfilePrivacy(n) => u.profile_privacy = Some(u64::from(n.min(2))),
            Change::SensitiveFriends(n) => {
                u.sensitive_content_friend_dm_filter = Some(u64::from(n.min(2)));
            }
            Change::SensitiveOthers(n) => {
                u.sensitive_content_non_friend_dm_filter = Some(u64::from(n.min(2)));
            }
            Change::SensitiveServers(n) => {
                u.sensitive_content_guild_filter = Some(u64::from(n.min(2)));
            }
            Change::FontSize(_)
            | Change::GroupSpacing(_)
            | Change::CompactGroupSpacing(_)
            | Change::CompactAvatars(_)
            | Change::ConvertEmoticons(_)
            | Change::Flag(..) => {}
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
            Change::CompactGroupSpacing(px) => {
                accessibility(prefs).compact_message_group_spacing = Some(f64::from(px));
            }
            Change::CompactAvatars(show) => {
                accessibility(prefs).show_user_avatars_in_compact_mode = Some(show);
            }
            Change::ConvertEmoticons(on) => {
                prefs
                    .chat_input
                    .get_or_insert_with(Default::default)
                    .convert_emoticons = Some(on);
            }
            Change::Flag(flag, on) => flag.write(prefs, on),
            _ => {}
        }
    }
}

/// The fields the server refused, for the error line: its answer to a write that is
/// refused names the fields (`errors` with a `path` and a `message`), and a write of
/// several settings at once is refused as a whole, so the person is told which one.
pub(super) fn refused_fields(err: &p::Error) -> String {
    let p::Error::Api(api) = err else {
        return String::new();
    };
    let Some(list) = api.errors.as_ref().and_then(|e| e.as_array()) else {
        return String::new();
    };
    let parts: Vec<String> = list
        .iter()
        .filter_map(|e| {
            let path = e.get("path")?.as_str()?;
            let message = e
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or_default();
            Some(crate::model::clean_name(&format!("{path}: {message}")))
        })
        .take(3)
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join("; "))
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

    /// A wish for the list of servers whose members or bots may not send DMs.
    pub fn guild_dms(through: u64, bots: bool, guild_ids: Vec<String>) -> Wish {
        let mut update = p::UserSettingsUpdate::default();
        if bots {
            update.bot_restricted_guilds = Some(guild_ids);
        } else {
            update.restricted_guilds = Some(guild_ids);
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
            restricted_guilds,
            bot_default_guilds_restricted,
            bot_restricted_guilds,
            inline_embed_media,
            inline_attachment_media,
            incoming_call_flags,
            group_dm_add_permission_flags,
            profile_privacy,
            sensitive_content_friend_dm_filter,
            sensitive_content_non_friend_dm_filter,
            sensitive_content_guild_filter,
            friend_source_flags
        );
        self.through = self.through.max(newer.through);
    }
}

/// What the write task does next.
#[derive(Debug, PartialEq)]
pub enum Next {
    /// Nothing waits.
    Idle,
    /// A write waits this long (for more changes or for the minimum gap).
    Wait(Duration),
    /// Write this now.
    Write(Box<Wish>),
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
        Next::Write(Box::new(std::mem::take(&mut self.wish)))
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
                    let fields = refused_fields(&err);
                    report(
                        &events,
                        &internal,
                        gen,
                        |reason| t!("settings-save-failed", reason = format!("{reason}{fields}")),
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
