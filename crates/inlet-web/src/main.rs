// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `inlet-web` helper program. See the `inlet_web` library for what it is and what
//! it does.

// A window program on Windows: no console window next to the web view.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    std::process::exit(inlet_web::helper::run());
}
