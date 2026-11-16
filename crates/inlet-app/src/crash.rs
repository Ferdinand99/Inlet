// SPDX-License-Identifier: AGPL-3.0-or-later
//! The crash log: a panic hook that writes a small text file, and the text of "Copy
//! diagnostics".
//!
//! A report holds the Inlet version and source commit, the operating system and
//! architecture, the thread name, the place of the panic, the panic message and the
//! backtrace, and nothing else. Everything that comes from the running program (message,
//! place, thread name, backtrace) goes through [`redact`] before it is written:
//!
//! - URLs keep their scheme and host only (no user info, path, query or fragment);
//! - the home folder becomes `~`, and the name after any `/home/`, `/Users/` or
//!   `\Users\` becomes `user`;
//! - the value after `token`, `authorization`, `password`, `secret`, `cookie`, `apikey`
//!   (`=` or `:`) and after `Bearer` is replaced;
//! - long runs of letters and digits that look like a token or a key are replaced;
//! - runs of 15 or more digits (message and user IDs) are replaced;
//! - a message that was built with `format!` (so it may hold chat text or names that an
//!   `unwrap` on an error printed) is cut to its first lines, and everything between
//!   double quotes is dropped.
//!
//! No message text, channel name or user name is collected on purpose: the hook reads
//! nothing from the app's state. What is left of a panic message after the steps above
//! can still hold a word that is not secret-shaped; that is why the message is short and
//! why a formatted message loses its quoted parts. Reports are plain files in the OS
//! state folder (the cache folder where there is no state folder), at most [`KEEP`] of
//! them, each at most [`MAX_FILE_BYTES`] bytes, and are never sent anywhere. Nothing is
//! read from them except by [`diagnostics`], which the user copies by hand.

use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many reports are kept; older ones are deleted when a new one is written.
pub const KEEP: usize = 5;

/// The most bytes in one report file; a longer report is cut and says so.
pub const MAX_FILE_BYTES: usize = 32 * 1024;

/// The most characters of a panic message that are kept (a message built with `format!`
/// keeps [`MAX_FORMATTED_MESSAGE_CHARS`]).
const MAX_MESSAGE_CHARS: usize = 300;
const MAX_FORMATTED_MESSAGE_CHARS: usize = 200;
const MAX_MESSAGE_LINES: usize = 5;

const FILE_PREFIX: &str = "crash-";
const FILE_SUFFIX: &str = ".txt";
const CUT_MARK: &str = "\n[report cut at the size limit]\n";

static INSTALLED: AtomicBool = AtomicBool::new(false);
static IN_HOOK: AtomicBool = AtomicBool::new(false);
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// Where reports go: `<state folder>/inlet/crashes`, or `<cache folder>/inlet/crashes`.
pub fn crash_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::cache_dir)
        .map(|d| d.join("inlet").join("crashes"))
}

/// Installs the panic hook once. The hook writes a report and then runs the hook that was
/// there before (the default one prints the message to the console). Without a state or
/// cache folder no report is written. Call it first thing in `main`.
pub fn install() {
    if let Some(dir) = crash_dir() {
        install_in(dir);
    }
}

/// [`install`] with a chosen folder (the tests use it).
pub fn install_in(dir: PathBuf) {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // A panic while writing a report, or a second thread panicking meanwhile, must
        // not loop or abort; the report of the first one is enough.
        if !IN_HOOK.swap(true, Ordering::SeqCst) {
            let _ = panic::catch_unwind(AssertUnwindSafe(|| {
                let report = report_for(info);
                let _ = write_report(&dir, &report, KEEP);
            }));
            IN_HOOK.store(false, Ordering::SeqCst);
        }
        previous(info);
    }));
}

/// The text of "Copy diagnostics": the version, the system and, when there is one, the
/// newest crash report. It holds nothing that [`redact`] would remove.
pub fn diagnostics() -> String {
    let mut text = header();
    match crash_dir().and_then(|dir| newest_report(&dir)) {
        Some(report) => {
            text.push_str("\nNewest crash report:\n\n");
            text.push_str(&report);
        }
        None => text.push_str("\nNo crash report.\n"),
    }
    text
}

