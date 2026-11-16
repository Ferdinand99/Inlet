// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sound effects: the list of sounds, their settings (volume per sound, muting, custom
//! sounds), the rules that keep a burst of events from becoming a burst of noise, and
//! the player.
//!
//! The list, the defaults and the rules follow Fluxer's client: 18 sounds, a master
//! volume from 0 to 200 percent (100 by default) that a volume per sound replaces, one
//! switch per sound and one for all, the same-channel sound off by default, and
//! per-group debouncing and cooldowns (`Scheduler`). The settings are the account's own
//! (`SyncedPreferences.sound`, so they follow the person between Inlet and Fluxer);
//! custom sounds are files on this computer only, as in Fluxer.
//!
//! The sounds that come with Inlet are Inlet's own (`assets/sounds`, CC0, synthesised
//! for Inlet by `make_sounds.py`), never Fluxer's.
//!
//! Threads: the UI thread only changes settings and queues requests; a scheduler thread
//! decides when a queued sound is due, and every sound plays on a thread of its own
//! (at most [`MAX_ACTIVE`] at once, more are dropped) through [`output`].

mod custom;
mod decode;
pub mod output;
mod schedule;
mod settings;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

pub use custom::{max_size_label, CustomEntry, Invalid};
pub use schedule::Scheduler;
pub use settings::{SoundSettings, DEFAULT_MASTER, MAX_PERCENT};

use output::Pcm;
use serde::{Deserialize, Serialize};

/// Sounds that may play at the same time.
pub const MAX_ACTIVE: usize = 4;
/// A change waits this long for more (a slider drag) before it is written to disk and
/// handed to the account.
const QUIET: Duration = Duration::from_millis(700);
/// After the settings were handed to the account, its own echo (and older ones) are not
/// taken as someone else's change for this long.
const ECHO_WAIT: Duration = Duration::from_secs(6);

/// One sound effect. The order is the order of Fluxer's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SoundKind {
    Message,
    DirectMessage,
    SameChannelMessage,
    Mute,
    Unmute,
    Deaf,
    Undeaf,
    UserJoin,
    UserLeave,
    UserMove,
    ViewerJoin,
    ViewerLeave,
    VoiceDisconnect,
    IncomingRing,
    CameraOn,
    CameraOff,
    ScreenShareStart,
    ScreenShareStop,
}

impl SoundKind {
    pub const ALL: [SoundKind; 18] = [
        SoundKind::Message,
        SoundKind::DirectMessage,
        SoundKind::SameChannelMessage,
        SoundKind::Mute,
        SoundKind::Unmute,
        SoundKind::Deaf,
        SoundKind::Undeaf,
        SoundKind::UserJoin,
        SoundKind::UserLeave,
        SoundKind::UserMove,
        SoundKind::ViewerJoin,
        SoundKind::ViewerLeave,
        SoundKind::VoiceDisconnect,
        SoundKind::IncomingRing,
        SoundKind::CameraOn,
        SoundKind::CameraOff,
        SoundKind::ScreenShareStart,
        SoundKind::ScreenShareStop,
    ];

