// SPDX-License-Identifier: AGPL-3.0-or-later
//! The social windows and menus through the events the backend sends: what the account
//! may do to a person or a group, and what a call that goes through (or does not) does
//! to the window. The backend is not there: commands go nowhere, the tests deliver the
//! answers themselves.

// Marks this file as test code for the text scan of `i18n.rs`, which skips what follows.
#[cfg(test)]
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext, Window};
use inlet_protocol::Ready;
use inlet_state::{AccountState, ChannelId, UserId};
use serde_json::json;

use super::group_dm::{self, header_button, Mode, Panel};
use super::{Confirm, RelationshipAction};
use crate::app::Inlet;
use crate::backend::social::relationships::{Ev, GroupDm, GroupInvite};
use crate::backend::{social, Event};

const GROUP: ChannelId = ChannelId(100);

/// The account (user 1) owns the garden guild; `owner` owns the group "Crew" (bo, ada and
/// the bot robo); "Pair" has the same two people as a second group; Bo, Ada and Dee are
/// friends, Cy asked to be.
fn state(owner: &str) -> AccountState {
    let ready: Ready = serde_json::from_value(json!({
        "session_id": "s", "user": {"id": "1", "username": "me"},
        "guilds": [{
            "id": "500",
            "properties": {"id": "500", "name": "Garden", "owner_id": "1", "features": []},
            "channels": [{"id": "501", "type": 0, "name": "general", "position": 0}],
            "roles": [{"id": "500", "name": "@everyone", "position": 0, "permissions": "1024"}],
            "members": [{"user": {"id": "1", "username": "me"}, "roles": [],
                         "joined_at": "2026-01-01T00:00:00Z"}],
            "member_count": 3
        }],
        "users": [
            {"id": "2", "username": "bo"}, {"id": "3", "username": "ada"},
            {"id": "4", "username": "robo", "bot": true}, {"id": "5", "username": "cy"},
            {"id": "6", "username": "dee"}
        ],
        "private_channels": [
            {"id": "100", "type": 3, "name": "Crew", "owner_id": owner,
             "recipients": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"},
                            {"id": "4", "username": "robo", "bot": true}]},
            {"id": "101", "type": 3, "name": "Pair", "owner_id": owner,
             "recipients": [{"id": "2", "username": "bo"}, {"id": "3", "username": "ada"}]},
            {"id": "200", "type": 1, "recipients": [{"id": "2", "username": "bo"}]},
            {"id": "201", "type": 1, "recipients": [{"id": "5", "username": "cy"}]}
        ],
        "relationships": [
            {"id": "2", "type": 1, "nickname": "Bobby"}, {"id": "3", "type": 1},
            {"id": "5", "type": 3}, {"id": "6", "type": 1}
        ],
        "read_states": [], "user_guild_settings": []
    }))
    .unwrap();
    AccountState::from_ready_at(&ready, 1)
}

/// A window with the account of `state(owner)` and nothing open.
fn rig<'a>(cx: &'a mut TestAppContext, owner: &str) -> (Entity<Inlet>, &'a mut VisualTestContext) {
    let (inlet, cx) = cx.add_window_view(Inlet::detached);
    let owner = owner.to_owned();
    inlet.update_in(cx, |this, _, cx| {
        this.account = Some(cx.new(|_| state(&owner)));
    });
    (inlet, cx)
}

/// The backend answers a call of the relationships part.
fn tell(this: &mut Inlet, ev: Ev, window: &mut Window, cx: &mut gpui::Context<Inlet>) {
    this.handle_event(Event::Social(social::Ev::Relationships(ev)), window, cx);
}

fn type_in(this: &Inlet, cx: &mut gpui::Context<Inlet>, text: &str) {
    let input = this
        .social
        .group_dm
        .as_ref()
        .and_then(|w| w.name.clone())
        .expect("the window has a field");
    input.update(cx, |i, cx| i.set_text(text.to_owned(), cx));
}

fn actions(this: &Inlet, user: u64, cx: &gpui::App) -> Vec<&'static str> {
    this.relationship_actions(UserId(user), cx)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
}

