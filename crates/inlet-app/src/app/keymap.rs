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

use gpui::{
    actions, Action, App, Context, Entity, Focusable, InteractiveElement, KeyBinding, Window,
};
use inlet_state::ChannelId;

use crate::app::{Inlet, Screen};
use crate::desktop::Layer;
use crate::ui::chat::messages::Row as MessageRow;
use crate::ui::composer::editor::ComposerView;
use crate::ui::inbox::Panel;
use crate::ui::sidebar::dm_list::dm_rows_of;
use crate::ui::social::group_dm::Mode;

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
        ShowKeybindHelp,
        PreviousMentionChannel,
        NextMentionChannel,
        OpenSearch,
        OpenSettings,
        ToggleMemberList,
        ToggleAccountSwitcher,
        NewDirectMessage,
        JumpToOldestUnread
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
/// Fluxer's other key for the list, for layouts where the slash needs Shift.
const HELP_ALSO: Keys = Keys {
    mac: "cmd-.",
    other: "ctrl-.",
};
const MENTION_PREVIOUS: Keys = Keys {
    mac: "cmd-alt-shift-up",
    other: "ctrl-alt-shift-up",
};
const MENTION_NEXT: Keys = Keys {
    mac: "cmd-alt-shift-down",
    other: "ctrl-alt-shift-down",
};
/// Fluxer also steps through the servers with Ctrl+Tab (Control on macOS too).
const SERVER_PREVIOUS_ALSO: Keys = Keys::same("ctrl-shift-tab");
const SERVER_NEXT_ALSO: Keys = Keys::same("ctrl-tab");
const SEARCH: Keys = Keys {
    mac: "cmd-f",
    other: "ctrl-f",
};
/// Bound, but not in the list: Fluxer hides it there too.
const SETTINGS: Keys = Keys {
    mac: "cmd-,",
    other: "ctrl-,",
};
const MEMBER_LIST: Keys = Keys {
    mac: "cmd-u",
    other: "ctrl-u",
};
const ACCOUNT_SWITCHER: Keys = Keys {
    mac: "cmd-shift-o",
    other: "ctrl-shift-o",
};
const NEW_DIRECT_MESSAGE: Keys = Keys {
    mac: "cmd-shift-t",
    other: "ctrl-shift-t",
};
const OLDEST_UNREAD: Keys = Keys::same("shift-pageup");
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
    /// A second key that does the same, where Fluxer has one.
    pub also: Option<Keys>,
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
                also: None,
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
        )
        .with_also(SERVER_PREVIOUS_ALSO),
        shortcut!("server-next", Navigation, SERVER_NEXT, NextServer, || t!(
            "keys-server-next"
        ))
        .with_also(SERVER_NEXT_ALSO),
        shortcut!("scroll-up", Navigation, PAGE_UP, ScrollPageUp, || t!(
            "keys-scroll-up"
        )),
        shortcut!("scroll-down", Navigation, PAGE_DOWN, ScrollPageDown, || t!(
            "keys-scroll-down"
        )),
        shortcut!(
            "mention-previous",
            Navigation,
            MENTION_PREVIOUS,
            PreviousMentionChannel,
            || t!("keys-mention-previous")
        ),
        shortcut!(
            "mention-next",
            Navigation,
            MENTION_NEXT,
            NextMentionChannel,
            || t!("keys-mention-next")
        ),
        shortcut!(
            "oldest-unread",
            Navigation,
            OLDEST_UNREAD,
            JumpToOldestUnread,
            || t!("keys-oldest-unread")
        ),
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
        shortcut!("search", Panels, SEARCH, OpenSearch, || t!("keys-search")),
        shortcut!("member-list", Panels, MEMBER_LIST, ToggleMemberList, || t!(
            "keys-member-list"
        )),
        shortcut!(
            "account-switcher",
            Panels,
            ACCOUNT_SWITCHER,
            ToggleAccountSwitcher,
            || t!("keys-account-switcher")
        ),
        shortcut!(
            "new-direct-message",
            Panels,
            NEW_DIRECT_MESSAGE,
            NewDirectMessage,
            || t!("keys-new-direct-message")
        ),
        shortcut!("help", Panels, HELP, ShowKeybindHelp, || t!("keys-help")).with_also(HELP_ALSO),
    ]
}