    /// The ID the account's settings use (Fluxer's `SoundType` values).
    pub const fn id(self) -> &'static str {
        match self {
            SoundKind::Message => "message",
            SoundKind::DirectMessage => "direct-message",
            SoundKind::SameChannelMessage => "same-channel-message",
            SoundKind::Mute => "mute",
            SoundKind::Unmute => "unmute",
            SoundKind::Deaf => "deaf",
            SoundKind::Undeaf => "undeaf",
            SoundKind::UserJoin => "user-join",
            SoundKind::UserLeave => "user-leave",
            SoundKind::UserMove => "user-move",
            SoundKind::ViewerJoin => "viewer-join",
            SoundKind::ViewerLeave => "viewer-leave",
            SoundKind::VoiceDisconnect => "voice-disconnect",
            SoundKind::IncomingRing => "incoming-ring",
            SoundKind::CameraOn => "camera-on",
            SoundKind::CameraOff => "camera-off",
            SoundKind::ScreenShareStart => "screen-share-start",
            SoundKind::ScreenShareStop => "screen-share-stop",
        }
    }

    #[allow(dead_code)] // for the settings of a newer client; read in the tests
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// The sounds the list shows first; the others are behind "Show more sound effects".
    pub const fn is_common(self) -> bool {
        matches!(
            self,
            SoundKind::Message
                | SoundKind::DirectMessage
                | SoundKind::SameChannelMessage
                | SoundKind::IncomingRing
                | SoundKind::Mute
                | SoundKind::Unmute
                | SoundKind::Deaf
                | SoundKind::Undeaf
        )
    }

    /// Off until the person turns it on.
    pub const fn default_disabled(self) -> bool {
        matches!(self, SoundKind::SameChannelMessage)
    }

    /// Someone else's coming and going: silent while you are deafened.
    const fn silent_when_deafened(self) -> bool {
        matches!(
            self,
            SoundKind::UserJoin
                | SoundKind::UserLeave
                | SoundKind::UserMove
                | SoundKind::ViewerJoin
                | SoundKind::ViewerLeave
        )
    }

    /// The WAV that comes with Inlet.
    fn wav(self) -> &'static [u8] {
        macro_rules! wav {
            ($name:literal) => {
                include_bytes!(concat!("../../../../../assets/sounds/", $name, ".wav"))
            };
        }
        match self {
            // Fluxer too plays one sound for messages in a community and in a DM.
            SoundKind::Message | SoundKind::DirectMessage => wav!("message"),
            SoundKind::SameChannelMessage => wav!("same-channel-message"),
            SoundKind::Mute => wav!("mute"),
            SoundKind::Unmute => wav!("unmute"),
            SoundKind::Deaf => wav!("deaf"),
            SoundKind::Undeaf => wav!("undeaf"),
            SoundKind::UserJoin => wav!("user-join"),
            SoundKind::UserLeave => wav!("user-leave"),
            SoundKind::UserMove => wav!("user-move"),
            SoundKind::ViewerJoin => wav!("viewer-join"),
            SoundKind::ViewerLeave => wav!("viewer-leave"),
            SoundKind::VoiceDisconnect => wav!("voice-disconnect"),
            SoundKind::IncomingRing => wav!("incoming-ring"),
            SoundKind::CameraOn => wav!("camera-on"),
            SoundKind::CameraOff => wav!("camera-off"),
            SoundKind::ScreenShareStart => wav!("screen-share-start"),
            SoundKind::ScreenShareStop => wav!("screen-share-stop"),
        }
    }
}

/// The sound that comes with Inlet, decoded once.
fn builtin(kind: SoundKind) -> Option<Arc<Pcm>> {
    static CACHE: [OnceLock<Option<Arc<Pcm>>>; 18] = [const { OnceLock::new() }; 18];
    CACHE[kind.index()]
        .get_or_init(|| output::parse_wav(kind.wav()).map(Arc::new))
        .clone()
}

/// What is kept in `settings.json` under `"sounds"`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    settings: SoundSettings,
    /// The custom sounds, by sound ID.
    files: BTreeMap<String, CustomEntry>,
}

const SAVED_KEY: &str = "sounds";

struct Inner {
    settings: SoundSettings,
    files: BTreeMap<String, CustomEntry>,
    /// Decoded custom sounds; filled the first time one plays.
    decoded: HashMap<SoundKind, Arc<Pcm>>,
    sched: Scheduler,
    deafened: bool,
    /// Streamer mode, or anything else that silences every sound.
    blocked: bool,
    loops: HashMap<SoundKind, Arc<AtomicBool>>,
    /// Plays that are running right now, to be stopped by [`SoundHub::stop_all`].
    stops: Vec<Arc<AtomicBool>>,
    /// `settings.json`; `None` in tests and demo runs, which write nothing.
    path: Option<PathBuf>,
    /// A local change not yet written to disk.
    save_at: Option<Instant>,
    /// A change not yet handed to the account.
    sync_at: Option<Instant>,
    /// When the settings were last handed to the account.
    synced: Option<Instant>,
    /// The account's settings as last seen.
    remote: Option<SoundSettings>,
    shutdown: bool,
    #[cfg(test)]
    log: Vec<(SoundKind, f32)>,
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    active: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Stops the scheduler thread when the last hub handle goes.
struct Guard(Arc<Shared>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.lock().shutdown = true;
        self.0.wake.notify_all();
    }
}

