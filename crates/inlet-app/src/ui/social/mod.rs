// SPDX-License-Identifier: AGPL-3.0-or-later
//! Social: presence and status (`presence`), the friends page (`friends`, `pending`,
//! `blocked`), adding a friend (`add_friend`), group DMs and the windows of a person or
//! a group (`group_dm`; nicknames in `nickname`, a group's invite links in
//! `group_invites`, inviting someone to a community in `community`) and personal notes
//! (`notes`). The DM list itself is in `ui/sidebar/dm_list.rs`.
//!
//! The add-friend window, the group DM window, the DM menu and the confirmations are
//! layers of their own (`desktop::Layer::{AddFriend, GroupDm, DmMenu, SocialConfirm}`):
//! the topmost has the keyboard, Escape closes it, and typing never reaches the
//! composer behind it.

pub mod add_friend;
pub mod blocked;
pub mod community;
pub mod friends;
pub mod group_dm;
pub mod group_invites;
pub mod nickname;
pub mod notes;
pub mod pending;
pub mod presence;

use std::collections::{HashMap, HashSet};

use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, Hsla, Pixels, Point, SharedString,
    Subscription, Svg, Window,
};
use inlet_state::{AccountState, ChannelId, UserId};

use crate::app::{Inlet, Screen};
use crate::backend;
use crate::backend::social::presence::Ev as PresenceEv;
use crate::backend::social::relationships::{self as rel, GroupChange, GroupDm};
use crate::theme;
use crate::ui::overlays::status_picker::Picker;

/// This domain's view state.
#[derive(Default)]
pub struct State {
    pub presence: presence::PresenceState,
    /// The status picker over the user panel, while it is open.
    pub picker: Option<Picker>,
    /// The friends page: its tab and search.
    pub friends: friends::FriendsPage,
    /// Calls running now (see `relationships::Cmd::key`): their buttons wait.
    pub busy: HashSet<String>,
    /// The last failure of a friend or DM action, until it is dismissed.
    pub notice: Option<String>,
    /// The add-friend window.
    pub add_friend: Option<add_friend::AddFriend>,
    /// The group DM window (new group, add people, group settings).
    pub group_dm: Option<group_dm::GroupDmWindow>,
    /// The menu of a DM row (right click).
    pub dm_menu: Option<DmMenu>,
    /// A question before a lasting action (remove a friend, block, leave a group).
    pub confirm: Option<Confirm>,
    /// What happens when a call that was asked for answers with success, by call key.
    pub done: HashMap<String, Done>,
    /// DMs this client is closing: their deletion is not reported as a lost channel.
    pub closing: HashSet<String>,
    /// The DM last open in the DM list, which the DM list opens again (Fluxer's home
    /// button), until the Friends row is picked.
    pub last_dm: Option<String>,
    /// The DM list's rows, until something they show changes (see `dm_rows_changed`).
    pub dm_rows: Option<Vec<crate::ui::sidebar::dm_list::DmRow>>,
    /// Input in the window ends "away" at once.
    _input: Option<Subscription>,
}

/// The menu of a DM row.
#[derive(Clone, Debug, PartialEq)]
pub struct DmMenu {
    pub channel: ChannelId,
    pub at: Point<Pixels>,
}

/// A question before a lasting action.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    RemoveFriend {
        user: UserId,
        name: String,
    },
    Block {
        user: UserId,
        name: String,
    },
    LeaveGroup {
        channel: ChannelId,
        name: String,
    },
    /// Hand a group to someone else.
    MakeOwner {
        channel: ChannelId,
        user: UserId,
        name: String,
    },
    RemoveFromGroup {
        channel: ChannelId,
        user: UserId,
        name: String,
    },
    RevokeInvite {
        channel: ChannelId,
        code: String,
    },
}

/// What a successful call leads to: a line for the person, and what the window does.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub notice: Option<String>,
    pub then: Then,
}

impl Done {
    pub fn notice(text: String, then: Then) -> Self {
        Done {
            notice: Some(text),
            then,
        }
    }
}

/// What the group window does when its call has gone through.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    Nothing,
    /// The window closes (and the one it came from opens).
    Close,
    /// The group's invites are read again.
    ReloadInvites(ChannelId),
}

impl State {
    /// The state with the idle watch running.
    pub fn new(cx: &mut Context<Inlet>) -> Self {
        let mut state = State::default();
        state.presence.start(cx);
        state._input = Some(cx.observe_keystrokes(|this, _, _, cx| {
            if this.social.presence.idle.input() {
                this.sync_presence(cx);
                cx.notify();
            }
        }));
        state
    }

    /// Sign-out or a backend restart: the picker and every social window close, and the
    /// next session sends its presence again.
    pub fn reset(&mut self) {
        self.picker = None;
        self.presence.reset();
        self.close_windows();
        self.friends = Default::default();
    }

