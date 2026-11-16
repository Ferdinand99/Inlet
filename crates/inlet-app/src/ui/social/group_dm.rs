// SPDX-License-Identifier: AGPL-3.0-or-later
//! The social window, as Fluxer's modals for a person or a group: start a DM or a group
//! DM with friends (the + in the DM list, as Fluxer's `CreateDMModal`), add friends to a
//! group (`AddFriendsToGroupModal`, with its invite link), the group's settings
//! (`EditGroupModal`: icon, name, mature content, and the people with their actions), a
//! nickname (`ChangeFriendNicknameModal`, `ChangeGroupDMNicknameModal`; see `nickname`),
//! a group's invites (`GroupInvitesModal`; see `group_invites`) and the list of
//! communities to invite someone to (see `community`). A layer, like the other social
//! windows.
//!
//! Creating a group and adding someone may need a CAPTCHA, which the protocol solves
//! (the window says "still working" while it does). When it cannot be passed, or the
//! hourly limit is reached, the window says so and stays as it is: nothing is sent again
//! on its own, and a long wait shows as minutes or hours.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, Focusable, FontWeight, PathPromptOptions,
    SharedString, Subscription, Window,
};
use inlet_protocol::api::dms::MAX_GROUP_DM_RECIPIENTS;
use inlet_protocol::models::channel_type;
use inlet_state::relationships::Kind;
use inlet_state::{AccountState, ChannelId, UserId};

use super::group_invites::InviteList;
use super::nickname::MAX_NICKNAME_CHARS;
use super::{person_name, person_user, Confirm, Done, SocialIcon, SocialPart, Then};
use crate::app::Inlet;
use crate::backend::social::relationships::{Cmd, GroupChange, Icon, MAX_ICON_BYTES};
use crate::text_input::{InputEvent, TextInput};
use crate::theme;
use crate::ui::overlays::modal::modal;
use crate::ui::widgets::button;

/// What the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A new DM (one friend picked) or group DM.
    Create,
    /// Adds friends to a group.
    Add { channel: ChannelId },
    /// The group's settings.
    Settings { channel: ChannelId },
    /// The private nickname for a friend.
    FriendNickname { user: UserId },
    /// Someone's nickname in a group.
    GroupNickname { channel: ChannelId, user: UserId },
    /// The group's invites (owner only).
    Invites { channel: ChannelId },
    /// The communities to invite someone to.
    InviteToCommunity { user: UserId },
}

/// What the picked group icon will become when the settings are saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum IconChoice {
    /// The icon stays as it is.
    #[default]
    Keep,
    Remove,
    /// A picture picked from disk, checked when it is sent.
    File(PathBuf),
}

/// A group the account has already, with the people it is about to create.
#[derive(Clone, Debug, PartialEq)]
pub struct Duplicate {
    pub channel: ChannelId,
    pub name: String,
    pub last_active: String,
}

/// A question that takes the place of the friend list until it is answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Panel {
    /// "You already have a group with these people": the groups, and a way on.
    Duplicates(Vec<Duplicate>),
    /// Some people cannot be added: who and why, and the user IDs that can be.
    Unaddable {
        rows: Vec<(String, String)>,
        addable: Vec<String>,
    },
}

/// The group DM window.
pub struct GroupDmWindow {
    pub mode: Mode,
    pub search: Entity<TextInput>,
    /// The group's name field (settings), or the nickname field.
    pub name: Option<Entity<TextInput>>,
    pub picked: Vec<UserId>,
    /// People that stay picked (the person of the DM a group is made from).
    pub sticky: Vec<UserId>,
    /// A lasting line: the CAPTCHA, the hourly limit or another failure.
    pub message: Option<String>,
    /// "Still working on it", while a slow call runs.
    pub hint: Option<String>,
    pub panel: Option<Panel>,
    pub icon: IconChoice,
    /// The mature-content switch as it stands in the settings.
    pub mature: bool,
    /// The invite link made in the add window (it lasts a day).
    pub link: Option<String>,
    /// The link was copied.
    pub copied: bool,
    /// The group's invites (invites window).
    pub invites: Option<InviteList>,
    /// The window to come back to when this one closes.
    pub back: Option<Mode>,
    _observe: Subscription,
    _submit: Option<Subscription>,
}

impl GroupDmWindow {
    /// The key its calls go by (see `relationships::Cmd::key`).
    pub fn key(&self) -> String {
        match self.mode {
            Mode::Create => "group-dm".into(),
            Mode::Add { channel }
            | Mode::Settings { channel }
            | Mode::GroupNickname { channel, .. } => format!("group:{channel}"),
            Mode::FriendNickname { user } => format!("nick:{user}"),
            Mode::Invites { channel } => format!("invites:{channel}"),
            Mode::InviteToCommunity { user } => format!("community:{user}:"),
        }
    }

    /// Whether the call named `key` is this window's: its own key, the invite link of
    /// the add window, a revoke in the invites window, or a call to any community (whose
    /// key starts with the window's).
    pub fn owns(&self, key: &str) -> bool {
        match self.mode {
            Mode::Add { channel } => key == self.key() || key == format!("invite-link:{channel}"),
            Mode::Invites { channel } => {
                key == self.key() || key.starts_with(&format!("invite-revoke:{channel}:"))
            }
            Mode::InviteToCommunity { .. } => key.starts_with(&self.key()),
            _ => key == self.key(),
        }
    }
}

