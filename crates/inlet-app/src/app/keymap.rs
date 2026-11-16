// SPDX-License-Identifier: AGPL-3.0-or-later
//! The keyboard shortcuts: Fluxer's defaults, the actions behind the ones that act on
//! the whole window, and the list the help and the settings show.
//!
//! - [`shortcuts`] lists every shortcut a person can press, with the keys, the action
//!   that runs it and its label. The keys are written out for each platform (Command on
//!   macOS where Control is used elsewhere): gpui's own `secondary` needs an OS context
//!   in the pinned version and crashes without one, so [`Keys::now`] picks the modifier
//!   here.
//! - [`bind_keys`] binds the shortcuts that are new here (channel, unread channel and
//!   server steps, the inbox, the pins, upload, the page keys and the help). The others
//!   (the quick switcher, the pickers, Escape and Shift+Escape, Up in an empty message
//!   box) are bound by their own domains, and the tests check that every row of the list
//!   is a live binding of its action and that every other app-wide binding has a row.
//! - The handlers act only on the main screen with nothing over the chat (a layer or a
//!   message menu has the keyboard then), so a shortcut never works behind a window.
//!   Every shortcut needs a modifier or is not a character (the arrows, the page keys,
//!   Escape), so typing in a field is never taken.
//!
//! Own keys come later: [`Shortcut::keys`] is where they will replace the defaults.

use gpui::{actions, Action, App, Context, InteractiveElement, KeyBinding, Window};
use inlet_state::ChannelId;

use crate::app::{Inlet, Screen};
use crate::desktop::Layer;
use crate::ui::inbox::Panel;
use crate::ui::sidebar::dm_list::dm_rows_of;

actions!(
    inlet_keys,
    [
        PreviousChannel,
        NextChannel,
        PreviousUnreadChannel,
        NextUnreadChannel,
        PreviousServer,
        NextServer,
        ToggleInbox,
        TogglePins,
        UploadFile,
        ScrollPageUp,
        ScrollPageDown,
        ShowKeybindHelp
    ]
);

/// Keys as gpui spells them, for macOS and for the other platforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keys {
    pub mac: &'static str,
    pub other: &'static str,
}

impl Keys {
    /// The same keys on every platform.
    const fn same(keys: &'static str) -> Keys {
        Keys {
            mac: keys,
            other: keys,
        }
    }

    /// The keys of this platform.
    pub const fn now(self) -> &'static str {
        if cfg!(target_os = "macos") {
            self.mac
        } else {
            self.other
        }
    }
}

/// The default keys.
const CHANNEL_PREVIOUS: Keys = Keys::same("alt-up");
const CHANNEL_NEXT: Keys = Keys::same("alt-down");
const UNREAD_PREVIOUS: Keys = Keys::same("alt-shift-up");
const UNREAD_NEXT: Keys = Keys::same("alt-shift-down");
const SERVER_PREVIOUS: Keys = Keys {
    mac: "cmd-alt-up",
    other: "ctrl-alt-up",
};
const SERVER_NEXT: Keys = Keys {
    mac: "cmd-alt-down",
    other: "ctrl-alt-down",
};
const INBOX: Keys = Keys {
    mac: "cmd-i",
    other: "ctrl-i",
};
const PINS: Keys = Keys {
    mac: "cmd-p",
    other: "ctrl-p",
};
const UPLOAD: Keys = Keys {
    mac: "cmd-shift-u",
    other: "ctrl-shift-u",
};
const PAGE_UP: Keys = Keys::same("pageup");
const PAGE_DOWN: Keys = Keys::same("pagedown");
const HELP: Keys = Keys {
    mac: "cmd-/",
    other: "ctrl-/",
};
/// Keys whose binding is made by another domain.
const QUICK_SWITCHER: Keys = Keys {
    mac: "cmd-k",
    other: "ctrl-k",
};
const MARK_CHANNEL_READ: Keys = Keys::same("escape");
const MARK_SERVER_READ: Keys = Keys::same("shift-escape");
const EDIT_LAST: Keys = Keys::same("up");
const EMOJI_PICKER: Keys = Keys {
    mac: "cmd-e",
    other: "ctrl-e",
};
const GIF_PICKER: Keys = Keys {
    mac: "cmd-g",
    other: "ctrl-g",
};
const STICKER_PICKER: Keys = Keys {
    mac: "cmd-s",
    other: "ctrl-s",
};

