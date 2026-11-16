//! The path other programs use to start this Inlet again: the login item and the
//! `inlet://` link handler.
//!
//! Usually that is the running executable. Inside an AppImage it is not: the running
//! executable lives in a temporary mount that disappears when Inlet exits, so a login
//! item or link handler pointing there would be dead. The AppImage runtime sets
//! `APPIMAGE` to the path of the `.AppImage` file itself; that is what to register.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::Result;

/// The executable to register for a login item or a link handler.
pub fn launch_exe() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    Ok(choose(std::env::var_os("APPIMAGE"), current))
}

/// `appimage` is the value of `APPIMAGE`, if set. It counts on Linux only, and only
/// when it names an existing file by an absolute path; anything else falls back to the
/// running executable.
fn choose(appimage: Option<OsString>, current: PathBuf) -> PathBuf {
    if cfg!(target_os = "linux") {
        if let Some(path) = appimage.map(PathBuf::from) {
            if path.is_absolute() && path.is_file() {
                return path;
            }
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::choose;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn current() -> PathBuf {
        PathBuf::from("/tmp/.mount_abc/usr/bin/inlet")
    }

    #[test]
    fn without_appimage_the_running_executable_is_used() {
        assert_eq!(choose(None, current()), current());
    }

    #[test]
    fn a_missing_or_relative_appimage_path_is_ignored() {
        let missing = OsString::from("/nonexistent/dir/Inlet.AppImage");
        assert_eq!(choose(Some(missing), current()), current());
        let relative = OsString::from("Cargo.toml");
        assert_eq!(choose(Some(relative), current()), current());
        assert_eq!(choose(Some(OsString::new()), current()), current());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_existing_appimage_wins_over_the_mount_path() {
        let dir = std::env::temp_dir().join(format!("inlet-launch-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("Inlet.AppImage");
        std::fs::write(&image, b"x").unwrap();
        assert_eq!(
            choose(Some(image.clone().into_os_string()), current()),
            image
        );
        // A directory is not an AppImage.
        assert_eq!(
            choose(Some(dir.clone().into_os_string()), current()),
            current()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
