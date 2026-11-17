//! Message windows: the most recent messages of a few channels, in memory only.
//!
//! A window holds at most [`MAX_MESSAGES_PER_CHANNEL`] messages of one
//! channel in ID order, and at most [`MAX_MESSAGE_WINDOWS`] channels have a
//! window; the least recently used one is evicted. Pages and live events are
//! merged, never swapped (A2 from the v0 app review):
//!
//! - The app opens a window with [`crate::AccountState::begin_load`] before
//!   it asks for the newest page. Until that page lands the window is
//!   *loading*: live creates and edits wait in a small buffer, deletes are
//!   remembered.
//! - After a new READY every window is *stale* and behaves the same way until
//!   its newest page is loaded again.
//! - When the page lands, buffered messages newer than the page are added,
//!   buffered edits win when they are newer, and remembered deletes stay
//!   deleted. If the buffer overflowed, the window stays loading
//!   ([`MessageWindow::has_newest`] is `false`): load the newest page again.

use std::collections::{BTreeMap, VecDeque};

use inlet_protocol::models::{parse_timestamp_ms, Emoji, Reaction};
use inlet_protocol::Message;
use serde_json::{Map, Value};

use crate::bounded::LruMap;
use crate::ids::{ChannelId, MessageId};
use crate::limits::{
    LOAD_TIMEOUT_MS, MAX_BACKGROUND_MESSAGES, MAX_MESSAGES_PER_CHANNEL, MAX_MESSAGE_BYTES,
    MAX_MESSAGE_CACHE, MAX_MESSAGE_WINDOWS, MAX_PENDING_LIVE, MAX_PENDING_REACTIONS,
    MAX_REACTIONS_PER_MESSAGE, MAX_RECENTLY_DELETED, MAX_REOPEN_MESSAGES, MAX_TRUNCATED_VIEW,
    MAX_WINDOW_BYTES,
};
use crate::size::{heap_map, heap_value, opt_len};

/// One page from `GET /channels/{id}/messages`, as the protocol crate's
/// `Client::messages` returns it (any order; newest first from the API).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MessagesPage {
    /// The messages.
    pub messages: Vec<Message>,
    /// The `before` the page was asked with; `None` for the newest page.
    pub before: Option<MessageId>,
    /// The `limit` it was asked with (1-100). Fewer messages than this
    /// means the start of the channel was reached.
    pub limit: u8,
}

impl MessagesPage {
    /// The newest messages of a channel.
    pub fn latest(messages: Vec<Message>, limit: u8) -> Self {
        MessagesPage {
            messages,
            before: None,
            limit,
        }
    }

    /// Messages older than `before`.
    pub fn before(before: MessageId, messages: Vec<Message>, limit: u8) -> Self {
        MessagesPage {
            messages,
            before: Some(before),
            limit,
        }
    }

    fn reached_start(&self) -> bool {
        self.messages.len() < usize::from(self.limit.max(1))
    }
}

/// The key a message over [`MAX_MESSAGE_BYTES`] carries in `extra` after it
/// was cut down to a stub (no content, embeds, attachments or mentions).
pub const OVERSIZED_KEY: &str = "inlet_oversized";

/// Whether a stored message is a stub for an oversized message.
pub fn is_oversized(m: &Message) -> bool {
    m.extra.get(OVERSIZED_KEY) == Some(&Value::Bool(true))
}

/// A copy fit for storage: the author trimmed, and a message over the byte
/// cap replaced by a stub, so the window keeps its place without a gap.
fn prepare(m: &Message) -> Message {
    let mut m = m.clone();
    crate::size::cap_user(&mut m.author);
    if approx_size(&m) > MAX_MESSAGE_BYTES {
        m.content.clear();
        m.embeds.clear();
        m.attachments.clear();
        m.mentions.clear();
        m.flags = None;
        m.nonce = None;
        crate::size::truncate(&mut m.timestamp, 64);
        m.edited_timestamp = None;
        m.extra = Map::new();
        m.extra.insert(OVERSIZED_KEY.into(), Value::Bool(true));
    }
    m
}

