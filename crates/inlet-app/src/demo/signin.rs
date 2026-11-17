// SPDX-License-Identifier: AGPL-3.0-or-later
//! The steps of the sign-in screen the demo scenes can show (`sign-in-<step>`). The
//! screen is opened to add an account, and `ui/accounts/demo_steps.rs` puts it on the
//! step. Nothing here talks to an instance.

/// A step to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Identifier-first: the email address.
    Identifier,
    /// The classic form: email and password together.
    Classic,
    /// Identifier-first: the password step, after a wrong password.
    Password,
    /// The emailed code.
    Code,
    /// The authenticator or backup code.
    Mfa,
    /// Waiting for the new-IP email link.
    Ip,
}

impl Step {
    pub fn from_name(name: &str) -> Option<Step> {
        Some(match name {
            "sign-in-identifier" => Step::Identifier,
            "sign-in-classic" => Step::Classic,
            "sign-in-password" => Step::Password,
            "sign-in-code" => Step::Code,
            "sign-in-mfa" => Step::Mfa,
            "sign-in-ip" => Step::Ip,
            _ => return None,
        })
    }
}

/// The account's email the steps show.
pub const IDENTIFIER: &str = "ana@example.org";
/// The instance the steps are on.
pub const ORIGIN: &str = "https://chat.example";
/// How the instance is named on screen.
pub const HOST: &str = "chat.example";
