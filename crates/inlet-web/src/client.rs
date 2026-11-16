// SPDX-License-Identifier: AGPL-3.0-or-later
//! The client: starts the `inlet-web` helper, sends it one `open` request and hands back
//! its events as a stream.
//!
//! What the helper gets: the request (a URL and the rules for it), and a small set of
//! environment variables it needs to show a window ([`helper_env`]). No argument, no
//! token, no cookie and none of Inlet's other variables, so even a page that took over
//! the helper finds nothing of Inlet's in its process.
//!
//! The helper lives as long as the [`Session`]: dropping the session kills it, and it
//! exits by itself when its input closes (Inlet going away included).
//!
//! The client must run inside a tokio runtime.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::policy::{Policy, PolicyOptions};
use crate::protocol::{
    decode_event, encode_request, ErrorKind, Event, OpenRequest, ProtocolError, Request,
    RequestError, MAX_LINE_LEN,
};

/// How long the helper has to clean up and exit after its last event, before it is
/// killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// How much of the helper's standard error is kept, for the log.
const STDERR_KEPT: usize = 4096;

/// The file name of the helper next to the app.
pub const HELPER_FILE: &str = if cfg!(windows) {
    "inlet-web.exe"
} else {
    "inlet-web"
};

/// Where the helper is, and what extra to hand it.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// The helper program.
    pub helper: PathBuf,
    /// Environment variables to set in addition to [`helper_env`]'s. For tests.
    pub extra_env: Vec<(OsString, OsString)>,
}

impl ClientOptions {
    /// The helper that ships next to the running executable. In a debug build,
    /// `INLET_WEB_HELPER` names another one.
    pub fn next_to_current_exe() -> io::Result<ClientOptions> {
        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("INLET_WEB_HELPER") {
            return Ok(ClientOptions::at(PathBuf::from(path)));
        }
        let exe = std::env::current_exe()?;
        Ok(ClientOptions::at(helper_beside(&exe)))
    }

    /// A helper at a given path.
    pub fn at(helper: PathBuf) -> ClientOptions {
        ClientOptions {
            helper,
            extra_env: Vec::new(),
        }
    }
}

/// Why a session could not be started.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The request was refused before anything started.
    #[error("the request is not accepted: {0}")]
    Invalid(#[from] RequestError),
    /// There is no helper where it should be.
    #[error("the web view helper is missing: {0}")]
    HelperMissing(PathBuf),
    /// The helper could not be started.
    #[error("the web view helper could not be started: {0}")]
    Spawn(io::Error),
}

/// One open window of the helper. A stream of its [`Event`]s: it ends after the last
/// event (`Closed` or `Error`).
pub struct Session {
    events: mpsc::Receiver<Event>,
    stdin: Option<ChildStdin>,
    supervisor: Option<JoinHandle<()>>,
    pid: Option<u32>,
}

impl Session {
    /// Starts the helper and asks it to open `request`.
    pub async fn open(options: &ClientOptions, request: OpenRequest) -> Result<Session, OpenError> {
        // The helper checks the same rules; doing it here gives the caller the answer at
        // once, without a process.
        Policy::for_request(&request, PolicyOptions::of_this_build())?;
        let line = encode_request(&Request::Open(request));
        if line.len() > MAX_LINE_LEN {
            return Err(RequestError::TooLarge.into());
        }
        if !options.helper.is_file() {
            return Err(OpenError::HelperMissing(options.helper.clone()));
        }

        let mut command = Command::new(&options.helper);
        command
            .env_clear()
            .envs(helper_env(std::env::vars_os()))
            .envs(options.extra_env.iter().map(|(k, v)| (k, v)))
            // Not Inlet's folder, which says nothing about where the helper may write.
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = command.spawn().map_err(OpenError::Spawn)?;
        let pid = child.id();

        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(OpenError::Spawn(io::Error::other("no pipes to the helper")));
        };
        if let Err(err) = write_line(&mut stdin, &line).await {
            let _ = child.kill().await;
            return Err(OpenError::Spawn(err));
        }

