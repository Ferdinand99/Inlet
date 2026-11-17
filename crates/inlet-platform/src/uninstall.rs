//! What the uninstaller removes that the installer does not know about (Windows).
//!
//! Velopack's uninstaller deletes the install folder, the Start menu shortcut and the
//! "Apps & features" entry. Inlet also writes three things into the user's registry while
//! it runs: the `inlet://` link handler, the login item, and the notification identity
//! (AppUserModelID) that gives toasts their name. They point at a file that is about to
//! disappear, so [`cleanup`] removes them. It runs from Velopack's uninstall hook, which
//! has 30 seconds, and it ignores what is already gone.
//!
//! There is no such hook on Linux (an AppImage is deleted by hand); the desktop entries
//! the app wrote there stay until the user removes them.

/// Removes the `inlet://` handler, the login item and the notification identity of the
/// current user. Every step is tried even when an earlier one fails.
#[cfg(windows)]
pub fn cleanup() {
    cleanup_named(crate::deep_link::SCHEME, crate::APP_ID, crate::APP_ID);
}

/// [`cleanup`] with the names given, so a test can use names of its own.
#[cfg(windows)]
fn cleanup_named(scheme: &str, run_value: &str, app_id: &str) {
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;

    let gone = |err: &windows::core::Error| err.code() == ERROR_FILE_NOT_FOUND.to_hresult();

    if let Err(err) = crate::deep_link::unregister(scheme) {
        log::warn!("uninstall: the link handler was not removed: {err}");
    }

    let user = windows_registry::CURRENT_USER;
    // The login item: the Run value, and the Task Manager switch that goes with it.
    for key in [
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
    ] {
        if let Ok(key) = user.create(key) {
            if let Err(err) = key.remove_value(run_value) {
                if !gone(&err) {
                    log::warn!("uninstall: the login item was not removed: {err}");
                }
            }
        }
    }

    if let Err(err) = user.remove_tree(format!(r"Software\Classes\AppUserModelId\{app_id}")) {
        if !gone(&err) {
            log::warn!("uninstall: the notification identity was not removed: {err}");
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::cleanup_named;

    /// Writes the three things under test names into the real `HKCU`, runs the cleanup and
    /// checks that they are gone and that a second run is harmless.
    #[test]
    #[ignore = "touches the real registry; run in CI with --include-ignored"]
    fn removes_what_the_app_wrote() {
        let id = std::process::id();
        let scheme = format!("inlet-test{id}");
        let run_value = format!("InletTest{id}");
        let app_id = format!("no.opland.inlet.test{id}");
        let user = windows_registry::CURRENT_USER;

        crate::deep_link::register(&scheme).unwrap();
        user.create(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .unwrap()
            .set_string(&run_value, "x")
            .unwrap();
        user.create(format!(r"Software\Classes\AppUserModelId\{app_id}"))
            .unwrap()
            .set_string("DisplayName", "Inlet test")
            .unwrap();

        cleanup_named(&scheme, &run_value, &app_id);
        assert!(user.open(format!(r"Software\Classes\{scheme}")).is_err());
        assert!(user
            .open(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .unwrap()
            .get_string(&run_value)
            .is_err());
        assert!(user
            .open(format!(r"Software\Classes\AppUserModelId\{app_id}"))
            .is_err());

        // Nothing left to remove: no panic, no error.
        cleanup_named(&scheme, &run_value, &app_id);
    }
}
