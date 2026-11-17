// SPDX-License-Identifier: AGPL-3.0-or-later
//! The synthetic gateway recording the soak replays: three accounts' worth of gateway
//! events over a stretch of (virtual) time, made from a seed and nothing else. Every
//! name, message and ID is invented. Nothing real is in it and no token is needed.
//!
//! The recording is a stream of [`Frame`]s, made a minute at a time as the replay asks
//! for them, so the replay itself never holds more than a minute of it (a day of it is
//! tens of thousands of events). The same seed gives the same frames; [`write_jsonl`]
//! writes them as JSON lines when a file is wanted.
//!
//! The world is fixed ([`World`]): the first account has 3 guilds with 10 text channels
//! each (30 open channels), one of them with 5,000 messages of history; the two others
//! (background accounts) have 2 guilds and a few DMs each. IDs are plain numbers in
//! their own ranges, message IDs are made from the message's time like real snowflakes.

use std::collections::VecDeque;
use std::io::Write;

use serde_json::{json, Value};

use super::super::accounts;
use super::super::media::attachment;
use super::super::{message_id, rfc3339, DEMO_DAY_MS};

/// One virtual day in milliseconds.
pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Messages in the biggest channel's history (the scrolling test).
pub const BIG_HISTORY: usize = 5_000;
/// Distance between two history messages.
const HISTORY_STEP_MS: i64 = 90_000;
/// People that can write in an account's guilds. Small enough that the caches they fill
/// (users, members, names) are full within a few virtual hours, so a long run shows growth
/// that is not a cache filling up.
pub const POOL: u64 = 400;
/// Members a guild's READY carries (the rest of `member_count` is not sent).
const READY_MEMBERS: u64 = 60;

/// The recording starts the day after the demo's "today", so history lies in the past.
pub const START_MS: i64 = DEMO_DAY_MS + DAY_MS as i64;

/// One gateway event of the recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Milliseconds after the start of the recording.
    pub at_ms: u64,
    /// Which account's gateway it arrives on (index into [`World::accounts`]).
    pub account: usize,
    /// The event name (`MESSAGE_CREATE`).
    pub name: &'static str,
    /// The payload.
    pub data: Value,
}

/// A channel of the fake world.
#[derive(Clone, Debug)]
pub struct ChannelSpec {
    pub id: String,
    pub name: String,
    /// Messages of history before the recording starts.
    pub history: usize,
    /// Makes this channel's message IDs differ from every other channel's.
    pub ordinal: u64,
}

#[derive(Clone, Debug)]
pub struct GuildSpec {
    pub id: String,
    pub name: String,
    pub channels: Vec<ChannelSpec>,
}

#[derive(Clone, Debug)]
pub struct AccountSpec {
    pub index: usize,
    /// The account's own user ID.
    pub me: String,
    pub name: String,
    pub guilds: Vec<GuildSpec>,
    pub dms: Vec<ChannelSpec>,
}

/// Everything the recording talks about.
#[derive(Clone, Debug)]
pub struct World {
    pub accounts: Vec<AccountSpec>,
}

impl World {
    pub fn build() -> World {
        let keys = accounts::list();
        let mut out = Vec::new();
        for (a, info) in keys.iter().enumerate() {
            let (guilds, channels, history, dms) = if a == 0 {
                (3, 10, 200, 12)
            } else {
                (2, 6, 100, 8)
            };
            let mut ordinal = 1u64;
            let mut guild_specs = Vec::new();
            for g in 0..guilds {
                let mut chans = Vec::new();
                for c in 0..channels {
                    let big = a == 0 && g == 0 && c == channels - 1;
                    chans.push(ChannelSpec {
                        id: (2_000_000_000_000 + (a * 1000 + g * 100 + c) as u64).to_string(),
                        name: if big {
                            "archive".into()
                        } else {
                            format!("channel-{g}-{c}")
                        },
                        history: if big { BIG_HISTORY } else { history },
                        ordinal,
                    });
                    ordinal += 1;
                }
                guild_specs.push(GuildSpec {
                    id: (1_000_000_000_000 + (a * 100 + g) as u64).to_string(),
                    name: format!("Guild {a}-{g}"),
                    channels: chans,
                });
            }
            let dm_specs = (0..dms)
                .map(|d| {
                    let spec = ChannelSpec {
                        id: (2_500_000_000_000 + (a * 1000 + d) as u64).to_string(),
                        // The one other person's name, as `channel_json` makes them.
                        name: format!("Person {ordinal}"),
                        history: 40,
                        ordinal,
                    };
                    ordinal += 1;
                    spec
                })
                .collect();
            out.push(AccountSpec {
                index: a,
                me: info.key.user_id.to_string(),
                name: info.name.clone(),
                guilds: guild_specs,
                dms: dm_specs,
            });
        }
        World { accounts: out }
    }

