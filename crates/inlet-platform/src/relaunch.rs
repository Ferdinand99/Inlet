//! Starting Inlet again (after a setting that needs a restart, or a reset), and the
//! new Inlet waiting for the old one to be gone.
//!
//! Only one Inlet runs per user (`single_instance`), so the new process cannot start
//! while the old one still holds the lock. It is started with `--relaunch-after <pid>`
//! and waits for that process to exit before it claims the lock.

use std::time::Duration;

use crate::{launch_exe, Result};

/// The flag the new process is started with.
pub const RELAUNCH_FLAG: &str = "--relaunch-after";

/// Starts this Inlet again; the new one waits for this process to exit. Call it, then
/// quit.
pub fn spawn_relaunch() -> Result<()> {
    spawn_relaunch_with(&[])
}

/// As [`spawn_relaunch`], with more arguments for the new process (after the flag that
/// names this one).
pub fn spawn_relaunch_with(extra: &[&str]) -> Result<()> {
    let exe = launch_exe()?;
    std::process::Command::new(exe)
        .arg(RELAUNCH_FLAG)
        .arg(std::process::id().to_string())
        .args(extra)
        .spawn()?;
    Ok(())
}

/// The process the old Inlet had, from the arguments of a process started by
/// [`spawn_relaunch`].
pub fn relaunch_parent(args: &[String]) -> Option<u32> {
    let at = args.iter().position(|a| a == RELAUNCH_FLAG)?;
    args.get(at + 1)?.parse().ok()
}

/// Waits (at most `timeout`) for the process the old Inlet had to be gone, when `args`
/// say this start is a relaunch. Returns whether it waited.
pub fn wait_for_relaunch_parent(args: &[String], timeout: Duration) -> bool {
    match relaunch_parent(args) {
        Some(pid) => {
            wait_for_exit(pid, timeout);
            true
        }
        None => false,
    }
}

/// Waits (at most `timeout`) until the process `pid` has exited. Gives up quietly when
/// the process cannot be looked at.
pub fn wait_for_exit(pid: u32, timeout: Duration) {
    imp::wait_for_exit(pid, timeout);
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    pub(super) fn wait_for_exit(pid: u32, timeout: Duration) {
        // SAFETY: a plain query for a handle that can be waited on; it is closed below.
        let Ok(handle) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
            // Gone already, or not ours to look at.
            return;
        };
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the handle is valid until CloseHandle.
        unsafe {
            WaitForSingleObject(handle, ms);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::time::{Duration, Instant};

    pub(super) fn wait_for_exit(pid: u32, timeout: Duration) {
        let Ok(pid) = i32::try_from(pid) else {
            return;
        };
        let end = Instant::now() + timeout;
        // Signal 0 only asks whether the process exists.
        // SAFETY: kill with signal 0 sends nothing.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use std::time::Duration;

    pub(super) fn wait_for_exit(_: u32, _: Duration) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_relaunch_names_the_process_to_wait_for() {
        assert_eq!(
            relaunch_parent(&args(&["--relaunch-after", "4242"])),
            Some(4242)
        );
        assert_eq!(
            relaunch_parent(&args(&["--autostart", "--relaunch-after", "7"])),
            Some(7)
        );
        assert_eq!(relaunch_parent(&args(&["--autostart"])), None);
        assert_eq!(relaunch_parent(&args(&["--relaunch-after"])), None);
        assert_eq!(relaunch_parent(&args(&["--relaunch-after", "x"])), None);
        assert!(!wait_for_relaunch_parent(
            &args(&["inlet://invite/abc"]),
            Duration::from_millis(10)
        ));
    }

    #[test]
    fn waiting_for_a_process_that_is_gone_returns_at_once() {
        // A child that has already exited.
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit 0"])
                .spawn()
        } else {
            std::process::Command::new("true").spawn()
        }
        .expect("start a short program");
        let pid = child.id();
        child.wait().unwrap();
        let started = std::time::Instant::now();
        wait_for_exit(pid, Duration::from_secs(20));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
