// SPDX-License-Identifier: AGPL-3.0-or-later
//! A group's invite links, as Fluxer has them in two places:
//!
//! - The add-friends window has an invite link for a friend who is not a friend of the
//!   account: "Create" makes one that lasts a day (the server hands back the same one
//!   while it exists), "Copy" puts it on the clipboard.
//! - The invites window (the owner only, as `GroupInvitesModal`) lists the group's
//!   invites with who made them, the code, the uses and the time left or the creation
//!   date, copies a link and revokes an invite after a question.

use gpui::{div, prelude::*, px, AnyElement, ClipboardItem, Context, FontWeight, SharedString};
use inlet_state::ChannelId;

use super::group_dm::{caption, group_owner, notice_line, row_button, title_line, Mode};
use super::{Confirm, Done, Then};
use crate::app::Inlet;
use crate::backend::social::relationships::{Cmd, GroupInvite};
use crate::theme;
use crate::ui::widgets::button;

/// What the invites window has of the group's invites.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    Failed(String),
    Ready(Vec<GroupInvite>),
}

/// The invites window's list and its date toggle.
#[derive(Clone, Debug, PartialEq)]
pub struct InviteList {
    pub load: Load,
    /// Show when an invite was made instead of when it ends.
    pub show_created: bool,
}

/// Width of the columns after the inviter.
const CODE_W: f32 = 120.;
const USES_W: f32 = 70.;
const DATE_W: f32 = 150.;
const ACTIONS_W: f32 = 120.;

