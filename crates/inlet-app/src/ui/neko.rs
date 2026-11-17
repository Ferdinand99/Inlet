// SPDX-License-Identifier: AGPL-3.0-or-later
//! Neko: a cat that follows the pointer round the window, as in Fluxer's client
//! (Accessibility: "Show Neko", "Keep Neko still"). It chases the pointer, sits down and
//! scratches when the pointer rests, can be petted with a click (a heart floats up), and
//! can be dragged and flung: it bounces off the edges of the window and sulks a little.
//!
//! The behaviour is a port of Fluxer's `NekoSprite` (the walking, the idle animations
//! and their timing, the fling and bounce physics), which in turn follows oneko.js. The
//! sprite sheet is oneko.js's (MIT, `assets/neko/LICENSE`).
// Deviation: Fluxer's sheet has twelve more drawings (looking up and down, being
// squashed against a wall, sulking). Inlet uses the original eight columns; a cat that
// is squashed or sulks sits with its tired face instead, and one that is flung looks
// startled.
//!
//! [`Neko`] is the whole behaviour and knows nothing of gpui; this file draws it and
//! feeds it the pointer.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, img, prelude::*, px, App, Context, CursorStyle, DispatchPhase, Global, Image,
    ImageFormat, MouseButton, MouseMoveEvent, MouseUpEvent,
};

use crate::app::Inlet;
use crate::desktop::Desktop;
use crate::theme;
use crate::ui::titlebar;

/// The sprite sheet: eight columns and four rows of 32 px drawings.
const SHEET: &[u8] = include_bytes!("../../../../assets/neko/neko.png");
const SHEET_SIZE: (f32, f32) = (256., 128.);
const CELL: f32 = 32.;
const HALF: f32 = 16.;
const SPEED: f32 = 10.;
/// The time between two drawings.
const FRAME_MS: f64 = 100.;
const IDLE_DISTANCE: f32 = 48.;
/// The pointer must move this far before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 6.;
const FRICTION: f32 = 0.9;
const STOP_SPEED: f32 = 0.02;
const BOUNCE_FRICTION: f32 = 0.85;
const MIN_FLING_SPEED: f32 = 0.05;
const MAX_FLING_SPEED: f32 = 5.;
const FLING_SAMPLE_WINDOW_MS: f64 = 100.;
const FLING_STALE_MS: f64 = 80.;
const MAX_FRAME_DT: f32 = 32.;
const SULK_CHANCE: f32 = 0.4;
const HEAVY_IMPACT: f32 = 1.5;
const SQUISH_SMALL_MS: f64 = 200.;
const SQUISH_LARGE_MS: f64 = 350.;
const HEART_MS: f64 = 1000.;

/// The drawings. Each has one or more frames, as (column, row) of the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    Idle,
    Alert,
    Tired,
    Sleeping,
    ScratchSelf,
    ScratchWallN,
    ScratchWallS,
    ScratchWallE,
    ScratchWallW,
    Sulk,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Set {
    fn cells(self) -> &'static [(u32, u32)] {
        match self {
            Set::Idle => &[(3, 3)],
            Set::Alert => &[(7, 3)],
            Set::Tired => &[(3, 2)],
            Set::Sleeping => &[(2, 0), (2, 1)],
            Set::ScratchSelf => &[(5, 0), (6, 0), (7, 0)],
            Set::ScratchWallN => &[(0, 0), (0, 1)],
            Set::ScratchWallS => &[(7, 1), (6, 2)],
            Set::ScratchWallE => &[(2, 2), (2, 3)],
            Set::ScratchWallW => &[(4, 0), (4, 1)],
            Set::Sulk => &[(3, 2), (3, 3)],
            Set::N => &[(1, 2), (1, 3)],
            Set::NE => &[(0, 2), (0, 3)],
            Set::E => &[(3, 0), (3, 1)],
            Set::SE => &[(5, 1), (5, 2)],
            Set::S => &[(6, 3), (7, 2)],
            Set::SW => &[(5, 3), (6, 1)],
            Set::W => &[(4, 2), (4, 3)],
            Set::NW => &[(1, 0), (1, 1)],
        }
    }

    /// The drawing's `frame`th picture (frames wrap round).
    pub fn cell(self, frame: usize) -> (u32, u32) {
        let cells = self.cells();
        cells[frame % cells.len()]
    }
}