/// The loaded messages of one channel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MessageWindow {
    messages: BTreeMap<MessageId, Message>,
    /// Approximate memory of the messages ([`approx_size`]).
    bytes: usize,
    has_more_before: bool,
    has_newest: bool,
    /// Live creates and edits while the window waits for its newest page.
    pending: BTreeMap<MessageId, Message>,
    /// Reaction events while the window waits, applied after the page.
    pending_reactions: Vec<(MessageId, ReactionOp)>,
    /// Something was lost while waiting (buffer or delete memory full): the
    /// page merge leaves the window waiting for another page.
    pending_overflow: bool,
    /// A newest page was asked for and has not landed: evicted last.
    loading: bool,
    /// When the load began (the account clock), for the load timeout.
    loading_since_ms: i64,
    /// Marked stale by a new READY until its newest page lands.
    stale: bool,
    /// The channel is not open in the view: the window keeps only
    /// [`MAX_BACKGROUND_MESSAGES`] messages (see [`MessageWindows::set_open`]).
    background: bool,
    /// Messages cut from the older end of the list, nearest first kept: scrolling back
    /// takes them before asking the server ([`MessageWindow::drain_older`]). Fluxer's client
    /// keeps the same (its `beforeBuffer`, up to [`MAX_MESSAGE_CACHE`]).
    before_buf: BTreeMap<MessageId, Message>,
    /// Messages cut from the newer end after older pages were loaded
    /// ([`MessageWindow::drain_newer`]).
    after_buf: BTreeMap<MessageId, Message>,
    /// Approximate memory of both buffers; counts with `bytes` against the window's budget.
    buf_bytes: usize,
    recently_deleted: VecDeque<MessageId>,
}

