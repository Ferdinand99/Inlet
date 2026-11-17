// SPDX-License-Identifier: AGPL-3.0-or-later
//! Inlet: a native desktop client that works with Fluxer.
//!
//! The app lives in the library (`lib.rs`); this file only starts it. Two things are
//! answered before the app starts, because they must not open a window or claim the
//! single-instance lock: `--version`, which the package smoke tests use, and the hook
//! arguments the installer passes while it installs, updates or removes the app (handled
//! by `inlet::update::startup`, which exits after a hook).
// Release builds on Windows are GUI apps: no console window next to Inlet.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::ffi::OsStr;

/// What to do instead of starting the app, for the given first command-line argument.
#[derive(Debug, PartialEq, Eq)]
enum EarlyExit {
    /// Print the version and exit.
    Version,
}

/// Only these exact names count. Any other argument starts the app as usual, and so does
/// an argument that is not valid Unicode (it cannot be one of them). The installer's
/// `--veloapp-*` hook arguments are not here: the Velopack call in `main` handles them.
fn early_exit(first_arg: Option<&OsStr>) -> Option<EarlyExit> {
    match first_arg?.to_str()? {
        "--version" | "-V" => Some(EarlyExit::Version),
        _ => None,
    }
}

/// Prints one line to standard output.
///
/// The Windows release build is a GUI program, which has no console of its own. When it
/// is started from a terminal without redirected output it has no standard output at
/// all, so the line goes to the terminal that started it (the parent's console). When
/// the output is redirected to a pipe or a file, the redirection is used as usual. A
/// closed pipe is not an error worth a panic.
#[cfg(windows)]
fn print_line(line: &str) {
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;

    let handle = std::io::stdout().as_raw_handle();
    if handle.is_null() || handle as isize == -1 {
        // SAFETY: a plain Win32 call that takes no pointers.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
            if let Ok(mut console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                let _ = writeln!(console, "{line}");
            }
        }
        return;
    }
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// Prints one line to standard output; a closed pipe is not an error worth a panic.
#[cfg(not(windows))]
fn print_line(line: &str) {
    use std::io::Write;

    let _ = writeln!(std::io::stdout(), "{line}");
}

fn main() {
    // First, so that a panic anywhere after this (the installer hooks included) is recorded.
    inlet::crash::install();
    if let Some(EarlyExit::Version) = early_exit(std::env::args_os().nth(1).as_deref()) {
        print_line(&format!("inlet {}", env!("CARGO_PKG_VERSION")));
        return;
    }
    // The installer's hooks (install, update, uninstall) run the app with `--veloapp-*`
    // and wait for it to exit: this call handles them and exits, before any window.
    inlet::update::startup();
    inlet::run();
}

#[cfg(test)]
mod tests {
    use super::{early_exit, EarlyExit};
    use std::ffi::OsStr;

    fn exit_for(arg: &str) -> Option<EarlyExit> {
        early_exit(Some(OsStr::new(arg)))
    }

    #[test]
    fn version_flags_print_the_version() {
        assert_eq!(exit_for("--version"), Some(EarlyExit::Version));
        assert_eq!(exit_for("-V"), Some(EarlyExit::Version));
    }

    #[test]
    fn installer_hooks_are_left_to_velopack() {
        for hook in [
            "--veloapp-install",
            "--veloapp-updated",
            "--veloapp-obsolete",
            "--veloapp-uninstall",
        ] {
            assert_eq!(exit_for(hook), None);
        }
    }

    #[test]
    fn everything_else_starts_the_app() {
        assert_eq!(early_exit(None), None);
        assert_eq!(exit_for("inlet://invite/abc"), None);
        assert_eq!(exit_for("--url"), None);
        assert_eq!(exit_for("--versions"), None);
        assert_eq!(exit_for(""), None);
        // Other Velopack arguments are not hooks: the app starts normally.
        assert_eq!(exit_for("--veloapp-firstrun"), None);
        assert_eq!(exit_for("--veloapp-"), None);
        assert_eq!(exit_for("--veloapp-install-now"), None);
    }

    #[cfg(unix)]
    #[test]
    fn an_argument_that_is_not_unicode_starts_the_app() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(early_exit(Some(OsStr::from_bytes(b"--version\xff"))), None);
        assert_eq!(early_exit(Some(OsStr::from_bytes(b"\xff"))), None);
    }
}
