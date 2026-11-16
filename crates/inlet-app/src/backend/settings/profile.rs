// SPDX-License-Identifier: AGPL-3.0-or-later
//! The account's own profile: reading it as it is edited (display name, pronouns, bio,
//! avatar, banner) and saving an edit. A picture is read from the file the person
//! picked, at most 10 MiB, and sent only when its first bytes say it is a PNG, JPEG,
//! GIF or WebP; the file is never opened by the system or run.

use std::path::PathBuf;

use inlet_protocol as p;
use tokio::sync::mpsc::UnboundedSender as TokioSender;

use crate::backend::session::{Failure, FailureKind};
use crate::backend::Internal;
use crate::model::UserAvatar;

/// Largest picture the profile takes (Fluxer's default).
pub const MAX_IMAGE_BYTES: u64 = p::api::profiles::MAX_IMAGE_BYTES as u64;
/// Longest display name, in characters.
pub const MAX_NAME_CHARS: usize = 32;
/// Longest pronouns text, in characters.
pub const MAX_PRONOUNS_CHARS: usize = 40;
/// Longest bio the tab lets the person type (Fluxer's default limit is 320; an instance
/// may allow more, and the server checks).
pub const MAX_BIO_CHARS: usize = 320;
/// The most of a bio read back and kept.
const MAX_BIO_READ: usize = 4096;

/// How a picture of the profile changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ImageChange {
    #[default]
    Keep,
    Remove,
    /// A file the person picked.
    Set(PathBuf),
}

/// The edit the My profile tab asks for. `None` fields are left as they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileSave {
    /// Display name; empty clears it (the username shows instead).
    pub name: Option<String>,
    pub pronouns: Option<String>,
    pub bio: Option<String>,
    pub avatar: ImageChange,
    pub banner: ImageChange,
}

/// The account's own profile, as the My profile tab shows and edits it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnProfile {
    pub user_id: String,
    pub username: String,
    /// The display name, empty when none is set.
    pub name: String,
    pub avatar: Option<UserAvatar>,
    /// The banner's hash, if there is one.
    pub banner: Option<String>,
    pub pronouns: String,
    /// The bio as written (markdown).
    pub bio: String,
}

/// A failed profile request: the protocol's error, or a sentence of our own (a picture
/// that cannot be sent).
pub enum ProfileError {
    Api(p::Error),
    Text(String),
}

impl From<p::Error> for ProfileError {
    fn from(err: p::Error) -> Self {
        ProfileError::Api(err)
    }
}

/// Why a request failed, as a sentence; `None` when the session ended instead (the
/// accounts domain says so).
pub(in crate::backend) fn reason(
    internal: &TokioSender<Internal>,
    gen: u64,
    err: ProfileError,
) -> Option<String> {
    match err {
        ProfileError::Text(t) => Some(t),
        ProfileError::Api(err) => {
            let failure = Failure::from(err);
            if failure.kind == FailureKind::SessionEnded {
                let _ = internal.send(Internal::Ended {
                    gen,
                    message: failure.message,
                });
                None
            } else {
                Some(failure.message)
            }
        }
    }
}

/// Reads the account's own profile.
pub async fn load(client: &p::Client) -> Result<OwnProfile, ProfileError> {
    let me = client.me().await?;
    let full = client.user_profile(&me.id, None, false).await?;
    Ok(own_profile(&full))
}

/// The profile as the tab edits it: texts without control characters (a bio keeps its
/// line breaks), cut to what a profile may hold.
pub fn own_profile(full: &p::FullUserProfile) -> OwnProfile {
    let clean = |s: Option<&str>, max: usize, multiline: bool| -> String {
        s.map(|t| {
            t.chars()
                .filter(|c| !hidden(*c, multiline))
                .take(max)
                .collect()
        })
        .unwrap_or_default()
    };
    OwnProfile {
        user_id: full.user.id.clone(),
        username: crate::model::clean_name(&full.user.username),
        name: clean(full.user.global_name.as_deref(), MAX_NAME_CHARS, false)
            .trim()
            .to_owned(),
        avatar: crate::backend::media::user_avatar(&full.user),
        banner: full
            .user_profile
            .banner
            .as_deref()
            .filter(|h| !h.is_empty() && h.len() <= 128)
            .map(str::to_owned),
        pronouns: clean(
            full.user_profile.pronouns.as_deref(),
            MAX_PRONOUNS_CHARS,
            false,
        )
        .trim()
        .to_owned(),
        bio: clean(full.user_profile.bio.as_deref(), MAX_BIO_READ, true),
    }
}