/// The friends the window offers: friends matching `query`, not already in `group`,
/// by name.
pub fn friend_choices(
    state: &AccountState,
    query: &str,
    group: Option<ChannelId>,
) -> Vec<(UserId, String)> {
    let query = query.trim().to_lowercase();
    let members: Vec<UserId> = group
        .and_then(|g| state.channel(g))
        .map(|c| {
            c.recipients
                .iter()
                .filter_map(|u| UserId::parse(&u.id))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<(UserId, String)> = state
        .relationships()
        .of_kind(Kind::Friend)
        .filter(|p| !members.contains(&p.id))
        .map(|p| (p.id, person_name(state, p.id)))
        .filter(|(_, name)| query.is_empty() || name.to_lowercase().contains(&query))
        .collect();
    out.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    out.truncate(super::friends::MAX_ROWS);
    out
}

/// How someone is named in a group: their nickname there, then the friend nickname, then
/// the display name (Fluxer's `getNickname` with the channel).
pub fn group_person_name(state: &AccountState, channel: ChannelId, user: UserId) -> String {
    let nick = state
        .channel(channel)
        .and_then(|c| c.group_nick(&user.to_string()))
        .map(crate::model::clean_name)
        .filter(|n| !n.is_empty());
    match nick {
        Some(n) => n,
        None if user == state.me_id() => crate::model::clean_name(state.me().display_name()),
        None => person_name(state, user),
    }
}

/// The name a group shows when it has none of its own: its people (by their nickname in
/// the group when they have one).
pub fn default_group_name(state: &AccountState, channel: ChannelId) -> String {
    let Some(c) = state.channel(channel) else {
        return String::new();
    };
    c.recipients
        .iter()
        .map(|u| match c.group_nick(&u.id) {
            Some(nick) => crate::model::clean_name(nick),
            None => crate::model::clean_name(u.display_name()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name a group shows: its own, or its people's.
pub fn group_name(state: &AccountState, channel: ChannelId) -> String {
    let Some(c) = state.channel(channel) else {
        return String::new();
    };
    match c.name.as_deref().filter(|n| !n.trim().is_empty()) {
        Some(n) => crate::model::clean_name(n),
        None => default_group_name(state, channel),
    }
}

/// The owner of a group.
pub fn group_owner(state: &AccountState, channel: ChannelId) -> Option<UserId> {
    state
        .channel(channel)
        .and_then(|c| c.group_owner_id())
        .and_then(UserId::parse)
}

/// One person in a group, as the settings list them.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub user: UserId,
    pub name: String,
    pub owner: bool,
    pub bot: bool,
}

/// The group's people, the account first (Fluxer lists everyone).
pub fn group_members(state: &AccountState, channel: ChannelId) -> Vec<Member> {
    let Some(c) = state.channel(channel) else {
        return Vec::new();
    };
    let owner = group_owner(state, channel);
    let me = state.me_id();
    let mut out = vec![Member {
        user: me,
        name: group_person_name(state, channel, me),
        owner: owner == Some(me),
        bot: false,
    }];
    out.extend(c.recipients.iter().filter_map(|u| {
        let user = UserId::parse(&u.id)?;
        Some(Member {
            user,
            name: group_person_name(state, channel, user),
            owner: owner == Some(user),
            bot: u.bot,
        })
    }));
    out
}

/// The groups the account already has with exactly these people, newest activity first
/// (Fluxer's `getDuplicateGroupDMChannels`).
pub fn duplicate_groups(state: &AccountState, people: &[UserId]) -> Vec<Duplicate> {
    let mut wanted: Vec<UserId> = people.to_vec();
    wanted.sort();
    wanted.dedup();
    let mut found: Vec<(i64, Duplicate)> = state
        .private_channels()
        .into_iter()
        .filter(|c| c.kind == channel_type::GROUP_DM && !c.recipients.is_empty())
        .filter_map(|c| {
            let id = ChannelId::parse(&c.id)?;
            let mut have: Vec<UserId> = c
                .recipients
                .iter()
                .filter_map(|u| UserId::parse(&u.id))
                .collect();
            have.sort();
            have.dedup();
            if have != wanted {
                return None;
            }
            let last = c.last_message_id.as_deref().and_then(ChannelId::parse);
            let active = last.map_or(id.timestamp_ms(), |l| l.timestamp_ms());
            let last_active = match last {
                Some(l) => crate::ui::chat::time::full(l.timestamp_ms()),
                None => t!("social-group-no-activity"),
            };
            Some((
                active,
                Duplicate {
                    channel: id,
                    name: group_name(state, id),
                    last_active,
                },
            ))
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.into_iter().map(|(_, d)| d).collect()
}

/// What saving the group's settings changes: the name when it differs from the group's
/// (an empty one clears it), the icon when one was picked or removed, and the mature
/// switch when the account owns the group and moved it. `None` for a group the state does
/// not know.
pub fn settings_change(
    state: &AccountState,
    channel: ChannelId,
    name: &str,
    icon: IconChoice,
    mature: bool,
) -> Option<GroupChange> {
    let current = state.channel(channel)?;
    let name = name.trim();
    let mut change = GroupChange::default();
    if name != current.name.as_deref().map_or("", str::trim) {
        change.name = Some((!name.is_empty()).then(|| name.to_owned()));
    }
    match icon {
        IconChoice::Keep => {}
        IconChoice::Remove => change.icon = Some(Icon::Remove),
        IconChoice::File(path) => change.icon = Some(Icon::File(path)),
    }
    let owner = group_owner(state, channel) == Some(state.me_id());
    if owner && mature != current.group_mature() {
        change.nsfw = Some(mature);
    }
    Some(change)
}

/// The sentence for Fluxer's reason a person cannot be added to a group.
pub fn unaddable_reason(reason: &str) -> String {
    match reason {
        "unknown_user" => t!("social-unaddable-unknown"),
        "blocked" => t!("social-unaddable-blocked"),
        "not_friends" => t!("social-unaddable-not-friends"),
        "group_dm_add_disabled" => t!("social-unaddable-disabled"),
        _ => t!("social-unaddable-other"),
    }
}

/// Most people a group takes in all (the account included).
const GROUP_SIZE: usize = MAX_GROUP_DM_RECIPIENTS + 1;

/// How many more people a group with `others` other people takes. The state keeps at
/// most 32 recipients of a group, so a bigger one looks emptier than it is: the server
/// has the last word and answers a full group with an error the window shows.
pub fn room_in_group(others: usize) -> usize {
    GROUP_SIZE.saturating_sub(others + 1)
}

/// What the header button of a DM does.
#[derive(Clone, Copy)]
enum HeaderAction {
    /// A group with this friend, who stays picked.
    Create(UserId),
    /// Friends for the open group.
    Add(ChannelId),
}

/// The header button of a DM, as Fluxer's: with a friend it starts a group with them,
/// in a group that has room it adds friends.
pub fn header_button(this: &Inlet, cx: &mut Context<Inlet>) -> Option<AnyElement> {
    this.selected_guild.is_none().then_some(())?;
    let channel = ChannelId::parse(this.selected_channel.as_deref()?)?;
    let state = this.account.as_ref()?.read(cx);
    let c = state.channel(channel)?;
    let (action, id) = match c.kind {
        channel_type::DM => {
            let user = UserId::parse(&c.recipients.first()?.id)?;
            if state.relationships().kind(user) != Some(Kind::Friend) {
                return None;
            }
            (HeaderAction::Create(user), "create-group-button")
        }
        channel_type::GROUP_DM if room_in_group(c.recipients.len()) > 0 => {
            (HeaderAction::Add(channel), "add-friends-button")
        }
        _ => return None,
    };
    Some(
        div()
            .id(id)
            .flex_none()
            .p(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover_bg()))
            .child(SocialIcon::UserPlus.el(24., theme::text_muted()))
            .on_click(cx.listener(move |this, _, window, cx| match action {
                HeaderAction::Create(user) => {
                    this.open_group_dm_from(Mode::Create, vec![user], None, window, cx)
                }
                HeaderAction::Add(channel) => this.open_group_dm(Mode::Add { channel }, window, cx),
            }))
            .into_any_element(),
    )
}

/// A message line over the buttons: a failure (framed in red) or a hint.
pub fn notice_line(text: String, failure: bool) -> AnyElement {
    div()
        .debug_selector(move || {
            if failure {
                "group-dm-message".into()
            } else {
                "group-dm-hint".into()
            }
        })
        .px_3()
        .py_2()
        .rounded(px(6.))
        .bg(theme::bg_app())
        .border_1()
        .border_color(if failure {
            theme::danger()
        } else {
            theme::accent_border()
        })
        .text_size(px(14.))
        .text_color(if failure {
            theme::text_strong()
        } else {
            theme::text_muted()
        })
        .child(text)
        .into_any_element()
}

/// The 20 px title of a window.
pub fn title_line(text: String) -> AnyElement {
    div()
        .text_size(px(20.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text_strong())
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(text)
        .into_any_element()
}

/// A muted caption in capitals over a field.
pub fn caption(text: String) -> AnyElement {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text_muted())
        .child(text.to_uppercase())
        .into_any_element()
}

/// A small text button on a row (Nickname, Make owner, Remove, Copy).
pub fn row_button(
    id: impl Into<SharedString>,
    label: String,
    danger: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex_none()
        .px(px(8.))
        .h(px(26.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if danger {
            theme::danger()
        } else {
            theme::text_muted()
        })
        .bg(theme::bg_app())
        .hover(|s| s.bg(theme::hover_bg()).text_color(theme::text_strong()))
        .child(label)
}

/// A switch, on or off.
pub fn switch(id: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .w(px(40.))
        .h(px(22.))
        .flex_none()
        .rounded_full()
        .px(px(3.))
        .flex()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .when(on, |d| d.justify_end().bg(theme::brand()))
        .when(!on, |d| d.bg(theme::text_muted()))
        .child(div().size(px(16.)).rounded_full().bg(theme::text_strong()))
}

impl Inlet {
    /// Opens the window.
    pub(crate) fn open_group_dm(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_group_dm_from(mode, Vec::new(), None, window, cx);
    }

    /// Opens a window with `picked` people already chosen (they stay chosen) and, when it
    /// closes, the window `back` to come back to.
    pub(crate) fn open_group_dm_from(
        &mut self,
        mode: Mode,
        picked: Vec<UserId>,
        back: Option<Mode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match mode {
            Mode::InviteToCommunity { .. } => t!("social-community-search"),
            _ => t!("social-group-dm-search"),
        };
        let search = cx.new(|cx| TextInput::new(placeholder, cx).max_chars(100));
        let observe = cx.observe(&search, |_, _, cx| cx.notify());
        let (name, mature) = self.window_fields(mode, cx);
        let submit = name
            .as_ref()
            .map(|n| cx.subscribe(n, |this, _, _: &InputEvent, cx| this.group_dm_submit(cx)));
        let focus = match (&name, mode) {
            (Some(n), _) => Some(n.focus_handle(cx)),
            (None, Mode::Create | Mode::Add { .. } | Mode::InviteToCommunity { .. }) => {
                Some(search.focus_handle(cx))
            }
            (None, _) => None,
        };
        if let Some(focus) = focus {
            self.focus_layer_field(&focus, window, cx);
        }
        self.social.dm_menu = None;
        self.social.group_dm = Some(GroupDmWindow {
            mode,
            search,
            name,
            picked: picked.clone(),
            sticky: picked,
            message: None,
            hint: None,
            panel: None,
            icon: IconChoice::Keep,
            mature,
            link: None,
            copied: false,
            invites: None,
            back,
            _observe: observe,
            _submit: submit,
        });
        if let Mode::Invites { channel } = mode {
            self.load_group_invites(channel, cx);
        }
        cx.notify();
    }

    /// The text field of a window and the mature switch, as the account state has them.
    fn window_fields(
        &self,
        mode: Mode,
        cx: &mut Context<Self>,
    ) -> (Option<Entity<TextInput>>, bool) {
        // What the account state says, as owned text, before any entity is made.
        let (current, placeholder, mature, max) = {
            let Some(state) = self.account.as_ref().map(|a| a.read(cx)) else {
                return (None, false);
            };
            match mode {
                Mode::Settings { channel } => {
                    let placeholder = match default_group_name(state, channel) {
                        n if n.is_empty() => t!("social-group-name-placeholder"),
                        n => n,
                    };
                    (
                        state
                            .channel(channel)
                            .and_then(|c| c.name.clone())
                            .unwrap_or_default(),
                        placeholder,
                        state.channel(channel).is_some_and(|c| c.group_mature()),
                        100,
                    )
                }
                Mode::FriendNickname { user } => (
                    state
                        .relationships()
                        .get(user)
                        .and_then(|p| p.nickname.clone())
                        .unwrap_or_default(),
                    person_user(state, user)
                        .map(|u| crate::model::clean_name(u.display_name()))
                        .unwrap_or_default(),
                    false,
                    MAX_NICKNAME_CHARS,
                ),
                Mode::GroupNickname { channel, user } => (
                    state
                        .channel(channel)
                        .and_then(|c| c.group_nick(&user.to_string()))
                        .unwrap_or_default()
                        .to_owned(),
                    if user == state.me_id() {
                        crate::model::clean_name(state.me().display_name())
                    } else {
                        person_user(state, user)
                            .map(|u| crate::model::clean_name(u.display_name()))
                            .unwrap_or_default()
                    },
                    false,
                    MAX_NICKNAME_CHARS,
                ),
                _ => return (None, false),
            }
        };
        let input = cx.new(|cx| {
            TextInput::new(placeholder, cx)
                .max_chars(max)
                .with_text(current)
        });
        (Some(input), mature)
    }

    /// The field the window's keyboard goes to, when it has one.
    pub(crate) fn group_dm_field(&self, cx: &gpui::App) -> Option<gpui::FocusHandle> {
        let w = self.social.group_dm.as_ref()?;
        match w.mode {
            Mode::Settings { .. } | Mode::FriendNickname { .. } | Mode::GroupNickname { .. } => {
                w.name.as_ref().map(|n| n.focus_handle(cx))
            }
            Mode::Create | Mode::Add { .. } | Mode::InviteToCommunity { .. } => {
                Some(w.search.focus_handle(cx))
            }
            Mode::Invites { .. } => None,
        }
    }

    /// Enter in the window's name or nickname field saves it.
    fn group_dm_submit(&mut self, cx: &mut Context<Self>) {
        match self.social.group_dm.as_ref().map(|w| w.mode) {
            Some(Mode::Settings { channel }) => self.group_dm_save(channel, cx),
            Some(Mode::FriendNickname { .. } | Mode::GroupNickname { .. }) => {
                self.save_nickname(cx)
            }
            _ => {}
        }
    }

    /// Closes the window, and opens the one it came from.
    pub(crate) fn group_dm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let back = self.social.group_dm.take().and_then(|w| w.back);
        if let Some(mode) = back {
            self.open_group_dm(mode, window, cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_pick(&mut self, user: UserId, cx: &mut Context<Self>) {
        let room = self.pick_room(cx);
        if let Some(w) = self.social.group_dm.as_mut() {
            w.panel = None;
            if let Some(i) = w.picked.iter().position(|u| *u == user) {
                if !w.sticky.contains(&user) {
                    w.picked.remove(i);
                }
            } else if w.picked.len() < room {
                w.picked.push(user);
            }
        }
        cx.notify();
    }

    /// How many people the window can take: a new group's limit, or what a group has
    /// room for.
    pub(super) fn pick_room(&self, cx: &gpui::App) -> usize {
        match self.social.group_dm.as_ref().map(|w| w.mode) {
            Some(Mode::Add { channel }) => {
                let have = self
                    .account
                    .as_ref()
                    .and_then(|a| a.read(cx).channel(channel))
                    .map_or(0, |c| c.recipients.len());
                room_in_group(have)
            }
            _ => MAX_GROUP_DM_RECIPIENTS,
        }
    }

    /// Create (or open the DM, with one friend picked), or add the picked friends.
    pub(super) fn group_dm_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.social.group_dm.as_ref() else {
            return;
        };
        let (mode, people) = (w.mode, w.picked.clone());
        let picked: Vec<String> = people.iter().map(|u| u.to_string()).collect();
        let cmd = match (mode, picked.as_slice()) {
            (Mode::Create, []) => Cmd::CreateGroupDm {
                recipients: Vec::new(),
            },
            (Mode::Create, [_]) => {
                self.social.group_dm = None;
                return self.open_dm_with(people[0], window, cx);
            }
            (Mode::Create, _) => {
                // A group with these very people: ask before making another.
                let dupes = self
                    .account
                    .as_ref()
                    .map(|a| duplicate_groups(a.read(cx), &people))
                    .unwrap_or_default();
                if !dupes.is_empty() {
                    if let Some(w) = self.social.group_dm.as_mut() {
                        w.panel = Some(Panel::Duplicates(dupes));
                    }
                    cx.notify();
                    return;
                }
                Cmd::CreateGroupDm { recipients: picked }
            }
            (Mode::Add { channel }, [_, ..]) => Cmd::AddRecipients {
                channel_id: channel.to_string(),
                user_ids: picked,
            },
            _ => return,
        };
        self.group_dm_send(cmd, cx);
    }

    /// Sends a group call and clears the last answer; a new try is never sent by itself.
    fn group_dm_send(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        if let Some(w) = self.social.group_dm.as_mut() {
            w.message = None;
            w.hint = None;
            w.panel = None;
        }
        self.social_action(cmd, cx);
    }

    /// "Create new group" over a duplicate, or "Create without them" over people who
    /// cannot be added.
    pub(super) fn group_dm_confirm_panel(&mut self, cx: &mut Context<Self>) {
        let Some(w) = self.social.group_dm.as_ref() else {
            return;
        };
        let recipients = match &w.panel {
            Some(Panel::Duplicates(_)) => w.picked.iter().map(|u| u.to_string()).collect(),
            Some(Panel::Unaddable { addable, .. }) => addable.clone(),
            None => return,
        };
        self.group_dm_send(Cmd::CreateGroupDm { recipients }, cx);
    }

    /// Saves the group's settings: only what changed goes out.
    pub(crate) fn group_dm_save(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let Some(w) = self.social.group_dm.as_ref() else {
            return;
        };
        let text = w
            .name
            .as_ref()
            .map(|n| n.read(cx).text().trim().to_owned())
            .unwrap_or_default();
        let (icon, mature) = (w.icon.clone(), w.mature);
        let Some(state) = self.account.as_ref().map(|a| a.read(cx)) else {
            return;
        };
        let Some(change) = settings_change(state, channel, &text, icon, mature) else {
            return;
        };
        if change == GroupChange::default() {
            // Nothing to send: the window is done.
            self.social.group_dm = None;
            cx.notify();
            return;
        }
        self.edit_group_dm(
            channel,
            change,
            Some(Done::notice(t!("social-group-updated"), Then::Close)),
            cx,
        );
    }

    /// Asks for a picture and holds it as the group's new icon until the settings are
    /// saved.
    fn group_dm_pick_icon(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(t!("social-group-icon-prompt"))),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            // Its size is looked at off the UI thread; its type when it is sent.
            let too_large = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        std::fs::metadata(&path)
                            .map(|m| m.len() > MAX_ICON_BYTES)
                            .unwrap_or(false)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Only while the same group's settings are still open (not after a close
                // or an account switch).
                if let Some(w) = this
                    .social
                    .group_dm
                    .as_mut()
                    .filter(|w| w.mode == Mode::Settings { channel })
                {
                    if too_large {
                        w.message = Some(t!("social-icon-too-large"));
                    } else {
                        w.message = None;
                        w.icon = IconChoice::File(path);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_group_dm(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(mode) = self.social.group_dm.as_ref().map(|w| w.mode) else {
            return div().into_any_element();
        };
        let parts = match mode {
            Mode::Create | Mode::Add { .. } => self.pick_parts(mode, cx),
            Mode::Settings { channel } => self.settings_parts(channel, cx),
            Mode::FriendNickname { .. } | Mode::GroupNickname { .. } => {
                self.nickname_parts(mode, cx)
            }
            Mode::Invites { channel } => self.invites_parts(channel, cx),
            Mode::InviteToCommunity { user } => self.community_parts(user, cx),
        };
        let width = match mode {
            Mode::Invites { .. } => 680.,
            _ => 460.,
        };
        let card = modal("group-dm", width, parts);
        self.social_keys(SocialPart::GroupDm, card, cx)
            .into_any_element()
    }

    /// The failure line and the hint of the window, for the parts above the buttons.
    pub(super) fn window_notices(&self) -> Vec<AnyElement> {
        let Some(w) = self.social.group_dm.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(m) = w.message.clone() {
            out.push(notice_line(m, true));
        } else if let Some(h) = w.hint.clone() {
            out.push(notice_line(h, false));
        }
        out
    }

    /// The Close button, which goes back to the window the person came from.
    pub(super) fn close_button(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        button(id, t!("social-close"), false)
            .on_click(cx.listener(|this, _, window, cx| this.group_dm_close(window, cx)))
            .into_any_element()
    }

    /// Create a group, or add friends: the question, or the friends to pick, and for an
    /// added group the invite link.
    fn pick_parts(&mut self, mode: Mode, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(w) = self.social.group_dm.as_ref() else {
            return Vec::new();
        };
        let (search, picked, panel, link, copied) = (
            w.search.clone(),
            w.picked.clone(),
            w.panel.clone(),
            w.link.clone(),
            w.copied,
        );
        let busy = self.social_busy(&w.key())
            || matches!(mode, Mode::Add { channel } if self.social_busy(&format!("invite-link:{channel}")));
        let query = search.read(cx).text().to_owned();
        let room = self.pick_room(cx);
        let group = match mode {
            Mode::Add { channel } => Some(channel),
            _ => None,
        };
        let (choices, title) = match self.account.as_ref().map(|a| a.read(cx)) {
            Some(state) => (
                friend_choices(state, &query, group),
                match mode {
                    Mode::Add { channel } => {
                        t!("social-group-add-title", name = group_name(state, channel))
                    }
                    _ => t!("social-new-dm-title"),
                },
            ),
            None => (Vec::new(), String::new()),
        };
        if let Some(panel) = panel {
            // The question has its own title, as Fluxer's confirmations.
            let title = match &panel {
                Panel::Duplicates(_) => t!("social-duplicate-title"),
                Panel::Unaddable { .. } => t!("social-unaddable-title"),
            };
            let mut parts = vec![title_line(title)];
            parts.extend(self.panel_parts(panel, cx));
            return parts;
        }
        let mut parts = vec![title_line(title)];
        let left = room.saturating_sub(picked.len());
        parts.push(
            div()
                .text_size(px(14.))
                .text_color(theme::text_muted())
                .child(match mode {
                    Mode::Create => t!("social-new-dm-body", count = left),
                    _ if room == 0 => t!("social-group-full"),
                    _ => t!("social-group-add-body", count = left),
                })
                .into_any_element(),
        );
        parts.push(search.into_any_element());
        let rows: Vec<AnyElement> = choices
            .iter()
            .map(|(user, name)| self.render_choice(*user, name, picked.contains(user), cx))
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .py_4()
                .flex()
                .justify_center()
                .text_color(theme::text_muted())
                .child(t!("social-group-no-friends"))
        });
        parts.push(
            div()
                .id("group-dm-friends")
                .max_h(px(320.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(rows)
                .children(empty)
                .into_any_element(),
        );
        if let Mode::Add { channel } = mode {
            parts.push(self.invite_link_section(channel, link, copied, cx));
        }
        parts.extend(self.window_notices());
        let go_label = match (mode, picked.len()) {
            (Mode::Create, 1) => t!("social-new-dm-open"),
            (Mode::Create, _) => t!("social-new-group-create"),
            _ => t!("social-group-add"),
        };
        let ready = !busy
            && match mode {
                Mode::Create => true,
                _ => !picked.is_empty() && room > 0,
            };
        let actions = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .child(self.close_button("group-dm-cancel", cx))
            .child(
                button(
                    "group-dm-go",
                    if busy { t!("social-sending") } else { go_label },
                    true,
                )
                .when(!ready, |b| b.opacity(0.5))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if ready {
                        this.group_dm_go(window, cx)
                    }
                })),
            );
        parts.push(actions.into_any_element());
        parts
    }

    /// The question that takes the place of the friend list.
    fn panel_parts(&mut self, panel: Panel, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = |text: String| {
            div()
                .text_size(px(14.))
                .text_color(theme::text_muted())
                .child(text)
                .into_any_element()
        };
        let mut parts = Vec::new();
        let (confirm_label, can_confirm) = match &panel {
            Panel::Duplicates(groups) => {
                parts.push(muted(t!("social-duplicate-body")));
                let rows: Vec<AnyElement> = groups
                    .iter()
                    .map(|g| {
                        let channel = g.channel;
                        div()
                            .id(SharedString::from(format!("group-dupe-{channel}")))
                            .px_2()
                            .py_2()
                            .rounded(px(6.))
                            .flex()
                            .flex_col()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover_bg()))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(theme::text_strong())
                                    .child(g.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(g.last_active.clone()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.social.group_dm = None;
                                this.select_channel(channel.to_string(), window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect();
                parts.push(
                    div()
                        .id("group-dm-duplicates")
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(rows)
                        .into_any_element(),
                );
                (t!("social-duplicate-create"), true)
            }
            Panel::Unaddable { rows, addable } => {
                parts.push(muted(t!("social-unaddable-body")));
                let lines: Vec<AnyElement> = rows
                    .iter()
                    .map(|(name, reason)| {
                        div()
                            .text_size(px(14.))
                            .text_color(theme::text_strong())
                            .child(format!("{name}: {reason}"))
                            .into_any_element()
                    })
                    .collect();
                parts.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(lines)
                        .into_any_element(),
                );
                parts.push(muted(if addable.is_empty() {
                    t!("social-unaddable-none-left")
                } else {
                    t!("social-unaddable-rest", count = addable.len())
                }));
                (t!("social-unaddable-create"), !addable.is_empty())
            }
        };
        parts.extend(self.window_notices());
        let busy = self
            .social
            .group_dm
            .as_ref()
            .is_some_and(|w| self.social_busy(&w.key()));
        let mut actions = div().flex().flex_row().justify_end().gap_2().child(
            button("group-dm-panel-cancel", t!("common-cancel"), false).on_click(cx.listener(
                |this, _, _, cx| {
                    if let Some(w) = this.social.group_dm.as_mut() {
                        w.panel = None;
                    }
                    cx.notify();
                },
            )),
        );
        if can_confirm {
            actions = actions.child(
                button(
                    "group-dm-panel-go",
                    if busy {
                        t!("social-sending")
                    } else {
                        confirm_label
                    },
                    true,
                )
                .when(busy, |b| b.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !busy {
                        this.group_dm_confirm_panel(cx)
                    }
                })),
            );
        }
        parts.push(actions.into_any_element());
        parts
    }

    /// A friend to pick, with a box.
    fn render_choice(
        &mut self,
        user: UserId,
        name: &str,
        picked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let record = self
            .account
            .as_ref()
            .and_then(|a| person_user(a.read(cx), user));
        let avatar = record.as_ref().and_then(crate::backend::media::user_avatar);
        let picture = self.user_avatar(avatar.as_ref(), name, 32., false, cx);
        div()
            .id(SharedString::from(format!("group-pick-{user}")))
            .h(px(44.))
            .px_2()
            .rounded(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover_bg()))
            .child(picture)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(theme::text_strong())
                    .child(name.to_owned()),
            )
            .child(
                div()
                    .size(px(22.))
                    .flex_none()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if picked {
                        theme::brand()
                    } else {
                        theme::text_muted()
                    })
                    .when(picked, |d| d.bg(theme::brand()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(picked, |d| {
                        d.child(SocialIcon::Check.el(14., theme::on_brand()))
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_pick(user, cx)))
            .into_any_element()
    }

    /// The settings (Fluxer's "Edit group"): icon, name and mature content, then the
    /// people with what the account may do to them, and the group's other windows.
    fn settings_parts(&mut self, channel: ChannelId, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(w) = self.social.group_dm.as_ref() else {
            return Vec::new();
        };
        let (name, icon_choice, mature) = (w.name.clone(), w.icon.clone(), w.mature);
        let busy = self.social_busy(&w.key());
        let (me, me_owner, members, icon_hash) = {
            let Some(state) = self.account.as_ref().map(|a| a.read(cx)) else {
                return Vec::new();
            };
            let me = state.me_id();
            (
                me,
                group_owner(state, channel) == Some(me),
                group_members(state, channel),
                state
                    .channel(channel)
                    .and_then(|c| c.extra.get("icon"))
                    .and_then(|v| v.as_str())
                    .filter(|h| !h.is_empty() && h.len() <= 128)
                    .map(str::to_owned),
            )
        };
        let has_icon = match icon_choice {
            IconChoice::Keep => icon_hash.is_some(),
            IconChoice::Remove => false,
            IconChoice::File(_) => true,
        };
        let full = room_in_group(members.len().saturating_sub(1)) == 0;
        let mut out: Vec<AnyElement> = vec![title_line(t!("social-edit-group"))];

        // The icon: what it is now, and what the buttons do to it.
        out.push(caption(t!("social-group-icon")));
        let picture = match (&icon_choice, icon_hash) {
            (IconChoice::Keep, Some(hash)) => {
                self.group_dm_icon_image(&channel.to_string(), &hash, 64., cx)
            }
            _ => None,
        };
        let picture = match picture {
            Some(img) => img.into_any_element(),
            None => div()
                .size(px(64.))
                .flex_none()
                .rounded_full()
                .bg(theme::brand())
                .flex()
                .items_center()
                .justify_center()
                .child(SocialIcon::Friends.el(30., theme::on_brand()))
                .into_any_element(),
        };
        let status = match &icon_choice {
            IconChoice::Keep => None,
            IconChoice::Remove => Some(t!("social-group-icon-removing")),
            IconChoice::File(path) => Some(t!(
                "social-group-icon-chosen",
                file = crate::model::clean_name(
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            )),
        };
        out.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(picture)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    button(
                                        "group-icon-change",
                                        if has_icon {
                                            t!("social-group-icon-change")
                                        } else {
                                            t!("social-group-icon-upload")
                                        },
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.group_dm_pick_icon(channel, cx),
                                    )),
                                )
                                .children(has_icon.then(|| {
                                    button(
                                        "group-icon-remove",
                                        t!("social-group-icon-remove"),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            if let Some(w) = this.social.group_dm.as_mut() {
                                                w.icon = IconChoice::Remove;
                                            }
                                            cx.notify();
                                        },
                                    ))
                                })),
                        )
                        .children(status.map(|s| {
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(s)
                        })),
                )
                .into_any_element(),
        );

        // The name.
        out.push(caption(t!("social-group-name")));
        out.extend(name.map(|n| n.into_any_element()));

        // Mature content: the owner's switch.
        if me_owner {
            out.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_strong())
                                    .child(t!("social-group-mature")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(t!("social-group-mature-body")),
                            ),
                    )
                    .child(switch("group-mature", mature).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(w) = this.social.group_dm.as_mut() {
                                w.mature = !w.mature;
                            }
                            cx.notify();
                        },
                    )))
                    .into_any_element(),
            );
        }

        // The people.
        out.push(caption(t!("social-group-members", count = members.len())));
        let rows: Vec<AnyElement> = members
            .into_iter()
            .map(|m| self.member_row(channel, m, me, me_owner, cx))
            .collect();
        out.push(
            div()
                .id("group-members")
                .max_h(px(200.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element(),
        );

        // The group's other windows, and leaving.
        let mut links = div().flex().flex_row().flex_wrap().gap_2();
        if !full {
            links = links.child(
                button("group-add-friends", t!("social-group-add"), false).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_group_dm_from(
                            Mode::Add { channel },
                            Vec::new(),
                            Some(Mode::Settings { channel }),
                            window,
                            cx,
                        )
                    },
                )),
            );
        }
        if me_owner {
            links = links.child(
                button("group-invites", t!("social-group-invites"), false).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_group_dm_from(
                            Mode::Invites { channel },
                            Vec::new(),
                            Some(Mode::Settings { channel }),
                            window,
                            cx,
                        )
                    },
                )),
            );
        }
        links = links.child(
            button("group-leave", t!("social-leave-group"), false)
                .bg(theme::danger())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let name = this
                        .account
                        .as_ref()
                        .map(|a| group_name(a.read(cx), channel))
                        .unwrap_or_default();
                    this.social.confirm = Some(Confirm::LeaveGroup { channel, name });
                    cx.notify();
                })),
        );
        out.push(links.into_any_element());
        out.extend(self.window_notices());
        out.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(self.close_button("group-dm-cancel", cx))
                .child(
                    button(
                        "group-save",
                        if busy {
                            t!("social-sending")
                        } else {
                            t!("social-save")
                        },
                        true,
                    )
                    .when(busy, |b| b.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.group_dm_save(channel, cx)
                        }
                    })),
                )
                .into_any_element(),
        );
        out
    }

    /// One person in the settings, with what the account may do: its own nickname, and
    /// as the owner the others' nickname, ownership and removal.
    fn member_row(
        &mut self,
        channel: ChannelId,
        m: Member,
        me: UserId,
        me_owner: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let user = m.user;
        let name = m.name.clone();
        let mut row = div()
            .id(SharedString::from(format!("group-member-{user}")))
            .min_h(px(36.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(m.name.clone()),
            );
        if m.owner {
            row = row.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(t!("social-group-owner")),
            );
        }
        if user == me || me_owner {
            row = row.child(
                row_button(
                    format!("group-nick-{user}"),
                    if user == me {
                        t!("social-group-my-nickname")
                    } else {
                        t!("social-group-nickname")
                    },
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_group_dm_from(
                        Mode::GroupNickname { channel, user },
                        Vec::new(),
                        Some(Mode::Settings { channel }),
                        window,
                        cx,
                    )
                })),
            );
        }
        if me_owner && user != me {
            if !m.bot {
                let name = name.clone();
                row = row.child(
                    row_button(
                        format!("group-owner-{user}"),
                        t!("social-group-make-owner"),
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.social.confirm = Some(Confirm::MakeOwner {
                            channel,
                            user,
                            name: name.clone(),
                        });
                        cx.notify();
                    })),
                );
            }
            row = row.child(
                row_button(
                    format!("group-remove-{user}"),
                    t!("social-group-remove"),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.social.confirm = Some(Confirm::RemoveFromGroup {
                        channel,
                        user,
                        name: name.clone(),
                    });
                    cx.notify();
                })),
            );
        }
        row.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::Ready;
    use serde_json::json;

    fn state(channels: serde_json::Value) -> AccountState {
        let ready: Ready = serde_json::from_value(json!({
            "session_id": "s", "user": {"id": "1", "username": "me"}, "guilds": [],
            "users": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"},
                      {"id": "4", "username": "cy"}],
            "private_channels": channels,
            "relationships": [{"id": "2", "type": 1}, {"id": "3", "type": 1}, {"id": "4", "type": 3}],
            "read_states": [], "user_guild_settings": []
        }))
        .unwrap();
        AccountState::from_ready_at(&ready, 1)
    }

    #[test]
    fn the_window_offers_friends_not_in_the_group() {
        let s = state(json!([{"id": "100", "type": 3, "name": "  ",
                "recipients": [{"id": "2", "username": "bo"}]}]));
        let all: Vec<String> = friend_choices(&s, "", None)
            .into_iter()
            .map(|c| c.1)
            .collect();
        assert_eq!(all, ["ada", "bo"], "friends only, by name");
        let add: Vec<String> = friend_choices(&s, "", Some(ChannelId(100)))
            .into_iter()
            .map(|c| c.1)
            .collect();
        assert_eq!(add, ["ada"], "not the people already in the group");
        assert_eq!(friend_choices(&s, "B", None).len(), 1);
        assert_eq!(
            group_name(&s, ChannelId(100)),
            "bo",
            "a blank name shows the people"
        );
    }

    #[test]
    fn people_are_named_by_their_nickname_in_the_group_first() {
        let s = state(
            json!([{"id": "100", "type": 3, "name": null, "owner_id": "1",
                "nicks": {"2": "Captain", "1": "Skipper"},
                "recipients": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"}]}]),
        );
        let g = ChannelId(100);
        assert_eq!(group_person_name(&s, g, UserId(2)), "Captain");
        assert_eq!(
            group_person_name(&s, g, UserId(3)),
            "ada",
            "no nickname: the name"
        );
        assert_eq!(
            group_person_name(&s, g, UserId(1)),
            "Skipper",
            "the account too"
        );
        assert_eq!(
            group_name(&s, g),
            "Captain, ada",
            "an unnamed group shows them"
        );
        let members = group_members(&s, g);
        assert_eq!(
            members
                .iter()
                .map(|m| (m.user.get(), m.owner))
                .collect::<Vec<_>>(),
            [(1, true), (2, false), (3, false)],
            "the account first, and who owns the group"
        );
    }

    #[test]
    fn a_new_group_with_the_same_people_finds_the_old_ones_newest_first() {
        let at = |ms: u64| (ms << 22).to_string();
        let s = state(json!([
            {"id": at(10), "type": 3, "name": "Old", "last_message_id": at(50),
             "recipients": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"}]},
            {"id": at(20), "type": 3, "name": "Newer", "last_message_id": at(90),
             "recipients": [{"id": "3", "username": "ada"}, {"id": "2", "username": "bo"}]},
            {"id": at(30), "type": 3, "name": "Other",
             "recipients": [{"id": "2", "username": "bo"}, {"id": "4", "username": "cy"}]},
            {"id": at(40), "type": 3, "name": "Three", "last_message_id": at(99),
             "recipients": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"},
                            {"id": "4", "username": "cy"}]}
        ]));
        let found = duplicate_groups(&s, &[UserId(3), UserId(2)]);
        assert_eq!(
            found.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["Newer", "Old"],
            "the same people in any order, newest activity first"
        );
        assert!(duplicate_groups(&s, &[UserId(2), UserId(3), UserId(4), UserId(5)]).is_empty());
        assert_eq!(
            found[1].last_active,
            crate::ui::chat::time::full(ChannelId(50 << 22).timestamp_ms())
        );
        let none = state(json!([{"id": at(10), "type": 3, "name": "Quiet",
            "recipients": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"}]}]));
        assert_eq!(
            duplicate_groups(&none, &[UserId(2), UserId(3)])[0].last_active,
            t!("social-group-no-activity")
        );
    }

    #[test]
    fn saving_the_settings_changes_only_what_differs() {
        let owned = state(
            json!([{"id": "100", "type": 3, "name": "Crew", "owner_id": "1",
            "recipients": [{"id": "2", "username": "bo"}]}]),
        );
        let joined = state(
            json!([{"id": "100", "type": 3, "name": "Crew", "owner_id": "2",
            "recipients": [{"id": "2", "username": "bo"}]}]),
        );
        let g = ChannelId(100);
        let change = |s: &AccountState, name: &str, icon: IconChoice, mature: bool| {
            settings_change(s, g, name, icon, mature).unwrap()
        };
        // Nothing moved: nothing to send.
        assert_eq!(
            change(&owned, " Crew ", IconChoice::Keep, false),
            GroupChange::default()
        );
        // A new name, a cleared name, a removed and a new icon.
        assert_eq!(
            change(&owned, "Navy", IconChoice::Keep, false).name,
            Some(Some("Navy".into()))
        );
        assert_eq!(
            change(&owned, "  ", IconChoice::Keep, false).name,
            Some(None),
            "an empty name clears it"
        );
        assert_eq!(
            change(&owned, "Crew", IconChoice::Remove, false).icon,
            Some(Icon::Remove)
        );
        let file = PathBuf::from("pic.png");
        assert_eq!(
            change(&owned, "Crew", IconChoice::File(file.clone()), false).icon,
            Some(Icon::File(file))
        );
        // The mature switch is the owner's: a member's move of it is not sent.
        assert_eq!(
            change(&owned, "Crew", IconChoice::Keep, true).nsfw,
            Some(true)
        );
        assert_eq!(change(&joined, "Crew", IconChoice::Keep, true).nsfw, None);
        // A group the state does not know.
        assert_eq!(
            settings_change(&owned, ChannelId(7), "x", IconChoice::Keep, false),
            None
        );
    }

    #[test]
    fn every_reason_a_person_cannot_be_added_has_its_own_sentence() {
        let reasons = [
            "unknown_user",
            "blocked",
            "not_friends",
            "group_dm_add_disabled",
            "something_new",
        ];
        let texts: std::collections::HashSet<String> =
            reasons.iter().map(|r| unaddable_reason(r)).collect();
        assert_eq!(texts.len(), reasons.len());
    }
}
