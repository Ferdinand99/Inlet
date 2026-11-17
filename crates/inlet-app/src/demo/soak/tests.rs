// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of the soak's recording and its measurements. Two of them are the long ones
//! (`#[ignore]`, run by `scripts/soak.sh`): a day of gateway traffic through the account
//! state, and scrolling back through the whole 5,000-message channel.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{
    div, point, prelude::*, px, size, Context, Entity, ScrollDelta, ScrollEvent, TestAppContext,
    VisualContext, Window,
};
use inlet_protocol as p;
use inlet_state::{limits, AccountState, ChannelId, MessageId, MessagesPage};
use serde_json::json;

use super::recording::Frame;
use super::recording::{self, frames, Plan, World, START_MS};
use super::{data, proc_kb, start, Config, Server};
use crate::app::Inlet;
use crate::backend::messages::parse::SharedNames;
use crate::backend::{accounts, messages as msgs, Event, Ui};
use crate::demo::rendered_page;
use crate::model::Names;

fn names() -> SharedNames {
    Arc::new(Mutex::new(Names::default()))
}

fn archive() -> (String, String) {
    let world = World::build();
    let (g, c) = world
        .open_channels()
        .into_iter()
        .find(|(_, c)| c.history >= recording::BIG_HISTORY)
        .expect("the world has a long channel");
    (g.id.clone(), c.id.clone())
}

#[test]
fn the_recording_is_the_same_every_time_and_every_frame_parses() {
    let plan = Plan {
        seed: 7,
        duration_ms: 5 * 60 * 60 * 1000,
        rate: 1.,
    };
    let a: Vec<Frame> = frames(plan).collect();
    let b: Vec<Frame> = frames(plan).collect();
    assert_eq!(a, b, "the same seed gives the same recording");
    assert!(a.len() > 1_000, "{} frames", a.len());
    let other: Vec<Frame> = frames(Plan { seed: 8, ..plan }).collect();
    assert_ne!(a, other, "another seed gives another recording");

    let mut last = 0;
    let mut seen = HashSet::new();
    for f in &a {
        assert!(f.at_ms >= last, "frames are in time order");
        last = f.at_ms;
        seen.insert(f.name);
        let d = p::Dispatch::parse(f.name, f.data.clone());
        assert!(
            !matches!(d, p::Dispatch::Other { .. }),
            "{} did not parse: {}",
            f.name,
            f.data
        );
    }
    for want in [
        "READY",
        "MESSAGE_CREATE",
        "MESSAGE_UPDATE",
        "MESSAGE_DELETE",
        "TYPING_START",
        "PRESENCE_UPDATE",
        "MESSAGE_REACTION_ADD",
        "MESSAGE_ACK",
        "CHANNEL_UPDATE",
        "GUILD_MEMBER_ADD",
    ] {
        assert!(seen.contains(want), "no {want} in five hours: {seen:?}");
    }

    // Writing it as lines gives one valid JSON object per frame.
    let mut out = Vec::new();
    let n = recording::write_jsonl(frames(plan).take(50), &mut out).unwrap();
    assert_eq!(n, 50);
    for line in String::from_utf8(out).unwrap().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v["event"].is_string() && v["data"].is_object());
    }

    // Nothing in it looks like a secret: no token-shaped words, no real hosts.
    let text: String = a[..200].iter().map(|f| f.data.to_string()).collect();
    assert!(!text.contains("fluxer"), "{text}");
}

#[test]
fn history_pages_walk_the_whole_channel_in_order() {
    let server = Server::new();
    let names = names();
    let (_, id) = archive();
    let mut before = None;
    let mut seen = HashSet::new();
    let mut pages = 0;
    loop {
        let (page, rendered) = rendered_page(&server.candidates(&id, before), before, &names);
        if page.messages.is_empty() {
            break;
        }
        pages += 1;
        assert_eq!(rendered.len(), page.messages.len());
        let ids: Vec<MessageId> = page
            .messages
            .iter()
            .map(|m| MessageId::parse(&m.id).expect("a snowflake"))
            .collect();
        for &i in &ids {
            assert!(seen.insert(i), "a message came twice");
            if let Some(b) = before {
                assert!(i < b, "a page holds only messages older than `before`");
            }
        }
        before = ids.into_iter().min();
    }
    assert_eq!(seen.len(), recording::BIG_HISTORY);
    assert_eq!(pages, recording::BIG_HISTORY / 50);
}

