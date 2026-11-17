// SPDX-License-Identifier: AGPL-3.0-or-later
//! The list of every setting the tabs offer, for the search across all tabs and for the
//! Advanced tab (which lists the settings Fluxer calls advanced, each with its control).
//! One entry names the tab the setting lives in, its category, its words (label,
//! description, search words) and, for the Advanced tab, the control that changes it.
//!
//! The search follows Fluxer's: every word of the query must match the label, a search
//! word or the description; an exact match of a label counts most, then a prefix, then a
//! part of it.

use inlet_state::flags::Flag;

use super::sync::Values;
use crate::backend::settings::write::Change;

/// What a setting is about; the Advanced tab groups its rows by it (Fluxer's order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Account,
    Privacy,
    Appearance,
    Accessibility,
    Chat,
    Media,
    Notifications,
    Desktop,
    Developer,
}

/// Every category, in the order Fluxer's Advanced tab lists them.
pub const CATEGORIES: [Category; 9] = [
    Category::Account,
    Category::Privacy,
    Category::Appearance,
    Category::Accessibility,
    Category::Chat,
    Category::Media,
    Category::Notifications,
    Category::Desktop,
    Category::Developer,
];

impl Category {
    /// The word Fluxer's search takes the category for (its "tags"): a search for
    /// "developer" finds every setting of that category.
    pub fn tag(self) -> &'static str {
        match self {
            Category::Account => "account",
            Category::Privacy => "privacy",
            Category::Appearance => "appearance",
            Category::Accessibility => "accessibility",
            Category::Chat => "chat",
            Category::Media => "media",
            Category::Notifications => "notifications",
            Category::Desktop => "desktop",
            Category::Developer => "developer",
        }
    }

    pub fn label(self) -> String {
        match self {
            Category::Account => t!("settings-category-account"),
            Category::Privacy => t!("settings-category-privacy"),
            Category::Appearance => t!("settings-category-appearance"),
            Category::Accessibility => t!("settings-category-accessibility"),
            Category::Chat => t!("settings-category-chat"),
            Category::Media => t!("settings-category-media"),
            Category::Notifications => t!("settings-category-notifications"),
            Category::Desktop => t!("settings-category-desktop"),
            Category::Developer => t!("settings-category-developer"),
        }
    }
}

/// A switch that belongs to a group (the group's rows are shown together).
pub struct Sub {
    pub id: &'static str,
    pub label: fn() -> String,
    pub get: fn(&Values) -> bool,
    pub set: fn(bool) -> Change,
    /// The row cannot be changed while this is true (Fluxer greys it out).
    pub disabled: fn(&Values) -> bool,
}

