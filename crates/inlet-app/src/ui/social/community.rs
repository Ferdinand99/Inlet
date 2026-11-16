// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Invite to Community", as Fluxer's `InviteToCommunityMenuItem`: from a person's menu
//! the account sends them an invite to one of its communities as a DM.
//!
//! The communities offered are those where the person is not a member (as far as the
//! account knows) and the account can invite: it may create invites in the first
//! invitable text channel, or the community has a vanity link and that channel is open
//! to everyone. A new invite does not expire and has no limit; a vanity link is sent as
//! it is. Bots are not invited, and nothing is offered when no community fits.
// Deviation: Fluxer opens the list as a submenu of the person's menu; the menu here has
// no submenus, so the entry opens the list in a window.

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString};
use inlet_protocol::models::channel_type;
use inlet_state::{AccountState, GuildId, UserId};
use serde_json::Value;

use super::group_dm::{title_line, Mode};
use super::{Done, Then};
use crate::app::Inlet;
use crate::backend::social::relationships::Cmd;
use crate::theme;

/// A community the account can invite someone to.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub guild: GuildId,
    pub name: String,
    /// The channel the invite is for.
    pub channel: inlet_state::ChannelId,
    /// The community's vanity link, sent instead of a new invite when the account may not
    /// make invites there.
    pub vanity: Option<String>,
}

/// Whether the `@everyone` overwrite of a channel denies viewing it.
fn hidden_from_everyone(
    state: &AccountState,
    channel: inlet_state::ChannelId,
    guild: GuildId,
) -> bool {
    state.channels().overwrites(channel).is_some_and(|list| {
        list.iter().any(|o| {
            o.kind == inlet_protocol::models::overwrite_type::ROLE as u8
                && o.id == guild.get()
                && o.deny & inlet_protocol::permissions::VIEW_CHANNEL != 0
        })
    })
}

/// The first channel of a community, in the order of its channel list, that the account
/// can invite to: itself, or by the vanity link.
fn invitable_channel(
    state: &AccountState,
    guild: GuildId,
    vanity: Option<&str>,
) -> Option<(inlet_state::ChannelId, Option<String>)> {
    let channels = state.guild_channels(guild);
    // Text channels before voice channels, then by position (Fluxer's list order).
    let order = |c: &&inlet_protocol::Channel| {
        (
            c.kind == channel_type::GUILD_VOICE,
            c.position.unwrap_or(0),
            c.id.parse::<u64>().unwrap_or(u64::MAX),
        )
    };
    let mut categories: Vec<&inlet_protocol::Channel> = channels
        .iter()
        .copied()
        .filter(|c| c.kind == channel_type::GUILD_CATEGORY)
        .collect();
    categories.sort_by_key(|c| {
        (
            c.position.unwrap_or(0),
            c.id.parse::<u64>().unwrap_or(u64::MAX),
        )
    });
    let mut listed: Vec<&inlet_protocol::Channel> = channels
        .iter()
        .copied()
        .filter(|c| c.kind != channel_type::GUILD_CATEGORY && c.parent_id.is_none())
        .collect();
    listed.sort_by_key(order);
    for category in categories {
        let mut inside: Vec<&inlet_protocol::Channel> = channels
            .iter()
            .copied()
            .filter(|c| {
                c.kind != channel_type::GUILD_CATEGORY
                    && c.parent_id.as_deref() == Some(category.id.as_str())
            })
            .collect();
        inside.sort_by_key(order);
        listed.extend(inside);
    }
    listed
        .into_iter()
        .filter(|c| inlet_state::channels::is_guild_text_based(c.kind))
        .find_map(|c| {
            let id = inlet_state::ChannelId::parse(&c.id)?;
            if state
                .permissions(guild, id)
                .has(inlet_protocol::permissions::CREATE_INSTANT_INVITE)
            {
                return Some((id, None));
            }
            let code = vanity.filter(|v| !v.is_empty())?;
            (!hidden_from_everyone(state, id, guild)).then(|| (id, Some(code.to_owned())))
        })
}