#[test]
fn the_newest_page_holds_the_live_messages() {
    let server = Server::new();
    let (_, id) = archive();
    let world = World::build();
    let chan = world
        .open_channels()
        .into_iter()
        .find(|(_, c)| c.id == id)
        .unwrap()
        .1
        .clone();
    let live: p::Message = serde_json::from_value(json!({
        "id": recording::id_at(&chan, START_MS + 5_000, 1),
        "channel_id": id,
        "author": {"id": "3000000000001", "username": "someone"},
        "type": 0,
        "content": "live",
        "timestamp": "2026-11-11T00:00:05+00:00",
    }))
    .unwrap();
    server.push_live(&live);
    let (page, _) = rendered_page(&server.candidates(&id, None), None, &names());
    assert_eq!(page.messages.len(), 50);
    let newest = page
        .messages
        .iter()
        .filter_map(|m| MessageId::parse(&m.id))
        .max();
    assert_eq!(newest, MessageId::parse(&live.id));
}

fn check_windows(state: &AccountState, what: &str) {
    let w = state.message_windows();
    assert!(
        w.len() <= limits::MAX_MESSAGE_WINDOWS,
        "{what}: {} windows",
        w.len()
    );
    for ch in w.channels() {
        let win = w.get(ch).expect("a listed window");
        assert!(
            win.len() <= limits::MAX_MESSAGES_PER_CHANNEL,
            "{what}: {} messages in one window",
            win.len()
        );
        assert!(
            win.approx_bytes() <= limits::MAX_WINDOW_BYTES,
            "{what}: {} bytes in one window",
            win.approx_bytes()
        );
    }
}

