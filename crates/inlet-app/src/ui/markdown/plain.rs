// SPDX-License-Identifier: AGPL-3.0-or-later
//! A message as one line of plain text, for places that cannot draw markdown (the
//! preview above a reply). The same parser as the chat, so what counts as bold, code or
//! a mention is the same; the markers disappear and only the words stay. Mentions show
//! their names, custom emoji their `:name:`, and spoilers are covered letter by letter,
//! so their text cannot be read before it is revealed.

use super::{Block, Emoji, Inline, Style};
use crate::model::Names;

/// What a spoiler's letters show as.
const SPOILER_COVER: char = '\u{2588}';

/// The text of `input` without its markdown markers, blocks joined by a space.
/// Whitespace is not collapsed and nothing is cut: see [`crate::model::snippet`].
pub fn plain_text(input: &str, names: &Names) -> String {
    let blocks = super::parse_with(input, names);
    let mut parts = Vec::new();
    blocks_text(&blocks, false, &mut parts);
    parts.join(" ")
}

fn cover(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { c } else { SPOILER_COVER })
        .collect()
}

fn blocks_text(blocks: &[Block], hidden: bool, out: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Text(i) | Block::Heading(_, i) | Block::Subtext(i) => {
                out.push(inline_text(i, hidden));
            }
            Block::Code(code) => out.push(if hidden {
                cover(&code.content)
            } else {
                code.content.to_string()
            }),
            Block::Quote(inner) | Block::Alert(_, inner) => blocks_text(inner, hidden, out),
            Block::Spoiler(inner) => blocks_text(inner, true, out),
            Block::List(list) => {
                for item in &list.items {
                    blocks_text(item, hidden, out);
                }
            }
            Block::Table(table) => {
                for cell in table.header.iter().chain(table.rows.iter().flatten()) {
                    out.push(inline_text(cell, hidden));
                }
            }
        }
    }
}

fn inline_text(inline: &Inline, hidden: bool) -> String {
    let in_style = |pos: usize, style: Style| {
        inline
            .highlights
            .iter()
            .any(|(r, s)| *s == style && r.contains(&pos))
    };
    let mut out = String::with_capacity(inline.text.len());
    let mut pos = 0;
    while pos < inline.text.len() {
        let covered = hidden || in_style(pos, Style::Spoiler);
        if let Some((range, emoji)) = inline.emoji.iter().find(|(r, _)| r.start == pos) {
            if covered {
                out.push(SPOILER_COVER);
            } else {
                match emoji {
                    Emoji::Standard(raw) => out.push_str(raw),
                    Emoji::Custom { name, .. } => {
                        out.push(':');
                        out.push_str(name);
                        out.push(':');
                    }
                }
            }
            pos = range.end;
            continue;
        }
        let c = inline.text[pos..].chars().next().unwrap_or(' ');
        pos += c.len_utf8();
        // The host a masked link points to is an addition of the chat, not the text.
        if in_style(pos - c.len_utf8(), Style::LinkTarget) {
            continue;
        }
        out.push(match c {
            // Mention labels keep their spaces from wrapping with no-break spaces.
            '\u{a0}' => ' ',
            c if covered && !c.is_whitespace() => SPOILER_COVER,
            c => c,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::snippet;

    fn names() -> Names {
        let mut n = Names::default();
        n.users.insert("42".into(), "Ada Lovelace".into());
        n
    }

    #[test]
    fn markers_disappear_and_the_words_stay() {
        let plain = plain_text(
            "**bold** and *italic* and `code` and ~~gone~~ and __under__",
            &names(),
        );
        assert_eq!(plain, "bold and italic and code and gone and under");
    }

    #[test]
    fn a_spoiler_is_covered_letter_by_letter() {
        let plain = plain_text("the end: ||Bruce is **dead**|| ok", &names());
        assert_eq!(
            plain,
            "the end: \u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}\u{2588}\u{2588} ok"
        );
        assert!(!plain.contains("dead") && !plain.contains("**"));
    }

    #[test]
    fn mentions_and_custom_emoji_show_their_names() {
        let plain = plain_text("hi <@42> look <:party:123456789> now", &names());
        assert_eq!(plain, "hi @Ada Lovelace look :party: now");
    }

    #[test]
    fn a_whole_message_with_every_kind_of_marker() {
        let content = "**bold** *italic* `code` ||secret|| <@42> <a:spin:99>";
        let plain = snippet(&plain_text(content, &names()), 90);
        assert_eq!(
            plain,
            "bold italic code \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} @Ada Lovelace :spin:"
        );
    }

    #[test]
    fn blocks_become_one_line() {
        let plain = plain_text(
            "# Title\n> quoted\n```rs\nlet x = 1;\n```\n- one\n- two",
            &names(),
        );
        assert_eq!(snippet(&plain, 90), "Title quoted let x = 1; one two");
    }

    #[test]
    fn an_unclosed_spoiler_marker_stays_as_text() {
        assert_eq!(plain_text("ok ||x", &names()), "ok ||x");
    }

    #[test]
    fn a_masked_link_shows_its_text_only() {
        let plain = plain_text("[docs](https://example.com/a)", &names());
        assert_eq!(plain, "docs");
    }
}
