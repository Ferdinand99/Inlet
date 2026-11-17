// SPDX-License-Identifier: AGPL-3.0-or-later
//! Saving the account's trusted link domains (`trusted_domains` in the user settings).
//!
//! Request budget: one `PATCH /users/@me/settings` per "always trust" click, through the
//! limiter. The list is checked first (at most 1000 entries of at most 253 characters).

use super::send::failed;
use super::Ev;
#[cfg(feature = "demo")]
use crate::backend::Active;
use crate::backend::Backend;

impl Backend {
    /// Saves the whole list. A failure says so; the next settings update from the
    /// server puts the shown list back.
    pub(super) fn trust_domains(&mut self, domains: Vec<String>) {
        #[cfg(feature = "demo")]
        if let Some(Active::Demo(_)) = &self.active {
            return;
        }
        let Some(ctx) = self.send_ctx() else {
            return;
        };
        self.spawn_guarded("trusted domains", None, async move {
            if let Err(err) = ctx.client.set_trusted_domains(&domains).await {
                if let Some(e) = failed(&ctx.internal, ctx.gen, err) {
                    let _ = ctx
                        .events
                        .unbounded_send(Ev::TrustedDomainsFailed(e.reason));
                }
            }
        });
    }
}
