//! Whether the taskbar is dark, for the colour of the monochrome tray icon.
//!
//! Windows keeps it in the user's registry (`SystemUsesLightTheme`, 0 for a dark
//! taskbar). Other systems do not say; the caller then picks its own default.

/// `Some(true)` for a dark taskbar, `Some(false)` for a light one, `None` where the
/// system does not say. A registry read; cheap.
pub fn taskbar_is_dark() -> Option<bool> {
    imp::dark()
}

#[cfg(windows)]
mod imp {
    const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

    pub(super) fn dark() -> Option<bool> {
        let key = windows_registry::CURRENT_USER.open(PERSONALIZE).ok()?;
        let light = key.get_u32("SystemUsesLightTheme").ok()?;
        Some(light == 0)
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn dark() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_answer_is_a_yes_a_no_or_nothing_and_never_a_panic() {
        let _ = super::taskbar_is_dark();
        #[cfg(not(windows))]
        assert_eq!(super::taskbar_is_dark(), None);
    }
}