/// The sounds: settings, custom files and the player. Cheap to clone; every clone
/// shares one state. A gpui global in the app.
#[derive(Clone)]
pub struct SoundHub {
    shared: Arc<Shared>,
    _guard: Arc<Guard>,
}

impl gpui::Global for SoundHub {}

impl SoundHub {
    /// The hub with the settings kept in `settings.json` at `path` (`None`: nothing is
    /// read or written). Starts the scheduler thread.
    pub fn new(path: Option<PathBuf>) -> Self {
        let saved: Saved = path
            .as_deref()
            .map(|p| crate::backend::platform::prefs::load_section(p, SAVED_KEY))
            .unwrap_or_default();
        let inner = Inner {
            settings: saved.settings,
            files: saved.files,
            decoded: HashMap::new(),
            sched: Scheduler::default(),
            deafened: false,
            blocked: false,
            loops: HashMap::new(),
            stops: Vec::new(),
            path,
            save_at: None,
            sync_at: None,
            synced: None,
            remote: None,
            shutdown: false,
            #[cfg(test)]
            log: Vec::new(),
        };
        let shared = Arc::new(Shared {
            inner: Mutex::new(inner),
            wake: Condvar::new(),
            active: AtomicUsize::new(0),
        });
        let thread = Arc::clone(&shared);
        if let Err(err) = std::thread::Builder::new()
            .name("inlet-sounds".into())
            .spawn(move || scheduler_loop(&thread))
        {
            log::warn!("no sound thread: {err}");
        }
        SoundHub {
            _guard: Arc::new(Guard(Arc::clone(&shared))),
            shared,
        }
    }

    /// The settings as they are now.
    pub fn settings(&self) -> SoundSettings {
        self.shared.lock().settings.clone()
    }

    /// Changes the settings: shown at once, kept on disk and handed to the account
    /// after a short quiet time. A sound the change turns off stops waiting to play.
    pub fn edit(&self, change: impl FnOnce(&mut SoundSettings)) {
        let mut inner = self.shared.lock();
        let before = inner.settings.clone();
        change(&mut inner.settings);
        if inner.settings == before {
            return;
        }
        if inner.settings.all_disabled {
            inner.sched.clear();
        }
        for kind in SoundKind::ALL {
            if !inner.settings.enabled(kind) {
                inner.sched.clear_kind(kind);
            }
        }
        let now = Instant::now();
        inner.save_at = Some(now + QUIET);
        inner.sync_at = Some(now + QUIET);
        drop(inner);
        self.shared.wake.notify_all();
    }

    /// The settings to hand to the account, once the person has stopped changing them.
    /// Call about once a second.
    pub fn take_sync(&self) -> Option<SoundSettings> {
        let mut inner = self.shared.lock();
        let now = Instant::now();
        if inner.sync_at.is_some_and(|at| at <= now) {
            inner.sync_at = None;
            inner.synced = Some(now);
            return Some(inner.settings.clone());
        }
        None
    }

    /// The account's settings (`None`: it has none yet). Taken in when they differ from
    /// what was last seen, unless a change of the person's is on its way to the account.
    pub fn adopt_remote(&self, remote: Option<SoundSettings>) {
        let mut inner = self.shared.lock();
        let Some(remote) = remote else {
            return;
        };
        if inner.remote.as_ref() == Some(&remote) {
            return;
        }
        let waiting = inner.sync_at.is_some()
            || inner
                .synced
                .is_some_and(|t| t.elapsed() < ECHO_WAIT && remote != inner.settings);
        if waiting {
            return;
        }
        inner.remote = Some(remote.clone());
        if inner.settings != remote {
            inner.settings = remote;
            inner.save_at = Some(Instant::now() + QUIET);
        }
        drop(inner);
        self.shared.wake.notify_all();
    }

    /// Another account is shown: its settings are taken in as new.
    pub fn forget_remote(&self) {
        let mut inner = self.shared.lock();
        inner.remote = None;
        inner.synced = None;
    }

