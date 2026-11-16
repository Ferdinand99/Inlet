// SPDX-License-Identifier: AGPL-3.0-or-later
//! The on/off switches of the settings that live in the account's synced preferences
//! (Fluxer's `AccessibilitySettings`, `PrivacyPreferences` and a few more), and the ones
//! this computer keeps for itself. One enum names them all; each has Fluxer's default and,
//! for the synced ones, the way to read it from and write it into the preferences.
//!
//! Fluxer's rule for reading: a switch that is absent from the preferences has its
//! default; one that is present has its value.

use inlet_protocol::api::preferences::Preferences;

/// Where a switch is kept.
#[derive(Clone, Copy)]
pub enum Store {
    /// In the account's synced preferences.
    Synced {
        read: fn(&Preferences) -> Option<bool>,
        write: fn(&mut Preferences, bool),
    },
    /// Only on this computer (`settings.json`); Fluxer keeps these in local storage.
    Local,
}

/// What one switch is: the key it is stored under locally, its default, and its store.
pub struct Def {
    pub key: &'static str,
    pub default: bool,
    pub store: Store,
}

/// An optional accessibility field.
macro_rules! acc_opt {
    ($f:ident) => {
        Store::Synced {
            read: |p| p.accessibility.as_ref().and_then(|a| a.$f),
            write: |p, v| {
                p.accessibility.get_or_insert_with(Default::default).$f = Some(v);
            },
        }
    };
}

macro_rules! define_flags {
    ($($variant:ident, $key:literal, $default:literal, $store:expr;)*) => {
        /// One switch of the settings.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum Flag { $($variant),* }

        impl Flag {
            /// Every switch, in a fixed order.
            pub const ALL: &'static [Flag] = &[$(Flag::$variant),*];

            /// What the switch is.
            pub fn def(self) -> &'static Def {
                const DEFS: &[Def] = &[$(Def { key: $key, default: $default, store: $store }),*];
                &DEFS[self as usize]
            }
        }
    };
}

define_flags! {
    // The message action bar (Advanced).
    ActionBar, "action_bar", true, acc_opt!(show_message_action_bar);
    ActionBarQuickReactions, "action_bar_quick_reactions", true, acc_opt!(show_message_action_bar_quick_reactions);
    ActionBarOnlyMore, "action_bar_only_more", false, acc_opt!(show_message_action_bar_only_more_button);
    // What the colon autocomplete offers.
    AutocompleteDefaultEmojis, "autocomplete_default_emojis", true, acc_opt!(show_default_emojis_in_autocomplete);
    AutocompleteCustomEmojis, "autocomplete_custom_emojis", true, acc_opt!(show_custom_emojis_in_autocomplete);
    // The buttons of the message input.
    InputGifButton, "input_gif_button", true, acc_opt!(show_gif_button);
    InputStickersButton, "input_stickers_button", true, acc_opt!(show_stickers_button);
    InputEmojiButton, "input_emoji_button", true, acc_opt!(show_emoji_button);
    // Behaviour of the chat.
    ScrollToBottomOnSend, "scroll_to_bottom_on_send", true, acc_opt!(scroll_to_bottom_on_message_send);
    DoubleClickActions, "double_click_actions", false, acc_opt!(message_double_click_actions);
    StayInteractiveUnfocused, "stay_interactive_unfocused", false, acc_opt!(stay_interactive_when_unfocused);
    // Kept on this computer only (Fluxer's "AdvancedSettings" in local storage).
    HideBlockedMessages, "hide_blocked_messages", false, Store::Local;
}

impl Flag {
    /// The switch whose local key is `key`.
    pub fn from_key(key: &str) -> Option<Flag> {
        Flag::ALL.iter().copied().find(|f| f.def().key == key)
    }

    /// The switch is kept in the synced preferences (else only on this computer).
    pub fn is_synced(self) -> bool {
        matches!(self.def().store, Store::Synced { .. })
    }

