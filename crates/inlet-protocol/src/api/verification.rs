//! Calls behind the account banners: sending the verification email again, and
//! accepting updated terms.

use reqwest::Method;

use crate::error::Result;
use crate::models::CurrentUser;
use crate::Client;

impl Client {
    /// `POST /auth/verify/resend`: sends the verification email to the account's
    /// address again. The instance rate limits it hard, so a caller waits at least a
    /// minute between two clicks.
    pub async fn resend_verification_email(&self) -> Result<()> {
        self.raw(Method::POST, "/auth/verify/resend", None).await?;
        Ok(())
    }

    /// `POST /users/@me/terms-acceptance`: the account accepts the instance's updated
    /// terms. Returns the account as it is now.
    pub async fn accept_terms(&self) -> Result<CurrentUser> {
        self.raw(Method::POST, "/users/@me/terms-acceptance", None)
            .await?
            .json()
    }
}
