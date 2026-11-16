// SPDX-License-Identifier: AGPL-3.0-or-later
//! The soak: the app running for hours on a recorded gateway, to see that it stays small
//! and does not grow. Demo builds only, started with `INLET_SOAK=1` (with `INLET_DEMO=1`).
//!
//! What it plays: three accounts, as the account switcher has them. The first is on
//! screen with 3 guilds and 30 text channels, one of them with 5,000 messages of
//! history; the two others run in the background. Their gateways are fed from the
//! synthetic recording in [`recording`] (all invented, nothing real), through the same
//! `forward_gateway` the real accounts use, so the conversion, the account state, the
//! unread marks and the notifications take the real paths. A driver opens the channels
//! one after another in the window, and scrolls the long one back page by page.
//!
//! The server side is faked without storing the history: history pages are made on
//! demand from the channel and the page asked for ([`Server`]), and only the newest live
//! messages of each channel are kept, so the fake server itself stays small.
//!
//! Variables:
//! - `INLET_SOAK`: turns the soak on.
//! - `INLET_SOAK_HOURS`: hours of recording (default 24).
//! - `INLET_SOAK_SPEED`: how many times faster than real time the recording plays
//!   (default 1; 48 plays a day in 30 minutes).
//! - `INLET_SOAK_NAV_MS`: real time between two channel switches (default 5000; 0: none).
//! - `INLET_SOAK_LOG_SECS`: real time between two lines of numbers on stderr (default 60).
//! - `INLET_SOAK_SEED`: the recording's seed (default 1).
//! - `INLET_SOAK_RATE`: how busy the recording is (default 1; 24 fills the message windows in hours).
//! - `INLET_SOAK_STARTUP`: also without `INLET_SOAK`, prints the time to the first frame.
//!
//! The numbers (`soak: ...` lines on stderr) are what `scripts/soak.sh` reads, next to
//! its own sampling of the process's memory.

mod recording;
#[cfg(test)]
mod tests;

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, WindowHandle};
use inlet_protocol as p;
use inlet_state::MessageId;
use tokio::task::JoinHandle;

use self::recording::{frames, history_message, ready_json, ChannelSpec, GuildSpec, Plan, World};
use super::scenes::Data;
use crate::app::Inlet;
use crate::backend::composer;
use crate::backend::gateway::forward_gateway;
use crate::backend::messages::parse::SharedNames;
use crate::backend::Ui;
use crate::model::{Channel, ChannelKind, Guild, Names};

/// Live messages the fake server keeps per channel (a page is 50).
const LIVE_PER_CHANNEL: usize = 60;
/// The history pages a request may be answered from, on top of the page asked for.
const PAGE_SLACK: usize = 2;
/// Messages of history one page asks for.
const PAGE: usize = crate::backend::messages::PAGE_SIZE as usize;
/// Channel switches that go to the long channel, one in this many, and how many older
/// pages are loaded there before the next switch.
const ARCHIVE_EVERY: usize = 10;
const ARCHIVE_PAGES: u32 = 6;

/// What `INLET_SOAK*` asks for.
#[derive(Clone, Debug)]
pub struct Config {
    pub seed: u64,
    pub hours: f64,
    pub speed: f64,
    pub nav_ms: u64,
    pub log_secs: u64,
    pub rate: f64,
}

impl Config {
    /// `None` unless `INLET_SOAK` is set.
    pub fn from_env() -> Option<Config> {
        std::env::var_os("INLET_SOAK")?;
        let num = |name: &str, default: f64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.)
                .unwrap_or(default)
        };
        Some(Config {
            seed: num("INLET_SOAK_SEED", 1.) as u64,
            hours: num("INLET_SOAK_HOURS", 24.),
            speed: num("INLET_SOAK_SPEED", 1.).max(0.01),
            nav_ms: match num("INLET_SOAK_NAV_MS", 5000.) {
                0. => 0,
                ms => ms.max(100.) as u64,
            },
            log_secs: num("INLET_SOAK_LOG_SECS", 60.).max(1.) as u64,
            rate: num("INLET_SOAK_RATE", 1.),
        })
    }

    pub fn plan(&self) -> Plan {
        Plan {
            seed: self.seed,
            duration_ms: (self.hours * 3_600_000.) as u64,
            rate: self.rate,
        }
    }
}

