// SPDX-License-Identifier: AGPL-3.0-or-later
//! The channel list of a guild: categories that fold, and channel rows.

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString, Window};
use inlet_state::{ChannelId, GuildId};

use super::floating::Severity;
use crate::app::Inlet;
use crate::assets::Icon;
use crate::model::{Channel, ChannelKind};
use crate::theme;
use crate::ui::notifications::badge::{mention_badge, RowMarks};
use crate::ui::overlays::nav_dialog::{NavDialog, Soon};

/// Height of a channel row (20 px of text and 6 px above and below).
pub const ROW_HEIGHT: f32 = 32.;
/// Corner radius of a channel row (`--radius-md`).
pub const ROW_RADIUS: f32 = 6.;

pub fn channel_icon(kind: ChannelKind) -> Icon {
    match kind {
        ChannelKind::Text => Icon::Hash,
        ChannelKind::Announcement => Icon::Megaphone,
        ChannelKind::Voice => Icon::Speaker,
        ChannelKind::Dm | ChannelKind::GroupDm => Icon::At,
        ChannelKind::Media => Icon::ImagesSquare,
        ChannelKind::Link => Icon::Link,
        _ => Icon::Chats,
    }
}

pub fn channel_label(c: &Channel) -> String {
    match c.kind {
        ChannelKind::Dm | ChannelKind::GroupDm => format!("@{}", c.name),
        ChannelKind::Text | ChannelKind::Announcement => format!("#{}", c.name),
        _ => c.name.clone(),
    }
}

impl Inlet {
    /// The rows of a guild's channel list: the "Members" row, then the categories with
    /// their channels (and who is in each voice channel).
    pub(super) fn channel_rows(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Vec<(usize, Severity)>) {
        let selected = self.selected_channel.clone();
        let mut previous_was_channel = false;
        let mut rows: Vec<AnyElement> = Vec::new();
        // The rows of channels with something unread, for the floating indicators.
        let mut unread: Vec<(usize, Severity)> = Vec::new();
        rows.extend(self.members_row(cx));
        let folded = self.folded_categories(cx);
        let hide_muted = self.open_guild_hides_muted(cx);
        let channels: Vec<Channel> = self
            .channels
            .iter()
            .filter(|c| {
                if c.kind == ChannelKind::Category {
                    return true;
                }
                let open = selected.as_deref() == Some(c.id.as_str());
                let badge = self.badge(&c.id);
                // "Hide muted channels": a muted channel goes, unless it is the open one
                // or has a mention.
                if hide_muted && badge.dimmed && !open && badge.mentions == 0 {
                    return false;
                }
                // Folded categories hide their channels, except what Fluxer keeps: the
                // open channel, and channels with something unread (in a muted category
                // only those with mentions).
                let parent = c.parent_id.as_ref();
                if !parent.is_some_and(|p| folded.contains(p)) {
                    return true;
                }
                let category_muted = parent.is_some_and(|p| self.badge(p).dimmed);
                open || badge.mentions > 0 || (badge.unread && !badge.muted && !category_muted)
            })
            .cloned()
            .collect();
        for c in &channels {
            if c.kind == ChannelKind::Category {
                let spaced = std::mem::replace(&mut previous_was_channel, false);
                rows.push(self.category_row(c, spaced, cx));
                if folded.contains(&c.id) {
                    rows.extend(self.folded_voice_strip(c, cx));
                }
                continue;
            }
            previous_was_channel = true;
            let active = selected.as_deref() == Some(c.id.as_str());
            let badge = self.badge(&c.id);
            if badge.mentions > 0 {
                unread.push((rows.len(), Severity::Mention));
            } else if RowMarks::of(badge, active).pill {
                unread.push((rows.len(), Severity::Unread));
            }
            rows.push(self.channel_row(c, active, cx));
            if c.kind == ChannelKind::Voice {
                rows.extend(self.voice_rows(c, cx));
            }
        }
        (rows, unread)
    }

