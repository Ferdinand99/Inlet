// SPDX-License-Identifier: AGPL-3.0-or-later
//! Friends, friend requests, blocks and the DM list's actions: open a DM, close it, pin
//! it, and create, edit, add to and leave group DMs. The results come back as gateway
//! events (RELATIONSHIP_*, CHANNEL_*, USER_PINNED_DMS_UPDATE) that the account state
//! applies; this domain only answers whether a call went through.
//!
//! Budget (every call also goes through the protocol's limiter):
//! - One call per target at a time ([`Gate`]): a second click on the same person or DM
//!   while the first call runs sends nothing.
//! - At most [`WRITES_PER_WINDOW`] relationship and DM writes per [`WRITE_WINDOW`] per
//!   account; more are refused at once, without a request. Friend requests by name
//!   also stay within Fluxer's 10 per minute ([`FRIEND_REQUESTS_PER_MINUTE`]).
//! - Closing a DM and editing a group DM take the channel as the account state knows
//!   it, so the protocol's check that it is a DM or group DM sees the server's type.
//! - Group DMs: Fluxer allows 10 creations and 10 additions per hour per account, every
//!   attempt counts, and one call takes two (without a token, then with the answer of the
//!   CAPTCHA). Inlet makes at most [`GROUP_CALLS_PER_HOUR`] create calls and as many
//!   added people per hour per account (five of each is ten attempts; a rare third
//!   request ends in the server's own 429, which closes the gate), and none at all while
//!   the server's `retry_after` runs. People not reached because an add stopped early
//!   are given back. A create or add that is still solving the CAPTCHA stops when the
//!   window closes, the account changes or the session ends ([`Cmd::CancelGroupCall`],
//!   [`cancel_all_solves`]). A CAPTCHA it cannot pass or a rate limit ends the call: the UI shows a
//!   lasting message, and nothing is tried again on its own. Inlet solves the CAPTCHA
//!   itself, as the official client does, and never hands it to the browser; a call
//!   that takes more than [`SLOW_HINT`] says so ([`Ev::Working`]).
//! - Invites: a group's invites are listed, made and revoked here, and an invite to a
//!   community is made and sent to a person as a DM, one call at a time per target.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use inlet_protocol as p;
use tokio::sync::mpsc::UnboundedSender as TokioSender;

use super::super::accounts::AccountKey;
use super::super::session::{Failure, FailureKind};
use super::super::{Active, Backend, Internal};

/// Relationship and DM writes per window and account.
pub const WRITES_PER_WINDOW: usize = 10;
/// The window of [`WRITES_PER_WINDOW`].
pub const WRITE_WINDOW: Duration = Duration::from_secs(10);
/// Friend requests by name per minute and account (Fluxer's bucket).
pub const FRIEND_REQUESTS_PER_MINUTE: usize = 10;
/// Group DM create calls (and, apart, added people) per hour and account: Fluxer's 10
/// attempts an hour, two for every call that solves a CAPTCHA.
pub const GROUP_CALLS_PER_HOUR: usize = 5;
/// The window of [`GROUP_CALLS_PER_HOUR`].
pub const GROUP_WINDOW: Duration = Duration::from_secs(3600);
/// Longest server wait taken for group DM calls (the protocol reports at most 6 h).
pub const MAX_GROUP_WAIT: Duration = Duration::from_secs(6 * 3600);
/// Largest group DM icon read from disk (Fluxer takes up to 10 MiB).
pub const MAX_ICON_BYTES: u64 = 10 * 1024 * 1024;
/// A group DM call that runs longer than this reports [`Ev::Working`] (the official
/// client shows "Still working on it" after two seconds of CAPTCHA work).
pub const SLOW_HINT: Duration = Duration::from_secs(2);

/// A new icon for a group DM.
#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    /// The picture in this file (picked by the person).
    File(PathBuf),
    Remove,
}

/// What one save of a group's settings changes; `None` and empty keep a field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupChange {
    /// A new name (`Some(None)` clears it).
    pub name: Option<Option<String>>,
    /// A new icon, or none at all.
    pub icon: Option<Icon>,
    /// The mature-content switch (owner only).
    pub nsfw: Option<bool>,
    /// The new owner (owner only).
    pub owner_id: Option<String>,
    /// Nicknames in the group by user ID; `None` clears one.
    pub nicks: Vec<(String, Option<String>)>,
}

