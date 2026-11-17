// SPDX-License-Identifier: AGPL-3.0-or-later
//! The state of the My profile tab: the account's own profile as the server holds it,
//! the text fields and the pictures picked but not saved yet. The profile is read each
//! time the settings open, and saved only when "Save" is pressed. The bio is a
//! multi-line editor that keeps every line break, with the account's own limit.

use gpui::{AppContext, Context, Entity, PathPromptOptions, SharedString, Window};
use inlet_editor::{Editor, EditorStyle, NoNames};

use crate::app::Inlet;
use crate::backend::settings::account::Cmd;
use crate::backend::settings::profile::{
    ImageChange, OwnProfile, ProfileSave, MAX_BIO_CHARS, MAX_NAME_CHARS, MAX_PRONOUNS_CHARS,
};
use crate::text_input::TextInput;
use crate::theme;

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
    /// The bio. Shift+Enter starts a new line, Enter does nothing.
    pub bio: Option<Entity<Editor>>,
    /// The limit the bio editor was made with.
    bio_limit: usize,
    /// The accent colour as `#rrggbb`, or empty for none.
    pub accent: Option<Entity<TextInput>>,
    /// The pictures picked, not saved yet.
    pub avatar: ImageChange,
    pub banner: ImageChange,
}

/// The editor of a bio: the look of a text box, with the account's limit.
fn bio_editor(limit: usize, cx: &mut gpui::App) -> Entity<Editor> {
    cx.new(|cx| {
        let mut editor = Editor::new(limit, cx);
        editor.set_style(
            EditorStyle {
                text: theme::text(),
                placeholder: theme::text_tertiary(),
                selection: theme::selection(),
                caret: theme::text_strong(),
                pill_background: theme::mention_fill(),
                pill_text: theme::mention_text(),
                pill_radius: gpui::px(4.),
                max_height_fraction: 0.4,
            },
            cx,
        );
        editor.set_placeholder(SharedString::from(t!("settings-profile-bio-hint")), cx);
        editor
    })
}