impl MessageWindow {
    /// Messages, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Message> {
        self.messages.values()
    }

    /// One message.
    pub fn get(&self, id: MessageId) -> Option<&Message> {
        self.messages.get(&id)
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the window is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// The oldest loaded message; load older ones with `before` set to it.
    pub fn oldest_id(&self) -> Option<MessageId> {
        self.messages.keys().next().copied()
    }

    /// The newest loaded message.
    pub fn newest_id(&self) -> Option<MessageId> {
        self.messages.keys().next_back().copied()
    }

    /// Whether older messages exist on the server.
    pub fn has_more_before(&self) -> bool {
        self.has_more_before
    }

    /// Whether the window reaches the newest message, so live messages are
    /// appended. `false` while loading, after a reconnect, after scrolling
    /// far back, or when the live buffer overflowed: load the newest page.
    pub fn has_newest(&self) -> bool {
        self.has_newest
    }

    /// Live messages waiting for the newest page.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Whether a newest page was asked for and has not landed.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Approximate memory the messages take, in bytes: the list and the two buffers.
    pub fn approx_bytes(&self) -> usize {
        self.bytes + self.buf_bytes
    }

    /// Messages waiting in the buffers (older side, newer side).
    pub fn buffered(&self) -> (usize, usize) {
        (self.before_buf.len(), self.after_buf.len())
    }

    fn put(&mut self, id: MessageId, m: Message) {
        self.bytes = self.bytes.saturating_add(approx_size(&m));
        if let Some(old) = self.messages.insert(id, m) {
            self.bytes = self.bytes.saturating_sub(approx_size(&old));
        }
    }

    fn take(&mut self, id: MessageId) -> Option<Message> {
        let m = self.messages.remove(&id)?;
        self.bytes = self.bytes.saturating_sub(approx_size(&m));
        Some(m)
    }

    /// The count above which the list is cut, and what it is cut to (Fluxer's client cuts a
    /// loaded list of more than 200 to 100; a window that is not on screen keeps 100).
    fn count_limits(&self) -> (usize, usize) {
        if self.background {
            (MAX_BACKGROUND_MESSAGES, MAX_BACKGROUND_MESSAGES)
        } else {
            (MAX_MESSAGES_PER_CHANNEL, MAX_TRUNCATED_VIEW)
        }
    }

    fn over_bytes(&self) -> bool {
        self.bytes + self.buf_bytes > MAX_WINDOW_BYTES && self.messages.len() > 1
    }

    fn clear_buffers(&mut self) {
        self.before_buf.clear();
        self.after_buf.clear();
        self.buf_bytes = 0;
    }

    /// A buffered message changed or went: the buffers are a cache, so they are dropped.
    fn forget_buffered(&mut self, id: MessageId) {
        if self.before_buf.contains_key(&id) || self.after_buf.contains_key(&id) {
            self.clear_buffers();
        }
    }

    fn buffer_older(&mut self, id: MessageId, m: Message) {
        self.buf_bytes += approx_size(&m);
        self.before_buf.insert(id, m);
        // The furthest go first.
        while self.before_buf.len() > MAX_MESSAGE_CACHE {
            if let Some((_, old)) = self.before_buf.pop_first() {
                self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&old));
            }
        }
    }

    fn buffer_newer(&mut self, id: MessageId, m: Message) {
        self.buf_bytes += approx_size(&m);
        self.after_buf.insert(id, m);
        while self.after_buf.len() > MAX_MESSAGE_CACHE {
            if let Some((_, old)) = self.after_buf.pop_last() {
                self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&old));
            }
        }
    }

    fn forget_before(&mut self) {
        for (_, m) in std::mem::take(&mut self.before_buf) {
            self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&m));
        }
    }

    fn forget_after(&mut self) {
        for (_, m) in std::mem::take(&mut self.after_buf) {
            self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&m));
        }
    }

    /// Cuts the oldest message off the list into the buffer (a window that is not on screen
    /// keeps nothing but its newest messages, and no buffer).
    fn cut_oldest(&mut self) -> bool {
        let Some(id) = self.oldest_id() else {
            return false;
        };
        let Some(m) = self.take(id) else {
            return false;
        };
        self.has_more_before = true;
        if self.background {
            self.forget_before();
        } else {
            self.buffer_older(id, m);
        }
        true
    }

    fn cut_newest(&mut self) -> bool {
        let Some(id) = self.newest_id() else {
            return false;
        };
        let Some(m) = self.take(id) else {
            return false;
        };
        self.has_newest = false;
        if self.background {
            self.forget_after();
        } else {
            self.buffer_newer(id, m);
        }
        true
    }

    /// Frees bytes beyond the budget: the buffers first (the furthest messages), then the
    /// list from the end `oldest` names.
    fn shed(&mut self, oldest: bool) {
        while self.over_bytes() {
            if let Some((_, m)) = self.before_buf.pop_first() {
                self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&m));
            } else if let Some((_, m)) = self.after_buf.pop_last() {
                self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&m));
            } else {
                let id = if oldest {
                    self.oldest_id()
                } else {
                    self.newest_id()
                };
                let Some(id) = id else { break };
                self.take(id);
                if oldest {
                    self.has_more_before = true;
                } else {
                    self.has_newest = false;
                }
            }
        }
    }

    /// Cuts the list to its `n` newest messages (they go to the buffer). Whether it cut any.
    fn cut_to(&mut self, n: usize) -> bool {
        let before = self.messages.len();
        while self.messages.len() > n {
            if !self.cut_oldest() {
                break;
            }
        }
        self.shed(true);
        self.messages.len() < before
    }

    /// Takes up to `limit` messages from the older buffer into the list, as scrolling back
    /// asks for older ones. Returns whether there were any.
    fn drain_older(&mut self, limit: usize) -> bool {
        let mut any = false;
        for _ in 0..limit {
            let Some((id, m)) = self.before_buf.pop_last() else {
                break;
            };
            self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&m));
            self.put(id, m);
            any = true;
        }
        if any {
            self.trim_newest();
        }
        any
    }

    /// The same from the newer buffer, as scrolling down asks for newer ones.
    fn drain_newer(&mut self, limit: usize) -> bool {
        let mut any = false;
        for _ in 0..limit {
            let Some((id, m)) = self.after_buf.pop_first() else {
                break;
            };
            self.buf_bytes = self.buf_bytes.saturating_sub(approx_size(&m));
            self.put(id, m);
            any = true;
        }
        if any {
            self.trim_oldest();
        }
        any
    }

    fn was_deleted(&self, id: MessageId) -> bool {
        self.recently_deleted.contains(&id)
    }

    fn remember_deleted(&mut self, id: MessageId) {
        if self.was_deleted(id) {
            return;
        }
        if self.recently_deleted.len() >= MAX_RECENTLY_DELETED {
            self.recently_deleted.pop_front();
            // A delete forgotten while a page is on its way could come back
            // with that page: ask for another one instead.
            if !self.has_newest {
                self.pending_overflow = true;
            }
        }
        self.recently_deleted.push_back(id);
    }

    /// Cuts the oldest messages when the list passes its limit (down to what Fluxer's
    /// client cuts it to), then frees bytes beyond the budget.
    fn trim_oldest(&mut self) {
        let (limit, target) = self.count_limits();
        if self.messages.len() > limit {
            while self.messages.len() > target {
                if !self.cut_oldest() {
                    break;
                }
            }
        }
        self.shed(true);
    }

    /// The same for the newest messages (after older pages were loaded).
    fn trim_newest(&mut self) {
        let (limit, target) = self.count_limits();
        if self.messages.len() > limit {
            while self.messages.len() > target {
                if !self.cut_newest() {
                    break;
                }
            }
        }
        self.shed(false);
    }

    /// Buffers a live create or edit while the window is loading.
    fn buffer(&mut self, id: MessageId, mut m: Message) {
        if let Some(old) = self.pending.get(&id) {
            if edited_ms(&m) < edited_ms(old) {
                return;
            }
            carry_reactions(old, &mut m);
        }
        self.pending.insert(id, m);
        if self.pending.len() > MAX_PENDING_LIVE {
            self.pending.pop_first();
            self.pending_overflow = true;
        }
    }

    /// Merges the newest page and the live buffer.
    fn merge_latest(&mut self, page: BTreeMap<MessageId, Message>, reached_start: bool) {
        self.clear_buffers();
        let pending = std::mem::take(&mut self.pending);
        let reactions = std::mem::take(&mut self.pending_reactions);
        let overflow = std::mem::take(&mut self.pending_overflow);
        self.loading = false;
        self.stale = false;
        let fresh = self.has_newest;
        let mut merged = BTreeMap::new();
        let mut has_more_before = !reached_start;
        let page_newest = page.keys().next_back().copied();
        match (page.keys().next().copied(), page_newest) {
            (Some(p_old), Some(p_new)) => {
                let contiguous = self.messages.contains_key(&p_old);
                let mut old = std::mem::take(&mut self.messages);
                let newer = old.split_off(&MessageId(p_new.0.saturating_add(1)));
                let mut in_range = old.split_off(&p_old);
                if contiguous {
                    merged.extend(old);
                    has_more_before = self.has_more_before;
                }
                for (id, m) in page {
                    let chosen = match in_range.remove(&id) {
                        Some(live) if fresh && edited_ms(&live) >= edited_ms(&m) => live,
                        _ => m,
                    };
                    merged.insert(id, chosen);
                }
                // Messages in the page's range that the page lacks were deleted.
                if fresh {
                    merged.extend(newer);
                }
            }
            _ => {
                // The channel had no messages when the page was read; keep
                // what arrived live since then.
                if fresh {
                    merged = std::mem::take(&mut self.messages);
                }
                has_more_before = false;
            }
        }
        for (id, mut m) in pending {
            if self.was_deleted(id) {
                continue;
            }
            match merged.get(&id) {
                Some(cur) => {
                    if edited_ms(&m) >= edited_ms(cur) {
                        carry_reactions(cur, &mut m);
                        merged.insert(id, m);
                    }
                }
                // A create newer than the page; after an overflow the
                // buffer has a hole, so wait for the next page instead.
                None if !overflow && page_newest.is_none_or(|n| id > n) => {
                    merged.insert(id, m);
                }
                None => {}
            }
        }
        // Reactions that arrived while waiting, in order. After an overflow
        // the next page brings the server's counts instead.
        if !overflow {
            for (id, op) in &reactions {
                if let Some(m) = merged.get_mut(id) {
                    apply_reaction(m, op);
                }
            }
        }
        self.bytes = merged.values().map(approx_size).sum();
        self.messages = merged;
        self.has_more_before = has_more_before;
        self.has_newest = !overflow;
        self.trim_oldest();
    }

    /// Merges a page of messages older than `before`. Returns `false` when
    /// the page no longer fits the window (it was replaced meanwhile).
    fn merge_older(
        &mut self,
        before: MessageId,
        page: BTreeMap<MessageId, Message>,
        reached_start: bool,
    ) -> bool {
        if !self.messages.contains_key(&before) {
            return false;
        }
        let at_oldest = self.oldest_id() == Some(before);
        // The page covers what the older buffer held.
        self.forget_before();
        for (id, m) in page.range(..before) {
            if !self.messages.contains_key(id) {
                self.put(*id, m.clone());
            }
        }
        if at_oldest {
            self.has_more_before = !reached_start;
        }
        self.trim_newest();
        true
    }

    /// Replaces the window with a page around a message (a jump). Older
    /// messages are always taken to exist; scrolling up finds out.
    ///
    /// When the page reaches the channel's newest message (`reaches_newest`),
    /// the window keeps the newest messages: live messages newer than the
    /// page (in the window, or buffered while it waited) stay. Otherwise the
    /// window misses its newest messages ([`MessageWindow::has_newest`] is
    /// `false`): live messages are buffered for the next newest page, as for a
    /// window scrolled far back.
    fn replace_around(&mut self, page: BTreeMap<MessageId, Message>, reaches_newest: bool) {
        self.clear_buffers();
        let fresh = self.has_newest;
        let page_newest = page.keys().next_back().copied();
        let mut old = std::mem::take(&mut self.messages);
        let mut merged = page;
        // A message the window already had and that changed live meanwhile (an edit, a
        // reaction) keeps that change, as in `merge_latest`.
        if fresh {
            for (id, m) in merged.iter_mut() {
                if let Some(live) = old.get(id) {
                    if edited_ms(live) >= edited_ms(m) {
                        *m = live.clone();
                    }
                }
            }
        }
        // The page carries the server's reactions; buffered ones are in it or
        // belong to messages it does not have.
        self.pending_reactions.clear();
        self.loading = false;
        self.stale = false;
        if reaches_newest {
            if let (true, Some(n)) = (fresh, page_newest) {
                merged.extend(old.split_off(&MessageId(n.0.saturating_add(1))));
            }
            let overflow = std::mem::take(&mut self.pending_overflow);
            for (id, m) in std::mem::take(&mut self.pending) {
                if !overflow && !self.was_deleted(id) && page_newest.is_none_or(|n| id > n) {
                    merged.insert(id, m);
                }
            }
            self.has_newest = !overflow;
        } else {
            self.has_newest = false;
        }
        self.bytes = merged.values().map(approx_size).sum();
        self.messages = merged;
        self.has_more_before = true;
        self.trim_oldest();
    }

    /// A live MESSAGE_CREATE. Returns whether the visible messages changed.
    fn insert_live(&mut self, id: MessageId, m: &Message) -> bool {
        if self.was_deleted(id) {
            return false;
        }
        let mut m = prepare(m);
        if !self.has_newest {
            self.buffer(id, m);
            return false;
        }
        // An old message arriving late would leave a gap below the window.
        if self.has_more_before && self.oldest_id().is_some_and(|o| id < o) {
            return false;
        }
        if let Some(old) = self.messages.get(&id) {
            if old == &m {
                return false;
            }
            carry_reactions(old, &mut m);
        }
        self.put(id, m);
        self.trim_oldest();
        true
    }

    /// A MESSAGE_UPDATE. Edits that are older than the stored one are ignored.
    fn update(&mut self, id: MessageId, m: &Message) -> bool {
        if self.was_deleted(id) {
            return false;
        }
        let mut m = prepare(m);
        if !self.has_newest {
            self.buffer(id, m.clone());
        }
        self.forget_buffered(id);
        let Some(old) = self.messages.get(&id) else {
            return false;
        };
        if edited_ms(&m) < edited_ms(old) || old == &m {
            return false;
        }
        carry_reactions(old, &mut m);
        self.put(id, m);
        self.trim_oldest();
        true
    }

    fn delete(&mut self, id: MessageId) -> bool {
        self.remember_deleted(id);
        self.pending.remove(&id);
        self.forget_buffered(id);
        self.take(id).is_some()
    }

    /// A reaction event. While the window is loading or stale the event is
    /// buffered and applied after the merge, like creates; a window that is
    /// only scrolled back applies it to the messages it holds.
    fn react(&mut self, id: MessageId, op: ReactionOp) -> bool {
        if self.loading || self.stale {
            // A repeat of an idempotent event (the account's own reaction, a
            // clear) adds nothing.
            let idempotent = match &op {
                ReactionOp::Add { me, .. } | ReactionOp::Remove { me, .. } => *me,
                ReactionOp::Clear(_) => true,
            };
            let repeat = self
                .pending_reactions
                .iter()
                .rev()
                .find(|(m, _)| *m == id)
                .is_some_and(|(_, last)| *last == op);
            if idempotent && repeat {
                return false;
            }
            if self.pending_reactions.len() >= MAX_PENDING_REACTIONS {
                self.pending_overflow = true;
            } else {
                self.pending_reactions.push((id, op));
            }
            return false;
        }
        self.forget_buffered(id);
        let Some(m) = self.messages.get(&id) else {
            return false;
        };
        let mut m = m.clone();
        if !apply_reaction(&mut m, &op) {
            return false;
        }
        self.put(id, m);
        true
    }
}