/// Where a shortcut sits in the help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Navigation,
    Messages,
    Panels,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Navigation, Group::Messages, Group::Panels];

    pub fn title(self) -> String {
        match self {
            Group::Navigation => t!("keys-group-navigation"),
            Group::Messages => t!("keys-group-messages"),
            Group::Panels => t!("keys-group-panels"),
        }
    }
}

/// One shortcut: what it does and which keys do it.
pub struct Shortcut {
    /// Stable name, for the keys a person sets themselves later (only the tests read it
    /// so far).
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: &'static str,
    pub group: Group,
    /// The default keys.
    pub keys: Keys,
    /// The name of the gpui action the keys run (the tests check it is bound).
    #[cfg_attr(not(test), allow(dead_code))]
    pub action: &'static str,
    /// What it does, from the catalogue.
    pub label: fn() -> String,
}

macro_rules! action_name {
    ($action:ty) => {
        <$action as Action>::name_for_type()
    };
}

/// Every shortcut, in the order the help shows them.
pub fn shortcuts() -> Vec<Shortcut> {
    use crate::ui::composer::pickers::{ToggleEmojiPicker, ToggleGifPicker, ToggleStickerPicker};
    use crate::ui::notifications::{MarkChannelRead, MarkGuildRead};
    use crate::ui::search::ToggleQuickSwitcher;

    macro_rules! shortcut {
        ($id:literal, $group:ident, $keys:expr, $action:ty, $label:expr) => {
            Shortcut {
                id: $id,
                group: Group::$group,
                keys: $keys,
                action: action_name!($action),
                label: $label,
            }
        };
    }
    vec![
        shortcut!(
            "quick-switcher",
            Navigation,
            QUICK_SWITCHER,
            ToggleQuickSwitcher,
            || t!("keys-quick-switcher")
        ),
        shortcut!(
            "channel-previous",
            Navigation,
            CHANNEL_PREVIOUS,
            PreviousChannel,
            || t!("keys-channel-previous")
        ),
        shortcut!(
            "channel-next",
            Navigation,
            CHANNEL_NEXT,
            NextChannel,
            || t!("keys-channel-next")
        ),
        shortcut!(
            "unread-previous",
            Navigation,
            UNREAD_PREVIOUS,
            PreviousUnreadChannel,
            || t!("keys-unread-previous")
        ),
        shortcut!(
            "unread-next",
            Navigation,
            UNREAD_NEXT,
            NextUnreadChannel,
            || t!("keys-unread-next")
        ),
        shortcut!(
            "server-previous",
            Navigation,
            SERVER_PREVIOUS,
            PreviousServer,
            || t!("keys-server-previous")
        ),
        shortcut!("server-next", Navigation, SERVER_NEXT, NextServer, || t!(
            "keys-server-next"
        )),
        shortcut!("scroll-up", Navigation, PAGE_UP, ScrollPageUp, || t!(
            "keys-scroll-up"
        )),
        shortcut!("scroll-down", Navigation, PAGE_DOWN, ScrollPageDown, || t!(
            "keys-scroll-down"
        )),
        shortcut!(
            "mark-channel-read",
            Messages,
            MARK_CHANNEL_READ,
            MarkChannelRead,
            || t!("keys-mark-channel-read")
        ),
        shortcut!(
            "mark-server-read",
            Messages,
            MARK_SERVER_READ,
            MarkGuildRead,
            || t!("keys-mark-server-read")
        ),
        shortcut!(
            "edit-last",
            Messages,
            EDIT_LAST,
            inlet_editor::actions::Up,
            || t!("keys-edit-last")
        ),
        shortcut!("upload", Messages, UPLOAD, UploadFile, || t!("keys-upload")),
        shortcut!(
            "emoji-picker",
            Messages,
            EMOJI_PICKER,
            ToggleEmojiPicker,
            || t!("keys-emoji-picker")
        ),
        shortcut!("gif-picker", Messages, GIF_PICKER, ToggleGifPicker, || t!(
            "keys-gif-picker"
        )),
        shortcut!(
            "sticker-picker",
            Messages,
            STICKER_PICKER,
            ToggleStickerPicker,
            || t!("keys-sticker-picker")
        ),
        shortcut!("inbox", Panels, INBOX, ToggleInbox, || t!("keys-inbox")),
        shortcut!("pins", Panels, PINS, TogglePins, || t!("keys-pins")),
        shortcut!("help", Panels, HELP, ShowKeybindHelp, || t!("keys-help")),
    ]
}

/// One line of the help: a shortcut as a person reads it.
pub struct Row {
    pub group: Group,
    pub label: String,
    /// The keys one by one as the platform writes them: `["Ctrl", "Alt", "↑"]`.
    pub keys: Vec<String>,
}