/// The control a row of the Advanced tab has.
pub enum Control {
    /// One switch.
    Switch {
        get: fn(&Values) -> bool,
        set: fn(bool) -> Change,
    },
    /// Several switches that belong together.
    Group(&'static [Sub]),
    /// Copies the text of the diagnostics.
    CopyDiagnostics,
    /// Copies the account's own user ID.
    CopyId,
    /// The setting lives in another tab: the row has an "Open" button.
    Open,
}

/// One setting.
pub struct Item {
    pub id: &'static str,
    /// The ID of the tab the setting lives in.
    pub tab: &'static str,
    pub category: Category,
    pub label: fn() -> String,
    /// What it does; empty for none.
    pub description: fn() -> String,
    /// More words that find it, separated by commas.
    pub keywords: fn() -> String,
    pub control: Control,
    /// The Advanced tab lists it with its control.
    pub advanced: bool,
}

/// A switch of the synced or local flags.
macro_rules! flag_switch {
    ($flag:expr) => {
        Control::Switch {
            get: |v| v.flag($flag),
            set: |on| Change::Flag($flag, on),
        }
    };
}

/// A row of a group.
macro_rules! sub {
    ($id:literal, $label:expr, $flag:expr, $disabled:expr) => {
        Sub {
            id: $id,
            label: || $label,
            get: |v| v.flag($flag),
            set: |on| Change::Flag($flag, on),
            disabled: $disabled,
        }
    };
}

/// A setting with a control in the Advanced tab.
macro_rules! advanced {
    ($id:literal, $cat:expr, $label:expr, $desc:expr, $kw:expr, $control:expr) => {
        Item {
            id: $id,
            tab: "advanced",
            category: $cat,
            label: || $label,
            description: || $desc,
            keywords: || $kw,
            control: $control,
            advanced: true,
        }
    };
}

/// A setting that lives in another tab; the search finds it, the Advanced tab does not
/// list it.
macro_rules! found {
    ($id:literal, $tab:literal, $cat:expr, $label:expr, $desc:expr, $kw:expr) => {
        Item {
            id: $id,
            tab: $tab,
            category: $cat,
            label: || $label,
            description: || $desc,
            keywords: || $kw,
            control: Control::Open,
            advanced: false,
        }
    };
}

const ACTION_BAR: &[Sub] = &[
    sub!(
        "action-bar-show",
        t!("settings-adv-action-bar-show"),
        Flag::ActionBar,
        |_| false
    ),
    sub!(
        "action-bar-only-more",
        t!("settings-adv-action-bar-only-more"),
        Flag::ActionBarOnlyMore,
        |v| !v.flag(Flag::ActionBar)
    ),
    sub!(
        "action-bar-quick-reactions",
        t!("settings-adv-action-bar-quick-reactions"),
        Flag::ActionBarQuickReactions,
        |v| !v.flag(Flag::ActionBar) || v.flag(Flag::ActionBarOnlyMore)
    ),
];

const INPUT_BUTTONS: &[Sub] = &[
    sub!(
        "input-gif",
        t!("settings-adv-input-gif"),
        Flag::InputGifButton,
        |_| false
    ),
    sub!(
        "input-stickers",
        t!("settings-adv-input-stickers"),
        Flag::InputStickersButton,
        |_| false
    ),
    sub!(
        "input-emoji",
        t!("settings-adv-input-emoji"),
        Flag::InputEmojiButton,
        |_| false
    ),
];

const AUTOCOMPLETE: &[Sub] = &[
    sub!(
        "autocomplete-default",
        t!("settings-adv-autocomplete-default"),
        Flag::AutocompleteDefaultEmojis,
        |_| false
    ),
    sub!(
        "autocomplete-custom",
        t!("settings-adv-autocomplete-custom"),
        Flag::AutocompleteCustomEmojis,
        |_| false
    ),
];

/// Every setting, in the order the Advanced tab lists them inside a category.
pub const ITEMS: &[Item] = &[
    // The settings with a control of their own in the Advanced tab.
    advanced!(
        "advanced-stay-interactive",
        Category::Accessibility,
        t!("settings-adv-stay-interactive"),
        t!("settings-adv-stay-interactive-desc"),
        t!("settings-adv-stay-interactive-kw"),
        flag_switch!(Flag::StayInteractiveUnfocused)
    ),
    advanced!(
        "advanced-action-bar",
        Category::Chat,
        t!("settings-adv-action-bar"),
        t!("settings-adv-action-bar-desc"),
        t!("settings-adv-action-bar-kw"),
        Control::Group(ACTION_BAR)
    ),
    advanced!(
        "advanced-autocomplete",
        Category::Chat,
        t!("settings-adv-autocomplete"),
        t!("settings-adv-autocomplete-desc"),
        t!("settings-adv-autocomplete-kw"),
        Control::Group(AUTOCOMPLETE)
    ),
    advanced!(
        "advanced-input-buttons",
        Category::Chat,
        t!("settings-adv-input-buttons"),
        t!("settings-adv-input-buttons-desc"),
        t!("settings-adv-input-buttons-kw"),
        Control::Group(INPUT_BUTTONS)
    ),
    found!(
        "advanced-emoticons",
        "messages-media",
        Category::Chat,
        t!("settings-adv-emoticons"),
        t!("settings-adv-emoticons-desc"),
        t!("settings-adv-emoticons-kw")
    ),
    advanced!(
        "advanced-double-click",
        Category::Chat,
        t!("settings-adv-double-click"),
        t!("settings-adv-double-click-desc"),
        t!("settings-adv-double-click-kw"),
        flag_switch!(Flag::DoubleClickActions)
    ),
    advanced!(
        "advanced-scroll-on-send",
        Category::Chat,
        t!("settings-adv-scroll-on-send"),
        t!("settings-adv-scroll-on-send-desc"),
        t!("settings-adv-scroll-on-send-kw"),
        flag_switch!(Flag::ScrollToBottomOnSend)
    ),
    advanced!(
        "advanced-hide-blocked",
        Category::Chat,
        t!("settings-adv-hide-blocked"),
        t!("settings-adv-hide-blocked-desc"),
        t!("settings-adv-hide-blocked-kw"),
        flag_switch!(Flag::HideBlockedMessages)
    ),
    advanced!(
        "advanced-developer-mode",
        Category::Developer,
        t!("settings-adv-developer-mode"),
        t!("settings-adv-developer-mode-desc"),
        t!("settings-adv-developer-mode-kw"),
        Control::Switch {
            get: |v| v.developer_mode,
            set: Change::DeveloperMode,
        }
    ),
    advanced!(
        "advanced-diagnostics",
        Category::Developer,
        t!("settings-adv-diagnostics"),
        t!("settings-adv-diagnostics-desc"),
        t!("settings-adv-diagnostics-kw"),
        Control::CopyDiagnostics
    ),
    advanced!(
        "advanced-copy-id",
        Category::Developer,
        t!("settings-adv-copy-id"),
        t!("settings-adv-copy-id-desc"),
        t!("settings-adv-copy-id-kw"),
        Control::CopyId
    ),
    // The settings of the other tabs, found by the search.
    found!(
        "profile-picture",
        "my-profile",
        Category::Account,
        t!("settings-find-profile-picture"),
        t!("settings-find-profile-picture-desc"),
        t!("settings-find-profile-picture-kw")
    ),
    found!(
        "profile-name",
        "my-profile",
        Category::Account,
        t!("settings-find-profile-name"),
        t!("settings-find-profile-name-desc"),
        t!("settings-find-profile-name-kw")
    ),
    found!(
        "profile-bio",
        "my-profile",
        Category::Account,
        t!("settings-find-profile-bio"),
        t!("settings-find-profile-bio-desc"),
        t!("settings-find-profile-bio-kw")
    ),
    found!(
        "profile-banner",
        "my-profile",
        Category::Account,
        t!("settings-find-profile-banner"),
        t!("settings-find-profile-banner-desc"),
        t!("settings-find-profile-banner-kw")
    ),
    found!(
        "appearance-theme",
        "appearance",
        Category::Appearance,
        t!("settings-find-theme"),
        t!("settings-find-theme-desc"),
        t!("settings-find-theme-kw")
    ),
    found!(
        "appearance-font-size",
        "appearance",
        Category::Appearance,
        t!("settings-find-font-size"),
        t!("settings-find-font-size-desc"),
        t!("settings-find-font-size-kw")
    ),
    found!(
        "appearance-spacing",
        "appearance",
        Category::Appearance,
        t!("settings-find-spacing"),
        t!("settings-find-spacing-desc"),
        t!("settings-find-spacing-kw")
    ),
    found!(
        "appearance-display-mode",
        "appearance",
        Category::Appearance,
        t!("settings-find-display-mode"),
        t!("settings-find-display-mode-desc"),
        t!("settings-find-display-mode-kw")
    ),
    found!(
        "appearance-zoom",
        "appearance",
        Category::Appearance,
        t!("settings-find-zoom"),
        t!("settings-find-zoom-desc"),
        t!("settings-find-zoom-kw")
    ),
    found!(
        "media-animation",
        "messages-media",
        Category::Media,
        t!("settings-find-animation"),
        t!("settings-find-animation-desc"),
        t!("settings-find-animation-kw")
    ),
    found!(
        "media-embeds",
        "messages-media",
        Category::Media,
        t!("settings-find-embeds"),
        t!("settings-find-embeds-desc"),
        t!("settings-find-embeds-kw")
    ),
    found!(
        "media-reactions",
        "messages-media",
        Category::Chat,
        t!("settings-find-reactions"),
        t!("settings-find-reactions-desc"),
        t!("settings-find-reactions-kw")
    ),
    found!(
        "media-spoilers",
        "messages-media",
        Category::Chat,
        t!("settings-find-spoilers"),
        t!("settings-find-spoilers-desc"),
        t!("settings-find-spoilers-kw")
    ),
    found!(
        "privacy-friend-requests",
        "privacy",
        Category::Privacy,
        t!("settings-find-friend-requests"),
        t!("settings-find-friend-requests-desc"),
        t!("settings-find-friend-requests-kw")
    ),
    found!(
        "privacy-dms",
        "privacy",
        Category::Privacy,
        t!("settings-find-dms"),
        t!("settings-find-dms-desc"),
        t!("settings-find-dms-kw")
    ),
    found!(
        "privacy-calls",
        "privacy",
        Category::Privacy,
        t!("settings-find-calls"),
        t!("settings-find-calls-desc"),
        t!("settings-find-calls-kw")
    ),
    found!(
        "privacy-groups",
        "privacy",
        Category::Privacy,
        t!("settings-find-groups"),
        t!("settings-find-groups-desc"),
        t!("settings-find-groups-kw")
    ),
    found!(
        "privacy-profile",
        "privacy",
        Category::Privacy,
        t!("settings-find-profile-privacy"),
        t!("settings-find-profile-privacy-desc"),
        t!("settings-find-profile-privacy-kw")
    ),
    found!(
        "privacy-sensitive",
        "privacy",
        Category::Privacy,
        t!("settings-find-sensitive"),
        t!("settings-find-sensitive-desc"),
        t!("settings-find-sensitive-kw")
    ),
    found!(
        "notifications",
        "notifications",
        Category::Notifications,
        t!("settings-find-notifications"),
        t!("settings-find-notifications-desc"),
        t!("settings-find-notifications-kw")
    ),
    found!(
        "desktop-tray",
        "desktop",
        Category::Desktop,
        t!("settings-find-tray"),
        t!("settings-find-tray-desc"),
        t!("settings-find-tray-kw")
    ),
    found!(
        "desktop-autostart",
        "desktop",
        Category::Desktop,
        t!("settings-find-autostart"),
        t!("settings-find-autostart-desc"),
        t!("settings-find-autostart-kw")
    ),
    found!(
        "keybinds",
        "keybinds",
        Category::Accessibility,
        t!("settings-find-keybinds"),
        t!("settings-find-keybinds-desc"),
        t!("settings-find-keybinds-kw")
    ),
    found!(
        "accounts",
        "accounts",
        Category::Account,
        t!("settings-find-accounts"),
        t!("settings-find-accounts-desc"),
        t!("settings-find-accounts-kw")
    ),
];

/// The settings the Advanced tab lists.
#[cfg(test)]
pub fn advanced_items() -> impl Iterator<Item = &'static Item> {
    ITEMS.iter().filter(|i| i.advanced)
}