    /// The first account's text channels, guild first, in the order the soak opens them.
    pub fn open_channels(&self) -> Vec<(&GuildSpec, &ChannelSpec)> {
        self.accounts[0]
            .guilds
            .iter()
            .flat_map(|g| g.channels.iter().map(move |c| (g, c)))
            .collect()
    }
}

// ---- Random numbers ---------------------------------------------------------------------

/// xorshift64*: small, fast and the same everywhere.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0..n
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            (self.next() >> 11) % n
        }
    }

    /// True with probability `percent` out of 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

// ---- Pieces -----------------------------------------------------------------------------

const WORDS: &[&str] = &[
    "build", "ship", "the", "release", "tomorrow", "maybe", "sounds", "good", "fine", "thanks",
    "looks", "great", "can", "someone", "review", "this", "merge", "fixed", "again", "broken",
    "works", "now", "lunch", "meeting", "later", "today", "weekend", "plan", "idea", "link",
    "photo", "server", "update", "agree", "disagree", "really", "quick", "question", "answer",
    "docs", "test", "green", "red", "cache", "memory", "window", "scroll", "channel", "message",
    "coffee", "morning", "evening", "night", "okay",
];

/// A short or long line of invented text, sometimes with formatting.
fn sentence(rng: &mut Rng, me: &str) -> String {
    let long = rng.chance(8);
    let words = 3 + rng.below(if long { 90 } else { 14 }) as usize;
    let mut text: Vec<String> = (0..words).map(|_| (*rng.pick(WORDS)).to_string()).collect();
    if let Some(first) = text.first_mut() {
        if let Some(c) = first.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
    }
    let mut line = text.join(" ");
    match rng.below(40) {
        0 => line = format!("**{line}**"),
        1 => line = format!("*{line}* :tada:"),
        2 => {
            line = format!(
                "```rust\nfn main() {{ println!(\"{}\"); }}\n```",
                rng.next() % 1000
            )
        }
        3 => line = format!("{line} https://example.com/page/{}", rng.below(10_000)),
        4 => line = format!("> {line}\n{}", rng.pick(WORDS)),
        5 => line = format!("{line} <@{me}>"),
        6 => line = format!("||{line}||"),
        _ => {}
    }
    line.push(['.', '!', '?', ' '][rng.below(4) as usize]);
    line
}

fn user_json(account: usize, n: u64) -> Value {
    let id = pool_user(account, n);
    json!({
        "id": id,
        "username": format!("user{account}x{n}"),
        "global_name": format!("Person {n}"),
        "avatar": (!n.is_multiple_of(10)).then(|| format!("sk{account}x{n}")),
    })
}

fn me_json(spec: &AccountSpec) -> Value {
    json!({
        "id": spec.me,
        "username": spec.name.to_lowercase().replace(' ', "."),
        "global_name": spec.name,
    })
}

/// The ID of the `n`th invented person of an account.
pub fn pool_user(account: usize, n: u64) -> String {
    (3_000_000_000_000 + account as u64 * 1_000_000 + n).to_string()
}

/// The ID of a message of channel `chan` made at `ms`, with a counter for messages in
/// the same millisecond.
pub fn id_at(chan: &ChannelSpec, ms: i64, counter: u64) -> String {
    message_id(ms, (chan.ordinal << 12) | (counter & 0xFFF))
}

/// A message as the API sends it.
fn message_json(
    rng: &mut Rng,
    spec: &AccountSpec,
    guild: Option<&GuildSpec>,
    chan: &ChannelSpec,
    ms: i64,
    counter: u64,
    extras: bool,
) -> Value {
    let author = rng.below(POOL);
    let content = sentence(rng, &spec.me);
    let mentions_me = content.contains(&format!("<@{}>", spec.me));
    let mut v = json!({
        "id": id_at(chan, ms, counter),
        "channel_id": chan.id,
        "author": user_json(spec.index, author),
        "type": 0,
        "content": content,
        "timestamp": rfc3339(ms),
        "member": {"roles": []},
        "mentions": if mentions_me { vec![me_json(spec)] } else { Vec::new() },
        "mention_roles": [],
    });
    if let Some(g) = guild {
        v["guild_id"] = json!(g.id);
    }
    if extras && rng.chance(12) {
        let (file, w, h) = *rng.pick(&[
            ("beach.png", 800, 450),
            ("screenshot.png", 640, 360),
            ("weekend.png", 1280, 720),
        ]);
        let id = 9_000_000_000 + rng.below(1_000_000_000);
        v["attachments"] = json!([attachment(
            &chan.id,
            id,
            file,
            "image/png",
            json!({"width": w, "height": h, "size": 400_000})
        )]);
    }
    if extras && rng.chance(5) {
        v["embeds"] = json!([{
            "type": "rich",
            "title": format!("Link {}", rng.below(1000)),
            "description": sentence(rng, &spec.me),
            "color": 0x5865F2,
        }]);
    }
    v
}

