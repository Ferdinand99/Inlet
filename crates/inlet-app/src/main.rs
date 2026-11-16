// SPDX-License-Identifier: AGPL-3.0-or-later
//! Inlet: a native desktop client that works with Fluxer.
//!
//! The app lives in the library (`lib.rs`); this file only starts it. Two things are
//! answered before the app starts, because they must not open a window or claim the
//! single-instance lock: `--version`, which the package smoke tests use, and the hook
//! arguments the installer passes while it installs, updates or removes the app.
// Release builds on Windows are GUI apps: no console window next to Inlet.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

/// What to do instead of starting the app, for the given first command-line argument.
#[derive(Debug, PartialEq, Eq)]
enum EarlyExit {
    /// Print the version and exit.
    Version,
    /// An installer hook (`--veloapp-install`, `--veloapp-updated`, `--veloapp-obsolete`,
    /// `--veloapp-uninstall`): there is nothing to do yet, so exit with success at once.
    /// The installer waits for the hook and would otherwise start a second Inlet.
    InstallerHook,
}

fn early_exit(first_arg: Option<&str>) -> Option<EarlyExit> {
    match first_arg? {
        "--version" | "-V" => Some(EarlyExit::Version),
        arg if arg.starts_with("--veloapp-") => Some(EarlyExit::InstallerHook),
        _ => None,
    }
}

fn main() {
    match early_exit(std::env::args().nth(1).as_deref()) {
        Some(EarlyExit::Version) => println!("inlet {}", env!("CARGO_PKG_VERSION")),
        Some(EarlyExit::InstallerHook) => {}
        None => inlet::run(),
    }
}

#[cfg(test)]
mod tests {
    use super::{early_exit, EarlyExit};

    #[test]
    fn version_flags_print_the_version() {
        assert_eq!(early_exit(Some("--version")), Some(EarlyExit::Version));
        assert_eq!(early_exit(Some("-V")), Some(EarlyExit::Version));
    }

    #[test]
    fn installer_hooks_exit_at_once() {
        for hook in [
            "--veloapp-install",
            "--veloapp-updated",
            "--veloapp-obsolete",
            "--veloapp-uninstall",
        ] {
            assert_eq!(early_exit(Some(hook)), Some(EarlyExit::InstallerHook));
        }
    }

    #[test]
    fn everything_else_starts_the_app() {
        assert_eq!(early_exit(None), None);
        assert_eq!(early_exit(Some("inlet://invite/abc")), None);
        assert_eq!(early_exit(Some("--url")), None);
        assert_eq!(early_exit(Some("--versions")), None);
    }
}
