// SPDX-License-Identifier: AGPL-3.0-or-later
//! A message as one line of plain text, for places that cannot draw markdown (the
//! preview above a reply). The same parser as the chat, so what counts as bold, code or
//! a mention is the same; the markers disappear and only the words stay. Mentions show
//! their names, custom emoji their `:name:`, and spoilers are covered letter by letter,
//! so their text cannot be read before it is revealed. Parsed with the flags of Fluxer's
//! reply preview ([`super::parse_reply_with`]): headings, lists, quotes, alerts and tables
//! are not parsed there, so their markers stay as typed.

use super::{Block, Emoji, Inline, LiveKind, Style};
use crate::model::Names;

/// What a spoiler's letters show as.
const SPOILER_COVER: char = '\u{2588}';

/// The text of `input` without its markdown markers, blocks joined by a space.
/// Whitespace is not collapsed and nothing is cut: see [`crate::model::snippet`].
pub fn plain_text(input: &str, names: &Names) -> String {
    let blocks = super::parse_reply_with(input, names);
    let mut parts = Vec::new();
    blocks_text(&blocks, names, false, &mut parts);
    parts.join(" ")
}

fn cover(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { c } else { SPOILER_COVER })
        .collect()
}

fn blocks_text(blocks: &[Block], names: &Names, hidden: bool, out: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Text(i) | Block::Heading(_, i) | Block::Subtext(i) => {
                out.push(inline_text(i, names, hidden));
            }
            Block::Code(code) => out.push(if hidden {
                cover(&code.content)
            } else {
                code.content.to_string()
            }),
            Block::Quote(inner) | Block::Alert(_, inner) => blocks_text(inner, names, hidden, out),
            Block::Spoiler(inner) => blocks_text(inner, names, true, out),
            Block::List(list) => {
                for item in &list.items {
                    blocks_text(item, names, hidden, out);
                }
            }
            Block::Table(table) => {
                for cell in table.header.iter().chain(table.rows.iter().flatten()) {
                    out.push(inline_text(cell, names, hidden));
                }
            }
        }
    }
}