/// One reaction event.
#[derive(Debug, Clone, PartialEq)]
enum ReactionOp {
    /// Someone reacted; `me` when it was the account.
    Add { emoji: Emoji, me: bool },
    /// A reaction was removed.
    Remove { emoji: Emoji, me: bool },
    /// Every reaction, or every reaction with one emoji, was removed.
    Clear(Option<Emoji>),
}

/// Applies a reaction event to a message. Returns whether it changed.
fn apply_reaction(m: &mut Message, op: &ReactionOp) -> bool {
    let mut rs = m.reactions();
    let changed = match op {
        ReactionOp::Add { emoji, me } => {
            if let Some(r) = rs.iter_mut().find(|r| r.emoji.same_reaction(emoji)) {
                // The account's own reaction is idempotent; others' cannot be told apart.
                if *me && r.me {
                    false
                } else {
                    r.count = r.count.saturating_add(1);
                    r.me |= *me;
                    if emoji.animated {
                        r.emoji.animated = true;
                    }
                    true
                }
            } else if rs.len() >= MAX_REACTIONS_PER_MESSAGE {
                false
            } else {
                rs.push(Reaction {
                    emoji: emoji.clone(),
                    count: 1,
                    me: *me,
                });
                true
            }
        }
        ReactionOp::Remove { emoji, me } => {
            match rs.iter().position(|r| r.emoji.same_reaction(emoji)) {
                Some(i) if !*me || rs[i].me => {
                    rs[i].count = rs[i].count.saturating_sub(1);
                    if *me {
                        rs[i].me = false;
                    }
                    if rs[i].count == 0 {
                        rs.remove(i);
                    }
                    true
                }
                _ => false,
            }
        }
        ReactionOp::Clear(emoji) => {
            let before = rs.len();
            match emoji {
                Some(e) => rs.retain(|r| !r.emoji.same_reaction(e)),
                None => rs.clear(),
            }
            rs.len() != before
        }
    };
    if changed {
        let value = serde_json::to_value(&rs).unwrap_or(Value::Array(Vec::new()));
        m.extra.insert("reactions".into(), value);
    }
    changed
}