/// The surroundings: the size of the window and what the cat may do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Env {
    pub width: f32,
    pub height: f32,
    /// The height of the title bar: the cat stays below it.
    pub top: f32,
    /// "Keep Neko still": it does not chase the pointer.
    pub still: bool,
}

impl Env {
    fn min_x(&self) -> f32 {
        HALF
    }
    fn max_x(&self) -> f32 {
        (self.width - HALF).max(HALF)
    }
    fn min_y(&self) -> f32 {
        self.top + HALF
    }
    fn max_y(&self) -> f32 {
        (self.height - HALF).max(self.min_y())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Idle {
    Sleeping,
    ScratchSelf,
    ScratchWall(Set),
}

#[derive(Clone, Copy, Debug)]
struct Queued {
    set: Set,
    hold_frame: Option<usize>,
    ticks: u32,
    frame: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Smash {
    heavy: bool,
    at: f64,
}

/// The cat.
#[derive(Clone, Debug)]
pub struct Neko {
    /// The middle of the cat, in window coordinates.
    pub x: f32,
    pub y: f32,
    mouse: (f32, f32),
    frame_count: u32,
    idle_time: u32,
    idle: Option<Idle>,
    idle_frame: u32,
    queue: VecDeque<Queued>,
    last_tick: Option<f64>,
    last_frame: f64,
    /// The pointer is down on the cat; where.
    pressed: Option<(f32, f32)>,
    pub dragging: bool,
    samples: VecDeque<(f32, f32, f64)>,
    vel: (f32, f32),
    flinging: bool,
    did_fling: bool,
    did_bounce: bool,
    smash: Option<Smash>,
    /// What is drawn: a drawing and which of its pictures.
    pub sprite: (Set, usize),
    /// When the cat was last petted.
    pub heart_since: Option<f64>,
    rng: u64,
    placed: bool,
}

impl Neko {
    pub fn new(seed: u64) -> Self {
        Neko {
            x: 0.,
            y: 0.,
            mouse: (0., 0.),
            frame_count: 0,
            idle_time: 0,
            idle: None,
            idle_frame: 0,
            queue: VecDeque::new(),
            last_tick: None,
            last_frame: 0.,
            pressed: None,
            dragging: false,
            samples: VecDeque::new(),
            vel: (0., 0.),
            flinging: false,
            did_fling: false,
            did_bounce: false,
            smash: None,
            sprite: (Set::Idle, 0),
            heart_since: None,
            rng: seed | 1,
            placed: false,
        }
    }

    fn random(&mut self) -> f32 {
        // xorshift64*: enough for a cat's whims.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (r >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Whether the cat moves or is being handled, so a frame is wanted soon.
    pub fn is_busy(&self) -> bool {
        self.dragging || self.flinging || self.pressed.is_some()
    }

    /// The pointer moved to `(x, y)`: the cat looks for it, or is dragged along.
    pub fn pointer_moved(&mut self, x: f32, y: f32, now: f64, env: &Env) {
        if let Some((px, py)) = self.pressed {
            if !self.dragging {
                if (x - px).hypot(y - py) <= DRAG_THRESHOLD {
                    return;
                }
                self.dragging = true;
                self.idle = None;
                self.idle_frame = 0;
                self.idle_time = 0;
            }
            self.x = x.clamp(env.min_x(), env.max_x());
            self.y = y.clamp(env.min_y(), env.max_y());
            self.mouse = (self.x, self.y);
            self.samples.push_back((x, y, now));
            while self.samples.len() > 5 {
                self.samples.pop_front();
            }
            return;
        }
        self.mouse = (x, y);
    }

    /// The pointer went down on the cat.
    pub fn press(&mut self, x: f32, y: f32) {
        self.flinging = false;
        self.did_fling = false;
        self.vel = (0., 0.);
        self.samples.clear();
        self.queue.clear();
        self.smash = None;
        self.did_bounce = false;
        self.pressed = Some((x, y));
        self.dragging = false;
    }

    /// The pointer went up. A press that never became a drag is a pat: the cat looks up
    /// and a heart floats. A drag ends in a fling when the pointer was moving. Returns
    /// whether it was a pat.
    pub fn release(&mut self, now: f64, still_motion: bool) -> bool {
        if self.pressed.take().is_none() {
            return false;
        }
        let was_dragging = self.dragging;
        self.dragging = false;
        let pat = !was_dragging;
        if pat {
            self.idle_time = 0;
            self.idle = None;
            self.idle_frame = 0;
            self.sprite = (Set::Alert, 0);
            self.heart_since = Some(now);
        }
        let fling = if was_dragging {
            self.fling_velocity(now)
        } else {
            (0., 0., 0.)
        };
        let stale = self
            .samples
            .back()
            .is_none_or(|s| now - s.2 > FLING_STALE_MS);
        if stale || !still_motion {
            self.vel = (0., 0.);
            self.flinging = false;
        } else {
            self.vel = (fling.0, fling.1);
            self.flinging = fling.2 > MIN_FLING_SPEED;
        }
        self.smash = None;
        self.did_bounce = false;
        self.samples.clear();
        pat
    }

    /// The speed of the last moments of a drag, in pixels per millisecond.
    fn fling_velocity(&self, now: f64) -> (f32, f32, f32) {
        let recent: Vec<_> = self
            .samples
            .iter()
            .filter(|s| now - s.2 <= FLING_SAMPLE_WINDOW_MS)
            .collect();
        if recent.len() < 2 {
            return (0., 0., 0.);
        }
        let (first, last) = (recent[0], recent[recent.len() - 1]);
        let dt = (last.2 - first.2) as f32;
        if dt <= 0. {
            return (0., 0., 0.);
        }
        let (mut vx, mut vy) = ((last.0 - first.0) / dt, (last.1 - first.1) / dt);
        let scalar = vx.hypot(vy);
        if scalar > MAX_FLING_SPEED {
            let k = MAX_FLING_SPEED / scalar;
            vx *= k;
            vy *= k;
            return (vx, vy, MAX_FLING_SPEED);
        }
        (vx, vy, scalar)
    }

    fn set_sprite(&mut self, set: Set, frame: usize) {
        self.sprite = (set, frame % set.cells().len());
    }

    /// Moves the cat on. `now` is in milliseconds. Returns whether anything to draw
    /// changed.
    pub fn tick(&mut self, now: f64, env: &Env) -> bool {
        let before = (self.x, self.y, self.sprite, self.heart_since.is_some());
        if !self.placed {
            // The first time: the lower right corner, as Fluxer.
            self.x = (env.width - 48.).clamp(env.min_x(), env.max_x());
            self.y = (env.height - 96.).clamp(env.min_y(), env.max_y());
            self.mouse = (self.x, self.y);
            self.placed = true;
        }
        self.x = self.x.clamp(env.min_x(), env.max_x());
        self.y = self.y.clamp(env.min_y(), env.max_y());
        let dt = self.last_tick.map_or(16.67, |t| (now - t) as f32);
        self.last_tick = Some(now);
        if self.flinging {
            self.momentum(now, dt, env);
        }
        if now - self.last_frame >= FRAME_MS {
            self.last_frame = now;
            if !self.play_queued() {
                if self.dragging {
                    self.set_sprite(Set::Alert, 0);
                } else if self.flinging {
                    if self.smash.is_none() {
                        self.flying_sprite();
                    }
                } else if self.did_fling {
                    self.idle(env);
                } else {
                    self.step(env);
                }
            }
        }
        if self.heart_since.is_some_and(|t| now - t >= HEART_MS) {
            self.heart_since = None;
        }
        before != (self.x, self.y, self.sprite, self.heart_since.is_some())
    }

    fn play_queued(&mut self) -> bool {
        let Some(current) = self.queue.front_mut() else {
            return false;
        };
        let (set, frame) = (current.set, current.hold_frame.unwrap_or(current.frame));
        current.frame += 1;
        current.ticks = current.ticks.saturating_sub(1);
        let done = current.ticks == 0;
        if done {
            self.queue.pop_front();
        }
        self.set_sprite(set, frame);
        true
    }

    fn flying_sprite(&mut self) {
        // The cat looks startled while it flies.
        self.set_sprite(Set::Alert, 0);
    }

    /// The idle cat: now and then it dozes off or scratches (the wall, if it is next to
    /// one).
    fn idle(&mut self, env: &Env) {
        self.idle_time += 1;
        if self.idle_time > 10 && self.random() < 0.005 && self.idle.is_none() {
            let mut available = vec![Idle::Sleeping, Idle::ScratchSelf];
            if self.x < CELL {
                available.push(Idle::ScratchWall(Set::ScratchWallW));
            }
            if self.y < env.top + CELL {
                available.push(Idle::ScratchWall(Set::ScratchWallN));
            }
            if self.x > env.width - CELL {
                available.push(Idle::ScratchWall(Set::ScratchWallE));
            }
            if self.y > env.height - CELL {
                available.push(Idle::ScratchWall(Set::ScratchWallS));
            }
            let pick = (self.random() * available.len() as f32) as usize;
            self.idle = Some(available[pick.min(available.len() - 1)]);
        }
        match self.idle {
            Some(Idle::Sleeping) => {
                if self.idle_frame < 8 {
                    self.set_sprite(Set::Tired, 0);
                } else {
                    self.set_sprite(Set::Sleeping, (self.idle_frame / 4) as usize);
                    if self.idle_frame > 192 {
                        self.reset_idle();
                        self.idle_frame += 1;
                        return;
                    }
                }
            }
            Some(Idle::ScratchSelf) => {
                self.set_sprite(Set::ScratchSelf, self.idle_frame as usize);
                if self.idle_frame > 9 {
                    self.reset_idle();
                    self.idle_frame += 1;
                    return;
                }
            }
            Some(Idle::ScratchWall(set)) => {
                self.set_sprite(set, self.idle_frame as usize);
                if self.idle_frame > 9 {
                    self.reset_idle();
                    self.idle_frame += 1;
                    return;
                }
            }
            None => {
                self.set_sprite(Set::Idle, 0);
                return;
            }
        }
        self.idle_frame += 1;
    }

    fn reset_idle(&mut self) {
        self.idle = None;
        self.idle_frame = 0;
    }

    /// One walking step towards the pointer (or the idle cat when it is near, or the
    /// person asked for stillness).
    fn step(&mut self, env: &Env) {
        if env.still {
            self.idle(env);
            return;
        }
        self.frame_count += 1;
        let (dx, dy) = (self.x - self.mouse.0, self.y - self.mouse.1);
        let distance = dx.hypot(dy);
        if distance < SPEED || distance < IDLE_DISTANCE {
            self.idle(env);
            return;
        }
        self.reset_idle();
        if self.idle_time > 1 {
            self.set_sprite(Set::Alert, 0);
            self.idle_time = self.idle_time.min(7) - 1;
            return;
        }
        let vertical = if dy / distance > 0.5 {
            Some('N')
        } else if dy / distance < -0.5 {
            Some('S')
        } else {
            None
        };
        let horizontal = if dx / distance > 0.5 {
            Some('W')
        } else if dx / distance < -0.5 {
            Some('E')
        } else {
            None
        };
        let set = match (vertical, horizontal) {
            (Some('N'), Some('W')) => Set::NW,
            (Some('N'), Some('E')) => Set::NE,
            (Some('S'), Some('W')) => Set::SW,
            (Some('S'), Some('E')) => Set::SE,
            (Some('N'), _) => Set::N,
            (Some('S'), _) => Set::S,
            (_, Some('W')) => Set::W,
            (_, Some('E')) => Set::E,
            _ => Set::Idle,
        };
        self.set_sprite(set, self.frame_count as usize);
        self.x -= dx / distance * SPEED;
        self.y -= dy / distance * SPEED;
        self.x = self.x.clamp(env.min_x(), env.max_x());
        self.y = self.y.clamp(env.min_y(), env.max_y());
    }

    /// A flung cat: slows down, bounces off the edges and, when it has stopped, may sulk.
    fn momentum(&mut self, now: f64, dt: f32, env: &Env) {
        let frame_dt = dt.min(MAX_FRAME_DT);
        let decay = FRICTION.powf(frame_dt / 16.67);
        self.vel.0 *= decay;
        self.vel.1 *= decay;
        let speed = self.vel.0.hypot(self.vel.1);
        let mut bounced = false;
        let mut surface_wall = false;
        let mut next = self.x + self.vel.0 * frame_dt;
        if next < env.min_x() {
            next = (env.min_x() + (env.min_x() - next) * BOUNCE_FRICTION)
                .clamp(env.min_x(), env.max_x());
            self.vel.0 = -self.vel.0 * BOUNCE_FRICTION;
            bounced = true;
            surface_wall = true;
        } else if next > env.max_x() {
            next = (env.max_x() - (next - env.max_x()) * BOUNCE_FRICTION)
                .clamp(env.min_x(), env.max_x());
            self.vel.0 = -self.vel.0 * BOUNCE_FRICTION;
            bounced = true;
            surface_wall = true;
        }
        self.x = next;
        let mut next = self.y + self.vel.1 * frame_dt;
        if next < env.min_y() {
            next = (env.min_y() + (env.min_y() - next) * BOUNCE_FRICTION)
                .clamp(env.min_y(), env.max_y());
            self.vel.1 = -self.vel.1 * BOUNCE_FRICTION;
            bounced = true;
        } else if next > env.max_y() {
            next = (env.max_y() - (next - env.max_y()) * BOUNCE_FRICTION)
                .clamp(env.min_y(), env.max_y());
            self.vel.1 = -self.vel.1 * BOUNCE_FRICTION;
            bounced = true;
        }
        self.y = next;
        let _ = surface_wall;
        if bounced {
            self.did_bounce = true;
            self.smash = Some(Smash {
                heavy: speed >= HEAVY_IMPACT,
                at: now,
            });
        }
        if self.vel.0.hypot(self.vel.1) < STOP_SPEED {
            self.flinging = false;
            self.vel = (0., 0.);
            self.did_fling = true;
            self.smash = None;
            if self.did_bounce && self.random() < SULK_CHANCE {
                let hold = 10 + (self.random() * 22.) as u32;
                self.queue.push_back(Queued {
                    set: Set::Sulk,
                    hold_frame: Some(0),
                    ticks: 4,
                    frame: 0,
                });
                self.queue.push_back(Queued {
                    set: Set::Sulk,
                    hold_frame: Some(1),
                    ticks: hold,
                    frame: 0,
                });
            }
            return;
        }
        if !self.draw_smash(now) {
            self.flying_sprite();
        }
    }

    /// Squashed against an edge for a moment: the tired face stands in for the squash.
    fn draw_smash(&mut self, now: f64) -> bool {
        let Some(smash) = self.smash else {
            return false;
        };
        let hold = if smash.heavy {
            SQUISH_LARGE_MS
        } else {
            SQUISH_SMALL_MS
        };
        if now - smash.at >= hold {
            self.smash = None;
            return false;
        }
        self.set_sprite(Set::Tired, 0);
        true
    }
}

/// What the overlay keeps: the cat, the size of the window it saw last, and the sheet.
pub struct NekoState {
    pub cat: Neko,
    env: Env,
    started: Instant,
    sheet: Arc<Image>,
}

impl Global for NekoState {}

impl NekoState {
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.
    }
}

/// Makes the cat and starts the clock that moves it; call once at start-up.
pub fn init(cx: &mut App) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9E37_79B9, |d| d.as_nanos() as u64);
    cx.set_global(NekoState {
        cat: Neko::new(seed),
        env: Env {
            width: 1200.,
            height: 780.,
            top: 0.,
            still: false,
        },
        started: Instant::now(),
        sheet: Arc::new(Image::from_bytes(ImageFormat::Png, SHEET.to_vec())),
    });
    cx.spawn(async move |cx| loop {
        let wait = cx.update(tick);
        cx.background_executor().timer(wait).await;
    })
    .detach();
}

/// Whether the cat is switched on, and whether it must sit still.
fn settings(cx: &App) -> (bool, bool, f32) {
    cx.try_global::<Desktop>().map_or((false, false, 0.), |d| {
        (
            d.prefs.show_neko,
            d.prefs.keep_neko_still,
            if d.custom_titlebar {
                titlebar::HEIGHT
            } else {
                0.
            },
        )
    })
}

/// One step of the clock: moves the cat and asks for a frame when something changed.
/// Returns how long to wait for the next.
fn tick(cx: &mut App) -> Duration {
    let (on, still, top) = settings(cx);
    if !on || !cx.has_global::<NekoState>() {
        return Duration::from_millis(250);
    }
    let state = cx.global_mut::<NekoState>();
    state.env.still = still;
    state.env.top = top;
    let now = state.now();
    let env = state.env;
    let changed = state.cat.tick(now, &env);
    let wait = if state.cat.is_busy() { 16 } else { 50 };
    if changed {
        cx.refresh_windows();
    }
    Duration::from_millis(wait)
}

impl Inlet {
    /// The cat over the window, when it is switched on.
    pub(crate) fn render_neko(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (on, _, _) = settings(cx);
        if !on || !cx.has_global::<NekoState>() {
            return None;
        }
        let state = cx.global::<NekoState>();
        let now = state.now();
        let (x, y) = (state.cat.x, state.cat.y);
        let (set, frame) = state.cat.sprite;
        let (col, row) = set.cell(frame);
        let sheet = Arc::clone(&state.sheet);
        let dragging = state.cat.dragging;
        let heart = state
            .cat
            .heart_since
            .map(|t| ((now - t) / HEART_MS).clamp(0., 1.) as f32);

        // Lives in the paint phase of this element: the size of the window, and the
        // pointer wherever it goes.
        let watcher = canvas(
            |bounds, _, cx| {
                if cx.has_global::<NekoState>() {
                    let s = cx.global_mut::<NekoState>();
                    s.env.width = bounds.size.width.as_f32();
                    s.env.height = bounds.size.height.as_f32();
                }
            },
            |_, (), window, _| {
                window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !cx.has_global::<NekoState>() {
                        return;
                    }
                    let s = cx.global_mut::<NekoState>();
                    let (now, env) = (s.now(), s.env);
                    s.cat.pointer_moved(
                        event.position.x.as_f32(),
                        event.position.y.as_f32(),
                        now,
                        &env,
                    );
                    if s.cat.dragging {
                        window.refresh();
                    }
                });
                window.on_mouse_event(|_: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !cx.has_global::<NekoState>() {
                        return;
                    }
                    let s = cx.global_mut::<NekoState>();
                    let now = s.now();
                    if s.cat.release(now, true) || s.cat.is_busy() {
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let sprite = div()
            .id("neko")
            .absolute()
            .left(px(x - HALF))
            .top(px(y - HALF))
            .size(px(CELL))
            .overflow_hidden()
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .on_mouse_down(MouseButton::Left, |event, window, cx| {
                if cx.has_global::<NekoState>() {
                    cx.global_mut::<NekoState>()
                        .cat
                        .press(event.position.x.as_f32(), event.position.y.as_f32());
                    cx.stop_propagation();
                    window.refresh();
                }
            })
            .child(
                img(sheet)
                    .absolute()
                    .left(px(-(col as f32) * CELL))
                    .top(px(-(row as f32) * CELL))
                    .w(px(SHEET_SIZE.0))
                    .h(px(SHEET_SIZE.1)),
            );

        let mut layer = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(watcher)
            .child(sprite);
        if let Some(t) = heart {
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(x - 4.))
                    .top(px(y - HALF - 4. - 24. * t))
                    .text_size(px(12.))
                    .text_color(theme::danger())
                    .opacity(1. - t)
                    .child("\u{2665}"),
            );
        }
        Some(layer.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            width: 800.,
            height: 600.,
            top: 0.,
            still: false,
        }
    }

