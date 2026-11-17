// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the helper does without a window: it starts, says what it is and refuses
//! arguments. Runs on every platform, so the CI of Windows and macOS shows that the
//! helper links, finds its web engine's libraries and starts.

use std::process::{Command, Stdio};

const HELPER: &str = env!("CARGO_BIN_EXE_inlet-web");

#[test]
fn it_prints_its_version() {
    let out = Command::new(HELPER)
        .arg("--version")
        .output()
        .expect("start the helper");
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("inlet-web {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn it_takes_no_arguments_that_carry_anything() {
    for args in [
        &["https://id.example/"][..],
        &["--url", "https://id.example/"],
        &["--token", "x"],
    ] {
        let out = Command::new(HELPER)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("start the helper");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            out.stdout.is_empty(),
            "nothing is written to the event stream"
        );
    }
}