fn invite(code: &str) -> GroupInvite {
    GroupInvite {
        code: code.into(),
        url: format!("https://fluxer.test/invite/{code}"),
        inviter: Some(("1".into(), "me".into())),
        uses: 1,
        max_uses: 0,
        expires_ms: None,
        created_ms: Some(0),
    }
}

#[gpui::test]
fn a_friend_nickname_goes_out_trimmed_and_the_window_closes_when_it_went_through(
    cx: &mut TestAppContext,
) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_friend_nickname(UserId(2), window, cx);
        // The field starts with the nickname the account has given.
        let field = this.social.group_dm.as_ref().unwrap().name.clone().unwrap();
        assert_eq!(field.read(cx).text(), "Bobby");
        type_in(this, cx, "  Bo B  ");
        this.save_nickname(cx);
        assert!(this.social.busy.contains("nick:2"));
        assert!(
            this.social.group_dm.is_some(),
            "the window waits for the answer"
        );
        // A second Save while it runs sends nothing more (the key is busy).
        this.save_nickname(cx);
        assert_eq!(this.social.busy.len(), 1);

        tell(
            this,
            Ev::Done {
                key: "nick:2".into(),
            },
            window,
            cx,
        );
        assert!(this.social.group_dm.is_none());
        assert_eq!(this.chat.notice, Some(t!("social-nickname-updated")));
        assert!(this.social.busy.is_empty() && this.social.done.is_empty());
    });
}

#[gpui::test]
fn a_refused_nickname_stays_in_its_window_with_the_reason(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_friend_nickname(UserId(3), window, cx);
        type_in(this, cx, "Ada L.");
        this.save_nickname(cx);
        tell(
            this,
            Ev::Failed {
                key: "nick:3".into(),
                reason: "Slow down".into(),
            },
            window,
            cx,
        );
        let w = this.social.group_dm.as_ref().expect("the window stays");
        assert_eq!(w.message.as_deref(), Some("Slow down"));
        assert_eq!(this.chat.notice, None, "no success line");
        assert!(this.social.busy.is_empty() && this.social.done.is_empty());
    });
}

#[gpui::test]
fn a_nickname_over_32_characters_is_refused_before_anything_is_sent(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_friend_nickname(UserId(3), window, cx);
        type_in(this, cx, &"x".repeat(33));
        this.save_nickname(cx);
        let w = this.social.group_dm.as_ref().unwrap();
        assert_eq!(w.message, Some(t!("social-nickname-invalid")));
        assert!(this.social.busy.is_empty());

        // A blank field is fine: it clears the nickname.
        type_in(this, cx, "   ");
        this.save_nickname(cx);
        assert!(this.social.busy.contains("nick:3"));
        assert!(this.social.group_dm.as_ref().unwrap().message.is_none());
    });
}

#[gpui::test]
fn a_nickname_in_a_group_goes_back_to_the_settings_it_came_from(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_group_dm(Mode::Settings { channel: GROUP }, window, cx);
        this.open_group_dm_from(
            Mode::GroupNickname {
                channel: GROUP,
                user: UserId(3),
            },
            Vec::new(),
            Some(Mode::Settings { channel: GROUP }),
            window,
            cx,
        );
        type_in(this, cx, "Countess");
        this.save_nickname(cx);
        assert!(this.social.busy.contains("group:100"));
        tell(
            this,
            Ev::Done {
                key: "group:100".into(),
            },
            window,
            cx,
        );
        assert_eq!(
            this.social.group_dm.as_ref().map(|w| w.mode),
            Some(Mode::Settings { channel: GROUP }),
            "back in the settings"
        );
        assert_eq!(this.chat.notice, Some(t!("social-nickname-updated")));
    });
}