impl Shortcut {
    fn with_also(mut self, also: Keys) -> Shortcut {
        self.also = Some(also);
        self
    }
}

/// One line of the help: a shortcut as a person reads it.
pub struct Row {
    pub group: Group,
    pub label: String,
    /// The keys one by one as the platform writes them: `["Ctrl", "Alt", "↑"]`.
    pub keys: Vec<String>,
    /// The second key that does the same, if there is one.
    pub also: Option<Vec<String>>,
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
            also: s.also.map(|k| key_names(k.now(), mac)),
        })
        .collect()
}

/// The names of the keys in `keys` as gpui spells them (`ctrl-alt-up`), as the platform
/// writes them.
pub fn key_names(keys: &str, mac: bool) -> Vec<String> {
    keys.split('-')
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
            "tab" => t!("keys-tab"),
            "escape" => t!("keys-escape"),
            other => other.to_uppercase(),
        })
        .collect()
}

/// Binds the shortcuts that are new here, and the second keys of the ones that have
/// one. The others are bound by their domains.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(CHANNEL_PREVIOUS.now(), PreviousChannel, None),
        KeyBinding::new(CHANNEL_NEXT.now(), NextChannel, None),
        KeyBinding::new(UNREAD_PREVIOUS.now(), PreviousUnreadChannel, None),
        KeyBinding::new(UNREAD_NEXT.now(), NextUnreadChannel, None),
        KeyBinding::new(MENTION_PREVIOUS.now(), PreviousMentionChannel, None),
        KeyBinding::new(MENTION_NEXT.now(), NextMentionChannel, None),
        KeyBinding::new(SERVER_PREVIOUS.now(), PreviousServer, None),
        KeyBinding::new(SERVER_NEXT.now(), NextServer, None),
        KeyBinding::new(SERVER_PREVIOUS_ALSO.now(), PreviousServer, None),
        KeyBinding::new(SERVER_NEXT_ALSO.now(), NextServer, None),
        KeyBinding::new(INBOX.now(), ToggleInbox, None),
        KeyBinding::new(PINS.now(), TogglePins, None),
        KeyBinding::new(UPLOAD.now(), UploadFile, None),
        KeyBinding::new(PAGE_UP.now(), ScrollPageUp, None),
        KeyBinding::new(PAGE_DOWN.now(), ScrollPageDown, None),
        KeyBinding::new(OLDEST_UNREAD.now(), JumpToOldestUnread, None),
        KeyBinding::new(SEARCH.now(), OpenSearch, None),
        KeyBinding::new(SETTINGS.now(), OpenSettings, None),
        KeyBinding::new(MEMBER_LIST.now(), ToggleMemberList, None),
        KeyBinding::new(ACCOUNT_SWITCHER.now(), ToggleAccountSwitcher, None),
        KeyBinding::new(NEW_DIRECT_MESSAGE.now(), NewDirectMessage, None),
        KeyBinding::new(HELP.now(), ShowKeybindHelp, None),
        KeyBinding::new(HELP_ALSO.now(), ShowKeybindHelp, None),
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

/// Where Ctrl+Shift+U opens the file dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UploadTarget {
    /// Nowhere: the message box is not on screen (a forum).
    Nowhere,
    /// The main message box.
    Chat,
    /// The thread panel's message box, which has the keyboard.
    ThreadPanel,
}

/// The message box an upload key is for, by the same rule as the picker keys: the thread
/// panel's when it has the keyboard, the main one otherwise, and none while a forum
/// replaces the chat (its message box is hidden).
fn upload_target(forum_open: bool, thread_composer_focused: bool) -> UploadTarget {
    if forum_open {
        UploadTarget::Nowhere
    } else if thread_composer_focused {
        UploadTarget::ThreadPanel
    } else {
        UploadTarget::Chat
    }
}

impl Inlet {
    /// Whether a shortcut that acts on the chat may act now: signed in on the main
    /// screen, with nothing over the chat that has the keyboard (a layer, a message menu
    /// or dialog) and not under the About view.
    fn shortcuts_allowed(&self) -> bool {
        self.keys_allowed_over(None)
    }

