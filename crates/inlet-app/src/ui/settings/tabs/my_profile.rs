// SPDX-License-Identifier: AGPL-3.0-or-later
//! The My profile tab: the avatar, the banner, the display name, the pronouns, the bio
//! and the accent colour of the account. The profile is read each time the settings
//! open; nothing is sent until "Save". A picture is read from the file picked, only then.
//! The bio is a multi-line field with a counter against the account's own limit, as in
//! Fluxer; the banner can be changed only where the account's plan allows one.

use gpui::{div, prelude::*, px, AnyElement, Context, Entity, SharedString};
use inlet_editor::Editor;

use super::super::controls::page;
use super::super::profile::{Load, Picture};
use super::super::{with_app, SettingsTab, SettingsView};
use super::desktop::{heading, note};
use crate::backend::settings::profile::{ImageChange, OwnProfile};
use crate::text_input::TextInput;
use crate::theme;
use crate::ui::widgets::{button, field};

pub struct MyProfileTab;

/// What the tab draws, taken from the app in one go.
struct View {
    own: Option<OwnProfile>,
    load: Load,
    saving: bool,
    error: Option<String>,
    avatar: ImageChange,
    banner: ImageChange,
    changed: bool,
    name: Option<Entity<TextInput>>,
    pronouns: Option<Entity<TextInput>>,
    bio: Option<Entity<Editor>>,
    bio_count: (usize, usize),
    accent: Option<Entity<TextInput>>,
    accent_typed: Option<Option<u32>>,
    picture: Option<AnyElement>,
}

