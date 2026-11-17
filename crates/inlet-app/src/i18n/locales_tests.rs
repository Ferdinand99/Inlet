// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of the language catalogues: Norwegian Bokmål against English, the choice of
//! language, and the formatting that follows it. They call the language-taking functions
//! (`tr_in`, `accept_language_for`, `decimal_in`) so none of them touches the process-wide
//! language that the other tests read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fluent_bundle::{FluentArgs, FluentResource};
use fluent_syntax::ast::{
    CallArguments, Entry, Expression, InlineExpression, Pattern, PatternElement,
};

use super::*;

fn locales_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales")
}

fn collect_arguments(arguments: &CallArguments<&str>, out: &mut BTreeSet<String>) {
    for positional in &arguments.positional {
        collect_inline(positional, out);
    }
    for named in &arguments.named {
        collect_inline(&named.value, out);
    }
}

fn collect_inline(expression: &InlineExpression<&str>, out: &mut BTreeSet<String>) {
    match expression {
        InlineExpression::VariableReference { id } => {
            out.insert(id.name.to_string());
        }
        InlineExpression::FunctionReference { arguments, .. } => collect_arguments(arguments, out),
        InlineExpression::TermReference {
            arguments: Some(arguments),
            ..
        } => collect_arguments(arguments, out),
        InlineExpression::Placeable { expression } => collect_expression(expression, out),
        _ => {}
    }
}

fn collect_expression(expression: &Expression<&str>, out: &mut BTreeSet<String>) {
    match expression {
        Expression::Inline(inline) => collect_inline(inline, out),
        Expression::Select { selector, variants } => {
            collect_inline(selector, out);
            for variant in variants {
                collect_pattern(&variant.value, out);
            }
        }
    }
}

/// Every variable a pattern uses, also inside selectors and their variants.
fn collect_pattern(pattern: &Pattern<&str>, out: &mut BTreeSet<String>) {
    for element in &pattern.elements {
        if let PatternElement::Placeable { expression } = element {
            collect_expression(expression, out);
        }
    }
}

/// Message ID and the variables its text uses, for every `.ftl` file of a language.
/// Panics on a syntax error and on an ID in two files.
fn texts(lang: &str) -> BTreeMap<String, BTreeSet<String>> {
    let dir = locales_dir().join(lang);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "ftl"))
        .collect();
    files.sort();
    let mut out = BTreeMap::new();
    let mut origin: BTreeMap<String, String> = BTreeMap::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(&path).expect("read a catalogue file");
        let resource = FluentResource::try_new(source)
            .unwrap_or_else(|(_, errors)| panic!("{lang}/{name}: syntax errors {errors:?}"));
        for entry in resource.entries() {
            let Entry::Message(message) = entry else {
                continue;
            };
            let id = message.id.name.to_string();
            let Some(value) = &message.value else {
                panic!("{lang}/{name}: {id} has no value");
            };
            let mut vars = BTreeSet::new();
            collect_pattern(value, &mut vars);
            if let Some(other) = origin.insert(id.clone(), name.clone()) {
                panic!("{lang}: {id} is in both {other} and {name}");
            }
            out.insert(id, vars);
        }
    }
    out
}