/// Commands of the relationships part of the social domain.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// A friend request by username (and tag, on instances that have them).
    AddFriend {
        username: String,
        discriminator: Option<String>,
    },
    /// A friend request to someone by their ID (the user menu).
    SendRequest {
        user_id: String,
    },
    /// Accepts a friend request.
    Accept {
        user_id: String,
    },
    /// Ends a friendship, ignores or cancels a request, or unblocks.
    Remove {
        user_id: String,
    },
    Block {
        user_id: String,
    },
    /// Opens (or finds) the DM with someone; [`Ev::DmOpened`] answers.
    OpenDm {
        user_id: String,
    },
    /// A new group DM with these friends.
    CreateGroupDm {
        recipients: Vec<String>,
    },
    /// Adds friends to a group DM, one call each, until one does not go through.
    AddRecipients {
        channel_id: String,
        user_ids: Vec<String>,
    },
    RemoveRecipient {
        channel_id: String,
        user_id: String,
    },
    /// Closes a DM or leaves a group DM: the channel as the account state has it.
    CloseDm {
        channel: Box<p::Channel>,
    },
    /// Changes a group DM's name, icon, nicknames, mature switch or owner: the channel
    /// as the account state has it.
    EditGroupDm {
        channel: Box<p::Channel>,
        change: GroupChange,
    },
    PinDm {
        channel_id: String,
        pin: bool,
    },
    /// Stops a group create or add that still solves the CAPTCHA (the call named `key`):
    /// the window was closed.
    CancelGroupCall {
        key: String,
    },
    /// Asks how many more people can be added to groups this hour ([`Ev::AddSlots`]).
    AddSlots,
    /// Sets or clears (`None`) the private nickname for a friend.
    SetFriendNickname {
        user_id: String,
        nickname: Option<String>,
    },
    /// Makes an invite to a group DM that expires in a day ([`Ev::InviteLink`]).
    CreateGroupInvite {
        channel_id: String,
    },
    /// Lists a group DM's invites ([`Ev::Invites`]); owner only.
    ListGroupInvites {
        channel_id: String,
    },
    /// Revokes an invite of a group DM; owner only.
    RevokeInvite {
        channel_id: String,
        code: String,
    },
    /// Sends an invite to a community in a DM with someone: their own vanity link when
    /// the account may not make invites there, otherwise a new invite to `channel_id`.
    InviteToCommunity {
        user_id: String,
        guild_id: String,
        channel_id: String,
        vanity: Option<String>,
    },
}

impl Cmd {
    /// What the UI marks as busy while the call runs, and what the answer names.
    pub fn key(&self) -> String {
        match self {
            Cmd::AddFriend { .. } => "add-friend".into(),
            Cmd::SendRequest { user_id }
            | Cmd::Accept { user_id }
            | Cmd::Remove { user_id }
            | Cmd::Block { user_id } => {
                format!("user:{user_id}")
            }
            Cmd::OpenDm { user_id } => format!("open:{user_id}"),
            Cmd::CreateGroupDm { .. } => "group-dm".into(),
            Cmd::AddRecipients { channel_id, .. } | Cmd::RemoveRecipient { channel_id, .. } => {
                format!("group:{channel_id}")
            }
            Cmd::EditGroupDm { channel, .. } => format!("group:{}", channel.id),
            Cmd::CloseDm { channel } => format!("dm:{}", channel.id),
            Cmd::PinDm { channel_id, .. } => format!("dm:{channel_id}"),
            Cmd::SetFriendNickname { user_id, .. } => format!("nick:{user_id}"),
            Cmd::CancelGroupCall { key } => format!("cancel:{key}"),
            Cmd::AddSlots => "add-slots".into(),
            Cmd::CreateGroupInvite { channel_id } => format!("invite-link:{channel_id}"),
            Cmd::ListGroupInvites { channel_id } => format!("invites:{channel_id}"),
            // Its own key: a revoke is not dropped because a list is being read.
            Cmd::RevokeInvite { channel_id, code } => format!("invite-revoke:{channel_id}:{code}"),
            Cmd::InviteToCommunity {
                user_id, guild_id, ..
            } => format!("community:{user_id}:{guild_id}"),
        }
    }

    fn budget(&self) -> Budget {
        match self {
            Cmd::AddFriend { .. } | Cmd::SendRequest { .. } => Budget::FriendRequest,
            Cmd::CreateGroupDm { .. } => Budget::GroupCreate,
            Cmd::AddRecipients { user_ids, .. } => Budget::GroupAdd(user_ids.len().max(1)),
            _ => Budget::Write,
        }
    }
}

/// How a group DM create or add ended.
#[derive(Clone, Debug, PartialEq)]
pub enum GroupDm {
    /// Done (a created group opens through [`Ev::DmOpened`]).
    Done,
    /// The instance wants a CAPTCHA Inlet could not pass (the official client fails the
    /// same way); nothing was changed and nothing is tried again on its own.
    Captcha,
    /// Over Fluxer's limit (or Inlet's): wait this long.
    RateLimited {
        retry_after: Duration,
    },
    /// Some people cannot be added: `unaddable` holds their user ID and Fluxer's reason,
    /// `addable` the user IDs that could be (nothing was created).
    NotAddable {
        unaddable: Vec<(String, String)>,
        addable: Vec<String>,
    },
    Failed(String),
    /// Some people were added (their user IDs) before the rest ended as `rest`.
    Partial {
        added: Vec<String>,
        rest: Box<GroupDm>,
    },
}

