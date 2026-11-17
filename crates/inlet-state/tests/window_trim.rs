//! What a message window keeps, by Fluxer's client's rules: a shown channel keeps a loaded
//! list that is cut to 100 when it passes 200, the cut messages waiting in a buffer of up to
//! 250 that scrolling takes before asking the server; live messages cut the list by where the
//! reader is (at the bottom: the oldest, to 100; away from it: the newest side of a list over
//! 200); a channel shown again is cut to its newest 50.
//!
//! Inlet's own rules (Fluxer has none like them): a channel that is not shown keeps its newest
//! 100 and no buffer, also when the chat shows no channel at all; the thread panel's window
//! keeps no buffer; an edit, delete or reaction of a buffered message drops the buffers.

mod common;

use common::*;
use inlet_state::limits::{
    MAX_BACKGROUND_MESSAGES, MAX_MESSAGES_PER_CHANNEL, MAX_MESSAGE_CACHE, MAX_REOPEN_MESSAGES,
    MAX_TRUNCATED_VIEW, MAX_WINDOW_BYTES,
};
use inlet_state::{AccountState, MessagesPage};
use serde_json::json;

/// Opens the window of `channel` with a first page of 50 older messages (so the channel has
/// more before it) and fills it with `n` live messages, `base` seconds in.
fn fill(s: &mut AccountState, channel: &str, n: u64, base: u64) {
    s.begin_load(ch(channel));
    let page: Vec<_> = (0..50)
        .rev()
        .map(|i| message(mid(500 + i, 7), channel, OTHER))
        .collect();
    s.apply_messages(ch(channel), MessagesPage::latest(page, 50));
    for i in 0..n {
        s.apply(&create(mid(base + i, 0), channel, OTHER, json!({})));
    }
}

/// The chat shows `channel`, with the reader away from the bottom (the list is not cut by
/// live messages until it passes 200).
fn show_scrolled(s: &mut AccountState, channel: &str) {
    s.set_open_channel(Some(ch(channel)));
    s.set_view_scrolled(true);
}

fn len(s: &AccountState, channel: &str) -> usize {
    s.messages(ch(channel)).map_or(0, |w| w.len())
}

fn buffered(s: &AccountState, channel: &str) -> (usize, usize) {
    s.messages(ch(channel)).unwrap().buffered()
}

#[test]
fn a_reader_at_the_bottom_keeps_the_newest_100_and_the_rest_waits_in_the_buffer() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 150, 1000);
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_TRUNCATED_VIEW);
    assert_eq!(w.newest_id(), Some(mid(1149, 0)));
    assert_eq!(w.buffered(), (100, 0));
    assert!(w.has_more_before() && w.has_newest());
    assert!(w.buffers_are_consistent());
}

#[test]
fn a_reader_away_from_the_bottom_loses_the_newest_side_of_a_list_over_200() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    fill(&mut s, GENERAL, 150, 1000);
    // 50 older plus 150 live: 200 is kept as it is, and the rows being read stay.
    assert_eq!(len(&s, GENERAL), MAX_MESSAGES_PER_CHANNEL);
    assert_eq!(buffered(&s, GENERAL), (0, 0));
    let oldest = s.messages(ch(GENERAL)).unwrap().oldest_id();
    // The 201st message: the newest side is cut to 100 (it goes to the newer buffer) and
    // the window no longer reaches the newest message.
    s.apply(&create(mid(1300, 0), GENERAL, OTHER, json!({})));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_TRUNCATED_VIEW);
    assert_eq!(w.oldest_id(), oldest, "the oldest rows stay");
    assert_eq!(
        w.buffered(),
        (0, MAX_MESSAGES_PER_CHANNEL + 1 - MAX_TRUNCATED_VIEW)
    );
    assert!(!w.has_newest());
    assert!(w.buffers_are_consistent());
}