/// The rows of the help and of the settings, in the order of [`shortcuts`].
pub fn rows() -> Vec<Row> {
    let mac = cfg!(target_os = "macos");
    shortcuts()
        .into_iter()
        .map(|s| Row {
            group: s.group,
            label: (s.label)(),
            keys: key_names(s.keys.now(), mac),
        })
        .collect()
}

/// The names of the keys in `keys` as gpui spells them (`ctrl-alt-up`), as the platform
/// writes them.
pub fn key_names(keys: &str, mac: bool) -> Vec<String> {
    keys
        .split('-')
        .map(|part| match part {
            "ctrl" => t!("keys-ctrl"),
            "cmd" => t!("keys-command"),
            "alt" if mac => t!("keys-option"),
            "alt" => t!("keys-alt"),
            "shift" if mac => t!("keys-shift-symbol"),
            "shift" => t!("keys-shift"),
            "up" => t!("keys-up"),
            "down" => t!("keys-down"),
            "pageup" => t!("keys-page-up"),
            "pagedown" => t!("keys-page-down"),
            "escape" => t!("keys-escape"),
            other => other.to_uppercase(),
        })
        .collect()
}

/// Binds the shortcuts that are new here. The others are bound by their domains.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(CHANNEL_PREVIOUS.now(), PreviousChannel, None),
        KeyBinding::new(CHANNEL_NEXT.now(), NextChannel, None),
        KeyBinding::new(UNREAD_PREVIOUS.now(), PreviousUnreadChannel, None),
        KeyBinding::new(UNREAD_NEXT.now(), NextUnreadChannel, None),
        KeyBinding::new(SERVER_PREVIOUS.now(), PreviousServer, None),
        KeyBinding::new(SERVER_NEXT.now(), NextServer, None),
        KeyBinding::new(INBOX.now(), ToggleInbox, None),
        KeyBinding::new(PINS.now(), TogglePins, None),
        KeyBinding::new(UPLOAD.now(), UploadFile, None),
        KeyBinding::new(PAGE_UP.now(), ScrollPageUp, None),
        KeyBinding::new(PAGE_DOWN.now(), ScrollPageDown, None),
        KeyBinding::new(HELP.now(), ShowKeybindHelp, None),
    ]);
}

/// The item after `current` in `items` (or before it when not `forward`), going round
/// at the ends, that is `wanted`. `current` is never the answer, and when it is not in
/// `items` the walk starts at the first (or last) one. `None` when nothing qualifies.
pub(crate) fn step_where<'a, T: PartialEq>(
    items: &'a [T],
    current: Option<&T>,
    forward: bool,
    wanted: impl Fn(&T) -> bool,
) -> Option<&'a T> {
    let n = items.len();
    let at = current.and_then(|c| items.iter().position(|i| i == c));
    (1..=n)
        .map(|k| match (at, forward) {
            (Some(i), true) => (i + k) % n,
            (Some(i), false) => (i + n - k) % n,
            (None, true) => k - 1,
            (None, false) => n - k,
        })
        .map(|ix| &items[ix])
        .find(|item| current != Some(*item) && wanted(item))
}

/// The next (or previous) item, going round at the ends.
pub(crate) fn step<'a, T: PartialEq>(
    items: &'a [T],
    current: Option<&T>,
    forward: bool,
) -> Option<&'a T> {
    step_where(items, current, forward, |_| true)
}

impl Inlet {
    /// Whether a shortcut that acts on the chat may act now: signed in on the main
    /// screen, with nothing over the chat that has the keyboard (a layer, a message menu
    /// or dialog) and not under the About view.
    fn shortcuts_allowed(&self) -> bool {
        self.screen == Screen::Main
            && self.account.is_some()
            && !self.show_about
            && !self.layer_open()
            && self.chat.actions.overlay.is_none()
    }

    /// Like [`Inlet::shortcuts_allowed`], but a panel (the pins or the inbox) may be
    /// the layer that is open: its own key closes it and the other one swaps it.
    fn panel_keys_allowed(&self) -> bool {
        self.screen == Screen::Main
            && self.account.is_some()
            && !self.show_about
            && matches!(self.open_layers().topmost(), None | Some(Layer::Panel))
            && self.chat.actions.overlay.is_none()
    }

