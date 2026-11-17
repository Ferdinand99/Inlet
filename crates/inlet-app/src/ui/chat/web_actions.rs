// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Translate" on a message (Fluxer's `TranslateMenuItems`): the message's text goes into the
//! translator's address and the browser opens it. Nothing is sent from Inlet itself; the text leaves the app only through the
//! link the person chose to open, to the search site they chose it for.
//!
//! "Search the web" is not offered: Fluxer shows it only for selected text, and messages
//! cannot be selected in part yet. Translate falls back to the whole message, as in Fluxer.
//!
//! Deviation: Fluxer lets the reader pick a translator and add their own; Inlet uses Fluxer's
//! default, Google Translate.

/// What the menu item searches for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    /// A translation of the text.
    Translate,
}

/// Most characters of a message that go into the address.
pub const MAX_QUERY_CHARS: usize = 1_000;

impl Search {
    fn template(self) -> &'static str {
        match self {
            Search::Translate => {
                "https://translate.google.com/?sl=auto&tl=auto&text={query}&op=translate"
            }
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

    fn text_of(url: &str) -> String {
        url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "text")
            .unwrap()
            .1
            .into_owned()
    }

    #[test]
    fn the_text_becomes_one_encoded_parameter() {
        let url = search_url(Search::Translate, "cats & dogs=1 #x\nsecond  line").unwrap();
        assert!(url.contains("text=cats%20%26%20dogs%3D1%20%23x%20second%20line&op=translate"));
        // The text cannot add a parameter or end the address: it parses as the one `text`
        // value, cleaned.
        assert_eq!(text_of(&url), "cats & dogs=1 #x second line");
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.query_pairs().filter(|(k, _)| k == "text").count(), 1);
        assert_eq!(parsed.fragment(), None);
    }

    #[test]
    fn a_translation_keeps_the_other_parameters_of_the_address() {
        let url = search_url(Search::Translate, "bonjour & merci").unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("translate.google.com"));
        assert_eq!(text_of(&url), "bonjour & merci");
        assert!(parsed
            .query_pairs()
            .any(|(k, v)| k == "op" && v == "translate"));
    }

    #[test]
    fn nothing_to_translate_gives_no_address() {
        assert_eq!(search_url(Search::Translate, ""), None);
        assert_eq!(search_url(Search::Translate, " \n\t\u{0} "), None);
    }

    #[test]
    fn a_long_text_is_cut() {
        let url = search_url(Search::Translate, &"a".repeat(5_000)).unwrap();
        assert!(url.len() < 1_200, "{}", url.len());
        let url = search_url(Search::Translate, &"\u{e5}".repeat(5_000)).unwrap();
        assert_eq!(text_of(&url).chars().count(), MAX_QUERY_CHARS);
    }

    #[test]
    fn only_an_https_address_without_user_info_comes_out() {
        let url = url::Url::parse(&search_url(Search::Translate, "x").unwrap()).unwrap();
        assert_eq!(url.scheme(), "https");
        assert!(url.username().is_empty());
    }
}
