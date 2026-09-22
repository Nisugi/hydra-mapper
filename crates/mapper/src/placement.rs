//! Turning a drag into something that can travel.
//!
//! The store records a drag as a **cell**: a group's offset onto the sheet,
//! or a room's position inside its group's frame. That is the right shape
//! for redrawing it here, and the wrong shape for sending anywhere, because
//! a cell only means something in the solve it was measured against.
//!
//! What travels instead is the same drag stated as a **relationship**: this
//! room sits `(dx, dy)` from *that* room. Both ends are uids, so the pair
//! survives a rebuild that renumbers every room id in the map; and the
//! offset is a fact about two rooms rather than about one grid.
//!
//! # Why the anchor is a room that did not move
//!
//! The anchor is picked as the nearest room **outside the moved set**. If
//! it were merely the nearest room, a drag could anchor to another dragged
//! room, and the two offsets would compound: the consumer applies the
//! anchor's own correction, then measures from where it now is, and lands
//! somewhere neither correction asked for. An unmoved anchor is at the same
//! cell before and after, so the offset means the same thing whether it is
//! applied to a corrected layout or a fresh one.
//!
//! This is computed **here, at edit time**, where the whole solve is in
//! front of us -- not derived downstream from a file that has lost it.

use std::collections::{BTreeMap, BTreeSet};

use cena_map::{Map, RoomId};
use cena_map_layout::{Cell, Layout};
use serde::{Deserialize, Serialize};

use crate::overrides::{LocationOverrides, RoomKey};

/// One room's placement, as a relationship to a room that did not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// The uid of the room this offset is measured from.
    pub anchor: i64,
    /// Cells east of the anchor (negative is west).
    pub dx: i32,
    /// Cells south of the anchor (negative is north), matching the grid's
    /// own axis, where y grows downward.
    pub dy: i32,
}

/// A placement the editor worked out, with the rooms still named as this
/// map names them so the inspector can show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub room: RoomId,
    pub anchor: RoomId,
    pub dx: i32,
    pub dy: i32,
}

/// Every moved room's placement, measured against the nearest room that
/// did not move.
///
/// `layout` must be the **corrected** layout -- the one with `apply`
/// already run -- because the point is to record where the rooms ended up,
/// not where the solver first put them.
///
/// A moved room with no unmoved room anywhere in its area yields nothing:
/// there is no relationship to state, and inventing one against a room
/// that itself moved would compound two corrections.
#[must_use]
pub fn resolve(layout: &Layout, map: &Map, ov: &LocationOverrides) -> Vec<Resolved> {
    let moved = moved_rooms(layout, map, ov);
    if moved.is_empty() {
        return Vec::new();
    }

    // Every room's final cell, and the candidate anchors among them.
    let mut cells: BTreeMap<RoomId, Cell> = BTreeMap::new();
    for group in &layout.groups {
        if group.base_offset.is_none() {
            continue;
        }
        for &id in &group.room_ids {
            cells.insert(id, group.final_cell(id));
        }
    }
    let anchors: Vec<(RoomId, Cell)> = cells
        .iter()
        .filter(|(id, _)| !moved.contains(*id))
        .map(|(&id, &c)| (id, c))
        .collect();
    if anchors.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for &room in &moved {
        let Some(&at) = cells.get(&room) else {
            continue;
        };
        // Nearest by Chebyshev, the grid's own notion of distance -- the
        // same one the hill climb costs edges with. Ties break on room id
        // so the choice is stable across runs rather than hash order.
        let Some(&(anchor, from)) = anchors
            .iter()
            .min_by_key(|(id, c)| ((c.x - at.x).abs().max((c.y - at.y).abs()), id.0))
        else {
            continue;
        };
        out.push(Resolved {
            room,
            anchor,
            dx: at.x - from.x,
            dy: at.y - from.y,
        });
    }
    out
}

/// The rooms a person moved: every room of an offset group, plus every
/// pinned room.
///
/// A group offset moves all of its rooms, not just the anchor the offset
/// is keyed by -- so all of them are ineligible as anchors, and all of
/// them need a placement to be reproduced.
fn moved_rooms(layout: &Layout, map: &Map, ov: &LocationOverrides) -> BTreeSet<RoomId> {
    let mut moved = BTreeSet::new();
    for group in &layout.groups {
        let Some(key) = RoomKey::anchor(group, map) else {
            continue;
        };
        if ov.group_offsets.contains_key(&key) {
            moved.extend(group.room_ids.iter().copied());
        }
    }
    if !ov.room_pins.is_empty() {
        for group in &layout.groups {
            for &id in &group.room_ids {
                if ov.room_pins.contains_key(&RoomKey::of(id, map)) {
                    moved.insert(id);
                }
            }
        }
    }
    moved
}

