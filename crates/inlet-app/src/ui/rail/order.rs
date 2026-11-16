// SPDX-License-Identifier: AGPL-3.0-or-later
//! The order of the guild rail and its folders, as the account settings keep it
//! (`guild_folders`, shared with every Fluxer client).
//!
//! The settings hold a list of entries: a folder with an ID and its guilds, or an
//! entry with ID -1 that holds a run of guilds outside any folder. [`organize`] turns
//! that and the guilds the account has into what the rail shows; the drop functions turn
//! a drag into the new list to save. All of it is pure, so the order is tested without a
//! screen. The rules follow Fluxer's `GuildList` and `GuildsLayout`:
//!
//! - a guild the list does not name shows first (newest first, as Fluxer does), and a
//!   name the account does not have is skipped;
//! - a guild is shown once, a folder without guilds is not shown;
//! - dropping a guild on another guild makes a folder with the next free ID;
//! - dropping a guild on a folder puts it last in it; a folder is never inside a folder.

use inlet_protocol::api::folders::UNCATEGORIZED_FOLDER_ID;
use inlet_protocol::models::GuildFolder;

/// What the rail shows at the top level, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Organized {
    /// A guild outside any folder.
    Guild(String),
    /// A folder and the guilds it shows (the ones the account has, in order).
    Folder {
        folder: GuildFolder,
        guilds: Vec<String>,
    },
}

impl Organized {
    /// The key a drag and a drop name this entry by: the guild ID, or `folder-<id>`.
    pub fn key(&self) -> String {
        match self {
            Organized::Guild(id) => id.clone(),
            Organized::Folder { folder, .. } => folder_key(folder.id.unwrap_or(0)),
        }
    }
}

/// The key of a folder in a drag.
pub fn folder_key(id: i64) -> String {
    format!("folder-{id}")
}

/// What is dragged: a guild (with the folder it sits in, if any) or a whole folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Guild { id: String, folder: Option<i64> },
    Folder(i64),
}

impl Source {
    fn key(&self) -> String {
        match self {
            Source::Guild { id, .. } => id.clone(),
            Source::Folder(id) => folder_key(*id),
        }
    }
}

/// What it is dropped on: a top-level guild, a guild inside a folder, or a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Guild { id: String, folder: Option<i64> },
    Folder(i64),
}

impl Target {
    fn key(&self) -> String {
        match self {
            Target::Guild { id, .. } => id.clone(),
            Target::Folder(id) => folder_key(*id),
        }
    }
}

/// Where in the target the drop lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Before,
    After,
    /// Onto a folder: last in it.
    Inside,
    /// Onto a guild outside a folder: the two make a folder.
    Combine,
}

fn is_uncategorized(f: &GuildFolder) -> bool {
    f.id.is_none_or(|id| id == UNCATEGORIZED_FOLDER_ID)
}

/// What the rail shows: `guilds` are the IDs and names of the guilds the account has.
pub fn organize(folders: &[GuildFolder], guilds: &[(String, String)]) -> Vec<Organized> {
    let known: std::collections::HashMap<&str, &str> = guilds
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str()))
        .collect();
    let mut placed = std::collections::HashSet::new();
    let mut emitted = std::collections::HashSet::new();
    let mut out = Vec::new();
    for folder in folders {
        let uncategorized = is_uncategorized(folder);
        if !uncategorized && emitted.contains(&folder.id) {
            continue;
        }
        let mut shown = Vec::new();
        for id in &folder.guild_ids {
            if known.contains_key(id.as_str()) && placed.insert(id.clone()) {
                shown.push(id.clone());
            }
        }
        if shown.is_empty() {
            continue;
        }
        if uncategorized {
            out.extend(shown.into_iter().map(Organized::Guild));
        } else {
            emitted.insert(folder.id);
            out.push(Organized::Folder {
                folder: folder.clone(),
                guilds: shown,
            });
        }
    }
    // Guilds the list does not name: by name, shown last to first.
    let mut loose: Vec<&(String, String)> = guilds
        .iter()
        .filter(|(id, _)| !placed.contains(id))
        .collect();
    loose.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    let mut front: Vec<Organized> = loose
        .into_iter()
        .rev()
        .map(|(id, _)| Organized::Guild(id.clone()))
        .collect();
    front.extend(out);
    front
}

/// The next free folder ID.
pub fn next_folder_id(folders: &[GuildFolder]) -> i64 {
    folders
        .iter()
        .filter_map(|f| f.id)
        .max()
        .map_or(1, |m| m.max(0) + 1)
}