    /// The channels the arrow keys step through, in the order the list shows them:
    /// the open server's text and forum channels, or the direct messages as the DM list
    /// orders them.
    fn stepping_channels(&self, cx: &App) -> Vec<String> {
        match (&self.selected_guild, self.account.as_ref()) {
            (None, Some(account)) => match self.social.dm_rows.clone() {
                Some(rows) => rows,
                None => dm_rows_of(account.read(cx), &self.channels),
            }
            .into_iter()
            .map(|r| r.id)
            .collect(),
            _ => self
                .channels
                .iter()
                .filter(|c| c.is_messageable() || c.is_forum())
                .map(|c| c.id.clone())
                .collect(),
        }
    }

    /// Alt+Up and Alt+Down: the next or previous channel.
    fn step_channel(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        let ids = self.stepping_channels(cx);
        let next = step(&ids, self.selected_channel.as_ref(), forward).cloned();
        if let Some(id) = next {
            self.select_channel(id, window, cx);
        }
    }

    /// Alt+Shift+Up and Alt+Shift+Down: the next or previous channel with something
    /// unread (a mention, or unread messages when it is not muted).
    fn step_unread_channel(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        let ids = self.stepping_channels(cx);
        let next = {
            let Some(account) = self.account.as_ref() else {
                return;
            };
            let state = account.read(cx);
            step_where(&ids, self.selected_channel.as_ref(), forward, |id| {
                ChannelId::parse(id).is_some_and(|ch| {
                    let u = state.unread(ch);
                    (u.unread && !u.muted) || u.mentions > 0
                })
            })
            .cloned()
        };
        if let Some(id) = next {
            self.select_channel(id, window, cx);
        }
    }

    /// Ctrl+Alt+Up and Ctrl+Alt+Down: the next or previous server, the direct messages
    /// first, as the rail lists them.
    fn step_server(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        let rail: Vec<Option<String>> = std::iter::once(None)
            .chain(self.guilds.iter().map(|g| Some(g.id.clone())))
            .collect();
        let next = step(&rail, Some(&self.selected_guild), forward).cloned();
        if let Some(guild) = next {
            self.select_guild(guild, cx);
            // The message box is gone until the new server's channel opens (and for
            // good in a server without one): the window keeps the keyboard, so the
            // next shortcut still works.
            window.focus(&self.focus, cx);
        }
    }

    /// PageUp and PageDown: the message list scrolls by most of a screen.
    fn scroll_page(&mut self, down: bool, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        let list = self.chat.message_list.clone();
        let page = list.viewport_bounds().size.height * 0.9;
        if down {
            list.scroll_by(page);
        } else {
            // Leaves the newest messages, so new ones do not pull the list back down.
            list.pause_following_tail();
            list.scroll_by(-page);
            // The list reports only scrolling with the wheel or the bar, so the
            // older messages are asked for here when the top is reached.
            if list.logical_scroll_top().item_ix <= 1 {
                self.load_older(cx);
            }
        }
        cx.notify();
    }

    /// Ctrl+I and Ctrl+P: the inbox or the pins of the open channel, opened or closed.
    fn toggle_panel_key(&mut self, panel: Panel, cx: &mut Context<Self>) {
        if self.panel_keys_allowed() {
            self.toggle_panel(panel, cx);
        }
    }

    /// Ctrl+Shift+U: the file dialog of the message box.
    fn upload_key(&mut self, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() || self.open_channel_id().is_none() {
            return;
        }
        self.composer
            .view
            .update(cx, |view, cx| view.pick_files(cx));
    }

    /// Ctrl+/: the list of shortcuts, opened or closed. It never opens over another
    /// layer.
    pub(crate) fn toggle_keybind_help(&mut self, cx: &mut Context<Self>) {
        if self.overlays.keybind_help {
            self.overlays.keybind_help = false;
        } else if !self.show_about && !self.layer_open() {
            self.overlays.keybind_help = true;
        } else {
            return;
        }
        cx.notify();
    }