/// The communities to offer for inviting `user`, by name. None for a bot.
pub fn invite_candidates(state: &AccountState, user: UserId) -> Vec<Candidate> {
    if state.user(user).is_some_and(|u| u.bot) {
        return Vec::new();
    }
    let mut out: Vec<Candidate> = state
        .guilds()
        .filter(|g| !g.unavailable && !g.left && !g.visiting)
        .filter(|g| state.member(g.id, user).is_none())
        .filter_map(|g| {
            let vanity = g
                .properties
                .as_ref()
                .and_then(|p| p.extra.get("vanity_url_code"))
                .and_then(Value::as_str);
            let (channel, vanity) = invitable_channel(state, g.id, vanity)?;
            Some(Candidate {
                guild: g.id,
                name: crate::model::clean_name(g.name()),
                channel,
                vanity,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.guild.cmp(&b.guild))
    });
    out
}

impl Inlet {
    /// Whether there is a community to invite `user` to (the menu offers the entry then).
    pub(crate) fn can_invite_to_community(&self, user: UserId, cx: &gpui::App) -> bool {
        self.account
            .as_ref()
            .is_some_and(|a| !invite_candidates(a.read(cx), user).is_empty())
    }

    /// Sends the invite to `candidate` and closes the window when it is out.
    pub(super) fn invite_to_community(
        &mut self,
        user: UserId,
        candidate: Candidate,
        cx: &mut Context<Self>,
    ) {
        // One at a time: a click while an invite goes out sends nothing.
        if self
            .social
            .group_dm
            .as_ref()
            .is_some_and(|w| self.social_busy(&w.key()) || self.social_busy_prefix(&w.key()))
        {
            return;
        }
        if let Some(w) = self.social.group_dm.as_mut() {
            w.message = None;
        }
        self.social_action_done(
            Cmd::InviteToCommunity {
                user_id: user.to_string(),
                guild_id: candidate.guild.to_string(),
                channel_id: candidate.channel.to_string(),
                vanity: candidate.vanity,
            },
            Done::notice(
                t!("social-invite-sent", guild = candidate.name),
                Then::Close,
            ),
            cx,
        );
    }

    /// The parts of the window: the communities that fit, by name.
    pub(super) fn community_parts(
        &mut self,
        user: UserId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(w) = self.social.group_dm.as_ref() else {
            return Vec::new();
        };
        let search = w.search.clone();
        let busy = self.social_busy_prefix(&w.key());
        let query = search.read(cx).text().trim().to_lowercase();
        let (whose, candidates) = match self.account.as_ref().map(|a| a.read(cx)) {
            Some(state) => (
                super::person_name(state, user),
                invite_candidates(state, user),
            ),
            None => (String::new(), Vec::new()),
        };
        let candidates: Vec<Candidate> = candidates
            .into_iter()
            .filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query))
            .collect();
        let mut parts = vec![
            title_line(t!("social-community-title")),
            div()
                .text_size(px(14.))
                .text_color(theme::text_muted())
                .child(t!("social-community-body", name = whose))
                .into_any_element(),
            search.into_any_element(),
        ];
        let rows: Vec<AnyElement> = candidates
            .into_iter()
            .map(|c| self.community_row(user, c, busy, cx))
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .py_4()
                .flex()
                .justify_center()
                .text_color(theme::text_muted())
                .child(t!("social-community-none"))
        });
        parts.push(
            div()
                .id("community-list")
                .max_h(px(320.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(rows)
                .children(empty)
                .into_any_element(),
        );
        parts.extend(self.window_notices());
        parts.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(self.close_button("community-close", cx))
                .into_any_element(),
        );
        parts
    }

    /// A community to pick: its icon (or initial) and its name.
    fn community_row(
        &mut self,
        user: UserId,
        candidate: Candidate,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = self.guild_icon_image(&candidate.guild.to_string(), 32., cx);
        let picture = match icon {
            Some(img) => img.rounded(px(8.)).into_any_element(),
            None => div()
                .size(px(32.))
                .flex_none()
                .rounded(px(8.))
                .bg(theme::guild_item())
                .flex()
                .items_center()
                .justify_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_strong())
                .child(
                    candidate
                        .name
                        .chars()
                        .next()
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_default(),
                )
                .into_any_element(),
        };
        let name = candidate.name.clone();
        let id = SharedString::from(format!("community-{}", candidate.guild));
        div()
            .id(id)
            .h(px(44.))
            .px_2()
            .rounded(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .when(busy, |d| d.opacity(0.5))
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
                    .child(name),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.invite_to_community(user, candidate.clone(), cx)
            }))
            .into_any_element()
    }

    /// Opens the list of communities for `user`.
    pub(crate) fn open_invite_to_community(
        &mut self,
        user: UserId,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.open_group_dm(Mode::InviteToCommunity { user }, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::Ready;
    use serde_json::json;

    /// An account in four communities: one it owns (it may invite), one where it can only
    /// share the vanity link, one without a vanity link and one whose channels are hidden
    /// from everyone (nothing to share).
    fn state() -> AccountState {
        let guild = |id: &str, name: &str, owner: &str, vanity: Option<&str>, hidden: bool| {
            let overwrites = if hidden {
                json!([{"id": id, "type": 0, "allow": "0", "deny": "1024"}])
            } else {
                json!([])
            };
            json!({
                "id": id,
                "properties": {"id": id, "name": name, "owner_id": owner, "vanity_url_code": vanity,
                               "features": []},
                "channels": [
                    {"id": format!("{id}1"), "type": 4, "name": "Text", "position": 0},
                    {"id": format!("{id}2"), "type": 0, "name": "late", "position": 5,
                     "parent_id": format!("{id}1"), "permission_overwrites": overwrites},
                    {"id": format!("{id}3"), "type": 0, "name": "first", "position": 1,
                     "parent_id": format!("{id}1"), "permission_overwrites": overwrites}
                ],
                "roles": [{"id": id, "name": "@everyone", "position": 0, "permissions": "1024"}],
                "members": [{"user": {"id": "1", "username": "me"}, "roles": [],
                             "joined_at": "2026-01-01T00:00:00Z"}],
                "member_count": 3
            })
        };
        let ready: Ready = serde_json::from_value(json!({
            "session_id": "s", "user": {"id": "1", "username": "me"},
            "guilds": [
                guild("100", "Zebra", "1", None, false),
                guild("200", "apple", "2", Some("apples"), false),
                guild("300", "Quiet", "2", None, false),
                guild("400", "Hidden", "2", Some("secret"), true)
            ],
            "users": [{"id": "5", "username": "ada"}, {"id": "6", "username": "bot", "bot": true}],
            "private_channels": [], "relationships": [],
            "read_states": [], "user_guild_settings": []
        }))
        .unwrap();
        AccountState::from_ready_at(&ready, 1)
    }

    #[test]
    fn invites_go_to_the_communities_where_the_account_can_invite() {
        let s = state();
        let found = invite_candidates(&s, UserId(5));
        assert_eq!(
            found.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["apple", "Zebra"],
            "by name, without Quiet (no vanity link) and Hidden (nobody sees its channels): {found:?}"
        );
        // Apple: the vanity link, for the first text channel in the order of the list.
        assert_eq!(found[0].channel.get(), 2003);
        assert_eq!(found[0].vanity.as_deref(), Some("apples"));
        // Zebra: the account owns it, so it makes an invite itself.
        assert_eq!(found[1].channel.get(), 1003);
        assert_eq!(found[1].vanity, None);
    }

    #[test]
    fn a_bot_is_never_invited() {
        let s = state();
        assert!(invite_candidates(&s, UserId(6)).is_empty());
    }
}
