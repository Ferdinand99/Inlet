// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs Inlet's update logic against a local folder of Velopack packages (a fake release
//! feed), without the app around it. `packaging/velopack/update-test.sh` packs this
//! program as the app, installs two versions of it and drives it:
//!
//! ```text
//! inlet-update-probe version                 print the installed version
//! inlet-update-probe check <folder>          look in the folder, download a newer package
//! inlet-update-probe pending                 print the version that waits for a restart
//! inlet-update-probe apply                   hand the waiting package to the updater
//! ```
//!
//! The folder is read with Velopack's own file source, but through the same rules as the
//! real source (`inlet::update::Checked`): the same channel, package id, hash and
//! no-downgrade checks. Built only with the `update-probe` feature; never shipped.

use inlet::update::{Engine, Outcome};
use velopack::sources::FileSource;

fn main() {
    // The same first step as the app: installer hooks are handled here.
    inlet::update::startup();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("version") => Engine::with_source(FileSource::new("."))
            .map(|e| println!("version {}", e.current_version())),
        Some("check") => {
            let folder = args.get(1).cloned().unwrap_or_default();
            Engine::with_source(FileSource::new(folder)).and_then(|engine| {
                match engine.check_and_download(|release, percent| {
                    println!("progress {} {percent}", release.version)
                })? {
                    Outcome::UpToDate => println!("up-to-date {}", engine.current_version()),
                    Outcome::Ready(release) => {
                        println!("ready {}", release.version);
                        println!("notes {}", release.notes.replace('\n', " | "));
                    }
                }
                Ok(())
            })
        }
        Some("pending") => Engine::with_source(FileSource::new(".")).map(|e| match e.pending() {
            Some(release) => println!("pending {}", release.version),
            None => println!("pending none"),
        }),
        Some("apply") => {
            Engine::with_source(FileSource::new(".")).and_then(|e| e.apply(&["version".to_owned()]))
        }
        _ => Err("usage: inlet-update-probe version|check <folder>|pending|apply".to_owned()),
    };
    if let Err(err) = result {
        println!("error {err}");
        std::process::exit(1);
    }
}
