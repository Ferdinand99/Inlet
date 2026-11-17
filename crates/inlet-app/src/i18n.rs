// SPDX-License-Identifier: AGPL-3.0-or-later
//! The text catalogue. Every text the app shows comes from Project Fluent files under
//! `locales/<language>/`, one file per domain; English is the source and Norwegian
//! Bokmål (`nb`) is the second language. Look a text up with [`t!`]:
//!
//! ```ignore
//! t!("offline-retry-now")
//! t!("offline-title", instance = instance)
//! ```
//!
//! `build.rs` joins the `.ftl` files of a language into one, so a task that adds a
//! domain adds its own file and never edits another. A missing ID shows the ID itself
//! and is logged; the tests make sure that never ships. A text that exists in English
//! but not yet in the chosen language shows the English text, so a language can lag
//! behind without showing IDs.
//!
//! The language: [`set_language`] wins (a language picker calls it; until there is one,
//! `INLET_LANG=nb` or `INLET_LANG=en` does, through [`init`]), otherwise the first of the
//! system's preferred languages that Inlet has, otherwise English. Dates and numbers
//! follow the same choice ([`formatting_locale`]), and the language is sent to the
//! instance as `Accept-Language` ([`accept_language`]).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, OnceLock};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

/// Every `.ftl` file of `locales/en`, joined by `build.rs`.
const EN: &str = include_str!(concat!(env!("OUT_DIR"), "/locales-en.ftl"));
/// Every `.ftl` file of `locales/nb`.
const NB: &str = include_str!(concat!(env!("OUT_DIR"), "/locales-nb.ftl"));

/// The English catalogue. Shared by the UI and the backend thread, and the fallback for
/// texts another language does not have yet.
static EN_BUNDLE: LazyLock<FluentBundle<FluentResource>> = LazyLock::new(|| bundle("en", EN));
/// The Norwegian Bokmål catalogue.
static NB_BUNDLE: LazyLock<FluentBundle<FluentResource>> = LazyLock::new(|| bundle("nb", NB));

/// A language Inlet has a catalogue for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    NorwegianBokmal,
}

impl Language {
    /// Every language.
    pub const ALL: [Language; 2] = [Language::English, Language::NorwegianBokmal];

    /// The folder under `locales/` and the BCP 47 language tag.
    pub fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::NorwegianBokmal => "nb",
        }
    }

    /// The locale with its region, for dates and numbers.
    pub fn region_locale(self) -> &'static str {
        match self {
            Language::English => "en-US",
            Language::NorwegianBokmal => "nb-NO",
        }
    }

    /// The language a locale name stands for (`nb-NO`, `no`, `nn_NO.UTF-8`, `en_GB`),
    /// or `None` when Inlet has no catalogue for it. Nynorsk gets Bokmål, the nearest
    /// language Inlet has.
    pub fn from_locale(name: &str) -> Option<Language> {
        let name = name.split(['.', '@']).next().unwrap_or_default();
        let lang = name.split(['-', '_']).next().unwrap_or_default();
        match lang.to_ascii_lowercase().as_str() {
            "en" => Some(Language::English),
            "nb" | "nn" | "no" => Some(Language::NorwegianBokmal),
            _ => None,
        }
    }

    /// The first of `locales` (most wanted first) that Inlet has a catalogue for.
    pub fn negotiate<S: AsRef<str>>(locales: &[S]) -> Option<Language> {
        locales
            .iter()
            .find_map(|l| Language::from_locale(l.as_ref()))
    }

    fn code(self) -> u8 {
        match self {
            Language::English => 1,
            Language::NorwegianBokmal => 2,
        }
    }

    fn from_code(code: u8) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.code() == code)
    }
}

/// The language set with [`set_language`]; 0 while none is.
static CHOSEN: AtomicU8 = AtomicU8::new(0);

/// Sets the language, or goes back to following the system with `None`. Texts drawn
/// after this use the new language; `Accept-Language` follows for connections made after
/// it.
pub fn set_language(language: Option<Language>) {
    CHOSEN.store(language.map_or(0, Language::code), Ordering::Relaxed);
    push_accept_language();
}

