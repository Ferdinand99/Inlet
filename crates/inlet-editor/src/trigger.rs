// SPDX-License-Identifier: AGPL-3.0-or-later
//! Autocomplete triggers: `@name`, `#channel`, `:emoji` and `/command` at the caret.

use std::ops::Range;

use crate::buffer::Buffer;

/// What the user started typing. The string is the text after the trigger character,
/// up to the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    Mention(String),
    Channel(String),
    Emoji(String),
    Command(String),
    /// `/gif`, `/sticker` or `/saved` with the search text after it: the list offers
    /// results to send, not text to insert. The range covers the whole message up to the
    /// caret.
    Media(MediaCommand, String),
}

/// The commands that search for something to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    Gif,
    Sticker,
    Saved,
}

impl MediaCommand {
    /// The command as typed, with its slash.
    pub fn name(self) -> &'static str {
        match self {
            MediaCommand::Gif => "/gif",
            MediaCommand::Sticker => "/sticker",
            MediaCommand::Saved => "/saved",
        }
    }
}

/// A media command at the start of `before_caret` (white space may come first), whole
/// and followed by white space or nothing, with the search text after it.
fn media_command(before_caret: &str) -> Option<(MediaCommand, String)> {
    let t = before_caret.trim_start();
    let rest = t.strip_prefix('/')?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let kind = match name.as_str() {
        "gif" => MediaCommand::Gif,
        "sticker" => MediaCommand::Sticker,
        "saved" => MediaCommand::Saved,
        _ => return None,
    };
    let after = &rest[name.len()..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    Some((kind, after.trim().to_owned()))
}

/// The fewest characters after `:` before the emoji list opens. It keeps `:)` and `:D` quiet.
const MIN_EMOJI_QUERY: usize = 2;

/// Finds a trigger that ends at `cursor`. The trigger character must start a word: it is
/// the first character of the text, or comes after white space. `/` only counts at the very
/// start of the message. The range covers the trigger character and the query, so the caller
/// can replace it with the chosen mention.
pub fn trigger_at(buf: &Buffer, cursor: usize) -> Option<(Range<usize>, Trigger)> {
    let text = buf.text();
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    if buf.token_inside(cursor).is_some() {
        return None;
    }
    // A media command takes the whole message before the caret, so it comes before the
    // word walk (the search text has spaces). A pill anywhere in it is not a command.
    if buf.tokens().is_empty() {
        if let Some((kind, query)) = media_command(&text[..cursor]) {
            return Some((0..cursor, Trigger::Media(kind, query)));
        }
    }
    // Walk back to the start of the word.
    let mut start = cursor;
    for (i, ch) in text[..cursor].char_indices().rev() {
        if ch.is_whitespace() {
            break;
        }
        start = i;
    }
    // A pill ends a word too, but text glued to a pill is not a new word.
    if buf
        .tokens()
        .iter()
        .any(|t| t.range.start < cursor && t.range.end > start)
    {
        return None;
    }
    let word = &text[start..cursor];
    let mut chars = word.chars();
    let sigil = chars.next()?;
    let query = chars.as_str();
    let trigger = match sigil {
        '@' => Trigger::Mention(query.to_owned()),
        '#' => Trigger::Channel(query.to_owned()),
        ':' => {
            let valid = query
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '+' | '-'));
            if !valid || query.chars().count() < MIN_EMOJI_QUERY {
                return None;
            }
            Trigger::Emoji(query.to_owned())
        }
        '/' if start == 0 => Trigger::Command(query.to_owned()),
        _ => return None,
    };
    Some((start..cursor, trigger))
}