#[test]
fn the_buffer_holds_at_most_250_and_loses_the_furthest_first() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 700, 1000);
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.len() <= MAX_MESSAGES_PER_CHANNEL);
    assert_eq!(w.buffered().0, MAX_MESSAGE_CACHE);
    let oldest = w.oldest_id().unwrap();
    assert!(s.drain_older(ch(GENERAL), 1).is_some());
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.oldest_id().unwrap() < oldest);
    assert!(w.approx_bytes() <= MAX_WINDOW_BYTES);
}

#[test]
fn scrolling_back_takes_the_buffer_before_asking_for_a_page() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 260, 1000);
    assert!(buffered(&s, GENERAL).0 > 50);
    let mut taken = 0;
    while let Some(c) = s.drain_older(ch(GENERAL), 50) {
        assert!(c.messages.contains(&ch(GENERAL)));
        taken += 1;
        assert!(taken < 10);
        assert!(len(&s, GENERAL) <= MAX_MESSAGES_PER_CHANNEL);
    }
    assert_eq!(buffered(&s, GENERAL).0, 0, "the buffer is empty now");
    // The buffer did not reach the start of the channel (more older messages exist), so
    // the pager asks the server, from the list's oldest message.
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.has_more_before());
    let oldest = w.oldest_id().unwrap();
    let page: Vec<_> = (0..50)
        .map(|i| message(mid(100 + i, 1), GENERAL, OTHER))
        .collect();
    s.apply_messages(ch(GENERAL), MessagesPage::before(oldest, page, 50));
    assert!(s.messages(ch(GENERAL)).unwrap().oldest_id().unwrap() < oldest);
}

#[test]
fn a_buffer_that_reaches_the_start_of_the_channel_ends_the_paging() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    // A channel that began with these messages: the first page reached its start.
    s.begin_load(ch(GENERAL));
    s.apply_messages(ch(GENERAL), MessagesPage::latest(vec![], 50));
    for i in 0..150 {
        s.apply(&create(mid(100 + i, 0), GENERAL, OTHER, json!({})));
    }
    assert!(buffered(&s, GENERAL).0 > 0);
    while s.drain_older(ch(GENERAL), 50).is_some() {}
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.buffered().0, 0);
    assert!(
        !w.has_more_before(),
        "nothing older exists: no empty request is needed"
    );
}

#[test]
fn scrolling_down_takes_the_newer_buffer_and_reaches_the_newest_message_again() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    fill(&mut s, GENERAL, 140, 1000);
    // An older page pushes the list past 200: the newest messages are cut to the newer buffer.
    let oldest = s.messages(ch(GENERAL)).unwrap().oldest_id().unwrap();
    let page: Vec<_> = (0..80)
        .map(|i| message(mid(100 + i, 1), GENERAL, OTHER))
        .collect();
    s.apply_messages(ch(GENERAL), MessagesPage::before(oldest, page, 80));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_TRUNCATED_VIEW);
    assert!(
        !w.has_newest(),
        "the list no longer reaches the newest message"
    );
    let newer = w.buffered().1;
    assert!(newer > 0);
    let newest = w.newest_id().unwrap();
    assert!(s.drain_newer(ch(GENERAL), 50).is_some());
    assert!(s.messages(ch(GENERAL)).unwrap().newest_id().unwrap() > newest);
    assert!(buffered(&s, GENERAL).1 < newer);
    // The buffer reached the newest message: draining it all restores that.
    while s.drain_newer(ch(GENERAL), 50).is_some() {}
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(
        w.has_newest(),
        "the newer buffer reached the newest message"
    );
    assert_eq!(w.newest_id(), Some(mid(1139, 0)));
}