        let (tx, rx) = mpsc::channel(16);
        let supervisor = tokio::spawn(supervise(child, stdout, stderr, tx));
        Ok(Session {
            events: rx,
            stdin: Some(stdin),
            supervisor: Some(supervisor),
            pid,
        })
    }

    /// The next event; `None` after the last one.
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Asks the helper to close its window. The `closed` event follows.
    pub async fn close(&mut self) {
        if let Some(stdin) = self.stdin.as_mut() {
            let line = encode_request(&Request::Close);
            if write_line(stdin, &line).await.is_err() {
                self.stdin = None;
            }
        }
    }

    /// Waits until the helper has exited (or has been killed). Call it after the stream
    /// has ended.
    pub async fn finished(&mut self) {
        self.stdin = None;
        if let Some(task) = self.supervisor.take() {
            let _ = task.await;
        }
    }

    /// The helper's process id, for the tests.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl Stream for Session {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.events.poll_recv(cx)
    }
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await
}

/// What one bounded read of a line gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A whole line, without its newline.
    Text(String),
    /// A line longer than the limit.
    TooLong,
    /// The end of the input (a half line before it is dropped).
    End,
    /// Bytes that are not UTF-8.
    NotText,
}

/// Reads one line of at most `max` bytes, without ever holding more than `max + 2`.
pub async fn read_line<R>(reader: &mut R, max: usize) -> io::Result<Line>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut buf = Vec::new();
    let read = (&mut *reader)
        .take(max as u64 + 2)
        .read_until(b'\n', &mut buf)
        .await?;
    if read == 0 {
        return Ok(Line::End);
    }
    if buf.last() != Some(&b'\n') {
        return Ok(if buf.len() > max {
            Line::TooLong
        } else {
            Line::End
        });
    }
    buf.pop();
    if buf.len() > max {
        return Ok(Line::TooLong);
    }
    Ok(match String::from_utf8(buf) {
        Ok(text) => Line::Text(text),
        Err(_) => Line::NotText,
    })
}

/// Forwards the helper's events until the last one, then lets it exit.
async fn supervise(
    mut child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    tx: mpsc::Sender<Event>,
) {
    let stderr_tail = tokio::spawn(keep_tail(stderr));
    let mut reader = BufReader::new(stdout);
    let mut abnormal = false;
    let mut kill_now = false;
    let mut tx = Some(tx);
    while let Some(sender) = tx.as_ref() {
        let line = tokio::select! {
            _ = sender.closed() => {
                // The session was dropped: nobody listens, the window must go.
                kill_now = true;
                break;
            }
            line = read_line(&mut reader, MAX_LINE_LEN) => line,
        };
        let event = match line {
            Ok(Line::Text(text)) => match decode_event(&text) {
                Ok(event) => event,
                Err(err) => {
                    log::warn!("inlet-web wrote something that is not the protocol: {err}");
                    abnormal = true;
                    Event::Error {
                        kind: match err {
                            ProtocolError::UnsupportedVersion(_) => ErrorKind::UnsupportedVersion,
                            _ => ErrorKind::Protocol,
                        },
                    }
                }
            },
            Ok(Line::TooLong | Line::NotText) => {
                abnormal = true;
                Event::Error {
                    kind: ErrorKind::Protocol,
                }
            }
            Ok(Line::End) | Err(_) => {
                abnormal = true;
                Event::Error {
                    kind: ErrorKind::HelperExited,
                }
            }
        };
        let last = event.is_last();
        if abnormal {
            // Whatever it says from here on is not to be trusted.
            kill_now = true;
        }
        if sender.send(event).await.is_err() {
            kill_now = true;
            break;
        }
        if last || abnormal {
            // End the stream now; the cleanup below takes a moment.
            tx = None;
        }
    }
    drop(tx);

    if kill_now {
        let _ = child.kill().await;
    }
    let status = match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
        Ok(status) => status.ok(),
        Err(_) => {
            let _ = child.kill().await;
            child.wait().await.ok()
        }
    };
    if abnormal || status.is_none_or(|s| !s.success()) {
        let tail = tokio::time::timeout(Duration::from_millis(500), stderr_tail)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        log::warn!(
            "inlet-web ended with {}{}",
            status.map_or_else(|| "no status".to_string(), |s| s.to_string()),
            if tail.is_empty() {
                String::new()
            } else {
                format!("; it said: {}", tail.trim())
            }
        );
    }
}

