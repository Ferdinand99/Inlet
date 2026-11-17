// SPDX-License-Identifier: AGPL-3.0-or-later
//! The composer: sending and editing messages, the typing indicator, the client commands
//! that call the server (`/nick`, `/msg`), the data autocomplete needs, and messages
//! with files (`upload`, with photo metadata removal in `exif`). GIFs get their own
//! submodule.
//!
//! Request budget (all REST goes through the protocol's limiter):
//! - one `GET /users/@me` per session, for the account ID and the message length limit;
//! - one send (`POST`) or edit (`PATCH`) per Enter; `/msg` adds one `POST` for the DM;
//!   `/nick` is one `PATCH`;
//! - typing: at most one request per channel every 8 s (the protocol's debounce), and the
//!   UI asks at most once a second;
//! - a Retry more than five minutes after an unclear failure first reads up to 100 of the
//!   channel's messages since the first send (one `GET`) and sends only if none of them
//!   is an own message with the same text (lists carry no nonce);
//! - autocomplete reads roles, channels, emoji and rights from the account state (no
//!   request); member search goes over the gateway (op 8), which the protocol caps at 10
//!   per 10 s;
//! - a message with files: see [`upload`] (one plan request, one `PUT` per part, one
//!   complete request for files in parts, then the message).
//!
//! The pickers' requests (GIFs, preferences, memes, stickers) are in `gif`, with their
//! own budget.

mod directory;
pub mod exif;
pub mod gif;
mod links;
mod send;
pub mod upload;

pub(crate) use directory::{members_chunk, State};
pub use directory::{unix_ms, ChannelEntry, CustomEmoji, Directory, Person, Rights, RoleEntry};
pub use send::{snowflake_u64, Mentions, Outgoing};
pub use upload::{FileToSend, Limits, Refused, Source, Upload, Uploads};

use super::Backend;

pub enum Cmd {
    /// Who is signed in and how long a message may be. Answered with [`Ev::Session`]
    /// carrying the same `token`.
    Session { token: u64 },
    /// Send a message.
    Send(Outgoing),
    /// New text for one of the account's messages.
    Edit {
        channel_id: String,
        message_id: String,
        content: String,
        mentions: Mentions,
    },
    /// The person is typing in this channel.
    Typing { channel_id: String },
    /// Members whose name starts with `query`. Answered with [`Ev::Members`].
    SearchMembers {
        guild_id: String,
        query: String,
        nonce: String,
    },
    /// `/nick`: a new nickname in the guild, `None` to remove it.
    SetNick {
        guild_id: String,
        nick: Option<String>,
    },
    /// `/msg`: a direct message to `user_id`. `message.channel_id` is ignored.
    DirectMessage { user_id: String, message: Outgoing },
    /// Send a message with files. Answered like [`Cmd::Send`], plus the upload events.
    Upload(Upload),
    /// Stop an upload and let go of its files.
    CancelUpload { nonce: String },
    /// The emoji, GIF, sticker and meme pickers.
    Pickers(gif::Cmd),
    /// Save the account's whole list of trusted link domains.
    TrustDomains(Vec<String>),
}

/// Texts and lists from the instance's discovery document (no request: it is read at
/// connect), cut to sizes the views can show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceTexts {
    /// `branding.upload_limit_message`: shown with a refusal of a file that is too large.
    pub upload_note: Option<String>,
    /// The instance's own hosts and `branding.trusted_domains`: links to these open
    /// without a warning.
    pub trusted_domains: Vec<String>,
}

impl InstanceTexts {
    pub fn of(discovery: &inlet_protocol::Discovery) -> Self {
        InstanceTexts {
            upload_note: discovery
                .upload_limit_message()
                .map(|m| plain_note(&m))
                .filter(|m| !m.is_empty()),
            trusted_domains: instance_hosts(discovery)
                .into_iter()
                .chain(discovery.trusted_domains())
                .collect(),
        }
    }
}

pub enum Ev {
    Session {
        /// The `token` of the request.
        token: u64,
        user_id: String,
        /// The instance's limit for this account, in UTF-16 code units.
        max_message_length: usize,
        /// The account's upload limits.
        uploads: Limits,
        /// What the instance's own settings add: its text for oversized files and its
        /// trusted link domains.
        instance: InstanceTexts,
    },
    /// A send was refused with a wait (slowmode or a rate limit); the countdown takes it.
    /// The message list shows the failed message itself.
    SlowmodeWait { channel_id: String, secs: u64 },
    /// A `/msg` was not delivered.
    DirectMessageFailed { nonce: String, reason: String },
    /// The server took an edit (the gateway brings the new text to the list).
    Edited { message_id: String },
    /// An edit was refused.
    EditFailed { message_id: String, reason: String },
    Members {
        guild_id: String,
        members: Vec<Person>,
    },
    /// A sentence for the notice bar (a command finished or failed).
    Notice(String),
    /// An upload began (or began again for a Retry).
    UploadStarted {
        nonce: String,
        channel_id: String,
        names: Vec<String>,
        /// Bytes, as attached.
        total: u64,
    },
    /// About every 1 % of an upload.
    UploadProgress {
        nonce: String,
        sent: u64,
        total: u64,
    },
    /// The upload ended: `sent`, or failed (the message list's row shows Retry).
    UploadDone { nonce: String, sent: bool },
    /// Nothing was sent: the metadata of photo `name` could not be removed. The message
    /// goes back to the composer with that file marked; its pending row goes.
    UploadStuck { nonce: String, name: String },
    /// Answers for the pickers.
    Pickers(gif::Ev),
    /// Saving the trusted link domains failed.
    TrustedDomainsFailed(String),
}