/// The start-up data of the first account: its guilds, channels and READY. The messages
/// are not in it: the [`Server`] makes the pages.
pub fn data() -> Data {
    let world = World::build();
    let a0 = &world.accounts[0];
    let guilds = a0
        .guilds
        .iter()
        .map(|g| Guild {
            id: g.id.clone(),
            name: g.name.clone(),
        })
        .collect();
    let mut channels: HashMap<Option<String>, Vec<Channel>> = HashMap::new();
    for g in &a0.guilds {
        let list = g
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| Channel {
                id: c.id.clone(),
                name: c.name.clone(),
                kind: ChannelKind::Text,
                parent_id: None,
                position: i as i64,
                topic: Some(format!("The {} channel", c.name)),
                slowmode_secs: 0,
            })
            .collect();
        channels.insert(Some(g.id.clone()), list);
    }
    channels.insert(
        None,
        a0.dms
            .iter()
            .enumerate()
            .map(|(i, c)| Channel {
                id: c.id.clone(),
                name: c.name.clone(),
                kind: ChannelKind::Dm,
                parent_id: None,
                position: i as i64,
                topic: None,
                slowmode_secs: 0,
            })
            .collect(),
    );
    let ready: p::Ready =
        serde_json::from_value(ready_json(a0)).expect("the soak's READY is valid");
    Data {
        guilds,
        channels,
        messages: HashMap::new(),
        ready,
        outgoing: Vec::new(),
    }
}

/// The fake server's side of the first account: pages of history made on demand, and
/// the newest live messages of every channel.
pub struct Server {
    world: World,
    live: Mutex<HashMap<String, VecDeque<p::Message>>>,
}

impl Server {
    pub fn new() -> Server {
        Server {
            world: World::build(),
            live: Mutex::default(),
        }
    }

    fn find(&self, channel_id: &str) -> Option<(Option<&GuildSpec>, &ChannelSpec)> {
        let a0 = &self.world.accounts[0];
        a0.guilds
            .iter()
            .flat_map(|g| g.channels.iter().map(move |c| (Some(g), c)))
            .chain(a0.dms.iter().map(|c| (None, c)))
            .find(|(_, c)| c.id == channel_id)
    }

    /// A live message the gateway delivered; the oldest of the channel's live messages
    /// go when there are more than [`LIVE_PER_CHANNEL`].
    pub fn push_live(&self, m: &p::Message) {
        let mut live = self.live.lock().unwrap();
        let list = live.entry(m.channel_id.clone()).or_default();
        list.push_back(m.clone());
        while list.len() > LIVE_PER_CHANNEL {
            list.pop_front();
        }
    }