/// Keeps the first bytes of the helper's standard error and drains the rest, so the
/// helper never blocks on a full pipe.
async fn keep_tail(mut stderr: ChildStderr) -> String {
    let mut kept = Vec::new();
    let _ = (&mut stderr)
        .take(STDERR_KEPT as u64)
        .read_to_end(&mut kept)
        .await;
    let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
    String::from_utf8_lossy(&kept).into_owned()
}

/// The environment variables the helper may have. Everything else of Inlet's environment
/// stays behind: a variable can hold a token, a proxy password or a path to a secret.
///
/// What is passed is what a window needs: the display, the session bus, the locale, the
/// temporary folder and the home folder (the web engine's own settings), and on Windows
/// the folders the system's web view looks up.
pub fn helper_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    vars.into_iter()
        .filter(|(name, _)| env_name_is_allowed(name))
        .collect()
}

/// Names passed as they are.
const ENV_NAMES: &[&str] = &[
    // Everywhere
    "PATH",
    "HOME",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    // Linux
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "LD_LIBRARY_PATH",
    // macOS
    "__CF_USER_TEXT_ENCODING",
    // Windows
    "SystemRoot",
    "SystemDrive",
    "windir",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    "OS",
];

/// Prefixes passed as they are: the locale, the desktop session, GTK and WebKit's
/// own switches, and software rendering.
const ENV_PREFIXES: &[&str] = &[
    "LC_", "XDG_", "GDK_", "GTK_", "GIO_", "WEBKIT_", "LIBGL_", "MESA_",
];

fn env_name_is_allowed(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    // Windows names are not case sensitive.
    let same = |a: &str, b: &str| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    ENV_NAMES.iter().any(|allowed| same(name, allowed))
        || ENV_PREFIXES.iter().any(|prefix| {
            name.len() >= prefix.len()
                && name.is_char_boundary(prefix.len())
                && same(&name[..prefix.len()], prefix)
        })
}

