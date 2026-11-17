//! Windows of channels that are not on screen keep only their newest messages: cut when the
//! channel is left, kept to that as live messages arrive, never cut for the open channel or
//! the thread panel's, and loaded back by the history paging when the channel is scrolled.

mod common;

use common::*;
use inlet_state::limits::{MAX_BACKGROUND_MESSAGES, MAX_MESSAGES_PER_CHANNEL};
use inlet_state::{AccountState, MessagesPage};
use serde_json::json;

/// Opens the window of `channel` and fills it with `n` live messages, `base` seconds in.
fn fill(s: &mut AccountState, channel: &str, n: u64, base: u64) {
    s.begin_load(ch(channel));
    s.apply_messages(ch(channel), MessagesPage::latest(vec![], 50));
    for i in 0..n {
        s.apply(&create(mid(base + i, 0), channel, OTHER, json!({})));
    }
}

fn len(s: &AccountState, channel: &str) -> usize {
    s.messages(ch(channel)).map_or(0, |w| w.len())
}

#[test]
fn leaving_a_channel_cuts_its_window_to_the_newest_messages() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 300, 10);
    assert_eq!(len(&s, GENERAL), 300, "the open channel keeps what it has");
    let newest = s.messages(ch(GENERAL)).unwrap().newest_id();

    let c = s.set_open_channel(Some(ch(RANDOM)));
    assert!(c.messages.contains(&ch(GENERAL)), "the cut is reported");
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_BACKGROUND_MESSAGES);
    assert_eq!(
        w.newest_id(),
        newest,
        "the newest messages are the ones kept"
    );
    assert!(w.has_more_before(), "older ones can be loaded again");
    assert!(w.has_newest(), "live messages still land in it");
    assert_eq!(w.oldest_id(), Some(mid(10 + 200, 0)));
}

#[test]
fn the_open_channel_and_the_thread_panels_channel_are_never_cut() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    s.set_open_thread(Some(ch(RANDOM)));
    fill(&mut s, GENERAL, MAX_MESSAGES_PER_CHANNEL as u64 + 50, 10);
    fill(&mut s, RANDOM, 300, 10);
    fill(&mut s, DM, 300, 10);
    // The open ones keep their caps; the DM is not shown.
    assert_eq!(len(&s, GENERAL), MAX_MESSAGES_PER_CHANNEL);
    assert_eq!(len(&s, RANDOM), 300);
    // A window made for a channel that is not shown is cut as it fills.
    assert_eq!(len(&s, DM), MAX_BACKGROUND_MESSAGES);

    // Showing another channel in the chat cuts the chat's old one, not the panel's.
    s.set_open_channel(Some(ch(DM)));
    assert_eq!(len(&s, GENERAL), MAX_BACKGROUND_MESSAGES);
    assert_eq!(len(&s, RANDOM), 300);
    // Closing the panel cuts the thread.
    let c = s.set_open_thread(None);
    assert!(c.messages.contains(&ch(RANDOM)));
    assert_eq!(len(&s, RANDOM), MAX_BACKGROUND_MESSAGES);
    // The channel on screen grows past the limit.
    fill(&mut s, DM, 200, 2000);
    assert!(len(&s, DM) > MAX_BACKGROUND_MESSAGES);
}

#[test]
fn live_messages_into_a_window_that_is_not_open_keep_it_at_the_limit() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 250, 10);
    s.set_open_channel(Some(ch(RANDOM)));
    for i in 0..80 {
        s.apply(&create(mid(1000 + i, 0), GENERAL, OTHER, json!({})));
    }
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_BACKGROUND_MESSAGES);
    assert_eq!(w.newest_id(), Some(mid(1079, 0)));
}

#[test]
fn scrolling_back_in_a_cut_window_loads_older_pages_again() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 300, 1000);
    s.set_open_channel(Some(ch(RANDOM)));
    assert_eq!(len(&s, GENERAL), MAX_BACKGROUND_MESSAGES);

    // The channel is shown again: its window is there at once, and the pager asks for the
    // page before the oldest message it holds, as for any window.
    s.set_open_channel(Some(ch(GENERAL)));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.has_more_before());
    let mut oldest = w.oldest_id().unwrap();
    let mut t = 1200;
    for _ in 0..8 {
        t -= 50;
        let page: Vec<_> = (0..50)
            .map(|i| message(mid(t + i, 1), GENERAL, OTHER))
            .collect();
        s.apply_messages(ch(GENERAL), MessagesPage::before(oldest, page, 50));
        oldest = s.messages(ch(GENERAL)).unwrap().oldest_id().unwrap();
    }
    assert!(
        len(&s, GENERAL) >= MAX_BACKGROUND_MESSAGES + 350,
        "{}",
        len(&s, GENERAL)
    );
    assert!(len(&s, GENERAL) <= MAX_MESSAGES_PER_CHANNEL);
}

#[test]
fn nothing_is_cut_until_the_app_says_which_channels_are_open() {
    let mut s = state();
    fill(&mut s, GENERAL, 300, 10);
    assert_eq!(len(&s, GENERAL), 300);
}