fn without(folders: &[GuildFolder], gone: &str) -> Vec<GuildFolder> {
    folders
        .iter()
        .map(|f| {
            let mut f = f.clone();
            f.guild_ids.retain(|g| g != gone);
            f
        })
        .filter(|f| !f.guild_ids.is_empty())
        .collect()
}

/// A top-level entry of the list being edited.
#[derive(Clone, Debug)]
enum Top {
    Guild(String),
    Folder(GuildFolder),
}

fn tops(folders: &[GuildFolder]) -> Vec<Top> {
    let mut out = Vec::new();
    for f in folders {
        if is_uncategorized(f) {
            out.extend(f.guild_ids.iter().cloned().map(Top::Guild));
        } else if !f.guild_ids.is_empty() {
            out.push(Top::Folder(f.clone()));
        }
    }
    out
}

/// Runs of loose guilds become entries with the ID -1, as Fluxer writes them.
fn folders_of(tops: Vec<Top>) -> Vec<GuildFolder> {
    let mut out = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let flush = |run: &mut Vec<String>, out: &mut Vec<GuildFolder>| {
        if !run.is_empty() {
            out.push(GuildFolder::uncategorized(std::mem::take(run)));
        }
    };
    for t in tops {
        match t {
            Top::Guild(id) => run.push(id),
            Top::Folder(f) => {
                flush(&mut run, &mut out);
                if !f.guild_ids.is_empty() {
                    out.push(f);
                }
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

fn insert_at(place: Place, target_index: usize) -> usize {
    match place {
        Place::After => target_index + 1,
        _ => target_index,
    }
}

/// The list to save after `source` was dropped on `target`, or `None` when the drop
/// changes nothing (onto itself, onto something that is not there).
pub fn drop_on(
    folders: &[GuildFolder],
    shown: &[Organized],
    source: &Source,
    target: &Target,
    place: Place,
) -> Option<Vec<GuildFolder>> {
    if source.key() == target.key() {
        return None;
    }
    match (source, target, place) {
        // A guild onto a folder: last in it.
        (Source::Guild { id, .. }, Target::Folder(fid), Place::Inside) => {
            let mut list = without(folders, id);
            let folder = list.iter_mut().find(|f| f.id == Some(*fid))?;
            folder.guild_ids.push(id.clone());
            Some(list)
        }
        // A guild before or after a guild inside a folder.
        (
            Source::Guild { id, .. },
            Target::Guild {
                id: target_id,
                folder: Some(fid),
            },
            Place::Before | Place::After,
        ) => {
            let mut list = without(folders, id);
            let folder = list.iter_mut().find(|f| f.id == Some(*fid))?;
            match folder.guild_ids.iter().position(|g| g == target_id) {
                Some(at) => folder.guild_ids.insert(insert_at(place, at), id.clone()),
                None => folder.guild_ids.push(id.clone()),
            }
            Some(list)
        }
        // Two loose guilds make a folder where the target was.
        (
            Source::Guild { id, .. },
            Target::Guild {
                id: target_id,
                folder: None,
            },
            Place::Combine,
        ) => {
            let list = without(folders, id);
            let mut top = tops(&list);
            let at = top
                .iter()
                .position(|t| matches!(t, Top::Guild(g) if g == target_id))?;
            let folder = GuildFolder::new(
                next_folder_id(folders),
                None,
                vec![target_id.clone(), id.clone()],
            );
            top[at] = Top::Folder(folder);
            Some(folders_of(top))
        }
        // A guild out of a folder, before or after a folder.
        (
            Source::Guild {
                id,
                folder: Some(_),
            },
            Target::Folder(fid),
            Place::Before | Place::After,
        ) => {
            let original = tops(folders);
            let original_at = original
                .iter()
                .position(|t| matches!(t, Top::Folder(f) if f.id == Some(*fid)))?;
            let mut top = tops(&without(folders, id));
            let at = top
                .iter()
                .position(|t| matches!(t, Top::Folder(f) if f.id == Some(*fid)))
                .unwrap_or(original_at.min(top.len()));
            top.insert(insert_at(place, at).min(top.len()), Top::Guild(id.clone()));
            Some(folders_of(top))
        }
        // A guild out of a folder, before or after a loose guild.
        (
            Source::Guild {
                id,
                folder: Some(_),
            },
            Target::Guild {
                id: target_id,
                folder: None,
            },
            Place::Before | Place::After,
        ) => {
            let mut top = tops(&without(folders, id));
            let at = top
                .iter()
                .position(|t| matches!(t, Top::Guild(g) if g == target_id))?;
            top.insert(insert_at(place, at), Top::Guild(id.clone()));
            Some(folders_of(top))
        }
        // Everything else moves a top-level entry (a loose guild or a folder).
        (_, _, Place::Before | Place::After) => reorder(shown, source, target, place),
        _ => None,
    }
}

fn reorder(
    shown: &[Organized],
    source: &Source,
    target: &Target,
    place: Place,
) -> Option<Vec<GuildFolder>> {
    let (skey, tkey) = (source.key(), target.key());
    let old = shown.iter().position(|o| o.key() == skey)?;
    let to = shown.iter().position(|o| o.key() == tkey)?;
    let to = if old < to { to - 1 } else { to };
    let new = insert_at(place, to);
    let mut items: Vec<&Organized> = shown.iter().collect();
    let moved = items.remove(old);
    items.insert(new.min(items.len()), moved);
    Some(folders_of(
        items
            .into_iter()
            .map(|o| match o {
                Organized::Guild(id) => Top::Guild(id.clone()),
                Organized::Folder { folder, guilds } => {
                    // What is saved is what is shown: a guild the account no longer has
                    // is dropped from its folder here.
                    let mut f = folder.clone();
                    f.guild_ids = guilds.clone();
                    Top::Folder(f)
                }
            })
            .collect(),
    ))
}

/// Takes a folder away; its guilds stay where it was, outside any folder.
pub fn delete_folder(folders: &[GuildFolder], id: i64) -> Vec<GuildFolder> {
    let mut top = Vec::new();
    for f in folders {
        if f.id == Some(id) || is_uncategorized(f) {
            top.extend(f.guild_ids.iter().cloned().map(Top::Guild));
        } else {
            top.push(Top::Folder(f.clone()));
        }
    }
    folders_of(top)
}

/// Changes a folder's name, colour, icon and flags.
pub fn edit_folder(
    folders: &[GuildFolder],
    id: i64,
    name: Option<String>,
    color: Option<u64>,
    icon: &str,
    flags: u64,
) -> Vec<GuildFolder> {
    folders
        .iter()
        .map(|f| {
            let mut f = f.clone();
            if f.id == Some(id) {
                f.name = name.clone().filter(|n| !n.trim().is_empty());
                f.color = color.filter(|c| *c != 0);
                f.icon = icon.to_owned();
                f.flags = flags;
            }
            f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use inlet_protocol::models::UserSettings;

    fn g(ids: &[&str]) -> Vec<(String, String)> {
        ids.iter().map(|i| ((*i).into(), format!("g{i}"))).collect()
    }

    fn loose(ids: &[&str]) -> GuildFolder {
        GuildFolder::uncategorized(ids.iter().map(|s| (*s).into()).collect())
    }

    fn folder(id: i64, ids: &[&str]) -> GuildFolder {
        GuildFolder::new(id, Some("Games"), ids.iter().map(|s| (*s).into()).collect())
    }

    fn keys(shown: &[Organized]) -> Vec<String> {
        shown.iter().map(Organized::key).collect()
    }

    fn ids_of(list: &[GuildFolder]) -> Vec<(Option<i64>, Vec<String>)> {
        list.iter().map(|f| (f.id, f.guild_ids.clone())).collect()
    }

    #[test]
    fn folder_order_round_trips_through_user_settings() {
        let guilds = g(&["1", "2", "3", "4", "5"]);
        let saved = vec![loose(&["3", "1"]), folder(2, &["5", "4"]), loose(&["2"])];
        let shown = organize(&saved, &guilds);
        assert_eq!(keys(&shown), ["3", "1", "folder-2", "2"]);

        // The settings the server sends back carry the same list.
        let json = serde_json::json!({ "guild_folders": saved });
        let settings: UserSettings = serde_json::from_value(json).unwrap();
        assert_eq!(settings.guild_folders, saved);
        assert_eq!(organize(&settings.guild_folders, &guilds), shown);

        // Moving a guild and saving it again keeps what the other clients see: the
        // order of `guild_order` is the order of the rail, folder by folder.
        let target = Target::Guild {
            id: "2".into(),
            folder: None,
        };
        let source = Source::Guild {
            id: "3".into(),
            folder: None,
        };
        let moved = drop_on(&saved, &shown, &source, &target, Place::After).unwrap();
        let again: UserSettings =
            serde_json::from_value(serde_json::json!({ "guild_folders": moved })).unwrap();
        let reshown = organize(&again.guild_folders, &guilds);
        assert_eq!(keys(&reshown), ["1", "folder-2", "2", "3"]);
        assert_eq!(
            inlet_protocol::api::folders::guild_order(&again.guild_folders),
            ["1", "5", "4", "2", "3"]
        );
        // Loose guilds are written as runs with the ID -1, folders keep theirs.
        assert_eq!(
            ids_of(&again.guild_folders),
            [
                (Some(-1), vec!["1".into()]),
                (Some(2), vec!["5".into(), "4".into()]),
                (Some(-1), vec!["2".into(), "3".into()])
            ]
        );
    }

    #[test]
    fn guilds_the_list_does_not_name_show_first_and_unknown_ones_are_skipped() {
        let guilds = vec![
            ("1".into(), "Zeta".into()),
            ("2".into(), "Alpha".into()),
            ("3".into(), "Mid".into()),
        ];
        let shown = organize(&[loose(&["1", "99", "1"]), folder(4, &["99"])], &guilds);
        // Two guilds are not named; they come first, last name first.
        assert_eq!(keys(&shown), ["3", "2", "1"]);
        // A guild is shown once even when two entries name it, and an empty folder goes.
        let shown = organize(&[folder(1, &["1", "2"]), folder(2, &["2", "3"])], &guilds);
        assert_eq!(keys(&shown), ["folder-1", "folder-2"]);
        match &shown[1] {
            Organized::Folder { guilds, .. } => assert_eq!(guilds, &["3"]),
            _ => panic!("a folder"),
        }
    }

    #[test]
    fn dropping_guilds_makes_folders_and_moves_them_in_and_out() {
        let guilds = g(&["1", "2", "3", "4"]);
        let list = vec![loose(&["1", "2", "3", "4"])];
        let shown = organize(&list, &guilds);
        let src = |id: &str| Source::Guild {
            id: id.into(),
            folder: None,
        };
        let on = |id: &str| Target::Guild {
            id: id.into(),
            folder: None,
        };
        // 1 onto 3 makes folder 1 where 3 was.
        let made = drop_on(&list, &shown, &src("1"), &on("3"), Place::Combine).unwrap();
        assert_eq!(
            ids_of(&made),
            [
                (Some(-1), vec!["2".into()]),
                (Some(1), vec!["3".into(), "1".into()]),
                (Some(-1), vec!["4".into()])
            ]
        );
        // 4 onto the folder: last in it.
        let shown = organize(&made, &guilds);
        let into = drop_on(&made, &shown, &src("4"), &Target::Folder(1), Place::Inside).unwrap();
        assert_eq!(into[1].guild_ids, ["3", "1", "4"]);
        // 4 out of it, before 2.
        let shown = organize(&into, &guilds);
        let out = drop_on(
            &into,
            &shown,
            &Source::Guild {
                id: "4".into(),
                folder: Some(1),
            },
            &on("2"),
            Place::Before,
        )
        .unwrap();
        assert_eq!(
            ids_of(&out),
            [
                (Some(-1), vec!["4".into(), "2".into()]),
                (Some(1), vec!["3".into(), "1".into()])
            ]
        );
        // 1 within folder 1, before 3.
        let shown = organize(&into, &guilds);
        let within = drop_on(
            &into,
            &shown,
            &Source::Guild {
                id: "1".into(),
                folder: Some(1),
            },
            &Target::Guild {
                id: "3".into(),
                folder: Some(1),
            },
            Place::Before,
        )
        .unwrap();
        assert_eq!(within[1].guild_ids, ["1", "3", "4"]);
        // A folder moved after 2; a drop on itself or on nothing changes nothing.
        let shown = organize(&made, &guilds);
        let moved = drop_on(&made, &shown, &Source::Folder(1), &on("2"), Place::Before).unwrap();
        assert_eq!(keys(&organize(&moved, &guilds)), ["folder-1", "2", "4"]);
        assert!(drop_on(&made, &shown, &src("2"), &on("2"), Place::Before).is_none());
        assert!(drop_on(&made, &shown, &src("2"), &Target::Folder(9), Place::Inside).is_none());
    }

    #[test]
    fn deleting_and_editing_a_folder_keeps_the_guilds() {
        let list = vec![loose(&["1"]), folder(2, &["2", "3"]), folder(3, &["4"])];
        let deleted = delete_folder(&list, 2);
        assert_eq!(
            ids_of(&deleted),
            [
                (Some(-1), vec!["1".into(), "2".into(), "3".into()]),
                (Some(3), vec!["4".into()])
            ]
        );
        let edited = edit_folder(&list, 3, Some("  ".into()), Some(0xFF0000), "star", 1);
        assert_eq!(edited[2].name, None);
        assert_eq!(edited[2].color, Some(0xFF0000));
        assert_eq!((edited[2].icon.as_str(), edited[2].flags), ("star", 1));
        assert_eq!(edited[1], list[1]);
        assert_eq!(next_folder_id(&list), 4);
        assert_eq!(next_folder_id(&[]), 1);
    }
}