fn list(ids: &[&String]) -> String {
    ids.iter()
        .map(|i| format!("  {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_language_folder_is_a_language_inlet_has() {
    let mut folders: Vec<String> = std::fs::read_dir(locales_dir())
        .expect("read locales/")
        .map(|e| e.expect("an entry").path())
        .filter(|p| p.is_dir())
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    folders.sort();
    let mut tags: Vec<String> = Language::ALL.iter().map(|l| l.tag().to_string()).collect();
    tags.sort();
    assert_eq!(
        folders, tags,
        "a folder under locales/ needs a Language variant (and the other way round)"
    );
}

#[test]
fn the_norwegian_catalogue_has_no_text_english_lacks() {
    let en = texts("en");
    let nb = texts("nb");
    let extra: Vec<_> = nb.keys().filter(|k| !en.contains_key(*k)).collect();
    assert!(
        extra.is_empty(),
        "locales/nb has IDs that locales/en does not (renamed or removed in English?):\n{}",
        list(&extra)
    );
}

#[test]
fn the_norwegian_texts_take_the_arguments_of_the_english_texts() {
    let en = texts("en");
    let nb = texts("nb");
    let mut wrong = Vec::new();
    for (id, vars) in &nb {
        if let Some(english) = en.get(id) {
            if english != vars {
                wrong.push(format!("  {id}: English {english:?}, Norwegian {vars:?}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "arguments differ from English:\n{}",
        wrong.join("\n")
    );
}

/// Norwegian has every English text. A new English text that another change adds shows
/// as a warning here and in the `Locales` workflow, not as a failure, so that change is
/// not held up; the English text shows until the Norwegian one is added. Set
/// `INLET_NB_STRICT=1` to make it a failure (a release gate can).
#[test]
fn the_norwegian_catalogue_covers_every_english_text() {
    let en = texts("en");
    let nb = texts("nb");
    let missing: Vec<_> = en.keys().filter(|k| !nb.contains_key(*k)).collect();
    if missing.is_empty() {
        return;
    }
    let message = format!(
        "{} texts have no Norwegian version in locales/nb:\n{}",
        missing.len(),
        list(&missing)
    );
    if std::env::var("INLET_NB_STRICT").is_ok_and(|v| v == "1") {
        panic!("{message}");
    }
    eprintln!("warning: {message}");
}

fn args(pairs: &[(&'static str, i64)]) -> FluentArgs<'static> {
    let mut a = FluentArgs::new();
    for (k, v) in pairs {
        a.set(*k, *v);
    }
    a
}

#[test]
fn plurals_dates_and_counts_read_naturally_in_norwegian() {
    let nb =
        |id: &str, a: &[(&'static str, i64)]| tr_in(Language::NorwegianBokmal, id, Some(&args(a)));
    assert_eq!(nb("social-wait-seconds", &[("count", 1)]), "1 sekund");
    assert_eq!(nb("social-wait-seconds", &[("count", 30)]), "30 sekunder");
    assert_eq!(nb("social-wait-minutes", &[("count", 1)]), "1 minutt");
    assert_eq!(nb("social-wait-hours", &[("count", 2)]), "2 timer");
    assert_eq!(
        nb("social-blocked-messages", &[("count", 1)]),
        "1 blokkert melding"
    );
    assert_eq!(
        nb("social-blocked-messages", &[("count", 3)]),
        "3 blokkerte meldinger"
    );
    assert_eq!(
        nb("social-new-dm-body", &[("count", 4)]),
        "Du kan legge til 4 venner til."
    );
    // A date separator: "torsdag 1. oktober 2026", as Fluxer's Norwegian writes it.
    let month = nb("markdown-month-10", &[]);
    let weekday = nb("markdown-weekday-4", &[]);
    assert_eq!(month, "oktober");
    assert_eq!(weekday, "torsdag");
    let mut a = FluentArgs::new();
    a.set("month", month);
    a.set("day", 1);
    a.set("year", 2026);
    let long = tr_in(
        Language::NorwegianBokmal,
        "markdown-date-long-dmy",
        Some(&a),
    );
    assert_eq!(long, "1. oktober 2026");
    let mut a = FluentArgs::new();
    a.set("weekday", weekday);
    a.set("date", long);
    assert_eq!(
        tr_in(
            Language::NorwegianBokmal,
            "markdown-weekday-and-date",
            Some(&a)
        ),
        "torsdag 1. oktober 2026"
    );
}

#[test]
fn a_text_a_language_lacks_shows_in_english_not_as_an_id() {
    let partial = bundle("nb", "only-here = Bare her\n");
    assert_eq!(tr_from(&partial, "only-here", None), "Bare her");
    assert_eq!(tr_from(&partial, "common-cancel", None), "Cancel");
    assert_eq!(tr_from(&partial, "no-such-id", None), "no-such-id");
}

#[test]
fn english_and_norwegian_differ_where_they_should() {
    let en = |id: &str| tr_in(Language::English, id, None);
    let nb = |id: &str| tr_in(Language::NorwegianBokmal, id, None);
    assert_eq!(en("common-cancel"), "Cancel");
    assert_eq!(nb("common-cancel"), "Avbryt");
    assert_eq!(nb("common-sign-out"), "Logg ut");
    assert_eq!(nb("app-window-title"), "Inlet \u{2013} fungerer med Fluxer");
    assert_eq!(nb("starting"), "Starter\u{2026}");
}

#[test]
fn locales_map_onto_the_languages_inlet_has() {
    assert_eq!(
        Language::from_locale("nb-NO"),
        Some(Language::NorwegianBokmal)
    );
    assert_eq!(
        Language::from_locale("nb_NO.UTF-8"),
        Some(Language::NorwegianBokmal)
    );
    assert_eq!(Language::from_locale("no"), Some(Language::NorwegianBokmal));
    assert_eq!(
        Language::from_locale("NN-no"),
        Some(Language::NorwegianBokmal)
    );
    assert_eq!(Language::from_locale("en_GB@euro"), Some(Language::English));
    assert_eq!(Language::from_locale("en"), Some(Language::English));
    assert_eq!(Language::from_locale("de-DE"), None);
    assert_eq!(Language::from_locale(""), None);
    assert_eq!(Language::from_locale("C"), None);
    // The first language Inlet has wins, in the order the system prefers them.
    assert_eq!(
        Language::negotiate(&["de-DE", "nb-NO", "en-US"]),
        Some(Language::NorwegianBokmal)
    );
    assert_eq!(
        Language::negotiate(&["fr-FR", "en-GB", "nb"]),
        Some(Language::English)
    );
    assert_eq!(Language::negotiate::<&str>(&[]), None);
}

#[test]
fn accept_language_names_the_language_first() {
    assert_eq!(accept_language_for(Language::English), "en-US,en;q=0.9");
    assert_eq!(
        accept_language_for(Language::NorwegianBokmal),
        "nb-NO,nb;q=0.9,en;q=0.5"
    );
    // Plain visible ASCII, so it is a valid header value for every language.
    for language in Language::ALL {
        assert!(accept_language_for(language)
            .bytes()
            .all(|b| (0x20..0x7f).contains(&b)));
    }
}

#[test]
fn decimals_use_the_separator_of_the_locale() {
    assert_eq!(decimal_in(1.5, 1, "en-US"), "1.5");
    assert_eq!(decimal_in(1.5, 1, "nb-NO"), "1,5");
    assert_eq!(decimal_in(12.0, 0, "nb"), "12");
    assert_eq!(decimal_in(2.75, 2, "de_DE.UTF-8"), "2,75");
    assert_eq!(decimal_in(2.0, 1, ""), "2.0");
    assert_eq!(decimal_separator_of("nn-NO"), ',');
    assert_eq!(decimal_separator_of("ja-JP"), '.');
}