/// What a picked picture says: its file name, or that it goes.
fn pending(change: &ImageChange) -> Option<String> {
    match change {
        ImageChange::Keep => None,
        ImageChange::Remove => Some(t!("settings-profile-will-remove")),
        ImageChange::Set(path) => Some(t!(
            "settings-profile-will-use",
            file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
    }
}

impl SettingsTab for MyProfileTab {
    fn id(&self) -> &'static str {
        "my-profile"
    }

    fn title(&self) -> SharedString {
        t!("settings-profile-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        let Some(view) = with_app(cx, |app, cx| {
            let p = &app.settings.profile;
            let shown_name = p
                .own
                .as_ref()
                .map(|o| {
                    if o.name.is_empty() {
                        o.username.clone()
                    } else {
                        o.name.clone()
                    }
                })
                .unwrap_or_default();
            let avatar = match p.avatar {
                ImageChange::Keep => p.own.as_ref().and_then(|o| o.avatar.clone()),
                _ => None,
            };
            let picture = app.user_avatar(avatar.as_ref(), &shown_name, 80., true, cx);
            let p = &app.settings.profile;
            View {
                own: p.own.clone(),
                load: p.load.clone(),
                saving: p.saving,
                error: p.error.clone(),
                avatar: p.avatar.clone(),
                banner: p.banner.clone(),
                changed: p.edit(cx).is_some(),
                name: p.name.clone(),
                pronouns: p.pronouns.clone(),
                bio: p.bio.clone(),
                bio_count: p.bio_count(cx),
                accent: p.accent.clone(),
                accent_typed: p.accent_typed(cx),
                picture: Some(picture),
            }
        }) else {
            return div().into_any_element();
        };

        let mut col = page(t!("settings-profile-title"));
        match &view.load {
            Load::NotAsked | Load::Loading => {
                return col
                    .child(note(t!("settings-profile-loading")).pl_0())
                    .into_any_element();
            }
            Load::Failed(reason) => {
                return col
                    .child(note(t!("settings-profile-load-failed", reason = reason.clone())).pl_0())
                    .child(
                        div().flex().flex_row().pt_2().child(
                            button(
                                "settings-profile-retry",
                                t!("settings-profile-retry"),
                                false,
                            )
                            .on_click(cx.listener(|_, _, _, cx| {
                                with_app(cx, |app, cx| {
                                    app.settings.profile.load = Load::NotAsked;
                                    app.profile_load(cx);
                                });
                            })),
                        ),
                    )
                    .into_any_element();
            }
            Load::Loaded => {}
        }
        let username = view
            .own
            .as_ref()
            .map(|o| o.username.clone())
            .unwrap_or_default();
        let has_banner = view.own.as_ref().is_some_and(|o| o.banner.is_some());
        let has_avatar = view.own.as_ref().is_some_and(|o| o.avatar.is_some());
        let can_banner = view.own.as_ref().is_some_and(|o| o.can_upload_banner);
        let accent = view.accent_typed;

        let picture_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .py_2()
            .child(div().size(px(80.)).flex_none().children(view.picture))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                button(
                                    "settings-profile-avatar-change",
                                    t!("settings-profile-avatar-change"),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |_, _, _, cx| {
                                        with_app(cx, |app, cx| {
                                            app.profile_pick_picture(Picture::Avatar, cx)
                                        });
                                    },
                                )),
                            )
                            .when(has_avatar, |d| {
                                d.child(
                                    button(
                                        "settings-profile-avatar-remove",
                                        t!("settings-profile-avatar-remove"),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |_, _, _, cx| {
                                            with_app(cx, |app, cx| {
                                                app.profile_remove_picture(Picture::Avatar, cx)
                                            });
                                        },
                                    )),
                                )
                            }),
                    )
                    .children(
                        pending(&view.avatar)
                            .map(|t| div().text_size(px(13.)).text_color(theme::brand()).child(t)),
                    ),
            );

        col = col
            .child(heading(t!("settings-profile-picture")))
            .child(picture_row)
            .child(
                div()
                    .pt_2()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(t!("settings-profile-username", username = username)),
            );

        if let Some(input) = view.name {
            col = col.child(
                div()
                    .pt_4()
                    .child(field(t!("settings-profile-name"), input)),
            );
        }
        col = col.child(note(t!("settings-profile-name-note")).pl_0().pt_1());
        if let Some(input) = view.pronouns {
            col = col.child(
                div()
                    .pt_4()
                    .child(field(t!("settings-profile-pronouns"), input)),
            );
        }
        if let Some(editor) = view.bio {
            let (typed, limit) = view.bio_count;
            col = col.child(
                div()
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child(t!("settings-profile-bio").to_uppercase())
                            .child(
                                div()
                                    .text_color(if typed >= limit {
                                        theme::danger()
                                    } else {
                                        theme::text_muted()
                                    })
                                    .child(t!(
                                        "settings-profile-bio-count",
                                        typed = typed,
                                        limit = limit
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .px_3()
                            .py(px(10.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::ring())
                            .bg(theme::bg_textarea())
                            .text_size(px(16.))
                            .line_height(px(22.))
                            .child(editor),
                    ),
            );
        }
        col = col.child(note(t!("settings-profile-bio-note")).pl_0().pt_1());

        if let Some(input) = view.accent {
            let swatch = accent.flatten();
            col = col.child(
                div()
                    .pt_4()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .child(field(t!("settings-profile-accent"), input)),
                    )
                    .child(
                        div()
                            .size(px(40.))
                            .flex_none()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::accent_border())
                            .when_some(swatch, |d, c| d.bg(gpui::rgb(c))),
                    ),
            );
            if accent.is_none() {
                col = col.child(
                    div()
                        .pt_1()
                        .text_size(px(13.))
                        .text_color(theme::danger())
                        .child(t!("settings-profile-accent-bad")),
                );
            }
        }
        col = col.child(note(t!("settings-profile-accent-note")).pl_0().pt_1());

        col = col.child(heading(t!("settings-profile-banner"))).child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .py_2()
                .child(div().text_color(theme::text_strong()).child(if has_banner {
                    t!("settings-profile-banner-set")
                } else {
                    t!("settings-profile-banner-none")
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        // Without the plan's banner feature the upload is not offered
                        // (a banner already there can still be removed).
                        .when(can_banner, |d| {
                            d.child(
                                button(
                                    "settings-profile-banner-change",
                                    t!("settings-profile-banner-change"),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |_, _, _, cx| {
                                        with_app(cx, |app, cx| {
                                            app.profile_pick_picture(Picture::Banner, cx)
                                        });
                                    },
                                )),
                            )
                        })
                        .when(has_banner, |d| {
                            d.child(
                                button(
                                    "settings-profile-banner-remove",
                                    t!("settings-profile-banner-remove"),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |_, _, _, cx| {
                                        with_app(cx, |app, cx| {
                                            app.profile_remove_picture(Picture::Banner, cx)
                                        });
                                    },
                                )),
                            )
                        }),
                )
                .children(
                    pending(&view.banner)
                        .map(|t| div().text_size(px(13.)).text_color(theme::brand()).child(t)),
                ),
        );
        col = col.child(
            note(if can_banner {
                t!("settings-profile-banner-note")
            } else {
                t!("settings-profile-banner-locked")
            })
            .pl_0(),
        );

        let can_save = view.changed && !view.saving;
        col = col
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .pt_6()
                    .child(
                        button(
                            "settings-profile-save",
                            if view.saving {
                                t!("settings-profile-saving")
                            } else {
                                t!("settings-profile-save")
                            },
                            true,
                        )
                        .when(!can_save, |d| d.opacity(0.5))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if can_save {
                                with_app(cx, |app, cx| app.profile_save(cx));
                            }
                        })),
                    )
                    .child(
                        button(
                            "settings-profile-reset",
                            t!("settings-profile-reset"),
                            false,
                        )
                        .when(!view.changed, |d| d.opacity(0.5))
                        .on_click(cx.listener(|_, _, window, cx| {
                            with_app(cx, |app, cx| app.profile_reset_fields(window, cx));
                        })),
                    ),
            )
            .children(view.error.map(|e| {
                div()
                    .pt_2()
                    .text_color(theme::danger())
                    .child(t!("settings-profile-save-failed", reason = e))
            }));
        col.into_any_element()
    }
}