/// The language the texts are in now.
pub fn language() -> Language {
    Language::from_code(CHOSEN.load(Ordering::Relaxed)).unwrap_or_else(system_language)
}

/// The system's choice, read once. Unit tests always get English, so a developer's own
/// language never changes what they assert.
fn system_language() -> Language {
    static SYSTEM: OnceLock<Language> = OnceLock::new();
    *SYSTEM.get_or_init(|| {
        if cfg!(test) {
            return Language::English;
        }
        Language::negotiate(&sys_locale::get_locales().collect::<Vec<_>>())
            .unwrap_or(Language::English)
    })
}

/// The system's own locale name, read once (`nb-NO`, `en_US.UTF-8`; empty when unknown).
fn system_locale() -> &'static str {
    static LOCALE: OnceLock<String> = OnceLock::new();
    LOCALE.get_or_init(|| sys_locale::get_locale().unwrap_or_default())
}

/// The locale that dates and numbers follow. With a language set by [`set_language`],
/// the region of that language; otherwise the system's regional format, as before the
/// UI had a second language.
pub fn formatting_locale() -> &'static str {
    match Language::from_code(CHOSEN.load(Ordering::Relaxed)) {
        Some(language) => language.region_locale(),
        None => system_locale(),
    }
}

/// The `Accept-Language` value for the instance: the language with its region first,
/// then the bare language, then English as the last resort.
pub fn accept_language() -> String {
    accept_language_for(language())
}

fn accept_language_for(language: Language) -> String {
    match language {
        Language::English => "en-US,en;q=0.9".to_string(),
        language => format!(
            "{},{};q=0.9,en;q=0.5",
            language.region_locale(),
            language.tag()
        ),
    }
}

/// Tells the protocol crate which `Accept-Language` to send.
fn push_accept_language() {
    inlet_protocol::instance::set_accept_language(&accept_language());
}

/// Called once at start-up, before the first connection is made. `INLET_LANG` (for
/// example `en` or `nb`) pins the language, which screenshots and tests of the real
/// views use; then the protocol crate learns the `Accept-Language` to send. A demo build
/// (the one the screenshot checks photograph, never shipped) is English unless
/// `INLET_LANG` says otherwise, so the pictures do not depend on the machine's language.
pub fn init() {
    let pinned = std::env::var("INLET_LANG")
        .ok()
        .and_then(|tag| Language::from_locale(&tag))
        .or(if cfg!(feature = "demo") {
            Some(Language::English)
        } else {
            None
        });
    match pinned {
        Some(language) => set_language(Some(language)),
        None => push_accept_language(),
    }
}

