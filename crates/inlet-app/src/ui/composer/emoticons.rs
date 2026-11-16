// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Turn text smileys into emoji": on send, a text smiley such as `:)` or `<3` that
//! stands alone (after a space or at the start, before a space or at the end) becomes
//! its emoji, as Fluxer does when the account has the setting on. Code (between
//! backticks), markdown links and web addresses are left as they are.

/// `content` with its text smileys turned into emoji.
pub fn convert(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut at = 0;
    while at < content.len() {
        if let Some(end) = protected_end(content, at) {
            out.push_str(&content[at..end]);
            at = end;
            continue;
        }
        let boundary = at == 0
            || content[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if boundary {
            let end = content[at..].find(' ').map_or(content.len(), |n| at + n);
            let token = &content[at..end];
            if let Some(emoji) = inlet_emoji::by_shortcut(token) {
                out.push_str(emoji.glyph);
                at = end;
                continue;
            }
        }
        let c = content[at..].chars().next().unwrap_or(' ');
        out.push(c);
        at += c.len_utf8();
    }
    out
}

/// Where the code, link or address that starts at `at` ends, if one starts there.
fn protected_end(content: &str, at: usize) -> Option<usize> {
    let rest = &content[at..];
    if rest.starts_with('`') {
        let run = rest.chars().take_while(|c| *c == '`').count();
        let ticks = &rest[..run];
        // The closing run is the next run of the same length.
        let mut from = run;
        while let Some(found) = rest[from..].find('`') {
            let start = from + found;
            let closing = rest[start..].chars().take_while(|c| *c == '`').count();
            if closing == run {
                return Some(at + start + run);
            }
            from = start + closing;
        }
        // No closing run: only the opening ticks are code.
        return Some(at + ticks.len());
    }
    if rest.starts_with('[') {
        // `[text](destination)`
        let close = rest.find("](")?;
        let end = rest[close + 2..].find(')')?;
        return Some(at + close + 2 + end + 1);
    }
    if rest.starts_with("http://") || rest.starts_with("https://") {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        return Some(at + end);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::convert;

    #[test]
    fn a_smiley_on_its_own_becomes_an_emoji() {
        let smile = inlet_emoji::by_shortcut(":)")
            .expect("the list knows :)")
            .glyph;
        assert_eq!(convert(":)"), smile);
        assert_eq!(convert("hello :) there"), format!("hello {smile} there"));
        assert_eq!(convert("a :)"), format!("a {smile}"));
    }

    #[test]
    fn a_smiley_inside_a_word_or_code_or_a_link_stays() {
        assert_eq!(convert("wow:)"), "wow:)");
        assert_eq!(
            convert("see https://example.org/a:) now"),
            "see https://example.org/a:) now"
        );
        assert_eq!(convert("`:)` and ``a :) b``"), "`:)` and ``a :) b``");
        assert_eq!(
            convert("[:)](https://example.org)"),
            "[:)](https://example.org)"
        );
        assert_eq!(convert("plain text"), "plain text");
        assert_eq!(convert(""), "");
    }

    #[test]
    fn text_with_other_characters_is_kept_whole() {
        assert_eq!(
            convert("na\u{ef}ve caf\u{e9} \u{1F600}"),
            "na\u{ef}ve caf\u{e9} \u{1F600}"
        );
    }
}
