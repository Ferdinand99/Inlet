//! The domains an account trusts for links, stored in the user settings
//! (`trusted_domains`) and shared with every Fluxer client. `*` trusts every domain.
//! Read the list from `UserSettings::extra["trusted_domains"]` (READY,
//! USER_SETTINGS_UPDATE); write the whole list back with
//! [`crate::Client::set_trusted_domains`].

use reqwest::Method;
use serde_json::json;

use crate::error::{Error, Result};
use crate::models::UserSettings;
use crate::Client;

/// Most entries the server stores.
pub const MAX_TRUSTED_DOMAINS: usize = 1000;
/// Longest entry the server accepts.
pub const MAX_TRUSTED_DOMAIN_LEN: usize = 253;

/// Checks a list before it is sent: not too long, no empty or over-long entries.
pub fn check_trusted_domains(domains: &[String]) -> Result<()> {
    if domains.len() > MAX_TRUSTED_DOMAINS {
        return Err(Error::InvalidInput(format!(
            "at most {MAX_TRUSTED_DOMAINS} trusted domains"
        )));
    }
    if domains.iter().any(|d| {
        d.is_empty() || d.len() > MAX_TRUSTED_DOMAIN_LEN || d.chars().any(char::is_whitespace)
    }) {
        return Err(Error::InvalidInput("a trusted domain is not valid".into()));
    }
    Ok(())
}

impl Client {
    /// `PATCH /users/@me/settings` with `trusted_domains`: replaces the account's list
    /// (every entry is checked before the request). USER_SETTINGS_UPDATE follows on
    /// the gateway.
    pub async fn set_trusted_domains(&self, domains: &[String]) -> Result<UserSettings> {
        check_trusted_domains(domains)?;
        self.raw(
            Method::PATCH,
            "/users/@me/settings",
            Some(&json!({ "trusted_domains": domains })),
        )
        .await?
        .json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_checked_before_it_is_sent() {
        assert!(check_trusted_domains(&["a.test".into(), "*.b.test".into(), "*".into()]).is_ok());
        assert!(check_trusted_domains(&[String::new()]).is_err());
        assert!(check_trusted_domains(&["two words".into()]).is_err());
        assert!(check_trusted_domains(&["x".repeat(254)]).is_err());
        let many: Vec<String> = (0..=MAX_TRUSTED_DOMAINS)
            .map(|i| format!("h{i}.test"))
            .collect();
        assert!(check_trusted_domains(&many).is_err());
        assert!(check_trusted_domains(&many[..MAX_TRUSTED_DOMAINS]).is_ok());
    }
}
