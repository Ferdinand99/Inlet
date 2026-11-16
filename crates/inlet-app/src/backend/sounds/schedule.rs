// SPDX-License-Identifier: AGPL-3.0-or-later
//! When a queued sound plays. A burst of events (five people join a channel, a busy
//! channel full of messages) must not become a burst of noise, so, as in Fluxer, sounds
//! are grouped: a sound waits a moment for others of its group, the newest of the
//! highest priority plays at the loudest volume asked for, a group cools down after a
//! sound, and no two sounds start closer than [`GLOBAL_GAP`]. Pure: the caller gives the
//! time.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::SoundKind;

/// No two queued sounds start closer than this.
pub const GLOBAL_GAP: Duration = Duration::from_millis(260);

/// How a sound is held back.
#[derive(Clone, Copy, Debug)]
struct Policy {
    group: &'static str,
    /// Waits this long for more sounds of the group.
    debounce: Duration,
    /// After a sound of the group, the next waits this long.
    cooldown: Duration,
    /// Never waits longer than this for more.
    max_wait: Duration,
    /// Of the sounds due together, the highest priority plays first.
    priority: u8,
}

const fn policy(
    group: &'static str,
    debounce: u64,
    cooldown: u64,
    max_wait: u64,
    priority: u8,
) -> Policy {
    Policy {
        group,
        debounce: Duration::from_millis(debounce),
        cooldown: Duration::from_millis(cooldown),
        max_wait: Duration::from_millis(max_wait),
        priority,
    }
}

fn policy_of(kind: SoundKind) -> Policy {
    match kind {
        SoundKind::Message | SoundKind::DirectMessage | SoundKind::SameChannelMessage => {
            policy("message-notification", 120, 900, 1000, 20)
        }
        SoundKind::UserJoin | SoundKind::UserLeave | SoundKind::UserMove => {
            policy("voice-presence", 180, 900, 1100, 30)
        }
        SoundKind::ViewerJoin | SoundKind::ViewerLeave => {
            policy("stream-viewer-presence", 180, 900, 1100, 30)
        }
        SoundKind::Mute | SoundKind::Unmute | SoundKind::Deaf | SoundKind::Undeaf => {
            policy("voice-control", 80, 250, 700, 25)
        }
        SoundKind::CameraOn
        | SoundKind::CameraOff
        | SoundKind::ScreenShareStart
        | SoundKind::ScreenShareStop => policy("voice-media", 100, 350, 800, 25),
        SoundKind::VoiceDisconnect => policy("voice-disconnect", 40, 750, 500, 60),
        SoundKind::IncomingRing => policy(SoundKind::IncomingRing.id(), 100, 450, 900, 10),
    }
}

/// A sound that is due.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Due {
    pub kind: SoundKind,
    /// The volume factor (1.0 at 100 percent).
    pub mult: f32,
    pub group: &'static str,
    pub bypass_deafened: bool,
}

#[derive(Clone, Debug)]
struct Pending {
    group: &'static str,
    kind: SoundKind,
    mult: f32,
    bypass_deafened: bool,
    queued: Instant,
    last_requested: Instant,
    play_after: Instant,
}

/// The sounds that wait, and when the last ones played.
#[derive(Debug, Default)]
pub struct Scheduler {
    pending: Vec<Pending>,
    last_played: Option<Instant>,
    last_in_group: HashMap<&'static str, Instant>,
}

impl Scheduler {
    fn play_after(&self, p: Policy, queued: Instant, now: Instant) -> Instant {
        let mut at = (now + p.debounce).min(queued + p.max_wait);
        if let Some(t) = self.last_in_group.get(p.group) {
            at = at.max(*t + p.cooldown);
        }
        if let Some(t) = self.last_played {
            at = at.max(t + GLOBAL_GAP);
        }
        at
    }

    /// A sound is asked for. One waiting in the same group takes the new one's place when
    /// that has at least its priority, and plays at the louder of the two.
    pub fn queue(&mut self, kind: SoundKind, mult: f32, bypass_deafened: bool, now: Instant) {
        let policy = policy_of(kind);
        if let Some(i) = self.pending.iter().position(|p| p.group == policy.group) {
            let existing_priority = policy_of(self.pending[i].kind).priority;
            let (queued, loudest) = (self.pending[i].queued, self.pending[i].mult.max(mult));
            let (new_kind, new_bypass) = if policy.priority >= existing_priority {
                (kind, bypass_deafened)
            } else {
                (self.pending[i].kind, self.pending[i].bypass_deafened)
            };
            let after = self.play_after(policy_of(new_kind), queued, now);
            let p = &mut self.pending[i];
            p.kind = new_kind;
            p.bypass_deafened = new_bypass;
            p.mult = loudest;
            p.last_requested = now;
            p.play_after = after;
            return;
        }
        let play_after = self.play_after(policy, now, now);
        self.pending.push(Pending {
            group: policy.group,
            kind,
            mult,
            bypass_deafened,
            queued: now,
            last_requested: now,
            play_after,
        });
    }

    /// When the next waiting sound is due, with the cooldowns taken into account.
    pub fn next_due(&mut self, _now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        for i in 0..self.pending.len() {
            let (kind, queued, requested) = {
                let p = &self.pending[i];
                (p.kind, p.queued, p.last_requested)
            };
            let after = self.play_after(policy_of(kind), queued, requested);
            let p = &mut self.pending[i];
            p.play_after = p.play_after.max(after);
            next = Some(next.map_or(p.play_after, |n| n.min(p.play_after)));
        }
        next
    }