#[gpui::test]
fn saving_the_group_settings_sends_only_what_changed_and_says_so(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        // Nothing changed: the window just closes.
        this.open_group_dm(Mode::Settings { channel: GROUP }, window, cx);
        this.group_dm_save(GROUP, cx);
        assert!(this.social.group_dm.is_none() && this.social.busy.is_empty());

        // A new name goes out; the window closes with a line when the server agrees.
        this.open_group_dm(Mode::Settings { channel: GROUP }, window, cx);
        type_in(this, cx, "The Crew");
        this.group_dm_save(GROUP, cx);
        assert!(this.social.busy.contains("group:100"));
        assert!(this.social.group_dm.is_some());
        tell(
            this,
            Ev::Done {
                key: "group:100".into(),
            },
            window,
            cx,
        );
        assert!(this.social.group_dm.is_none());
        assert_eq!(this.chat.notice, Some(t!("social-group-updated")));
    });
}

#[gpui::test]
fn the_user_menu_offers_what_the_account_may_do_to_a_person(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, _, cx| {
        // No group open: a friend's nickname and the invite, nothing about a group.
        assert_eq!(
            actions(this, 3, cx),
            [
                "user-menu-message",
                "user-menu-friend-nickname",
                "user-menu-invite-community",
                "user-menu-remove-friend",
                "user-menu-block"
            ]
        );
        assert_eq!(
            actions(this, 1, cx),
            Vec::<&str>::new(),
            "nothing for the account"
        );

        // In a group the account owns: ownership, nickname and removal come between.
        this.selected_channel = Some("100".into());
        assert_eq!(
            actions(this, 3, cx),
            [
                "user-menu-message",
                "user-menu-friend-nickname",
                "user-menu-make-owner",
                "user-menu-group-nickname",
                "user-menu-remove-from-group",
                "user-menu-invite-community",
                "user-menu-remove-friend",
                "user-menu-block"
            ]
        );
        // A bot cannot own a group, is not invited and is no friend.
        assert_eq!(
            actions(this, 4, cx),
            [
                "user-menu-message",
                "user-menu-group-nickname",
                "user-menu-remove-from-group",
                "user-menu-add-friend",
                "user-menu-block"
            ]
        );
        // The account's own nickname in the group, and nothing else.
        assert_eq!(actions(this, 1, cx), ["user-menu-my-group-nickname"]);
        // Someone who is not in the open group gets nothing about it.
        assert!(!actions(this, 5, cx).contains(&"user-menu-make-owner"));
        assert!(actions(this, 5, cx).contains(&"user-menu-accept"));
    });
}

#[gpui::test]
fn only_the_owner_gets_ownership_nicknames_and_removal_in_a_group(cx: &mut TestAppContext) {
    // Bo owns "Crew": the account is a member.
    let (inlet, cx) = rig(cx, "2");
    inlet.update_in(cx, |this, window, cx| {
        this.selected_channel = Some("100".into());
        assert_eq!(
            actions(this, 3, cx),
            [
                "user-menu-message",
                "user-menu-friend-nickname",
                "user-menu-invite-community",
                "user-menu-remove-friend",
                "user-menu-block"
            ]
        );
        assert_eq!(actions(this, 1, cx), ["user-menu-my-group-nickname"]);
        // Even a forged click does nothing.
        this.run_relationship_action(RelationshipAction::MakeGroupOwner, UserId(3), window, cx);
        this.run_relationship_action(RelationshipAction::RemoveFromGroup, UserId(3), window, cx);
        assert_eq!(this.social.confirm, None);
    });
}

#[gpui::test]
fn handing_over_a_group_and_removing_someone_ask_first_and_say_so_after(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.selected_channel = Some("100".into());
        this.run_relationship_action(RelationshipAction::MakeGroupOwner, UserId(3), window, cx);
        let ask = this.social.confirm.clone().expect("it asks first");
        assert_eq!(
            ask,
            Confirm::MakeOwner {
                channel: GROUP,
                user: UserId(3),
                name: "ada".into()
            }
        );
        assert!(
            this.social.busy.is_empty(),
            "nothing is sent before the yes"
        );
        this.run_confirm(&ask, cx);
        assert!(this.social.busy.contains("group:100"));
        tell(
            this,
            Ev::Done {
                key: "group:100".into(),
            },
            window,
            cx,
        );
        assert_eq!(this.chat.notice, Some(t!("social-ownership-transferred")));

        this.run_relationship_action(RelationshipAction::RemoveFromGroup, UserId(2), window, cx);
        let ask = this.social.confirm.clone().expect("it asks first");
        assert!(matches!(
            ask,
            Confirm::RemoveFromGroup {
                user: UserId(2),
                ..
            }
        ));
        this.run_confirm(&ask, cx);
        assert!(this.social.busy.contains("group:100"));
        tell(
            this,
            Ev::Failed {
                key: "group:100".into(),
                reason: "Nope".into(),
            },
            window,
            cx,
        );
        assert_eq!(
            this.chat.notice,
            Some("Nope".into()),
            "a refusal is not a success line"
        );
        assert!(this.social.done.is_empty());
    });
}

