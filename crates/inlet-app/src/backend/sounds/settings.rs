// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sound settings, with Fluxer's rules: a master volume, a volume per sound that
//! replaces it, a switch per sound and a switch for all of them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::SoundKind;

/// The master volume nobody has changed, in percent.
pub const DEFAULT_MASTER: u16 = 100;
/// The loudest a volume goes, in percent.
pub const MAX_PERCENT: u16 = 200;

/// What the sound settings are. Sounds are named by their ID (`SoundKind::id`), so an
/// ID this version does not know (a newer Fluxer client's) is kept and written back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    /// Every sound is off. The switches of the single sounds are kept.
    pub all_disabled: bool,
    /// The volume of every sound without a volume of its own, 0 to 200 percent.
    pub master: u16,
    /// Sounds the person turned off (true) or, for those that start off, on (false).
    pub disabled: BTreeMap<String, bool>,
    /// A volume of its own, per sound, 0 to 200 percent.
    pub overrides: BTreeMap<String, u16>,
}

impl Default for SoundSettings {
    fn default() -> Self {
        SoundSettings {
            all_disabled: false,
            master: DEFAULT_MASTER,
            disabled: BTreeMap::new(),
            overrides: BTreeMap::new(),
        }
    }
}

fn clamp(percent: u16) -> u16 {
    percent.min(MAX_PERCENT)
}

impl SoundSettings {
    /// Whether the sound is on: not by "all off", and not turned off by itself. A sound
    /// that starts off (the same-channel one) is on only when it was turned on.
    pub fn enabled(&self, kind: SoundKind) -> bool {
        if self.all_disabled {
            return false;
        }
        !self.is_disabled(kind)
    }

    /// Whether the sound's own switch is off (the all-off switch aside).
    pub fn is_disabled(&self, kind: SoundKind) -> bool {
        self.disabled
            .get(kind.id())
            .copied()
            .unwrap_or_else(|| kind.default_disabled())
    }

    /// Turns the sound's own switch. Like Fluxer, a sound that starts on only keeps an
    /// entry while it is off; one that starts off keeps one for either answer.
    pub fn set_enabled(&mut self, kind: SoundKind, on: bool) {
        if on {
            if kind.default_disabled() {
                self.disabled.insert(kind.id().to_owned(), false);
            } else {
                self.disabled.remove(kind.id());
            }
        } else {
            self.disabled.insert(kind.id().to_owned(), true);
        }
    }

    pub fn set_master(&mut self, percent: u16) {
        self.master = clamp(percent);
    }

    /// The sound's own volume, if it has one.
    pub fn override_of(&self, kind: SoundKind) -> Option<u16> {
        self.overrides.get(kind.id()).copied()
    }

    pub fn set_override(&mut self, kind: SoundKind, percent: u16) {
        self.overrides.insert(kind.id().to_owned(), clamp(percent));
    }