pub(super) fn handle(backend: &mut Backend, cmd: Cmd) {
    match cmd {
        Cmd::Session { token } => backend.composer_session(token),
        Cmd::Send(message) => backend.send(message),
        Cmd::Edit {
            channel_id,
            message_id,
            content,
            mentions,
        } => backend.edit(channel_id, message_id, content, mentions),
        Cmd::Typing { channel_id } => backend.typing(channel_id),
        Cmd::SearchMembers {
            guild_id,
            query,
            nonce,
        } => backend.search_members(guild_id, query, nonce),
        Cmd::SetNick { guild_id, nick } => backend.set_nick(guild_id, nick),
        Cmd::DirectMessage { user_id, message } => backend.direct_message(user_id, message),
        Cmd::Upload(upload) => backend.upload(upload),
        Cmd::CancelUpload { nonce } => backend.cancel_upload(nonce),
        Cmd::Pickers(cmd) => backend.pickers(cmd),
        Cmd::TrustDomains(domains) => backend.trust_domains(domains),
    }
}

/// The hosts of an instance's own endpoints (API, media, web app, invites, gifts, the
/// status page): Fluxer's client trusts the ones it was built from.
fn instance_hosts(discovery: &inlet_protocol::Discovery) -> Vec<String> {
    let e = &discovery.endpoints;
    let mut urls: Vec<&str> = vec![e.api_public.as_str()];
    urls.extend(e.media.as_deref());
    urls.extend(e.webapp.as_deref());
    for key in ["marketing", "invite", "gift", "static_cdn"] {
        urls.extend(e.extra.get(key).and_then(|v| v.as_str()));
    }
    urls.extend(
        discovery
            .app_public
            .get("branding")
            .and_then(|b| b.get("status_page_url"))
            .and_then(|v| v.as_str()),
    );
    let mut hosts: Vec<String> = Vec::new();
    for url in urls {
        let host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()));
        if let Some(host) = host.filter(|h| !h.is_empty() && !hosts.contains(h)) {
            hosts.push(host);
        }
    }
    hosts
}

/// An instance's free text as one plain line: line breaks and tabs become spaces,
/// control, bidi and zero-width characters go, runs of spaces become one.
pub fn plain_note(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        let c = if matches!(c, '\n' | '\r' | '\t') {
            ' '
        } else {
            c
        };
        let hidden = c.is_control()
            || matches!(c,
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                | '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{061C}');
        if hidden {
            continue;
        }
        if c.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instance_note_becomes_one_plain_line() {
        assert_eq!(
            plain_note("Ask\n\nthe  admin\u{202e}!\tThanks\u{0}"),
            "Ask the admin! Thanks"
        );
        assert_eq!(plain_note("\n\n"), "");
    }

    #[test]
    fn instance_texts_come_from_the_discovery_document() {
        let d: inlet_protocol::Discovery = serde_json::from_value(serde_json::json!({
            "api_code_version": 1,
            "endpoints": {"api_public": "https://x.example/api", "gateway": "wss://x.example/gw"},
            "app_public": {"branding": {
                "upload_limit_message": "Need more?\nAsk us.",
                "trusted_domains": ["Docs.Example.org", "*"]
            }}
        }))
        .unwrap();
        let t = InstanceTexts::of(&d);
        assert_eq!(t.upload_note.as_deref(), Some("Need more? Ask us."));
        // The instance's own API host first, then its list.
        assert_eq!(t.trusted_domains, ["x.example", "docs.example.org"]);
    }

    #[test]
    fn the_instances_own_endpoint_hosts_count_as_trusted() {
        let d: inlet_protocol::Discovery = serde_json::from_value(serde_json::json!({
            "api_code_version": 1,
            "endpoints": {
                "api_public": "https://api.chat.example/v1",
                "gateway": "wss://gw.chat.example",
                "media": "https://media.chat.example",
                "webapp": "https://chat.example",
                "invite": "https://join.chat.example",
                "other": "https://not-listed.example"
            },
            "app_public": {"branding": {"status_page_url": "https://status.chat.example/"}}
        }))
        .unwrap();
        let hosts = instance_hosts(&d);
        for h in [
            "api.chat.example",
            "media.chat.example",
            "chat.example",
            "join.chat.example",
            "status.chat.example",
        ] {
            assert!(hosts.contains(&h.to_string()), "{h}");
        }
        assert!(!hosts.contains(&"not-listed.example".to_string()));
        assert!(!hosts.contains(&"gw.chat.example".to_string()));
    }
}