    /// Like [`Inlet::shortcuts_allowed`], but `own` may be the layer that is open: the
    /// key of a panel or a window closes it, and another panel's key swaps it.
    fn keys_allowed_over(&self, own: Option<Layer>) -> bool {
        let top = self.open_layers().topmost();
        self.screen == Screen::Main
            && self.account.is_some()
            && !self.show_about
            && (top.is_none() || top == own)
            && self.chat.actions.overlay.is_none()
    }

    /// Whether a field has the keyboard that a key like Ctrl+I or PageUp must leave to
    /// the person typing: any other text field, or a message box with text in it. Fluxer
    /// lets these keys through only from an empty message box.
    fn typing_in_a_field(&self, window: &Window, cx: &App) -> bool {
        let stack = window.context_stack();
        if stack.iter().any(|c| c.contains("TextInput")) {
            return true;
        }
        if !stack.iter().any(|c| c.contains(inlet_editor::KEY_CONTEXT)) {
            return false;
        }
        let empty =
            |view: &Entity<ComposerView>| view.read(cx).editor.read(cx).buffer().text().is_empty();
        if self
            .composer
            .view
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            return !empty(&self.composer.view);
        }
        if let Some(view) = &self.threads.composer {
            if view.focus_handle(cx).contains_focused(window, cx) {
                return !empty(view);
            }
        }
        // Any other editor (the box that edits a message): it has text.
        true
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
    /// unread (a mention, or unread messages when it is not muted). With
    /// `mentions_only` (Ctrl+Alt+Shift+Up and Down) only a channel with a mention
    /// counts.
    fn step_unread_channel(
        &mut self,
        forward: bool,
        mentions_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                    u.mentions > 0 || (!mentions_only && u.unread && !u.muted)
                })
            })
            .cloned()
        };
        if let Some(id) = next {
            self.select_channel(id, window, cx);
        }
    }

    /// Ctrl+Alt+Up and Ctrl+Alt+Down (and Ctrl+Tab): the next or previous server, the
    /// direct messages first, as the rail lists them.
    fn step_server(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        let rail: Vec<Option<String>> = std::iter::once(None)
            .chain(self.guilds.iter().map(|g| Some(g.id.clone())))
            .collect();
        let next = step(&rail, Some(&self.selected_guild), forward).cloned();
        if let Some(guild) = next {
            self.select_guild(guild, cx);
        }
    }

    /// PageUp and PageDown: the message list scrolls by most of a screen, unless
    /// something is being typed.
    fn scroll_page(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() || self.typing_in_a_field(window, cx) {
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

    /// Shift+PageUp: the list scrolls to the line where the unread messages start, when
    /// the messages loaded have it (the same as the banner's "new messages").
    fn jump_to_oldest_unread(&mut self, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() {
            return;
        }
        let divider = self
            .chat
            .view
            .rows
            .iter()
            .position(|r| matches!(r, MessageRow::Unread | MessageRow::Day { new: true, .. }));
        if let Some(divider) = divider {
            self.chat.view.divider_above = false;
            self.chat.message_list.pause_following_tail();
            self.chat.message_list.scroll_to_reveal_item(divider);
            cx.notify();
        }
    }

    /// Ctrl+I and Ctrl+P: the inbox or the pins of the open channel, opened or closed.
    /// The inbox key leaves a field that is being typed in alone.
    fn toggle_panel_key(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.keys_allowed_over(Some(Layer::Panel)) {
            return;
        }
        if panel == Panel::Inbox && self.typing_in_a_field(window, cx) {
            return;
        }
        self.toggle_panel(panel, cx);
    }

    /// Ctrl+Shift+U: the file dialog of the message box that has the keyboard.
    fn upload_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcuts_allowed() || self.open_channel_id().is_none() {
            return;
        }
        let in_panel = self
            .threads
            .composer
            .as_ref()
            .is_some_and(|v| v.focus_handle(cx).contains_focused(window, cx));
        let view = match upload_target(self.forum_is_open(), in_panel) {
            UploadTarget::Nowhere => return,
            UploadTarget::Chat => self.composer.view.clone(),
            UploadTarget::ThreadPanel => match self.threads.composer.clone() {
                Some(view) => view,
                None => return,
            },
        };
        view.update(cx, |view, cx| view.pick_files(cx));
    }

    /// Ctrl+F: the search panel, with the keyboard in its field.
    fn search_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keys_allowed_over(Some(Layer::Panel)) {
            self.open_search(window, cx);
        }
    }

    /// Ctrl+,: the settings.
    fn settings_key(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts_allowed() {
            self.open_settings(None, cx);
        }
    }

    /// Ctrl+U: the member list, shown or hidden.
    fn member_list_key(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts_allowed() {
            self.toggle_member_list(cx);
        }
    }

    /// Ctrl+Shift+O: the account switcher, opened or closed.
    fn account_switcher_key(&mut self, cx: &mut Context<Self>) {
        if self.keys_allowed_over(Some(Layer::AccountSwitcher)) {
            self.switcher.open = !self.switcher.open;
            cx.notify();
        }
    }

    /// Ctrl+Shift+T: the window for a new direct message or group.
    fn new_direct_message_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcuts_allowed() {
            self.open_group_dm(Mode::Create, window, cx);
        }
    }

    /// Whether the list of shortcuts is up (it shows over the main screen only).
    pub(crate) fn keybind_help_open(&self) -> bool {
        self.overlays.keybind_help && self.screen == Screen::Main
    }

    /// Ctrl+/ (and Ctrl+.): the list of shortcuts, opened or closed. It opens over the
    /// main screen only, and never over another layer.
    pub(crate) fn toggle_keybind_help(&mut self, cx: &mut Context<Self>) {
        if self.keybind_help_open() {
            self.overlays.keybind_help = false;
        } else if self.screen == Screen::Main && !self.show_about && !self.layer_open() {
            self.overlays.keybind_help = true;
        } else {
            return;
        }
        cx.notify();
    }

    /// Once per frame: where no message box is on screen (a server without a channel to
    /// open, or while its list loads) the keyboard is on the window, not on the hidden
    /// box, so the shortcuts still work. A channel that opens takes it back for its box
    /// (`ComposerView::set_channel`); the friends page and a forum have their own rules.
    fn keys_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Main || self.layer_open() {
            return;
        }
        if self.selected_channel.is_none() {
            if self.composer.view.focus_handle(cx).is_focused(window) {
                window.focus(&self.focus, cx);
                self.overlays.keys_parked = true;
            }
        } else if self.overlays.keys_parked && !self.forum_is_open() {
            // A channel opened (also the one the box already had, which does not take the
            // keyboard itself): the box has it, if nothing else took it meanwhile.
            self.overlays.keys_parked = false;
            if self.focus.is_focused(window) {
                let handle = self.composer.view.focus_handle(cx);
                window.focus(&handle, cx);
            }
        }
    }

    /// The window's handlers of the shortcuts above, and the demo scene that opens the
    /// help.
    pub(crate) fn keymap_keys<E: InteractiveElement>(
        &mut self,
        el: E,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> E {
        self.keys_frame(window, cx);
        #[cfg(feature = "demo")]
        self.demo_keybind_help(window, cx);
        #[cfg(feature = "demo")]
        self.demo_sudo_confirm(window, cx);
        el.on_action(
            cx.listener(|this, _: &PreviousChannel, window, cx| {
                this.step_channel(false, window, cx)
            }),
        )
        .on_action(
            cx.listener(|this, _: &NextChannel, window, cx| this.step_channel(true, window, cx)),
        )
        .on_action(cx.listener(|this, _: &PreviousUnreadChannel, window, cx| {
            this.step_unread_channel(false, false, window, cx)
        }))
        .on_action(cx.listener(|this, _: &NextUnreadChannel, window, cx| {
            this.step_unread_channel(true, false, window, cx)
        }))
        .on_action(cx.listener(|this, _: &PreviousMentionChannel, window, cx| {
            this.step_unread_channel(false, true, window, cx)
        }))
        .on_action(cx.listener(|this, _: &NextMentionChannel, window, cx| {
            this.step_unread_channel(true, true, window, cx)
        }))
        .on_action(cx.listener(|this, _: &PreviousServer, _, cx| this.step_server(false, cx)))
        .on_action(cx.listener(|this, _: &NextServer, _, cx| this.step_server(true, cx)))
        .on_action(cx.listener(|this, _: &ToggleInbox, window, cx| {
            this.toggle_panel_key(Panel::Inbox, window, cx)
        }))
        .on_action(cx.listener(|this, _: &TogglePins, window, cx| {
            this.toggle_panel_key(Panel::Pins, window, cx)
        }))
        .on_action(cx.listener(|this, _: &UploadFile, window, cx| this.upload_key(window, cx)))
        .on_action(
            cx.listener(|this, _: &ScrollPageUp, window, cx| this.scroll_page(false, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ScrollPageDown, window, cx| this.scroll_page(true, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &JumpToOldestUnread, _, cx| this.jump_to_oldest_unread(cx)),
        )
        .on_action(cx.listener(|this, _: &OpenSearch, window, cx| this.search_key(window, cx)))
        .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.settings_key(cx)))
        .on_action(cx.listener(|this, _: &ToggleMemberList, _, cx| this.member_list_key(cx)))
        .on_action(
            cx.listener(|this, _: &ToggleAccountSwitcher, _, cx| this.account_switcher_key(cx)),
        )
        .on_action(cx.listener(|this, _: &NewDirectMessage, window, cx| {
            this.new_direct_message_key(window, cx)
        }))
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
    const INSIDE_A_FIELD: [&str; 8] = [
        // Bound, but Fluxer hides it from its list too.
        "inlet_keys::OpenSettings",
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
            let bound = |action: &str, keys: &'static str| {
                let typed = [Keystroke::parse(keys).expect("keys parse")];
                keymap.bindings().any(|b| {
                    b.action().name() == action && b.match_keystrokes(&typed) == Some(false)
                })
            };
            for s in &list {
                for keys in std::iter::once(s.keys).chain(s.also) {
                    assert!(
                        bound(s.action, keys.now()),
                        "{} ({}) is not bound to {}",
                        s.id,
                        keys.now(),
                        s.action
                    );
                }
                assert!(!(s.label)().trim().is_empty(), "{} has no label", s.id);
            }
            assert!(
                bound("inlet_keys::OpenSettings", SETTINGS.now()),
                "the settings key is not bound"
            );
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
            for set in std::iter::once(s.keys).chain(s.also) {
                for keys in [set.mac, set.other] {
                    assert!(!keys.contains("secondary"), "{keys}");
                    assert!(Keystroke::parse(keys).is_ok(), "{keys} does not parse");
                }
                // Command where the others use Control, and nothing else differs
                // (Fluxer's Ctrl+Tab is Control on macOS too).
                let same = set.mac == set.other;
                assert!(
                    same || set.mac.replace("cmd", "ctrl") == set.other,
                    "{}",
                    s.id
                );
            }
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
                {"id": "303", "mention_count": 2, "last_message_id": mid(20)}
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
        cx.simulate_keystrokes(SERVER_NEXT.now());
        assert_eq!(guild(&inlet, cx).as_deref(), Some(OTHER_GUILD));
        first_channel_opens(&inlet, "401", cx);
        cx.simulate_keystrokes(SERVER_NEXT.now());
        assert_eq!(guild(&inlet, cx), None, "the direct messages come first");
        first_channel_opens(&inlet, "401", cx);
        cx.simulate_keystrokes(SERVER_PREVIOUS.now());
        assert_eq!(guild(&inlet, cx).as_deref(), Some(OTHER_GUILD));
        first_channel_opens(&inlet, "401", cx);
        cx.simulate_keystrokes(SERVER_PREVIOUS.now());
        assert_eq!(guild(&inlet, cx).as_deref(), Some(GUILD));
    }

    /// Nothing steps behind a layer: with the quick switcher open the keys leave the
    /// channel, the server, the inbox and the pins alone.
    #[gpui::test]
    fn shortcuts_do_nothing_behind_a_layer(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.update(|window, cx| inlet.update(cx, |this, cx| this.toggle_quick_switcher(window, cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes(&format!(
            "{} {} {}",
            CHANNEL_NEXT.now(),
            SERVER_NEXT.now(),
            PINS.now()
        ));
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
        cx.simulate_keystrokes(INBOX.now());
        assert_eq!(open(&inlet, cx), Some(Panel::Inbox));
        cx.simulate_keystrokes(INBOX.now());
        assert_eq!(open(&inlet, cx), None);
        cx.simulate_keystrokes(&format!("{} {}", INBOX.now(), PINS.now()));
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
        cx.simulate_keystrokes(HELP.now());
        cx.run_until_parked();
        assert!(help(&inlet, cx));
        cx.simulate_keystrokes("alt-down");
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("301"));
        cx.simulate_keystrokes(HELP.now());
        assert!(!help(&inlet, cx));
        cx.simulate_keystrokes(HELP.now());
        cx.run_until_parked();
        assert!(help(&inlet, cx));
        cx.simulate_keystrokes("escape");
        assert!(!help(&inlet, cx));
        // Over the inbox it does not open.
        cx.simulate_keystrokes(&format!("{} {}", INBOX.now(), HELP.now()));
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

    /// The new server's channel list arrives and its first channel opens, as it does a moment
    /// after a server key (the message box is on screen again and takes the keyboard).
    fn first_channel_opens(inlet: &Entity<Inlet>, id: &str, cx: &mut VisualTestContext) {
        inlet.update_in(cx, |this, window, cx| {
            this.select_channel(id.into(), window, cx)
        });
        cx.run_until_parked();
    }

    fn composer_focused(inlet: &Entity<Inlet>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| {
            inlet
                .read(cx)
                .composer
                .view
                .focus_handle(cx)
                .is_focused(window)
        })
    }

    fn window_focused(inlet: &Entity<Inlet>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| inlet.read(cx).focus.is_focused(window))
    }

    /// A server step that lands where no message box shows (here: its channels have not
    /// arrived, as in an empty server) puts the keyboard on the window, so the next
    /// server keys, Ctrl+Tab too, still work. A channel that opens gives the keyboard to
    /// its message box.
    #[gpui::test]
    fn server_keys_do_not_go_dead_in_a_server_without_a_message_box(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        let guild = |inlet: &Entity<Inlet>, cx: &mut VisualTestContext| {
            inlet.read_with(cx, |i, _| i.selected_guild.clone())
        };
        assert!(composer_focused(&inlet, cx));
        cx.simulate_keystrokes(SERVER_NEXT.now());
        assert_eq!(guild(&inlet, cx).as_deref(), Some(OTHER_GUILD));
        assert!(window_focused(&inlet, cx), "the keyboard is on the window");
        assert!(!composer_focused(&inlet, cx));
        // No channel opened: the next keys still work.
        cx.simulate_keystrokes(SERVER_NEXT_ALSO.now());
        assert_eq!(guild(&inlet, cx), None);
        cx.simulate_keystrokes(SERVER_PREVIOUS_ALSO.now());
        assert_eq!(guild(&inlet, cx).as_deref(), Some(OTHER_GUILD));
        cx.simulate_keystrokes(SERVER_PREVIOUS.now());
        assert_eq!(guild(&inlet, cx).as_deref(), Some(GUILD));
        // A channel opens: its message box has the keyboard again.
        first_channel_opens(&inlet, "301", cx);
        assert!(
            composer_focused(&inlet, cx),
            "the box takes the keyboard back"
        );
        assert!(!window_focused(&inlet, cx));
    }

    /// Ctrl+Alt+Shift+Down goes to the next channel with a mention, not to one that is
    /// only unread.
    #[gpui::test]
    fn mention_keys_step_through_the_channels_with_a_mention(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.simulate_keystrokes(MENTION_NEXT.now());
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
        // Only one channel has a mention: nowhere else to go.
        cx.simulate_keystrokes(MENTION_NEXT.now());
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
        cx.simulate_keystrokes(MENTION_PREVIOUS.now());
        assert_eq!(open_channel(&inlet, cx).as_deref(), Some("303"));
    }

    /// Ctrl+. opens the list like Ctrl+/ (Fluxer's key where the slash needs Shift), and
    /// neither opens it on the sign-in screen.
    #[gpui::test]
    fn the_help_has_a_second_key_and_stays_off_the_sign_in_screen(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        let help = |inlet: &Entity<Inlet>, cx: &mut VisualTestContext| {
            inlet.read_with(cx, |i, _| i.keybind_help_open())
        };
        cx.simulate_keystrokes(HELP_ALSO.now());
        cx.run_until_parked();
        assert!(help(&inlet, cx), "Ctrl+. did not open the list");
        cx.simulate_keystrokes(HELP_ALSO.now());
        assert!(!help(&inlet, cx));
        inlet.update(cx, |this, cx| {
            this.screen = Screen::SignIn;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(HELP.now());
        cx.simulate_keystrokes(HELP_ALSO.now());
        assert!(!help(&inlet, cx), "the list opened on the sign-in screen");
        assert!(!inlet.read_with(cx, |i, _| i.layer_open()));
    }

    /// The picker keys do nothing behind the list of shortcuts.
    #[gpui::test]
    fn picker_keys_do_nothing_behind_the_help(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.simulate_keystrokes(HELP.now());
        cx.run_until_parked();
        assert!(inlet.read_with(cx, |i, _| i.keybind_help_open()));
        cx.simulate_keystrokes(&format!(
            "{} {} {}",
            EMOJI_PICKER.now(),
            GIF_PICKER.now(),
            STICKER_PICKER.now()
        ));
        inlet.read_with(cx, |i, _| {
            assert!(
                !i.composer.pickers.is_open(),
                "a picker opened behind the list"
            );
        });
        // With the list closed the same key opens the picker.
        cx.simulate_keystrokes(HELP.now());
        cx.simulate_keystrokes(EMOJI_PICKER.now());
        cx.run_until_parked();
        assert!(inlet.read_with(cx, |i, _| i.composer.pickers.is_open()));
    }

    /// Ctrl+I and PageUp leave a message box with text in it alone, as Fluxer does, and
    /// work from an empty one; the pins key works either way.
    #[gpui::test]
    fn ctrl_i_waits_for_an_empty_message_box(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.simulate_keystrokes("a b");
        cx.simulate_keystrokes(INBOX.now());
        assert_eq!(inlet.read_with(cx, |i, _| i.inbox.open), None);
        cx.simulate_keystrokes(PINS.now());
        assert_eq!(inlet.read_with(cx, |i, _| i.inbox.open), Some(Panel::Pins));
        cx.simulate_keystrokes("escape backspace backspace");
        cx.simulate_keystrokes(INBOX.now());
        assert_eq!(inlet.read_with(cx, |i, _| i.inbox.open), Some(Panel::Inbox));
    }

    /// Ctrl+Shift+U goes to the message box that has the keyboard, and to none while a
    /// forum replaces the chat.
    #[test]
    fn upload_goes_to_the_box_that_has_the_keyboard() {
        assert_eq!(upload_target(false, false), UploadTarget::Chat);
        assert_eq!(upload_target(false, true), UploadTarget::ThreadPanel);
        assert_eq!(upload_target(true, false), UploadTarget::Nowhere);
        assert_eq!(upload_target(true, true), UploadTarget::Nowhere);
    }

    /// Ctrl+F, Ctrl+, Ctrl+Shift+O and Ctrl+Shift+T open the search, the settings, the
    /// account switcher and the new direct message window, and the switcher's key
    /// closes it again.
    #[gpui::test]
    fn the_window_keys_open_their_windows(cx: &mut TestAppContext) {
        let (inlet, cx) = rig(cx);
        cx.simulate_keystrokes(SETTINGS.now());
        assert!(inlet.read_with(cx, |i, _| i.settings.view.is_some()));
        cx.simulate_keystrokes("escape");
        assert!(inlet.read_with(cx, |i, _| i.settings.view.is_none()));
        cx.simulate_keystrokes(ACCOUNT_SWITCHER.now());
        assert!(inlet.read_with(cx, |i, _| i.switcher.open));
        cx.simulate_keystrokes(ACCOUNT_SWITCHER.now());
        assert!(!inlet.read_with(cx, |i, _| i.switcher.open));
        cx.simulate_keystrokes(NEW_DIRECT_MESSAGE.now());
        cx.run_until_parked();
        assert!(inlet.read_with(cx, |i, _| i.social.group_dm.is_some()));
        // Nothing opens behind it.
        cx.simulate_keystrokes(SETTINGS.now());
        assert!(inlet.read_with(cx, |i, _| i.settings.view.is_none()));
    }
}
