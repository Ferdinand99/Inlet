// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Privacy tab: who may message the account and who may send it a friend request.
//! Both are the account's own settings on the server, the ones Fluxer's Privacy and
//! safety page changes.

use gpui::{prelude::*, AnyElement, Context, SharedString};

use super::super::controls::page;
use super::super::store;
use super::super::sync::friend_sources;
use super::super::{SettingsTab, SettingsView};
use super::desktop::{heading, note, toggle};
use crate::backend::settings::write::Change;

pub struct PrivacyTab;

impl SettingsTab for PrivacyTab {
    fn id(&self) -> &'static str {
        "privacy"
    }

    fn title(&self) -> SharedString {
        t!("settings-privacy-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let v = store::values(cx);
        let dms = v.dms_from_members;
        let sources = v.friend_sources;
        let has = |flag: u8| sources & flag != 0;
        let (friends, servers, anyone) = (
            has(friend_sources::MUTUAL_FRIENDS),
            has(friend_sources::MUTUAL_SERVERS),
            has(friend_sources::ANYONE),
        );

        page(t!("settings-privacy-title"))
            .child(heading(t!("settings-privacy-dms")))
            .child(
                toggle(
                    "settings-privacy-dms-from-members",
                    t!("settings-privacy-dms-from-members"),
                    dms,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, Change::DmsFromMembers(!dms));
                    cx.notify();
                })),
            )
            .child(note(t!("settings-privacy-dms-note")))
            .child(heading(t!("settings-privacy-friends")))
            .child(
                toggle(
                    "settings-privacy-friends-anyone",
                    t!("settings-privacy-friends-anyone"),
                    anyone,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    let flags = sources ^ friend_sources::ANYONE;
                    store::change(cx, Change::FriendSources(flags));
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "settings-privacy-friends-of-friends",
                    t!("settings-privacy-friends-of-friends"),
                    friends,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    let flags = sources ^ friend_sources::MUTUAL_FRIENDS;
                    store::change(cx, Change::FriendSources(flags));
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "settings-privacy-friends-servers",
                    t!("settings-privacy-friends-servers"),
                    servers,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    let flags = sources ^ friend_sources::MUTUAL_SERVERS;
                    store::change(cx, Change::FriendSources(flags));
                    cx.notify();
                })),
            )
            .when(sources & 7 == 0, |d| {
                d.child(note(t!("settings-privacy-friends-none")))
            })
            .child(note(t!("settings-privacy-friends-note")))
            .into_any_element()
    }
}