    /// Runs the clock for `ms` milliseconds in steps of 16 ms.
    fn run(cat: &mut Neko, from: f64, ms: f64, env: &Env) -> f64 {
        let mut t = from;
        while t < from + ms {
            cat.tick(t, env);
            t += 16.;
        }
        t
    }

    #[test]
    fn the_cat_starts_in_the_lower_right_corner_and_sits() {
        let mut cat = Neko::new(1);
        cat.tick(0., &env());
        assert_eq!((cat.x, cat.y), (752., 504.));
        assert_eq!(cat.sprite.0, Set::Idle);
    }

    #[test]
    fn the_cat_chases_the_pointer_and_stops_near_it() {
        let mut cat = Neko::new(2);
        let e = env();
        cat.tick(0., &e);
        cat.pointer_moved(100., 100., 0., &e);
        let t = run(&mut cat, 16., 15_000., &e);
        let distance = (cat.x - 100.).hypot(cat.y - 100.);
        assert!((10.0..=IDLE_DISTANCE).contains(&distance), "{distance}");
        // It walked: the drawing while it walked was a direction, not the idle cat.
        let mut seen_walking = false;
        let mut cat = Neko::new(3);
        cat.tick(0., &e);
        cat.pointer_moved(100., 100., 0., &e);
        let mut t2 = 16.;
        while t2 < 600. {
            cat.tick(t2, &e);
            seen_walking |= matches!(
                cat.sprite.0,
                Set::NW | Set::W | Set::N | Set::SW | Set::S | Set::Alert
            );
            t2 += 16.;
        }
        assert!(seen_walking);
        assert!(t > 0.);
    }