/// `#rrggbb` for a colour, or nothing for none.
fn hex(color: Option<u32>) -> String {
    color.map_or_else(String::new, |c| format!("#{c:06x}"))
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

    /// The bio as typed, line breaks included.
    pub fn bio_text(&self, cx: &gpui::App) -> String {
        self.bio
            .as_ref()
            .map(|e| e.read(cx).to_wire())
            .unwrap_or_default()
    }

    /// The accent colour as typed: `Some(None)` for none, `None` when the text is not a
    /// colour.
    pub fn accent_typed(&self, cx: &gpui::App) -> Option<Option<u32>> {
        let text = Self::text(&self.accent, cx);
        if text.is_empty() {
            return Some(None);
        }
        theme::parse_hex(&text).map(Some)
    }

    /// What the bio holds and what the account allows, for the counter: characters
    /// typed, and the limit.
    pub fn bio_count(&self, cx: &gpui::App) -> (usize, usize) {
        let limit = self.own.as_ref().map_or(MAX_BIO_CHARS, |o| o.max_bio);
        (self.bio_text(cx).chars().count(), limit)
    }

    /// What the person typed, against what the server holds: the edit to save, or
    /// `None` when nothing changed (or the accent colour is not a colour yet).
    pub fn edit(&self, cx: &gpui::App) -> Option<ProfileSave> {
        let own = self.own.as_ref()?;
        let name = Self::text(&self.name, cx);
        let pronouns = Self::text(&self.pronouns, cx);
        let bio = self.bio_text(cx);
        let accent = self.accent_typed(cx)?;
        let save = ProfileSave {
            name: (name != own.name).then_some(name),
            pronouns: (pronouns != own.pronouns).then_some(pronouns),
            // The exact text: a bio is never flattened or trimmed behind the person's back.
            bio: (bio != own.bio).then_some(bio),
            max_bio: own.max_bio,
            accent: (accent != own.accent).then_some(accent),
            avatar: self.avatar.clone(),
            banner: self.banner.clone(),
        };
        let nothing = ProfileSave {
            max_bio: own.max_bio,
            ..ProfileSave::default()
        };
        (save != nothing).then_some(save)
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
            p.accent = Some(
                cx.new(|cx| TextInput::new(t!("settings-profile-accent-hint"), cx).max_chars(7)),
            );
            p.bio_limit = MAX_BIO_CHARS;
            p.bio = Some(bio_editor(MAX_BIO_CHARS, cx));
        }
    }

    /// Asks for the profile (one `GET`); the settings ask each time they open.
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
    pub(crate) fn on_own_profile(
        &mut self,
        own: OwnProfile,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = &mut self.settings.profile;
        p.load = Load::Loaded;
        p.saving = false;
        p.error = None;
        if saved {
            p.avatar = ImageChange::Keep;
            p.banner = ImageChange::Keep;
        }
        // The editor has the account's limit, which is known only now.
        if p.bio_limit != own.max_bio || p.bio.is_none() {
            p.bio_limit = own.max_bio;
            p.bio = Some(bio_editor(own.max_bio.max(1), cx));
        }
        let fields = [
            (p.name.clone(), own.name.clone()),
            (p.pronouns.clone(), own.pronouns.clone()),
            (p.accent.clone(), hex(own.accent)),
        ];
        let bio = p.bio.clone();
        let bio_text = own.bio.clone();
        p.own = Some(own);
        for (input, text) in fields {
            if let Some(input) = input {
                input.update(cx, |i, cx| i.set_text(SharedString::from(text), cx));
            }
        }
        if let Some(editor) = bio {
            editor.update(cx, |e, cx| e.set_wire(&bio_text, &NoNames, window, cx));
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
    pub(crate) fn profile_reset_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = &mut self.settings.profile;
        p.avatar = ImageChange::Keep;
        p.banner = ImageChange::Keep;
        p.error = None;
        if let Some(own) = p.own.clone() {
            self.on_own_profile(own, false, window, cx);
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
    use gpui::{div, IntoElement, Render, TestAppContext};

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn a_colour_shows_as_hex_and_back() {
        assert_eq!(hex(Some(0x413cdd)), "#413cdd");
        assert_eq!(hex(Some(0xff)), "#0000ff");
        assert_eq!(hex(None), "");
    }

    /// A bio written in Fluxer with line breaks is never flattened: editing another field
    /// sends no bio, and editing the bio sends exactly its text.
    #[gpui::test]
    fn a_multi_line_bio_survives_an_edit_of_another_field(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| Blank);
        cx.update(|window, cx| {
            let own = OwnProfile {
                name: "Ada".into(),
                bio: "line one

line three"
                    .into(),
                max_bio: 320,
                ..OwnProfile::default()
            };
            let name = cx.new(|cx| TextInput::new("n", cx));
            let pronouns = cx.new(|cx| TextInput::new("p", cx));
            let accent = cx.new(|cx| TextInput::new("a", cx));
            let bio = bio_editor(320, cx);
            let p = ProfileState {
                own: Some(own.clone()),
                name: Some(name.clone()),
                pronouns: Some(pronouns),
                accent: Some(accent),
                bio: Some(bio.clone()),
                bio_limit: 320,
                ..ProfileState::default()
            };
            name.update(cx, |i, cx| i.set_text("Ada", cx));
            bio.update(cx, |e, cx| e.set_wire(&own.bio, &NoNames, window, cx));
            assert!(p.edit(cx).is_none(), "nothing changed yet");
            assert_eq!(p.bio_count(cx), (own.bio.chars().count(), 320));

            name.update(cx, |i, cx| i.set_text("Ada Lovelace", cx));
            let save = p.edit(cx).expect("the name changed");
            assert_eq!(save.name.as_deref(), Some("Ada Lovelace"));
            assert_eq!(save.bio, None, "the bio is left as it is");

            bio.update(cx, |e, cx| {
                e.insert_text("!", window, cx).unwrap();
            });
            let save = p.edit(cx).expect("the bio changed");
            assert_eq!(
                save.bio.as_deref(),
                Some(
                    "line one

line three!"
                ),
                "every line break stays"
            );
            assert_eq!(save.max_bio, 320);
        });
    }
}