/// When an invite ends, as people say it: "Expired", or what is left in days and hours,
/// hours and minutes, or minutes.
pub fn time_left(expires_ms: i64, now_ms: i64) -> String {
    let secs = (expires_ms - now_ms).div_euclid(1000);
    if secs <= 0 {
        return t!("social-invite-expired");
    }
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if days > 0 {
        t!("social-invite-left-days", days = days, hours = hours)
    } else if hours > 0 {
        t!("social-invite-left-hours", hours = hours, minutes = minutes)
    } else {
        t!("social-invite-left-minutes", minutes = minutes.max(1))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Inlet {
    /// Asks for the group's invites (the owner's; anyone else sees why not).
    pub(crate) fn load_group_invites(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let owner = self.account.as_ref().is_some_and(|a| {
            let s = a.read(cx);
            group_owner(s, channel) == Some(s.me_id())
        });
        if let Some(w) = self.social.group_dm.as_mut() {
            let show_created = w.invites.as_ref().is_some_and(|i| i.show_created);
            w.invites = Some(InviteList {
                load: Load::Loading,
                show_created,
            });
        }
        if owner {
            self.social_action(
                Cmd::ListGroupInvites {
                    channel_id: channel.to_string(),
                },
                cx,
            );
        }
    }

    /// The list came back (or why not).
    pub(super) fn invites_loaded(
        &mut self,
        channel_id: &str,
        result: Result<Vec<GroupInvite>, String>,
    ) {
        let Some(channel) = ChannelId::parse(channel_id) else {
            return;
        };
        let Some(w) = self
            .social
            .group_dm
            .as_mut()
            .filter(|w| w.mode == Mode::Invites { channel })
        else {
            return;
        };
        let show_created = w.invites.as_ref().is_some_and(|i| i.show_created);
        w.invites = Some(InviteList {
            load: match result {
                Ok(list) => Load::Ready(list),
                Err(reason) => Load::Failed(reason),
            },
            show_created,
        });
    }

    /// Revokes an invite once the question was answered with yes; the list is read again.
    pub(super) fn revoke_invite(
        &mut self,
        channel: ChannelId,
        code: String,
        cx: &mut Context<Self>,
    ) {
        self.social_action_done(
            Cmd::RevokeInvite {
                channel_id: channel.to_string(),
                code,
            },
            Done::notice(t!("social-invite-revoked"), Then::ReloadInvites(channel)),
            cx,
        );
    }

    /// Puts an invite link on the clipboard.
    fn copy_invite(&mut self, url: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.chat.notice = Some(t!("social-invite-copied"));
        cx.notify();
    }

    /// The parts of the invites window.
    pub(super) fn invites_parts(
        &mut self,
        channel: ChannelId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(w) = self.social.group_dm.as_ref() else {
            return Vec::new();
        };
        let list = w.invites.clone();
        let owner = self.account.as_ref().is_some_and(|a| {
            let s = a.read(cx);
            group_owner(s, channel) == Some(s.me_id())
        });
        let mut parts = vec![title_line(t!("social-group-invites"))];
        let muted = |text: String| {
            div()
                .py_4()
                .flex()
                .justify_center()
                .text_size(px(14.))
                .text_color(theme::text_muted())
                .child(text)
                .into_any_element()
        };
        match (owner, list) {
            (false, _) => parts.push(muted(t!("social-invites-owner-only"))),
            (true, None)
            | (
                true,
                Some(InviteList {
                    load: Load::Loading,
                    ..
                }),
            ) => parts.push(muted(t!("social-invites-loading"))),
            (
                true,
                Some(InviteList {
                    load: Load::Failed(reason),
                    ..
                }),
            ) => {
                parts.push(notice_line(
                    format!("{} {reason}", t!("social-invites-failed")),
                    true,
                ));
                parts.push(
                    div()
                        .flex()
                        .child(
                            button("invites-retry", t!("social-invites-retry"), false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.load_group_invites(channel, cx)
                                }),
                            ),
                        )
                        .into_any_element(),
                );
            }
            (
                true,
                Some(InviteList {
                    load: Load::Ready(invites),
                    show_created,
                }),
            ) => {
                if invites.is_empty() {
                    parts.push(muted(t!("social-invites-none")));
                } else {
                    parts.push(self.invite_toggle(show_created, cx));
                    parts.push(self.invite_table(channel, &invites, show_created, cx));
                }
            }
        }
        parts.extend(self.window_notices());
        parts.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(self.close_button("invites-close", cx))
                .into_any_element(),
        );
        parts
    }

    /// "Show creation date instead of expiration date".
    fn invite_toggle(&mut self, on: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("invites-date-toggle")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(list) = this
                    .social
                    .group_dm
                    .as_mut()
                    .and_then(|w| w.invites.as_mut())
                {
                    list.show_created = !list.show_created;
                }
                cx.notify();
            }))
            .child(
                div()
                    .size(px(18.))
                    .flex_none()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if on {
                        theme::brand()
                    } else {
                        theme::text_muted()
                    })
                    .when(on, |d| d.bg(theme::brand()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(on, |d| {
                        d.child(super::SocialIcon::Check.el(12., theme::on_brand()))
                    }),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(theme::text_muted())
                    .child(t!("social-invites-show-created")),
            )
            .into_any_element()
    }

    /// The header and the rows of the invites.
    fn invite_table(
        &mut self,
        channel: ChannelId,
        invites: &[GroupInvite],
        show_created: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let head = |text: String, width: Option<f32>| {
            let d = div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_muted())
                .child(text.to_uppercase());
            match width {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1().min_w_0(),
            }
        };
        let header = div()
            .px_2()
            .flex()
            .flex_row()
            .gap_2()
            .child(head(t!("social-invites-inviter"), None))
            .child(head(t!("social-invites-code"), Some(CODE_W)))
            .child(head(t!("social-invites-uses"), Some(USES_W)))
            .child(head(
                if show_created {
                    t!("social-invites-created")
                } else {
                    t!("social-invites-expires")
                },
                Some(DATE_W),
            ))
            .child(div().w(px(ACTIONS_W)).flex_none());
        let now = now_ms();
        let rows: Vec<AnyElement> = invites
            .iter()
            .map(|i| {
                let date = if show_created {
                    i.created_ms
                        .map(crate::ui::chat::time::full)
                        .unwrap_or_default()
                } else {
                    i.expires_ms
                        .map(|e| time_left(e, now))
                        .unwrap_or_else(|| t!("social-invite-never"))
                };
                let uses = if i.max_uses > 0 {
                    format!("{} / {}", i.uses, i.max_uses)
                } else {
                    i.uses.to_string()
                };
                let (url, code) = (i.url.clone(), i.code.clone());
                let (copy_id, revoke_id) = (
                    SharedString::from(format!("invite-copy-{}", i.code)),
                    SharedString::from(format!("invite-revoke-{}", i.code)),
                );
                let text = |s: String, w: Option<f32>| {
                    let d = div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(14.))
                        .text_color(theme::text_strong())
                        .child(s);
                    match w {
                        Some(w) => d.w(px(w)).flex_none(),
                        None => d.flex_1().min_w_0(),
                    }
                };
                div()
                    .min_h(px(36.))
                    .px_2()
                    .rounded(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .hover(|s| s.bg(theme::hover_bg()))
                    .child(text(
                        i.inviter
                            .as_ref()
                            .map(|(_, name)| name.clone())
                            .unwrap_or_else(|| t!("social-invite-unknown")),
                        None,
                    ))
                    .child(text(i.code.clone(), Some(CODE_W)))
                    .child(text(uses, Some(USES_W)))
                    .child(text(date, Some(DATE_W)))
                    .child(
                        div()
                            .w(px(ACTIONS_W))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_1()
                            .child(
                                row_button(copy_id, t!("social-invite-copy"), false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        if !url.is_empty() {
                                            this.copy_invite(url.clone(), cx)
                                        }
                                    }),
                                ),
                            )
                            .child(
                                row_button(revoke_id, t!("social-invite-revoke"), true).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.social.confirm = Some(Confirm::RevokeInvite {
                                            channel,
                                            code: code.clone(),
                                        });
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .child(
                div()
                    .id("group-invites-rows")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }

    /// The invite link of the add window: Create makes it, Copy copies it.
    pub(super) fn invite_link_section(
        &mut self,
        channel: ChannelId,
        link: Option<String>,
        copied: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.social_busy(&format!("invite-link:{channel}"));
        let has_link = link.is_some();
        let action = if busy {
            t!("social-sending")
        } else if !has_link {
            t!("social-invite-create")
        } else if copied {
            t!("social-invite-copied-label")
        } else {
            t!("social-invite-copy")
        };
        let shown = link.clone().unwrap_or_else(|| t!("social-invite-generate"));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(caption(t!("social-invite-for-friend")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("group-invite-link")
                            .flex_1()
                            .min_w_0()
                            .h(px(36.))
                            .px_3()
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .bg(theme::bg_app())
                            .text_size(px(14.))
                            .text_color(if has_link {
                                theme::text_strong()
                            } else {
                                theme::text_muted()
                            })
                            .child(shown),
                    )
                    .child(
                        button("group-invite-action", action, true)
                            .when(busy, |b| b.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if busy {
                                    return;
                                }
                                match this.social.group_dm.as_mut().and_then(|w| w.link.clone()) {
                                    Some(url) => {
                                        cx.write_to_clipboard(ClipboardItem::new_string(url));
                                        if let Some(w) = this.social.group_dm.as_mut() {
                                            w.copied = true;
                                        }
                                        cx.notify();
                                    }
                                    None => this.social_action(
                                        Cmd::CreateGroupInvite {
                                            channel_id: channel.to_string(),
                                        },
                                        cx,
                                    ),
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(t!("social-invite-expires-day")),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_left_is_days_and_hours_then_hours_and_minutes_then_minutes() {
        let now = 1_000_000_000_000;
        let left = |s: i64| time_left(now + s * 1000, now);
        assert_eq!(left(0), t!("social-invite-expired"));
        assert_eq!(left(-5), t!("social-invite-expired"));
        assert_eq!(left(30), t!("social-invite-left-minutes", minutes = 1));
        assert_eq!(
            left(59 * 60 + 59),
            t!("social-invite-left-minutes", minutes = 59)
        );
        assert_eq!(
            left(86_400 - 1),
            t!("social-invite-left-hours", hours = 23, minutes = 59)
        );
        assert_eq!(
            left(86_400 + 3 * 3600 + 40),
            t!("social-invite-left-days", days = 1, hours = 3)
        );
    }
}
