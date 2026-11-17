// SPDX-License-Identifier: AGPL-3.0-or-later
//! The nickname windows: the private nickname for a friend (Fluxer's
//! `ChangeFriendNicknameModal`) and someone's nickname in a group DM
//! (`ChangeGroupDMNicknameModal`). Both are Fluxer's `BaseChangeNicknameModal`: a field
//! with the person's name as its placeholder, a button that clears it, and Save (Enter
//! saves too). A blank field clears the nickname; at most 32 characters.
//!
//! The window is the group window in a nickname mode (see `group_dm`), so the keyboard
//! rules of layers apply as for the other social windows.

use gpui::{div, prelude::*, px, AnyElement, Context, Window};
use inlet_state::UserId;

use super::group_dm::{caption, row_button, title_line, Mode};
use super::{person_name, Done, Then};
use crate::app::Inlet;
use crate::backend::social::relationships::{clean_nickname, Cmd, GroupChange};
use crate::theme;
use crate::ui::widgets::button;

/// Longest nickname, in characters (Fluxer's own limit in its dialog).
pub const MAX_NICKNAME_CHARS: usize = 32;

impl Inlet {
    /// Opens the window for the private nickname of a friend.
    pub(crate) fn open_friend_nickname(
        &mut self,
        user: UserId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_group_dm(Mode::FriendNickname { user }, window, cx);
    }

    /// Saves the nickname in the field: the friend's private one, or the one in the group.
    pub(super) fn save_nickname(&mut self, cx: &mut Context<Self>) {
        let Some(w) = self.social.group_dm.as_ref() else {
            return;
        };
        let (mode, text) = (
            w.mode,
            w.name
                .as_ref()
                .map(|n| n.read(cx).text().to_owned())
                .unwrap_or_default(),
        );
        if self.social_busy(&w.key()) {
            return;
        }
        let nickname = match clean_nickname(&text) {
            Ok(n) => n,
            Err(reason) => {
                if let Some(w) = self.social.group_dm.as_mut() {
                    w.message = Some(reason);
                }
                cx.notify();
                return;
            }
        };
        if let Some(w) = self.social.group_dm.as_mut() {
            w.message = None;
        }
        let done = Done::notice(t!("social-nickname-updated"), Then::Close);
        match mode {
            Mode::FriendNickname { user } => self.social_action_done(
                Cmd::SetFriendNickname {
                    user_id: user.to_string(),
                    nickname,
                },
                done,
                cx,
            ),
            Mode::GroupNickname { channel, user } => self.edit_group_dm(
                channel,
                GroupChange {
                    nicks: vec![(user.to_string(), nickname)],
                    ..Default::default()
                },
                Some(done),
                cx,
            ),
            _ => {}
        }
    }

    /// The parts of the window: the field, its Clear button and Save.
    pub(super) fn nickname_parts(&mut self, mode: Mode, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(w) = self.social.group_dm.as_ref() else {
            return Vec::new();
        };
        let input = w.name.clone();
        let busy = self.social_busy(&w.key());
        let typed = input
            .as_ref()
            .is_some_and(|n| !n.read(cx).text().is_empty());
        let user = match mode {
            Mode::FriendNickname { user } | Mode::GroupNickname { user, .. } => user,
            _ => return Vec::new(),
        };
        let whose = match self.account.as_ref().map(|a| a.read(cx)) {
            Some(state) if user == state.me_id() => {
                crate::model::clean_name(state.me().display_name())
            }
            Some(state) => super::person_user(state, user)
                .map(|u| crate::model::clean_name(u.display_name()))
                .unwrap_or_else(|| person_name(state, user)),
            None => String::new(),
        };
        let mut parts = vec![
            title_line(t!("social-nickname-title")),
            div()
                .text_size(px(14.))
                .text_color(theme::text_muted())
                .child(t!("social-nickname-for", name = whose))
                .into_any_element(),
            caption(t!("social-nickname")),
        ];
        let clear = typed.then(|| {
            let input = input.clone();
            row_button("nickname-clear", t!("social-nickname-clear"), false).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(i) = &input {
                        i.update(cx, |i, cx| i.clear(cx));
                    }
                    if let Some(w) = this.social.group_dm.as_mut() {
                        w.message = None;
                    }
                    cx.notify();
                },
            ))
        });
        parts.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().children(input))
                .children(clear)
                .into_any_element(),
        );
        parts.extend(self.window_notices());
        parts.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(self.close_button("nickname-cancel", cx))
                .child(
                    button(
                        "nickname-save",
                        if busy {
                            t!("social-sending")
                        } else {
                            t!("social-save")
                        },
                        true,
                    )
                    .when(busy, |b| b.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.save_nickname(cx))),
                )
                .into_any_element(),
        );
        parts
    }
}