/// History message `k` (0 = oldest) of a channel, as the API sends it. The same `k`
/// always gives the same message.
pub fn history_message(
    spec: &AccountSpec,
    guild: Option<&GuildSpec>,
    chan: &ChannelSpec,
    k: usize,
) -> Value {
    let mut rng = Rng::new(chan.ordinal.wrapping_mul(1_000_003).wrapping_add(k as u64));
    message_json(&mut rng, spec, guild, chan, history_time(chan, k), 0, true)
}

/// The time of history message `k`, in milliseconds.
pub fn history_time(chan: &ChannelSpec, k: usize) -> i64 {
    START_MS - (chan.history - k) as i64 * HISTORY_STEP_MS
}

/// How many history messages of `chan` are older than `ms`.
pub fn history_before(chan: &ChannelSpec, ms: i64) -> usize {
    if ms >= START_MS {
        return chan.history;
    }
    let back = ((START_MS - ms) / HISTORY_STEP_MS) as usize;
    chan.history.saturating_sub(back)
}

fn channel_json(
    guild: Option<&GuildSpec>,
    chan: &ChannelSpec,
    position: usize,
    newest: Option<String>,
) -> Value {
    let mut v = json!({
        "id": chan.id,
        "type": if guild.is_some() { 0 } else { 1 },
        "position": position,
        "last_message_id": newest,
    });
    if let Some(g) = guild {
        v["guild_id"] = json!(g.id);
        v["name"] = json!(chan.name);
        v["topic"] = json!(format!("The {} channel", chan.name));
    } else {
        v["recipients"] = json!([user_json(0, chan.ordinal)]);
    }
    v
}

fn newest_history_id(chan: &ChannelSpec) -> String {
    id_at(chan, history_time(chan, chan.history.saturating_sub(1)), 0)
}

/// The READY of an account: its guilds with channels, roles, some members and
/// presences, its DMs, and read states (most channels have a few unread messages).
pub fn ready_json(spec: &AccountSpec) -> Value {
    let mut read_states = Vec::new();
    let mut presences = Vec::new();
    let guilds: Vec<Value> = spec
        .guilds
        .iter()
        .map(|g| {
            let channels: Vec<Value> = g
                .channels
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    // Read up to some way before the newest message.
                    let read = id_at(
                        c,
                        history_time(c, c.history.saturating_sub(1 + (i % 4) * 2)),
                        0,
                    );
                    read_states.push(json!({"id": c.id, "last_message_id": read, "mention_count": 0}));
                    channel_json(Some(g), c, i, Some(newest_history_id(c)))
                })
                .collect();
            let mut members = vec![
                json!({"user": me_json(spec), "roles": [], "joined_at": "2026-01-01T00:00:00+00:00"}),
            ];
            for n in 0..READY_MEMBERS {
                members.push(json!({
                    "user": user_json(spec.index, n),
                    "roles": [],
                    "joined_at": "2026-01-02T00:00:00+00:00",
                }));
                if n % 3 == 0 {
                    presences.push(json!({
                        "user": {"id": pool_user(spec.index, n)},
                        "status": "online",
                        "guild_id": g.id,
                    }));
                }
            }
            json!({
                "id": g.id,
                "properties": {"id": g.id, "name": g.name, "owner_id": pool_user(spec.index, 0)},
                "channels": channels,
                "roles": [{"id": g.id, "name": "@everyone", "permissions": "20266198323167231", "position": 0}],
                "members": members,
                "member_count": 800,
                "joined_at": "2026-01-01T00:00:00+00:00",
            })
        })
        .collect();
    let private: Vec<Value> = spec
        .dms
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let read = id_at(c, history_time(c, c.history.saturating_sub(1 + i % 3)), 0);
            read_states.push(json!({"id": c.id, "last_message_id": read, "mention_count": 0}));
            channel_json(None, c, i, Some(newest_history_id(c)))
        })
        .collect();
    json!({
        "session_id": format!("soak-{}", spec.index),
        "user": me_json(spec),
        "guilds": guilds,
        "private_channels": private,
        "users": [],
        "read_states": read_states,
        "user_guild_settings": [],
        "presences": presences,
        "relationships": [],
    })
}