/// One invite of a group DM, as the invites window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupInvite {
    pub code: String,
    /// The link to copy.
    pub url: String,
    /// Who made it: user ID and cleaned display name.
    pub inviter: Option<(String, String)>,
    pub uses: u64,
    /// 0 for no limit.
    pub max_uses: u64,
    /// When it stops working, in Unix milliseconds.
    pub expires_ms: Option<i64>,
    /// When it was made, in Unix milliseconds.
    pub created_ms: Option<i64>,
}

/// Events of the relationships part of the social domain.
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    /// The call named `key` went through.
    Done { key: String },
    /// The call named `key` failed; `reason` says why.
    Failed { key: String, reason: String },
    /// A DM is open (or a group DM was created): the UI shows it. The channel comes with
    /// it, so the state and the list know it before the gateway's CHANNEL_CREATE.
    DmOpened {
        key: String,
        channel: Box<p::Channel>,
    },
    /// The answer to a group DM create or add (`key` as in [`Cmd::key`]).
    GroupDm { key: String, result: GroupDm },
    /// How many more people can be added to groups this hour, and when the next slot
    /// comes back when there is none.
    AddSlots { free: usize, wait: Option<Duration> },
    /// The call named `key` has run for [`SLOW_HINT`] and is not done: "still working".
    Working { key: String },
    /// A new invite to a group DM, as a link.
    InviteLink { key: String, url: String },
    /// A group DM's invites, newest first, or why they could not be read.
    Invites {
        key: String,
        channel_id: String,
        result: Result<Vec<GroupInvite>, String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Budget {
    Write,
    /// A write that is also a friend request by name.
    FriendRequest,
    GroupCreate,
    /// Adds this many people to a group.
    GroupAdd(usize),
}

/// Why a call may not start now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The same call runs already.
    Busy,
    /// Too many writes just now.
    TooFast,
    /// The group DM budget is used up, or the server asked to wait: free in `wait`.
    GroupLimit { wait: Duration },
    /// More people at once than the hour ever takes.
    TooMany { max: usize },
}

/// One account's budget for these calls.
#[derive(Default, Debug)]
pub struct Gate {
    busy: HashSet<String>,
    writes: VecDeque<Instant>,
    requests: VecDeque<Instant>,
    creates: VecDeque<Instant>,
    adds: VecDeque<Instant>,
    /// The server's `retry_after` for group DM calls ends here.
    group_wait_until: Option<Instant>,
}

fn take_slot(
    sent: &mut VecDeque<Instant>,
    max: usize,
    window: Duration,
    now: Instant,
) -> Result<(), Duration> {
    take_slots(sent, 1, max, window, now)
}

/// Takes `n` slots at once, or none: when they are not free, the wait is until enough
/// earlier calls have left the window (never more than a window).
fn take_slots(
    sent: &mut VecDeque<Instant>,
    n: usize,
    max: usize,
    window: Duration,
    now: Instant,
) -> Result<(), Duration> {
    while sent
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= window)
    {
        sent.pop_front();
    }
    if sent.len() + n > max {
        // The call is let in once `sent.len() + n - max` of the oldest have expired. More
        // than `max` at once can never be let in: report a full window.
        let leaving = (sent.len() + n).saturating_sub(max);
        let wait = sent
            .get(leaving.saturating_sub(1))
            .map_or(window, |t| (*t + window).saturating_duration_since(now));
        return Err(wait.min(window));
    }
    for _ in 0..n {
        sent.push_back(now);
    }
    Ok(())
}