    #[test]
    fn a_still_cat_does_not_chase() {
        let mut cat = Neko::new(4);
        let e = Env {
            still: true,
            ..env()
        };
        cat.tick(0., &e);
        let start = (cat.x, cat.y);
        cat.pointer_moved(10., 10., 0., &e);
        run(&mut cat, 16., 3000., &e);
        assert_eq!((cat.x, cat.y), start);
    }

    #[test]
    fn a_click_pats_the_cat_and_a_heart_floats_then_goes() {
        let mut cat = Neko::new(5);
        let e = env();
        cat.tick(0., &e);
        cat.press(cat.x, cat.y);
        assert!(cat.is_busy());
        assert!(cat.release(500., true), "it was a pat");
        assert_eq!(cat.sprite.0, Set::Alert);
        assert_eq!(cat.heart_since, Some(500.));
        cat.tick(1600., &e);
        assert_eq!(cat.heart_since, None);
        assert!(!cat.is_busy());
    }

    #[test]
    fn a_dragged_cat_follows_the_pointer_and_stays_inside_the_window() {
        let mut cat = Neko::new(6);
        let e = env();
        cat.tick(0., &e);
        cat.press(cat.x, cat.y);
        // Moving less than the threshold is still a press.
        cat.pointer_moved(cat.x + 3., cat.y, 10., &e);
        assert!(!cat.dragging);
        cat.pointer_moved(400., 300., 20., &e);
        assert!(cat.dragging);
        assert_eq!((cat.x, cat.y), (400., 300.));
        // Dragged out of the window: kept inside.
        cat.pointer_moved(-500., 9000., 30., &e);
        assert_eq!((cat.x, cat.y), (HALF, 600. - HALF));
    }