fn inline_text(inline: &Inline, names: &Names, hidden: bool) -> String {
    // The styles per byte, worked out once, so the loop below is linear in the text.
    let mut spoiler = vec![false; inline.text.len()];
    let mut link_target = vec![false; inline.text.len()];
    for (range, style) in &inline.highlights {
        let marks = match style {
            Style::Spoiler => &mut spoiler,
            Style::LinkTarget => &mut link_target,
            _ => continue,
        };
        let end = range.end.min(marks.len());
        if let Some(slice) = marks.get_mut(range.start.min(end)..end) {
            slice.fill(true);
        }
    }
    // `live` and `emoji` are sorted and apart, and `pos` only grows: one cursor each.
    let (mut next_live, mut next_emoji) = (0, 0);
    let mut out = String::with_capacity(inline.text.len());
    let mut pos = 0;
    while pos < inline.text.len() {
        let covered = hidden || spoiler[pos];
        while inline
            .live
            .get(next_live)
            .is_some_and(|l| l.range.start < pos)
        {
            next_live += 1;
        }
        while inline
            .emoji
            .get(next_emoji)
            .is_some_and(|(r, _)| r.start < pos)
        {
            next_emoji += 1;
        }
        // Roles and channels are named when drawn in the chat; here from `names`.
        if let Some((range, label)) = inline
            .live
            .get(next_live)
            .filter(|l| l.range.start == pos)
            .and_then(|l| named(&l.kind, names).map(|name| (l.range.clone(), name)))
        {
            if covered {
                out.extend(label.chars().map(|_| SPOILER_COVER));
            } else {
                out.push_str(&label);
            }
            pos = range.end;
            continue;
        }
        if let Some((range, emoji)) = inline.emoji.get(next_emoji).filter(|(r, _)| r.start == pos) {
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
        let at = pos;
        pos += c.len_utf8();
        // The host a masked link points to is an addition of the chat, not the text.
        if link_target[at] {
            continue;
        }
        out.push(match c {
            // Line breaks, tabs and the no-break spaces of mention labels are one space
            // each; Fluxer's preview collapses whitespace too.
            c if c.is_whitespace() => ' ',
            _ if covered => SPOILER_COVER,
            c => c,
        });
    }
    out
}

/// The label of a role or channel mention whose name `names` knows.
fn named(kind: &LiveKind, names: &Names) -> Option<String> {
    match kind {
        LiveKind::Role { id } => names.roles.get(id).map(|n| format!("@{n}")),
        LiveKind::Channel { id } => names.channels.get(id).map(|n| format!("#{n}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::snippet;

    fn names() -> Names {
        let mut n = Names::default();
        n.users.insert("42".into(), "Ada Lovelace".into());
        n.roles.insert("7".into(), "Mods".into());
        n.channels.insert("9".into(), "rules".into());
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
        let plain = plain_text("# Title\n> quoted\n```rs\nlet x = 1;\n```", &names());
        // As in Fluxer's reply preview: no headings or quotes, so their markers show;
        // code blocks are still parsed.
        assert_eq!(snippet(&plain, 90), "# Title > quoted let x = 1;");
    }

    #[test]
    fn roles_and_channels_show_their_names() {
        assert_eq!(
            plain_text("ping <@&7> in <#9>", &names()),
            "ping @Mods in #rules"
        );
        // Not known: the labels the chat shows for an unknown role and channel.
        let unknown = plain_text("<@&8> <#10>", &Names::default());
        assert!(!unknown.contains("Mods") && unknown.contains('@') && !unknown.contains("rules"));
        // Inside a spoiler the names are covered as well.
        let hidden = plain_text("||<@&7>||", &names());
        assert_eq!(hidden, SPOILER_COVER.to_string().repeat(5));
    }

    #[test]
    fn block_markers_stay_as_typed_in_a_reply_preview() {
        for text in ["# Title", "> quoted", "- one", "1. one", "> [!NOTE] x"] {
            assert_eq!(plain_text(text, &names()), text, "{text}");
        }
        // Subtext and inline styles are still parsed.
        assert_eq!(plain_text("-# small **bold**", &names()), "small bold");
    }

    #[test]
    fn unicode_emoji_show_as_themselves() {
        // A family (ZWJ sequence), a flag, a skin tone and a keycap stay whole.
        let text = "ok \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{1F1F3}\u{1F1F4} \u{1F44D}\u{1F3FD} 1\u{FE0F}\u{20E3} end";
        assert_eq!(plain_text(text, &names()), text);
        // In a spoiler the emoji is covered, never shown.
        let hidden = plain_text("||\u{1F44D}\u{1F3FD}||", &names());
        assert!(!hidden.contains('\u{1F44D}') && hidden.contains(SPOILER_COVER));
    }

    #[test]
    fn links_and_everyone_show_their_text() {
        assert_eq!(
            plain_text("see https://example.com/a now", &names()),
            "see https://example.com/a now"
        );
        let plain = plain_text("@everyone and @here: hi", &names());
        assert_eq!(plain, "@everyone and @here: hi");
    }

    #[test]
    fn several_and_styled_spoilers_stay_covered() {
        let plain = plain_text("||one|| and ||two||", &names());
        let cover = |n: usize| SPOILER_COVER.to_string().repeat(n);
        assert_eq!(plain, format!("{} and {}", cover(3), cover(3)));
        // Markers inside a spoiler go and nothing of its text shows.
        let styled = plain_text("||**bold** and `code`||", &names());
        assert!(
            !styled.contains("bold") && !styled.contains("code"),
            "{styled}"
        );
        assert!(
            styled.contains(SPOILER_COVER) && !styled.contains('*'),
            "{styled}"
        );
    }

    #[test]
    fn a_spoiler_around_lines_covers_all_of_them() {
        let plain = plain_text("before\n||\nline one\nline two\n||\nafter", &names());
        assert!(!plain.contains("line") && !plain.contains("||"), "{plain}");
        assert!(plain.contains(SPOILER_COVER), "{plain}");
        assert!(
            plain.starts_with("before") && plain.ends_with("after"),
            "{plain}"
        );
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