/// What a panic hook saw, before redaction.
struct Raw<'a> {
    thread: &'a str,
    location: Option<String>,
    message: &'a str,
    /// The message was built with `format!`, so it may hold data.
    formatted: bool,
    backtrace: &'a str,
}

fn report_for(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let (message, formatted) = if let Some(s) = payload.downcast_ref::<&str>() {
        ((*s).to_string(), false)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        (s.clone(), true)
    } else {
        ("(a panic with no text)".to_string(), false)
    };
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    let thread = std::thread::current();
    let backtrace = Backtrace::force_capture().to_string();
    let home = dirs::home_dir();
    render(
        &Raw {
            thread: thread.name().unwrap_or("unnamed"),
            location,
            message: &message,
            formatted,
            backtrace: &backtrace,
        },
        home.as_deref().and_then(Path::to_str),
    )
}

/// The lines every report and the diagnostics start with.
fn header() -> String {
    format!(
        "Inlet {}\nsource: {}\nos: {} {}\n",
        env!("CARGO_PKG_VERSION"),
        env!("INLET_SOURCE_REV"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn render(raw: &Raw<'_>, home: Option<&str>) -> String {
    let message = if raw.formatted {
        let first = first_lines(raw.message, MAX_MESSAGE_LINES);
        cut_chars(&blank_quoted(&first), MAX_FORMATTED_MESSAGE_CHARS)
    } else {
        cut_chars(
            &first_lines(raw.message, MAX_MESSAGE_LINES),
            MAX_MESSAGE_CHARS,
        )
    };
    let mut out = String::from("Inlet crash report\n");
    out.push_str(&header());
    out.push_str(&format!(
        "time: {}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    ));
    out.push_str(&format!("thread: {}\n", redact(raw.thread, home)));
    if let Some(location) = &raw.location {
        out.push_str(&format!("location: {}\n", redact(location, home)));
    }
    out.push_str(&format!("message: {}\n", redact(&message, home)));
    out.push_str("backtrace:\n");
    out.push_str(&redact(raw.backtrace, home));
    out
}

fn first_lines(text: &str, n: usize) -> String {
    text.lines().take(n).collect::<Vec<_>>().join("\n")
}

fn cut_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_string(),
    }
}

/// Replaces the text between double quotes with an ellipsis (a string printed with `{:?}`
/// is where chat text and names show up). An unterminated quote drops the rest.
fn blank_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            out.push(c);
            continue;
        }
        out.push_str("\"...");
        let mut closed = false;
        while let Some(q) = chars.next() {
            if q == '\\' {
                chars.next();
            } else if q == '"' {
                closed = true;
                break;
            }
        }
        if closed {
            out.push('"');
        }
    }
    out
}

/// Removes what must not be in a report or in the diagnostics; see the module comment.
/// `home` is the home folder, when known.
pub fn redact(text: &str, home: Option<&str>) -> String {
    let text = redact_urls(text);
    let text = match home {
        Some(home) => replace_home(&text, home),
        None => text,
    };
    let text = redact_profile_paths(&text);
    let text = redact_secret_pairs(&text);
    let text = redact_runs(&text);
    redact_digits(&text)
}

/// Replaces every ASCII-case-insensitive occurrence of `needle`.
fn replace_ci(haystack: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut at = 0;
    while let Some(pos) = lower[at..].find(&needle) {
        out.push_str(&haystack[at..at + pos]);
        out.push_str(with);
        at += pos + needle.len();
    }
    out.push_str(&haystack[at..]);
    out
}

fn replace_home(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    // A short "home" such as "C:" or "" would eat the whole text.
    if home.len() < 4 {
        return text.to_string();
    }
    let flipped: String = home
        .chars()
        .map(|c| match c {
            '\\' => '/',
            '/' => '\\',
            c => c,
        })
        .collect();
    let text = replace_ci(text, home, "~");
    replace_ci(&text, &flipped, "~")
}

