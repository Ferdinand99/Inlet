// SPDX-License-Identifier: AGPL-3.0-or-later
//! The guild rail on the far left: direct messages, the favourites star, then the
//! account's servers and folders in the order the account settings keep, then the
//! entries for exploring communities and adding one.
//!
//! A server or folder can be dragged: dropped on the upper or lower part of another
//! entry it moves before or after it, on the middle of a server it makes a folder with
//! it, on the middle of a folder it goes into it. The new order is shown at once and
//! saved to the account settings (`guild_folders`), where every Fluxer client reads it
//! (see `order` for the rules and `backend::settings::folders` for the request budget).
//! Which folders are open is remembered on this computer (see `sidebar::remembered`).

pub mod folder;
pub mod order;

use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, Render,
    SharedString, Stateful, Window,
};
use inlet_protocol::models::GuildFolder;

use crate::app::Inlet;
use crate::assets::Icon;
use crate::backend::settings::{folders as backend_folders, Cmd as SettingsCmd};
use crate::model::Guild;
use crate::theme;
use crate::ui::notifications::badge::{rail_badge, RailMarks};
use crate::ui::overlays::nav_dialog::{NavDialog, Soon};
use order::{Organized, Place, Source, Target};

/// Guild rail width.
pub const RAIL_WIDTH: f32 = 72.;
/// Height of one rail entry (`--guild-list-item-box-size`).
pub const ITEM_BOX: f32 = 48.;
/// How long a change that was sent shows before the account's own list takes over again
/// (a save that never came back must not hide what another client changed).
const PENDING_FOR: Duration = Duration::from_secs(10);

/// The rail's view state.
#[derive(Default)]
pub struct RailState {
    /// The list that was sent and has not come back yet, and when.
    pub pending: Option<(Vec<GuildFolder>, Instant)>,
}

/// What is being dragged: a server or a folder.
#[derive(Clone)]
pub struct Dragged {
    pub source: Source,
    label: SharedString,
}

/// The picture that follows the pointer.
struct DragPicture {
    label: SharedString,
}

impl Render for DragPicture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(44.))
            .rounded(px(13.))
            .bg(theme::brand())
            .opacity(0.9)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::text_strong())
            .child(self.label.clone())
    }
}

/// The 4 px pill on the left (8 px at rest when there are unread messages, 20 on hover,
/// 40 when selected) and a 44 px icon, a circle that becomes a rounded square, with the
/// mention count at its bottom right.
fn entry(
    id: SharedString,
    content: AnyElement,
    active: bool,
    marks: RailMarks,
    selector: &'static str,
) -> Stateful<Div> {
    let group = SharedString::from(format!("{id}-g"));
    div()
        .id(id)
        .debug_selector(move || selector.into())
        .group(group.clone())
        .relative()
        .w(px(RAIL_WIDTH))
        .h(px(ITEM_BOX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(if active { 4. } else { 20. }))
                .w(px(4.))
                .h(px(if active { 40. } else { 8. }))
                .rounded_r(px(4.))
                .bg(theme::text_strong())
                .when(!active, |d| {
                    d.opacity(if marks.pill { 1. } else { 0. })
                        .group_hover(group.clone(), |s| s.opacity(1.).top(px(14.)).h(px(20.)))
                }),
        )
        .child(
            div()
                .relative()
                .size(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(if active { 13. } else { 22. }))
                .bg(if active {
                    theme::brand()
                } else {
                    theme::guild_item()
                })
                .text_color(theme::text_strong())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(15.))
                .group_hover(group, |s| s.rounded(px(13.)).bg(theme::brand()))
                .child(content)
                .when(marks.mentions > 0, |d| {
                    d.child(rail_badge(marks.mentions, theme::bg_app()))
                }),
        )
}