    #[test]
    fn a_flung_cat_bounces_and_comes_to_rest_inside_the_window() {
        let mut cat = Neko::new(7);
        let e = env();
        cat.tick(0., &e);
        cat.press(400., 300.);
        for (i, x) in [(1u32, 410.), (2, 450.), (3, 520.), (4, 640.)] {
            cat.pointer_moved(x, 300., f64::from(i) * 16., &e);
        }
        assert!(cat.dragging);
        // Let go while moving fast: it flies.
        assert!(!cat.release(72., true));
        assert!(cat.flinging, "a fast drag becomes a fling");
        let mut t = 80.;
        let mut left_the_window = false;
        while t < 20_000. && (cat.flinging || t < 200.) {
            cat.tick(t, &e);
            left_the_window |=
                cat.x < HALF || cat.x > e.width - HALF || cat.y < HALF || cat.y > e.height - HALF;
            t += 16.;
        }
        assert!(!left_the_window, "it never leaves the window");
        assert!(cat.did_bounce, "it reached the edge and bounced");
        assert!(!cat.flinging, "and came to rest");
    }

    #[test]
    fn a_slow_release_is_no_fling() {
        let mut cat = Neko::new(8);
        let e = env();
        cat.tick(0., &e);
        cat.press(400., 300.);
        cat.pointer_moved(420., 300., 0., &e);
        // The pointer rested for half a second before the release.
        assert!(!cat.release(500., true));
        assert!(!cat.flinging);
        // Reduced motion: no fling either.
        let mut cat = Neko::new(9);
        cat.tick(0., &e);
        cat.press(400., 300.);
        for (i, x) in [(1u32, 410.), (2, 450.), (3, 520.)] {
            cat.pointer_moved(x, 300., f64::from(i) * 16., &e);
        }
        cat.release(52., false);
        assert!(!cat.flinging);
    }