impl Gate {
    /// Whether the call named `key` may start at `now`; taking it uses a slot.
    fn admit(&mut self, key: &str, budget: Budget, now: Instant) -> Result<(), Refusal> {
        if self.busy.contains(key) {
            return Err(Refusal::Busy);
        }
        match budget {
            Budget::Write => take_slot(&mut self.writes, WRITES_PER_WINDOW, WRITE_WINDOW, now)
                .map_err(|_| Refusal::TooFast)?,
            Budget::FriendRequest => {
                // Checked first, so a refused request does not use a write slot.
                let minute = Duration::from_secs(60);
                while self
                    .requests
                    .front()
                    .is_some_and(|t| now.saturating_duration_since(*t) >= minute)
                {
                    self.requests.pop_front();
                }
                if self.requests.len() >= FRIEND_REQUESTS_PER_MINUTE {
                    return Err(Refusal::TooFast);
                }
                take_slot(&mut self.writes, WRITES_PER_WINDOW, WRITE_WINDOW, now)
                    .map_err(|_| Refusal::TooFast)?;
                self.requests.push_back(now);
            }
            Budget::GroupCreate | Budget::GroupAdd(_) => {
                if let Some(until) = self.group_wait_until.filter(|u| *u > now) {
                    return Err(Refusal::GroupLimit {
                        wait: until.saturating_duration_since(now),
                    });
                }
                let (sent, n) = match budget {
                    Budget::GroupAdd(n) => (&mut self.adds, n),
                    _ => (&mut self.creates, 1),
                };
                if n > GROUP_CALLS_PER_HOUR {
                    return Err(Refusal::TooMany {
                        max: GROUP_CALLS_PER_HOUR,
                    });
                }
                take_slots(sent, n, GROUP_CALLS_PER_HOUR, GROUP_WINDOW, now)
                    .map_err(|wait| Refusal::GroupLimit { wait })?;
            }
        }
        self.busy.insert(key.to_owned());
        Ok(())
    }

    fn finish(&mut self, key: &str) {
        self.busy.remove(key);
    }

    fn is_busy(&self, key: &str) -> bool {
        self.busy.contains(key)
    }

    /// How many more people can be added now, and when the next slot is free when none is.
    fn add_slots_left(&mut self, now: Instant) -> (usize, Option<Duration>) {
        if let Some(until) = self.group_wait_until.filter(|u| *u > now) {
            return (0, Some(until.saturating_duration_since(now)));
        }
        while self
            .adds
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= GROUP_WINDOW)
        {
            self.adds.pop_front();
        }
        let free = GROUP_CALLS_PER_HOUR.saturating_sub(self.adds.len());
        let wait = (free == 0)
            .then(|| {
                self.adds
                    .front()
                    .map(|t| (*t + GROUP_WINDOW).saturating_duration_since(now))
            })
            .flatten();
        (free, wait)
    }

    /// Gives back `n` add slots an add did not use (it stopped before those people).
    fn give_back_adds(&mut self, n: usize) {
        for _ in 0..n {
            self.adds.pop_back();
        }
    }

    /// The server said to wait `wait` before the next group DM call.
    fn group_rate_limited(&mut self, now: Instant, wait: Duration) {
        let Some(until) = now.checked_add(wait.min(MAX_GROUP_WAIT)) else {
            return;
        };
        self.group_wait_until = Some(self.group_wait_until.map_or(until, |u| u.max(until)));
    }
}

/// The CAPTCHA solves that run for group creates and adds, by account and call key.
static SOLVES: LazyLock<Mutex<HashMap<(AccountKey, String), p::altcha::SolveCancel>>> =
    LazyLock::new(Default::default);