    /// The "Members" row at the top of a server's list: it shows or hides the member
    /// list, and is drawn selected while the list is on screen. Only for a server whose
    /// members the account may see.
    fn members_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let guild = GuildId::parse(self.selected_guild.as_deref()?)?;
        let account = self.account.as_ref()?;
        // Fluxer shows the row to moderators only (any one of these permissions).
        use inlet_protocol::permissions as perm;
        let moderator = perm::MANAGE_GUILD
            | perm::MANAGE_ROLES
            | perm::MANAGE_NICKNAMES
            | perm::BAN_MEMBERS
            | perm::MODERATE_MEMBERS
            | perm::KICK_MEMBERS;
        if account.read(cx).guild_permissions(guild).bits() & moderator == 0 {
            return None;
        }
        let color = theme::text_channel();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("members-row")
                        .debug_selector(|| "members-row".into())
                        .flex_none()
                        .h(px(ROW_HEIGHT))
                        .ml_2()
                        .px_2()
                        .rounded(px(ROW_RADIUS))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .cursor_pointer()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color)
                        .hover(|s| s.bg(theme::hover_bg()).text_color(theme::text()))
                        .child(Icon::Users.el(px(20.), color))
                        .child(t!("sidebar-members"))
                        // Fluxer opens the members page here; Inlet has no such page yet.
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.nav_show(NavDialog::Soon(Soon::Members), cx)
                        })),
                )
                // The line under it, as Fluxer's `membersSeparator`.
                .child(div().h(px(1.)).mx_2().my(px(6.)).bg(theme::divider()))
                .into_any_element(),
        )
    }

    /// Who is in a voice channel, one row each, from the voice states.
    fn voice_rows(&mut self, c: &Channel, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(account) = self.account.clone() else {
            return Vec::new();
        };
        let (Some(channel), Some(guild)) = (
            ChannelId::parse(&c.id),
            self.selected_guild.as_deref().and_then(GuildId::parse),
        ) else {
            return Vec::new();
        };
        struct Who {
            name: String,
            avatar: Option<crate::model::UserAvatar>,
            muted: bool,
            deafened: bool,
            video: bool,
            streaming: bool,
        }
        let people: Vec<Who> = {
            let state = account.read(cx);
            state
                .voice()
                .participants(channel)
                .into_iter()
                .map(|p| {
                    let user = state.user(p.user);
                    let name = state
                        .member(guild, p.user)
                        .and_then(|m| m.display_name())
                        .or_else(|| user.map(|u| u.display_name()))
                        .map(crate::model::clean_name)
                        .unwrap_or_else(|| t!("sidebar-unknown-user"));
                    Who {
                        name,
                        avatar: user.and_then(crate::backend::media::user_avatar),
                        muted: p.muted,
                        deafened: p.deafened,
                        video: p.video,
                        streaming: p.streaming,
                    }
                })
                .collect()
        };
        people
            .into_iter()
            .enumerate()
            .map(|(i, who)| {
                let avatar = self.user_avatar(who.avatar.as_ref(), &who.name, 20., false, cx);
                let color = theme::text_muted();
                div()
                    .id(SharedString::from(format!("voice-{}-{i}", c.id)))
                    .debug_selector(|| "voice-participant".into())
                    .flex_none()
                    .h(px(28.))
                    .ml(px(36.))
                    .pr_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(14.))
                    .text_color(color)
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(who.name.clone()),
                    )
                    .when(who.streaming, |d| {
                        d.child(Icon::MonitorPlay.el(px(14.), color))
                    })
                    .when(who.video, |d| d.child(Icon::VideoCamera.el(px(14.), color)))
                    .when(who.deafened, |d| {
                        d.child(Icon::SpeakerSlash.el(px(14.), color))
                    })
                    .when(who.muted && !who.deafened, |d| {
                        d.child(Icon::MicrophoneSlash.el(px(14.), color))
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// Whether the open server leaves muted channels out of its list (`hide_muted_channels`).
    pub(crate) fn open_guild_hides_muted(&self, cx: &gpui::App) -> bool {
        let (Some(account), Some(guild)) = (
            self.account.as_ref(),
            self.selected_guild.as_deref().and_then(GuildId::parse),
        ) else {
            return false;
        };
        account
            .read(cx)
            .guild_settings()
            .get(Some(guild))
            .hide_muted_channels
    }

    /// The people in the voice channels of a folded category, as a strip of avatars
    /// under its row.
    fn folded_voice_strip(&mut self, cat: &Channel, cx: &mut Context<Self>) -> Option<AnyElement> {
        let account = self.account.clone()?;
        let voice_channels: Vec<ChannelId> = self
            .channels
            .iter()
            .filter(|c| {
                c.kind == ChannelKind::Voice && c.parent_id.as_deref() == Some(cat.id.as_str())
            })
            .filter_map(|c| ChannelId::parse(&c.id))
            .collect();
        let people: Vec<(String, Option<crate::model::UserAvatar>)> = {
            let state = account.read(cx);
            voice_channels
                .iter()
                .flat_map(|ch| state.voice().participants(*ch))
                .take(8)
                .map(|p| {
                    let user = state.user(p.user);
                    let name = user
                        .map(|u| crate::model::clean_name(u.display_name()))
                        .unwrap_or_default();
                    (name, user.and_then(crate::backend::media::user_avatar))
                })
                .collect()
        };
        if people.is_empty() {
            return None;
        }
        let avatars: Vec<AnyElement> = people
            .iter()
            .map(|(name, avatar)| self.user_avatar(avatar.as_ref(), name, 20., false, cx))
            .collect();
        Some(
            div()
                .id(SharedString::from(format!("voice-strip-{}", cat.id)))
                .flex_none()
                .h(px(28.))
                .ml(px(16.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .children(avatars)
                .into_any_element(),
        )
    }

    /// A click on a channel row: a link channel opens its address (after the
    /// confirmation every masked link gets), anything else opens in the chat. An
    /// age-restricted channel asks first.
    pub(crate) fn open_channel_row(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let link = self
            .channels
            .iter()
            .any(|c| c.id == id && c.kind == ChannelKind::Link);
        if !link {
            self.select_channel(id, window, cx);
            return;
        }
        if let Some(gate) = self.gate_for(&id, cx) {
            self.show_gate(gate, cx);
            return;
        }
        let url = ChannelId::parse(&id)
            .zip(self.account.as_ref())
            .and_then(|(ch, account)| {
                account
                    .read(cx)
                    .channel(ch)
                    .and_then(|c| c.extra.get("url"))
                    .and_then(|u| u.as_str())
                    .filter(|u| !u.is_empty() && u.len() <= 2048)
                    .map(str::to_owned)
            });
        match url {
            // Masked: the row only shows the channel's name, so the address is shown first.
            Some(url) => self.open_link(&url, true, cx),
            None => {
                self.chat.error = Some(t!("sidebar-link-missing"));
                cx.notify();
            }
        }
    }

    /// One channel (or direct message) row.
    pub(super) fn channel_row(
        &self,
        c: &Channel,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Forums and media channels open the forum view.
        let messageable = c.is_messageable() || c.is_forum() || c.kind == ChannelKind::Link;
        let id = c.id.clone();
        // Deviation: no hover actions (invite, channel settings) on rows; the bell in the
        // header and a right-click or the bell on a category open the notification settings.
        let badge = self.badge(&c.id);
        let marks = RowMarks::of(badge, active);
        let color = if active {
            theme::text_strong()
        } else if marks.highlight {
            // Fluxer's `.channelItemHighlight` colours an unread name `--text-secondary`.
            theme::text_secondary()
        } else {
            theme::text_channel()
        };
        let row = div()
            .id(SharedString::from(format!("ch-{}", c.id)))
            .debug_selector(|| "channel-row".into())
            // Rows must not shrink: a scrolling flex column shrinks its children
            // when the list is taller than the view, and `overflow_hidden` lets
            // a row shrink to nothing.
            .flex_none()
            .h(px(ROW_HEIGHT))
            .ml_2()
            .px_2()
            .rounded(px(ROW_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .whitespace_nowrap()
            .overflow_hidden()
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .when(active, |d| d.bg(theme::selected_bg()))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener({
                    let id = c.id.clone();
                    move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.channel_context_menu(id.clone(), event, cx)
                    }
                }),
            )
            .when(messageable, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme::hover_bg()).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_channel_row(id.clone(), window, cx)
                    }))
            })
            .child(channel_icon(c.kind).el(px(20.), color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(c.name.clone()),
            )
            .when(marks.count, |d| d.child(mention_badge(badge.mentions)));
        // The unread pill sits 4 px left of the row, outside its clipped box, so the row
        // goes in a frame that carries it (and the half-strength look of a muted channel).
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .when(marks.dimmed, |d| d.opacity(0.5))
            .when(marks.pill, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .top(px(12.))
                        .w(px(8.))
                        .h(px(8.))
                        .rounded_r(px(4.))
                        .bg(theme::text_strong())
                        .when(marks.pill_faded, |p| p.opacity(0.4)),
                )
            })
            .child(row)
            .into_any_element()
    }
}