/// Where the helper is when the app is at `exe`: next to it.
pub fn helper_beside(exe: &Path) -> PathBuf {
    exe.with_file_name(HELPER_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    fn passed(names: &[&str]) -> Vec<String> {
        helper_env(names.iter().map(|n| (os(n), os("value"))))
            .into_iter()
            .map(|(n, _)| n.into_string().unwrap())
            .collect()
    }

    #[test]
    fn secrets_and_unknown_variables_stay_behind() {
        let kept = passed(&[
            "INLET_TOKEN",
            "INLET_DEMO",
            "FLUXER_TOKEN",
            "TOKEN",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "SSH_AUTH_SOCK",
            "HTTPS_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "KEYRING_BACKEND",
            "GNOME_KEYRING_CONTROL",
            "CARGO_REGISTRY_TOKEN",
            "RUST_LOG",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "WEBVIEW2_USER_DATA_FOLDER",
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            "COMSPEC",
            "USERNAME",
            "USER",
            "LOGNAME",
            "HOSTNAME",
        ]);
        assert_eq!(kept, Vec::<String>::new());
    }

    #[test]
    fn what_a_window_needs_is_passed() {
        let kept = passed(&[
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XAUTHORITY",
            "XDG_RUNTIME_DIR",
            "XDG_SESSION_TYPE",
            "DBUS_SESSION_BUS_ADDRESS",
            "HOME",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TMPDIR",
            "GDK_BACKEND",
            "WEBKIT_DISABLE_COMPOSITING_MODE",
            "PATH",
        ]);
        assert_eq!(kept.len(), 14, "{kept:?}");
    }

    #[test]
    fn a_name_that_only_starts_like_an_allowed_prefix_is_not_enough() {
        // `LC` and `XDG` are not the prefixes `LC_` and `XDG_`.
        assert_eq!(
            passed(&["LCX_TOKEN", "XDGTOKEN", "GDK", "WEBKIT"]),
            Vec::<String>::new()
        );
        // And a name that merely contains an allowed name does not pass either.
        assert_eq!(
            passed(&["MY_HOME", "HOME_TOKEN", "PATHS"]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_environment_name_that_is_not_text_is_dropped() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let name = OsString::from_vec(vec![b'H', b'O', b'M', b'E', 0xff]);
            assert!(helper_env([(name, os("x"))]).is_empty());
        }
    }

    #[test]
    fn the_command_the_client_builds_starts_from_an_empty_environment() {
        // `open` clears the environment before it adds the filtered variables, and passes
        // the helper no argument; a change that drops either fails here.
        let source = include_str!("client.rs");
        let start = source.find("pub async fn open").unwrap();
        let end = source.find("pub async fn next_event").unwrap();
        let open = &source[start..end];
        let clear = open
            .find(".env_clear()")
            .expect("open clears the environment");
        let add = open
            .find(".envs(helper_env(")
            .expect("open adds the filtered variables");
        assert!(clear < add);
        assert!(
            !open.contains(".arg(") && !open.contains(".args("),
            "the helper takes no arguments"
        );
    }

    async fn feed(bytes: &[u8], max: usize) -> Vec<Line> {
        let (mut writer, reader) = tokio::io::duplex(1 << 20);
        writer.write_all(bytes).await.unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        let mut out = Vec::new();
        loop {
            let line = read_line(&mut reader, max).await.unwrap();
            let end = matches!(line, Line::End | Line::TooLong | Line::NotText);
            out.push(line);
            if end {
                return out;
            }
        }
    }

    #[tokio::test]
    async fn lines_are_split_at_newlines() {
        let lines = feed(b"one\ntwo\r\n\nthree\n", 100).await;
        assert_eq!(
            lines,
            vec![
                Line::Text("one".into()),
                Line::Text("two\r".into()),
                Line::Text(String::new()),
                Line::Text("three".into()),
                Line::End,
            ]
        );
    }

    #[tokio::test]
    async fn a_half_line_at_the_end_is_dropped() {
        assert_eq!(
            feed(b"one\ntwo", 100).await,
            vec![Line::Text("one".into()), Line::End]
        );
    }

    #[tokio::test]
    async fn a_long_line_stops_the_reading() {
        let mut long = vec![b'a'; 200];
        long.push(b'\n');
        assert_eq!(feed(&long, 100).await, vec![Line::TooLong]);
        // Without a newline, and far longer than the limit.
        assert_eq!(feed(&vec![b'a'; 10_000], 100).await, vec![Line::TooLong]);
        // Exactly the limit is fine.
        let mut exact = vec![b'a'; 100];
        exact.push(b'\n');
        assert_eq!(
            feed(&exact, 100).await,
            vec![Line::Text("a".repeat(100)), Line::End]
        );
    }

    #[tokio::test]
    async fn bytes_that_are_not_text_are_refused() {
        assert_eq!(feed(b"\xff\xfe\n", 100).await, vec![Line::NotText]);
    }

    #[tokio::test]
    async fn a_missing_helper_is_reported_before_anything_starts() {
        let request = OpenRequest {
            url: "https://id.example/".into(),
            allow: vec!["id.example".into()],
            intercept: vec![],
            title: "t".into(),
            size: crate::protocol::Size {
                width: 400,
                height: 400,
            },
            parent: None,
        };
        let options = ClientOptions::at(PathBuf::from("/does/not/exist/inlet-web"));
        let err = Session::open(&options, request.clone())
            .await
            .err()
            .unwrap();
        assert!(matches!(err, OpenError::HelperMissing(_)), "{err}");

        // A request the rules refuse never reaches the helper, present or not.
        let mut bad = request;
        bad.url = "javascript:alert(1)".into();
        let err = Session::open(&options, bad).await.err().unwrap();
        assert!(
            matches!(err, OpenError::Invalid(RequestError::BadUrl)),
            "{err}"
        );
    }

    #[test]
    fn the_helper_is_looked_for_beside_the_app() {
        let beside = helper_beside(Path::new("/opt/inlet/inlet"));
        assert_eq!(beside.file_name().unwrap(), HELPER_FILE);
        assert_eq!(beside.parent().unwrap(), Path::new("/opt/inlet"));
    }
}