#[test]
fn leaving_a_channel_cuts_its_window_to_the_newest_100_and_showing_it_again_to_50() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    fill(&mut s, GENERAL, 140, 1000);
    let newest = s.messages(ch(GENERAL)).unwrap().newest_id();
    assert_eq!(len(&s, GENERAL), 190);

    let c = s.set_open_channel(Some(ch(RANDOM)));
    assert!(c.messages.contains(&ch(GENERAL)), "the cut is reported");
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_BACKGROUND_MESSAGES);
    assert_eq!(
        w.buffered(),
        (0, 0),
        "a window not on screen keeps no buffer"
    );
    assert_eq!(
        w.newest_id(),
        newest,
        "the newest messages are the ones kept"
    );
    assert!(w.has_more_before() && w.has_newest());

    // Showing it again cuts it to the newest 50; the other 50 wait in the buffer.
    s.set_open_channel(Some(ch(GENERAL)));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_REOPEN_MESSAGES);
    assert_eq!(
        w.buffered(),
        (MAX_BACKGROUND_MESSAGES - MAX_REOPEN_MESSAGES, 0)
    );
    assert_eq!(w.newest_id(), newest);
}

#[test]
fn the_buffer_of_the_reopen_cut_survives_the_newest_page() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    fill(&mut s, GENERAL, 140, 1000);
    s.set_open_channel(Some(ch(RANDOM)));
    // The channel is shown again: the app loads the window and asks for the newest page.
    s.begin_load(ch(GENERAL));
    s.set_open_channel(Some(ch(GENERAL)));
    let newest: Vec<_> = (0..50)
        .rev()
        .map(|i| message(mid(1090 + i, 0), GENERAL, OTHER))
        .collect();
    s.apply_messages(ch(GENERAL), MessagesPage::latest(newest, 50));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(
        w.buffered().0,
        MAX_BACKGROUND_MESSAGES - MAX_REOPEN_MESSAGES
    );
    assert!(w.buffers_are_consistent());
    // Scrolling back takes it without a request.
    assert!(s.drain_older(ch(GENERAL), 50).is_some());
}

#[test]
fn a_newest_page_that_does_not_overlap_the_list_drops_the_buffers() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 200, 1000);
    assert!(buffered(&s, GENERAL).0 > 0);
    // The window waits for a newest page with messages far newer than the list.
    s.begin_load(ch(GENERAL));
    let page: Vec<_> = (0..50)
        .rev()
        .map(|i| message(mid(9000 + i, 0), GENERAL, OTHER))
        .collect();
    s.apply_messages(ch(GENERAL), MessagesPage::latest(page, 50));
    assert_eq!(buffered(&s, GENERAL), (0, 0));
}

#[test]
fn leaving_to_no_channel_cuts_at_once() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    fill(&mut s, GENERAL, 100, 1000);
    assert_eq!(len(&s, GENERAL), 150);
    // The Friends page, or another guild: the chat shows no channel.
    let c = s.set_open_channel(None);
    assert!(c.messages.contains(&ch(GENERAL)));
    assert_eq!(len(&s, GENERAL), MAX_BACKGROUND_MESSAGES);
}

#[test]
fn the_thread_panels_window_is_not_cut_as_a_channel_not_shown_and_keeps_no_buffer() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    s.set_open_thread(Some(ch(RANDOM)));
    fill(&mut s, GENERAL, 140, 1000);
    fill(&mut s, RANDOM, 60, 1000);
    fill(&mut s, DM, 190, 1000);
    // The panel's list is at the bottom: kept at 100 by live messages, nothing buffered.
    assert_eq!(len(&s, RANDOM), MAX_TRUNCATED_VIEW);
    assert_eq!(buffered(&s, RANDOM), (0, 0));
    // Older pages grow it past the limit for a channel that is not shown.
    let oldest = s.messages(ch(RANDOM)).unwrap().oldest_id().unwrap();
    let page: Vec<_> = (0..50)
        .map(|i| message(mid(100 + i, 2), RANDOM, OTHER))
        .collect();
    s.apply_messages(ch(RANDOM), MessagesPage::before(oldest, page, 50));
    assert_eq!(len(&s, RANDOM), 150);
    // A window made for a channel that is not shown is cut as it fills.
    assert_eq!(len(&s, DM), MAX_BACKGROUND_MESSAGES);

    // Showing another channel cuts the chat's old one, not the panel's.
    s.set_open_channel(Some(ch(DM)));
    assert_eq!(len(&s, GENERAL), MAX_BACKGROUND_MESSAGES);
    assert_eq!(len(&s, RANDOM), 150);
    // Closing the panel cuts the thread.
    let c = s.set_open_thread(None);
    assert!(c.messages.contains(&ch(RANDOM)));
    assert_eq!(len(&s, RANDOM), MAX_BACKGROUND_MESSAGES);
}

