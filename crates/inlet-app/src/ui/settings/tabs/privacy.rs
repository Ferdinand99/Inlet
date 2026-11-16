// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Privacy tab, with the options of Fluxer's Privacy and safety page: who may send
//! a friend request, direct messages from the members and bots of communities (with the
//! question whether to apply a change to the communities already joined), who may call
//! and who may add to group chats, who sees the full profile, and what to do with
//! sensitive media. All are the account's own settings on the server. Exporting data and
//! deleting the account are Fluxer's pages, not here.

use gpui::{div, prelude::*, px, AnyElement, Context, SharedString};

use super::super::controls::{page, radio, segment};
use super::super::store;
use super::super::sync::friend_sources;
use super::super::{with_app, DmQuestion, SettingsTab, SettingsView};
use super::desktop::{heading, note, toggle};
use crate::backend::settings::write::Change;
use crate::theme;
use crate::ui::widgets::button;

pub struct PrivacyTab;

/// What a choice of who may call, or add to group chats, comes to (Fluxer's flags).
pub mod who {
    pub const FRIENDS_OF_FRIENDS: u8 = 1;
    pub const SERVER_MEMBERS: u8 = 2;
    pub const EVERYONE: u8 = 4;
    pub const FRIENDS_ONLY: u8 = 8;
    pub const NOBODY: u8 = 16;
    /// Calls only: they come through without ringing.
    pub const SILENT: u8 = 32;

    /// The choice the flags stand for.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Base {
        Nobody,
        FriendsOnly,
        Custom,
        Everyone,
    }

    /// Which choice the flags are: nobody beats everyone beats friends only; anything
    /// else is a custom set.
    pub fn base(flags: u8) -> Base {
        if flags & NOBODY != 0 {
            Base::Nobody
        } else if flags & EVERYONE != 0 {
            Base::Everyone
        } else if flags & FRIENDS_ONLY != 0 {
            Base::FriendsOnly
        } else {
            Base::Custom
        }
    }

    /// The flags after picking `base` (Fluxer's `nextFlagsForBaseChange`): a custom set
    /// keeps the groups it had, or starts with friends of friends. The silent flag goes.
    pub fn with_base(flags: u8, base: Base) -> u8 {
        match base {
            Base::Nobody => NOBODY,
            Base::FriendsOnly => FRIENDS_ONLY,
            Base::Everyone => EVERYONE,
            Base::Custom => match flags & (FRIENDS_OF_FRIENDS | SERVER_MEMBERS) {
                0 => FRIENDS_OF_FRIENDS,
                groups => groups,
            },
        }
    }

    /// The flags after switching one group on or off in a custom set. With none left it
    /// falls back to friends only.
    pub fn with_group(flags: u8, group: u8, on: bool) -> u8 {
        let silent = flags & SILENT;
        let mut groups = flags & !(NOBODY | FRIENDS_ONLY | EVERYONE | SILENT);
        if on {
            groups |= group;
        } else {
            groups &= !group;
        }
        silent | if groups == 0 { FRIENDS_ONLY } else { groups }
    }
}

/// The three places a sensitive media setting is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensitiveRow {
    FriendDms,
    OtherDms,
    Servers,
}

/// The values Fluxer offers for a row: show, blur and block for direct messages; only show
/// and blur for community channels (its API refuses block there).
pub fn sensitive_offered(row: SensitiveRow) -> &'static [u8] {
    match row {
        SensitiveRow::Servers => &[0, 1],
        SensitiveRow::FriendDms | SensitiveRow::OtherDms => &[0, 1, 2],
    }
}

/// Whether a value may be picked. An account that may not see mature content (`adult`
/// false) cannot show sensitive media in direct messages and cannot change the community
/// filter at all: Fluxer's server refuses both, and its own client disables them.
pub fn sensitive_enabled(row: SensitiveRow, adult: bool, value: u8) -> bool {
    match row {
        SensitiveRow::Servers => adult,
        SensitiveRow::FriendDms | SensitiveRow::OtherDms => adult || value != 0,
    }
}