// ---- The stream -------------------------------------------------------------------------

/// What to record.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub seed: u64,
    /// Virtual length of the recording.
    pub duration_ms: u64,
    /// How busy it is, 1 for a lively day. The message windows (up to 200 messages each,
    /// with their buffers) take days to fill at 1; 24 fills them in hours, which shows what a full set weighs.
    pub rate: f64,
}

/// Events per minute at the busiest, for the first account and for the others.
const FOREGROUND_BUSY: u64 = 9;
const BACKGROUND_BUSY: u64 = 5;
/// Virtual time between two fresh READYs (a new Identify after a dropped connection).
const READY_EVERY_MS: u64 = 4 * 60 * 60 * 1000;
/// How active the hours of the day are, in percent of the busy rate.
const DAYLIGHT: [u64; 24] = [
    25, 25, 25, 25, 25, 30, 40, 60, 80, 90, 100, 100, 100, 100, 100, 100, 100, 100, 100, 90, 80,
    60, 40, 30,
];

struct Recent {
    /// The newest messages of one channel: (id, time), newest last.
    ids: VecDeque<(String, i64)>,
    counter: u64,
}

/// The frames of a [`Plan`], in time order.
pub struct Frames {
    plan: Plan,
    world: World,
    rng: Rng,
    minute: u64,
    buffer: VecDeque<Frame>,
    recent: Vec<Vec<Recent>>,
}

pub fn frames(plan: Plan) -> Frames {
    let world = World::build();
    let recent = world
        .accounts
        .iter()
        .map(|a| {
            let n = a.guilds.iter().map(|g| g.channels.len()).sum::<usize>() + a.dms.len();
            (0..n)
                .map(|_| Recent {
                    ids: VecDeque::new(),
                    counter: 0,
                })
                .collect()
        })
        .collect();
    let mut buffer = VecDeque::new();
    // The background accounts' first READY (the first account's comes with the app's
    // start-up data).
    for a in 1..world.accounts.len() {
        buffer.push_back(Frame {
            at_ms: 0,
            account: a,
            name: "READY",
            data: ready_json(&world.accounts[a]),
        });
    }
    Frames {
        plan,
        world,
        rng: Rng::new(plan.seed),
        minute: 0,
        buffer,
        recent,
    }
}

impl Iterator for Frames {
    type Item = Frame;

    fn next(&mut self) -> Option<Frame> {
        loop {
            if let Some(f) = self.buffer.pop_front() {
                return Some(f);
            }
            if self.minute * 60_000 >= self.plan.duration_ms {
                return None;
            }
            self.fill_minute();
            self.minute += 1;
        }
    }
}

impl Frames {
    /// The events of one virtual minute, sorted by time.
    fn fill_minute(&mut self) {
        let minute = self.minute;
        let base = minute * 60_000;
        let mut out: Vec<Frame> = Vec::new();
        let day = DAYLIGHT[((minute / 60) % 24) as usize];
        for a in 0..self.world.accounts.len() {
            let busy = if a == 0 {
                FOREGROUND_BUSY
            } else {
                BACKGROUND_BUSY
            };
            let want = busy as f64 * self.plan.rate * day as f64 / 100.;
            let events =
                want.floor() as u64 + u64::from(self.rng.below(100) < (want.fract() * 100.) as u64);
            // A fresh READY now and then (a dropped connection and a new Identify).
            if minute > 0 && base.is_multiple_of(READY_EVERY_MS) {
                out.push(Frame {
                    at_ms: base,
                    account: a,
                    name: "READY",
                    data: ready_json(&self.world.accounts[a]),
                });
            }
            for _ in 0..events {
                let at = base + 1 + self.rng.below(59_000);
                if let Some(f) = self.event(a, at) {
                    out.push(f);
                }
            }
        }
        out.sort_by_key(|f| f.at_ms);
        self.buffer.extend(out);
    }

