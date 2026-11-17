// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sound effects section of the Notifications tab, as in Fluxer's: a master volume,
//! one switch for all sounds, a list with a switch, a preview, an upload and a remove
//! button per sound, and a list of volumes per sound. The values live in the sound hub
//! (`backend::sounds`); this file only shows and changes them.

use gpui::{
    div, prelude::*, px, svg, AnyElement, Context, Div, FontWeight, Hsla, SharedString, Stateful,
    Svg,
};

use super::super::controls::button_with_id;
use super::super::SettingsView;
use super::desktop::note;
use crate::assets::Icon;
use crate::backend::sounds::{max_size_label, SoundHub, SoundKind, SoundSettings, MAX_PERCENT};
use crate::theme;
use crate::ui::sounds::slider::slider;
use crate::ui::sounds::{self, SoundsUi};

const UPLOAD: &[u8] = include_bytes!("../../../../assets/icons/upload-simple.svg");
const TRASH: &[u8] = include_bytes!("../../../../assets/icons/trash.svg");
const SPEAKER: &[u8] = include_bytes!("../../../../assets/icons/speaker-high.svg");
const SPEAKER_OFF: &[u8] = include_bytes!("../../../../assets/icons/speaker-x.svg");
const RESET: &[u8] = include_bytes!("../../../../assets/icons/arrow-counter-clockwise.svg");

/// The name of a sound in the lists.
fn label_of(kind: SoundKind) -> String {
    match kind {
        SoundKind::Message => t!("sound-message"),
        SoundKind::DirectMessage => t!("sound-direct-message"),
        SoundKind::SameChannelMessage => t!("sound-same-channel-message"),
        SoundKind::Mute => t!("sound-mute"),
        SoundKind::Unmute => t!("sound-unmute"),
        SoundKind::Deaf => t!("sound-deaf"),
        SoundKind::Undeaf => t!("sound-undeaf"),
        SoundKind::UserJoin => t!("sound-user-join"),
        SoundKind::UserLeave => t!("sound-user-leave"),
        SoundKind::UserMove => t!("sound-user-move"),
        SoundKind::ViewerJoin => t!("sound-viewer-join"),
        SoundKind::ViewerLeave => t!("sound-viewer-leave"),
        SoundKind::VoiceDisconnect => t!("sound-voice-disconnect"),
        SoundKind::IncomingRing => t!("sound-incoming-ring"),
        SoundKind::CameraOn => t!("sound-camera-on"),
        SoundKind::CameraOff => t!("sound-camera-off"),
        SoundKind::ScreenShareStart => t!("sound-screen-share-start"),
        SoundKind::ScreenShareStop => t!("sound-screen-share-stop"),
    }
}

fn icon(data: &'static [u8], size: f32, color: Hsla) -> Svg {
    svg()
        .data(data)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A small button with a picture. One that is not enabled is dimmed and does nothing.
fn icon_button(id: SharedString, data: &'static [u8], color: Hsla, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .p_1()
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| {
            d.cursor_pointer().hover(|s| s.bg(theme::hover_bg()))
        })
        .when(!enabled, |d| d.opacity(0.4))
        .child(icon(data, 16., color))
}