    /// The file name of the custom sound, if there is one.
    pub fn custom_name(&self, kind: SoundKind) -> Option<String> {
        self.shared
            .lock()
            .files
            .get(kind.id())
            .map(|e| e.name.clone())
    }

    /// Makes the file at `path` the sound for `kind`. Reads and decodes the file, so
    /// call it off the UI thread.
    pub fn set_custom(&self, kind: SoundKind, path: &Path) -> Result<(), Invalid> {
        let dir = self.sounds_dir();
        let (entry, pcm) = custom::import(dir.as_deref(), kind, path)?;
        let mut inner = self.shared.lock();
        if let Some(old) = inner.files.insert(kind.id().to_owned(), entry.clone()) {
            custom::forget_file(dir.as_deref(), &old, &entry);
        }
        inner.decoded.insert(kind, Arc::new(pcm));
        inner.save_at = Some(Instant::now());
        drop(inner);
        self.shared.wake.notify_all();
        Ok(())
    }

    /// Goes back to the sound that comes with Inlet.
    pub fn remove_custom(&self, kind: SoundKind) {
        let dir = self.sounds_dir();
        let mut inner = self.shared.lock();
        inner.decoded.remove(&kind);
        if let Some(old) = inner.files.remove(kind.id()) {
            custom::delete_file(dir.as_deref(), &old);
            inner.save_at = Some(Instant::now());
        }
        drop(inner);
        self.shared.wake.notify_all();
    }