    pub fn clear_override(&mut self, kind: SoundKind) {
        self.overrides.remove(kind.id());
    }

    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
    }

    /// How many of the sounds have a volume of their own.
    pub fn override_count(&self) -> usize {
        SoundKind::ALL
            .iter()
            .filter(|k| self.overrides.contains_key(k.id()))
            .count()
    }

    /// The volume of the sound in percent: its own, else the master.
    pub fn percent(&self, kind: SoundKind) -> u16 {
        clamp(self.override_of(kind).unwrap_or(self.master))
    }

    /// The factor the sound's samples are multiplied with: 1.0 at 100 percent.
    pub fn multiplier(&self, kind: SoundKind) -> f32 {
        f32::from(self.percent(kind)) / 100.0
    }

    /// The account's settings (`SyncedPreferences.sound`) as these.
    pub fn from_proto(p: &inlet_protocol::proto::preferences::SoundSettings) -> Self {
        let percent = |v: f64| {
            if v.is_finite() {
                v.round().clamp(0.0, f64::from(MAX_PERCENT)) as u16
            } else {
                DEFAULT_MASTER
            }
        };
        SoundSettings {
            all_disabled: p.all_sounds_disabled,
            master: p.master_volume.map_or(DEFAULT_MASTER, percent),
            disabled: p
                .disabled_sounds
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            overrides: p
                .sound_overrides
                .iter()
                .map(|(k, v)| (k.clone(), percent(*v)))
                .collect(),
        }
    }

    /// Puts these settings into the account's (`SyncedPreferences.sound`). Fluxer leaves
    /// the master volume out while it is at its default; entries of sounds this version
    /// does not know stay as they are.
    pub fn write_proto(&self, prefs: &mut inlet_protocol::api::preferences::Preferences) {
        let out = prefs.sound.get_or_insert_with(Default::default);
        out.all_sounds_disabled = self.all_disabled;
        out.master_volume = (self.master != DEFAULT_MASTER).then_some(f64::from(self.master));
        for kind in SoundKind::ALL {
            match self.disabled.get(kind.id()) {
                Some(v) => {
                    out.disabled_sounds.insert(kind.id().to_owned(), *v);
                }
                None => {
                    out.disabled_sounds.remove(kind.id());
                }
            }
            match self.overrides.get(kind.id()) {
                Some(v) => {
                    out.sound_overrides
                        .insert(kind.id().to_owned(), f64::from(*v));
                }
                None => {
                    out.sound_overrides.remove(kind.id());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::api::preferences::Preferences;

    #[test]
    fn the_defaults_are_fluxers() {
        let s = SoundSettings::default();
        assert_eq!(s.master, 100);
        assert!(!s.all_disabled);
        for kind in SoundKind::ALL {
            assert_eq!(s.percent(kind), 100);
            assert_eq!(s.multiplier(kind), 1.0);
            // Everything is on except the same-channel sound.
            assert_eq!(
                s.enabled(kind),
                kind != SoundKind::SameChannelMessage,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_volume_of_its_own_replaces_the_master_and_both_stop_at_200() {
        let mut s = SoundSettings::default();
        s.set_master(50);
        assert_eq!(s.multiplier(SoundKind::Message), 0.5);
        s.set_override(SoundKind::Message, 150);
        assert_eq!(s.multiplier(SoundKind::Message), 1.5);
        assert_eq!(s.multiplier(SoundKind::Mute), 0.5);
        s.set_override(SoundKind::Mute, 5000);
        assert_eq!(s.override_of(SoundKind::Mute), Some(200));
        s.set_master(9999);
        assert_eq!(s.master, 200);
        assert_eq!(s.override_count(), 2);
        s.clear_override(SoundKind::Message);
        assert_eq!(s.override_count(), 1);
        s.clear_overrides();
        assert_eq!(s.override_count(), 0);
    }

    #[test]
    fn switches_follow_fluxers_rules() {
        let mut s = SoundSettings::default();
        s.set_enabled(SoundKind::Mute, false);
        assert!(!s.enabled(SoundKind::Mute));
        assert_eq!(s.disabled.get("mute"), Some(&true));
        // On again: a sound that starts on keeps no entry.
        s.set_enabled(SoundKind::Mute, true);
        assert!(s.enabled(SoundKind::Mute));
        assert!(!s.disabled.contains_key("mute"));
        // One that starts off keeps an explicit "not disabled".
        s.set_enabled(SoundKind::SameChannelMessage, true);
        assert!(s.enabled(SoundKind::SameChannelMessage));
        assert_eq!(s.disabled.get("same-channel-message"), Some(&false));
        // "All off" wins over every switch but keeps them.
        s.all_disabled = true;
        assert!(!s.enabled(SoundKind::SameChannelMessage));
        assert!(!s.is_disabled(SoundKind::SameChannelMessage));
    }

    #[test]
    fn the_account_settings_round_trip_and_keep_what_this_version_does_not_know() {
        let mut prefs = Preferences::default();
        {
            let remote = prefs.sound.get_or_insert_with(Default::default);
            remote.disabled_sounds.insert("a-newer-sound".into(), true);
            remote.sound_overrides.insert("a-newer-sound".into(), 33.0);
            remote.sound_overrides.insert("message".into(), 120.0);
            remote.master_volume = Some(80.0);
        }
        let mut s = SoundSettings::from_proto(prefs.sound.as_ref().unwrap());
        assert_eq!(s.master, 80);
        assert_eq!(s.override_of(SoundKind::Message), Some(120));

        s.set_master(100);
        s.clear_override(SoundKind::Message);
        s.set_override(SoundKind::Mute, 20);
        s.set_enabled(SoundKind::Deaf, false);
        s.write_proto(&mut prefs);
        let out = prefs.sound.as_ref().unwrap();
        // The default master is left out, as Fluxer does.
        assert_eq!(out.master_volume, None);
        assert_eq!(out.sound_overrides.get("mute"), Some(&20.0));
        assert!(!out.sound_overrides.contains_key("message"));
        assert_eq!(out.disabled_sounds.get("deaf"), Some(&true));
        // What a newer client wrote stays.
        assert_eq!(out.disabled_sounds.get("a-newer-sound"), Some(&true));
        assert_eq!(out.sound_overrides.get("a-newer-sound"), Some(&33.0));
        // And it reads back the same.
        let back = SoundSettings::from_proto(out);
        assert_eq!(back.master, 100);
        assert_eq!(back.override_of(SoundKind::Mute), Some(20));
        assert!(!back.enabled(SoundKind::Deaf));
    }

    #[test]
    fn odd_numbers_from_the_account_are_made_safe() {
        let mut p = inlet_protocol::proto::preferences::SoundSettings {
            master_volume: Some(f64::NAN),
            ..Default::default()
        };
        p.sound_overrides.insert("mute".into(), -40.0);
        p.sound_overrides.insert("deaf".into(), 1e12);
        let s = SoundSettings::from_proto(&p);
        assert_eq!(s.master, 100);
        assert_eq!(s.override_of(SoundKind::Mute), Some(0));
        assert_eq!(s.override_of(SoundKind::Deaf), Some(200));
    }
}