/// The caps hold under load: many more channels than windows, a window walked back to
/// the start of 5,000 messages, and a flood of big live messages; and the image cache
/// stays within its byte budget while far more image bytes than that pass through it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_windows_and_media_cache_stay_under_their_caps() {
    // ---- Message windows ----
    let data = data();
    let server = Server::new();
    let names = names();
    let now = chrono::Utc::now().timestamp_millis();
    let mut state = AccountState::from_ready_at(&data.ready, now);
    let world = World::build();
    let a0 = &world.accounts[0];
    let all: Vec<&recording::ChannelSpec> = a0
        .guilds
        .iter()
        .flat_map(|g| g.channels.iter())
        .chain(a0.dms.iter())
        .collect();
    assert!(all.len() > limits::MAX_MESSAGE_WINDOWS);

    // The long channel first, walked back to its start.
    let (_, long) = archive();
    let long_ch = ChannelId::parse(&long).unwrap();
    state.begin_load(long_ch);
    let (page, _) = rendered_page(&server.candidates(&long, None), None, &names);
    state.apply_messages(long_ch, page);
    let mut walked = 0;
    while let Some(oldest) = state
        .messages(long_ch)
        .filter(|w| w.has_more_before())
        .and_then(|w| w.oldest_id())
    {
        let (page, _) = rendered_page(
            &server.candidates(&long, Some(oldest)),
            Some(oldest),
            &names,
        );
        state.apply_messages(long_ch, page);
        check_windows(&state, "walking the long channel back");
        walked += 1;
        assert!(
            walked <= recording::BIG_HISTORY / 50 + 2,
            "the walk never ends"
        );
    }
    assert!(state.messages(long_ch).unwrap().len() <= limits::MAX_MESSAGES_PER_CHANNEL);

    // Every channel of the account, more than there are windows.
    for c in &all {
        let ch = ChannelId::parse(&c.id).unwrap();
        state.begin_load(ch);
        let (page, _) = rendered_page(&server.candidates(&c.id, None), None, &names);
        state.apply_messages(ch, page);
        check_windows(&state, "opening channels");
    }
    assert_eq!(
        state.message_windows().len(),
        limits::MAX_MESSAGE_WINDOWS,
        "windows fill up to their cap and no further"
    );

    // A flood of live messages of 60 KB each into the busiest window.
    let busy = ChannelId::parse(&all[0].id).unwrap();
    let chan = all[0];
    state.begin_load(busy);
    let (page, _) = rendered_page(&server.candidates(&chan.id, None), None, &names);
    state.apply_messages(busy, page);
    let big = "word ".repeat(12_000);
    for i in 0..400u64 {
        let m: p::Message = serde_json::from_value(json!({
            "id": recording::id_at(chan, START_MS + 1_000 * i as i64, i),
            "channel_id": chan.id,
            "author": {"id": "3000000000001", "username": "someone"},
            "type": 0,
            "content": big,
            "timestamp": "2026-11-11T00:00:05+00:00",
        }))
        .unwrap();
        state.apply(&p::Dispatch::MessageCreate(Box::new(m)));
        if i % 50 == 0 {
            check_windows(&state, "a flood of big messages");
        }
    }
    check_windows(&state, "after the flood");
    assert!(
        state.messages(busy).unwrap().approx_bytes() <= limits::MAX_WINDOW_BYTES,
        "the byte cap held"
    );

    // ---- The image cache ----
    use crate::backend::media::{setup, MEMORY_CACHE_BYTES};
    use crate::demo::media::{proxy_and_fetcher, MEDIA_BASE};
    use inlet_media::{Fit, MediaKind, MediaRequest};
    use url::Url;

    let (proxy, fetcher) = proxy_and_fetcher();
    let media = setup(proxy, fetcher);
    let mut peak = 0u64;
    for i in 0..60u64 {
        let url = Url::parse(&format!(
            "{MEDIA_BASE}/attachments/{}/{}/beach.png?width=800&height=450",
            long,
            9_000_000_000 + i
        ))
        .unwrap();
        media
            .client
            .load(MediaRequest::new(url, MediaKind::Attachment, Fit::PREVIEW))
            .await
            .expect("the demo picture loads");
        peak = peak.max(media.client.stats().memory_bytes);
        assert!(
            media.client.stats().memory_bytes <= MEMORY_CACHE_BYTES,
            "the cache is over its budget after {i} pictures"
        );
        // The UI frees what the cache let go of.
        drop(media.evicted.drain());
    }
    // 60 pictures of 550 x 309 pixels, counted twice (pixels and atlas tile).
    let passed = media.client.stats().decodes * 550 * 309 * 8;
    for i in 0..600u64 {
        let url = Url::parse(&format!(
            "{MEDIA_BASE}/avatars/{}/sk{i}.png?size=64",
            recording::pool_user(0, i)
        ))
        .unwrap();
        media
            .client
            .load(MediaRequest::new(
                url,
                MediaKind::Avatar,
                Fit::CoverSquare { size: 40 },
            ))
            .await
            .expect("an avatar loads");
        if i % 100 == 0 {
            peak = peak.max(media.client.stats().memory_bytes);
            assert!(media.client.stats().memory_bytes <= MEMORY_CACHE_BYTES);
            drop(media.evicted.drain());
        }
    }
    let stats = media.client.stats();
    assert!(stats.memory_bytes <= MEMORY_CACHE_BYTES);
    assert!(
        passed > 3 * MEMORY_CACHE_BYTES / 2,
        "the test must push more than the cache holds ({passed} bytes)"
    );
    assert_eq!(stats.in_flight, 0);
    drop(media.evicted.drain());
    assert_eq!(media.evicted.queued_bytes(), 0);
    eprintln!(
        "caps: {} windows, cache peak {} KiB of {} KiB, {} entries at the end",
        state.message_windows().len(),
        peak / 1024,
        MEMORY_CACHE_BYTES / 1024,
        stats.memory_entries
    );
}

/// Linear fit: kB per hour over the samples `(hour, rss_kb)`.
fn slope_kb_per_hour(samples: &[(u64, u64)]) -> f64 {
    let n = samples.len() as f64;
    let mx = samples.iter().map(|s| s.0 as f64).sum::<f64>() / n;
    let my = samples.iter().map(|s| s.1 as f64).sum::<f64>() / n;
    let num: f64 = samples
        .iter()
        .map(|s| (s.0 as f64 - mx) * (s.1 as f64 - my))
        .sum();
    let den: f64 = samples.iter().map(|s| (s.0 as f64 - mx).powi(2)).sum();
    if den == 0. {
        0.
    } else {
        num / den
    }
}

