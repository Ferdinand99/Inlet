// SPDX-License-Identifier: AGPL-3.0-or-later
//! The real panic hook, in a process of its own (the hook is global): a thread panics
//! with a token in its message and the report on disk must not hold it.

use std::fs;

const TOKEN: &str = "MTE5NzQ4.Gx7yQp.q8Wn3LzR0aVbT5kYjHcUe9XdFo2iS1m";

#[test]
fn a_panic_writes_one_report_without_the_token() {
    let dir = std::env::temp_dir().join(format!("inlet-crash-hook-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    inlet::crash::install_in(dir.clone());

    let result = std::thread::Builder::new()
        .name("test-worker".to_string())
        .spawn(|| {
            // A formatted message, as `unwrap` on an error makes.
            panic!(
                "send failed: https://chat.example/x?token={TOKEN} Authorization: Bearer {TOKEN}"
            );
        })
        .unwrap()
        .join();
    assert!(result.is_err(), "the thread should have panicked");

    let files: Vec<_> = fs::read_dir(&dir)
        .expect("the hook creates the folder")
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let report = fs::read_to_string(&files[0]).unwrap();
    assert!(report.starts_with("Inlet crash report\n"), "{report}");
    assert!(report.contains("thread: test-worker"), "{report}");
    assert!(report.contains("backtrace:"), "{report}");
    for part in TOKEN.split('.') {
        assert!(
            !report.contains(part),
            "token part {part} in the report:\n{report}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