/// The uid-keyed placements for the export, and whichever could not be
/// named across a rebuild.
///
/// Both ends must have a uid: an offset measured from a room the combiner
/// cannot identify is not a relationship it can reproduce.
#[must_use]
pub fn for_export(resolved: &[Resolved], map: &Map) -> (BTreeMap<i64, Placement>, Vec<Resolved>) {
    let uid = |id: RoomId| map.room(id).and_then(|r| r.uid.first()).map(|u| u.0);
    let mut out = BTreeMap::new();
    let mut skipped = Vec::new();
    for r in resolved {
        match (uid(r.room), uid(r.anchor)) {
            (Some(room), Some(anchor)) => {
                out.insert(
                    room,
                    Placement {
                        anchor,
                        dx: r.dx,
                        dy: r.dy,
                    },
                );
            }
            _ => skipped.push(r.clone()),
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map_layout::generate_layout;

    fn room(id: u32, uid: Option<i64>, exits: Vec<cena_map::Exit>) -> cena_map::Room {
        cena_map::Room {
            id: RoomId(id),
            uid: uid.map(cena_map::Uid).into_iter().collect(),
            title: vec![format!("[Room {id}]")],
            description: vec![],
            paths: vec![],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits,
        }
    }

    fn exit(to: u32, command: &str) -> cena_map::Exit {
        cena_map::Exit {
            to: RoomId(to),
            kind: cena_map::ExitKind::Cardinal,
            crossing: cena_map::Crossing::Command(command.to_owned()),
            cost: Some(cena_map::Cost::Fixed(1.0)),
        }
    }

    /// Three rooms in a row, all with uids.
    fn row() -> Map {
        Map::from_rooms(vec![
            room(1, Some(7_000_001), vec![exit(2, "east")]),
            room(2, Some(7_000_002), vec![exit(1, "west"), exit(3, "east")]),
            room(3, Some(7_000_003), vec![exit(2, "west")]),
        ])
        .expect("no duplicate ids")
    }

    /// A pinned room is measured from a room that did not move, and the
    /// offset is the real cell delta between them.
    #[test]
    fn a_pin_becomes_an_offset_from_an_unmoved_room() {
        let map = row();
        let mut layout = generate_layout(&map);
        let mut ov = LocationOverrides::default();

        // Move room 3 two cells down from wherever the solver put it.
        let was = layout.groups[0].final_cell(RoomId(3));
        let inside = layout.groups[0].positions[&RoomId(3)];
        ov.room_pins.insert(
            RoomKey::of(RoomId(3), &map),
            Cell {
                x: inside.x,
                y: inside.y + 2,
            },
        );
        crate::overrides::apply(&mut layout, &map, &ov);
        assert_eq!(layout.groups[0].final_cell(RoomId(3)).y, was.y + 2);

        let resolved = resolve(&layout, &map, &ov);
        assert_eq!(resolved.len(), 1, "expected one placement");
        let r = &resolved[0];
        assert_eq!(r.room, RoomId(3));
        assert_ne!(r.anchor, RoomId(3), "a room cannot anchor to itself");

        // The offset must actually name where the room now sits.
        let anchor_at = layout.groups[0].final_cell(r.anchor);
        let room_at = layout.groups[0].final_cell(RoomId(3));
        assert_eq!(
            (r.dx, r.dy),
            (room_at.x - anchor_at.x, room_at.y - anchor_at.y)
        );
    }

    /// The anchor is never another moved room: two rooms dragged together
    /// must both measure from something that stayed put, or their offsets
    /// would compound when applied.
    #[test]
    fn an_anchor_is_never_a_room_that_moved() {
        let map = row();
        let mut layout = generate_layout(&map);
        let mut ov = LocationOverrides::default();
        for id in [RoomId(2), RoomId(3)] {
            let inside = layout.groups[0].positions[&id];
            ov.room_pins.insert(
                RoomKey::of(id, &map),
                Cell {
                    x: inside.x,
                    y: inside.y + 3,
                },
            );
        }
        crate::overrides::apply(&mut layout, &map, &ov);

        let resolved = resolve(&layout, &map, &ov);
        assert_eq!(resolved.len(), 2);
        for r in &resolved {
            assert_eq!(
                r.anchor,
                RoomId(1),
                "anchored to a room that was itself moved"
            );
        }
    }

    /// A group offset moves every room in the group, so every one of them
    /// gets a placement -- not just the anchor the offset is keyed by.
    #[test]
    fn a_group_offset_places_every_room_in_the_group() {
        let map = row();
        let mut layout = generate_layout(&map);
        let mut ov = LocationOverrides::default();
        let anchor = RoomKey::anchor(&layout.groups[0], &map).expect("group has an anchor");
        ov.group_offsets.insert(anchor, Cell { x: 5, y: 0 });
        crate::overrides::apply(&mut layout, &map, &ov);

        // Every room moved, so nothing is left to anchor to.
        let resolved = resolve(&layout, &map, &ov);
        assert!(
            resolved.is_empty(),
            "invented an anchor when every room had moved"
        );
    }

    /// Nothing moved, nothing to say.
    #[test]
    fn an_unedited_layout_yields_no_placements() {
        let map = row();
        let layout = generate_layout(&map);
        assert!(resolve(&layout, &map, &LocationOverrides::default()).is_empty());
    }

    /// Both ends need a uid: an offset from a room the combiner cannot
    /// name is not one it can reproduce.
    #[test]
    fn a_placement_needs_a_uid_at_both_ends() {
        let map = Map::from_rooms(vec![
            room(1, None, vec![exit(2, "east")]),
            room(2, Some(7_000_002), vec![exit(1, "west")]),
        ])
        .expect("no duplicate ids");

        let resolved = vec![Resolved {
            room: RoomId(2),
            anchor: RoomId(1),
            dx: 3,
            dy: -2,
        }];
        let (out, skipped) = for_export(&resolved, &map);
        assert!(out.is_empty(), "exported an anchor with no uid");
        assert_eq!(skipped.len(), 1);
    }

    /// The uid form carries the same numbers the editor worked out.
    #[test]
    fn the_export_form_keeps_the_offset() {
        let map = row();
        let resolved = vec![Resolved {
            room: RoomId(3),
            anchor: RoomId(1),
            dx: 3,
            dy: -2,
        }];
        let (out, skipped) = for_export(&resolved, &map);
        assert!(skipped.is_empty());
        assert_eq!(
            out[&7_000_003],
            Placement {
                anchor: 7_000_001,
                dx: 3,
                dy: -2
            }
        );
    }
}