#[gpui::test]
fn a_new_group_asks_before_copying_an_old_one_and_before_leaving_people_out(
    cx: &mut TestAppContext,
) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_group_dm(Mode::Create, window, cx);
        this.social.group_dm.as_mut().unwrap().picked = vec![UserId(2), UserId(3)];
        this.group_dm_go(window, cx);
        // "Pair" has these very two people: nothing is sent, the window asks.
        match &this.social.group_dm.as_ref().unwrap().panel {
            Some(Panel::Duplicates(found)) => {
                assert_eq!(found.len(), 1);
                assert_eq!(found[0].name, "Pair");
            }
            other => panic!("{other:?}"),
        }
        assert!(this.social.busy.is_empty());

        // "Create new group": the call goes out, and a slow one says it is still working.
        this.group_dm_confirm_panel(cx);
        assert!(this.social.busy.contains("group-dm"));
        assert!(this.social.group_dm.as_ref().unwrap().panel.is_none());
        tell(
            this,
            Ev::Working {
                key: "group-dm".into(),
            },
            window,
            cx,
        );
        assert_eq!(
            this.social.group_dm.as_ref().unwrap().hint,
            Some(t!("social-still-working"))
        );

        // Ada cannot be added: the window asks whether to go on without her.
        tell(
            this,
            Ev::GroupDm {
                key: "group-dm".into(),
                result: GroupDm::NotAddable {
                    unaddable: vec![("3".into(), "not_friends".into())],
                    addable: vec!["2".into()],
                },
            },
            window,
            cx,
        );
        let w = this.social.group_dm.as_ref().unwrap();
        assert_eq!(w.hint, None);
        assert_eq!(w.picked, [UserId(2)], "she is taken off the list");
        assert_eq!(
            w.panel,
            Some(Panel::Unaddable {
                rows: vec![("ada".into(), t!("social-unaddable-not-friends"))],
                addable: vec!["2".into()],
            })
        );
        this.group_dm_confirm_panel(cx);
        assert!(this.social.busy.contains("group-dm"), "created without her");

        // A CAPTCHA that cannot be passed ends in a lasting line, not in the browser.
        tell(
            this,
            Ev::GroupDm {
                key: "group-dm".into(),
                result: GroupDm::Captcha,
            },
            window,
            cx,
        );
        let w = this.social.group_dm.as_ref().unwrap();
        assert_eq!(w.message, Some(t!("social-group-dm-captcha")));
        assert!(this.social.busy.is_empty());
    });
}

#[gpui::test]
fn a_group_can_start_empty_and_a_single_friend_stays_picked_from_the_header(
    cx: &mut TestAppContext,
) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        // Nobody picked: an empty group, as Fluxer's "Create group DM".
        this.open_group_dm(Mode::Create, window, cx);
        this.group_dm_go(window, cx);
        assert!(this.social.busy.contains("group-dm"));
        this.social.busy.clear();

        // From a friend's DM the friend is picked and cannot be taken off.
        this.open_group_dm_from(Mode::Create, vec![UserId(2)], None, window, cx);
        let w = this.social.group_dm.as_ref().unwrap();
        assert_eq!(w.picked, [UserId(2)]);
        this.toggle_pick(UserId(2), cx);
        assert_eq!(this.social.group_dm.as_ref().unwrap().picked, [UserId(2)]);
        this.toggle_pick(UserId(6), cx);
        assert_eq!(
            this.social.group_dm.as_ref().unwrap().picked,
            [UserId(2), UserId(6)]
        );
    });
}

