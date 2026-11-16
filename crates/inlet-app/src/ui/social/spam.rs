// SPDX-License-Identifier: AGPL-3.0-or-later
//! Messages from people the server flags as spammers (Fluxer's `LocalUserSpamOverride` and
//! the `MESSAGE_GROUP_SPAMMER` rows): each run of them folds into one line ("3 potential
//! spammer messages") like a blocked person's, and a click shows it. The folding happens
//! where the rows are built (`ui/chat/messages.rs`).
//!
//! The server's flag (`SPAMMER`, bit 6 of a user's public flags) can be overridden for
//! this account: "Mark as spam locally" folds someone the server did not flag, "Ignore
//! spam flag" shows someone it did. The marks live in the account's synced preferences
//! (`local_spam_overrides`), so they roam with the account, and the menu items for them
//! show only with developer mode on, as in Fluxer. Your own messages never fold.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::Context;
use inlet_protocol as p;
use inlet_state::UserId;

use crate::app::Inlet;
use crate::backend::composer::gif::PrefEdit;

/// The `SPAMMER` public user flag.
pub const SPAMMER_FLAG: u64 = 1 << 6;

/// The account's local marks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpamLists {
    spammer: HashSet<UserId>,
    not_spammer: HashSet<UserId>,
}

impl SpamLists {
    /// The lists from the stored IDs; an entry that is not an ID is dropped.
    pub fn from_ids(spammer: &[String], not_spammer: &[String]) -> Self {
        let ids = |list: &[String]| -> HashSet<UserId> {
            list.iter().filter_map(|s| UserId::parse(s)).collect()
        };
        SpamLists {
            spammer: ids(spammer),
            not_spammer: ids(not_spammer),
        }
    }

    /// Fluxer's `isUserMarkedAsSpammer`: a mark of "not a spammer" wins, then a mark of
    /// "spammer", then the server's flag.
    pub fn is_spammer(&self, user: UserId, flags: u64) -> bool {
        if self.not_spammer.contains(&user) {
            return false;
        }
        self.spammer.contains(&user) || flags & SPAMMER_FLAG != 0
    }

    pub fn marked_spammer(&self, user: UserId) -> bool {
        self.spammer.contains(&user)
    }

    pub fn marked_not_spammer(&self, user: UserId) -> bool {
        self.not_spammer.contains(&user)
    }
}

/// The public flags of a message's author.
pub fn user_flags(m: &p::Message) -> u64 {
    m.author
        .flags
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

impl Inlet {
    /// Whether developer mode is on (from the account's settings, READY and updates).
    pub(crate) fn spam_on_dispatch(&mut self, d: &p::Dispatch) {
        match d {
            p::Dispatch::Ready(ready) => {
                self.profile.developer_mode = ready
                    .extra
                    .get("user_settings")
                    .and_then(|s| s.get("developer_mode"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
            }
            p::Dispatch::UserSettingsUpdate(s) => self.profile.developer_mode = s.developer_mode,
            _ => {}
        }
    }

    /// The marks follow the synced preferences: the lists are rebuilt, and the rows
    /// with them when they changed.
    pub(crate) fn spam_follow_prefs(&mut self, cx: &mut Context<Self>) {
        let (spammer, not_spammer) = self.composer.pickers.prefs.spam_marks();
        let next = SpamLists::from_ids(spammer, not_spammer);
        if *self.chat.view.spam == next {
            return;
        }
        let next = Rc::new(next);
        self.chat.view.spam = next.clone();
        self.refresh_rows(cx);
        if let Some(panel) = self.threads.panel.as_mut() {
            panel.view.spam = next;
            self.refresh_panel_rows(cx);
        }
        cx.notify();
    }

    /// Marks a person (`Some(true)` spammer, `Some(false)` ignore the flag, `None`
    /// clear); shown at once, saved with the synced preferences.
    pub(crate) fn mark_spam(&mut self, user: UserId, mark: Option<bool>, cx: &mut Context<Self>) {
        self.edit_prefs(PrefEdit::SpamMark {
            user_id: user.to_string(),
            mark,
        });
        self.spam_follow_prefs(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::composer::pickers::prefs::PickerPrefs;

    fn id(n: u64) -> UserId {
        UserId::parse(&n.to_string()).unwrap()
    }

    #[test]
    fn the_server_flag_and_the_local_marks_decide_together() {
        let lists = SpamLists::from_ids(&["10".into(), "x".into()], &["20".into()]);
        // Not flagged, not marked.
        assert!(!lists.is_spammer(id(30), 0));
        // The server's flag alone.
        assert!(lists.is_spammer(id(30), SPAMMER_FLAG));
        // Other flags do not count.
        assert!(!lists.is_spammer(id(30), 1 << 5));
        // Marked as a spammer without the flag.
        assert!(lists.is_spammer(id(10), 0));
        // "Ignore the flag" wins over the server's flag.
        assert!(!lists.is_spammer(id(20), SPAMMER_FLAG));
        // And over a mark of spammer, if both were ever stored.
        let both = SpamLists::from_ids(&["20".into()], &["20".into()]);
        assert!(!both.is_spammer(id(20), SPAMMER_FLAG));
        assert!(lists.marked_spammer(id(10)) && lists.marked_not_spammer(id(20)));
    }

    #[test]
    fn marks_are_stored_one_per_person_in_the_synced_preferences() {
        let mut prefs = PickerPrefs::default();
        prefs.edit(PrefEdit::SpamMark {
            user_id: "10".into(),
            mark: Some(true),
        });
        assert_eq!(prefs.spam_marks().0, ["10"]);
        // Changing the mark moves the person to the other list.
        prefs.edit(PrefEdit::SpamMark {
            user_id: "10".into(),
            mark: Some(false),
        });
        assert!(prefs.spam_marks().0.is_empty());
        assert_eq!(prefs.spam_marks().1, ["10"]);
        // Clearing takes it out of both.
        prefs.edit(PrefEdit::SpamMark {
            user_id: "10".into(),
            mark: None,
        });
        assert!(prefs.spam_marks().0.is_empty() && prefs.spam_marks().1.is_empty());
        // Something that is not an ID is never written.
        prefs.edit(PrefEdit::SpamMark {
            user_id: "../1".into(),
            mark: Some(true),
        });
        assert!(prefs.spam_marks().0.is_empty());
    }

    #[test]
    fn the_flag_is_read_from_the_authors_public_flags() {
        let m: p::Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "content": "hi", "type": 0,
            "timestamp": "2026-01-01T00:00:00Z",
            "author": {"id": "3", "username": "a", "flags": 64}
        }))
        .unwrap();
        assert_eq!(user_flags(&m), SPAMMER_FLAG);
        let none: p::Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "content": "hi", "type": 0,
            "timestamp": "2026-01-01T00:00:00Z",
            "author": {"id": "3", "username": "a"}
        }))
        .unwrap();
        assert_eq!(user_flags(&none), 0);
    }
}
