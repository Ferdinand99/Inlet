// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reading a sound file the person chose: MP3, WAV, Ogg Vorbis, FLAC and AAC/M4A, with
//! symphonia. The file is untrusted: its size, its length in time and its sample rate
//! are limited before anything is kept, and decoding happens off the UI thread.

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::output::Pcm;

/// The longest custom sound, in seconds.
pub const MAX_SECONDS: usize = 60;
/// The highest sample rate accepted, in Hz.
const MAX_RATE: u32 = 192_000;
/// Most channels mixed down.
const MAX_CHANNELS: usize = 8;

/// Why a file is no sound.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// No decoder for it here (a format this build does not read, or not audio).
    Unsupported,
    /// Longer than [`MAX_SECONDS`].
    TooLong,
    /// A file that is broken, or has no sound in it.
    Broken,
}

/// The sound in `bytes` as mono samples at its own rate. `extension` (without the dot)
/// only helps to find the format.
pub fn decode(bytes: &[u8], extension: &str) -> Result<Pcm, Error> {
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| Error::Unsupported)?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(Error::Broken)?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| Error::Unsupported)?;

    let mut rate = 0u32;
    let mut samples: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // The end of the file arrives as an end-of-file error; a stream that must be
            // reset (chained files) is not a sound effect.
            Err(SymphoniaError::IoError(_) | SymphoniaError::ResetRequired) => break,
            Err(_) => return Err(Error::Broken),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // One bad packet is skipped; the rest of the file may be fine.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(_) => return Err(Error::Broken),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if channels == 0 || channels > MAX_CHANNELS || spec.rate == 0 || spec.rate > MAX_RATE {
            return Err(Error::Unsupported);
        }
        if rate == 0 {
            rate = spec.rate;
        } else if rate != spec.rate {
            return Err(Error::Unsupported);
        }
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        for frame in buffer.samples().chunks_exact(channels) {
            samples.push(frame.iter().sum::<f32>() / channels as f32);
        }
        if samples.len() > MAX_SECONDS * rate as usize {
            return Err(Error::TooLong);
        }
    }
    if samples.is_empty() || rate == 0 {
        return Err(Error::Broken);
    }
    for s in &mut samples {
        *s = if s.is_finite() {
            s.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
    Ok(Pcm { rate, samples })
}

#[cfg(test)]
mod tests {
    use super::super::test_wav as wav;
    use super::*;

    #[test]
    fn a_stereo_wav_is_mixed_down_to_mono() {
        let pcm = decode(&wav(44_100, 2, 4410, 16384), "wav").expect("a WAV decodes");
        assert_eq!(pcm.rate, 44_100);
        assert_eq!(pcm.samples.len(), 4410);
        assert!(pcm.samples.iter().all(|s| (s - 0.5).abs() < 1e-3));
    }

    #[test]
    fn the_extension_is_only_a_hint() {
        let pcm = decode(&wav(8000, 1, 800, 1000), "mp3").expect("found by its content");
        assert_eq!(pcm.samples.len(), 800);
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused() {
        assert_eq!(
            decode(b"definitely not a sound", "mp3"),
            Err(Error::Unsupported)
        );
        assert_eq!(decode(&[], "wav"), Err(Error::Unsupported));
        // A WAV cut off before any sample.
        let cut = wav(8000, 1, 0, 0);
        assert!(decode(&cut, "wav").is_err());
    }

    #[test]
    fn a_sound_longer_than_a_minute_is_refused() {
        let long = wav(8000, 1, 8000 * (MAX_SECONDS + 2), 100);
        assert_eq!(decode(&long, "wav"), Err(Error::TooLong));
    }
}
