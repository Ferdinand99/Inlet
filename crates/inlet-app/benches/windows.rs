// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a message costs in a window, in heap bytes counted by an allocator that wraps the
//! system's: for a plain message, one with an attachment and one with an embed, next to
//! the `approx_bytes` the account state budgets with. A full set of windows (30 channels
//! of 500 messages) is the largest part of the memory the app holds, so this is the
//! number that says whether the budget is honest.
//!
//!     INLET_PASS_ENV=CARGO_PROFILE_CI_OPT_LEVEL CARGO_PROFILE_CI_OPT_LEVEL=3 \
//!         scripts/docker-build.sh bench --profile ci -p inlet --bench windows --locked
//!
//! (The ci profile, because the dev-dependencies cannot go in a release-like build.)
//!
//! Run without `--bench` each case runs once with small numbers.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use inlet_protocol::{Dispatch, Message};
use inlet_state::{limits, AccountState, ChannelId, MessagesPage};
use serde_json::{json, Value};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(
            new_size as isize - layout.size() as isize,
            Ordering::Relaxed,
        );
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live() -> isize {
    LIVE.load(Ordering::Relaxed)
}

const ME: u64 = 1;
const CHANNEL: u64 = 5_000;
const GUILD: u64 = 100;

fn ready() -> inlet_protocol::Ready {
    serde_json::from_value(json!({
        "session_id": "bench",
        "user": {"id": ME.to_string(), "username": "me"},
        "guilds": [{
            "id": GUILD.to_string(),
            "properties": {"id": GUILD.to_string(), "name": "Guild", "owner_id": "2"},
            "channels": [{"id": CHANNEL.to_string(), "type": 0, "guild_id": GUILD.to_string(),
                          "name": "general", "position": 0}],
            "roles": [{"id": GUILD.to_string(), "name": "@everyone",
                       "permissions": "20266198323167231", "position": 0}],
            "members": [{"user": {"id": ME.to_string(), "username": "me"}, "roles": [],
                         "joined_at": "2026-01-01T00:00:00+00:00"}],
            "member_count": 1, "joined_at": "2026-01-01T00:00:00+00:00"
        }],
        "private_channels": [], "users": [], "read_states": [], "user_guild_settings": [],
        "presences": [], "relationships": [],
    }))
    .expect("a valid READY")
}

#[derive(Clone, Copy)]
enum Kind {
    Plain,
    Attachment,
    Embed,
}

/// A message as the API sends it: the author with the fields Fluxer sends, a member
/// with roles, a few mentions.
fn message(n: u64, kind: Kind) -> Message {
    let ms = 1_794_355_200_000u64 + n * 1000;
    let id = ((ms - 1_420_070_400_000) << 22) | (n & 0xFFF);
    let mut v: Value = json!({
        "id": id.to_string(), "channel_id": CHANNEL.to_string(), "guild_id": GUILD.to_string(),
        "author": {
            "id": (1_000_000 + n % 60).to_string(), "username": format!("user{}", n % 60),
            "global_name": format!("Person {}", n % 60), "discriminator": "0",
            "avatar": format!("{:032x}", n % 60), "flags": 0, "bot": false,
        },
        "type": 0,
        "content": "A message about as long as most of them are, with a few words more.",
        "timestamp": "2026-11-11T00:00:00+00:00",
        "edited_timestamp": null, "pinned": false, "mention_everyone": false, "tts": false,
        "flags": 0,
        "member": {"roles": ["300", "301"], "joined_at": "2026-01-02T00:00:00+00:00", "nick": null},
        "mentions": [], "mention_roles": [], "attachments": [], "embeds": [], "reactions": [],
    });
    match kind {
        Kind::Plain => {}
        Kind::Attachment => {
            v["attachments"] = json!([{
                "id": (9_000_000_000 + n).to_string(), "filename": "beach.png",
                "content_type": "image/png", "size": 400_000, "width": 800, "height": 450,
                "url": format!("https://media.example/attachments/{CHANNEL}/{n}/beach.png?ex=1&is=2&hm=3"),
                "proxy_url": format!("https://media.example/attachments/{CHANNEL}/{n}/beach.png?ex=1&is=2&hm=3"),
            }]);
        }
        Kind::Embed => {
            v["embeds"] = json!([{
                "type": "rich", "title": "A title", "color": 5793266,
                "description": "A description of about the length a link preview has, give or take.",
                "url": "https://example.com/page",
                "fields": [{"name": "One", "value": "Two", "inline": true}],
                "footer": {"text": "A footer"},
            }]);
        }
    }
    serde_json::from_value(v).expect("a valid message")
}

fn main() {
    let bench = std::env::args().any(|a| a == "--bench");
    let full = if bench {
        limits::MAX_MESSAGES_PER_CHANNEL
    } else {
        50
    };
    println!(
        "message window of {full} messages (cap {} messages, {} bytes)",
        limits::MAX_MESSAGES_PER_CHANNEL,
        limits::MAX_WINDOW_BYTES
    );
    println!(
        "{:<14} {:>14} {:>14} {:>8}",
        "kind", "heap B/msg", "budget B/msg", "ratio"
    );
    for (name, kind) in [
        ("plain", Kind::Plain),
        ("attachment", Kind::Attachment),
        ("embed", Kind::Embed),
    ] {
        let mut state = AccountState::from_ready_at(&ready(), 0);
        let ch = ChannelId(CHANNEL);
        let before = live();
        state.begin_load(ch);
        // The newest page first, then the pages before it, as scrolling back asks for them.
        // Each page is made from its JSON and moved in, as a page off the wire is.
        let pages = full.div_ceil(50) as u64;
        for (i, p) in (0..pages).rev().enumerate() {
            let page: Vec<Message> = (p * 50..(p * 50 + 50).min(full as u64))
                .map(|n| message(1_000_000 + n, kind))
                .collect();
            let oldest = state.messages(ch).and_then(|w| w.oldest_id());
            let page = match (i, oldest) {
                (0, _) | (_, None) => MessagesPage::latest(page, 50),
                (_, Some(oldest)) => MessagesPage::before(oldest, page, 50),
            };
            state.apply_messages(ch, page);
        }
        // What the pages left behind is the window's.
        let heap = live() - before;
        let win = state.messages(ch).expect("a window");
        let n = win.len().max(1) as f64;
        let budget = win.approx_bytes() as f64 / n;
        println!(
            "{name:<14} {:>14.0} {:>14.0} {:>7.2}x   ({} messages kept)",
            heap as f64 / n,
            budget,
            heap as f64 / n / budget,
            win.len()
        );
        drop(state);
    }

    // And the live path: messages arriving one by one into an open window.
    let mut state = AccountState::from_ready_at(&ready(), 0);
    let ch = ChannelId(CHANNEL);
    state.begin_load(ch);
    state.apply_messages(ch, MessagesPage::latest(Vec::new(), 50));
    let before = live();
    for n in 0..full as u64 {
        state.apply(&Dispatch::MessageCreate(Box::new(message(
            2_000_000 + n,
            Kind::Plain,
        ))));
    }
    let heap = live() - before;
    let win = state.messages(ch).expect("a window");
    println!(
        "live, plain   {:>14.0} B/msg over {} messages (30 full windows would hold {:.0} MiB)",
        heap as f64 / win.len().max(1) as f64,
        win.len(),
        heap as f64 / win.len().max(1) as f64 * 500. * 30. / 1048576.
    );
}