#[gpui::test]
fn friends_are_added_to_a_group_and_the_invite_link_lasts_a_day(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_group_dm(Mode::Add { channel: GROUP }, window, cx);
        this.toggle_pick(UserId(6), cx);
        this.group_dm_go(window, cx);
        assert!(this.social.busy.contains("group:100"));
        tell(
            this,
            Ev::GroupDm {
                key: "group:100".into(),
                result: GroupDm::Done,
            },
            window,
            cx,
        );
        assert!(this.social.group_dm.is_none(), "done");

        // The invite link made in the window shows in it.
        this.open_group_dm(Mode::Add { channel: GROUP }, window, cx);
        tell(
            this,
            Ev::InviteLink {
                key: "invite-link:100".into(),
                url: "https://fluxer.test/invite/aB3dEf7h".into(),
            },
            window,
            cx,
        );
        let w = this.social.group_dm.as_ref().unwrap();
        assert_eq!(
            w.link.as_deref(),
            Some("https://fluxer.test/invite/aB3dEf7h")
        );
        assert!(w.owns("invite-link:100") && w.owns("group:100") && !w.owns("group:101"));
    });
}

#[gpui::test]
fn the_owner_reads_and_revokes_the_groups_invites(cx: &mut TestAppContext) {
    use super::group_invites::{InviteList, Load};
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_group_dm(Mode::Invites { channel: GROUP }, window, cx);
        assert!(
            this.social.busy.contains("invites:100"),
            "the list is asked for"
        );
        let loading = |this: &Inlet| this.social.group_dm.as_ref().unwrap().invites.clone();
        assert_eq!(
            loading(this).map(|l| l.load),
            Some(Load::Loading),
            "until it comes"
        );

        tell(
            this,
            Ev::Invites {
                key: "invites:100".into(),
                channel_id: "100".into(),
                result: Ok(vec![invite("aaaa1111"), invite("bbbb2222")]),
            },
            window,
            cx,
        );
        assert!(this.social.busy.is_empty());
        let Some(InviteList {
            load: Load::Ready(list),
            ..
        }) = loading(this)
        else {
            panic!("the list is in");
        };
        assert_eq!(list.len(), 2);

        // Revoking asks first; the yes sends it, and when it is done the list is read
        // again.
        let ask = Confirm::RevokeInvite {
            channel: GROUP,
            code: "aaaa1111".into(),
        };
        this.run_confirm(&ask, cx);
        assert!(this.social.busy.contains("invite-revoke:100:aaaa1111"));
        assert!(
            this.social
                .group_dm
                .as_ref()
                .unwrap()
                .owns("invite-revoke:100:aaaa1111"),
            "its refusal shows in the window"
        );
        tell(
            this,
            Ev::Done {
                key: "invite-revoke:100:aaaa1111".into(),
            },
            window,
            cx,
        );
        assert_eq!(this.chat.notice, Some(t!("social-invite-revoked")));
        assert!(
            this.social.busy.contains("invites:100"),
            "read again after the revoke"
        );

        // A list that cannot be read says why, and Try again asks once more.
        tell(
            this,
            Ev::Invites {
                key: "invites:100".into(),
                channel_id: "100".into(),
                result: Err("Missing permissions".into()),
            },
            window,
            cx,
        );
        assert_eq!(
            loading(this).map(|l| l.load),
            Some(Load::Failed("Missing permissions".into()))
        );
    });
}

#[gpui::test]
fn a_member_who_does_not_own_the_group_asks_for_no_invites(cx: &mut TestAppContext) {
    // Bo owns "Crew": the account is only a member.
    let (inlet, cx) = rig(cx, "2");
    inlet.update_in(cx, |this, window, cx| {
        this.open_group_dm(Mode::Invites { channel: GROUP }, window, cx);
        assert!(
            this.social.busy.is_empty(),
            "the server only lists them for the owner"
        );
    });
}