/// The name after `/home/`, `/Users/` and `\Users\` becomes `user`: other accounts' home
/// folders (CI paths, a path in a dependency's panic location) name people too.
fn redact_profile_paths(text: &str) -> String {
    const MARKERS: [&str; 3] = ["/home/", "/users/", "\\users\\"];
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some(marker) = MARKERS.iter().find(|m| lower[i..].starts_with(**m)) {
            let start = i + marker.len();
            out.push_str(&text[i..start]);
            let len = text[start..]
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(c, '/' | '\\' | '"' | '\'' | ':' | ')' | ']' | '>' | ',')
                })
                .unwrap_or(text.len() - start);
            if len > 0 {
                out.push_str("user");
            }
            i = start + len;
        } else {
            let c = text[i..].chars().next().expect("not at the end");
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// A URL keeps its scheme and host: no user info, path, query or fragment.
fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = text[i..].find("://") {
        let sep = i + pos;
        let end = text[sep + 3..]
            .find(|c: char| {
                c.is_whitespace()
                    || matches!(c, '"' | '\'' | '<' | '>' | '`' | ')' | ']' | '}' | ',')
            })
            .map(|p| sep + 3 + p)
            .unwrap_or(text.len());
        let rest = &text[sep + 3..end];
        let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_len];
        let host = authority.rsplit('@').next().unwrap_or("");
        out.push_str(&text[i..sep + 3]);
        out.push_str(host);
        if authority_len < rest.len() {
            out.push_str("/...");
        }
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

const SECRET_KEYS: [&str; 7] = [
    "token",
    "authorization",
    "password",
    "passwd",
    "secret",
    "cookie",
    "apikey",
];

/// The end of the value that starts at `from`: the next space, quote, comma or bracket.
fn value_end(text: &str, from: usize) -> usize {
    text[from..]
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ')' | '}' | ']'))
        .map(|p| from + p)
        .unwrap_or(text.len())
}

/// `Bearer x`, and `token=x`, `"password": "x"`, `Authorization: x` and the like.
fn redact_secret_pairs(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if lower[i..].starts_with("bearer ") {
            let from = i + "bearer ".len();
            out.push_str(&text[i..from]);
            let end = value_end(text, from);
            if end > from {
                out.push_str("[redacted]");
            }
            i = end;
            continue;
        }
        let key = SECRET_KEYS.iter().find(|k| lower[i..].starts_with(**k));
        let Some(key) = key else {
            let c = text[i..].chars().next().expect("not at the end");
            out.push(c);
            i += c.len_utf8();
            continue;
        };
        // The rest of the word (`token_value`, `access_token`), a closing quote, spaces,
        // then `=` or `:` (but not the `::` of a Rust path).
        let mut p = i + key.len();
        while p < bytes.len()
            && (bytes[p].is_ascii_alphanumeric() || matches!(bytes[p], b'_' | b'-'))
        {
            p += 1;
        }
        let key_end = p;
        if matches!(bytes.get(p), Some(b'"' | b'\'')) {
            p += 1;
        }
        while bytes.get(p) == Some(&b' ') {
            p += 1;
        }
        let is_pair = matches!(bytes.get(p), Some(b'=' | b':'))
            && !(bytes[p] == b':' && bytes.get(p + 1) == Some(&b':'));
        if !is_pair {
            out.push_str(&text[i..key_end]);
            i = key_end;
            continue;
        }
        p += 1;
        if bytes.get(p) == Some(&b'>') {
            p += 1;
        }
        while bytes.get(p) == Some(&b' ') {
            p += 1;
        }
        if matches!(bytes.get(p), Some(b'"' | b'\'')) {
            p += 1;
        }
        let end = value_end(text, p);
        out.push_str(&text[i..p]);
        if end > p {
            out.push_str("[redacted]");
        }
        i = end;
    }
    out
}

fn is_run_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.')
}