    /// The window's handlers of the shortcuts above, and the demo scene that opens the
    /// help.
    pub(crate) fn keymap_keys<E: InteractiveElement>(
        &mut self,
        el: E,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> E {
        #[cfg(feature = "demo")]
        self.demo_keybind_help(window, cx);
        let _ = window;
        el.on_action(
            cx.listener(|this, _: &PreviousChannel, window, cx| {
                this.step_channel(false, window, cx)
            }),
        )
        .on_action(
            cx.listener(|this, _: &NextChannel, window, cx| this.step_channel(true, window, cx)),
        )
        .on_action(cx.listener(|this, _: &PreviousUnreadChannel, window, cx| {
            this.step_unread_channel(false, window, cx)
        }))
        .on_action(cx.listener(|this, _: &NextUnreadChannel, window, cx| {
            this.step_unread_channel(true, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &PreviousServer, window, cx| this.step_server(false, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &NextServer, window, cx| this.step_server(true, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ToggleInbox, _, cx| this.toggle_panel_key(Panel::Inbox, cx)),
        )
        .on_action(
            cx.listener(|this, _: &TogglePins, _, cx| this.toggle_panel_key(Panel::Pins, cx)),
        )
        .on_action(cx.listener(|this, _: &UploadFile, _, cx| this.upload_key(cx)))
        .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| this.scroll_page(false, cx)))
        .on_action(cx.listener(|this, _: &ScrollPageDown, _, cx| this.scroll_page(true, cx)))
        .on_action(cx.listener(|this, _: &ShowKeybindHelp, _, cx| this.toggle_keybind_help(cx)))
    }

    /// Demo scene `keybind-help`: the list is open from the first frame with the chat.
    #[cfg(feature = "demo")]
    fn demo_keybind_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::demo::scenes::{current, Scene};
        if self.overlays.demo_help_done || self.screen != Screen::Main {
            return;
        }
        self.overlays.demo_help_done = true;
        if current() == Scene::KeybindHelp {
            cx.defer_in(window, |this, _, cx| this.toggle_keybind_help(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Arc;

    use gpui::{AppContext, Entity, Focusable, Keystroke, TestAppContext, VisualTestContext};

    use super::*;
    use crate::backend::accounts::{AccountInfo, AccountKey, Accounts};
    use crate::model::{Channel, ChannelKind};
    use crate::token_store::MemoryStore;
    use inlet_protocol::models::FLUXER_EPOCH_MS;
    use inlet_state::AccountState;

    /// Bindings that only work inside a field, a dialog or a list that has the keyboard
    /// (a text field's editing keys, moving in a list, closing a window). They are not
    /// shortcuts for the whole window, so the help does not list them.
    const INSIDE_A_FIELD: [&str; 7] = [
        "inlet_composer::AcceptCompletion",
        "inlet_composer::Cancel",
        "inlet_message_actions::CloseMessageOverlay",
        "inlet_message_actions::ConfirmMessageOverlay",
        "inlet_member_search::ClearMemberSearch",
        "search::SwitcherUp",
        "search::SwitcherDown",
    ];
    /// The prefixes of the actions of the two text fields.
    const FIELD_PREFIXES: [&str; 2] = ["text_input::", "inlet_editor::"];

    /// Every shortcut of the list is a live binding of its action under the keys it
    /// shows, with every domain's keys in place, and every shortcut that works in the
    /// whole window is in the list.
    #[gpui::test]
    fn every_default_binding_has_an_action_and_a_help_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.clear_key_bindings();
            crate::bind_keys(cx);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let list = shortcuts();
            let ids: BTreeSet<_> = list.iter().map(|s| s.id).collect();
            assert_eq!(ids.len(), list.len(), "two shortcuts share an id");
            for s in &list {
                let typed = [Keystroke::parse(s.keys.now()).expect("keys parse")];
                assert!(
                    keymap.bindings().any(|b| b.action().name() == s.action
                        && b.match_keystrokes(&typed) == Some(false)),
                    "{} ({}) is not bound to {}",
                    s.id,
                    s.keys.now(),
                    s.action
                );
                assert!(!(s.label)().trim().is_empty(), "{} has no label", s.id);
            }
            let rows = rows();
            assert_eq!(rows.len(), list.len());
            assert!(
                rows.iter()
                    .all(|r| !r.keys.is_empty() && !r.label.is_empty()),
                "a row without keys or a label"
            );
            let listed: BTreeSet<&str> = list.iter().map(|s| s.action).collect();
            for b in keymap.bindings() {
                let name = b.action().name();
                let own = listed.contains(name)
                    || INSIDE_A_FIELD.contains(&name)
                    || FIELD_PREFIXES.iter().any(|p| name.starts_with(p));
                assert!(own, "{name} is bound but has no row in the help");
            }
        });
    }

    /// `secondary` needs an OS context in the pinned gpui and crashes without one: no
    /// shortcut, on either platform, and no binding in the code uses it.
    #[test]
    fn bindings_never_use_secondary_modifier() {
        for s in shortcuts() {
            for keys in [s.keys.mac, s.keys.other] {
                assert!(!keys.contains("secondary"), "{keys}");
                assert!(Keystroke::parse(keys).is_ok(), "{keys} does not parse");
            }
            // Command where the others use Control, and nothing else differs.
            assert_eq!(s.keys.mac.replace("cmd", "ctrl"), s.keys.other, "{}", s.id);
        }
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect(&src, &mut files);
        let needle = ["second", "ary"].concat();
        for (path, text) in files {
            for (n, line) in text.lines().enumerate() {
                let binds = ["KeyBinding::new(", "bind!(", "bind_cmd!("]
                    .iter()
                    .any(|m| line.contains(m));
                assert!(
                    !(binds && line.contains(&needle)),
                    "{path}:{}: a binding uses the secondary modifier",
                    n + 1
                );
            }
        }
    }

    fn collect(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read src/") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read a source file");
                out.push((path.display().to_string(), text));
            }
        }
    }

