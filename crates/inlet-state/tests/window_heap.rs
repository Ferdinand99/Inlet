//! The byte budget of a message window is the heap it really takes: windows are filled
//! with several kinds of messages as live events, and the heap the program holds (a counting
//! allocator) is compared with `approx_bytes`, then a window of huge messages is filled to
//! its byte cap and its heap must stay within the cap plus a margin.
//!
//! One test in this file: the allocator counts every thread of the test binary.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use inlet_protocol::{Dispatch, Message, Ready};
use inlet_state::limits::{MAX_MESSAGES_PER_CHANNEL, MAX_WINDOW_BYTES};
use inlet_state::{AccountState, ChannelId, MessagesPage};
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

const CHANNEL: u64 = 5_000;
const GUILD: u64 = 100;

fn ready() -> Ready {
    serde_json::from_value(json!({
        "session_id": "heap",
        "user": {"id": "1", "username": "me"},
        "guilds": [{
            "id": GUILD.to_string(),
            "properties": {"id": GUILD.to_string(), "name": "Guild", "owner_id": "2"},
            "channels": [{"id": CHANNEL.to_string(), "type": 0, "guild_id": GUILD.to_string(),
                          "name": "general", "position": 0}],
            "roles": [{"id": GUILD.to_string(), "name": "@everyone",
                       "permissions": "20266198323167231", "position": 0}],
            "members": [{"user": {"id": "1", "username": "me"}, "roles": [],
                         "joined_at": "2026-01-01T00:00:00+00:00"}],
            "member_count": 1, "joined_at": "2026-01-01T00:00:00+00:00"
        }],
        "private_channels": [], "users": [], "read_states": [], "user_guild_settings": [],
        "presences": [], "relationships": [],
    }))
    .expect("a valid READY")
}

fn user(n: u64) -> Value {
    json!({
        "id": (1_000_000 + n % 60).to_string(), "username": format!("user{}", n % 60),
        "global_name": format!("Person {}", n % 60), "discriminator": "0",
        "avatar": format!("{:032x}", n % 60), "flags": 0, "bot": false,
    })
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Plain,
    Attachments,
    Embed,
    Mentions,
    Reply,
    LongText,
    Huge,
}

/// A message as the API sends it, made from its JSON (so its strings and maps have the
/// capacities a parsed message has).
fn message(n: u64, kind: Kind) -> Message {
    let ms = 1_794_355_200_000u64 + n * 1000;
    let id = ((ms - 1_420_070_400_000) << 22) | (n & 0xFFF);
    let content = match kind {
        Kind::LongText => "A sentence of some length, repeated. ".repeat(80),
        Kind::Huge => "word ".repeat(12_000),
        _ => "A message about as long as most of them are, with a few words more.".to_string(),
    };
    let mut v = json!({
        "id": id.to_string(), "channel_id": CHANNEL.to_string(), "guild_id": GUILD.to_string(),
        "author": user(n), "type": 0, "content": content,
        "timestamp": "2026-11-11T00:00:00+00:00",
        "edited_timestamp": null, "pinned": false, "mention_everyone": false, "tts": false,
        "flags": 0,
        "member": {"roles": ["300", "301"], "joined_at": "2026-01-02T00:00:00+00:00", "nick": null},
        "mentions": [], "mention_roles": [], "attachments": [], "embeds": [], "reactions": [],
    });
    match kind {
        Kind::Attachments => {
            v["attachments"] = (0..2)
                .map(|k| {
                    json!({
                        "id": (9_000_000_000 + n * 10 + k).to_string(), "filename": "beach.png",
                        "content_type": "image/png", "size": 400_000, "width": 800, "height": 450,
                        "url": format!("https://media.example/attachments/{CHANNEL}/{n}/{k}/beach.png?ex=1&is=2&hm=3"),
                        "proxy_url": format!("https://media.example/attachments/{CHANNEL}/{n}/{k}/beach.png?ex=1&is=2&hm=3"),
                    })
                })
                .collect();
        }
        Kind::Embed => {
            v["embeds"] = json!([{
                "type": "rich", "title": "A title", "color": 5793266,
                "description": "A description of about the length a link preview has, give or take.",
                "url": "https://example.com/page",
                "fields": [{"name": "One", "value": "Two", "inline": true},
                           {"name": "Three", "value": "Four", "inline": false}],
                "footer": {"text": "A footer"}, "thumbnail": {"url": "https://example.com/t.png"},
            }]);
        }
        Kind::Mentions => {
            v["mentions"] = (1..=5).map(|k| user(n + k)).collect();
        }
        Kind::Reply => {
            v["type"] = json!(19);
            v["message_reference"] =
                json!({"message_id": "123", "channel_id": CHANNEL.to_string()});
            v["referenced_message"] =
                serde_json::to_value(message(n.saturating_sub(1), Kind::Plain))
                    .expect("a message is JSON");
        }
        _ => {}
    }
    serde_json::from_value(v).expect("a valid message")
}

/// Fills a window with live messages of `kind` until it stops growing (its count cap or
/// its byte cap); returns (messages, approx bytes, heap bytes the window holds).
fn fill(kind: Kind) -> (usize, usize, isize) {
    let mut state = AccountState::from_ready_at(&ready(), 0);
    let ch = ChannelId(CHANNEL);
    state.begin_load(ch);
    state.apply_messages(ch, MessagesPage::latest(Vec::new(), 50));
    let before = live();
    // Past the caps: the count cap holds 500 messages, the byte cap far fewer huge ones.
    for n in 0..(MAX_MESSAGES_PER_CHANNEL as u64 + 100) {
        state.apply(&Dispatch::MessageCreate(Box::new(message(
            1_000_000 + n,
            kind,
        ))));
    }
    let heap = live() - before;
    let w = state.messages(ch).expect("a window");
    (w.len(), w.approx_bytes(), heap)
}

#[test]
fn the_byte_budget_of_a_window_is_the_heap_it_takes() {
    for kind in [
        Kind::Plain,
        Kind::Attachments,
        Kind::Embed,
        Kind::Mentions,
        Kind::Reply,
        Kind::LongText,
    ] {
        let (n, approx, heap) = fill(kind);
        let ratio = heap as f64 / approx as f64;
        println!(
            "{kind:?}: {n} messages, budget {approx} B, heap {heap} B, heap/budget {ratio:.2}"
        );
        assert!(
            n == MAX_MESSAGES_PER_CHANNEL || approx > MAX_WINDOW_BYTES / 10 * 9,
            "{kind:?}: neither cap limited the window ({n} messages, {approx} bytes)"
        );
        assert!(
            (0.8..=1.2).contains(&ratio),
            "{kind:?}: the budget counts {approx} bytes, the heap is {heap} ({ratio:.2} times)"
        );
    }

    // Huge messages: the byte cap limits the window, and the heap stays within the cap plus
    // a margin (the window's own tree and bookkeeping, and the estimate's error).
    let (n, approx, heap) = fill(Kind::Huge);
    println!("Huge: {n} messages, budget {approx} B, heap {heap} B, cap {MAX_WINDOW_BYTES} B");
    assert!(
        n > 1 && n < MAX_MESSAGES_PER_CHANNEL,
        "the byte cap limits it: {n}"
    );
    assert!(approx <= MAX_WINDOW_BYTES);
    let margin = MAX_WINDOW_BYTES / 5 + 64 * 1024;
    assert!(
        heap <= (MAX_WINDOW_BYTES + margin) as isize,
        "{heap} bytes of heap for a window capped at {MAX_WINDOW_BYTES}"
    );
}
