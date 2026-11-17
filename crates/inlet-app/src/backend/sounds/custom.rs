// SPDX-License-Identifier: AGPL-3.0-or-later
//! Custom sounds: a file the person chose for one sound. As in Fluxer the file stays on
//! this computer only (never sent anywhere), may be at most 2 MiB, and goes back to the
//! sound that comes with Inlet when removed. The file is copied into the `sounds` folder
//! next to `settings.json`, so it keeps working when the original moves.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::decode::{self, Error};
use super::output::Pcm;
use super::SoundKind;

/// The largest file accepted: 2 MiB, Fluxer's limit.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// The file types Fluxer offers. Opus and WebM audio are offered too but this build
/// cannot decode them yet, so a file of those types is refused with "unsupported".
pub const EXTENSIONS: [&str; 8] = ["mp3", "wav", "ogg", "m4a", "aac", "flac", "opus", "webm"];
/// The longest name kept, in characters.
const NAME_MAX: usize = 64;

/// A custom sound as kept in `settings.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomEntry {
    /// The file's own name, for the list.
    pub name: String,
    /// The copy's name in the `sounds` folder.
    pub file: String,
}

/// Why a file cannot be a custom sound.
#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    TooLarge,
    /// Not one of the offered types, or this build cannot decode it.
    Unsupported,
    TooLong,
    /// The file could not be read, or is broken.
    Unreadable,
}

/// The largest file as the settings say it ("2 MB").
pub fn max_size_label() -> String {
    format!("{} MB", MAX_BYTES / (1024 * 1024))
}

impl Invalid {
    /// What to tell the person.
    pub fn text(&self) -> String {
        match self {
            Invalid::TooLarge => t!("settings-sounds-too-large", size = max_size_label()),
            Invalid::Unsupported => t!("settings-sounds-unsupported"),
            Invalid::TooLong => t!("settings-sounds-too-long"),
            Invalid::Unreadable => t!("settings-sounds-unreadable"),
        }
    }
}

fn extension_of(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// The file in the `sounds` folder; `None` for a name that is not one of ours (the
/// settings file may be edited by hand).
fn stored_path(dir: Option<&Path>, entry: &CustomEntry) -> Option<PathBuf> {
    let ok = !entry.file.is_empty()
        && entry.file.len() <= 64
        && entry
            .file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !entry.file.contains("..");
    ok.then(|| dir.map(|d| d.join(&entry.file))).flatten()
}

/// Reads at most [`MAX_BYTES`] of the file.
fn read_limited(path: &Path) -> Result<Vec<u8>, Invalid> {
    let file = std::fs::File::open(path).map_err(|_| Invalid::Unreadable)?;
    let meta = file.metadata().map_err(|_| Invalid::Unreadable)?;
    if !meta.is_file() {
        return Err(Invalid::Unreadable);
    }
    if meta.len() > MAX_BYTES {
        return Err(Invalid::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Invalid::Unreadable)?;
    // The file grew while it was read.
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Invalid::TooLarge);
    }
    Ok(bytes)
}

fn from_decode(err: Error) -> Invalid {
    match err {
        Error::Unsupported => Invalid::Unsupported,
        Error::TooLong => Invalid::TooLong,
        Error::Broken => Invalid::Unreadable,
    }
}

/// Checks the file at `src`, copies it into `dir` (when there is one) as the sound for
/// `kind`, and returns its entry and its samples.
pub fn import(
    dir: Option<&Path>,
    kind: SoundKind,
    src: &Path,
) -> Result<(CustomEntry, Pcm), Invalid> {
    let ext = extension_of(src).ok_or(Invalid::Unsupported)?;
    let bytes = read_limited(src)?;
    let pcm = decode::decode(&bytes, &ext).map_err(from_decode)?;
    let name: String = crate::model::clean_name(
        &src.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
    .chars()
    .take(NAME_MAX)
    .collect();
    let entry = CustomEntry {
        name: if name.is_empty() {
            format!("{}.{ext}", kind.id())
        } else {
            name
        },
        file: format!("{}.{ext}", kind.id()),
    };
    if let Some(dir) = dir {
        let target = stored_path(Some(dir), &entry).ok_or(Invalid::Unreadable)?;
        write_atomic(dir, &target, &bytes).map_err(|err| {
            log::warn!("could not keep the custom sound: {err}");
            Invalid::Unreadable
        })?;
    }
    Ok((entry, pcm))
}

fn write_atomic(dir: &Path, target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".sound.{}.tmp", std::process::id()));
    let result = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Reads and decodes a custom sound kept in `dir`.
