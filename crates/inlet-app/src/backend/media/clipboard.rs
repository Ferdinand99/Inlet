// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Copy image": the picture itself goes to the clipboard, not its link (Fluxer's
//! `MediaClipboard`). The original comes from the media proxy like every image; the
//! bytes are checked and decoded by `inlet-media` with its limits for attachments (at
//! most 20 MiB, 8192 pixels a side, 24 Mpx), and what lands on the clipboard is a PNG
//! this code wrote itself from the decoded pixels. An animation gives its first frame.
//! The source bytes never reach the clipboard, so nothing in them is passed on.

use std::sync::Arc;

use futures::StreamExt;
use image::{ExtendedColorType, ImageEncoder};
use inlet_media::{DecodeControl, FetchError, Fetcher, Fit, KindLimits, MediaError, MediaKind};
use url::Url;

/// Why an image could not be copied, for the notice.
#[derive(Debug)]
pub enum CopyError {
    Fetch(FetchError),
    TooLarge,
    Decode(MediaError),
    Encode,
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::Fetch(e) => write!(f, "{e}"),
            CopyError::TooLarge => f.write_str(&t!("media-copy-too-large")),
            CopyError::Decode(e) => write!(f, "{e}"),
            CopyError::Encode => f.write_str(&t!("media-copy-encode")),
        }
    }
}

/// The limits a copy decodes with: those of attachment previews.
fn limits() -> KindLimits {
    KindLimits::for_kind(MediaKind::Attachment)
}

/// Fetches `url` from the media proxy and returns the picture as PNG bytes. At most
/// 4096 pixels on the longest side (a larger picture is scaled down once).
pub async fn png_of(fetcher: Arc<dyn Fetcher>, url: Url) -> Result<Vec<u8>, CopyError> {
    let limits = limits();
    let resp = fetcher.fetch(&url).await.map_err(CopyError::Fetch)?;
    if resp
        .content_length
        .is_some_and(|n| n > limits.max_encoded_bytes as u64)
    {
        return Err(CopyError::TooLarge);
    }
    let mut body = resp.body;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(CopyError::Fetch)?;
        if bytes.len() + chunk.len() > limits.max_encoded_bytes {
            return Err(CopyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || png_from_bytes(&bytes, &limits))
        .await
        .map_err(|_| CopyError::Encode)?
}

/// Decodes `bytes` under `limits` and writes the first frame as a PNG.
pub fn png_from_bytes(bytes: &[u8], limits: &KindLimits) -> Result<Vec<u8>, CopyError> {
    let fit = Fit::Contain {
        max_width: Fit::MAX_SIDE,
        max_height: Fit::MAX_SIDE,
    };
    let control = DecodeControl::with_budget(limits.decode_budget);
    let decoded = inlet_media::decode(bytes, fit, limits, &control).map_err(CopyError::Decode)?;
    let frame = decoded.frames.first().ok_or(CopyError::Encode)?;
    // gpui's pixel order is B, G, R, A; a PNG wants R, G, B, A.
    let mut rgba = frame.bgra.clone();
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            &rgba,
            decoded.width,
            decoded.height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| CopyError::Encode)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::media::png;

    #[test]
    fn a_picture_comes_back_as_a_png_with_the_same_pixels() {
        // One red pixel next to one half-transparent blue one.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 128];
        let source = png::encode(2, 1, &rgba);
        let out = png_from_bytes(&source, &limits()).unwrap();
        assert_eq!(&out[..8], b"\x89PNG\r\n\x1a\n");
        let back = image::load_from_memory_with_format(&out, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(back.dimensions(), (2, 1));
        assert_eq!(back.as_raw(), &rgba);
    }

    #[test]
    fn an_animation_gives_its_first_frame() {
        let first = vec![10, 200, 30, 255];
        let second = vec![250, 0, 0, 255];
        let gif = png::encode_gif(1, 1, &[first, second], 10);
        let out = png_from_bytes(&gif, &limits()).unwrap();
        let back = image::load_from_memory_with_format(&out, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(back.dimensions(), (1, 1));
        // A GIF palette keeps the colour within a step of the original.
        let px = back.get_pixel(0, 0).0;
        assert!(px[1] > 150 && px[0] < 100, "{px:?}");
    }

    #[test]
    fn something_that_is_not_a_picture_or_claims_too_much_is_refused() {
        assert!(matches!(
            png_from_bytes(b"#!/bin/sh\necho no\n", &limits()),
            Err(CopyError::Decode(_))
        ));
        // A header that claims a huge size is refused before any pixels exist.
        let huge = png::claiming(60_000, 60_000);
        assert!(matches!(
            png_from_bytes(&huge, &limits()),
            Err(CopyError::Decode(_))
        ));
        // Too many bytes.
        let big = vec![0u8; limits().max_encoded_bytes + 1];
        assert!(matches!(
            png_from_bytes(&big, &limits()),
            Err(CopyError::Decode(MediaError::TooLargeEncoded { .. }))
        ));
    }
}