/// The decimal separator of a locale name: a comma for most of Europe, a point
/// otherwise.
pub fn decimal_separator_of(locale: &str) -> char {
    let lang = locale
        .split(['-', '_', '.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match lang.as_str() {
        "nb" | "nn" | "no" | "da" | "sv" | "fi" | "is" | "de" | "fr" | "es" | "it" | "pt"
        | "nl" | "pl" | "cs" | "sk" | "sl" | "hr" | "sr" | "bg" | "ru" | "uk" | "be" | "ro"
        | "hu" | "tr" | "el" | "et" | "lv" | "lt" | "sq" | "mk" | "kk" | "az" | "id" | "vi" => ',',
        _ => '.',
    }
}

/// `value` with `digits` decimals and the decimal separator of [`formatting_locale`].
pub fn decimal(value: f64, digits: usize) -> String {
    decimal_in(value, digits, formatting_locale())
}

fn decimal_in(value: f64, digits: usize, locale: &str) -> String {
    let text = format!("{value:.digits$}");
    match decimal_separator_of(locale) {
        '.' => text,
        sep => text.replace('.', &sep.to_string()),
    }
}

fn bundle(lang: &str, source: &str) -> FluentBundle<FluentResource> {
    let id: LanguageIdentifier = lang.parse().expect("a valid language tag");
    let mut bundle = FluentBundle::new_concurrent(vec![id]);
    // No Unicode isolation marks around arguments: gpui draws them as gaps, and names
    // are already cleaned of direction controls by `model::clean_name`.
    bundle.set_use_isolating(false);
    let resource = FluentResource::try_new(source.to_string()).unwrap_or_else(|(res, errors)| {
        log::error!("text catalogue {lang} has {} syntax errors", errors.len());
        res
    });
    if let Err(errors) = bundle.add_resource(resource) {
        log::error!("text catalogue {lang} has {} duplicate IDs", errors.len());
    }
    bundle
}

/// The text `id` with `args`. Use [`t!`] instead of calling this.
pub fn tr(id: &str, args: Option<&FluentArgs>) -> String {
    tr_in(language(), id, args)
}

/// The text `id` in `language`.
fn tr_in(language: Language, id: &str, args: Option<&FluentArgs>) -> String {
    let own: &FluentBundle<FluentResource> = match language {
        Language::English => &EN_BUNDLE,
        Language::NorwegianBokmal => &NB_BUNDLE,
    };
    tr_from(own, id, args)
}

/// The text `id` from `own`, else from the English catalogue, else the ID itself. A
/// language can lag behind English without showing IDs.
fn tr_from(own: &FluentBundle<FluentResource>, id: &str, args: Option<&FluentArgs>) -> String {
    let en: &FluentBundle<FluentResource> = &EN_BUNDLE;
    let found = [own, en]
        .into_iter()
        .find_map(|b| b.get_message(id).and_then(|m| m.value()).map(|p| (b, p)));
    let Some((bundle, pattern)) = found else {
        log::warn!("no text with the ID {id}");
        return id.to_string();
    };
    let mut errors = Vec::new();
    let text = bundle.format_pattern(pattern, args, &mut errors);
    if !errors.is_empty() {
        log::warn!("text {id}: {errors:?}");
    }
    text.into_owned()
}

/// A value for a placeable in a text: strings and numbers.
pub trait ToArg {
    fn to_arg(&self) -> FluentValue<'static>;
}

impl ToArg for str {
    fn to_arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.to_string())
    }
}

impl ToArg for String {
    fn to_arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.clone())
    }
}

impl<T: ToArg + ?Sized> ToArg for &T {
    fn to_arg(&self) -> FluentValue<'static> {
        (**self).to_arg()
    }
}

macro_rules! number_args {
    ($($ty:ty),*) => {
        $(impl ToArg for $ty {
            fn to_arg(&self) -> FluentValue<'static> {
                FluentValue::from(*self)
            }
        })*
    };
}
number_args!(u8, u16, u32, u64, usize, i32, i64);

/// Looks up a text in the catalogue: `t!("id")` or `t!("id", name = value, …)`. Values
/// are strings or numbers; a placeable `{ $name }` in the text takes the value.
macro_rules! t {
    ($id:literal) => {
        $crate::i18n::tr($id, None)
    };
    ($id:literal $(, $key:ident = $value:expr)+ $(,)?) => {{
        let mut args = ::fluent_bundle::FluentArgs::new();
        $(args.set(stringify!($key), $crate::i18n::ToArg::to_arg(&$value));)+
        $crate::i18n::tr($id, Some(&args))
    }};
}