/// The flags of Fluxer's friend request switches after one is switched.
pub fn toggled_sources(sources: u8, flag: u8) -> u8 {
    (sources ^ flag) & 7
}

impl SettingsTab for PrivacyTab {
    fn id(&self) -> &'static str {
        "privacy"
    }

    fn title(&self) -> SharedString {
        t!("settings-privacy-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let v = store::values(cx);
        let question = with_app(cx, |app, _| app.settings.dm_question).flatten();
        let mut col = page(t!("settings-privacy-title"));
        col = friend_requests(col, v.friend_sources, cx);
        col = direct_messages(
            col,
            v.dms_from_members,
            v.bot_dms_from_members,
            question,
            cx,
        );
        col = communication(
            col,
            Communication {
                heading: t!("settings-privacy-calls"),
                hint: t!("settings-privacy-calls-hint"),
                flags: v.call_flags,
                ids: [
                    "settings-calls-nobody",
                    "settings-calls-friends",
                    "settings-calls-custom",
                    "settings-calls-everyone",
                    "settings-calls-fof",
                    "settings-calls-servers",
                    "settings-calls-silent",
                ],
                labels: [
                    t!("settings-privacy-calls-nobody"),
                    t!("settings-privacy-calls-friends"),
                    t!("settings-privacy-calls-custom"),
                    t!("settings-privacy-calls-everyone"),
                ],
                silent: true,
                make: Change::IncomingCalls,
            },
            cx,
        );
        col = communication(
            col,
            Communication {
                heading: t!("settings-privacy-groups"),
                hint: t!("settings-privacy-groups-hint"),
                flags: v.group_dm_flags,
                ids: [
                    "settings-groups-nobody",
                    "settings-groups-friends",
                    "settings-groups-custom",
                    "settings-groups-everyone",
                    "settings-groups-fof",
                    "settings-groups-servers",
                    "settings-groups-silent",
                ],
                labels: [
                    t!("settings-privacy-groups-nobody"),
                    t!("settings-privacy-groups-friends"),
                    t!("settings-privacy-groups-custom"),
                    t!("settings-privacy-groups-everyone"),
                ],
                silent: false,
                make: Change::GroupDmAdd,
            },
            cx,
        );
        col = profile_privacy(col, v.profile_privacy, cx);
        // `nsfw_allowed` of the account, as Fluxer's `matureContentAllowed`.
        let adult = with_app(cx, |app, cx| {
            app.account
                .as_ref()
                .is_some_and(|a| a.read(cx).me().nsfw_allowed())
        })
        .unwrap_or(false);
        col = sensitive(
            col,
            [v.sensitive_friends, v.sensitive_others, v.sensitive_servers],
            adult,
            cx,
        );
        col.into_any_element()
    }
}