fn solves() -> std::sync::MutexGuard<'static, HashMap<(AccountKey, String), p::altcha::SolveCancel>>
{
    SOLVES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stops every CAPTCHA solve a group create or add runs: the account on screen changes
/// or its session ends. The calls answer quietly.
pub(in crate::backend) fn cancel_all_solves() {
    for cancel in solves().values() {
        cancel.cancel();
    }
}

/// The gates of every account, by account (at most a handful of entries).
static GATES: LazyLock<Mutex<HashMap<AccountKey, Gate>>> = LazyLock::new(Default::default);

fn with_gate<T>(account: &AccountKey, f: impl FnOnce(&mut Gate) -> T) -> T {
    let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    f(gates.entry(account.clone()).or_default())
}

/// The sentence for a refusal.
pub fn refusal_text(r: &Refusal) -> String {
    match r {
        Refusal::Busy => t!("social-busy"),
        Refusal::TooFast => t!("social-too-fast"),
        Refusal::GroupLimit { wait } => t!("social-group-dm-limit", wait = wait_text(*wait)),
        Refusal::TooMany { max } => t!("social-group-add-too-many", max = *max),
    }
}

/// A wait as people say it, rounded up: seconds, minutes up to 90 minutes, then hours.
pub fn wait_text(wait: Duration) -> String {
    let secs = wait.as_secs().max(1);
    if secs < 60 {
        t!("social-wait-seconds", count = secs)
    } else if secs <= 90 * 60 {
        t!("social-wait-minutes", count = secs.div_ceil(60))
    } else {
        t!("social-wait-hours", count = secs.div_ceil(3600))
    }
}

/// A failed call: the session ends on an auth failure (`None`), anything else is the
/// sentence to show.
fn failure(internal: &TokioSender<Internal>, gen: u64, err: p::Error) -> Option<String> {
    // A CAPTCHA solve that was stopped on purpose is not a failure to show.
    if matches!(err, p::Error::Cancelled) {
        return None;
    }
    let failure = Failure::from(err);
    if failure.kind == FailureKind::SessionEnded {
        let _ = internal.send(Internal::Ended {
            gen,
            message: failure.message,
        });
        return None;
    }
    Some(failure.message)
}

/// A group DM call's outcome as the UI shows it; other errors stay errors.
pub fn group_result<T>(
    result: p::Result<p::GroupDmOutcome<T>>,
) -> Result<(GroupDm, Option<T>), p::Error> {
    match result {
        Ok(p::GroupDmOutcome::Done(v)) => Ok((GroupDm::Done, Some(v))),
        Ok(p::GroupDmOutcome::Captcha) => Ok((GroupDm::Captcha, None)),
        Ok(p::GroupDmOutcome::NotAddable { unaddable, addable }) => Ok((
            GroupDm::NotAddable {
                unaddable: unaddable
                    .into_iter()
                    .map(|u| (u.user_id, u.reason))
                    .collect(),
                addable,
            },
            None,
        )),
        Ok(_) => Ok((GroupDm::Failed(t!("social-group-dm-failed")), None)),
        Err(p::Error::RateLimited { retry_after }) => {
            Ok((GroupDm::RateLimited { retry_after }, None))
        }
        Err(err) => Err(err),
    }
}

/// Reads a picked icon file, within [`MAX_ICON_BYTES`], with its type from its bytes.
fn read_icon(path: &PathBuf) -> Result<p::ImageData, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_ICON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ICON_BYTES {
        return Err(t!("social-icon-too-large"));
    }
    let kind = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return Err(t!("social-icon-not-an-image"));
    };
    // Fluxer takes static group icons only.
    if is_animated(&bytes, kind) {
        return Err(t!("social-icon-animated"));
    }
    p::ImageData::new(kind, bytes).map_err(|e| e.to_string())
}

/// Whether a picture of this type has more than one frame (an animated GIF, APNG or
/// WebP). Reads only the structure, never decodes; a file it cannot follow counts as
/// not animated, and the server has the last word.
pub fn is_animated(bytes: &[u8], kind: &str) -> bool {
    match kind {
        "image/gif" => gif_frames(bytes) > 1,
        "image/png" => png_is_animated(bytes),
        "image/webp" => webp_is_animated(bytes),
        _ => false,
    }
}

/// Frames in a GIF, counted from its block structure (at most 2 are needed).
fn gif_frames(b: &[u8]) -> usize {
    // "GIF89a", the screen descriptor (7 bytes, a global colour table when bit 7 of the
    // flags is set) and then blocks.
    let Some(&flags) = b.get(10) else { return 0 };
    let mut i = 13
        + if flags & 0x80 != 0 {
            3 << ((flags & 7) + 1)
        } else {
            0
        };
    let skip_sub_blocks = |mut i: usize| -> Option<usize> {
        loop {
            let len = *b.get(i)? as usize;
            i += 1;
            if len == 0 {
                return Some(i);
            }
            i += len;
        }
    };
    let mut frames = 0;
    while let Some(&block) = b.get(i) {
        match block {
            0x21 => match skip_sub_blocks(i + 2) {
                Some(next) => i = next,
                None => break,
            },
            0x2C => {
                frames += 1;
                if frames > 1 {
                    break;
                }
                let Some(&local) = b.get(i + 9) else { break };
                i += 10
                    + if local & 0x80 != 0 {
                        3 << ((local & 7) + 1)
                    } else {
                        0
                    };
                // The LZW code size, then the data.
                match skip_sub_blocks(i + 1) {
                    Some(next) => i = next,
                    None => break,
                }
            }
            _ => break,
        }
    }
    frames
}