#[test]
fn live_messages_into_a_window_that_is_not_open_keep_it_at_the_limit() {
    let mut s = state();
    show_scrolled(&mut s, GENERAL);
    fill(&mut s, GENERAL, 100, 1000);
    s.set_open_channel(Some(ch(RANDOM)));
    for i in 0..80 {
        s.apply(&create(mid(3000 + i, 0), GENERAL, OTHER, json!({})));
    }
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_BACKGROUND_MESSAGES);
    assert_eq!(w.newest_id(), Some(mid(3079, 0)));
    assert_eq!(w.buffered(), (0, 0));
}

#[test]
fn an_older_page_after_the_reader_left_does_not_cut_the_newest_side_of_a_window_not_shown() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 60, 1000);
    let oldest = s.messages(ch(GENERAL)).unwrap().oldest_id().unwrap();
    s.set_open_channel(Some(ch(RANDOM)));
    let newest = s.messages(ch(GENERAL)).unwrap().newest_id();
    let page: Vec<_> = (0..80)
        .map(|i| message(mid(100 + i, 2), GENERAL, OTHER))
        .collect();
    s.apply_messages(ch(GENERAL), MessagesPage::before(oldest, page, 80));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.len() <= MAX_BACKGROUND_MESSAGES);
    assert_eq!(w.newest_id(), newest);
    assert!(w.has_newest());
}

#[test]
fn an_edit_a_delete_or_a_reaction_of_a_buffered_message_drops_the_buffers() {
    for event in ["MESSAGE_DELETE", "MESSAGE_UPDATE", "MESSAGE_REACTION_ADD"] {
        let mut s = state();
        s.set_open_channel(Some(ch(GENERAL)));
        fill(&mut s, GENERAL, 250, 1000);
        assert!(buffered(&s, GENERAL).0 > 0);
        // Message 15 was cut to the buffer.
        let id = mid(1015, 0).to_string();
        let data = match event {
            "MESSAGE_DELETE" => json!({"id": id, "channel_id": GENERAL}),
            "MESSAGE_UPDATE" => message_json(
                mid(1015, 0),
                GENERAL,
                OTHER,
                json!({"content": "edited", "edited_timestamp": "2026-01-01T02:00:00Z"}),
            ),
            _ => json!({"user_id": OTHER, "channel_id": GENERAL, "message_id": id,
                        "emoji": {"name": "x"}}),
        };
        s.apply(&d(event, data));
        assert_eq!(buffered(&s, GENERAL), (0, 0), "{event}");
    }
}

#[test]
fn a_new_ready_marks_the_windows_stale_and_drops_the_buffers() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 250, 1000);
    assert!(buffered(&s, GENERAL).0 > 0);
    let ready = ready_from(ready_json(
        guild_ready(default_roles(), default_channels(), &[MOD_ROLE]),
        json!([]),
        json!([]),
    ));
    s.apply(&inlet_protocol::Dispatch::Ready(Box::new(ready)));
    assert_eq!(buffered(&s, GENERAL), (0, 0));
}

#[test]
fn a_window_is_cut_even_before_the_app_says_which_channels_are_open() {
    let mut s = state();
    fill(&mut s, GENERAL, 250, 1000);
    assert!(len(&s, GENERAL) <= MAX_MESSAGES_PER_CHANNEL);
}