    /// Fluxer's default.
    pub fn default_value(self) -> bool {
        self.def().default
    }

    /// What the preferences say; `None` when they do not say (or it is a local switch).
    pub fn read(self, prefs: &Preferences) -> Option<bool> {
        match self.def().store {
            Store::Synced { read, .. } => read(prefs),
            Store::Local => None,
        }
    }

    /// Sets the switch in the preferences. Sets, never toggles, so a write may be applied
    /// again after a conflict. Does nothing for a local switch.
    pub fn write(self, prefs: &mut Preferences, on: bool) {
        if let Store::Synced { write, .. } = self.def().store {
            write(prefs, on);
        }
    }
}

/// A value for every switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagSet(u64);

impl Default for FlagSet {
    /// Fluxer's defaults.
    fn default() -> Self {
        let mut set = FlagSet(0);
        for f in Flag::ALL {
            set = set.with(*f, f.default_value());
        }
        set
    }
}

impl FlagSet {
    pub fn get(self, f: Flag) -> bool {
        self.0 & (1 << f as u8) != 0
    }

    pub fn with(self, f: Flag, on: bool) -> FlagSet {
        if on {
            FlagSet(self.0 | (1 << f as u8))
        } else {
            FlagSet(self.0 & !(1 << f as u8))
        }
    }
}

/// Which synced switches the preferences name, and their values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Read {
    present: u64,
    on: u64,
}

impl Read {
    /// Reads every synced switch from `prefs`.
    pub fn of(prefs: &Preferences) -> Read {
        let mut r = Read::default();
        for f in Flag::ALL {
            if let Some(v) = f.read(prefs) {
                r.present |= 1 << *f as u8;
                if v {
                    r.on |= 1 << *f as u8;
                }
            }
        }
        r
    }

    /// What the preferences say about `f`.
    pub fn get(&self, f: Flag) -> Option<bool> {
        (self.present & (1 << f as u8) != 0).then(|| self.on & (1 << f as u8) != 0)
    }
}

impl FlagSet {
    /// Puts what the synced preferences say on top: a switch they do not name has its
    /// default again. Local switches stay as they are.
    pub fn with_synced(self, read: &Read) -> FlagSet {
        let mut set = self;
        for f in Flag::ALL.iter().filter(|f| f.is_synced()) {
            set = set.with(*f, read.get(*f).unwrap_or(f.default_value()));
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_keys_are_unique_and_the_set_has_room_for_every_switch() {
        assert!(Flag::ALL.len() <= 64);
        let keys: BTreeSet<_> = Flag::ALL.iter().map(|f| f.def().key).collect();
        assert_eq!(keys.len(), Flag::ALL.len());
        for f in Flag::ALL {
            assert_eq!(Flag::from_key(f.def().key), Some(*f));
        }
    }

    #[test]
    fn a_switch_the_preferences_do_not_name_has_fluxers_default() {
        let set = FlagSet::default().with_synced(&Read::of(&Preferences::default()));
        assert!(set.get(Flag::ActionBar));
        assert!(!set.get(Flag::ActionBarOnlyMore));
        assert!(set.get(Flag::ScrollToBottomOnSend));
        assert!(!set.get(Flag::DoubleClickActions));
    }

    #[test]
    fn a_written_switch_reads_back_and_a_local_one_is_left_alone() {
        let mut prefs = Preferences::default();
        for f in Flag::ALL.iter().filter(|f| f.is_synced()) {
            f.write(&mut prefs, !f.default_value());
        }
        let read = Read::of(&prefs);
        let set = FlagSet::default().with_synced(&read);
        for f in Flag::ALL {
            if f.is_synced() {
                assert_eq!(set.get(*f), !f.default_value(), "{f:?}");
            } else {
                assert_eq!(set.get(*f), f.default_value(), "{f:?} stays local");
                assert_eq!(f.read(&prefs), None);
            }
        }
    }
}
