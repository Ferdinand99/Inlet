// SPDX-License-Identifier: AGPL-3.0-or-later
//! Text and CSV attachments: which files get a preview under their message, fetching
//! their text from the media proxy, and reading CSV into rows (Fluxer's
//! `AttachmentPreviewUtils`, `TextualAttachmentPreviewFetch`, `CsvAttachmentPreviewUtils`).
//!
//! The text is untrusted. It is only ever shown as plain text: nothing in it is run,
//! opened, followed or read as markup. It is fetched with a hard cap
//! ([`MAX_BYTES`], also checked against the announced size and while streaming), cut to
//! [`MAX_CHARS`] characters, and cleaned of control characters, line breaks aside, and of
//! characters that can reorder or hide other text. A line is cut at [`MAX_LINE_CHARS`] for
//! display, a CSV table at [`MAX_CSV_ROWS`] rows, [`MAX_CSV_COLUMNS`] columns and
//! [`MAX_CELL_CHARS`] characters a cell.

use std::sync::Arc;

use futures::StreamExt;
use inlet_media::{FetchError, Fetcher};
use url::Url;

/// Largest file that gets a preview (Fluxer's `TEXT_PREVIEW_MAX_BYTES`).
pub const MAX_BYTES: usize = 128 * 1024;
/// Most characters kept of a file.
pub const MAX_CHARS: usize = 128 * 1024;
/// A line is cut after this many characters when shown.
pub const MAX_LINE_CHARS: usize = 400;
/// Most CSV rows kept.
pub const MAX_CSV_ROWS: usize = 2_000;
/// Most CSV columns kept per row.
pub const MAX_CSV_COLUMNS: usize = 32;
/// A CSV cell is cut after this many characters when shown.
pub const MAX_CELL_CHARS: usize = 120;
/// Most cells the table holds (Fluxer's `MAX_CSV_TABLE_CELLS` is 262144; ours is smaller
/// because every cell is a drawn box).
pub const MAX_CSV_CELLS: usize = 20_000;

const TEXTUAL_MIME_PREFIXES: &[&str] = &[
    "text/",
    "application/json",
    "application/ld+json",
    "application/xml",
    "application/javascript",
];
const TEXTUAL_MIME_EXACT: &[&str] = &[
    "application/x-sh",
    "application/x-shellscript",
    "application/x-bash",
    "application/sql",
    "application/x-yaml",
    "package/manifest+json",
];
const TEXTUAL_EXTENSIONS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "log",
    "json",
    "js",
    "jsx",
    "ts",
    "tsx",
    "py",
    "java",
    "c",
    "cpp",
    "h",
    "cs",
    "css",
    "scss",
    "sass",
    "html",
    "htm",
    "xml",
    "yml",
    "yaml",
    "sh",
    "bash",
    "ps1",
    "go",
    "rb",
    "php",
    "rust",
    "rs",
    "toml",
    "dockerfile",
    "sql",
    "ini",
    "cfg",
    "conf",
    "csv",
];

/// The MIME type without parameters, in lower case.
fn mime(content_type: Option<&str>) -> String {
    content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase()
}

fn extension(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.trim().to_lowercase())
        .unwrap_or_default()
}

/// Whether a file gets a text preview: a text type or a known text extension, and not
/// larger than [`MAX_BYTES`] when its size is known.
pub fn previewable(content_type: Option<&str>, filename: &str, size: u64) -> bool {
    if size > MAX_BYTES as u64 {
        return false;
    }
    let kind = mime(content_type);
    if !kind.is_empty()
        && (TEXTUAL_MIME_PREFIXES.iter().any(|p| kind.starts_with(p))
            || TEXTUAL_MIME_EXACT.contains(&kind.as_str()))
    {
        return true;
    }
    TEXTUAL_EXTENSIONS.contains(&extension(filename).as_str())
}

/// Whether a file is shown as a table.
pub fn is_csv(content_type: Option<&str>, filename: &str) -> bool {
    let kind = mime(content_type);
    extension(filename) == "csv" || kind == "text/csv" || kind == "application/csv"
}