/// The parts of an entry that take a drop: the upper and lower part move before or
/// after it, the middle combines (two servers) or goes inside (a folder).
fn zones(
    id: &str,
    target: Target,
    middle: Option<Place>,
    cx: &mut Context<Inlet>,
) -> Vec<AnyElement> {
    let zone = |name: &'static str, top: f32, height: f32, place: Place, target: Target| {
        let base = div()
            .id(SharedString::from(format!("{id}-{name}")))
            .absolute()
            .left_0()
            .w_full()
            .top(px(top))
            .h(px(height))
            .on_drop(cx.listener(move |this, d: &Dragged, _, cx| {
                this.rail_drop(d.source.clone(), target.clone(), place, cx)
            }));
        match place {
            Place::Before => base
                .drag_over::<Dragged>(|s, _, _, _| s.border_t_2().border_color(theme::brand()))
                .into_any_element(),
            Place::After => base
                .drag_over::<Dragged>(|s, _, _, _| s.border_b_2().border_color(theme::brand()))
                .into_any_element(),
            _ => base
                .drag_over::<Dragged>(|s, _, _, _| s.bg(theme::hover_bg()).rounded(px(13.)))
                .into_any_element(),
        }
    };
    let edge = if middle.is_some() { 14. } else { 24. };
    let mut out = vec![zone("before", 0., edge, Place::Before, target.clone())];
    if let Some(place) = middle {
        out.push(zone(
            "middle",
            edge,
            ITEM_BOX - 2. * edge,
            place,
            target.clone(),
        ));
    }
    out.push(zone("after", ITEM_BOX - edge, edge, Place::After, target));
    out
}

impl Inlet {
    /// The folders the rail shows: what was just sent, until it comes back, else the
    /// account's own.
    pub(crate) fn rail_folders(&self, cx: &gpui::App) -> Vec<GuildFolder> {
        if let Some((list, at)) = &self.rail.pending {
            if at.elapsed() < PENDING_FOR {
                return list.clone();
            }
        }
        self.account
            .as_ref()
            .map(|a| a.read(cx).settings().guild_folders().to_vec())
            .unwrap_or_default()
    }

    fn rail_organized(&self, cx: &gpui::App) -> Vec<Organized> {
        let guilds: Vec<(String, String)> = self
            .guilds
            .iter()
            .map(|g| (g.id.clone(), g.name.clone()))
            .collect();
        order::organize(&self.rail_folders(cx), &guilds)
    }

    /// Shows a new list at once and saves it.
    pub(crate) fn rail_save(&mut self, folders: Vec<GuildFolder>, cx: &mut Context<Self>) {
        self.rail.pending = Some((folders.clone(), Instant::now()));
        self.backend
            .send(SettingsCmd::Folders(backend_folders::Cmd::Save { folders }));
        cx.notify();
    }

    fn rail_drop(&mut self, source: Source, target: Target, place: Place, cx: &mut Context<Self>) {
        let folders = self.rail_folders(cx);
        let shown = self.rail_organized(cx);
        if let Some(next) = order::drop_on(&folders, &shown, &source, &target, place) {
            self.rail_save(next, cx);
        }
    }

    /// The list that came back from the server: what was sent is no longer pending
    /// once the account has it.
    pub(crate) fn rail_sync(&mut self, cx: &mut Context<Self>) {
        let theirs = self
            .account
            .as_ref()
            .map(|a| a.read(cx).settings().guild_folders().to_vec());
        if let (Some((sent, _)), Some(theirs)) = (&self.rail.pending, theirs) {
            if *sent == theirs {
                self.rail.pending = None;
            }
        }
        cx.notify();
    }

    /// Events of the guild order saves.
    pub(crate) fn on_folders_event(&mut self, event: backend_folders::Ev, cx: &mut Context<Self>) {
        match event {
            backend_folders::Ev::Saved(settings) => {
                if let Some(account) = self.account.clone() {
                    let update = inlet_protocol::Dispatch::UserSettingsUpdate(settings);
                    account.update(cx, |s, _| s.apply(&update));
                }
                self.rail_sync(cx);
            }
            backend_folders::Ev::Failed(reason) => {
                self.rail.pending = None;
                self.chat.error = Some(t!("rail-save-failed", error = reason.as_str()));
                cx.notify();
            }
        }
    }