/// Every word of a setting that the search looks at, in lower case.
struct Words {
    label: String,
    description: String,
    keywords: Vec<String>,
}

impl Words {
    fn of(item: &Item) -> Words {
        Words {
            label: (item.label)().to_lowercase(),
            description: (item.description)().to_lowercase(),
            keywords: (item.keywords)()
                .split(',')
                .map(|k| k.trim().to_lowercase())
                .filter(|k| !k.is_empty())
                .chain(std::iter::once(item.category.tag().to_owned()))
                .collect(),
        }
    }
}

/// How well `text` matches `word`: nothing, a part of it, a prefix, or all of it.
fn text_score(text: &str, word: &str, exact: u32, prefix: u32, part: u32) -> u32 {
    if !text.contains(word) {
        0
    } else if text == word {
        exact
    } else if text.starts_with(word) {
        prefix
    } else {
        part
    }
}

/// Fluxer's score of one setting for the words of a query; 0 when a word matches
/// nothing.
fn score(words: &Words, query: &[&str]) -> u32 {
    let mut total = 0;
    for word in query {
        let mut s = text_score(&words.label, word, 100, 50, 25);
        for k in &words.keywords {
            s += text_score(k, word, 80, 40, 20);
        }
        if words.description.contains(word) {
            s += 10;
        }
        if s == 0 {
            return 0;
        }
        total += s;
    }
    total
}