/// The heap a stored message takes, as the program asks the allocator for it (checked
/// against a counting allocator in `tests/window_heap.rs`, to within 20 %): the message
/// as it sits in the window's tree (a leaf holds 11 and is rarely full), the text of every
/// string, and every JSON part (`extra`, embeds, flags) with the entries and tables of its
/// objects. It is what caps a window's size, so a server sending huge messages cannot fill
/// memory. The allocator adds about 16 bytes to each block on top.
pub fn approx_size(m: &Message) -> usize {
    use std::mem::size_of;
    fn user(u: &inlet_protocol::User) -> usize {
        u.id.len()
            + u.username.len()
            + u.discriminator.len()
            + opt_len(&u.global_name)
            + opt_len(&u.avatar)
            + u.flags.as_ref().map_or(0, heap_value)
            + heap_map(&u.extra)
    }
    let attachments: usize = m
        .attachments
        .iter()
        .map(|a| {
            size_of::<inlet_protocol::Attachment>()
                + a.id.len()
                + a.filename.len()
                + opt_len(&a.content_type)
                + opt_len(&a.url)
                + opt_len(&a.proxy_url)
                + opt_len(&a.description)
                + heap_map(&a.extra)
        })
        .sum();
    let mentions: usize = m
        .mentions
        .iter()
        .map(|u| size_of::<inlet_protocol::User>() + user(u))
        .sum();
    let embeds: usize = m
        .embeds
        .iter()
        .map(|e| size_of::<Value>() + heap_value(e))
        .sum();
    TREE_SLOT
        + size_of::<Message>()
        + m.id.len()
        + m.channel_id.len()
        + opt_len(&m.guild_id)
        + m.content.len()
        + m.timestamp.len()
        + opt_len(&m.edited_timestamp)
        + opt_len(&m.nonce)
        + m.flags.as_ref().map_or(0, heap_value)
        + user(&m.author)
        + mentions
        + embeds
        + heap_map(&m.extra)
        + attachments
}