/// A row with a check box, like the Desktop tab's, with an ID made at run time.
fn switch_row(id: SharedString, label: String, on: bool, enabled: bool) -> Stateful<Div> {
    let mark = if on {
        Icon::CheckSquare
            .el(px(20.), theme::brand())
            .into_any_element()
    } else {
        div()
            .size(px(20.))
            .flex_none()
            .rounded(px(4.))
            .border_2()
            .border_color(theme::text_muted())
            .into_any_element()
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .py_1()
        .when(enabled, |d| d.cursor_pointer())
        .when(!enabled, |d| d.opacity(0.5))
        .child(mark)
        .child(div().text_color(theme::text_strong()).child(label))
}

fn percent(v: u16) -> String {
    t!("settings-sounds-percent", percent = v)
}

/// The section. Empty before the sound hub exists (tests).
pub(super) fn section(cx: &mut Context<SettingsView>) -> AnyElement {
    let Some(hub) = sounds::hub(cx) else {
        return div().into_any_element();
    };
    let settings = hub.settings();
    let (show_all, open, message) = {
        let ui = cx.global_mut::<SoundsUi>();
        let open = *ui
            .overrides_open
            .get_or_insert(settings.override_count() > 0);
        (ui.show_all, open, ui.message.clone())
    };
    let all_off = settings.all_disabled;

    let mut col = div().flex().flex_col().gap_2().pt_2();

    // The master volume.
    col = col
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text_strong())
                        .child(t!("settings-sounds-master")),
                )
                .child({
                    let hub = hub.clone();
                    let can_reset =
                        !all_off && settings.master != crate::backend::sounds::DEFAULT_MASTER;
                    icon_button(
                        SharedString::from("settings-sounds-master-reset"),
                        RESET,
                        theme::text_muted(),
                        can_reset,
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        if can_reset {
                            hub.edit(|s| s.set_master(crate::backend::sounds::DEFAULT_MASTER));
                            window.refresh();
                            cx.notify();
                        }
                    }))
                }),
        )
        .child(note(t!("settings-sounds-master-note")).pl_0())
        .child({
            let hub = hub.clone();
            slider(
                "settings-sounds-master-slider",
                settings.master,
                MAX_PERCENT,
                &[0, 50, 100, 150, 200],
                percent,
                !all_off,
                move |v, window, _| {
                    hub.edit(|s| s.set_master(v));
                    window.refresh();
                },
            )
        });

    // One switch for every sound.
    col = col
        .child({
            let hub = hub.clone();
            switch_row(
                SharedString::from("settings-sounds-all-off"),
                t!("settings-sounds-disable-all"),
                all_off,
                true,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                hub.edit(|s| s.all_disabled = !all_off);
                if !all_off {
                    hub.stop_all();
                }
                cx.notify();
            }))
        })
        .child(note(t!("settings-sounds-disable-all-note")))
        .child(note(t!("settings-sounds-custom-note", size = max_size_label())).pl_0());
    if let Some(message) = message {
        col = col.child(
            div()
                .text_size(px(13.))
                .text_color(theme::danger())
                .child(message),
        );
    }

    // The list: the common sounds, and the others behind a button.
    let mut list = div().flex().flex_col();
    for kind in SoundKind::ALL
        .into_iter()
        .filter(|k| k.is_common() || show_all)
    {
        list = list.child(sound_row(kind, &settings, &hub, cx));
    }
    col = col.child(list).child(
        button_with_id(
            "settings-sounds-show-more",
            if show_all {
                t!("settings-sounds-show-fewer")
            } else {
                t!("settings-sounds-show-more")
            },
        )
        .on_click(cx.listener(move |_, _, _, cx| {
            let ui = cx.global_mut::<SoundsUi>();
            ui.show_all = !ui.show_all;
            cx.notify();
        })),
    );

    // The volume of each sound.
    let count = settings.override_count();
    col = col.child(
        div()
            .id("settings-sounds-per-sound")
            .mt_2()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_strong())
                            .child(t!("settings-sounds-per-sound")),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(if count > 0 {
                                t!("settings-sounds-per-sound-count", count = count)
                            } else {
                                t!("settings-sounds-per-sound-note")
                            }),
                    ),
            )
            .child(
                if open {
                    Icon::CaretDown
                } else {
                    Icon::CaretRight
                }
                .el(px(16.), theme::text_muted()),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                let ui = cx.global_mut::<SoundsUi>();
                ui.overrides_open = Some(!open);
                cx.notify();
            })),
    );
    if open {
        let mut volumes = div().flex().flex_col().gap_3().pb_4();
        for kind in SoundKind::ALL {
            volumes = volumes.child(volume_row(kind, &settings, &hub, cx));
        }
        if count > 0 && !all_off {
            let hub = hub.clone();
            volumes = volumes.child(
                div().flex().flex_row().child(
                    button_with_id("settings-sounds-reset-all", t!("settings-sounds-reset-all"))
                        .on_click(cx.listener(move |_, _, window, cx| {
                            hub.edit(SoundSettings::clear_overrides);
                            window.refresh();
                            cx.notify();
                        })),
                ),
            );
        }
        col = col.child(volumes);
    }
    col.into_any_element()
}