    /// Messages that may answer a request for the page before `before` (the newest page
    /// without it): more than a page, the caller cuts it to size.
    pub fn candidates(&self, channel_id: &str, before: Option<MessageId>) -> Vec<p::Message> {
        let Some((guild, chan)) = self.find(channel_id) else {
            return Vec::new();
        };
        let a0 = &self.world.accounts[0];
        let end = match before {
            None => chan.history,
            Some(b) => recording::history_before(chan, b.timestamp_ms()),
        };
        let start = end.saturating_sub(PAGE + PAGE_SLACK);
        let mut out: Vec<p::Message> = (start..end)
            .filter_map(|k| serde_json::from_value(history_message(a0, guild, chan, k)).ok())
            .collect();
        if let Some(list) = self.live.lock().unwrap().get(channel_id) {
            out.extend(list.iter().cloned());
        }
        out
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

/// The running replay: stops when dropped.
pub struct Running {
    pub server: Arc<Server>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Starts the three gateways' forwarders and the replay that feeds them. `events` is the
/// demo account's sender; `channels` and `guilds` are what [`data`] returned. Runs inside
/// the backend's runtime.
pub fn start(
    cfg: &Config,
    events: &Ui,
    names: &SharedNames,
    channels: &HashMap<Option<String>, Vec<Channel>>,
    guilds: &[Guild],
) -> Running {
    let server = Arc::new(Server::new());
    let keys = super::accounts::list();
    let mut tasks = Vec::new();
    let mut senders = Vec::new();
    for (a, info) in keys.iter().enumerate() {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let (ui, guild_list, cache, names) = if a == 0 {
            (
                events.clone(),
                guilds.to_vec(),
                channels.clone(),
                names.clone(),
            )
        } else {
            (
                events.for_account(info.key.clone()),
                Vec::new(),
                HashMap::new(),
                Arc::new(Mutex::new(Names::default())),
            )
        };
        let foreground = Arc::new(AtomicBool::new(a == 0));
        tasks.push(tokio::spawn(async move {
            let end = forward_gateway(
                rx,
                ui,
                Arc::new(Mutex::new(guild_list)),
                Arc::new(Mutex::new(cache)),
                names,
                foreground,
                composer::State::default(),
                || {},
            )
            .await;
            log::warn!("soak: gateway {a} ended: {end:?}");
        }));
        senders.push(tx);
    }
    let plan = cfg.plan();
    let speed = cfg.speed;
    let replay_server = server.clone();
    tasks.push(tokio::spawn(async move {
        let t0 = tokio::time::Instant::now();
        let mut seq = vec![0u64; senders.len()];
        let mut played = 0u64;
        let mut ended = vec![false; senders.len()];
        for mut frame in frames(plan) {
            let due = t0 + Duration::from_secs_f64(frame.at_ms as f64 / 1000. / speed);
            tokio::time::sleep_until(due).await;
            if frame.name == "TYPING_START" {
                frame.data["timestamp"] = chrono::Utc::now().timestamp().into();
            }
            let event = p::Dispatch::parse(frame.name, frame.data);
            if let (0, p::Dispatch::MessageCreate(m)) = (frame.account, &event) {
                replay_server.push_live(m);
            }
            seq[frame.account] += 1;
            // A forwarder that has ended costs its own account's events only: the others go on.
            let sent = senders[frame.account]
                .send(p::GatewayEvent::Dispatch {
                    seq: seq[frame.account],
                    event,
                })
                .await;
            if sent.is_err() {
                if !ended[frame.account] {
                    ended[frame.account] = true;
                    eprintln!(
                        "soak: the gateway of account {} has ended; its events are dropped",
                        frame.account
                    );
                }
                continue;
            }
            played += 1;
        }
        eprintln!("soak: the recording is over after {played} events");
        // The gateways stay open: closing them would end the forwarders.
        std::future::pending::<()>().await;
    }));
    Running { server, tasks }
}

// ---- Measuring --------------------------------------------------------------------------

/// Counts the bytes the program holds, so a log can tell what the data weighs from what
/// the allocator keeps around. It wraps the system allocator in demo builds (which are
/// never shipped) and changes nothing else.
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

/// Bytes the program has allocated and not freed.
pub fn heap_bytes() -> isize {
    LIVE.load(Ordering::Relaxed)
}

/// A number from `/proc/self/status` in kB (`VmRSS`, `VmHWM`).
pub fn proc_kb(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// How long ago this process started (Linux).
#[cfg(target_os = "linux")]
fn process_age() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Field 22 is the start time in clock ticks since boot; the fields after the
    // command's closing parenthesis start at field 3.
    let ticks: f64 = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    // Linux counts 100 ticks a second on every platform Inlet runs on.
    Some(Duration::from_secs_f64((uptime - ticks / 100.).max(0.)))
}

#[cfg(not(target_os = "linux"))]
fn process_age() -> Option<Duration> {
    None
}

/// Starts the driver in the window: the first-frame time, the channel switching and the
/// lines of numbers. Does nothing without `INLET_SOAK` or `INLET_SOAK_STARTUP`.
pub fn attach(handle: WindowHandle<Inlet>, cx: &mut App) {
    let cfg = Config::from_env();
    if cfg.is_some() || std::env::var_os("INLET_SOAK_STARTUP").is_some() {
        let before = process_age();
        let t0 = Instant::now();
        let _ = handle.update(cx, |_, window, _| {
            // Runs right after the first frame is drawn.
            window.on_next_frame(move |_, _| {
                let since = t0.elapsed() + before.unwrap_or_default();
                eprintln!(
                    "soak: first frame {} ms after the process started (window code ran {} ms after)",
                    since.as_millis(),
                    t0.elapsed().as_millis()
                );
            });
        });
    }
    let Some(cfg) = cfg else {
        return;
    };

    // The channels to open, and the long one.
    let world = World::build();
    let open: Vec<(String, String)> = world
        .open_channels()
        .iter()
        .map(|(g, c)| (g.id.clone(), c.id.clone()))
        .collect();
    let archive = world
        .open_channels()
        .iter()
        .find(|(_, c)| c.history >= recording::BIG_HISTORY)
        .map(|(g, c)| (g.id.clone(), c.id.clone()));
    let nav = Duration::from_millis(cfg.nav_ms);
    // INLET_SOAK_NAV_MS=0: nobody switches channels.
    if cfg.nav_ms > 0 {
        cx.spawn(async move |cx| {
            let mut step = 0usize;
            let mut dwell = 0u32;
            loop {
                cx.background_executor().timer(nav).await;
                let alive = handle.update(cx, |this, window, cx| {
                    if this.account.is_none() || open.is_empty() {
                        return;
                    }
                    // On the long channel: older pages, one per tick (the app's own pager
                    // allows one a second).
                    if dwell > 0 {
                        dwell -= 1;
                        this.load_older(cx);
                        return;
                    }
                    step += 1;
                    let (guild, channel) = match &archive {
                        Some(a) if step.is_multiple_of(ARCHIVE_EVERY) => {
                            dwell = ARCHIVE_PAGES;
                            a.clone()
                        }
                        _ => open[(step * 7 + step / open.len()) % open.len()].clone(),
                    };
                    if this.selected_guild.as_deref() == Some(guild.as_str()) {
                        this.select_channel(channel, window, cx);
                    } else {
                        this.demo_channel = Some(channel);
                        this.select_guild(Some(guild), cx);
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    // Numbers, so a log shows what grew.
    let every = Duration::from_secs(cfg.log_secs);
    cx.spawn(async move |cx| {
        let started = Instant::now();
        loop {
            let asked = Instant::now();
            cx.background_executor().timer(every).await;
            // How much later than asked the tick ran: the UI thread being behind shows here.
            let late = asked.elapsed().saturating_sub(every);
            let alive = handle.update(cx, |this, _, cx| {
                let (windows, messages, bytes, members) = match &this.account {
                    Some(account) => {
                        let state = account.read(cx);
                        let w = state.message_windows();
                        let (mut n, mut b) = (0usize, 0usize);
                        for ch in w.channels() {
                            if let Some(win) = w.get(ch) {
                                n += win.len();
                                b += win.approx_bytes();
                            }
                        }
                        (w.len(), n, b, state.member_total())
                    }
                    None => (0, 0, 0, 0),
                };
                eprintln!(
                    "soak: up_s={} rss_kb={} hwm_kb={} heap_kb={} windows={windows} window_messages={messages} window_bytes={bytes} members={members} late_ms={}",
                    started.elapsed().as_secs(),
                    proc_kb("VmRSS").unwrap_or(0),
                    proc_kb("VmHWM").unwrap_or(0),
                    heap_bytes() / 1024,
                    late.as_millis(),
                );
            });
            if alive.is_err() {
                return;
            }
        }
    })
    .detach();
}