/// What a message costs in the window's tree beyond its own size: the key, and the empty
/// slots of leaves that are not full (calibrated with the counting allocator).
const TREE_SLOT: usize = 160;

/// The edit time in milliseconds; an unedited message is 0.
fn edited_ms(m: &Message) -> i64 {
    m.edited_timestamp
        .as_deref()
        .and_then(parse_timestamp_ms)
        .unwrap_or(0)
}

/// Keeps the stored reactions when an update does not carry any.
fn carry_reactions(old: &Message, new: &mut Message) {
    if !new.extra.contains_key("reactions") {
        if let Some(r) = old.extra.get("reactions") {
            new.extra.insert("reactions".into(), r.clone());
        }
    }
}

/// The message windows of one account.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageWindows {
    windows: LruMap<ChannelId, MessageWindow>,
    /// The channels the view shows (the chat, the thread panel); `None` until the app says
    /// so, and then no window is trimmed.
    open: Option<[Option<ChannelId>; 2]>,
}

impl Default for MessageWindows {
    fn default() -> Self {
        MessageWindows {
            windows: LruMap::new(MAX_MESSAGE_WINDOWS),
            open: None,
        }
    }
}

/// Parses a page: messages of `channel` with a snowflake ID, by ID.
fn page_map(channel: ChannelId, messages: &[Message]) -> BTreeMap<MessageId, Message> {
    messages
        .iter()
        .filter(|m| ChannelId::parse(&m.channel_id) == Some(channel))
        .filter_map(|m| Some((MessageId::parse(&m.id)?, prepare(m))))
        .collect()
}

impl MessageWindows {
    /// Whether a window made now for `channel` is for a channel that is not open.
    fn is_background(&self, channel: ChannelId) -> bool {
        self.open.is_some_and(|open| !open.contains(&Some(channel)))
    }

