// SPDX-License-Identifier: AGPL-3.0-or-later
//! The helper's private data folder: where the web engine keeps what a page leaves
//! behind (cache, cookies, storage). It is new for every window and removed when the
//! window closes, so nothing outlives a sign-in. The web view also runs in its private
//! (incognito) mode, which keeps most of it in memory; the folder is for what the engine
//! writes anyway (on Windows, WebView2 always needs one).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The start of every folder name; only folders that start like this are ever removed.
pub const PREFIX: &str = "inlet-web-";

/// A folder left by a helper that was killed is removed when another one starts, once it
/// is this old.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// The folder of the window that is open, so an exit path that cannot drop it can still
/// remove it.
static REGISTERED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// A folder that is removed when this is dropped.
#[derive(Debug)]
pub struct DataDir {
    path: PathBuf,
}

impl DataDir {
    /// Makes a new, empty folder in the system's temporary folder, readable by the
    /// current user only, and removes the leftovers of helpers that did not get to clean
    /// up.
    pub fn create() -> io::Result<DataDir> {
        let base = std::env::temp_dir();
        sweep(&base, SystemTime::now(), STALE_AFTER);
        let dir = DataDir::create_in(&base)?;
        *REGISTERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.path.clone());
        Ok(dir)
    }

    /// As [`DataDir::create`], below `base`, without the sweep.
    pub fn create_in(base: &Path) -> io::Result<DataDir> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = base.join(format!("{PREFIX}{}-{nanos}", std::process::id()));
        // Fails when the name exists: a folder somebody else made is never reused.
        make_private_dir(&path)?;
        Ok(DataDir { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        remove_with_retries(&self.path);
        let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
        if registered.as_deref() == Some(self.path.as_path()) {
            *registered = None;
        }
    }
}

/// Removes the folder of the open window, if there is one. For an exit that skips the
/// drop.
pub fn remove_registered() {
    let path = REGISTERED.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(path) = path {
        remove_with_retries(&path);
    }
}

#[cfg(unix)]
fn make_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn make_private_dir(path: &Path) -> io::Result<()> {
    // A new folder in the user's own temporary folder, which only the user can read.
    std::fs::create_dir(path)
}

/// The engine's processes may still hold a file for a moment after the window closed, and
/// may write one more after the first removal (a network process that outlives the window
/// by a few milliseconds). So the folder is removed until it stays gone for a short while.
fn remove_with_retries(path: &Path) {
    let mut quiet = 0;
    for _ in 0..40 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => quiet = 0,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                quiet = 0;
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
        if path.exists() {
            quiet = 0;
            continue;
        }
        quiet += 1;
        if quiet >= 3 {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Removes the folders in `base` that a helper made and that are older than `older_than`
/// at `now`. Looks at nothing but real folders whose name starts with [`PREFIX`].
pub fn sweep(base: &Path, now: SystemTime, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        // `symlink_metadata`: a link is never followed, so a planted link cannot make
        // the sweep delete something elsewhere.
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= older_than);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder of the test's own, so tests never touch the real temp folder's
    /// other contents.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir().join(format!(
                "inlet-web-test-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_data_folder_is_new_and_goes_when_dropped() {
        let scratch = Scratch::new("drop");
        let dir = DataDir::create_in(&scratch.0).unwrap();
        let path = dir.path().to_path_buf();
        assert!(path.is_dir());
        assert!(path.starts_with(&scratch.0));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(PREFIX));
        std::fs::write(path.join("cache"), b"page data").unwrap();
        std::fs::create_dir(path.join("sub")).unwrap();
        std::fs::write(path.join("sub").join("cookies"), b"c").unwrap();
        drop(dir);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_folder_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("mode");
        let dir = DataDir::create_in(&scratch.0).unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }

    #[test]
    fn two_folders_never_share_a_name() {
        let scratch = Scratch::new("two");
        let a = DataDir::create_in(&scratch.0).unwrap();
        let b = DataDir::create_in(&scratch.0).unwrap();
        assert_ne!(a.path(), b.path());
    }

    #[test]
    fn a_folder_that_exists_is_not_taken_over() {
        let scratch = Scratch::new("exists");
        let first = DataDir::create_in(&scratch.0).unwrap();
        // An existing name is an error, never a reuse.
        assert!(make_private_dir(first.path()).is_err());
    }

    #[test]
    fn the_sweep_removes_only_old_helper_folders() {
        let scratch = Scratch::new("sweep");
        let old_dir = scratch.0.join(format!("{PREFIX}1-1"));
        let other = scratch.0.join("not-ours");
        let file = scratch.0.join(format!("{PREFIX}file"));
        std::fs::create_dir(&old_dir).unwrap();
        std::fs::write(old_dir.join("x"), b"x").unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::write(&file, b"x").unwrap();

        // Nothing is old yet.
        sweep(&scratch.0, SystemTime::now(), Duration::from_secs(3600));
        assert!(old_dir.exists() && other.exists() && file.exists());

        // A day later the helper folder goes; the other folder and the plain file stay.
        sweep(
            &scratch.0,
            SystemTime::now() + Duration::from_secs(2 * 3600),
            Duration::from_secs(3600),
        );
        assert!(!old_dir.exists());
        assert!(other.exists());
        assert!(file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_does_not_follow_a_link() {
        let scratch = Scratch::new("link");
        let target = scratch.0.join("precious");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"k").unwrap();
        let link = scratch.0.join(format!("{PREFIX}link"));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sweep(
            &scratch.0,
            SystemTime::now() + Duration::from_secs(48 * 3600),
            Duration::from_secs(3600),
        );
        assert!(target.join("keep").exists());
    }
}