/// A run of letters and digits (with `_ - + .`) that looks like a token, a key or a hash
/// of something secret: 20 or more characters with both a letter and a digit, or 32 or
/// more of mixed case without an underscore.
fn looks_like_secret(run: &str) -> bool {
    if run.len() < 20 {
        return false;
    }
    let digit = run.bytes().any(|c| c.is_ascii_digit());
    let alpha = run.bytes().any(|c| c.is_ascii_alphabetic());
    let upper = run.bytes().any(|c| c.is_ascii_uppercase());
    let lower = run.bytes().any(|c| c.is_ascii_lowercase());
    (digit && alpha) || (run.len() >= 32 && upper && lower && !run.contains('_'))
}

fn redact_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run_start: Option<usize> = None;
    let flush = |out: &mut String, from: usize, to: usize| {
        let run = &text[from..to];
        if looks_like_secret(run) {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
    };
    for (i, c) in text.char_indices() {
        if is_run_char(c) {
            run_start.get_or_insert(i);
        } else {
            if let Some(from) = run_start.take() {
                flush(&mut out, from, i);
            }
            out.push(c);
        }
    }
    if let Some(from) = run_start {
        flush(&mut out, from, text.len());
    }
    out
}

/// Runs of 15 or more digits (user, message and channel IDs).
fn redact_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut digits = String::new();
    let flush = |out: &mut String, digits: &mut String| {
        if digits.len() >= 15 {
            out.push_str("[id]");
        } else {
            out.push_str(digits);
        }
        digits.clear();
    };
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            flush(&mut out, &mut digits);
            out.push(c);
        }
    }
    flush(&mut out, &mut digits);
    out
}

/// Writes `text` as a new report file in `dir` (cut at [`MAX_FILE_BYTES`]) and deletes
/// the oldest reports beyond `keep`. Only files named like Inlet's own reports are ever
/// deleted.
pub fn write_report(dir: &Path, text: &str, keep: usize) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    // Millisecond time first, then the counter: sorting by name sorts by age.
    let name = format!(
        "{FILE_PREFIX}{millis:016}-{seq:06}-{}{FILE_SUFFIX}",
        std::process::id()
    );
    let path = dir.join(name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(bounded(text).as_bytes())?;
    file.flush()?;
    drop(file);
    prune(dir, keep);
    Ok(path)
}

/// `text`, or its start with a note when it is longer than [`MAX_FILE_BYTES`].
fn bounded(text: &str) -> String {
    if text.len() <= MAX_FILE_BYTES {
        return text.to_string();
    }
    let mut end = MAX_FILE_BYTES - CUT_MARK.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CUT_MARK}", &text[..end])
}

/// The report files in `dir`, newest first.
fn report_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(FILE_PREFIX) && name.ends_with(FILE_SUFFIX)
        })
        .map(|e| e.path())
        .collect();
    files.sort();
    files.reverse();
    files
}

fn prune(dir: &Path, keep: usize) {
    for old in report_files(dir).into_iter().skip(keep) {
        let _ = fs::remove_file(old);
    }
}

