//! The macOS privacy permissions Inlet's voice and keyboard features need: microphone,
//! camera, screen recording and input monitoring (for push-to-talk). The Desktop
//! settings show where each stands and ask for it only when the person presses the
//! button; nothing here is checked or requested by itself. Other systems have no such
//! permissions: every status is [`Status::Unsupported`].

/// A permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Microphone,
    Camera,
    ScreenRecording,
    InputMonitoring,
}

impl Permission {
    pub const ALL: [Permission; 4] = [
        Permission::Microphone,
        Permission::Camera,
        Permission::ScreenRecording,
        Permission::InputMonitoring,
    ];
}

/// Where a permission stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Granted,
    /// The person said no, or the system holds it back.
    Denied,
    /// Not asked yet.
    NotDetermined,
    /// This system has no such permission.
    Unsupported,
}

/// Where the permission stands now. Cheap; no prompt.
pub fn status(permission: Permission) -> Status {
    imp::status(permission)
}

/// Asks the system for the permission, which may show its prompt, and returns where it
/// stands afterwards. Only when the person pressed the button.
pub fn request(permission: Permission) -> Status {
    imp::request(permission)
}

/// Opens the system's privacy settings at this permission.
pub fn open_settings(permission: Permission) {
    imp::open_settings(permission);
}

/// The URL of the System Settings pane of a permission.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn settings_url(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Permission::Camera => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
        }
        Permission::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        Permission::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;

    use super::{settings_url, Permission, Status};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> u32;
        fn IOHIDRequestAccess(request_type: u32) -> bool;
    }

    // Forces AVFoundation to be loaded, so the class below can be found.
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    /// `kIOHIDRequestTypeListenEvent`.
    const LISTEN_EVENT: u32 = 1;
    /// `kIOHIDAccessTypeGranted` and `kIOHIDAccessTypeDenied`.
    const HID_GRANTED: u32 = 0;
    const HID_DENIED: u32 = 1;

    /// The screen recording prompt was shown in this run: the system then answers "no"
    /// until the person has allowed it in the settings and Inlet is started again.
    static SCREEN_ASKED: AtomicBool = AtomicBool::new(false);

    fn av_status(media_type: &str) -> Status {
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
            return Status::Unsupported;
        };
        let media = NSString::from_str(media_type);
        // SAFETY: `authorizationStatusForMediaType:` takes an NSString and returns an
        // NSInteger: 0 not determined, 1 restricted, 2 denied, 3 authorized.
        let raw: isize =
            unsafe { objc2::msg_send![class, authorizationStatusForMediaType: &*media] };
        match raw {
            3 => Status::Granted,
            1 | 2 => Status::Denied,
            _ => Status::NotDetermined,
        }
    }

    pub(super) fn status(permission: Permission) -> Status {
        match permission {
            // AVMediaTypeAudio and AVMediaTypeVideo.
            Permission::Microphone => av_status("soun"),
            Permission::Camera => av_status("vide"),
            Permission::ScreenRecording => {
                // SAFETY: a plain query with no arguments.
                if unsafe { CGPreflightScreenCaptureAccess() } {
                    Status::Granted
                } else if SCREEN_ASKED.load(Ordering::Relaxed) {
                    Status::Denied
                } else {
                    Status::NotDetermined
                }
            }
            Permission::InputMonitoring => {
                // SAFETY: a plain query with a request type.
                match unsafe { IOHIDCheckAccess(LISTEN_EVENT) } {
                    HID_GRANTED => Status::Granted,
                    HID_DENIED => Status::Denied,
                    _ => Status::NotDetermined,
                }
            }
        }
    }

    pub(super) fn request(permission: Permission) -> Status {
        match permission {
            // The microphone and camera prompt comes when a call first uses them; here
            // the settings are the place to change it.
            Permission::Microphone | Permission::Camera => {
                if status(permission) != Status::Granted {
                    open_settings(permission);
                }
            }
            Permission::ScreenRecording => {
                SCREEN_ASKED.store(true, Ordering::Relaxed);
                // SAFETY: shows the system's prompt; no arguments.
                unsafe { CGRequestScreenCaptureAccess() };
            }
            Permission::InputMonitoring => {
                // SAFETY: shows the system's prompt; a request type.
                unsafe { IOHIDRequestAccess(LISTEN_EVENT) };
            }
        }
        status(permission)
    }

    pub(super) fn open_settings(permission: Permission) {
        if let Err(err) = std::process::Command::new("open")
            .arg(settings_url(permission))
            .spawn()
        {
            log::info!("could not open the privacy settings: {err}");
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Permission, Status};

    pub(super) fn status(_: Permission) -> Status {
        Status::Unsupported
    }

    pub(super) fn request(_: Permission) -> Status {
        Status::Unsupported
    }

    pub(super) fn open_settings(_: Permission) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_has_a_settings_pane() {
        for p in Permission::ALL {
            assert!(settings_url(p).starts_with("x-apple.systempreferences:"));
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_systems_have_no_permissions() {
        for p in Permission::ALL {
            assert_eq!(status(p), Status::Unsupported);
            assert_eq!(request(p), Status::Unsupported);
        }
    }

    /// Reading a status shows no prompt and never fails, on any system.
    #[test]
    fn a_status_can_always_be_read() {
        for p in Permission::ALL {
            let _ = status(p);
        }
    }
}