    #[test]
    fn the_keys_are_written_as_the_platform_writes_them() {
        assert_eq!(SERVER_NEXT.mac, "cmd-alt-down");
        assert_eq!(SERVER_NEXT.other, "ctrl-alt-down");
        assert_eq!(UNREAD_NEXT.now(), "alt-shift-down");
        assert_eq!(
            key_names("ctrl-shift-u", false),
            vec!["Ctrl".to_string(), "Shift".into(), "U".into()]
        );
        assert_eq!(
            key_names("cmd-alt-up", true),
            vec!["\u{2318}".to_string(), "\u{2325}".into(), "\u{2191}".into()]
        );
    }

    #[test]
    fn stepping_goes_round_and_skips_what_is_not_wanted() {
        let items = ["a", "b", "c", "d"];
        assert_eq!(step(&items, Some(&"b"), true), Some(&"c"));
        assert_eq!(step(&items, Some(&"b"), false), Some(&"a"));
        // Round the ends.
        assert_eq!(step(&items, Some(&"d"), true), Some(&"a"));
        assert_eq!(step(&items, Some(&"a"), false), Some(&"d"));
        // Nothing open: the first, or the last.
        assert_eq!(step(&items, None, true), Some(&"a"));
        assert_eq!(step(&items, None, false), Some(&"d"));
        assert_eq!(step(&items, Some(&"gone"), true), Some(&"a"));
        // Only the open one, or nothing at all: there is nowhere to go.
        assert_eq!(step(&["a"], Some(&"a"), true), None);
        assert_eq!(step::<&str>(&[], None, true), None);
        // Wanted ones only, round the end; never the open one itself.
        let wanted = |i: &&str| *i == "a" || *i == "c";
        assert_eq!(step_where(&items, Some(&"c"), true, wanted), Some(&"a"));
        assert_eq!(step_where(&items, Some(&"a"), false, wanted), Some(&"c"));
        assert_eq!(step_where(&items, Some(&"a"), true, wanted), Some(&"c"));
        assert_eq!(step_where(&items, Some(&"a"), true, |i| *i == "a"), None);
        assert_eq!(step_where(&items, None, true, |i| *i == "z"), None);
    }

    const GUILD: &str = "100";
    const OTHER_GUILD: &str = "200";
    const JOINED_MS: u64 = 1_767_225_600_000;

    /// A message ID `secs` after the join time.
    fn mid(secs: u64) -> String {
        (((JOINED_MS + secs * 1000 - FLUXER_EPOCH_MS) << 22) + 1).to_string()
    }

    /// Three text channels in `GUILD` (`general` has nothing unread, `chan-b` and
    /// `chan-c` have), and a second, empty guild.
    fn state() -> AccountState {
        let guild = |id: &str, channels: serde_json::Value| {
            serde_json::json!({
                "id": id,
                "properties": {"id": id, "name": "g", "owner_id": "1"},
                "roles": [{"id": id, "name": "@everyone", "position": 0,
                           "permissions": "274877975552"}],
                "channels": channels,
                "members": [{"user": {"id": "10", "username": "me"}, "roles": [],
                             "joined_at": "2026-01-01T00:00:00Z"}]
            })
        };
        let ready: inlet_protocol::Ready = serde_json::from_value(serde_json::json!({
            "session_id": "s",
            "user": {"id": "10", "username": "me", "mfa_enabled": false},
            "guilds": [
                guild(GUILD, serde_json::json!([
                    {"id": "301", "type": 0, "name": "general", "position": 0,
                     "last_message_id": mid(10)},
                    {"id": "302", "type": 0, "name": "chan-b", "position": 1,
                     "last_message_id": mid(50)},
                    {"id": "303", "type": 0, "name": "chan-c", "position": 2,
                     "last_message_id": mid(60)}
                ])),
                guild(OTHER_GUILD, serde_json::json!([
                    {"id": "401", "type": 0, "name": "other", "position": 0}
                ])),
            ],
            "private_channels": [],
            "users": [],
            "read_states": [
                {"id": "301", "mention_count": 0, "last_message_id": mid(10)},
                {"id": "302", "mention_count": 0, "last_message_id": mid(20)},
                {"id": "303", "mention_count": 0, "last_message_id": mid(20)}
            ],
            "user_guild_settings": []
        }))
        .unwrap();
        AccountState::from_ready_at(&ready, JOINED_MS as i64 + 3_600_000)
    }