/// A character that shows nothing or changes how text runs, never kept from the server:
/// controls (a bio keeps its line breaks), the zero-width space, the direction marks and
/// the line and paragraph separators. The zero-width joiner and non-joiner stay: emoji
/// sequences and some scripts need them, and saving must not rewrite them.
fn hidden(c: char, multiline: bool) -> bool {
    (c.is_control() && !(multiline && c == '\n'))
        || matches!(c,
            '\u{200B}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' | '\u{2028}' | '\u{2029}')
}

/// The profile the demo "saves": what the person asked for, on top of the demo profile.
#[cfg(feature = "demo")]
pub fn demo_saved(save: ProfileSave) -> OwnProfile {
    let mut p = demo_profile();
    if let Some(name) = save.name {
        p.name = name;
    }
    if let Some(pronouns) = save.pronouns {
        p.pronouns = pronouns;
    }
    if let Some(bio) = save.bio {
        p.bio = bio;
    }
    p
}

/// Sends a profile edit, then reads the profile.
pub async fn save(client: &p::Client, save: ProfileSave) -> Result<OwnProfile, ProfileError> {
    let edit = edit_of(save).await?;
    let me = client.edit_profile(&edit).await?;
    let full = client.user_profile(&me.id, None, false).await?;
    Ok(own_profile(&full))
}

/// The protocol's edit for what the tab asked: text cut to its limit (empty text clears
/// the field), pictures read from their files.
pub async fn edit_of(save: ProfileSave) -> Result<p::ProfileEdit, ProfileError> {
    let text = |t: Option<String>, max: usize| -> Option<Option<String>> {
        t.map(|t| {
            let t: String = t.chars().take(max).collect();
            (!t.trim().is_empty()).then_some(t)
        })
    };
    Ok(p::ProfileEdit {
        global_name: text(save.name, MAX_NAME_CHARS),
        pronouns: text(save.pronouns, MAX_PRONOUNS_CHARS),
        bio: text(save.bio, MAX_BIO_READ),
        avatar: image(save.avatar).await?,
        banner: image(save.banner).await?,
        accent_color: None,
    })
}

async fn image(change: ImageChange) -> Result<Option<Option<p::ImageData>>, ProfileError> {
    Ok(match change {
        ImageChange::Keep => None,
        ImageChange::Remove => Some(None),
        ImageChange::Set(path) => Some(Some(read_image(path).await?)),
    })
}

/// The content type of a picture, from its first bytes (never from its file name).
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(&bytes[8..12], b"avif" | b"avis")
    {
        Some("image/avif")
    } else {
        None
    }
}

/// Reads a picture the person picked: at most [`MAX_IMAGE_BYTES`], and one of the types
/// Fluxer takes. The file is only read.
pub async fn read_image(path: PathBuf) -> Result<p::ImageData, ProfileError> {
    use tokio::io::AsyncReadExt;
    let unreadable = |e: std::io::Error| {
        ProfileError::Text(t!("settings-image-unreadable", error = e.to_string()))
    };
    let file = tokio::fs::File::open(&path).await.map_err(unreadable)?;
    let mut bytes = Vec::new();
    // One byte more than allowed tells a file that is too large from one that fits.
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ProfileError::Text(t!("settings-image-too-large")));
    }
    let kind =
        sniff_image(&bytes).ok_or_else(|| ProfileError::Text(t!("settings-image-unsupported")))?;
    Ok(p::ImageData::new(kind, bytes)?)
}

/// The demo account's profile.
#[cfg(feature = "demo")]
pub fn demo_profile() -> OwnProfile {
    OwnProfile {
        user_id: crate::demo::user_id(crate::demo::ME),
        username: "demo".into(),
        name: "Demo Account".into(),
        avatar: None,
        banner: None,
        pronouns: "they/them".into(),
        bio: "Tests Inlet against a demo instance.".into(),
    }
}