    /// The sound that is due now, if any: of those due, the one of the highest priority
    /// (the first asked for among equals). It leaves the queue.
    pub fn drain(&mut self, now: Instant) -> Option<Due> {
        // Cooldowns first: a sound that played a moment ago pushes the others back.
        self.next_due(now);
        let mut best: Option<usize> = None;
        for (i, p) in self.pending.iter().enumerate() {
            if p.play_after > now {
                continue;
            }
            if best.is_none_or(|b| {
                policy_of(p.kind).priority > policy_of(self.pending[b].kind).priority
            }) {
                best = Some(i);
            }
        }
        let p = self.pending.remove(best?);
        Some(Due {
            kind: p.kind,
            mult: p.mult,
            group: p.group,
            bypass_deafened: p.bypass_deafened,
        })
    }

    /// The sound of `group` has started playing.
    pub fn mark_played(&mut self, group: &'static str, now: Instant) {
        self.last_played = Some(now);
        self.last_in_group.insert(group, now);
    }

    /// Forgets the waiting sounds of one kind (it was turned off).
    pub fn clear_kind(&mut self, kind: SoundKind) {
        self.pending.retain(|p| p.kind != kind);
    }

    /// Forgets every waiting sound.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(base: Instant, n: u64) -> Instant {
        base + Duration::from_millis(n)
    }

    /// Plays everything due until `until`, returns the kinds in order with their times.
    fn run(s: &mut Scheduler, base: Instant, until: u64) -> Vec<(u64, SoundKind, f32)> {
        let mut out = Vec::new();
        for t in 0..=until {
            let now = ms(base, t);
            while let Some(d) = s.drain(now) {
                s.mark_played(d.group, now);
                out.push((t, d.kind, d.mult));
            }
        }
        out
    }

    #[test]
    fn a_sound_waits_for_its_debounce_then_plays() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        s.queue(SoundKind::Message, 1.0, false, base);
        assert_eq!(s.drain(ms(base, 50)), None);
        let played = run(&mut s, base, 400);
        assert_eq!(played, vec![(120, SoundKind::Message, 1.0)]);
    }

    #[test]
    fn five_people_joining_make_one_sound() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        for n in 0..5 {
            s.queue(SoundKind::UserJoin, 1.0, false, ms(base, n * 20));
        }
        let played = run(&mut s, base, 2000);
        assert_eq!(played.len(), 1, "{played:?}");
        // It waited for the last one: 180 ms after it (at 80 ms).
        assert_eq!(played[0].0, 260);
    }

    #[test]
    fn a_sound_never_waits_longer_than_its_maximum() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        // A message every 100 ms keeps pushing the debounce back; the group's 1 s cap
        // still lets the first sound out.
        for n in 0..30 {
            s.queue(SoundKind::Message, 1.0, false, ms(base, n * 100));
        }
        let first = (0..3200u64).find(|t| s.drain(ms(base, *t)).is_some());
        assert_eq!(first, Some(1000));
    }

    #[test]
    fn the_newest_sound_of_a_group_wins_at_the_loudest_volume() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        s.queue(SoundKind::UserJoin, 0.5, false, base);
        s.queue(SoundKind::UserMove, 1.5, false, ms(base, 10));
        // The same priority: the newer kind replaces the older.
        s.queue(SoundKind::UserLeave, 1.0, false, ms(base, 20));
        let played = run(&mut s, base, 1000);
        assert_eq!(played, vec![(200, SoundKind::UserLeave, 1.5)]);
    }

    #[test]
    fn a_group_cools_down_after_a_sound() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        s.queue(SoundKind::Message, 1.0, false, base);
        let played = run(&mut s, base, 200);
        assert_eq!(played.len(), 1);
        // 150 ms after it played, a new message: it waits out the 900 ms cooldown.
        s.queue(SoundKind::DirectMessage, 1.0, false, ms(base, 300));
        let more = run(&mut s, ms(base, 300), 2000);
        assert_eq!(more.len(), 1);
        // Played at 120 + 900 = 1020 ms from the start, which is 720 ms after queueing.
        assert_eq!(more[0].0, 720);
    }

    #[test]
    fn two_groups_do_not_start_closer_than_the_global_gap() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        // Both are due at 80 ms (mute) and 100 ms (camera).
        s.queue(SoundKind::Mute, 1.0, false, base);
        s.queue(SoundKind::CameraOn, 1.0, false, base);
        let played = run(&mut s, base, 1000);
        assert_eq!(played.len(), 2);
        assert_eq!(played[0], (80, SoundKind::Mute, 1.0));
        assert!(played[1].0 - played[0].0 >= 260, "{played:?}");
    }

    #[test]
    fn of_two_due_together_the_higher_priority_goes_first() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        s.queue(SoundKind::IncomingRing, 1.0, false, base);
        s.queue(SoundKind::VoiceDisconnect, 1.0, false, base);
        // Disconnect is due at 40 ms and ring at 100 ms; ask at 100 ms for both.
        let first = s.drain(ms(base, 100)).unwrap();
        assert_eq!(first.kind, SoundKind::VoiceDisconnect);
        let second = s.drain(ms(base, 100)).unwrap();
        assert_eq!(second.kind, SoundKind::IncomingRing);
    }

    #[test]
    fn turning_sounds_off_clears_the_queue() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        s.queue(SoundKind::Message, 1.0, false, base);
        s.queue(SoundKind::Mute, 1.0, false, base);
        s.clear_kind(SoundKind::Message);
        assert_eq!(s.waiting(), 1);
        s.clear();
        assert_eq!(s.waiting(), 0);
        assert_eq!(s.next_due(base), None);
    }

    #[test]
    fn a_deafen_bypass_survives_when_it_is_the_sound_that_plays() {
        let base = Instant::now();
        let mut s = Scheduler::default();
        s.queue(SoundKind::UserJoin, 1.0, true, base);
        let due = s.drain(ms(base, 400)).unwrap();
        assert!(due.bypass_deafened);
    }
}
