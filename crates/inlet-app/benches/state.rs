// SPDX-License-Identifier: AGPL-3.0-or-later
//! Micro-benchmarks of what every gateway event costs on the UI thread: building an
//! account state from a READY, applying live messages (with and without a window), and
//! merging a page of history. They use the public API of `inlet-state` and invented
//! data, and no benchmark library: each case runs its closure many times and prints the
//! median time of a batch.
//!
//!     scripts/docker-build.sh bench -p inlet --bench state --locked
//!
//! Run without `--bench` (as `cargo test --all-targets` does) each case runs once, to
//! check that it still works.

use std::hint::black_box;
use std::time::{Duration, Instant};

use inlet_protocol::{Dispatch, Message, Ready};
use inlet_state::{AccountState, ChannelId, MessagesPage};
use serde_json::{json, Value};

const GUILDS: u64 = 3;
const CHANNELS: u64 = 10;
const MEMBERS: u64 = 60;
const ME: u64 = 1;

fn ready() -> Ready {
    let user = |n: u64| json!({"id": (1_000_000 + n).to_string(), "username": format!("user{n}")});
    let guilds: Vec<Value> = (0..GUILDS)
        .map(|g| {
            let gid = 100 + g;
            let channels: Vec<Value> = (0..CHANNELS)
                .map(|c| {
                    json!({"id": channel(g, c).to_string(), "type": 0, "guild_id": gid.to_string(),
                           "name": format!("channel-{c}"), "position": c})
                })
                .collect();
            let mut members = vec![json!({"user": {"id": ME.to_string(), "username": "me"},
                                          "roles": [], "joined_at": "2026-01-01T00:00:00+00:00"})];
            members.extend((0..MEMBERS).map(|m| {
                json!({"user": user(m), "roles": [], "joined_at": "2026-01-01T00:00:00+00:00"})
            }));
            json!({"id": gid.to_string(),
                   "properties": {"id": gid.to_string(), "name": format!("Guild {g}"), "owner_id": "2"},
                   "channels": channels,
                   "roles": [{"id": gid.to_string(), "name": "@everyone",
                              "permissions": "20266198323167231", "position": 0}],
                   "members": members, "member_count": 800,
                   "joined_at": "2026-01-01T00:00:00+00:00"})
        })
        .collect();
    serde_json::from_value(json!({
        "session_id": "bench",
        "user": {"id": ME.to_string(), "username": "me"},
        "guilds": guilds, "private_channels": [], "users": [],
        "read_states": [], "user_guild_settings": [], "presences": [], "relationships": [],
    }))
    .expect("a valid READY")
}

fn channel(g: u64, c: u64) -> u64 {
    10_000 + g * 100 + c
}

/// A message of channel `ch`, `n` seconds into the day (IDs are made from the time).
fn message(ch: u64, guild: u64, n: u64) -> Message {
    let ms = 1_794_355_200_000u64 + n * 1000;
    let id = ((ms - 1_420_070_400_000) << 22) | (ch & 0xFFF);
    serde_json::from_value(json!({
        "id": id.to_string(), "channel_id": ch.to_string(), "guild_id": guild.to_string(),
        "author": {"id": (1_000_000 + n % 60).to_string(), "username": format!("user{}", n % 60)},
        "type": 0,
        "content": "A message about as long as most of them are, with a few words more.",
        "timestamp": "2026-11-11T00:00:00+00:00",
    }))
    .expect("a valid message")
}

/// Runs `f` in batches, prints the median per call.
fn case(name: &str, batch: u32, bench: bool, mut f: impl FnMut()) {
    if !bench {
        f();
        println!("{name}: ok");
        return;
    }
    for _ in 0..batch {
        f();
    }
    let mut times: Vec<Duration> = (0..15)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..batch {
                f();
            }
            t.elapsed() / batch
        })
        .collect();
    times.sort();
    let median = times[times.len() / 2];
    println!(
        "{name:<48} {:>10.2} us/call (best {:.2}, worst {:.2})",
        median.as_secs_f64() * 1e6,
        times[0].as_secs_f64() * 1e6,
        times[times.len() - 1].as_secs_f64() * 1e6
    );
}

fn main() {
    let bench = std::env::args().any(|a| a == "--bench");
    let ready = ready();

    case(
        "ready: 3 guilds, 30 channels, 183 members",
        20,
        bench,
        || {
            black_box(AccountState::from_ready_at(black_box(&ready), 0));
        },
    );

    // A window with the newest 50 messages, which the live messages then fill to its cap.
    let (g, c) = (0, 0);
    let ch = ChannelId(channel(g, c));
    let open = |state: &mut AccountState| {
        state.begin_load(ch);
        let page: Vec<Message> = (0..50)
            .map(|n| message(ch.0, 100 + g, 1_000_000 + n))
            .collect();
        state.apply_messages(ch, MessagesPage::latest(page, 50));
    };

    let mut state = AccountState::from_ready_at(&ready, 0);
    open(&mut state);
    let mut n = 2_000_000;
    case(
        "live message into an open window (at the cap)",
        500,
        bench,
        || {
            n += 1;
            let m = message(ch.0, 100 + g, n);
            black_box(state.apply(&Dispatch::MessageCreate(Box::new(m))));
        },
    );

    let mut state = AccountState::from_ready_at(&ready, 0);
    let mut n = 2_000_000;
    case("live message, channel has no window", 500, bench, || {
        n += 1;
        let m = message(ch.0, 100 + g, n);
        black_box(state.apply(&Dispatch::MessageCreate(Box::new(m))));
    });

    let mut state = AccountState::from_ready_at(&ready, 0);
    state.set_passive(true);
    let mut n = 2_000_000;
    case(
        "live message into a passive (background) state",
        500,
        bench,
        || {
            n += 1;
            let m = message(ch.0, 100 + g, n);
            black_box(state.apply(&Dispatch::MessageCreate(Box::new(m))));
        },
    );

    let mut state = AccountState::from_ready_at(&ready, 0);
    open(&mut state);
    let mut next_old = 999_999u64;
    case(
        "page of 50 older messages merged into a window",
        50,
        bench,
        || {
            // Keeps asking for older ones; the window stays at its cap.
            let Some(oldest) = state.messages(ch).and_then(|w| w.oldest_id()) else {
                return;
            };
            let page: Vec<Message> = (0..50)
                .map(|k| message(ch.0, 100 + g, next_old - k))
                .collect();
            next_old = next_old.saturating_sub(50);
            black_box(state.apply_messages(ch, MessagesPage::before(oldest, page, 50)));
        },
    );
}
