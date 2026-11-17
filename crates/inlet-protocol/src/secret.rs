//! A short-lived typed secret: a password or a one-time code.

use std::fmt;

use zeroize::Zeroize;

/// A password or a one-time code (email code, authenticator code, backup code).
///
/// Like [`crate::Token`], the value never shows up in `Debug` output and the
/// memory is wiped when the secret is dropped. It is not `Clone` and has no
/// `Display`: call [`Secret::expose_secret`] at the one place that needs the
/// raw value (the request body).
pub struct Secret(String);

impl Secret {
    /// Wraps a raw value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value. Keep the borrow short and never log it.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Whether the value is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let secret = Secret::new("hunter2 hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert!(!format!("{secret:#?}").contains("hunter2"));
        assert_eq!(secret.expose_secret(), "hunter2 hunter2");
    }
}