pub fn load(dir: Option<&Path>, entry: &CustomEntry) -> Result<Pcm, Invalid> {
    let path = stored_path(dir, entry).ok_or(Invalid::Unreadable)?;
    let ext = extension_of(&path).ok_or(Invalid::Unsupported)?;
    let bytes = read_limited(&path)?;
    decode::decode(&bytes, &ext).map_err(from_decode)
}

/// Removes the copy of a sound that was replaced, unless the new one has the same name
/// (it was written over the old one).
pub fn forget_file(dir: Option<&Path>, old: &CustomEntry, new: &CustomEntry) {
    if old.file != new.file {
        delete_file(dir, old);
    }
}

/// Removes the copy of a custom sound.
pub fn delete_file(dir: Option<&Path>, entry: &CustomEntry) {
    if let Some(path) = stored_path(dir, entry) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_wav as wav;
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("inlet-custom-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_wav_file_becomes_a_custom_sound_and_is_kept_in_the_sounds_folder() {
        let dir = temp("ok");
        let src = dir.join("My Ding.WAV");
        std::fs::write(&src, wav(22_050, 1, 2205, 8000)).unwrap();
        let store = dir.join("sounds");
        let (entry, pcm) = import(Some(&store), SoundKind::Mute, &src).unwrap();
        assert_eq!(entry.name, "My Ding.WAV");
        assert_eq!(entry.file, "mute.wav");
        assert_eq!(pcm.rate, 22_050);
        // The copy plays after the original is gone.
        std::fs::remove_file(&src).unwrap();
        let again = load(Some(&store), &entry).unwrap();
        assert_eq!(again, pcm);
        delete_file(Some(&store), &entry);
        assert!(load(Some(&store), &entry).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_two_mib_is_refused_before_it_is_read_in() {
        let dir = temp("big");
        let src = dir.join("big.wav");
        std::fs::write(&src, vec![0u8; (MAX_BYTES + 1) as usize]).unwrap();
        assert_eq!(
            import(None, SoundKind::Mute, &src).unwrap_err(),
            Invalid::TooLarge
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_file_types_and_broken_files_are_refused() {
        let dir = temp("types");
        let text = dir.join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        assert_eq!(
            import(None, SoundKind::Mute, &text).unwrap_err(),
            Invalid::Unsupported
        );
        let fake = dir.join("fake.mp3");
        std::fs::write(&fake, "not an mp3").unwrap();
        assert_eq!(
            import(None, SoundKind::Mute, &fake).unwrap_err(),
            Invalid::Unsupported
        );
        let missing = dir.join("gone.wav");
        assert_eq!(
            import(None, SoundKind::Mute, &missing).unwrap_err(),
            Invalid::Unreadable
        );
        // A folder with a sound-like name.
        let folder = dir.join("folder.wav");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(
            import(None, SoundKind::Mute, &folder).unwrap_err(),
            Invalid::Unreadable
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_reason_has_a_sentence() {
        for why in [
            Invalid::TooLarge,
            Invalid::Unsupported,
            Invalid::TooLong,
            Invalid::Unreadable,
        ] {
            assert!(!why.text().is_empty());
        }
        assert!(Invalid::TooLarge.text().contains("2 MB"));
    }

    #[test]
    fn the_stored_name_can_never_leave_the_sounds_folder() {
        let dir = PathBuf::from("sounds");
        let entry = |file: &str| CustomEntry {
            name: "x".into(),
            file: file.into(),
        };
        assert!(stored_path(Some(&dir), &entry("mute.wav")).is_some());
        for bad in [
            "../settings.json",
            "..\\x.wav",
            "a/b.wav",
            "",
            "x y.wav",
            "..",
            "a..b",
        ] {
            assert!(stored_path(Some(&dir), &entry(bad)).is_none(), "{bad}");
        }
        assert!(stored_path(None, &entry("mute.wav")).is_none());
    }
}