#[cfg(test)]
mod locales_tests;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use fluent_bundle::FluentResource;
    use fluent_syntax::ast;

    fn crate_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Message IDs and the variables each uses, per catalogue file.
    fn catalogue(lang: &str) -> BTreeMap<String, (String, BTreeSet<String>)> {
        let dir = crate_dir().join("locales").join(lang);
        let mut ids = BTreeMap::new();
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("read the locale directory")
            .map(|e| e.expect("an entry").path())
            .filter(|p| p.extension().is_some_and(|e| e == "ftl"))
            .collect();
        files.sort();
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let source = std::fs::read_to_string(&path).expect("read a catalogue file");
            let resource = FluentResource::try_new(source)
                .unwrap_or_else(|(_, errors)| panic!("{name}: syntax errors {errors:?}"));
            for entry in resource.entries() {
                if let ast::Entry::Message(m) = entry {
                    let mut vars = BTreeSet::new();
                    if let Some(value) = &m.value {
                        for el in &value.elements {
                            if let ast::PatternElement::Placeable {
                                expression:
                                    ast::Expression::Inline(ast::InlineExpression::VariableReference {
                                        id,
                                    }),
                            } = el
                            {
                                vars.insert(id.name.to_string());
                            }
                        }
                    } else {
                        panic!("{name}: {} has no value", m.id.name);
                    }
                    let prev = ids.insert(m.id.name.to_string(), (name.clone(), vars));
                    if let Some((other, _)) = prev {
                        panic!("{} is in both {other} and {name}", m.id.name);
                    }
                }
            }
        }
        ids
    }

    /// One string literal in a source file.
    struct Literal {
        line: usize,
        text: String,
        /// The code on the same line before the literal.
        before: String,
    }

    /// The string literals of a Rust file, up to its `#[cfg(test)]` module. Skips
    /// comments and char literals; good enough for this crate's code.
    fn literals(source: &str) -> Vec<Literal> {
        let source = match source.find("#[cfg(test)]") {
            Some(ix) => &source[..ix],
            None => source,
        };
        let chars: Vec<char> = source.chars().collect();
        let mut out = Vec::new();
        let (mut i, mut line, mut line_start) = (0, 1, 0);
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '\n' {
                line += 1;
                line_start = i + 1;
                i += 1;
            } else if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    if chars[i] == '\n' {
                        line += 1;
                        line_start = i + 1;
                    }
                    i += 1;
                }
                i += 2;
            } else if c == '\'' {
                // A char literal ('x', '\n', '\u{..}') or a lifetime.
                if next == Some('\\') {
                    i += 2;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                } else if chars.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            } else if c == '"' {
                let before: String = chars[line_start..i].iter().collect();
                let start_line = line;
                let mut text = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        text.push(chars[i]);
                        i += 1;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                        line_start = i + 1;
                    }
                    text.push(chars[i]);
                    i += 1;
                }
                i += 1;
                out.push(Literal {
                    line: start_line,
                    text,
                    before,
                });
            } else {
                i += 1;
            }
        }
        out
    }

    /// Whether a literal looks like text a person reads, rather than an element ID, a
    /// key, a URL scheme or a format string made of placeholders and symbols. Text has
    /// a space or a capital letter, or is a word introduced by a symbol (`#name`).
    fn looks_visible(literal: &str) -> bool {
        // Drop escapes (`\u{2026}`, `\n`) and format placeholders (`{name}`, `{}`).
        let mut s = String::new();
        let mut chars = literal.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if chars.next() == Some('u') {
                        for c in chars.by_ref() {
                            if c == '}' {
                                break;
                            }
                        }
                    }
                }
                '{' => {
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                }
                c => s.push(c),
            }
        }
        if !s.chars().any(char::is_alphabetic)
            || s.starts_with("https://")
            || s.starts_with("http://")
        {
            return false;
        }
        let id_like = s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.:/".contains(c));
        !id_like
    }

    /// Code that may hold literals nobody sees: logs, panics and the like.
    fn not_shown(before: &str) -> bool {
        [
            "log::",
            "panic!(",
            "unreachable!(",
            "expect(",
            "assert",
            "env::var",
            "matches!(",
            "env!(",
            "include_str!(",
        ]
        .iter()
        .any(|m| before.contains(m))
    }

    #[test]
    fn every_visible_string_comes_from_the_catalogue() {
        let mut files = Vec::new();
        rust_files(&crate_dir().join("src").join("ui"), &mut files);
        files.push(crate_dir().join("src").join("about.rs"));
        assert!(files.len() > 20, "the ui tree was not found");
        let mut found = Vec::new();
        for path in &files {
            let source = std::fs::read_to_string(path).expect("read a source file");
            for lit in literals(&source) {
                if lit.before.trim_end().ends_with("t!(") || not_shown(&lit.before) {
                    continue;
                }
                if looks_visible(&lit.text) {
                    found.push(format!("{}:{}: {:?}", path.display(), lit.line, lit.text));
                }
            }
        }
        assert!(
            found.is_empty(),
            "visible text outside the catalogue (use t!):\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn every_id_in_the_code_is_in_the_catalogue_with_its_arguments() {
        let ids = catalogue("en");
        let mut files = Vec::new();
        rust_files(&crate_dir().join("src"), &mut files);
        let call = t_calls();
        let mut used = BTreeSet::new();
        let mut problems = Vec::new();
        for path in &files {
            // The macro's own docs and tests use made-up IDs.
            if path.ends_with("i18n.rs") {
                continue;
            }
            let source = std::fs::read_to_string(path).expect("read a source file");
            for (id, keys) in call(&source) {
                used.insert(id.clone());
                match ids.get(&id) {
                    None => problems.push(format!("{}: unknown ID {id}", path.display())),
                    Some((_, vars)) if *vars != keys => problems.push(format!(
                        "{}: {id} takes {vars:?}, the code passes {keys:?}",
                        path.display()
                    )),
                    Some(_) => {}
                }
            }
        }
        for id in ids.keys() {
            if !used.contains(id) {
                problems.push(format!("{id} is in the catalogue but never used"));
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// Finds `t!("id", key = …, …)` calls: the ID and the argument names.
    fn t_calls() -> impl Fn(&str) -> Vec<(String, BTreeSet<String>)> {
        |source: &str| {
            let mut out = Vec::new();
            let mut rest = source;
            while let Some(ix) = rest.find("t!(") {
                let preceded_by_ident = rest[..ix]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
                rest = &rest[ix + 3..];
                if preceded_by_ident {
                    continue;
                }
                let body = rest.trim_start();
                let Some(body) = body.strip_prefix('"') else {
                    continue;
                };
                let Some(end) = body.find('"') else { continue };
                let id = body[..end].to_string();
                // The arguments run to the matching parenthesis.
                let mut depth = 1;
                let mut args = String::new();
                for c in body[end + 1..].chars() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    if depth == 1 {
                        args.push(c);
                    }
                }
                let keys = args
                    .split(',')
                    .filter_map(|a| a.split_once('='))
                    .map(|(k, _)| k.trim().to_string())
                    .filter(|k| !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_'))
                    .collect();
                if !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    out.push((id, keys));
                }
            }
            out
        }
    }

    #[test]
    fn texts_format_with_arguments_and_without_isolation_marks() {
        assert_eq!(t!("common-cancel"), "Cancel");
        assert_eq!(
            t!("offline-retry-in", seconds = 32u64),
            "You are still signed in. Inlet tries again in 32 s."
        );
        assert_eq!(
            t!("composer-placeholder", channel = "#general"),
            "Message #general"
        );
        let name = String::from("ada");
        assert_eq!(
            t!("chat-dismissable", text = name),
            "ada  (click to dismiss)"
        );
        assert_eq!(t!("app-window-title"), "Inlet \u{2013} works with Fluxer");
        assert_eq!(t!("starting"), "Starting\u{2026}");
    }

    #[test]
    fn the_scan_finds_visible_text_and_skips_ids() {
        assert!(looks_visible("Open this link?"));
        assert!(looks_visible("BOT"));
        assert!(looks_visible("#unknown-channel"));
        assert!(looks_visible("Message {name}"));
        assert!(!looks_visible("guild-rail"));
        assert!(!looks_visible("ch-{}"));
        assert!(!looks_visible("{id}-media{i}"));
        assert!(!looks_visible("\\u{21B1}"));
        assert!(!looks_visible("@{}"));
        assert!(!looks_visible("https"));
        let lits = literals(
            "let a = t!(\"x-y\");\n// \"Hidden\"\nlet b = 'c'; div().child(\"Shown text\");",
        );
        assert_eq!(lits.len(), 2);
        assert!(lits[0].before.ends_with("t!("));
        assert_eq!(lits[1].text, "Shown text");
        assert_eq!(lits[1].line, 3);
    }
}
