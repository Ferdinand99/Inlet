// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Translate" and "Search the web" on a message (Fluxer's `TranslateMenuItems` and
//! `WebSearchMenuItems`): the message's text goes into a search address and the browser
//! opens it. Nothing is sent from Inlet itself; the text leaves the app only through the
//! link the person chose to open, to the search site they chose it for.
//!
//! Deviation: Fluxer lets the reader pick an engine, add their own and works on selected
//! text. Inlet uses Fluxer's default engines (Google and Google Translate) for the whole
//! message text; messages cannot be selected in part yet.

/// What the menu item searches for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    /// A translation of the text.
    Translate,
    /// A web search for the text.
    Web,
}

/// Most characters of a message that go into the address.
pub const MAX_QUERY_CHARS: usize = 1_000;

impl Search {
    fn template(self) -> &'static str {
        match self {
            Search::Translate => {
                "https://translate.google.com/?sl=auto&tl=auto&text={query}&op=translate"
            }
            Search::Web => "https://www.google.com/search?q={query}",
        }
    }
}

/// The address to open for `text`, or `None` when there is nothing to search for. Control
/// characters and line breaks become spaces, white space runs become one, and the
/// text is cut to [`MAX_QUERY_CHARS`] and percent-encoded, so it can only be the value of
/// the one parameter it is put in.
pub fn search_url(kind: Search, text: &str) -> Option<String> {
    let mut query = String::new();
    let mut space = true;
    for c in text.chars().take(MAX_QUERY_CHARS * 2) {
        let c = if c.is_control() { ' ' } else { c };
        if c.is_whitespace() {
            if !space {
                query.push(' ');
            }
            space = true;
        } else {
            query.push(c);
            space = false;
        }
        if query.chars().count() >= MAX_QUERY_CHARS {
            break;
        }
    }
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    Some(kind.template().replace("{query}", &encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_becomes_one_encoded_parameter() {
        let url = search_url(Search::Web, "cats & dogs=1 #x\nsecond  line").unwrap();
        assert_eq!(
            url,
            "https://www.google.com/search?q=cats%20%26%20dogs%3D1%20%23x%20second%20line"
        );
        // It parses as exactly one `q` value that is the cleaned text.
        let parsed = url::Url::parse(&url).unwrap();
        let pairs: Vec<_> = parsed.query_pairs().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, "cats & dogs=1 #x second line");
    }

    #[test]
    fn a_translation_keeps_the_other_parameters_of_the_address() {
        let url = search_url(Search::Translate, "bonjour & merci").unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("translate.google.com"));
        let text: Vec<_> = parsed.query_pairs().filter(|(k, _)| k == "text").collect();
        assert_eq!(text[0].1, "bonjour & merci");
        assert!(parsed
            .query_pairs()
            .any(|(k, v)| k == "op" && v == "translate"));
    }

    #[test]
    fn nothing_to_search_for_gives_no_address() {
        assert_eq!(search_url(Search::Web, ""), None);
        assert_eq!(search_url(Search::Web, " \n\t\u{0} "), None);
    }

    #[test]
    fn a_long_text_is_cut() {
        let url = search_url(Search::Web, &"a".repeat(5_000)).unwrap();
        assert!(url.len() < 1_100, "{}", url.len());
        let url = search_url(Search::Web, &"\u{e5}".repeat(5_000)).unwrap();
        let q = url::Url::parse(&url)
            .unwrap()
            .query_pairs()
            .next()
            .unwrap()
            .1
            .into_owned();
        assert_eq!(q.chars().count(), MAX_QUERY_CHARS);
    }

    #[test]
    fn only_https_addresses_on_the_engines_hosts_come_out() {
        for kind in [Search::Web, Search::Translate] {
            let url = url::Url::parse(&search_url(kind, "x").unwrap()).unwrap();
            assert_eq!(url.scheme(), "https");
            assert!(url.username().is_empty());
        }
    }
}