fn newest_report(dir: &Path) -> Option<String> {
    let path = report_files(dir).into_iter().next()?;
    let bytes = fs::read(path).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > MAX_FILE_BYTES {
        text = bounded(&text);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looks like a session token: three dotted parts of base64 letters and digits.
    const TOKEN: &str = "MTE5NzQ4.Gx7yQp.q8Wn3LzR0aVbT5kYjHcUe9XdFo2iS1m";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("inlet-crash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn raw<'a>(message: &'a str, formatted: bool, backtrace: &'a str) -> Raw<'a> {
        Raw {
            thread: "worker",
            location: Some("src/backend/messages/send.rs:41:9".to_string()),
            message,
            formatted,
            backtrace,
        }
    }

    #[test]
    fn crash_report_never_contains_a_token() {
        // The token is in the message (plain and in a URL query), in a header-like
        // pair, in the location and in a path of the backtrace.
        let message = format!(
            "request failed: https://chat.example/api/v1/channels/1?token={TOKEN} \
             Authorization: Bearer {TOKEN} token={TOKEN} raw {TOKEN} end"
        );
        let backtrace = format!(
            "   0: inlet::backend::send\n             at C:\\Users\\alice\\AppData\\Local\\{TOKEN}\\send.rs:12\n   \
             1: core::result::unwrap_failed\n             at /home/bob/.cargo/registry/src/{TOKEN}/core.rs:9\n"
        );
        for formatted in [false, true] {
            let mut input = raw(&message, formatted, &backtrace);
            let location = format!("/home/alice/{TOKEN}/src/main.rs:1:1");
            input.location = Some(location);
            let report = render(&input, Some("/home/alice"));
            for part in TOKEN.split('.') {
                assert!(
                    !report.contains(part),
                    "token part {part} leaked:\n{report}"
                );
            }
            assert!(!report.contains(TOKEN));
            assert!(!report.contains("alice"), "user name leaked:\n{report}");
            assert!(!report.contains("bob"), "user name leaked:\n{report}");
            assert!(!report.contains("channels/1"), "URL path leaked:\n{report}");
            assert!(report.contains("https://chat.example"));
            assert!(report.contains(env!("CARGO_PKG_VERSION")));
            assert!(report.contains(std::env::consts::OS));
            assert!(report.contains("thread: worker"));
        }
    }

    #[test]
    fn crash_report_drops_urls_ids_and_quoted_text_of_formatted_messages() {
        let message = "called `Result::unwrap()` on an `Err` value: Send { channel: 1234567890123456789, body: \"hello Bea, my password is hunter2\" }";
        let report = render(&raw(message, true, ""), None);
        assert!(!report.contains("hunter2"), "{report}");
        assert!(!report.contains("hello Bea"), "{report}");
        assert!(!report.contains("1234567890123456789"), "{report}");
        assert!(report.contains("[id]"));

        // A literal message is kept as written.
        let report = render(&raw("the sidebar has no channel", false, ""), None);
        assert!(report.contains("message: the sidebar has no channel"));
    }

    #[test]
    fn redaction_keeps_ordinary_backtrace_lines() {
        let frame = "   3: inlet_protocol::token::Token::expose_secret\n             at crates/inlet-protocol/src/token.rs:26:9";
        assert_eq!(redact(frame, None), frame);
    }

    #[test]
    fn crash_files_are_capped() {
        let dir = temp_dir("capped");
        let mut written = Vec::new();
        for i in 0..(KEEP + 4) {
            written.push(write_report(&dir, &format!("report {i}"), KEEP).unwrap());
        }
        // Something that is not ours must survive the clean-up.
        fs::write(dir.join("notes.txt"), "keep me").unwrap();
        write_report(&dir, "one more", KEEP).unwrap();
        let left = report_files(&dir);
        assert_eq!(left.len(), KEEP, "{left:?}");
        assert!(dir.join("notes.txt").is_file());
        // The oldest are gone, the newest stay.
        assert!(!written[0].exists());
        assert!(written.last().unwrap().exists());

        // A huge report is cut to the size limit, on a character boundary.
        let huge = "é".repeat(MAX_FILE_BYTES);
        let path = write_report(&dir, &huge, KEEP).unwrap();
        let size = fs::metadata(&path).unwrap().len() as usize;
        assert!(size <= MAX_FILE_BYTES, "{size}");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with(CUT_MARK));
        assert_eq!(report_files(&dir).len(), KEEP);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_newest_report_is_the_one_diagnostics_reads() {
        let dir = temp_dir("newest");
        write_report(&dir, "first", KEEP).unwrap();
        write_report(&dir, "second", KEEP).unwrap();
        assert_eq!(newest_report(&dir).as_deref(), Some("second"));
        assert_eq!(newest_report(&dir.join("missing")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn diagnostics_start_with_version_and_system() {
        let text = diagnostics();
        assert!(text.starts_with(&format!("Inlet {}\n", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains(std::env::consts::OS));
    }
}