/// Why a preview could not be made.
#[derive(Debug)]
pub enum TextError {
    /// Larger than [`MAX_BYTES`].
    TooLarge,
    Fetch(FetchError),
}

/// The text of a file, from the media proxy: at most [`MAX_BYTES`] are read.
pub async fn fetch_text(fetcher: Arc<dyn Fetcher>, url: Url) -> Result<String, TextError> {
    let resp = fetcher.fetch(&url).await.map_err(TextError::Fetch)?;
    if resp.content_length.is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(TextError::TooLarge);
    }
    let mut body = resp.body;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(TextError::Fetch)?;
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err(TextError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(clean(&String::from_utf8_lossy(&bytes)))
}

/// Characters that can reorder or hide text, or that are not text at all.
fn is_unsafe(c: char) -> bool {
    (c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        || matches!(c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            | '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{061C}')
}

/// The text as shown: at most [`MAX_CHARS`] characters, a leading byte order mark gone,
/// unsafe characters dropped, tabs as four spaces.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_CHARS));
    for c in text.trim_start_matches('\u{FEFF}').chars().take(MAX_CHARS) {
        match c {
            '\t' => out.push_str("    "),
            c if is_unsafe(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// The lines of a text, a line break being `\n`, `\r\n` or `\r`.
pub fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find(['\n', '\r']) {
            Some(at) => {
                out.push(&rest[..at]);
                let after = &rest[at..];
                let skip = if after.starts_with("\r\n") { 2 } else { 1 };
                rest = &after[skip..];
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

/// A line or cell cut to `max` characters, with an ellipsis when it was longer.
pub fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}\u{2026}", &text[..at]),
        None => text.to_owned(),
    }
}

/// CSV rows, as Fluxer reads them: commas separate cells, double quotes hold commas and
/// line breaks (`""` is a quote), and a line break ends a row. Limits: [`MAX_CSV_ROWS`]
/// rows, [`MAX_CSV_COLUMNS`] columns, [`MAX_CSV_CELLS`] cells; the rest is dropped.
pub fn parse_csv(content: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut cells = 0usize;
    let mut chars = content.chars().peekable();
    let push_cell = |row: &mut Vec<String>, cell: &mut String, cells: &mut usize| {
        let done = std::mem::take(cell);
        if row.len() < MAX_CSV_COLUMNS && *cells < MAX_CSV_CELLS {
            row.push(done);
            *cells += 1;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => push_cell(&mut row, &mut cell, &mut cells),
            '\n' | '\r' if !in_quotes => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                push_cell(&mut row, &mut cell, &mut cells);
                if rows.len() < MAX_CSV_ROWS {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            c => {
                // A cell keeps only what it can show; the rest of a very long one is dropped
                // as it is read.
                if cell.chars().count() <= MAX_CELL_CHARS * 4 {
                    cell.push(c);
                }
            }
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        push_cell(&mut row, &mut cell, &mut cells);
        if rows.len() < MAX_CSV_ROWS {
            rows.push(row);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_files_get_a_preview() {
        assert!(previewable(Some("text/plain; charset=utf-8"), "a.bin", 10));
        assert!(previewable(Some("application/json"), "a", 10));
        assert!(previewable(None, "main.RS", 10));
        assert!(previewable(
            Some("application/octet-stream"),
            "notes.md",
            10
        ));
        assert!(previewable(None, "data.csv", 0), "an unknown size passes");
        assert!(!previewable(Some("image/png"), "a.png", 10));
        assert!(!previewable(Some("application/zip"), "a.zip", 10));
        assert!(!previewable(None, "noextension", 10));
        // Over the cap: no preview, so nothing is fetched for it.
        assert!(!previewable(
            Some("text/plain"),
            "big.txt",
            MAX_BYTES as u64 + 1
        ));
        assert!(previewable(Some("text/plain"), "ok.txt", MAX_BYTES as u64));
    }

    #[test]
    fn csv_files_are_found_by_type_or_extension() {
        assert!(is_csv(Some("text/csv"), "x"));
        assert!(is_csv(Some("application/csv; x=y"), "x"));
        assert!(is_csv(None, "DATA.CSV"));
        assert!(!is_csv(Some("text/plain"), "a.txt"));
    }

    #[test]
    fn text_is_cleaned_before_it_is_shown() {
        assert_eq!(clean("\u{feff}a\tb\u{0}c\u{202e}d\u{200b}e"), "a    bcde");
        assert_eq!(clean("l1\r\nl2\nl3"), "l1\r\nl2\nl3");
        let long = "x".repeat(MAX_CHARS + 50);
        assert_eq!(clean(&long).chars().count(), MAX_CHARS);
    }

    #[test]
    fn lines_split_on_every_kind_of_break() {
        assert_eq!(lines("a\r\nb\rc\nd"), ["a", "b", "c", "d"]);
        assert_eq!(lines("a\n"), ["a"]);
        assert_eq!(lines("a\n\nb"), ["a", "", "b"]);
        assert!(lines("").is_empty());
    }

    #[test]
    fn long_lines_and_cells_are_cut() {
        assert_eq!(cut("hello", 10), "hello");
        assert_eq!(cut("hello world", 5), "hello\u{2026}");
        assert_eq!(cut("åäö", 2), "åä\u{2026}");
    }

    #[test]
    fn csv_is_read_like_fluxer_reads_it() {
        let rows = parse_csv(
            "name,note\r\nAda,\"likes, commas\"\n\"Say \"\"hi\"\"\",\"two\nlines\"\nlast,",
        );
        assert_eq!(
            rows,
            vec![
                vec!["name", "note"],
                vec!["Ada", "likes, commas"],
                vec!["Say \"hi\"", "two\nlines"],
                vec!["last", ""],
            ]
        );
        assert!(parse_csv("").is_empty());
        assert_eq!(parse_csv("a,b"), vec![vec!["a", "b"]]);
    }

    #[test]
    fn csv_size_is_capped_in_rows_columns_and_cells() {
        let many_rows = "a\n".repeat(MAX_CSV_ROWS + 500);
        assert_eq!(parse_csv(&many_rows).len(), MAX_CSV_ROWS);
        let wide = (0..MAX_CSV_COLUMNS * 3)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(parse_csv(&wide)[0].len(), MAX_CSV_COLUMNS);
        let huge_cell = format!("\"{}\"", "y".repeat(10_000));
        let cell = &parse_csv(&huge_cell)[0][0];
        assert!(cell.chars().count() <= MAX_CELL_CHARS * 4 + 1);
        // The cell budget stops a table of many columns and many rows.
        let grid = format!("{}\n", vec!["c"; MAX_CSV_COLUMNS].join(",")).repeat(MAX_CSV_ROWS);
        let cells: usize = parse_csv(&grid).iter().map(Vec::len).sum();
        assert!(cells <= MAX_CSV_CELLS);
    }

    #[tokio::test]
    async fn fetching_stops_at_the_cap() {
        use futures::future::BoxFuture;
        use inlet_media::FetchResponse;

        struct Body(Vec<u8>, Option<u64>);
        impl Fetcher for Body {
            fn fetch<'a>(&'a self, _: &'a Url) -> BoxFuture<'a, Result<FetchResponse, FetchError>> {
                let mut r = FetchResponse::from_bytes(bytes::Bytes::from(self.0.clone()));
                if let Some(n) = self.1 {
                    r.content_length = Some(n);
                }
                Box::pin(async move { Ok(r) })
            }
        }
        let url = Url::parse("https://media.example.org/attachments/1/2/a.txt").unwrap();
        let ok = fetch_text(Arc::new(Body(b"hello\tworld".to_vec(), None)), url.clone())
            .await
            .unwrap();
        assert_eq!(ok, "hello    world");
        let big = vec![b'a'; MAX_BYTES + 1];
        assert!(matches!(
            fetch_text(Arc::new(Body(big, None)), url.clone()).await,
            Err(TextError::TooLarge)
        ));
        // An announced size over the cap is refused before the body is read.
        assert!(matches!(
            fetch_text(
                Arc::new(Body(b"x".to_vec(), Some(MAX_BYTES as u64 + 1))),
                url
            )
            .await,
            Err(TextError::TooLarge)
        ));
    }
}