/// An APNG has an `acTL` chunk before its first `IDAT`.
fn png_is_animated(b: &[u8]) -> bool {
    let mut i = 8;
    while let (Some(len), Some(kind)) = (b.get(i..i + 4), b.get(i + 4..i + 8)) {
        match kind {
            b"acTL" => return true,
            b"IDAT" | b"IEND" => return false,
            _ => {}
        }
        let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
        i = match i.checked_add(12).and_then(|n| n.checked_add(len)) {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

/// An animated WebP has the animation flag in its `VP8X` chunk, or `ANMF` frames.
fn webp_is_animated(b: &[u8]) -> bool {
    let mut i = 12;
    while let (Some(kind), Some(len)) = (b.get(i..i + 4), b.get(i + 4..i + 8)) {
        if kind == b"ANMF" || kind == b"ANIM" {
            return true;
        }
        if kind == b"VP8X" && b.get(i + 8).is_some_and(|flags| flags & 0x02 != 0) {
            return true;
        }
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]) as usize;
        // Chunks are padded to an even size.
        i = match i
            .checked_add(8)
            .and_then(|n| n.checked_add(len + (len & 1)))
        {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

/// A private or group nickname as people type it: trimmed, nothing for blank, and at
/// most 32 characters without control characters (Fluxer's own limit in its dialog).
pub fn clean_nickname(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > p::api::dms::MAX_GROUP_NICKNAME_CHARS
        || text.chars().any(char::is_control)
    {
        return Err(t!("social-nickname-invalid"));
    }
    Ok(Some(text.to_owned()))
}

pub(in crate::backend) fn handle(backend: &mut Backend, cmd: Cmd) {
    let key = cmd.key();
    let (client, gen, account, events) = match &backend.active {
        Some(Active::Real(r)) => (
            r.session.client().clone(),
            r.gen,
            r.key.clone(),
            r.events.clone(),
        ),
        #[cfg(feature = "demo")]
        Some(Active::Demo(_)) => {
            // The demo has no server: nothing changes, the call just ends.
            let _ = backend
                .events
                .unbounded_send(super::Ev::Relationships(Ev::Done { key }));
            return;
        }
        _ => {
            let _ = backend
                .events
                .unbounded_send(super::Ev::Relationships(Ev::Failed {
                    key,
                    reason: t!("social-not-connected"),
                }));
            return;
        }
    };
    // These two take no slot and no key: stopping a solve, and asking what is left.
    match &cmd {
        Cmd::CancelGroupCall { key: target } => {
            if let Some(cancel) = solves().get(&(account.clone(), target.clone())) {
                cancel.cancel();
            }
            return;
        }
        Cmd::AddSlots => {
            let (free, wait) = with_gate(&account, |g| g.add_slots_left(Instant::now()));
            let _ = events.unbounded_send(super::Ev::Relationships(Ev::AddSlots { free, wait }));
            return;
        }
        _ => {}
    }
    if let Err(refusal) = with_gate(&account, |g| g.admit(&key, cmd.budget(), Instant::now())) {
        let ev = match refusal {
            // A second click while the first runs: the first one answers.
            Refusal::Busy => return,
            Refusal::GroupLimit { wait } => Ev::GroupDm {
                key,
                result: GroupDm::RateLimited { retry_after: wait },
            },
            other => Ev::Failed {
                key,
                reason: refusal_text(&other),
            },
        };
        let _ = events.unbounded_send(super::Ev::Relationships(ev));
        return;
    }
    let internal = backend.internal.clone();
    // The CAPTCHA work can take a while: after a moment the window says so. The hint
    // ends with its call, so it never speaks for a later call with the same key.
    let hint = matches!(cmd, Cmd::CreateGroupDm { .. } | Cmd::AddRecipients { .. }).then(|| {
        backend.spawn_guarded(
            "social slow hint",
            None,
            slow_hint(events.clone(), account.clone(), key.clone()),
        )
    });
    // Made before the task, so the target is freed and the UI answered even when the
    // task panics, is aborted, or is dropped before it first runs.
    let solving = hint.is_some();
    let mut call = Call {
        account,
        key,
        events,
        hint,
        answered: false,
    };
    // A group create or add can solve a CAPTCHA for a long time: leaving stops it.
    let cancel = solving.then(p::altcha::SolveCancel::new);
    if let Some(cancel) = &cancel {
        solves().insert((call.account.clone(), call.key.clone()), cancel.clone());
    }
    backend.spawn_guarded("social action", None, async move {
        let work = run(&client, &internal, gen, &call.account, &call.key, cmd);
        let ev = match cancel {
            Some(cancel) => p::altcha::with_solve_cancel(cancel, work).await,
            None => work.await,
        };
        call.answer(ev);
    });
}

/// After [`SLOW_HINT`], tells the UI that the call named `key` is still running.
async fn slow_hint(events: super::super::Ui, account: AccountKey, key: String) {
    tokio::time::sleep(SLOW_HINT).await;
    if with_gate(&account, |g| g.is_busy(&key)) {
        let _ = events.unbounded_send(super::Ev::Relationships(Ev::Working { key }));
    }
}

/// One admitted call: its gate slot is freed and the UI answered exactly once, also
/// when the task panics or is aborted (then with a failure).
struct Call {
    account: AccountKey,
    key: String,
    events: super::super::Ui,
    /// The call's "still working" timer, stopped when the call ends.
    hint: Option<tokio::task::AbortHandle>,
    answered: bool,
}

impl Call {
    fn answer(&mut self, ev: Ev) {
        self.answered = true;
        if let Some(hint) = self.hint.take() {
            hint.abort();
        }
        solves().remove(&(self.account.clone(), self.key.clone()));
        with_gate(&self.account, |g| g.finish(&self.key));
        let _ = self.events.unbounded_send(super::Ev::Relationships(ev));
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if !self.answered {
            let ev = Ev::Failed {
                key: self.key.clone(),
                reason: t!("social-action-failed"),
            };
            self.answer(ev);
        }
    }
}

/// Runs one admitted call and says how it ended.
async fn run(
    client: &p::Client,
    internal: &TokioSender<Internal>,
    gen: u64,
    account: &AccountKey,
    key: &str,
    cmd: Cmd,
) -> Ev {
    let key = key.to_owned();
    let failed = |err: p::Error, key: String| match failure(internal, gen, err) {
        Some(reason) => Ev::Failed { key, reason },
        // The session ended: the sign-in screen says why.
        None => Ev::Done { key },
    };
    let done = |r: p::Result<()>, key: String| match r {
        Ok(()) => Ev::Done { key },
        Err(err) => failed(err, key),
    };
    let opened = |channel: p::Channel, key: String| Ev::DmOpened {
        key,
        channel: Box::new(channel),
    };
    match cmd {
        Cmd::AddFriend {
            username,
            discriminator,
        } => done(
            client
                .send_friend_request_by_tag(&username, discriminator.as_deref())
                .await
                .map(drop),
            key,
        ),
        Cmd::SendRequest { user_id } => {
            done(client.send_friend_request(&user_id).await.map(drop), key)
        }
        Cmd::Accept { user_id } => {
            done(client.accept_friend_request(&user_id).await.map(drop), key)
        }
        Cmd::Remove { user_id } => done(client.remove_relationship(&user_id).await, key),
        Cmd::Block { user_id } => done(client.block_user(&user_id).await.map(drop), key),
        Cmd::OpenDm { user_id } => match client.open_dm(&user_id).await {
            Ok(channel) => opened(channel, key),
            Err(err) => failed(err, key),
        },
        Cmd::CreateGroupDm { recipients } => {
            match group_result(client.create_group_dm(&recipients).await) {
                Ok((_, Some(channel))) => opened(channel, key),
                Ok((result, None)) => group_answer(account, key, result),
                Err(err) => group_failed(internal, gen, key, err),
            }
        }
        Cmd::AddRecipients {
            channel_id,
            user_ids,
        } => {
            // One call each; the first that does not go through ends the rest (a
            // CAPTCHA or a limit would stop the others too), and says who was added.
            let mut added = Vec::new();
            for (done_before, user_id) in user_ids.iter().enumerate() {
                let attempted = done_before + 1;
                let ended =
                    match group_result(client.add_group_dm_recipient(&channel_id, user_id).await) {
                        Ok((GroupDm::Done, _)) => {
                            added.push(user_id.clone());
                            continue;
                        }
                        Ok((result, _)) => group_answer(account, key, result),
                        Err(err) => group_failed(internal, gen, key, err),
                    };
                // The people never reached did not use their slots.
                with_gate(account, |g| g.give_back_adds(user_ids.len() - attempted));
                return with_added(ended, added);
            }
            group_answer(account, key, GroupDm::Done)
        }
        Cmd::RemoveRecipient {
            channel_id,
            user_id,
        } => done(
            client
                .remove_group_dm_recipient(&channel_id, &user_id, false)
                .await,
            key,
        ),
        Cmd::CloseDm { channel } => done(client.close_dm(&channel, false).await, key),
        Cmd::EditGroupDm { channel, change } => {
            let icon = match change.icon {
                None => None,
                Some(Icon::Remove) => Some(None),
                Some(Icon::File(path)) => {
                    match tokio::task::spawn_blocking(move || read_icon(&path)).await {
                        Ok(Ok(data)) => Some(Some(data)),
                        Ok(Err(reason)) => return Ev::Failed { key, reason },
                        Err(_) => {
                            return Ev::Failed {
                                key,
                                reason: t!("social-icon-not-an-image"),
                            }
                        }
                    }
                }
            };
            let edit = p::GroupDmEdit {
                name: change.name,
                icon,
                owner_id: change.owner_id,
                nicks: change.nicks.into_iter().collect::<BTreeMap<_, _>>(),
                nsfw: change.nsfw,
            };
            done(client.edit_group_dm(&channel, &edit).await.map(drop), key)
        }
        Cmd::PinDm { channel_id, pin } => done(
            if pin {
                client.pin_dm(&channel_id).await
            } else {
                client.unpin_dm(&channel_id).await
            },
            key,
        ),
        // Answered in `handle`, before a call starts.
        Cmd::CancelGroupCall { .. } | Cmd::AddSlots => Ev::Done { key },
        Cmd::SetFriendNickname { user_id, nickname } => {
            let nickname = match nickname.as_deref().map(clean_nickname).transpose() {
                Ok(n) => n.flatten(),
                Err(reason) => return Ev::Failed { key, reason },
            };
            done(
                client
                    .set_friend_nickname(&user_id, nickname.as_deref())
                    .await
                    .map(drop),
                key,
            )
        }
        Cmd::CreateGroupInvite { channel_id } => {
            let params = p::InviteParams {
                max_age: Some(p::api::invites::FRIEND_INVITE_AGE_SECONDS),
                ..Default::default()
            };
            match client.create_invite(&channel_id, params).await {
                Ok(invite) => match p::api::invites::invite_url(client.instance(), &invite.code) {
                    Some(url) => Ev::InviteLink { key, url },
                    None => Ev::Failed {
                        key,
                        reason: t!("social-invite-link-failed"),
                    },
                },
                Err(err) => failed(err, key),
            }
        }
        Cmd::ListGroupInvites { channel_id } => match client.channel_invites(&channel_id).await {
            Ok(list) => Ev::Invites {
                key,
                channel_id,
                // An entry whose code could not go in a link is not shown.
                result: Ok(list
                    .iter()
                    .filter(|i| p::api::invites::invite_code(&i.code).is_ok())
                    .map(|i| group_invite(client.instance(), i))
                    .collect()),
            },
            Err(err) => match failure(internal, gen, err) {
                Some(reason) => Ev::Invites {
                    key,
                    channel_id,
                    result: Err(reason),
                },
                // The session ended: the sign-in screen says why.
                None => Ev::Done { key },
            },
        },
        Cmd::RevokeInvite { code, .. } => done(client.delete_invite(&code).await, key),
        Cmd::InviteToCommunity {
            user_id,
            channel_id,
            vanity,
            ..
        } => {
            let code = match vanity {
                Some(code) => code,
                None => match client
                    .create_invite(&channel_id, p::InviteParams::default())
                    .await
                {
                    Ok(invite) => invite.code,
                    Err(err) => return failed(err, key),
                },
            };
            let Some(url) = p::api::invites::invite_url(client.instance(), &code) else {
                return Ev::Failed {
                    key,
                    reason: t!("social-invite-link-failed"),
                };
            };
            let dm = match client.open_dm(&user_id).await {
                Ok(dm) => dm,
                Err(err) => return failed(err, key),
            };
            done(
                client
                    .send_message(&dm.id, &url, Some(&p::rest::new_nonce()))
                    .await
                    .map(drop),
                key,
            )
        }
    }
}

/// An invite of a group DM as the invites window shows it.
fn group_invite(instance: &p::Instance, invite: &p::Invite) -> GroupInvite {
    let millis = |text: &Option<String>| {
        text.as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.timestamp_millis())
    };
    GroupInvite {
        code: invite.code.clone(),
        url: p::api::invites::invite_url(instance, &invite.code).unwrap_or_default(),
        inviter: invite
            .inviter
            .as_ref()
            .map(|u| (u.id.clone(), crate::model::clean_name(u.display_name()))),
        uses: invite.uses.unwrap_or(0),
        max_uses: invite.max_uses.unwrap_or(0),
        expires_ms: millis(&invite.expires_at),
        created_ms: millis(&invite.created_at),
    }
}

/// A group DM answer; a rate limit also closes the account's group DM calls until the
/// wait is over.
fn group_answer(account: &AccountKey, key: String, result: GroupDm) -> Ev {
    if let GroupDm::RateLimited { retry_after } = &result {
        with_gate(account, |g| {
            g.group_rate_limited(Instant::now(), *retry_after)
        });
    }
    Ev::GroupDm { key, result }
}

/// A group answer that says who was added before it, when anyone was.
fn with_added(ev: Ev, added: Vec<String>) -> Ev {
    match ev {
        Ev::GroupDm { key, result } if !added.is_empty() => Ev::GroupDm {
            key,
            result: GroupDm::Partial {
                added,
                rest: Box::new(result),
            },
        },
        other => other,
    }
}

fn group_failed(internal: &TokioSender<Internal>, gen: u64, key: String, err: p::Error) -> Ev {
    if matches!(&err, p::Error::Api(api) if api.code == "MAX_GROUP_DM_RECIPIENTS") {
        return Ev::GroupDm {
            key,
            result: GroupDm::Failed(t!("social-group-full")),
        };
    }
    match failure(internal, gen, err) {
        Some(reason) => Ev::GroupDm {
            key,
            result: GroupDm::Failed(reason),
        },
        // The session ended: the sign-in screen says why; the call just ends.
        None => Ev::Done { key },
    }
}

#[cfg(test)]
mod tests;