    /// Whether a folder is open in the rail.
    pub(crate) fn folder_open(&self, id: i64) -> bool {
        self.sidebar
            .remembered
            .open_folders
            .contains(&id.to_string())
    }

    fn toggle_folder(&mut self, id: i64, cx: &mut Context<Self>) {
        let key = id.to_string();
        let remembered = &mut self.sidebar.remembered;
        if !remembered.open_folders.remove(&key) {
            remembered.open_folders.insert(&key);
        }
        remembered.save_in_background(cx);
        cx.notify();
    }

    /// A server's entry. `folder` is the folder it sits in.
    fn rail_guild(&mut self, g: &Guild, folder: Option<i64>, cx: &mut Context<Self>) -> AnyElement {
        let active = self.selected_guild.as_deref() == Some(g.id.as_str());
        let id = g.id.clone();
        // The guild's icon when it has one, rounded like the entry.
        let icon = self.guild_icon_image(&g.id, 44., cx).map(|i| {
            i.rounded(px(if active { 13. } else { 22. }))
                .group_hover(format!("guild-{}-g", g.id), |s| s.rounded(px(13.)))
                .into_any_element()
        });
        let dragged = Dragged {
            source: Source::Guild {
                id: g.id.clone(),
                folder,
            },
            label: g.initials().into(),
        };
        let target = Target::Guild {
            id: g.id.clone(),
            folder,
        };
        let base = format!("guild-{}", g.id);
        entry(
            base.clone().into(),
            icon.unwrap_or_else(|| g.initials().into_any_element()),
            active,
            RailMarks::of(self.guild_badge(Some(&g.id))),
            "rail-guild",
        )
        .on_click(cx.listener(move |this, _, _, cx| this.select_guild(Some(id.clone()), cx)))
        .on_drag(dragged, |d, _, _, cx| {
            let label = d.label.clone();
            cx.new(|_| DragPicture { label })
        })
        .children(zones(
            &base,
            target,
            folder.is_none().then_some(Place::Combine),
            cx,
        ))
        .into_any_element()
    }

    /// A folder: closed, a tile with its servers' icons; open, its icon over its servers
    /// on a tinted backing.
    fn rail_folder(
        &mut self,
        folder: &GuildFolder,
        guilds: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(fid) = folder.id else {
            return div().into_any_element();
        };
        let open = self.folder_open(fid);
        let members: Vec<Guild> = guilds
            .iter()
            .filter_map(|id| self.guilds.iter().find(|g| &g.id == id).cloned())
            .collect();
        let selected_inside = self
            .selected_guild
            .as_ref()
            .is_some_and(|s| guilds.contains(s));
        let color = folder::color_of(folder.color);
        // The marks of the servers inside: any unread, and the mentions added up.
        let mut marks = RailMarks::default();
        for id in guilds {
            let m = RailMarks::of(self.guild_badge(Some(id)));
            marks.pill |= m.pill;
            marks.mentions += m.mentions;
        }
        let face = if open || folder::shows_icon_when_closed(folder) || members.is_empty() {
            folder::icon_of(&folder.icon)
                .el(px(22.), color)
                .into_any_element()
        } else {
            self.folder_preview(&members, cx)
        };
        let base = format!("folder-{fid}");
        let head = entry(
            base.clone().into(),
            face,
            selected_inside && !open,
            marks,
            "rail-folder",
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(fid, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_folder_settings(fid, window, cx)
            }),
        )
        .on_drag(
            Dragged {
                source: Source::Folder(fid),
                label: SharedString::default(),
            },
            |d, _, _, cx| {
                let label = d.label.clone();
                cx.new(|_| DragPicture { label })
            },
        )
        .children(zones(&base, Target::Folder(fid), Some(Place::Inside), cx));
        if !open {
            return head.into_any_element();
        }
        let children: Vec<AnyElement> = members
            .iter()
            .map(|g| self.rail_guild(g, Some(fid), cx))
            .collect();
        div()
            .relative()
            .w(px(RAIL_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .bottom(px(2.))
                    .left(px(8.))
                    .right(px(8.))
                    .rounded(px(20.))
                    .bg(color.opacity(0.18)),
            )
            .child(head)
            .children(children)
            .into_any_element()
    }

