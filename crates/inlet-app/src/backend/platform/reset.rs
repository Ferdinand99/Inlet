// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Reset app data and restart", as in Fluxer's Troubleshooting: Inlet forgets every
//! stored session and every file it keeps for the person (settings, the account list,
//! custom sounds, the picture cache) and starts again as if it had just been
//! installed. The accounts, communities and messages on the Fluxer instance are not
//! touched.
//!
//! The wiping is done by the *new* process, at its start and before it reads anything:
//! the old one only starts it (`--reset-app-data`, after `--relaunch-after <pid>`) and
//! quits, without writing its settings on the way out. So nothing written by the old
//! process during its shutdown can come back.
//!
//! What stays: the crash log (it holds no content, and is what to send when something
//! broke), the operating system's own login item (it only starts Inlet) and the
//! `inlet://` handler.

use std::path::{Path, PathBuf};

use crate::token_store::{self, AccountList};

/// The argument that makes the new process wipe first.
pub const FLAG: &str = "--reset-app-data";

/// Whether this start is a reset.
pub fn requested(args: &[String]) -> bool {
    args.iter().any(|a| a == FLAG)
}

/// The folders Inlet keeps data in.
#[derive(Clone, Debug, Default)]
pub struct Places {
    /// The settings, the account list, custom sounds.
    pub config: Option<PathBuf>,
    /// The picture cache.
    pub media_cache: Option<PathBuf>,
}

impl Places {
    pub fn of_this_system() -> Self {
        Places {
            config: dirs::config_dir().map(|d| d.join("inlet")),
            media_cache: dirs::cache_dir().map(|d| d.join("inlet").join("media")),
        }
    }
}

/// What the wipe did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Sessions removed from the keychain.
    pub sessions: usize,
    /// Sessions that could not be removed.
    pub sessions_left: usize,
    /// Folders removed.
    pub folders: usize,
    /// Folders that could not be removed.
    pub folders_left: usize,
}

/// The keychain entries of the signed-in accounts: those the list names, and the one
/// an older Inlet kept under the instance's address.
fn entries(config: &Path) -> Vec<String> {
    let list = token_store::load_list(&config.join("accounts.json")).unwrap_or_else(|bad| {
        log::warn!(
            "the account list could not be read for the reset: {}",
            bad.error
        );
        AccountList::default()
    });
    let mut out: Vec<String> = list.accounts.iter().map(|a| a.entry()).collect();
    out.extend(list.legacy_pending);
    out
}

/// Removes the sessions of the listed accounts (through `forget`, which gets an entry
/// name) and the folders. A missing folder is fine.
pub fn wipe(places: &Places, mut forget: impl FnMut(&str) -> anyhow::Result<()>) -> Report {
    let mut report = Report::default();
    if let Some(config) = &places.config {
        for entry in entries(config) {
            match forget(&entry) {
                Ok(()) => report.sessions += 1,
                Err(err) => {
                    log::warn!("a stored session could not be removed: {err:#}");
                    report.sessions_left += 1;
                }
            }
        }
    }
    for folder in [&places.config, &places.media_cache].into_iter().flatten() {
        match std::fs::remove_dir_all(folder) {
            Ok(()) => report.folders += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!("{} could not be removed: {err}", folder.display());
                report.folders_left += 1;
            }
        }
    }
    report
}

/// Does the reset on this system, through the keychain. Call before anything reads the
/// settings.
pub fn run() -> Report {
    use crate::token_store::SecretStore;
    let report = wipe(&Places::of_this_system(), |entry| {
        token_store::Keychain.delete(entry)
    });
    log::info!("app data reset: {report:?}");
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("inlet-reset-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_flag_is_only_what_the_relaunch_passes() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(requested(&args(&[
            "--relaunch-after",
            "12",
            "--reset-app-data"
        ])));
        assert!(!requested(&args(&["--relaunch-after", "12"])));
        assert!(!requested(&args(&["inlet://invite/abc"])));
    }

    #[test]
    fn a_reset_removes_the_sessions_the_files_and_the_cache_and_nothing_else() {
        let root = temp("all");
        let config = root.join("config").join("inlet");
        let media = root.join("cache").join("inlet").join("media");
        let crash = root.join("cache").join("inlet").join("crash");
        for dir in [&config, &media, &crash, &config.join("sounds")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(config.join("settings.json"), "{}").unwrap();
        std::fs::write(config.join("sounds").join("mute.wav"), b"x").unwrap();
        std::fs::write(media.join("a.bin"), b"x").unwrap();
        std::fs::write(crash.join("crash.log"), b"x").unwrap();
        let list = token_store::AccountList {
            accounts: vec![
                token_store::AccountRecord {
                    origin: "https://fluxer.test".into(),
                    user_id: "1".into(),
                    name: "Ada".into(),
                    handle: "ada".into(),
                },
                token_store::AccountRecord {
                    origin: "https://other.test".into(),
                    user_id: "2".into(),
                    name: String::new(),
                    handle: String::new(),
                },
            ],
            active: None,
            legacy_pending: Some("https://old.test".into()),
        };
        token_store::save_list(&config.join("accounts.json"), &list).unwrap();

        let mut forgotten = Vec::new();
        let report = wipe(
            &Places {
                config: Some(config.clone()),
                media_cache: Some(media.clone()),
            },
            |entry| {
                forgotten.push(entry.to_owned());
                Ok(())
            },
        );
        assert_eq!(
            forgotten,
            vec![
                "https://fluxer.test#1".to_owned(),
                "https://other.test#2".to_owned(),
                "https://old.test".to_owned()
            ]
        );
        assert_eq!(
            report,
            Report {
                sessions: 3,
                sessions_left: 0,
                folders: 2,
                folders_left: 0
            }
        );
        assert!(!config.exists() && !media.exists());
        // The crash log is not Inlet's data to wipe.
        assert!(crash.join("crash.log").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failing_keychain_does_not_stop_the_files_from_going() {
        let root = temp("keychain");
        let config = root.join("inlet");
        std::fs::create_dir_all(&config).unwrap();
        let list = token_store::AccountList {
            accounts: vec![token_store::AccountRecord {
                origin: "https://fluxer.test".into(),
                user_id: "1".into(),
                name: String::new(),
                handle: String::new(),
            }],
            ..Default::default()
        };
        token_store::save_list(&config.join("accounts.json"), &list).unwrap();
        let report = wipe(
            &Places {
                config: Some(config.clone()),
                media_cache: None,
            },
            |_| anyhow::bail!("the keychain is locked"),
        );
        assert_eq!(report.sessions, 0);
        assert_eq!(report.sessions_left, 1);
        assert!(!config.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_there_is_not_an_error() {
        let root = temp("empty");
        let report = wipe(
            &Places {
                config: Some(root.join("none")),
                media_cache: Some(root.join("also-none")),
            },
            |_| Ok(()),
        );
        assert_eq!(report, Report::default());
        let _ = std::fs::remove_dir_all(&root);
    }
}