    fn sounds_dir(&self) -> Option<PathBuf> {
        let inner = self.shared.lock();
        inner
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.join("sounds"))
    }

    /// Whether you are deafened: the sounds of other people coming and going stay
    /// silent then. For the voice tasks.
    #[allow(dead_code)]
    pub fn set_self_deafened(&self, deafened: bool) {
        self.shared.lock().deafened = deafened;
    }

    /// Silences every sound (streamer mode) or lets them play again. For the streamer
    /// mode setting.
    #[allow(dead_code)]
    pub fn set_blocked(&self, blocked: bool) {
        let mut inner = self.shared.lock();
        inner.blocked = blocked;
        if blocked {
            inner.sched.clear();
        }
        drop(inner);
        if blocked {
            self.stop_all();
        }
    }

    /// Plays a sound, if it is on: queued under the debouncing rules.
    pub fn play(&self, kind: SoundKind) {
        self.queue(kind, false);
    }

    /// As [`SoundHub::play`], also while you are deafened (your own join chime). For the
    /// voice tasks.
    #[allow(dead_code)]
    pub fn play_bypassing_deafen(&self, kind: SoundKind) {
        self.queue(kind, true);
    }

    fn queue(&self, kind: SoundKind, bypass: bool) {
        let mut inner = self.shared.lock();
        if !inner.enabled(kind, bypass) {
            return;
        }
        let mult = inner.settings.multiplier(kind);
        inner.sched.queue(kind, mult, bypass, Instant::now());
        drop(inner);
        self.shared.wake.notify_all();
    }

    /// The sound at its volume, now, for the settings list: it plays even when the
    /// sound is off (the list shows it so), but not in streamer mode.
    pub fn preview(&self, kind: SoundKind) {
        self.stop_all();
        let inner = self.shared.lock();
        if inner.blocked {
            return;
        }
        let mult = inner.settings.multiplier(kind);
        drop(inner);
        spawn_play(&self.shared, kind, mult, false);
    }

    /// Plays the sound over and over until [`SoundHub::stop_loop`] (the incoming call).
    /// For the voice tasks.
    #[allow(dead_code)]
    pub fn start_loop(&self, kind: SoundKind) {
        let mut inner = self.shared.lock();
        if inner.loops.contains_key(&kind) || !inner.enabled(kind, true) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        inner.loops.insert(kind, Arc::clone(&stop));
        let shared = Arc::clone(&self.shared);
        drop(inner);
        let spawned = std::thread::Builder::new()
            .name("inlet-sound-loop".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mult = shared.lock().settings.multiplier(kind);
                    let Some(pcm) = pcm_of(&shared, kind) else {
                        break;
                    };
                    let started = Instant::now();
                    if let Err(err) = output::play(&pcm, mult, &stop) {
                        log::info!("the sound did not play: {err}");
                        break;
                    }
                    // A system without a device returns at once: do not spin.
                    if started.elapsed() < Duration::from_millis(100) {
                        std::thread::sleep(pcm.duration().max(Duration::from_millis(100)));
                    }
                }
                shared.lock().loops.remove(&kind);
            });
        if spawned.is_err() {
            self.shared.lock().loops.remove(&kind);
        }
    }

    #[allow(dead_code)]
    pub fn stop_loop(&self, kind: SoundKind) {
        if let Some(stop) = self.shared.lock().loops.remove(&kind) {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Stops everything that plays or waits to play.
    pub fn stop_all(&self) {
        let mut inner = self.shared.lock();
        inner.sched.clear();
        for stop in inner.stops.drain(..) {
            stop.store(true, Ordering::Relaxed);
        }
        for (_, stop) in inner.loops.drain() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// The sounds played so far, for tests.
    #[cfg(test)]
    pub fn played(&self) -> Vec<(SoundKind, f32)> {
        self.shared.lock().log.clone()
    }
}

impl Inner {
    /// Whether a sound may play now: not while streamer mode or "all sounds off" holds,
    /// not when it is off itself, and the others' comings and goings not while deafened.
    fn enabled(&self, kind: SoundKind, bypass_deafened: bool) -> bool {
        !self.blocked
            && self.settings.enabled(kind)
            && !(self.deafened && !bypass_deafened && kind.silent_when_deafened())
    }

    fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let saved = Saved {
            settings: self.settings.clone(),
            files: self.files.clone(),
        };
        if let Err(err) = crate::backend::platform::prefs::save_section(&path, SAVED_KEY, &saved) {
            log::warn!("could not write the sound settings: {err}");
        }
    }
}

/// The samples of a sound: the custom file when there is one (decoded the first time),
/// else the one that comes with Inlet.
fn pcm_of(shared: &Arc<Shared>, kind: SoundKind) -> Option<Arc<Pcm>> {
    let (cached, entry, dir) = {
        let inner = shared.lock();
        (
            inner.decoded.get(&kind).cloned(),
            inner.files.get(kind.id()).cloned(),
            inner
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|d| d.join("sounds")),
        )
    };
    if let Some(pcm) = cached {
        return Some(pcm);
    }
    if let Some(entry) = entry {
        match custom::load(dir.as_deref(), &entry) {
            Ok(pcm) => {
                let pcm = Arc::new(pcm);
                shared.lock().decoded.insert(kind, Arc::clone(&pcm));
                return Some(pcm);
            }
            // A file that is gone or broken: the sound that comes with Inlet plays.
            Err(err) => log::info!("custom sound for {} not used: {err:?}", kind.id()),
        }
    }
    builtin(kind)
}