    fn channel(id: &str, name: &str, kind: ChannelKind, position: i64) -> Channel {
        Channel {
            id: id.into(),
            name: name.into(),
            kind,
            parent_id: None,
            position,
            topic: None,
            slowmode_secs: 0,
        }
    }

    /// A window with Inlet signed in (by hand) to one account, `GUILD` open at
    /// `general`, its channel list as the sidebar holds it (with a voice channel the
    /// steps must skip), and the app's keys.
    fn rig(cx: &mut TestAppContext) -> (Entity<Inlet>, &mut VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(crate::bind_keys);
        let (inlet, cx) = cx.add_window_view(|window, cx| {
            Inlet::new_with(window, cx, |tx| {
                crate::backend::start_with(tx, |ui| {
                    Accounts::new(Arc::new(MemoryStore::default()), None, Some(ui))
                })
            })
        });
        // The backend answers that no account is stored; after that it is quiet.
        for _ in 0..200 {
            cx.run_until_parked();
            if inlet.read_with(cx, |i, _| i.screen == Screen::SignIn) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        inlet.update_in(cx, |this, window, cx| {
            let key = AccountKey::new("https://a.example", "10").unwrap();
            this.switcher.accounts = vec![AccountInfo {
                key: key.clone(),
                name: "me".into(),
                handle: "me".into(),
            }];
            this.switcher.active = Some(key);
            this.account = Some(cx.new(|_| state()));
            this.guilds = ["100", "200"]
                .map(|id| crate::model::Guild {
                    id: id.into(),
                    name: format!("guild {id}"),
                })
                .to_vec();
            this.selected_guild = Some(GUILD.into());
            this.channels = vec![
                channel("301", "general", ChannelKind::Text, 0),
                channel("302", "chan-b", ChannelKind::Text, 1),
                channel("350", "voice", ChannelKind::Voice, 2),
                channel("303", "chan-c", ChannelKind::Text, 3),
            ];
            this.screen = Screen::Main;
            this.select_channel("301".into(), window, cx);
            // The keyboard is in the message box, where a person types.
            let handle = this.composer.view.focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
        (inlet, cx)
    }

    fn open_channel(inlet: &Entity<Inlet>, cx: &mut VisualTestContext) -> Option<String> {
        inlet.read_with(cx, |i, _| i.selected_channel.clone())
    }

    /// Alt+Down and Alt+Up step through the text channels (the voice channel is not
    /// one), also with the message box focused, and go round at the ends.
    #[gpui::test]
    fn alt_arrows_step_through_the_channels_even_while_typing(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.simulate_keystrokes("alt-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("302"));
        cx.simulate_keystrokes("alt-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
        cx.simulate_keystrokes("alt-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("301"));
        cx.simulate_keystrokes("alt-up");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
    }

    /// Alt+Shift+Down goes to the next channel with something unread and ignores the
    /// read ones; with nothing else unread it stays where it is.
    #[gpui::test]
    fn alt_shift_arrows_step_through_the_unread_channels(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.simulate_keystrokes("alt-shift-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("302"));
        cx.simulate_keystrokes("alt-shift-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
        // Round the end: `general` is read, so the walk comes back to `chan-b`.
        cx.simulate_keystrokes("alt-shift-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("302"));
        cx.simulate_keystrokes("alt-shift-up");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
    }

    /// Ctrl+Alt+Down and Ctrl+Alt+Up step through the direct messages and the servers
    /// in the rail's order.
    #[gpui::test]
    fn ctrl_alt_arrows_step_through_the_servers(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        let guild = |inlet: &Entity<Inlet>, cx: &mut VisualTestContext| {
            inlet.read_with(cx, |i, _| i.selected_guild.clone())
        };
        cx.simulate_keystrokes("ctrl-alt-down");
        assert_eq!(guild(&inlet, cx).as_deref(), Some(OTHER_GUILD));
        cx.simulate_keystrokes("ctrl-alt-down");
        assert_eq!(guild(&inlet, cx), None, "the direct messages come first");
        cx.simulate_keystrokes("ctrl-alt-up");
        assert_eq!(guild(&inlet, cx).as_deref(), Some(OTHER_GUILD));
        cx.simulate_keystrokes("ctrl-alt-up");
        assert_eq!(guild(&inlet, cx).as_deref(), Some(GUILD));
    }

    /// Nothing steps behind a layer: with the quick switcher open the keys leave the
    /// channel, the server, the inbox and the pins alone.
    #[gpui::test]
    fn shortcuts_do_nothing_behind_a_layer(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.update(|window, cx| inlet.update(cx, |this, cx| this.toggle_quick_switcher(window, cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes("alt-down ctrl-alt-down ctrl-p");
        inlet.read_with(cx, |i, _| {
            assert_eq!(i.selected_channel.as_deref(), Some("301"));
            assert_eq!(i.selected_guild.as_deref(), Some(GUILD));
            assert!(i.inbox.open.is_none());
        });
    }

    /// Ctrl+I opens the inbox and closes it again, Ctrl+P swaps it for the pins, and
    /// Escape closes whichever is open.
    #[gpui::test]
    fn ctrl_i_and_ctrl_p_open_the_panels(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        let open = |inlet: &Entity<Inlet>, cx: &mut VisualTestContext| {
            inlet.read_with(cx, |i, _| i.inbox.open)
        };
        cx.simulate_keystrokes("ctrl-i");
        assert_eq!(open(&inlet, cx), Some(Panel::Inbox));
        cx.simulate_keystrokes("ctrl-i");
        assert_eq!(open(&inlet, cx), None);
        cx.simulate_keystrokes("ctrl-i ctrl-p");
        assert_eq!(open(&inlet, cx), Some(Panel::Pins));
        cx.simulate_keystrokes("escape");
        assert_eq!(open(&inlet, cx), None);
    }

    /// Ctrl+/ opens the list of shortcuts and closes it; Escape closes it too; it never
    /// opens over another layer, and while it is open the channel keys do nothing.
    #[gpui::test]
    fn ctrl_slash_opens_the_help_and_escape_closes_it(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        let help = |inlet: &Entity<Inlet>, cx: &mut VisualTestContext| {
            inlet.read_with(cx, |i, _| i.overlays.keybind_help)
        };
        cx.simulate_keystrokes("ctrl-/");
        cx.run_until_parked();
        assert!(help(&inlet, cx));
        cx.simulate_keystrokes("alt-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("301"));
        cx.simulate_keystrokes("ctrl-/");
        assert!(!help(&inlet, cx));
        cx.simulate_keystrokes("ctrl-/");
        cx.run_until_parked();
        assert!(help(&inlet, cx));
        cx.simulate_keystrokes("escape");
        assert!(!help(&inlet, cx));
        // Over the inbox it does not open.
        cx.simulate_keystrokes("ctrl-i ctrl-/");
        assert!(!help(&inlet, cx));
    }

    /// Typing in the message box is not taken by any shortcut: the letters the
    /// shortcuts use (I, P, U, K, E, G, S and the slash) and the keys they are made of
    /// still go into the box, the keyboard stays there, and nothing opens.
    #[gpui::test]
    fn focus_in_composer_does_not_steal_typing_keys(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        inlet.update_in(cx, |this, window, cx| {
            let handle = this.composer.view.focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
        let text = |inlet: &Entity<Inlet>, cx: &mut VisualTestContext| {
            inlet.read_with(cx, |i, cx| {
                i.composer
                    .view
                    .read(cx)
                    .editor
                    .read(cx)
                    .buffer()
                    .text()
                    .to_string()
            })
        };
        cx.simulate_keystrokes("i p u k e g / s space");
        cx.run_until_parked();
        assert_eq!(text(&inlet, cx), "ipukeg/s ");
        inlet.read_with(cx, |i, _| {
            assert!(i.inbox.open.is_none(), "I opened the inbox");
            assert!(!i.overlays.keybind_help, "the slash opened the help");
            assert!(!i.layer_open(), "a layer took the keyboard");
            assert_eq!(i.selected_channel.as_deref(), Some("301"));
        });
        let focused = cx.update(|window, cx| {
            inlet
                .read(cx)
                .composer
                .view
                .focus_handle(cx)
                .is_focused(window)
        });
        assert!(focused, "the box lost the keyboard");
        // Editing keys still edit, and Up (without Alt) is not the channel key.
        cx.simulate_keystrokes("backspace backspace up");
        assert_eq!(text(&inlet, cx), "ipukeg/");
        inlet.read_with(cx, |i, _| {
            assert_eq!(i.selected_channel.as_deref(), Some("301"))
        });
    }
}