fn friend_requests(col: gpui::Div, sources: u8, cx: &mut Context<SettingsView>) -> gpui::Div {
    let anyone = sources & friend_sources::ANYONE != 0;
    let has = |flag: u8| anyone || sources & flag != 0;
    let switch = |flag: u8| {
        move |_: &mut SettingsView,
              _: &gpui::ClickEvent,
              _: &mut gpui::Window,
              cx: &mut Context<SettingsView>| {
            // Everyone covers the other two: they cannot be changed meanwhile.
            if anyone && flag != friend_sources::ANYONE {
                return;
            }
            store::change(cx, Change::FriendSources(toggled_sources(sources, flag)));
            cx.notify();
        }
    };
    col.child(heading(t!("settings-privacy-friends").to_uppercase()))
        .child(note(t!("settings-privacy-friends-hint")).pl_0())
        .child(
            toggle(
                "settings-privacy-friends-anyone",
                t!("settings-privacy-friends-anyone"),
                anyone,
                true,
            )
            .on_click(cx.listener(switch(friend_sources::ANYONE))),
        )
        .child(note(t!("settings-privacy-friends-anyone-hint")))
        .child(
            toggle(
                "settings-privacy-friends-of-friends",
                t!("settings-privacy-friends-of-friends"),
                has(friend_sources::MUTUAL_FRIENDS),
                !anyone,
            )
            .on_click(cx.listener(switch(friend_sources::MUTUAL_FRIENDS))),
        )
        .child(note(t!("settings-privacy-friends-of-friends-hint")))
        .child(
            toggle(
                "settings-privacy-friends-servers",
                t!("settings-privacy-friends-servers"),
                has(friend_sources::MUTUAL_SERVERS),
                !anyone,
            )
            .on_click(cx.listener(switch(friend_sources::MUTUAL_SERVERS))),
        )
        .child(note(t!("settings-privacy-friends-servers-hint")))
}