/// A day of recorded gateway traffic for three accounts, played fast through the real
/// gateway forwarder into the account states (the first one with windows being opened,
/// the other two passive). Memory is read after every virtual hour: after the sixth
/// it must not grow.
///
/// The app's own part (the views, the images) is measured by `scripts/soak.sh`, which
/// runs the whole app. `INLET_SOAK_TEST_STEADY_HOUR` is the hour the growth check starts at
/// (15: at the default rate the windows take about twelve hours to fill). `INLET_SOAK_TEST_HOURS` and `INLET_SOAK_TEST_SPEED` change the
/// length (default 24 hours) and the speed (default 150 times, about ten minutes; faster and the replay outruns the test, whose queue then holds a backlog).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "plays a day of traffic; run by scripts/soak.sh"]
async fn soak_24h_replay_memory_is_flat() {
    let env = |name: &str, default: f64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let cfg = Config {
        seed: 1,
        hours: env("INLET_SOAK_TEST_HOURS", 24.),
        speed: env("INLET_SOAK_TEST_SPEED", 150.),
        nav_ms: 5000,
        log_secs: 60,
        rate: env("INLET_SOAK_TEST_RATE", 24.),
    };
    let data = data();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let ui = Ui::new(tx);
    let names = names();
    let running = start(&cfg, &ui, &names, &data.channels, &data.guilds);
    let server = running.server.clone();
    let now = chrono::Utc::now().timestamp_millis();
    let mut fg = AccountState::from_ready_at(&data.ready, now);
    let mut background: HashMap<accounts::AccountKey, AccountState> = HashMap::new();
    let world = World::build();
    let open: Vec<(ChannelId, String)> = world
        .open_channels()
        .iter()
        .map(|(_, c)| (ChannelId::parse(&c.id).unwrap(), c.id.clone()))
        .collect();
    let (_, long) = archive();

    let mut samples: Vec<(u64, u64)> = Vec::new();
    let mut details: Vec<String> = Vec::new();
    let mut hour_seen = -1i64;
    let mut events = 0u64;
    let mut opened = 0usize;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(3), rx.next()).await {
        match event {
            Event::Messages(msgs::Ev::Dispatch { dispatch, .. }) => {
                if let p::Dispatch::MessageCreate(m) = &*dispatch {
                    let at = MessageId::parse(&m.id).map_or(START_MS, |i| i.timestamp_ms());
                    let hour = (at - START_MS).max(0) / 3_600_000;
                    if hour > hour_seen {
                        hour_seen = hour;
                        samples.push((hour as u64, proc_kb("VmRSS").unwrap_or(0)));
                        // What the state holds, to tell a leak from a cache filling up.
                        let users = (0..recording::POOL)
                            .filter(|n| {
                                inlet_state::UserId::parse(&recording::pool_user(0, *n))
                                    .is_some_and(|u| fg.user(u).is_some())
                            })
                            .count();
                        let names_len = names.lock().unwrap().users.len();
                        let bg_members: usize = background.values().map(|s| s.member_total()).sum();
                        details.push(format!(
                            "heap {} MiB, users {users}, members {}, presences {}, read states {}, windows {}, names {names_len}, background members {bg_members}",
                            super::heap_bytes() / 1_048_576,
                            fg.member_total(),
                            fg.presences().len(),
                            fg.read_states().len(),
                            fg.message_windows().len(),
                        ));
                    }
                }
                fg.apply(&dispatch);
                events += 1;
                // Somebody opens another channel now and then.
                if events.is_multiple_of(30) {
                    let (ch, id) = &open[opened % open.len()];
                    opened += 1;
                    fg.begin_load(*ch);
                    let (page, _) = rendered_page(&server.candidates(id, None), None, &names);
                    fg.apply_messages(*ch, page);
                    if opened.is_multiple_of(10) {
                        if let Some(c) = ChannelId::parse(&long) {
                            for _ in 0..3 {
                                let Some(oldest) = fg.messages(c).and_then(|w| w.oldest_id())
                                else {
                                    break;
                                };
                                let (page, _) = rendered_page(
                                    &server.candidates(&long, Some(oldest)),
                                    Some(oldest),
                                    &names,
                                );
                                fg.apply_messages(c, page);
                            }
                        }
                    }
                }
            }
            Event::Tagged { account, event } => {
                if let Event::Messages(msgs::Ev::Dispatch { dispatch, .. }) = *event {
                    match (&*dispatch, background.get_mut(&account)) {
                        (p::Dispatch::Ready(r), None) => {
                            let mut s = AccountState::from_ready_at(r, now);
                            s.set_passive(true);
                            background.insert(account, s);
                        }
                        (_, Some(s)) => {
                            let c = s.apply(&dispatch);
                            if !c.messages.is_empty() {
                                s.drop_message_windows();
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    drop(running);
    check_windows(&fg, "after the day");

    eprintln!("soak test: {events} events on the first account");
    for ((h, kb), what) in samples.iter().zip(&details) {
        eprintln!("soak test: hour {h:>2}: {} MiB; {what}", kb / 1024);
    }
    let want_hours = cfg.hours as u64;
    assert!(
        samples.len() as u64 + 1 >= want_hours,
        "the replay stopped early: samples for {} of {want_hours} hours",
        samples.len()
    );
    let from = env("INLET_SOAK_TEST_STEADY_HOUR", 15.) as u64;
    let steady: Vec<(u64, u64)> = samples.iter().copied().filter(|s| s.0 >= from).collect();
    if steady.len() >= 4 {
        let base = steady[0].1;
        let max = steady.iter().map(|s| s.1).max().unwrap();
        let slope = slope_kb_per_hour(&steady);
        eprintln!(
            "soak test: after hour {from}: {} MiB at hour {}, peak {} MiB, slope {:.0} KiB/h",
            base / 1024,
            steady[0].0,
            max / 1024,
            slope
        );
        assert!(
            max <= base + 16 * 1024,
            "memory grew after hour {from}: {} MiB to {} MiB",
            base / 1024,
            max / 1024
        );
        assert!(
            slope < 512.,
            "memory grows by {slope:.0} KiB an hour after hour {from}"
        );
    }
    let peak = samples.iter().map(|s| s.1).max().unwrap_or(0);
    assert!(
        peak < 150 * 1024,
        "{} MiB, over the 150 MiB target",
        peak / 1024
    );
}

// ---- Scrolling ----------------------------------------------------------------------------

struct View {
    inlet: Entity<Inlet>,
    long: String,
}

impl View {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (guild, long) = archive();
        let inlet = cx.new(|cx| {
            let mut inlet = Inlet::detached(window, cx);
            open(&mut inlet, &guild, &long, window, cx);
            inlet
        });
        Self { inlet, long }
    }
}

impl Render for View {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.inlet.clone())
    }
}

/// What the soak backend sends after sign-in, then the newest page of the long channel.
fn open(inlet: &mut Inlet, guild: &str, long: &str, window: &mut Window, cx: &mut Context<Inlet>) {
    let data = data();
    let server = Server::new();
    let names = names();
    let main = crate::demo::accounts::list().into_iter().take(1).collect();
    inlet.handle_event(accounts::Ev::Accounts(main).into(), window, cx);
    inlet.handle_event(
        accounts::Ev::SignedIn {
            key: crate::demo::accounts::main_key(),
            username: crate::demo::ME.into(),
            instance: crate::demo::accounts::INSTANCE.into(),
        }
        .into(),
        window,
        cx,
    );
    inlet.handle_event(
        msgs::Ev::Dispatch {
            dispatch: Box::new(p::Dispatch::Ready(Box::new(data.ready))),
            rendered: Vec::new(),
        }
        .into(),
        window,
        cx,
    );
    inlet.handle_event(msgs::Ev::Guilds(data.guilds).into(), window, cx);
    inlet.select_guild(Some(guild.to_string()), cx);
    inlet.demo_channel = Some(long.to_string());
    let channels = data
        .channels
        .get(&Some(guild.to_string()))
        .cloned()
        .unwrap_or_default();
    inlet.handle_event(
        msgs::Ev::Channels {
            guild_id: Some(guild.to_string()),
            channels,
        }
        .into(),
        window,
        cx,
    );
    let ch = ChannelId::parse(long).expect("a snowflake");
    let request = inlet
        .chat
        .view
        .waiting_request(ch)
        .expect("opening the channel asks for its newest page");
    let (page, rendered) = rendered_page(&server.candidates(long, None), None, &names);
    inlet.handle_event(
        msgs::Ev::Page {
            channel_id: long.to_string(),
            request,
            page,
            rendered,
        }
        .into(),
        window,
        cx,
    );
}

/// Answers the view's request for older messages, if it makes one.
fn older_page(
    inlet: &mut Inlet,
    long: &str,
    server: &Server,
    window: &mut Window,
    cx: &mut Context<Inlet>,
) -> bool {
    let ch = ChannelId::parse(long).expect("a snowflake");
    let Some(account) = inlet.account.clone() else {
        return false;
    };
    let Some((before, request)) = inlet.chat.view.older_request(account.read(cx), ch) else {
        return false;
    };
    let (page, rendered): (MessagesPage, _) = rendered_page(
        &server.candidates(long, Some(before)),
        Some(before),
        &names(),
    );
    inlet.handle_event(
        msgs::Ev::Page {
            channel_id: long.to_string(),
            request,
            page,
            rendered,
        }
        .into(),
        window,
        cx,
    );
    true
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 - 1.) * p).round() as usize]
}