    /// Another account is shown: the friend and DM windows, their waiting calls and the
    /// last failure were the old account's.
    pub fn close_windows(&mut self) {
        self.closing.clear();
        self.done.clear();
        self.last_dm = None;
        self.dm_rows = None;
        self.busy.clear();
        self.notice = None;
        self.add_friend = None;
        self.group_dm = None;
        self.dm_menu = None;
        self.confirm = None;
    }
}

/// Phosphor icons (MIT) of this domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocialIcon {
    Friends,
    Notes,
    Message,
    Plus,
    Close,
    Check,
    UserMinus,
    Block,
    Pin,
    Pencil,
    Leave,
    UserPlus,
}

impl SocialIcon {
    fn data(self) -> &'static [u8] {
        match self {
            SocialIcon::Friends => include_bytes!("../../../assets/icons/users-three.svg"),
            SocialIcon::Notes => include_bytes!("../../../assets/icons/notebook.svg"),
            SocialIcon::Message => include_bytes!("../../../assets/icons/chat-circle.svg"),
            SocialIcon::Plus => include_bytes!("../../../assets/icons/plus.svg"),
            SocialIcon::Close => include_bytes!("../../../assets/icons/x.svg"),
            SocialIcon::Check => include_bytes!("../../../assets/icons/check.svg"),
            SocialIcon::UserMinus => include_bytes!("../../../assets/icons/user-minus.svg"),
            SocialIcon::Block => include_bytes!("../../../assets/icons/prohibit.svg"),
            SocialIcon::Pin => include_bytes!("../../../assets/icons/push-pin.svg"),
            SocialIcon::Pencil => include_bytes!("../../../assets/icons/pencil-simple.svg"),
            SocialIcon::Leave => include_bytes!("../../../assets/icons/sign-out.svg"),
            SocialIcon::UserPlus => include_bytes!("../../../assets/icons/user-plus.svg"),
        }
    }

    /// The icon, `size` square, in `color`.
    pub fn el(self, size: f32, color: Hsla) -> Svg {
        gpui::svg()
            .data(self.data())
            .size(px(size))
            .flex_none()
            .text_color(color)
    }
}

/// How a person is named in the friends list and the DM list: the friend nickname,
/// then the display name, then the username; cleaned.
pub fn person_name(state: &AccountState, user: UserId) -> String {
    let p = state.relationships().get(user);
    if let Some(nick) = p.and_then(|p| p.nickname.as_deref()) {
        return crate::model::clean_name(nick);
    }
    let u = p.and_then(|p| p.user.as_ref()).or_else(|| state.user(user));
    match u {
        Some(u) => crate::model::clean_name(u.display_name()),
        None => user.to_string(),
    }
}

/// The username (`name` or `name#0001`) under a person's name.
pub fn person_tag(state: &AccountState, user: UserId) -> String {
    let u = state
        .relationships()
        .get(user)
        .and_then(|p| p.user.as_ref())
        .or_else(|| state.user(user));
    match u {
        Some(u) if !u.discriminator.is_empty() && u.discriminator != "0" => format!(
            "{}#{}",
            crate::model::clean_name(&u.username),
            crate::model::clean_name(&u.discriminator)
        ),
        Some(u) => crate::model::clean_name(&u.username),
        None => String::new(),
    }
}

/// The user record of a person, from the relationship or the user cache.
pub fn person_user(state: &AccountState, user: UserId) -> Option<inlet_protocol::User> {
    state
        .relationships()
        .get(user)
        .and_then(|p| p.user.clone())
        .or_else(|| state.user(user).cloned())
}

/// A person's shown status (`online`, `idle`, `dnd`, `offline`).
pub fn person_status(state: &AccountState, user: UserId) -> &'static str {
    match state.presences().status(user) {
        "online" => "online",
        "idle" => "idle",
        "dnd" => "dnd",
        _ => "offline",
    }
}