    /// Says which channel the view shows in `slot` (0 the chat, 1 the thread panel; `None`
    /// when it shows none), and applies Fluxer's rules for what a window keeps:
    /// - a channel that is not shown keeps only its newest [`MAX_BACKGROUND_MESSAGES`]
    ///   messages and no buffer (cut at once, and as live messages arrive);
    /// - a channel that is shown again is cut to its newest [`MAX_REOPEN_MESSAGES`], the rest
    ///   going to the older buffer, which scrolling back takes before asking the server;
    /// - a channel that stays shown keeps a loaded list of up to [`MAX_MESSAGES_PER_CHANNEL`]
    ///   (see [`MessageWindow::trim_oldest`]).
    ///
    /// Returns the channels whose window lost messages.
    pub(crate) fn set_open(&mut self, slot: usize, channel: Option<ChannelId>) -> Vec<ChannelId> {
        let was_open = self.open.unwrap_or([None, None]);
        let mut open = was_open;
        open[slot] = channel;
        self.open = Some(open);
        let mut trimmed = Vec::new();
        for (ch, w) in self.windows.iter_mut() {
            w.background = !open.contains(&Some(*ch));
            let before = w.messages.len();
            if w.background {
                // A window waiting for its newest page is cut when the page lands.
                if !w.loading {
                    w.trim_oldest();
                    w.clear_buffers();
                }
            } else if !was_open.contains(&Some(*ch)) && w.has_newest {
                w.cut_to(MAX_REOPEN_MESSAGES);
            }
            if w.messages.len() < before {
                trimmed.push(*ch);
            }
        }
        trimmed
    }

    /// Moves up to `limit` messages from the older buffer of `channel` into its window.
    /// Whether there were any (then no request for an older page is needed).
    pub(crate) fn drain_older(&mut self, channel: ChannelId, limit: usize) -> bool {
        self.windows
            .get_mut(&channel)
            .is_some_and(|w| !w.loading && w.drain_older(limit))
    }

    /// The same from the newer buffer.
    pub(crate) fn drain_newer(&mut self, channel: ChannelId, limit: usize) -> bool {
        self.windows
            .get_mut(&channel)
            .is_some_and(|w| !w.loading && w.drain_newer(limit))
    }

    /// A channel's window.
    pub fn get(&self, channel: ChannelId) -> Option<&MessageWindow> {
        self.windows.get(&channel)
    }