    /// Up to four servers' icons in a two by two grid.
    fn folder_preview(&mut self, members: &[Guild], cx: &mut Context<Self>) -> AnyElement {
        let cells: Vec<AnyElement> = members
            .iter()
            .take(4)
            .map(|g| match self.guild_icon_image(&g.id, 18., cx) {
                Some(i) => i.rounded_full().into_any_element(),
                None => div()
                    .size(px(18.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme::bg_app())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .text_color(theme::text_muted())
                    .child(g.initials().chars().take(1).collect::<String>())
                    .into_any_element(),
            })
            .collect();
        div()
            .size(px(44.))
            .p(px(3.))
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(px(2.))
            .children(cells)
            .into_any_element()
    }

    /// An entry that is not a server: an icon in the entry's circle.
    fn rail_button(&self, id: &'static str, icon: Icon, selector: &'static str) -> Stateful<Div> {
        entry(
            id.into(),
            icon.el(px(22.), theme::text_strong()).into_any_element(),
            false,
            RailMarks::default(),
            selector,
        )
    }

    pub(crate) fn render_guild_rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_guild.clone();
        let shown = self.rail_organized(cx);
        let mut items: Vec<AnyElement> = Vec::new();
        for item in &shown {
            match item {
                Organized::Guild(id) => {
                    if let Some(g) = self.guilds.iter().find(|g| &g.id == id).cloned() {
                        items.push(self.rail_guild(&g, None, cx));
                    }
                }
                Organized::Folder { folder, guilds } => {
                    items.push(self.rail_folder(folder, guilds, cx));
                }
            }
        }
        // Dropping after the last entry: a strip under the list.
        let last = shown.last().map(|o| match o {
            Organized::Guild(id) => Target::Guild {
                id: id.clone(),
                folder: None,
            },
            Organized::Folder { folder, .. } => Target::Folder(folder.id.unwrap_or(0)),
        });
        let tail = last.map(|target| {
            div()
                .id("rail-tail")
                .w(px(RAIL_WIDTH))
                .h(px(16.))
                .flex_none()
                .drag_over::<Dragged>(|s, _, _, _| s.border_t_2().border_color(theme::brand()))
                .on_drop(cx.listener(move |this, d: &Dragged, _, cx| {
                    this.rail_drop(d.source.clone(), target.clone(), Place::After, cx)
                }))
        });
        div()
            .id("guild-rail")
            .debug_selector(|| "guild-rail".into())
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .pt_2()
            .bg(theme::bg_app())
            .overflow_y_scroll()
            .child(
                entry(
                    "home".into(),
                    Icon::ChatCircle
                        .el(px(22.), theme::text_strong())
                        .into_any_element(),
                    selected.is_none(),
                    RailMarks::of(self.guild_badge(None)),
                    "rail-home",
                )
                .on_click(cx.listener(|this, _, _, cx| this.select_guild(None, cx))),
            )
            .child(
                self.rail_button("rail-favorites", Icon::Star, "rail-extra")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav_show(NavDialog::Soon(Soon::Favorites), cx)
                    })),
            )
            .child(
                div()
                    .w(px(32.))
                    .h(px(2.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme::guild_item()),
            )
            .children(items)
            .children(tail)
            .child(
                self.rail_button("rail-discover", Icon::Compass, "rail-extra")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav_show(NavDialog::Soon(Soon::Discover), cx)
                    })),
            )
            .child(
                self.rail_button("rail-add", Icon::Plus, "rail-extra")
                    .on_click(cx.listener(|this, _, window, cx| this.open_add_server(window, cx))),
            )
    }
}