/// Plays one sound on a thread of its own. `queued` sounds (from the queue, not a
/// preview) stay silent while the system wants quiet.
fn spawn_play(shared: &Arc<Shared>, kind: SoundKind, mult: f32, queued: bool) {
    if shared.active.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE {
        shared.active.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    shared.lock().stops.push(Arc::clone(&stop));
    let worker = Arc::clone(shared);
    let spawned = std::thread::Builder::new()
        .name("inlet-sound".into())
        .spawn(move || {
            // The system wants quiet (do not disturb, presentation, full screen): the
            // queued sounds stay silent; a preview in the settings does not ask.
            let play = cfg!(test) || !queued || !inlet_platform::quiet_mode();
            if play {
                #[cfg(test)]
                worker.lock().log.push((kind, mult));
                match pcm_of(&worker, kind) {
                    Some(pcm) => {
                        if let Err(err) = output::play(&pcm, mult, &stop) {
                            log::info!("the sound did not play: {err}");
                        }
                    }
                    None => log::info!("no sound for {}", kind.id()),
                }
            }
            worker.lock().stops.retain(|s| !Arc::ptr_eq(s, &stop));
            worker.active.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        shared.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The scheduler thread: plays what is due, writes the settings after a quiet time.
fn scheduler_loop(shared: &Arc<Shared>) {
    let mut inner = shared.lock();
    loop {
        if inner.shutdown {
            return;
        }
        let now = Instant::now();
        if inner.save_at.is_some_and(|at| at <= now) {
            inner.save_at = None;
            inner.save();
        }
        if let Some(due) = inner.sched.drain(now) {
            if inner.enabled(due.kind, due.bypass_deafened) {
                inner.sched.mark_played(due.group, now);
                drop(inner);
                spawn_play(shared, due.kind, due.mult, true);
                inner = shared.lock();
            }
            continue;
        }
        let mut wait = inner.sched.next_due(now);
        if let Some(at) = inner.save_at {
            wait = Some(wait.map_or(at, |w| w.min(at)));
        }
        inner = match wait {
            Some(at) => {
                let timeout = at
                    .saturating_duration_since(now)
                    .max(Duration::from_millis(1));
                shared
                    .wake
                    .wait_timeout(inner, timeout)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
            None => shared.wake.wait(inner).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

/// A 16-bit PCM WAV of `frames` frames with `channels` channels, every sample `value`,
/// for tests.
#[cfg(test)]
pub(crate) fn test_wav(rate: u32, channels: u16, frames: usize, value: i16) -> Vec<u8> {
    let data_len = (frames * usize::from(channels) * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for _ in 0..frames * usize::from(channels) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(hub: &SoundHub, count: usize) -> Vec<(SoundKind, f32)> {
        for _ in 0..300 {
            let played = hub.played();
            if played.len() >= count {
                return played;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        hub.played()
    }

    #[test]
    fn every_sound_has_an_id_and_a_sound_that_comes_with_inlet() {
        assert_eq!(SoundKind::ALL.len(), 18);
        for kind in SoundKind::ALL {
            assert_eq!(SoundKind::from_id(kind.id()), Some(kind), "{kind:?}");
            let pcm = builtin(kind).unwrap_or_else(|| panic!("{kind:?} has no sound"));
            assert_eq!(pcm.rate, 48_000);
            let secs = pcm.duration().as_secs_f32();
            assert!((0.15..=2.0).contains(&secs), "{kind:?}: {secs} s");
            // Not clipped, not silent, and it ends in silence (no click).
            let peak = pcm.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.1 && peak < 0.9, "{kind:?}: peak {peak}");
            assert!(pcm.samples.last().unwrap().abs() < 0.01, "{kind:?}");
        }
        assert_eq!(SoundKind::from_id("nonsense"), None);
    }

    #[test]
    fn a_sound_plays_at_the_volume_of_its_setting() {
        let hub = SoundHub::new(None);
        hub.edit(|s| s.set_master(150));
        hub.play(SoundKind::Mute);
        let played = wait_for(&hub, 1);
        assert_eq!(played, vec![(SoundKind::Mute, 1.5)]);
    }

    #[test]
    fn a_sound_that_is_off_does_not_play_and_the_same_channel_sound_starts_off() {
        let hub = SoundHub::new(None);
        hub.play(SoundKind::SameChannelMessage);
        hub.edit(|s| s.set_enabled(SoundKind::Unmute, false));
        hub.play(SoundKind::Unmute);
        hub.edit(|s| s.all_disabled = true);
        hub.play(SoundKind::Message);
        std::thread::sleep(Duration::from_millis(600));
        assert!(hub.played().is_empty(), "{:?}", hub.played());

        let hub = SoundHub::new(None);
        hub.edit(|s| s.set_enabled(SoundKind::SameChannelMessage, true));
        hub.play(SoundKind::SameChannelMessage);
        assert_eq!(wait_for(&hub, 1).len(), 1);
    }

    #[test]
    fn turning_a_sound_off_cancels_the_one_waiting() {
        let hub = SoundHub::new(None);
        hub.play(SoundKind::Message);
        hub.edit(|s| s.set_enabled(SoundKind::Message, false));
        std::thread::sleep(Duration::from_millis(600));
        assert!(hub.played().is_empty());
    }

    #[test]
    fn a_deafened_person_does_not_hear_others_come_and_go() {
        let hub = SoundHub::new(None);
        hub.set_self_deafened(true);
        hub.play(SoundKind::UserJoin);
        hub.play(SoundKind::ViewerLeave);
        std::thread::sleep(Duration::from_millis(600));
        assert!(hub.played().is_empty());
        // Their own join chime and the voice controls still sound.
        hub.play_bypassing_deafen(SoundKind::UserJoin);
        assert_eq!(wait_for(&hub, 1)[0].0, SoundKind::UserJoin);
        hub.play(SoundKind::Deaf);
        assert_eq!(wait_for(&hub, 2).len(), 2);
    }

    #[test]
    fn streamer_mode_silences_everything_including_previews() {
        let hub = SoundHub::new(None);
        hub.set_blocked(true);
        hub.play(SoundKind::Message);
        hub.preview(SoundKind::Message);
        std::thread::sleep(Duration::from_millis(600));
        assert!(hub.played().is_empty());
        hub.set_blocked(false);
        hub.preview(SoundKind::Message);
        assert_eq!(wait_for(&hub, 1).len(), 1);
    }

    #[test]
    fn a_preview_plays_a_sound_that_is_off() {
        let hub = SoundHub::new(None);
        hub.edit(|s| s.set_enabled(SoundKind::Mute, false));
        hub.edit(|s| s.set_override(SoundKind::Mute, 50));
        hub.preview(SoundKind::Mute);
        assert_eq!(wait_for(&hub, 1), vec![(SoundKind::Mute, 0.5)]);
    }

    #[test]
    fn changes_wait_for_quiet_then_go_to_the_account_once() {
        let hub = SoundHub::new(None);
        hub.edit(|s| s.set_master(120));
        hub.edit(|s| s.set_master(130));
        hub.edit(|s| s.set_master(140));
        assert_eq!(hub.take_sync(), None, "still in the quiet time");
        std::thread::sleep(QUIET + Duration::from_millis(100));
        let sent = hub.take_sync().expect("the newest settings go out");
        assert_eq!(sent.master, 140);
        assert_eq!(hub.take_sync(), None, "and only once");
        // The same value again changes nothing.
        hub.edit(|s| s.set_master(140));
        assert_eq!(hub.take_sync(), None);
    }

    #[test]
    fn the_accounts_settings_are_taken_in_but_never_over_a_change_on_its_way() {
        let hub = SoundHub::new(None);
        let mut theirs = SoundSettings::default();
        theirs.set_master(60);
        hub.adopt_remote(Some(theirs.clone()));
        assert_eq!(hub.settings().master, 60);

        // A change of the person's is waiting: the account's older word does not
        // override it.
        hub.edit(|s| s.set_master(90));
        let mut older = SoundSettings::default();
        older.set_master(70);
        hub.adopt_remote(Some(older));
        assert_eq!(hub.settings().master, 90);

        // After it was handed over, its echo changes nothing, and neither does an
        // older snapshot that arrives right after.
        std::thread::sleep(QUIET + Duration::from_millis(100));
        let sent = hub.take_sync().unwrap();
        hub.adopt_remote(Some(sent));
        assert_eq!(hub.settings().master, 90);
        hub.adopt_remote(Some(theirs));
        assert_eq!(hub.settings().master, 90);

        // Another account: its settings are new.
        hub.forget_remote();
        let mut other = SoundSettings::default();
        other.set_master(30);
        hub.adopt_remote(Some(other));
        assert_eq!(hub.settings().master, 30);
        // An account without settings leaves what is there.
        hub.adopt_remote(None);
        assert_eq!(hub.settings().master, 30);
    }

    #[test]
    fn the_settings_and_the_custom_file_names_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("inlet-sounds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        {
            let hub = SoundHub::new(Some(path.clone()));
            hub.edit(|s| {
                s.set_master(80);
                s.set_enabled(SoundKind::Deaf, false);
                s.set_override(SoundKind::Mute, 10);
            });
            // The write comes after the quiet time.
            for _ in 0..200 {
                if path.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let hub = SoundHub::new(Some(path));
        let s = hub.settings();
        assert_eq!(s.master, 80);
        assert!(!s.enabled(SoundKind::Deaf));
        assert_eq!(s.override_of(SoundKind::Mute), Some(10));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
