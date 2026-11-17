//! What a message window keeps, by Fluxer's client's rules: a shown channel keeps a loaded
//! list of up to 200 that is cut to 100 when it passes that, the cut messages waiting in a
//! buffer of up to 250 that scrolling takes before asking the server; a channel shown again is
//! cut to its newest 50; a channel that is not shown keeps its newest 100 and no buffer. The
//! open channel and the thread panel's are never cut by the rule for channels not shown.

mod common;

use common::*;
use inlet_state::limits::{
    MAX_BACKGROUND_MESSAGES, MAX_MESSAGES_PER_CHANNEL, MAX_MESSAGE_CACHE, MAX_REOPEN_MESSAGES,
    MAX_TRUNCATED_VIEW, MAX_WINDOW_BYTES,
};
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

fn buffered(s: &AccountState, channel: &str) -> (usize, usize) {
    s.messages(ch(channel)).unwrap().buffered()
}

#[test]
fn the_open_channels_list_is_cut_from_over_200_to_100() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, MAX_MESSAGES_PER_CHANNEL as u64, 10);
    assert_eq!(
        len(&s, GENERAL),
        MAX_MESSAGES_PER_CHANNEL,
        "200 is kept as it is"
    );
    assert_eq!(buffered(&s, GENERAL), (0, 0));
    // The 201st message cuts the list to 100; the 101 oldest wait in the buffer.
    s.apply(&create(mid(10 + 200, 0), GENERAL, OTHER, json!({})));
    let w = s.messages(ch(GENERAL)).unwrap();
    assert_eq!(w.len(), MAX_TRUNCATED_VIEW);
    assert_eq!(
        w.buffered(),
        (MAX_MESSAGES_PER_CHANNEL + 1 - MAX_TRUNCATED_VIEW, 0)
    );
    assert_eq!(w.newest_id(), Some(mid(210, 0)), "the newest messages stay");
    assert!(w.has_more_before() && w.has_newest());
}

#[test]
fn the_buffer_holds_at_most_250_and_loses_the_furthest_first() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 700, 10);
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.len() <= MAX_MESSAGES_PER_CHANNEL);
    assert_eq!(w.buffered().0, MAX_MESSAGE_CACHE);
    // The buffer holds the messages nearest the list: taking one gives the one just before it.
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
    // Each call moves a page of the buffer into the list: no request for an older page.
    let mut taken = 0;
    while let Some(c) = s.drain_older(ch(GENERAL), 50) {
        assert!(c.messages.contains(&ch(GENERAL)));
        taken += 1;
        assert!(taken < 10);
        assert!(len(&s, GENERAL) <= MAX_MESSAGES_PER_CHANNEL);
    }
    assert_eq!(buffered(&s, GENERAL).0, 0, "the buffer is empty now");
    // Then the pager asks the server, from the list's oldest message, as for any window.
    let w = s.messages(ch(GENERAL)).unwrap();
    assert!(w.has_more_before());
    let oldest = w.oldest_id().unwrap();
    let page: Vec<_> = (0..50)
        .map(|i| message(mid(500 + i, 1), GENERAL, OTHER))
        .collect();
    s.apply_messages(ch(GENERAL), MessagesPage::before(oldest, page, 50));
    assert!(s.messages(ch(GENERAL)).unwrap().oldest_id().unwrap() < oldest);
}

#[test]
fn scrolling_down_takes_the_newer_buffer() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 150, 1000);
    // An older page pushes the list past 200: the newest messages are cut to the newer buffer.
    let oldest = s.messages(ch(GENERAL)).unwrap().oldest_id().unwrap();
    let page: Vec<_> = (0..80)
        .map(|i| message(mid(500 + i, 1), GENERAL, OTHER))
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
}

#[test]
fn leaving_a_channel_cuts_its_window_to_the_newest_100_and_showing_it_again_to_50() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 300, 10);
    let newest = s.messages(ch(GENERAL)).unwrap().newest_id();
    assert!(buffered(&s, GENERAL).0 > 0);

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
    assert!(w.has_more_before(), "older ones can be loaded again");
    assert!(w.has_newest(), "live messages still land in it");
    assert_eq!(w.oldest_id(), Some(mid(10 + 200, 0)));

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
fn leaving_to_no_channel_cuts_at_once() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 150, 10);
    assert_eq!(len(&s, GENERAL), 150);
    // The Friends page, or another guild: the chat shows no channel.
    let c = s.set_open_channel(None);
    assert!(c.messages.contains(&ch(GENERAL)));
    assert_eq!(len(&s, GENERAL), MAX_BACKGROUND_MESSAGES);
}

#[test]
fn the_open_channel_and_the_thread_panels_channel_are_not_cut_as_channels_not_shown() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    s.set_open_thread(Some(ch(RANDOM)));
    fill(&mut s, GENERAL, 190, 10);
    fill(&mut s, RANDOM, 190, 10);
    fill(&mut s, DM, 190, 10);
    assert_eq!(len(&s, GENERAL), 190);
    assert_eq!(len(&s, RANDOM), 190);
    // A window made for a channel that is not shown is cut as it fills.
    assert_eq!(len(&s, DM), MAX_BACKGROUND_MESSAGES);

    // Showing another channel cuts the chat's old one, not the panel's.
    s.set_open_channel(Some(ch(DM)));
    assert_eq!(len(&s, GENERAL), MAX_BACKGROUND_MESSAGES);
    assert_eq!(len(&s, RANDOM), 190);
    // Closing the panel cuts the thread.
    let c = s.set_open_thread(None);
    assert!(c.messages.contains(&ch(RANDOM)));
    assert_eq!(len(&s, RANDOM), MAX_BACKGROUND_MESSAGES);
    // The channel on screen grows past the limit.
    fill(&mut s, DM, 100, 2000);
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
    assert_eq!(w.buffered(), (0, 0));
}

#[test]
fn a_delete_of_a_buffered_message_drops_the_buffer() {
    let mut s = state();
    s.set_open_channel(Some(ch(GENERAL)));
    fill(&mut s, GENERAL, 250, 10);
    assert!(buffered(&s, GENERAL).0 > 0);
    // The oldest messages were cut to the buffer; deleting one empties the cache.
    s.apply(&d(
        "MESSAGE_DELETE",
        json!({"id": mid(15, 0).to_string(), "channel_id": GENERAL}),
    ));
    assert_eq!(buffered(&s, GENERAL), (0, 0));
}

#[test]
fn a_window_is_cut_at_200_even_before_the_app_says_which_channels_are_open() {
    let mut s = state();
    fill(&mut s, GENERAL, 150, 10);
    assert_eq!(len(&s, GENERAL), 150);
    fill(&mut s, RANDOM, 210, 10);
    assert!(len(&s, RANDOM) <= MAX_MESSAGES_PER_CHANNEL);
}
