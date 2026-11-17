//! The heap of 30 windows when only one channel is on screen: the others are cut to their
//! newest messages, so the set weighs the open window plus 29 small ones, not 30 full ones.
//! The same 30 channels are filled without the app saying which one is open for comparison.
//!
//! One test in this file: the allocator counts every thread of the test binary.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use inlet_protocol::{Dispatch, Message, Ready};
use inlet_state::limits::{MAX_BACKGROUND_MESSAGES, MAX_MESSAGES_PER_CHANNEL, MAX_MESSAGE_WINDOWS};
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

const GUILD: u64 = 100;
const FIRST: u64 = 5_000;

fn ready() -> Ready {
    let channels: Vec<Value> = (0..MAX_MESSAGE_WINDOWS as u64)
        .map(|c| {
            json!({"id": (FIRST + c).to_string(), "type": 0, "guild_id": GUILD.to_string(),
                   "name": format!("channel-{c}"), "position": c})
        })
        .collect();
    serde_json::from_value(json!({
        "session_id": "trim",
        "user": {"id": "1", "username": "me"},
        "guilds": [{
            "id": GUILD.to_string(),
            "properties": {"id": GUILD.to_string(), "name": "Guild", "owner_id": "2"},
            "channels": channels,
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

fn message(channel: u64, n: u64) -> Message {
    let ms = 1_794_355_200_000u64 + n * 1000;
    let id = ((ms - 1_420_070_400_000) << 22) | (channel & 0xFFF);
    serde_json::from_value(json!({
        "id": id.to_string(), "channel_id": channel.to_string(), "guild_id": GUILD.to_string(),
        "author": {"id": (1_000_000 + n % 60).to_string(), "username": format!("user{}", n % 60),
                   "global_name": format!("Person {}", n % 60), "discriminator": "0",
                   "avatar": format!("{:032x}", n % 60), "flags": 0, "bot": false},
        "type": 0,
        "content": "A message about as long as most of them are, with a few words more.",
        "timestamp": "2026-11-11T00:00:00+00:00", "edited_timestamp": null, "pinned": false,
        "mention_everyone": false, "tts": false, "flags": 0,
        "member": {"roles": ["300", "301"], "joined_at": "2026-01-02T00:00:00+00:00", "nick": null},
        "mentions": [], "mention_roles": [], "attachments": [], "embeds": [], "reactions": [],
    }))
    .expect("a valid message")
}

/// 30 channels, each opened in turn and filled with live messages past the cap; with
/// `track` the app says which one is on screen. Returns the heap of the state, the total
/// of the windows' budgets, and the number of messages in each window.
fn run(track: bool) -> (isize, usize, Vec<usize>) {
    let mut s = AccountState::from_ready_at(&ready(), 0);
    let before = live();
    for c in 0..MAX_MESSAGE_WINDOWS as u64 {
        let ch = ChannelId(FIRST + c);
        if track {
            s.set_open_channel(Some(ch));
        }
        s.begin_load(ch);
        s.apply_messages(ch, MessagesPage::latest(Vec::new(), 50));
        for n in 0..(MAX_MESSAGES_PER_CHANNEL as u64 + 100) {
            s.apply(&Dispatch::MessageCreate(Box::new(message(
                FIRST + c,
                1_000_000 + n,
            ))));
        }
    }
    let heap = live() - before;
    let w = s.message_windows();
    let budget: usize = w.channels().map(|c| w.get(c).unwrap().approx_bytes()).sum();
    let lens: Vec<usize> = w.channels().map(|c| w.get(c).unwrap().len()).collect();
    (heap, budget, lens)
}

#[test]
fn thirty_windows_weigh_one_open_window_and_twenty_nine_small_ones() {
    let (full, full_budget, lens) = run(false);
    assert!(lens.iter().all(|n| *n == MAX_MESSAGES_PER_CHANNEL));
    let (cut, cut_budget, lens) = run(true);
    let open = lens
        .iter()
        .filter(|n| **n == MAX_MESSAGES_PER_CHANNEL)
        .count();
    let small = lens
        .iter()
        .filter(|n| **n == MAX_BACKGROUND_MESSAGES)
        .count();
    assert_eq!((open, small), (1, MAX_MESSAGE_WINDOWS - 1), "{lens:?}");
    println!(
        "30 full windows: {} KiB of heap (budget {} KiB); one open and 29 cut: {} KiB (budget {} KiB)",
        full / 1024,
        full_budget / 1024,
        cut / 1024,
        cut_budget / 1024
    );
    // The bound: the budget of the cut set, which tracks the heap to within 20 %, plus a
    // little for the state around the windows.
    assert!(
        cut as usize <= cut_budget * 12 / 10 + 256 * 1024,
        "{cut} bytes of heap for windows budgeted at {cut_budget}"
    );
    // And it is the saving that was meant: about (500 + 29 x 100) / (30 x 500) of the full set.
    let expected = full as usize * (MAX_MESSAGES_PER_CHANNEL + 29 * MAX_BACKGROUND_MESSAGES)
        / (30 * MAX_MESSAGES_PER_CHANNEL);
    assert!(
        (cut as usize) < expected * 12 / 10,
        "{cut} bytes against an expected {expected}"
    );
}