/// The words of a query: lower case, split at white space.
pub fn query_words(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// The settings that match `query`, best first (equal scores by label). `only_advanced`
/// restricts the search to the settings of the Advanced tab; an empty query matches all
/// of them.
pub fn search(query: &str, only_advanced: bool) -> Vec<&'static Item> {
    search_scored(query, only_advanced)
        .into_iter()
        .map(|(_, item)| item)
        .collect()
}

/// [`search`] with each setting's score (1 for all when nothing is typed).
pub fn search_scored(query: &str, only_advanced: bool) -> Vec<(u32, &'static Item)> {
    let words = query_words(query);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let mut hits: Vec<(u32, String, &'static Item)> = ITEMS
        .iter()
        .filter(|i| !only_advanced || i.advanced)
        .filter_map(|item| {
            let w = Words::of(item);
            let s = if words.is_empty() {
                1
            } else {
                score(&w, &words)
            };
            (s > 0).then_some((s, w.label, item))
        })
        .collect();
    // Fluxer's order: best score first, then by label (so without a query, by label).
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    hits.into_iter().map(|(s, _, item)| (s, item)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ids(query: &str, advanced: bool) -> Vec<&'static str> {
        search(query, advanced).iter().map(|i| i.id).collect()
    }

    #[test]
    fn every_setting_has_a_unique_id_and_words() {
        let mut seen = BTreeSet::new();
        for item in ITEMS {
            assert!(seen.insert(item.id), "{} twice", item.id);
            assert!(!(item.label)().is_empty(), "{} has no label", item.id);
            assert!(
                !(item.description)().is_empty(),
                "{}: no description",
                item.id
            );
            if let Control::Group(subs) = &item.control {
                for sub in *subs {
                    assert!(seen.insert(sub.id), "{} twice", sub.id);
                    assert!(!(sub.label)().is_empty());
                }
            }
        }
    }

    #[test]
    fn the_search_needs_every_word_and_ranks_a_label_over_a_description() {
        let hits = ids("action bar", true);
        assert_eq!(hits.first(), Some(&"advanced-action-bar"));
        assert!(!ids("action bar zebra", true).contains(&"advanced-action-bar"));
        // A search word finds a setting its label does not name.
        assert!(ids("quick reactions", true).contains(&"advanced-action-bar"));
        // Case and spacing do not matter.
        assert_eq!(ids("  ACTION   Bar ", true), ids("action bar", true));
        // Nothing found.
        assert!(ids("qqqqq", false).is_empty());
    }

    #[test]
    fn inside_a_category_the_list_is_by_score_and_then_by_label() {
        // Without a query every score is 1: alphabetical, as Fluxer's localeCompare.
        let developer: Vec<_> = search("", true)
            .iter()
            .filter(|i| i.category == Category::Developer)
            .map(|i| i.id)
            .collect();
        assert_eq!(
            developer,
            [
                "advanced-diagnostics",
                "advanced-copy-id",
                "advanced-developer-mode"
            ]
        );
        // A category word finds every setting of the category.
        let found = ids("developer", true);
        assert!(found.contains(&"advanced-developer-mode"));
        assert!(found.contains(&"advanced-diagnostics"));
        // The best match comes first.
        assert_eq!(
            ids("action bar", true).first(),
            Some(&"advanced-action-bar")
        );
    }

    #[test]
    fn the_search_across_the_tabs_finds_settings_of_every_tab() {
        let all = ids("theme", false);
        assert_eq!(all.first(), Some(&"appearance-theme"));
        let tabs: BTreeSet<_> = search("", false).iter().map(|i| i.tab).collect();
        for tab in [
            "my-profile",
            "appearance",
            "messages-media",
            "privacy",
            "notifications",
            "desktop",
            "keybinds",
            "advanced",
            "accounts",
        ] {
            assert!(tabs.contains(tab), "no setting of the {tab} tab");
        }
        // The Advanced tab lists only its own rows, in any case an empty query lists all.
        assert!(search("", true).iter().all(|i| i.advanced));
        assert_eq!(search("", true).len(), advanced_items().count());
    }

    #[test]
    fn the_advanced_tab_offers_diagnostics_developer_mode_and_the_id() {
        let developer: Vec<_> = advanced_items()
            .filter(|i| i.category == Category::Developer)
            .map(|i| match i.control {
                Control::CopyDiagnostics => "diagnostics",
                Control::CopyId => "id",
                Control::Switch { .. } => "switch",
                _ => "other",
            })
            .collect();
        assert_eq!(
            developer,
            ["switch", "diagnostics", "id"],
            "in the list's own order"
        );
        // The diagnostics text is what the crash module builds: it starts with the
        // version, and a search for "bug report" finds the row.
        assert!(crate::crash::diagnostics().contains(env!("CARGO_PKG_VERSION")));
        assert_eq!(ids("bug report", true), ["advanced-diagnostics"]);
    }

    #[test]
    fn every_switch_changes_the_value_it_shows() {
        let base = Values::default();
        let mut checked = 0;
        let mut flip = |get: fn(&Values) -> bool, set: fn(bool) -> Change, id: &str| {
            let mut v = base.clone();
            v.apply(&set(!get(&base)));
            assert_ne!(
                get(&v),
                get(&base),
                "{id}: the switch does not move its value"
            );
            checked += 1;
        };
        for item in ITEMS {
            match &item.control {
                Control::Switch { get, set } => flip(*get, *set, item.id),
                Control::Group(subs) => {
                    for sub in *subs {
                        flip(sub.get, sub.set, sub.id);
                    }
                }
                _ => {}
            }
        }
        assert!(
            checked >= 10,
            "the Advanced tab has its switches ({checked})"
        );
    }

    #[test]
    fn a_group_greys_out_what_the_action_bar_switch_turns_off() {
        let Control::Group(subs) = &ITEMS
            .iter()
            .find(|i| i.id == "advanced-action-bar")
            .unwrap()
            .control
        else {
            panic!("a group");
        };
        let mut v = Values::default();
        assert!(subs.iter().all(|s| !(s.disabled)(&v)));
        v.apply(&Change::Flag(Flag::ActionBar, false));
        let disabled: Vec<_> = subs
            .iter()
            .filter(|s| (s.disabled)(&v))
            .map(|s| s.id)
            .collect();
        assert_eq!(
            disabled,
            ["action-bar-only-more", "action-bar-quick-reactions"]
        );
    }
}