#[gpui::test]
fn an_invite_to_a_community_is_sent_once_and_the_window_says_so(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_invite_to_community(UserId(3), window, cx);
        let candidate = {
            let state = this.account.as_ref().unwrap().read(cx);
            let found = super::community::invite_candidates(state, UserId(3));
            assert_eq!(found.len(), 1);
            found[0].clone()
        };
        assert_eq!(candidate.name, "Garden");
        this.invite_to_community(UserId(3), candidate.clone(), cx);
        assert!(this.social.busy.contains("community:3:500"));
        // A second click while it goes out sends nothing.
        let before = this.social.done.clone();
        this.invite_to_community(UserId(3), candidate, cx);
        assert_eq!(this.social.done, before);

        tell(
            this,
            Ev::Done {
                key: "community:3:500".into(),
            },
            window,
            cx,
        );
        assert!(this.social.group_dm.is_none(), "the window closes");
        assert_eq!(
            this.chat.notice,
            Some(t!("social-invite-sent", guild = "Garden"))
        );
    });
}

#[gpui::test]
fn a_refused_community_invite_stays_in_the_list_with_the_reason(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_invite_to_community(UserId(3), window, cx);
        let candidate = {
            let state = this.account.as_ref().unwrap().read(cx);
            super::community::invite_candidates(state, UserId(3))[0].clone()
        };
        this.invite_to_community(UserId(3), candidate, cx);
        tell(
            this,
            Ev::Failed {
                key: "community:3:500".into(),
                reason: "Cannot send messages to this user".into(),
            },
            window,
            cx,
        );
        let w = this.social.group_dm.as_ref().expect("the window stays");
        assert_eq!(
            w.message.as_deref(),
            Some("Cannot send messages to this user")
        );
        assert_eq!(this.chat.notice, None);
        assert!(this.social.busy.is_empty() && this.social.done.is_empty());
    });
}

#[gpui::test]
fn the_header_offers_a_group_with_a_friend_or_friends_for_a_group_that_has_room(
    cx: &mut TestAppContext,
) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, _, cx| {
        let shows = |this: &mut Inlet, channel: &str, cx: &mut gpui::Context<Inlet>| {
            this.selected_channel = Some(channel.into());
            header_button(this, cx).is_some()
        };
        assert!(shows(this, "200", cx), "a friend's DM: start a group");
        assert!(!shows(this, "201", cx), "not a friend: nothing");
        assert!(shows(this, "100", cx), "a group with room: add friends");
        this.selected_guild = Some("500".into());
        assert!(!shows(this, "100", cx), "only among the DMs");
    });
}

#[test]
fn a_group_takes_fifty_people_in_all() {
    // The account and 49 others; the state may show fewer of a big group, never more.
    assert_eq!(group_dm::room_in_group(3), 46);
    assert_eq!(group_dm::room_in_group(48), 1);
    assert_eq!(group_dm::room_in_group(49), 0, "full");
    assert_eq!(group_dm::room_in_group(80), 0, "never negative");
}

#[gpui::test]
fn a_group_knows_its_owner_and_its_name(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, _, cx| {
        let state = this.account.as_ref().unwrap().read(cx);
        assert_eq!(group_dm::group_owner(state, GROUP), Some(UserId(1)));
        assert_eq!(group_dm::group_name(state, GROUP), "Crew");
    });
}

#[gpui::test]
fn people_added_before_a_refusal_leave_the_list_and_the_line_says_so(cx: &mut TestAppContext) {
    let (inlet, cx) = rig(cx, "1");
    inlet.update_in(cx, |this, window, cx| {
        this.open_group_dm(Mode::Add { channel: GROUP }, window, cx);
        this.toggle_pick(UserId(6), cx);
        this.group_dm_go(window, cx);
        tell(
            this,
            Ev::GroupDm {
                key: "group:100".into(),
                result: GroupDm::Partial {
                    added: vec!["6".into()],
                    rest: Box::new(GroupDm::Captcha),
                },
            },
            window,
            cx,
        );
        let w = this.social.group_dm.as_ref().expect("the window stays");
        assert!(w.picked.is_empty(), "Dee is in the group now");
        assert_eq!(
            w.message,
            Some(format!(
                "{} {}",
                t!("social-group-added-some", count = 1),
                t!("social-group-dm-captcha")
            ))
        );
        assert!(this.social.busy.is_empty());
    });
}