    /// Channels with a window, in ID order.
    pub fn channels(&self) -> impl Iterator<Item = ChannelId> + '_ {
        self.windows.iter().map(|(k, _)| *k)
    }

    /// Number of windows.
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// Whether there are no windows.
    pub fn is_empty(&self) -> bool {
        self.windows.len() == 0
    }

    /// Marks the channel as just used, so it is evicted last.
    pub(crate) fn touch(&mut self, channel: ChannelId) {
        self.windows.touch(&channel);
    }

    /// Marks the channel's window as loading (creating it if needed) before
    /// the newest page is requested; it never refuses. The least recently
    /// used idle window makes room; when every window is loading, the least
    /// recently used loading window goes. Returns a channel whose window was
    /// evicted.
    pub(crate) fn begin_load(&mut self, channel: ChannelId, now_ms: i64) -> Option<ChannelId> {
        if let Some(w) = self.windows.get_mut(&channel) {
            w.loading = true;
            w.loading_since_ms = now_ms;
            self.windows.touch(&channel);
            return None;
        }
        let mut evicted = None;
        if self.windows.len() >= MAX_MESSAGE_WINDOWS {
            if let Some(idle) = self.windows.oldest_where(|w| !w.loading) {
                self.windows.remove(&idle);
                evicted = Some(idle);
            }
        }
        let w = MessageWindow {
            loading: true,
            loading_since_ms: now_ms,
            background: self.is_background(channel),
            ..Default::default()
        };
        // Full of loading windows: the insert evicts the least recently used.
        evicted.or(self.windows.insert(channel, w))
    }

    /// Ends loading states older than [`LOAD_TIMEOUT_MS`]; returns their
    /// channels. Their pages may still land later.
    pub(crate) fn expire_loading(&mut self, now_ms: i64) -> Vec<ChannelId> {
        let mut out = Vec::new();
        for (ch, w) in self.windows.iter_mut() {
            if w.loading && now_ms.saturating_sub(w.loading_since_ms) >= LOAD_TIMEOUT_MS {
                w.loading = false;
                out.push(*ch);
            }
        }
        out
    }

    /// The request for the newest page failed: the window may be evicted again.
    pub(crate) fn cancel_load(&mut self, channel: ChannelId) {
        if let Some(w) = self.windows.get_mut(&channel) {
            w.loading = false;
        }
    }

    /// Merges a REST page. Returns whether the window changed, and a channel
    /// whose window was evicted to make room.
    pub(crate) fn apply_page(
        &mut self,
        channel: ChannelId,
        page: &MessagesPage,
        now_ms: i64,
    ) -> (bool, Option<ChannelId>) {
        let mut map = page_map(channel, &page.messages);
        let reached_start = page.reached_start();
        let mut evicted = None;
        if page.before.is_none() {
            evicted = self.begin_load(channel, now_ms);
        } else {
            self.windows.touch(&channel);
        }
        let Some(w) = self.windows.get_mut(&channel) else {
            return (false, evicted);
        };
        map.retain(|id, _| !w.was_deleted(*id));
        let before = w.clone();
        let fits = match page.before {
            None => {
                w.merge_latest(map, reached_start);
                true
            }
            Some(b) => w.merge_older(b, map, reached_start),
        };
        (fits && *w != before, evicted)
    }

    /// Replaces the window with a page around a message (see
    /// [`MessageWindow::replace_around`]), creating it if needed. A loading
    /// window is left alone: the newest page on its way would replace the
    /// jump. Returns whether the window changed, and a channel whose window
    /// was evicted to make room.
    pub(crate) fn apply_around(
        &mut self,
        channel: ChannelId,
        messages: &[Message],
        newest: Option<MessageId>,
    ) -> (bool, Option<ChannelId>) {
        let mut map = page_map(channel, messages);
        if map.is_empty() || self.windows.get(&channel).is_some_and(|w| w.loading) {
            return (false, None);
        }
        let mut evicted = None;
        if self.windows.get(&channel).is_none() {
            if self.windows.len() >= MAX_MESSAGE_WINDOWS {
                if let Some(idle) = self.windows.oldest_where(|w| !w.loading) {
                    self.windows.remove(&idle);
                    evicted = Some(idle);
                }
            }
            evicted = evicted.or(self.windows.insert(
                channel,
                MessageWindow {
                    background: self.is_background(channel),
                    ..Default::default()
                },
            ));
        }
        self.windows.touch(&channel);
        let Some(w) = self.windows.get_mut(&channel) else {
            return (false, evicted);
        };
        map.retain(|id, _| !w.was_deleted(*id));
        let reaches_newest = match (newest, map.keys().next_back()) {
            (Some(n), Some(last)) => *last >= n,
            _ => false,
        };
        let before = w.clone();
        w.replace_around(map, reaches_newest);
        (*w != before, evicted)
    }

    pub(crate) fn insert_live(&mut self, channel: ChannelId, id: MessageId, m: &Message) -> bool {
        self.windows
            .get_mut(&channel)
            .is_some_and(|w| w.insert_live(id, m))
    }

    pub(crate) fn update(&mut self, channel: ChannelId, id: MessageId, m: &Message) -> bool {
        self.windows
            .get_mut(&channel)
            .is_some_and(|w| w.update(id, m))
    }

    pub(crate) fn delete(&mut self, channel: ChannelId, id: MessageId) -> bool {
        self.windows.get_mut(&channel).is_some_and(|w| w.delete(id))
    }

    fn react(&mut self, channel: ChannelId, id: MessageId, op: ReactionOp) -> bool {
        self.windows
            .get_mut(&channel)
            .is_some_and(|w| w.react(id, op))
    }

    /// Adds one reaction. `me` says whether the account reacted.
    pub(crate) fn add_reaction(
        &mut self,
        channel: ChannelId,
        id: MessageId,
        emoji: &Emoji,
        me: bool,
    ) -> bool {
        let op = ReactionOp::Add {
            emoji: emoji.clone(),
            me,
        };
        self.react(channel, id, op)
    }

    /// Removes one reaction.
    pub(crate) fn remove_reaction(
        &mut self,
        channel: ChannelId,
        id: MessageId,
        emoji: &Emoji,
        me: bool,
    ) -> bool {
        let op = ReactionOp::Remove {
            emoji: emoji.clone(),
            me,
        };
        self.react(channel, id, op)
    }

    /// Removes every reaction, or every reaction with one emoji.
    pub(crate) fn clear_reactions(
        &mut self,
        channel: ChannelId,
        id: MessageId,
        emoji: Option<&Emoji>,
    ) -> bool {
        self.react(channel, id, ReactionOp::Clear(emoji.cloned()))
    }

    pub(crate) fn remove(&mut self, channel: ChannelId) -> bool {
        self.windows.remove(&channel).is_some()
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(ChannelId) -> bool) {
        self.windows.retain(|k, _| keep(*k));
    }

    /// After a new READY the windows may have missed messages: keep them,
    /// but buffer live messages until the newest page is loaded again.
    /// Requests from before the READY no longer count as loading.
    pub(crate) fn mark_stale(&mut self) {
        for w in self.windows.values_mut() {
            w.has_newest = false;
            w.stale = true;
            w.loading = false;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.windows.clear();
    }
}