    #[test]
    fn the_cat_stays_below_the_title_bar() {
        let mut cat = Neko::new(10);
        let e = Env { top: 32., ..env() };
        cat.tick(0., &e);
        cat.press(cat.x, cat.y);
        cat.pointer_moved(400., 0., 10., &e);
        cat.pointer_moved(400., 1., 20., &e);
        assert!(cat.y >= 32. + HALF - 0.01, "{}", cat.y);
    }

    #[test]
    fn every_drawing_has_pictures_inside_the_sheet() {
        for set in [
            Set::Idle,
            Set::Alert,
            Set::Tired,
            Set::Sleeping,
            Set::ScratchSelf,
            Set::ScratchWallN,
            Set::ScratchWallS,
            Set::ScratchWallE,
            Set::ScratchWallW,
            Set::Sulk,
            Set::N,
            Set::NE,
            Set::E,
            Set::SE,
            Set::S,
            Set::SW,
            Set::W,
            Set::NW,
        ] {
            for frame in 0..4 {
                let (col, row) = set.cell(frame);
                assert!(
                    (col + 1) as f32 * CELL <= SHEET_SIZE.0
                        && (row + 1) as f32 * CELL <= SHEET_SIZE.1,
                    "{set:?} {frame}"
                );
            }
        }
    }

    #[test]
    fn a_resting_cat_dozes_off_or_scratches_in_time() {
        let mut cat = Neko::new(11);
        let e = env();
        cat.tick(0., &e);
        cat.pointer_moved(cat.x, cat.y, 0., &e);
        let mut seen = std::collections::BTreeSet::new();
        let mut t = 16.;
        while t < 600_000. && seen.len() < 2 {
            cat.tick(t, &e);
            seen.insert(format!("{:?}", cat.sprite.0));
            t += 16.;
        }
        assert!(seen.len() >= 2, "{seen:?}");
    }
}
