// SPDX-License-Identifier: AGPL-3.0-or-later
//! The state of the My profile tab: the account's own profile as the server holds it,
//! the text fields and the pictures picked but not saved yet. The profile is read when
//! the settings open, and saved only when "Save" is pressed.

use gpui::{AppContext, Context, Entity, PathPromptOptions, SharedString};

use crate::app::Inlet;
use crate::backend::settings::account::Cmd;
use crate::backend::settings::profile::{
    ImageChange, OwnProfile, ProfileSave, MAX_BIO_CHARS, MAX_NAME_CHARS, MAX_PRONOUNS_CHARS,
};
use crate::text_input::TextInput;

/// Which of the two pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    Avatar,
    Banner,
}

/// Where the profile is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Load {
    #[default]
    NotAsked,
    Loading,
    Loaded,
    Failed(String),
}

/// The My profile tab's state.
#[derive(Default)]
pub struct ProfileState {
    /// The profile as the server holds it.
    pub own: Option<OwnProfile>,
    pub load: Load,
    /// A save is on its way.
    pub saving: bool,
    /// Why the last save failed.
    pub error: Option<String>,
    pub name: Option<Entity<TextInput>>,
    pub pronouns: Option<Entity<TextInput>>,
    pub bio: Option<Entity<TextInput>>,
    /// The pictures picked, not saved yet.
    pub avatar: ImageChange,
    pub banner: ImageChange,
}

/// A bio on one line: the field is a single line, so line breaks written elsewhere
/// show as spaces and are kept while the text is not changed.
// Deviation: Fluxer edits the bio in a multi-line box.
fn one_line(bio: &str) -> String {
    bio.split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

impl ProfileState {
    /// The signed-in account changed: nothing of the last one stays.
    pub fn reset(&mut self) {
        self.own = None;
        self.load = Load::NotAsked;
        self.saving = false;
        self.error = None;
        self.avatar = ImageChange::Keep;
        self.banner = ImageChange::Keep;
    }

    fn text(input: &Option<Entity<TextInput>>, cx: &gpui::App) -> String {
        input
            .as_ref()
            .map(|i| i.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    /// What the person typed, against what the server holds: the edit to save, or
    /// `None` when nothing changed.
    pub fn edit(&self, cx: &gpui::App) -> Option<ProfileSave> {
        let own = self.own.as_ref()?;
        let name = Self::text(&self.name, cx);
        let pronouns = Self::text(&self.pronouns, cx);
        let bio = Self::text(&self.bio, cx);
        let save = ProfileSave {
            name: (name != own.name).then_some(name),
            pronouns: (pronouns != own.pronouns).then_some(pronouns),
            bio: (bio != one_line(&own.bio)).then_some(bio),
            avatar: self.avatar.clone(),
            banner: self.banner.clone(),
        };
        (save != ProfileSave::default()).then_some(save)
    }
}

impl Inlet {
    /// The text fields, made once.
    pub(crate) fn profile_ensure_inputs(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        if p.name.is_none() {
            p.name = Some(cx.new(|cx| {
                TextInput::new(t!("settings-profile-name-hint"), cx).max_chars(MAX_NAME_CHARS)
            }));
            p.pronouns = Some(cx.new(|cx| {
                TextInput::new(t!("settings-profile-pronouns-hint"), cx)
                    .max_chars(MAX_PRONOUNS_CHARS)
            }));
            p.bio = Some(cx.new(|cx| {
                TextInput::new(t!("settings-profile-bio-hint"), cx).max_chars(MAX_BIO_CHARS)
            }));
        }
    }

    /// Asks for the profile once per account (the request is one `GET`).
    pub(crate) fn profile_load(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        if matches!(p.load, Load::Loading | Load::Loaded) {
            return;
        }
        p.load = Load::Loading;
        self.backend.send(Cmd::LoadProfile);
        cx.notify();
    }

    /// The profile arrived (or was saved): the fields show it.
    pub(crate) fn on_own_profile(&mut self, own: OwnProfile, saved: bool, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        p.load = Load::Loaded;
        p.saving = false;
        p.error = None;
        if saved {
            p.avatar = ImageChange::Keep;
            p.banner = ImageChange::Keep;
        }
        let fields = [
            (p.name.clone(), own.name.clone()),
            (p.pronouns.clone(), own.pronouns.clone()),
            (p.bio.clone(), one_line(&own.bio)),
        ];
        p.own = Some(own);
        for (input, text) in fields {
            if let Some(input) = input {
                input.update(cx, |i, cx| i.set_text(SharedString::from(text), cx));
            }
        }
    }

    /// Reading the profile, or saving it, failed.
    pub(crate) fn on_profile_failed(&mut self, reason: String, save: bool, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        if save {
            p.saving = false;
            p.error = Some(reason);
        } else {
            p.load = Load::Failed(reason);
        }
        cx.notify();
    }

    /// "Save": sends what changed, one save at a time.
    pub(crate) fn profile_save(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        if p.saving {
            return;
        }
        let Some(save) = p.edit(cx) else {
            return;
        };
        p.saving = true;
        p.error = None;
        self.backend.send(Cmd::SaveProfile(Box::new(save)));
        cx.notify();
    }

    /// "Reset": the fields show what the server holds again.
    pub(crate) fn profile_reset_fields(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        p.avatar = ImageChange::Keep;
        p.banner = ImageChange::Keep;
        p.error = None;
        if let Some(own) = p.own.clone() {
            self.on_own_profile(own, false, cx);
        }
        cx.notify();
    }

    /// A picture to remove.
    pub(crate) fn profile_remove_picture(&mut self, which: Picture, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        match which {
            Picture::Avatar => p.avatar = ImageChange::Remove,
            Picture::Banner => p.banner = ImageChange::Remove,
        }
        cx.notify();
    }

    /// Asks which file to use as the picture; nothing is read until "Save".
    pub(crate) fn profile_pick_picture(&mut self, which: Picture, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(t!("settings-profile-pick-prompt"))),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let p = &mut this.settings.profile;
                match which {
                    Picture::Avatar => p.avatar = ImageChange::Set(path),
                    Picture::Banner => p.banner = ImageChange::Set(path),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bio_shows_on_one_line() {
        assert_eq!(one_line("a\nb  \n\nc"), "a b  c");
        assert_eq!(one_line("  plain  "), "plain");
        assert_eq!(one_line(""), "");
    }
}