/// Fluxer's BOT and SYSTEM tags, for a user that is one.
pub fn user_tag(user: &inlet_protocol::User) -> Option<AnyElement> {
    let label = if user.system {
        t!("message-system-tag")
    } else if user.bot {
        t!("message-bot-tag")
    } else {
        return None;
    };
    Some(
        div()
            .flex_none()
            .px(px(5.6))
            .min_h(px(15.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(theme::brand())
            .text_size(px(10.))
            .line_height(px(15.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::on_brand())
            .child(label)
            .into_any_element(),
    )
}

/// A round 36 px action button (Fluxer's friend row buttons).
pub fn round_action(
    id: impl Into<SharedString>,
    icon: SocialIcon,
    danger: bool,
) -> gpui::Stateful<gpui::Div> {
    let color = if danger {
        theme::danger()
    } else {
        theme::text_muted()
    };
    div()
        .id(gpui::ElementId::Name(id.into()))
        .size(px(36.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(theme::bg_app())
        .hover(|s| s.bg(theme::hover_bg()))
        .child(icon.el(20., color))
}

impl Inlet {
    /// Sends a friend or DM action and marks its buttons busy until it answers. A
    /// second click while it runs sends nothing.
    pub(crate) fn social_action(&mut self, cmd: rel::Cmd, cx: &mut Context<Self>) {
        let key = cmd.key();
        if !self.social.busy.insert(key) {
            return;
        }
        self.backend.send(cmd);
        cx.notify();
    }

    /// Sends an action whose success has consequences: a line for the person, and what
    /// the window does. The line and the follow-up are dropped when the call fails.
    pub(crate) fn social_action_done(&mut self, cmd: rel::Cmd, done: Done, cx: &mut Context<Self>) {
        let key = cmd.key();
        if self.social.busy.contains(&key) {
            return;
        }
        self.social.done.insert(key, done);
        self.social_action(cmd, cx);
    }

    /// Whether a call whose key starts with `prefix` runs.
    pub(crate) fn social_busy_prefix(&self, prefix: &str) -> bool {
        self.social.busy.iter().any(|k| k.starts_with(prefix))
    }

    /// A line for the person where they are looking: over the chat, or on the friends
    /// page when no chat is open.
    pub(crate) fn social_say(&mut self, text: String) {
        if self.friends_page_shown() {
            self.social.notice = Some(text);
        } else {
            self.chat.notice = Some(text);
        }
    }

    /// The group DM that is open, when `user` is in it (or is the account): the group,
    /// and whether the account owns it.
    fn open_group_of(&self, user: UserId, cx: &gpui::App) -> Option<(ChannelId, bool)> {
        let state = self.account.as_ref()?.read(cx);
        let channel = ChannelId::parse(self.selected_channel.as_deref()?)?;
        let c = state
            .channel(channel)
            .filter(|c| c.kind == inlet_protocol::models::channel_type::GROUP_DM)?;
        let inside = user == state.me_id()
            || c.recipients
                .iter()
                .any(|r| UserId::parse(&r.id) == Some(user));
        inside.then(|| {
            (
                channel,
                group_dm::group_owner(state, channel) == Some(state.me_id()),
            )
        })
    }

    /// The relationship items of the user menu for `user`, as Fluxer's: message, the
    /// friend's nickname, what the account may do in an open group (nickname, ownership,
    /// removal), the invite to a community, then add, accept, cancel or remove the
    /// friend, then block or unblock. For the account itself only its nickname in an
    /// open group.
    pub(crate) fn relationship_actions(
        &self,
        user: UserId,
        cx: &gpui::App,
    ) -> Vec<(&'static str, String, RelationshipAction)> {
        use inlet_state::relationships::Kind;
        let Some(state) = self.account.as_ref().map(|a| a.read(cx)) else {
            return Vec::new();
        };
        let group = self.open_group_of(user, cx);
        if user == state.me_id() {
            return group
                .map(|_| {
                    (
                        "user-menu-my-group-nickname",
                        t!("social-group-my-nickname-item"),
                        RelationshipAction::MyGroupNickname,
                    )
                })
                .into_iter()
                .collect();
        }
        let kind = state.relationships().kind(user);
        let mut out = Vec::new();
        if kind != Some(Kind::Blocked) {
            out.push((
                "user-menu-message",
                t!("social-message"),
                RelationshipAction::Message,
            ));
        }
        if kind == Some(Kind::Friend) {
            out.push((
                "user-menu-friend-nickname",
                t!("social-friend-nickname-item"),
                RelationshipAction::ChangeFriendNickname,
            ));
        }
        if let Some((_, true)) = group {
            if !state.user(user).is_some_and(|u| u.bot) {
                out.push((
                    "user-menu-make-owner",
                    t!("social-group-make-owner-item"),
                    RelationshipAction::MakeGroupOwner,
                ));
            }
            out.push((
                "user-menu-group-nickname",
                t!("social-group-nickname-item"),
                RelationshipAction::GroupNickname,
            ));
            out.push((
                "user-menu-remove-from-group",
                t!("social-group-remove-item"),
                RelationshipAction::RemoveFromGroup,
            ));
        }
        if self.can_invite_to_community(user, cx) {
            out.push((
                "user-menu-invite-community",
                t!("social-community-item"),
                RelationshipAction::InviteToCommunity,
            ));
        }
        match kind {
            None => out.push((
                "user-menu-add-friend",
                t!("social-add-friend"),
                RelationshipAction::AddFriend,
            )),
            Some(Kind::Incoming) => out.push((
                "user-menu-accept",
                t!("social-accept-request"),
                RelationshipAction::Accept,
            )),
            Some(Kind::Outgoing) => out.push((
                "user-menu-cancel",
                t!("social-cancel-request"),
                RelationshipAction::CancelRequest,
            )),
            Some(Kind::Friend) => out.push((
                "user-menu-remove-friend",
                t!("social-remove-friend"),
                RelationshipAction::RemoveFriend,
            )),
            Some(Kind::Blocked) => {}
        }
        out.push(if kind == Some(Kind::Blocked) {
            (
                "user-menu-unblock",
                t!("social-unblock"),
                RelationshipAction::Unblock,
            )
        } else {
            (
                "user-menu-block",
                t!("social-block"),
                RelationshipAction::Block,
            )
        });
        out
    }

    /// Runs a relationship item of the user menu; removing a friend and blocking ask
    /// first.
    pub(crate) fn run_relationship_action(
        &mut self,
        action: RelationshipAction,
        user: UserId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .account
            .as_ref()
            .map(|a| person_name(a.read(cx), user))
            .unwrap_or_default();
        let user_id = user.to_string();
        match action {
            RelationshipAction::Message => self.open_dm_with(user, window, cx),
            RelationshipAction::AddFriend => {
                self.social_action(rel::Cmd::SendRequest { user_id }, cx)
            }
            RelationshipAction::Accept => self.social_action(rel::Cmd::Accept { user_id }, cx),
            RelationshipAction::CancelRequest | RelationshipAction::Unblock => {
                self.social_action(rel::Cmd::Remove { user_id }, cx)
            }
            RelationshipAction::RemoveFriend => {
                self.social.confirm = Some(Confirm::RemoveFriend { user, name })
            }
            RelationshipAction::Block => self.social.confirm = Some(Confirm::Block { user, name }),
            RelationshipAction::ChangeFriendNickname => self.open_friend_nickname(user, window, cx),
            RelationshipAction::InviteToCommunity => {
                self.open_invite_to_community(user, window, cx)
            }
            RelationshipAction::MyGroupNickname | RelationshipAction::GroupNickname => {
                if let Some((channel, _)) = self.open_group_of(user, cx) {
                    self.open_group_dm(group_dm::Mode::GroupNickname { channel, user }, window, cx);
                }
            }
            RelationshipAction::MakeGroupOwner => {
                if let Some((channel, true)) = self.open_group_of(user, cx) {
                    self.social.confirm = Some(Confirm::MakeOwner {
                        channel,
                        user,
                        name,
                    });
                }
            }
            RelationshipAction::RemoveFromGroup => {
                if let Some((channel, true)) = self.open_group_of(user, cx) {
                    self.social.confirm = Some(Confirm::RemoveFromGroup {
                        channel,
                        user,
                        name,
                    });
                }
            }
        }
        cx.notify();
    }

    /// The channel as the account state knows it (the server's own type and data).
    pub(crate) fn state_channel(
        &self,
        channel: ChannelId,
        cx: &gpui::App,
    ) -> Option<Box<inlet_protocol::Channel>> {
        let state = self.account.as_ref()?.read(cx);
        state.channel(channel).cloned().map(Box::new)
    }

    /// Closes a DM or leaves a group DM. Nothing is sent for a channel the state does
    /// not know; the protocol refuses anything but a DM or group DM.
    pub(crate) fn close_dm(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let Some(ch) = self.state_channel(channel, cx) else {
            return;
        };
        // Its CHANNEL_DELETE closes it quietly (no "was deleted" line).
        self.social.closing.insert(channel.to_string());
        self.social_action(rel::Cmd::CloseDm { channel: ch }, cx);
    }

    /// Changes a group DM: its name, icon, nicknames, mature switch or owner. `done` says
    /// what a success leads to.
    pub(crate) fn edit_group_dm(
        &mut self,
        channel: ChannelId,
        change: GroupChange,
        done: Option<Done>,
        cx: &mut Context<Self>,
    ) {
        let Some(ch) = self.state_channel(channel, cx) else {
            return;
        };
        let cmd = rel::Cmd::EditGroupDm {
            channel: ch,
            change,
        };
        match done {
            Some(done) => self.social_action_done(cmd, done, cx),
            None => self.social_action(cmd, cx),
        }
    }

    /// Whether the call named `key` runs.
    pub(crate) fn social_busy(&self, key: &str) -> bool {
        self.social.busy.contains(key)
    }

    /// The DM list's rows are worked out again when channels, read states (a new message
    /// moves a DM up), relationships, presences or users change.
    pub(crate) fn dm_rows_changed(&mut self, changes: &inlet_state::Changes) {
        if changes.reset
            || !changes.channels.is_empty()
            || changes.read_state
            || changes.relationships
            || changes.presence
            || changes.users
        {
            self.social.dm_rows = None;
        }
    }

    /// Whether the friends page fills the middle: the DM list is up and no DM is open.
    pub(crate) fn friends_page_shown(&self) -> bool {
        self.screen == Screen::Main
            && self.selected_guild.is_none()
            && self.selected_channel.is_none()
    }

    /// The Friends row: the friends page shows, and the open DM closes.
    pub(crate) fn show_friends(&mut self, cx: &mut Context<Self>) {
        self.social.last_dm = None;
        if self.selected_guild.is_some() {
            self.select_guild(None, cx);
            return;
        }
        if self.selected_channel.take().is_some() {
            self.threads_leave_channel(None, cx);
            self.clear_messages();
        }
        cx.notify();
    }

    /// Once per frame: the keyboard never stays in the composer while the friends page
    /// hides it; and the demo scenes open their windows.
    pub(crate) fn social_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui::Focusable;
        if self.friends_page_shown()
            && !self.layer_open()
            && self.composer.view.focus_handle(cx).is_focused(window)
        {
            window.focus(&self.focus, cx);
        }
        #[cfg(feature = "demo")]
        self.demo_social(window, cx);
    }

    /// Demo screenshots (CI): the social scenes set their state once the DM list is in.
    #[cfg(feature = "demo")]
    fn demo_social(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::demo::scenes::{current, Scene};
        use std::sync::atomic::{AtomicBool, Ordering};
        static DONE: AtomicBool = AtomicBool::new(false);
        let scene = current();
        if std::env::var_os("INLET_DEMO").is_none()
            || !matches!(
                scene,
                Scene::FriendsPending
                    | Scene::GroupDmCaptcha
                    | Scene::DmBlocked
                    | Scene::MediaErrors
                    | Scene::GroupSettings
                    | Scene::GroupInvites
                    | Scene::GroupAdd
                    | Scene::GroupUnaddable
                    | Scene::FriendNickname
                    | Scene::InviteCommunity
            )
            || self.account.is_none()
            || self.screen != Screen::Main
            || self.selected_guild.is_some()
            || self.channels.is_empty()
            || DONE.swap(true, Ordering::Relaxed)
        {
            return;
        }
        match scene {
            Scene::FriendsPending => self.social.friends.tab = friends::Tab::Pending,
            Scene::GroupDmCaptcha => {
                self.open_group_dm(group_dm::Mode::Create, window, cx);
                let picked: Vec<UserId> = ["ada", "grace"]
                    .iter()
                    .filter_map(|n| UserId::parse(&crate::demo::user_id(n)))
                    .collect();
                if let Some(w) = self.social.group_dm.as_mut() {
                    w.picked = picked;
                    w.message = Some(t!("social-group-dm-captcha"));
                }
            }
            Scene::GroupSettings | Scene::GroupInvites | Scene::GroupAdd => {
                if let Some(channel) = ChannelId::parse(&crate::demo::snowflake("dm-group")) {
                    self.demo_group_window(scene, channel, window, cx);
                }
            }
            Scene::GroupUnaddable => {
                self.open_group_dm(group_dm::Mode::Create, window, cx);
                let id = |n: &str| UserId::parse(&crate::demo::user_id(n));
                if let (Some(w), Some(ada)) = (self.social.group_dm.as_mut(), id("ada")) {
                    w.picked = vec![ada];
                    w.panel = Some(group_dm::Panel::Unaddable {
                        rows: vec![(
                            "grace".to_owned(),
                            group_dm::unaddable_reason("not_friends"),
                        )],
                        addable: vec![ada.to_string()],
                    });
                }
            }
            Scene::FriendNickname => {
                if let Some(ada) = UserId::parse(&crate::demo::user_id("ada")) {
                    self.open_friend_nickname(ada, window, cx);
                    if let Some(input) = self.social.group_dm.as_ref().and_then(|w| w.name.clone())
                    {
                        input.update(cx, |i, cx| i.set_text(crate::demo::social::NICKNAME, cx));
                    }
                }
            }
            Scene::InviteCommunity => {
                if let Some(ada) = UserId::parse(&crate::demo::user_id("ada")) {
                    self.open_invite_to_community(ada, window, cx);
                }
            }
            // The DM list opens the friends page now: the media scene opens Ada's DM.
            Scene::MediaErrors => self.select_channel(crate::demo::snowflake("dm-ada"), window, cx),
            _ => self.select_channel(crate::demo::snowflake("dm-group"), window, cx),
        }
        cx.notify();
    }

    /// The group windows of the demo scenes, with what the demo server would have said.
    #[cfg(feature = "demo")]
    fn demo_group_window(
        &mut self,
        scene: crate::demo::scenes::Scene,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::demo::scenes::Scene;
        use group_invites::{InviteList, Load};
        match scene {
            Scene::GroupSettings => {
                self.open_group_dm(group_dm::Mode::Settings { channel }, window, cx)
            }
            Scene::GroupAdd => {
                self.open_group_dm(group_dm::Mode::Add { channel }, window, cx);
                if let Some(w) = self.social.group_dm.as_mut() {
                    w.link = Some(crate::demo::social::INVITE_LINK.into());
                }
            }
            _ => {
                self.open_group_dm(group_dm::Mode::Invites { channel }, window, cx);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if let Some(w) = self.social.group_dm.as_mut() {
                    w.invites = Some(InviteList {
                        load: Load::Ready(crate::demo::social::group_invites(now)),
                        show_created: false,
                    });
                }
            }
        }
    }

    /// Events from the backend's social domain.
    pub(crate) fn on_social_event(
        &mut self,
        event: backend::social::Ev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            backend::social::Ev::Presence(PresenceEv::SaveFailed) => self.status_save_failed(cx),
            backend::social::Ev::Presence(PresenceEv::Saved(settings)) => {
                self.status_saved(&settings, cx)
            }
            backend::social::Ev::Relationships(ev) => self.on_relationships_event(ev, window, cx),
            backend::social::Ev::Members(ev) => self.on_members_event(ev, cx),
            backend::social::Ev::Profile(ev) => self.on_profile_event(ev, cx),
        }
    }

    fn on_relationships_event(&mut self, ev: rel::Ev, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            rel::Ev::Done { key } => {
                self.social.busy.remove(&key);
                if key == "add-friend" {
                    self.add_friend_sent(cx);
                }
                self.social_done(&key, window, cx);
            }
            rel::Ev::Failed { key, reason } => {
                self.social.busy.remove(&key);
                self.social.done.remove(&key);
                if key == "add-friend" && self.social.add_friend.is_some() {
                    self.add_friend_failed(reason);
                } else if let Some(w) = self.social.group_dm.as_mut().filter(|w| w.owns(&key)) {
                    w.hint = None;
                    w.message = Some(reason);
                } else {
                    // A DM is open: the line over the chat says it; else the page's.
                    self.social_fail(reason);
                }
            }
            rel::Ev::GroupDm { key, result } => {
                self.social.busy.remove(&key);
                self.group_dm_answer(&key, result, window, cx);
            }
            rel::Ev::Working { key } => {
                if let Some(w) = self.social.group_dm.as_mut().filter(|w| w.owns(&key)) {
                    w.hint = Some(t!("social-still-working"));
                }
            }
            rel::Ev::InviteLink { key, url } => {
                self.social.busy.remove(&key);
                if let Some(w) = self.social.group_dm.as_mut().filter(|w| w.owns(&key)) {
                    w.link = Some(url);
                    w.copied = false;
                    w.message = None;
                }
            }
            rel::Ev::Invites {
                key,
                channel_id,
                result,
            } => {
                self.social.busy.remove(&key);
                self.invites_loaded(&channel_id, result);
            }
            rel::Ev::DmOpened { key, channel } => {
                self.social.busy.remove(&key);
                if self
                    .social
                    .group_dm
                    .as_ref()
                    .is_some_and(|w| w.key() == key)
                {
                    self.social.group_dm = None;
                }
                self.open_dm_channel(&channel, window, cx);
            }
        }
        cx.notify();
    }

    /// Opens the DM with `user`: the one the account state has, or a new one (Fluxer
    /// sends nothing when the DM is already there).
    pub(crate) fn open_dm_with(
        &mut self,
        user: UserId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.account.as_ref().and_then(|a| {
            a.read(cx)
                .dm_list()
                .into_iter()
                .find(|c| {
                    c.kind == inlet_protocol::models::channel_type::DM
                        && c.recipients.first().and_then(|u| UserId::parse(&u.id)) == Some(user)
                })
                .cloned()
        });
        match existing {
            Some(channel) => self.open_dm_channel(&channel, window, cx),
            None => self.social_action(
                rel::Cmd::OpenDm {
                    user_id: user.to_string(),
                },
                cx,
            ),
        }
    }

    /// Shows a DM the backend just opened or created: the state and the DM list learn
    /// it now (the gateway's CHANNEL_CREATE comes too, and changes nothing more).
    pub(crate) fn open_dm_channel(
        &mut self,
        channel: &inlet_protocol::Channel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(account) = self.account.clone() {
            let create = inlet_protocol::Dispatch::ChannelCreate(Box::new(channel.clone()));
            account.update(cx, |s, _| s.apply(&create));
        }
        let shown = backend::messages::parse::to_channel(channel);
        self.social.dm_rows = None;
        if self.selected_guild.is_none() {
            if !self.channels.iter().any(|c| c.id == shown.id) {
                self.channels.push(shown.clone());
            }
            self.select_channel(shown.id, window, cx);
        } else {
            self.follow_channel_link(None, shown.id, window, cx);
        }
    }

    /// A failure of a call: over the chat when a DM is open, else on the friends page.
    fn social_fail(&mut self, reason: String) {
        if self.friends_page_shown() {
            self.social.notice = Some(reason);
        } else {
            self.chat.notice = Some(reason);
        }
    }

    /// A call that was asked for went through: its line, and what the window does.
    fn social_done(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(done) = self.social.done.remove(key) else {
            return;
        };
        if let Some(text) = done.notice {
            self.social_say(text);
        }
        let mine = self.social.group_dm.as_ref().is_some_and(|w| w.owns(key));
        match done.then {
            Then::Nothing => {}
            Then::Close if mine => self.group_dm_close(window, cx),
            Then::Close => {}
            Then::ReloadInvites(channel) => {
                let open = self
                    .social
                    .group_dm
                    .as_ref()
                    .is_some_and(|w| w.mode == group_dm::Mode::Invites { channel });
                if open {
                    self.load_group_invites(channel, cx);
                }
            }
        }
    }

    /// A group DM call answered: a lasting line in its window, never a new try.
    fn group_dm_answer(
        &mut self,
        key: &str,
        result: GroupDm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Some people went in before the call ended: they are no longer to be picked, and
        // the line says how many.
        let (added, result) = match result {
            GroupDm::Partial { added, rest } => (added, *rest),
            other => (Vec::new(), other),
        };
        let mine = self.social.group_dm.as_ref().is_some_and(|w| w.owns(key));
        if mine {
            if let Some(w) = self.social.group_dm.as_mut() {
                w.hint = None;
                let gone: Vec<UserId> = added.iter().filter_map(|id| UserId::parse(id)).collect();
                w.picked.retain(|u| !gone.contains(u));
            }
        }
        let text = match &result {
            GroupDm::Done => {
                self.social.done.remove(key);
                if mine {
                    self.group_dm_close(window, cx);
                }
                return;
            }
            GroupDm::Captcha => t!("social-group-dm-captcha"),
            GroupDm::RateLimited { retry_after } => t!(
                "social-group-dm-rate-limited",
                wait = rel::wait_text(*retry_after)
            ),
            GroupDm::NotAddable { unaddable, addable } => {
                let named: Vec<(String, String)> = {
                    let state = self.account.as_ref().map(|a| a.read(cx));
                    unaddable
                        .iter()
                        .map(|(id, reason)| {
                            let name = match (state, UserId::parse(id)) {
                                (Some(s), Some(u)) => person_name(s, u),
                                _ => id.clone(),
                            };
                            (name, group_dm::unaddable_reason(reason))
                        })
                        .collect()
                };
                let creating = self
                    .social
                    .group_dm
                    .as_ref()
                    .is_some_and(|w| w.owns(key) && w.mode == group_dm::Mode::Create);
                if creating {
                    // Fluxer asks whether to go on without them.
                    let gone: Vec<UserId> = unaddable
                        .iter()
                        .filter_map(|(id, _)| UserId::parse(id))
                        .collect();
                    if let Some(w) = self.social.group_dm.as_mut() {
                        w.picked.retain(|u| !gone.contains(u));
                        w.panel = Some(group_dm::Panel::Unaddable {
                            rows: named,
                            addable: addable.clone(),
                        });
                    }
                    return;
                }
                let names: Vec<String> = named
                    .into_iter()
                    .map(|(name, reason)| format!("{name} ({reason})"))
                    .collect();
                t!("social-group-dm-not-addable", names = names.join(", "))
            }
            GroupDm::Failed(reason) => reason.clone(),
            GroupDm::Partial { .. } => return,
        };
        let text = if added.is_empty() {
            text
        } else {
            format!(
                "{} {text}",
                t!("social-group-added-some", count = added.len())
            )
        };
        self.social.done.remove(key);
        match self.social.group_dm.as_mut().filter(|w| w.owns(key)) {
            Some(w) => w.message = Some(text),
            None => self.social.notice = Some(text),
        }
    }

    /// The social windows, bottom first: add friend, group DM, DM menu, confirmation.
    pub(crate) fn render_social_overlays(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if self.screen != Screen::Main {
            return out;
        }
        // The DM menu is drawn deferred: it goes when a link prompt opens over it.
        if self.overlays.pending_link.is_some() || self.overlays.link_prompt.is_some() {
            self.social.dm_menu = None;
        }
        if self.social.add_friend.is_some() {
            out.push(self.render_add_friend(cx));
        }
        if self.social.group_dm.is_some() {
            out.push(self.render_group_dm(cx));
        }
        if let Some(menu) = self.social.dm_menu.clone() {
            out.push(self.render_dm_menu(&menu, cx));
        }
        if let Some(confirm) = self.social.confirm.clone() {
            out.push(self.render_social_confirm(&confirm, cx));
        }
        out
    }

    /// Whether `part` is the topmost layer (it then has the keyboard and Escape).
    fn social_top(&self, part: SocialPart) -> bool {
        use crate::desktop::Layer;
        self.is_top_layer(match part {
            SocialPart::Confirm => Layer::SocialConfirm,
            SocialPart::Menu => Layer::DmMenu,
            SocialPart::GroupDm => Layer::GroupDm,
            SocialPart::AddFriend => Layer::AddFriend,
        })
    }

    /// The social window whose text field has the keyboard now, if one has.
    pub(crate) fn social_field_focused(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> Option<crate::desktop::Layer> {
        use crate::desktop::Layer;
        use gpui::Focusable;
        if let Some(w) = &self.social.group_dm {
            let name = w
                .name
                .as_ref()
                .is_some_and(|n| n.focus_handle(cx).is_focused(window));
            if name || w.search.focus_handle(cx).is_focused(window) {
                return Some(Layer::GroupDm);
            }
        }
        self.social
            .add_friend
            .as_ref()
            .filter(|w| w.input.focus_handle(cx).is_focused(window))
            .map(|_| Layer::AddFriend)
    }

    /// The layer's keys on the DM menu while it is the topmost social window.
    pub(crate) fn social_menu_keys<E: InteractiveElement>(
        &self,
        el: E,
        cx: &mut Context<Self>,
    ) -> E {
        self.social_keys(SocialPart::Menu, el, cx)
    }

    /// The field the topmost social window's keyboard goes to, when it has one.
    pub(crate) fn social_field(&self, cx: &gpui::App) -> Option<gpui::FocusHandle> {
        use gpui::Focusable;
        let s = &self.social;
        if s.confirm.is_some() || s.dm_menu.is_some() {
            return None;
        }
        if s.group_dm.is_some() {
            return self.group_dm_field(cx);
        }
        s.add_friend.as_ref().map(|w| w.input.focus_handle(cx))
    }

    /// The layer's keys on the topmost social window.
    fn social_keys<E: InteractiveElement>(
        &self,
        part: SocialPart,
        el: E,
        cx: &mut Context<Self>,
    ) -> E {
        if self.social_top(part) {
            self.layer_keys(el, cx)
        } else {
            el
        }
    }

    /// What a question's yes does.
    pub(super) fn run_confirm(&mut self, confirm: &Confirm, cx: &mut Context<Self>) {
        match confirm {
            Confirm::RemoveFriend { user, .. } => self.social_action(
                rel::Cmd::Remove {
                    user_id: user.to_string(),
                },
                cx,
            ),
            Confirm::Block { user, .. } => self.social_action(
                rel::Cmd::Block {
                    user_id: user.to_string(),
                },
                cx,
            ),
            Confirm::LeaveGroup { channel, .. } => self.close_dm(*channel, cx),
            Confirm::MakeOwner { channel, user, .. } => self.edit_group_dm(
                *channel,
                GroupChange {
                    owner_id: Some(user.to_string()),
                    ..Default::default()
                },
                Some(Done::notice(
                    t!("social-ownership-transferred"),
                    Then::Nothing,
                )),
                cx,
            ),
            Confirm::RemoveFromGroup { channel, user, .. } => self.social_action_done(
                rel::Cmd::RemoveRecipient {
                    channel_id: channel.to_string(),
                    user_id: user.to_string(),
                },
                Done::notice(t!("social-removed-from-group"), Then::Nothing),
                cx,
            ),
            Confirm::RevokeInvite { channel, code } => {
                self.revoke_invite(*channel, code.clone(), cx)
            }
        }
    }

    fn render_social_confirm(&mut self, confirm: &Confirm, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui::overlays::modal::modal;
        use crate::ui::widgets::button;
        let (title, body, label) = match confirm {
            Confirm::RemoveFriend { name, .. } => (
                t!("social-remove-friend-title", name = name.clone()),
                t!("social-remove-friend-body", name = name.clone()),
                t!("social-remove-friend"),
            ),
            Confirm::Block { name, .. } => (
                t!("social-block-title", name = name.clone()),
                t!("social-block-body", name = name.clone()),
                t!("social-block"),
            ),
            Confirm::LeaveGroup { name, .. } => (
                t!("social-leave-group-title", name = name.clone()),
                t!("social-leave-group-body", name = name.clone()),
                t!("social-leave-group"),
            ),
            Confirm::MakeOwner { name, .. } => (
                t!("social-group-make-owner-title"),
                t!("social-group-make-owner-body", name = name.clone()),
                t!("social-group-make-owner-title"),
            ),
            Confirm::RemoveFromGroup { name, .. } => (
                t!("social-group-remove-title"),
                t!("social-group-remove-body", name = name.clone()),
                t!("social-group-remove"),
            ),
            Confirm::RevokeInvite { .. } => (
                t!("social-invite-revoke-title"),
                t!("social-invite-revoke-body"),
                t!("social-invite-revoke"),
            ),
        };
        // Handing a group over is not a loss the way the others are.
        let danger = !matches!(confirm, Confirm::MakeOwner { .. });
        let action = confirm.clone();
        let card = modal(
            "social-confirm",
            440.,
            [
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_strong())
                    .child(title)
                    .into_any_element(),
                div()
                    .text_color(theme::text_muted())
                    .child(body)
                    .into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("social-confirm-cancel", t!("common-cancel"), false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.social.confirm = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button("social-confirm-ok", label, true)
                            .bg(if danger {
                                theme::danger()
                            } else {
                                theme::brand()
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.social.confirm = None;
                                this.run_confirm(&action, cx);
                            })),
                    )
                    .into_any_element(),
            ],
        );
        self.social_keys(SocialPart::Confirm, card, cx)
            .into_any_element()
    }
}

#[cfg(all(test, feature = "demo"))]
mod tests;

/// What the user menu offers about a person (see `relationship_actions`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipAction {
    Message,
    AddFriend,
    Accept,
    RemoveFriend,
    CancelRequest,
    Block,
    Unblock,
    /// The private nickname for a friend.
    ChangeFriendNickname,
    /// Send an invite to a community of the account.
    InviteToCommunity,
    /// The account's own nickname in the open group.
    MyGroupNickname,
    /// Someone's nickname in the open group (the owner's).
    GroupNickname,
    MakeGroupOwner,
    RemoveFromGroup,
}

/// The social windows, for the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocialPart {
    AddFriend,
    GroupDm,
    Menu,
    Confirm,
}