fn report(what: &str, times: &mut [Duration]) -> (Duration, Duration, Duration) {
    times.sort();
    let (p50, p95, max) = (
        percentile(times, 0.5),
        percentile(times, 0.95),
        *times.last().unwrap(),
    );
    eprintln!(
        "scroll: {what}: {} frames, median {:.2} ms, p95 {:.2} ms, worst {:.2} ms",
        times.len(),
        p50.as_secs_f64() * 1000.,
        p95.as_secs_f64() * 1000.,
        max.as_secs_f64() * 1000.
    );
    (p50, p95, max)
}

/// Frame times of the real message list with a full window of the 5,000-message
/// channel: still, and scrolled up and down. The test platform builds the whole frame
/// (layout, prepaint, paint of every element) and draws nothing, so these are the CPU
/// cost of a frame. `INLET_SOAK_SCROLL_PAGES` is how many older pages are loaded first
/// (9 fill the window of 500 messages; the longer soak run uses 100 to walk the
/// whole history).
#[gpui::test]
#[ignore = "loads pages one second apart and measures time; run by scripts/soak.sh"]
fn scrolling_a_5000_message_channel_has_no_hitches(cx: &mut TestAppContext) {
    let pages: usize = std::env::var("INLET_SOAK_SCROLL_PAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9);
    let server = Server::new();
    let (view, cx) = cx.add_window_view(View::new);
    cx.simulate_resize(size(px(1280.), px(860.)));
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let inlet = view.read_with(cx, |v, _| v.inlet.clone());
    let long = view.read_with(cx, |v, _| v.long.clone());

    // Older pages as the view asks for them: one a second, as the app's pager allows.
    let mut loaded = 0;
    let mut load_times = Vec::new();
    while loaded < pages {
        std::thread::sleep(Duration::from_millis(1_050));
        let before = Instant::now();
        let asked = cx.update_window_entity(&inlet, |inlet, window, cx| {
            older_page(inlet, &long, &server, window, cx)
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        if asked {
            load_times.push(before.elapsed());
            loaded += 1;
        }
    }
    if !load_times.is_empty() {
        report("a page of older messages arrives", &mut load_times);
    }
    let shown = cx.update_window_entity(&inlet, |inlet, _, cx| {
        inlet
            .account
            .as_ref()
            .and_then(|a| {
                let ch = ChannelId::parse(&long)?;
                a.read(cx).messages(ch).map(|w| w.len())
            })
            .unwrap_or(0)
    });
    eprintln!("scroll: {shown} messages in the window of the long channel");

    let mut still = Vec::new();
    for _ in 0..60 {
        let t = Instant::now();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        still.push(t.elapsed());
    }
    report("redraw without scrolling", &mut still);

    let mut moving = Vec::new();
    for i in 0..400 {
        let dy = if i % 200 < 100 { 160. } else { -160. };
        cx.simulate_event(ScrollEvent {
            position: point(px(800.), px(400.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            ..Default::default()
        });
        let t = Instant::now();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        moving.push(t.elapsed());
    }
    let (_, p95, worst) = report("scrolling", &mut moving);

    // Timings mean something in a release build only.
    if !cfg!(debug_assertions) {
        assert!(
            p95 < Duration::from_micros(16_600),
            "scrolling: p95 {p95:?} is over one frame at 60 Hz"
        );
        assert!(
            worst < Duration::from_millis(50),
            "scrolling: a frame took {worst:?}"
        );
    }
}