    /// One random event of account `a` at `at_ms`, if something fits (an edit needs a
    /// message to edit).
    fn event(&mut self, a: usize, at_ms: u64) -> Option<Frame> {
        let spec = self.world.accounts[a].clone();
        let channels: Vec<(Option<&GuildSpec>, &ChannelSpec)> = spec
            .guilds
            .iter()
            .flat_map(|g| g.channels.iter().map(move |c| (Some(g), c)))
            .chain(spec.dms.iter().map(|c| (None, c)))
            .collect();
        // Some channels are far busier than others: the pick leans to the first ones.
        let n = channels.len() as u64;
        let ix = ((self.rng.below(n) * self.rng.below(n)) / n) as usize;
        let (guild, chan) = channels[ix.min(channels.len() - 1)];
        let ix = ix.min(channels.len() - 1);
        let ms = START_MS + at_ms as i64;
        let guild_id = guild.map(|g| g.id.clone());
        let (name, data) = match self.rng.below(100) {
            0..=51 => {
                let recent = &mut self.recent[a][ix];
                recent.counter += 1;
                let counter = recent.counter;
                let m = message_json(&mut self.rng, &spec, guild, chan, ms, counter, true);
                let id = m["id"].as_str().unwrap_or_default().to_string();
                let recent = &mut self.recent[a][ix];
                recent.ids.push_back((id, ms));
                if recent.ids.len() > 24 {
                    recent.ids.pop_front();
                }
                ("MESSAGE_CREATE", m)
            }
            52..=59 => {
                let (id, t) = self.recent[a][ix].ids.back()?.clone();
                let mut m = message_json(&mut self.rng, &spec, guild, chan, t, 0, false);
                m["id"] = json!(id);
                m["edited_timestamp"] = json!(rfc3339(ms));
                ("MESSAGE_UPDATE", m)
            }
            60..=63 => {
                let (id, _) = self.recent[a][ix].ids.pop_front()?;
                (
                    "MESSAGE_DELETE",
                    json!({"id": id, "channel_id": chan.id, "guild_id": guild_id}),
                )
            }
            64..=77 if a == 0 => (
                "TYPING_START",
                // The replay stamps it with the clock of the moment.
                json!({
                    "channel_id": chan.id,
                    "user_id": pool_user(a, self.rng.below(POOL)),
                    "guild_id": guild_id,
                    "timestamp": 0,
                }),
            ),
            78..=87 => {
                let g = guild?;
                (
                    "PRESENCE_UPDATE",
                    json!({
                        "user": {"id": pool_user(a, self.rng.below(READY_MEMBERS))},
                        "status": *self.rng.pick(&["online", "idle", "dnd", "offline"]),
                        "guild_id": g.id,
                    }),
                )
            }
            88..=93 => {
                let (id, _) = self.recent[a][ix].ids.back()?.clone();
                let add = self.rng.chance(70);
                (
                    if add {
                        "MESSAGE_REACTION_ADD"
                    } else {
                        "MESSAGE_REACTION_REMOVE"
                    },
                    json!({
                        "user_id": pool_user(a, self.rng.below(POOL)),
                        "channel_id": chan.id,
                        "message_id": id,
                        "emoji": {"name": *self.rng.pick(&["\u{1F44D}", "\u{1F389}", "\u{2764}", "\u{1F602}"])},
                        "guild_id": guild_id,
                    }),
                )
            }
            94..=96 => {
                let (id, _) = self.recent[a][ix].ids.back()?.clone();
                (
                    "MESSAGE_ACK",
                    json!({"channel_id": chan.id, "message_id": id, "mention_count": 0}),
                )
            }
            97 => {
                let g = guild?;
                let mut c = channel_json(Some(g), chan, ix, None);
                c["topic"] = json!(format!("Topic number {}", self.rng.below(1000)));
                ("CHANNEL_UPDATE", c)
            }
            _ => {
                let g = guild?;
                let n = self.rng.below(POOL);
                if self.rng.chance(50) {
                    (
                        "GUILD_MEMBER_ADD",
                        json!({
                            "guild_id": g.id,
                            "user": user_json(a, n),
                            "roles": [],
                            "joined_at": rfc3339(ms),
                        }),
                    )
                } else {
                    (
                        "GUILD_MEMBER_REMOVE",
                        json!({"guild_id": g.id, "user": user_json(a, n)}),
                    )
                }
            }
        };
        Some(Frame {
            at_ms,
            account: a,
            name,
            data,
        })
    }
}

/// Writes frames as JSON lines: `{"at":ms,"account":n,"event":"NAME","data":{…}}`.
#[allow(dead_code)] // for looking at a recording by hand; the tests use it
pub fn write_jsonl(
    frames: impl Iterator<Item = Frame>,
    out: &mut impl Write,
) -> std::io::Result<u64> {
    let mut n = 0;
    for f in frames {
        let line = json!({"at": f.at_ms, "account": f.account, "event": f.name, "data": f.data});
        writeln!(out, "{line}")?;
        n += 1;
    }
    Ok(n)
}