fn direct_messages(
    col: gpui::Div,
    members: bool,
    bots: bool,
    question: Option<DmQuestion>,
    cx: &mut Context<SettingsView>,
) -> gpui::Div {
    // Turning it on or off asks whether the communities already joined follow, when
    // there are any (Fluxer's question); without any the change is made at once.
    let ask = |bots: bool, allow: bool| {
        move |_: &mut SettingsView,
              _: &gpui::ClickEvent,
              _: &mut gpui::Window,
              cx: &mut Context<SettingsView>| {
            let joined = with_app(cx, |app, _| !app.guilds.is_empty()).unwrap_or(false);
            if joined {
                with_app(cx, |app, cx| {
                    app.settings.dm_question = Some(DmQuestion { bots, allow });
                    cx.notify();
                });
            } else {
                store::change(
                    cx,
                    if bots {
                        Change::BotDmsFromMembers(allow)
                    } else {
                        Change::DmsFromMembers(allow)
                    },
                );
            }
            cx.notify();
        }
    };
    let mut col = col
        .child(heading(t!("settings-privacy-dms").to_uppercase()))
        .child(note(t!("settings-privacy-dms-hint")).pl_0())
        .child(
            toggle(
                "settings-privacy-dms-from-members",
                t!("settings-privacy-dms-from-members"),
                members,
                true,
            )
            .on_click(cx.listener(ask(false, !members))),
        )
        .child(note(t!("settings-privacy-dms-note")))
        .child(
            toggle(
                "settings-privacy-dms-from-bots",
                t!("settings-privacy-dms-from-bots"),
                bots,
                true,
            )
            .on_click(cx.listener(ask(true, !bots))),
        )
        .child(note(t!("settings-privacy-dms-bots-note")));
    if let Some(q) = question {
        let change = if q.bots {
            Change::BotDmsFromMembers(q.allow)
        } else {
            Change::DmsFromMembers(q.allow)
        };
        let title = match (q.bots, q.allow) {
            (false, true) => t!("settings-privacy-ask-members-allow"),
            (false, false) => t!("settings-privacy-ask-members-block"),
            (true, true) => t!("settings-privacy-ask-bots-allow"),
            (true, false) => t!("settings-privacy-ask-bots-block"),
        };
        let apply_label = if q.allow {
            t!("settings-privacy-ask-allow-all")
        } else {
            t!("settings-privacy-ask-block-all")
        };
        col = col.child(
            div()
                .mt_3()
                .p_4()
                .rounded(px(8.))
                .bg(theme::bg_textarea())
                .border_1()
                .border_color(theme::accent_border())
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_color(theme::text_strong())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::text_muted())
                        .child(t!("settings-privacy-ask-body")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            button("settings-privacy-ask-apply", apply_label, true).on_click(
                                cx.listener(move |_, _, _, cx| {
                                    let seq = store::change(cx, change);
                                    // Every community joined: none may send (or all may).
                                    let ids: Vec<String> = with_app(cx, |app, _| {
                                        app.guilds.iter().map(|g| g.id.clone()).collect()
                                    })
                                    .unwrap_or_default();
                                    let list = if q.allow { Vec::new() } else { ids };
                                    store::restrict_guilds(cx, seq, q.bots, list);
                                    with_app(cx, |app, _| app.settings.dm_question = None);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            button(
                                "settings-privacy-ask-skip",
                                t!("settings-privacy-ask-skip"),
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    store::change(cx, change);
                                    with_app(cx, |app, _| app.settings.dm_question = None);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            button(
                                "settings-privacy-ask-cancel",
                                t!("settings-privacy-ask-cancel"),
                                false,
                            )
                            .on_click(cx.listener(|_, _, _, cx| {
                                with_app(cx, |app, _| app.settings.dm_question = None);
                                cx.notify();
                            })),
                        ),
                ),
        );
    }
    col
}

/// One of the two "who may" sections: calls and group chats.
struct Communication {
    heading: String,
    hint: String,
    flags: u8,
    /// Element IDs: nobody, friends, custom, everyone, friends of friends, server
    /// members, silent.
    ids: [&'static str; 7],
    /// Nobody, friends only, custom, everyone.
    labels: [String; 4],
    /// Offers the switch for calls that do not ring.
    silent: bool,
    make: fn(u8) -> Change,
}

fn communication(col: gpui::Div, c: Communication, cx: &mut Context<SettingsView>) -> gpui::Div {
    let flags = c.flags;
    let base = who::base(flags);
    let make = c.make;
    let [nobody, friends, custom, everyone] = c.labels;
    let pick =
        |id: &'static str, label: String, value: who::Base, cx: &mut Context<SettingsView>| {
            radio(id, label, None, base == value).on_click(cx.listener(move |_, _, _, cx| {
                store::change(cx, make(who::with_base(flags, value)));
                cx.notify();
            }))
        };
    let mut col = col
        .child(heading(c.heading.to_uppercase()))
        .child(note(c.hint).pl_0())
        .child(pick(c.ids[0], nobody, who::Base::Nobody, cx))
        .child(pick(c.ids[1], friends, who::Base::FriendsOnly, cx))
        .child(pick(c.ids[2], custom, who::Base::Custom, cx))
        .child(pick(c.ids[3], everyone, who::Base::Everyone, cx));
    if base == who::Base::Custom {
        let group = |id: &'static str, label: String, bit: u8, cx: &mut Context<SettingsView>| {
            let on = flags & bit != 0;
            toggle(id, label, on, true).on_click(cx.listener(move |_, _, _, cx| {
                store::change(cx, make(who::with_group(flags, bit, !on)));
                cx.notify();
            }))
        };
        col = col
            .child(group(
                c.ids[4],
                t!("settings-privacy-friends-of-friends"),
                who::FRIENDS_OF_FRIENDS,
                cx,
            ))
            .child(group(
                c.ids[5],
                t!("settings-privacy-friends-servers"),
                who::SERVER_MEMBERS,
                cx,
            ));
    }
    if c.silent && base != who::Base::Nobody {
        let on = flags & who::SILENT != 0;
        col = col
            .child(
                toggle(c.ids[6], t!("settings-privacy-calls-silent"), on, true).on_click(
                    cx.listener(move |_, _, _, cx| {
                        let to = if on {
                            flags & !who::SILENT
                        } else {
                            flags | who::SILENT
                        };
                        store::change(cx, make(to));
                        cx.notify();
                    }),
                ),
            )
            .child(note(t!("settings-privacy-calls-silent-hint")));
    }
    col
}

fn profile_privacy(col: gpui::Div, level: u8, cx: &mut Context<SettingsView>) -> gpui::Div {
    let pick = |id: &'static str,
                label: String,
                hint: String,
                value: u8,
                cx: &mut Context<SettingsView>| {
        radio(id, label, Some(hint), level == value).on_click(cx.listener(move |_, _, _, cx| {
            store::change(cx, Change::ProfilePrivacy(value));
            cx.notify();
        }))
    };
    col.child(heading(t!("settings-privacy-profile").to_uppercase()))
        .child(pick(
            "settings-profile-privacy-all",
            t!("settings-privacy-profile-all"),
            t!("settings-privacy-profile-all-hint"),
            0,
            cx,
        ))
        .child(pick(
            "settings-profile-privacy-small",
            t!("settings-privacy-profile-small"),
            t!("settings-privacy-profile-small-hint"),
            1,
            cx,
        ))
        .child(pick(
            "settings-profile-privacy-friends",
            t!("settings-privacy-profile-friends"),
            t!("settings-privacy-profile-friends-hint"),
            2,
            cx,
        ))
}

/// Show, blur or block, for three kinds of place.
fn sensitive(
    col: gpui::Div,
    levels: [u8; 3],
    adult: bool,
    cx: &mut Context<SettingsView>,
) -> gpui::Div {
    let row = |label: String,
               hint: String,
               kind: SensitiveRow,
               ids: [&'static str; 3],
               level: u8,
               make: fn(u8) -> Change,
               cx: &mut Context<SettingsView>| {
        let seg = |id: &'static str,
                   label: String,
                   value: u8,
                   cx: &mut Context<SettingsView>|
         -> Option<AnyElement> {
            if !sensitive_offered(kind).contains(&value) {
                return None;
            }
            let on = sensitive_enabled(kind, adult, value);
            let seg = segment(id, label, level == value);
            Some(if on {
                seg.on_click(cx.listener(move |_, _, _, cx| {
                    store::change(cx, make(value));
                    cx.notify();
                }))
                .into_any_element()
            } else {
                // Not for this account: dimmed, and a click does nothing.
                seg.opacity(0.4).into_any_element()
            })
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .child(div().text_color(theme::text_strong()).child(label))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(hint),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .children(seg(ids[0], t!("settings-privacy-sensitive-show"), 0, cx))
                    .children(seg(ids[1], t!("settings-privacy-sensitive-blur"), 1, cx))
                    .children(seg(ids[2], t!("settings-privacy-sensitive-block"), 2, cx)),
            )
            .when(!adult && kind == SensitiveRow::Servers, |d| {
                d.child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::text_muted())
                        .child(t!("settings-privacy-sensitive-age")),
                )
            })
    };
    col.child(heading(t!("settings-privacy-sensitive").to_uppercase()))
        .child(note(t!("settings-privacy-sensitive-hint")).pl_0())
        .child(row(
            t!("settings-privacy-sensitive-friends"),
            t!("settings-privacy-sensitive-friends-hint"),
            SensitiveRow::FriendDms,
            [
                "settings-sensitive-friends-show",
                "settings-sensitive-friends-blur",
                "settings-sensitive-friends-block",
            ],
            levels[0],
            Change::SensitiveFriends,
            cx,
        ))
        .child(row(
            t!("settings-privacy-sensitive-others"),
            t!("settings-privacy-sensitive-others-hint"),
            SensitiveRow::OtherDms,
            [
                "settings-sensitive-others-show",
                "settings-sensitive-others-blur",
                "settings-sensitive-others-block",
            ],
            levels[1],
            Change::SensitiveOthers,
            cx,
        ))
        .child(row(
            t!("settings-privacy-sensitive-servers"),
            t!("settings-privacy-sensitive-servers-hint"),
            SensitiveRow::Servers,
            [
                "settings-sensitive-servers-show",
                "settings-sensitive-servers-blur",
                "settings-sensitive-servers-block",
            ],
            levels[2],
            Change::SensitiveServers,
            cx,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::settings::sync::Values;
    use serde_json::json;

    /// A person who allowed nobody to send a friend request in Fluxer picks one group
    /// here: exactly that group is written, never the wider set the defaults would show.
    #[test]
    fn the_privacy_tab_never_writes_wider_flags_than_the_user_picked() {
        let s: inlet_protocol::UserSettings = serde_json::from_value(json!({
            "theme": "dark", "friend_source_flags": 0, "default_guilds_restricted": true,
            "incoming_call_flags": 16, "group_dm_add_permission_flags": 16
        }))
        .unwrap();
        let v = Values::from_server(&s, None, &Values::default());
        assert_eq!(
            v.friend_sources, 0,
            "what the tab shows is what the server holds"
        );
        assert_eq!(
            toggled_sources(v.friend_sources, friend_sources::MUTUAL_FRIENDS),
            1
        );
        assert_eq!(toggled_sources(v.friend_sources, friend_sources::ANYONE), 4);
        assert_eq!(toggled_sources(4, friend_sources::ANYONE), 0);
        assert_eq!(v.call_flags, 16);
        assert_eq!(who::base(v.call_flags), who::Base::Nobody);
        // Someone who blocked calls and picks "friends only" gets that and no more.
        assert_eq!(
            who::with_base(v.call_flags, who::Base::FriendsOnly),
            who::FRIENDS_ONLY
        );
    }

    /// What Fluxer's server accepts: no block for community channels, and for an account
    /// that may not see mature content no show in direct messages and no change at all
    /// to the community filter.
    #[test]
    fn the_sensitive_options_are_the_ones_fluxer_accepts() {
        use SensitiveRow::*;
        assert_eq!(
            sensitive_offered(Servers),
            [0, 1],
            "no block for communities"
        );
        assert_eq!(sensitive_offered(FriendDms), [0, 1, 2]);
        assert_eq!(sensitive_offered(OtherDms), [0, 1, 2]);
        for value in [0, 1] {
            assert!(sensitive_enabled(Servers, true, value));
            assert!(
                !sensitive_enabled(Servers, false, value),
                "an under-18 account cannot change it"
            );
        }
        for row in [FriendDms, OtherDms] {
            assert!(
                !sensitive_enabled(row, false, 0),
                "no show for an under-18 account"
            );
            assert!(sensitive_enabled(row, false, 1) && sensitive_enabled(row, false, 2));
            assert!((0..3).all(|v| sensitive_enabled(row, true, v)));
        }
    }

    #[test]
    fn the_who_flags_follow_fluxers_rules() {
        use who::*;
        assert_eq!(base(0), Base::Custom);
        assert_eq!(base(FRIENDS_ONLY | SILENT), Base::FriendsOnly);
        assert_eq!(base(NOBODY | EVERYONE), Base::Nobody);
        // A custom set keeps its groups, or starts with friends of friends.
        assert_eq!(with_base(SERVER_MEMBERS, Base::Custom), SERVER_MEMBERS);
        assert_eq!(with_base(FRIENDS_ONLY, Base::Custom), FRIENDS_OF_FRIENDS);
        // The silent flag goes with a change of base, as in Fluxer.
        assert_eq!(
            with_base(EVERYONE | SILENT, Base::FriendsOnly),
            FRIENDS_ONLY
        );
        // Switching groups: none left falls back to friends only.
        assert_eq!(
            with_group(FRIENDS_OF_FRIENDS, SERVER_MEMBERS, true),
            FRIENDS_OF_FRIENDS | SERVER_MEMBERS
        );
        assert_eq!(
            with_group(FRIENDS_OF_FRIENDS, FRIENDS_OF_FRIENDS, false),
            FRIENDS_ONLY
        );
        assert_eq!(
            with_group(FRIENDS_OF_FRIENDS | SILENT, SERVER_MEMBERS, true),
            FRIENDS_OF_FRIENDS | SERVER_MEMBERS | SILENT
        );
    }
}
