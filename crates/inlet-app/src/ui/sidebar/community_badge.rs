// SPDX-License-Identifier: AGPL-3.0-or-later
//! The badge a server shows beside its name when the instance has marked it: verified,
//! partnered (with or without verification) or discoverable.
//!
//! Fluxer's `GuildBadge` reads the server's `features` (`VERIFIED`, `PARTNERED`,
//! `DISCOVERABLE`) and draws one icon; a partnered server wins over a verified one, and a
//! verified one over a discoverable one. The icons here are Phosphor's, not Fluxer's.
// Deviation: Fluxer names the badge in a tooltip; Inlet has no tooltips yet, so the badge
// carries no text.

use gpui::{px, App, Hsla};

use crate::app::Inlet;
use crate::assets::Icon;
use crate::theme;

use crate::model::CommunityBadge;

impl CommunityBadge {
    fn icon(self) -> Icon {
        match self {
            CommunityBadge::Partnered => Icon::Handshake,
            CommunityBadge::Verified => Icon::SealCheck,
            CommunityBadge::Discoverable => Icon::Compass,
        }
    }

    /// The badge as a 20 px icon in `color`.
    pub fn el(self, color: Hsla) -> gpui::Svg {
        self.icon().el(px(20.), color)
    }
}

impl Inlet {
    /// The badge of a server the account is in.
    pub(crate) fn community_badge(&self, guild_id: &str, cx: &App) -> Option<CommunityBadge> {
        let id = inlet_state::GuildId::parse(guild_id)?;
        let state = self.account.as_ref()?.read(cx);
        let features = &state.guild(id)?.properties.as_ref()?.features;
        CommunityBadge::of(features)
    }

    /// The icon to draw beside the name of the open server.
    pub(crate) fn community_badge_icon(&self, guild_id: &str, cx: &App) -> Option<gpui::Svg> {
        self.community_badge(guild_id, cx)
            .map(|b| b.el(theme::brand()))
    }
}