/// One line of the list: the switch, a "Custom" mark, preview, upload and remove.
fn sound_row(
    kind: SoundKind,
    settings: &SoundSettings,
    hub: &SoundHub,
    cx: &mut Context<SettingsView>,
) -> Div {
    let all_off = settings.all_disabled;
    let on = !all_off && settings.enabled(kind);
    let name = label_of(kind);
    let custom = hub.custom_name(kind).is_some();
    let id = |what: &str| SharedString::from(format!("settings-sound-{what}-{}", kind.id()));

    let switch = {
        let hub = hub.clone();
        switch_row(id("switch"), name.clone(), on, !all_off)
            .flex_1()
            .on_click(cx.listener(move |_, _, _, cx| {
                if !all_off {
                    hub.edit(|s| s.set_enabled(kind, !on));
                    cx.notify();
                }
            }))
    };
    let preview = {
        let hub = hub.clone();
        div()
            .id(id("preview"))
            .text_size(px(13.))
            .text_color(theme::brand())
            .when(!all_off, |d| d.cursor_pointer().hover(|s| s.opacity(0.8)))
            .when(all_off, |d| d.opacity(0.4))
            .child(t!("settings-sounds-preview"))
            .on_click(cx.listener(move |_, _, _, _| {
                if !all_off {
                    hub.preview(kind);
                }
            }))
    };
    let upload = icon_button(id("upload"), UPLOAD, theme::text_muted(), !all_off).on_click(
        cx.listener(move |_, _, _, cx| {
            if !all_off {
                sounds::pick_custom(kind, cx);
            }
        }),
    );
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(switch)
        .children(custom.then(|| {
            div()
                .px_2()
                .rounded(px(4.))
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .bg(theme::bg_textarea())
                .text_color(theme::text_muted())
                .child(t!("settings-sounds-custom-badge"))
        }))
        .child(preview)
        .child(upload);
    if custom {
        let hub = hub.clone();
        row = row.child(
            icon_button(id("remove"), TRASH, theme::danger(), !all_off).on_click(cx.listener(
                move |_, _, _, cx| {
                    if !all_off {
                        hub.remove_custom(kind);
                        cx.notify();
                    }
                },
            )),
        );
    }
    row
}

/// One entry of the per-sound volumes: name, mute button, level, reset, and the slider.
fn volume_row(
    kind: SoundKind,
    settings: &SoundSettings,
    hub: &SoundHub,
    cx: &mut Context<SettingsView>,
) -> Div {
    let all_off = settings.all_disabled;
    // The sound's own switch; "all off" dims the row but keeps what the switch says.
    let enabled = !settings.is_disabled(kind);
    let own = settings.override_of(kind);
    let shown = settings.percent(kind);
    let name = label_of(kind);
    let row_off = all_off || !enabled;
    let id = |what: &str| SharedString::from(format!("settings-volume-{what}-{}", kind.id()));

    let mute = {
        let hub = hub.clone();
        let now_on = enabled;
        icon_button(
            id("mute"),
            if now_on { SPEAKER } else { SPEAKER_OFF },
            if now_on {
                theme::text_muted()
            } else {
                theme::danger()
            },
            !all_off,
        )
        .on_click(cx.listener(move |_, _, _, cx| {
            if !all_off {
                hub.edit(|s| s.set_enabled(kind, !now_on));
                cx.notify();
            }
        }))
    };
    let reset = {
        let hub = hub.clone();
        let can = !row_off && own.is_some();
        icon_button(id("reset"), RESET, theme::text_muted(), can).on_click(cx.listener(
            move |_, _, window, cx| {
                if can {
                    hub.edit(|s| s.clear_override(kind));
                    window.refresh();
                    cx.notify();
                }
            },
        ))
    };
    let status = match own {
        Some(v) => percent(v),
        None => t!("settings-sounds-following-master", percent = shown),
    };
    let hub = hub.clone();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_color(if own.is_some() {
                            theme::text_strong()
                        } else {
                            theme::text_channel()
                        })
                        .child(name),
                )
                .child(mute)
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::text_muted())
                        .child(status),
                )
                .child(reset),
        )
        .child(slider(
            SharedString::from(format!("settings-volume-slider-{}", kind.id())),
            shown,
            MAX_PERCENT,
            &[],
            percent,
            !row_off,
            move |v, window, _| {
                hub.edit(|s| s.set_override(kind, v));
                window.refresh();
            },
        ))
}
