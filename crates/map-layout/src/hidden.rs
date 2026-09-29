//! The rooms left off the sheet. The author, 2026-09-29: *"We leave all
//! indoor rooms without terrain off the map. their room that leads to an
//! outside room, shows up on the map as a dot like it does now, the rest of
//! them are hidden on the indoor sheet not to be shown."*
//!
//! Indoors is "Obvious exits" ([`room_sense`]); no terrain is none recorded,
//! or `none`. A doorway -- an indoor room with an exit to or from a room
//! drawn in its own right -- stays, and is hung beside its street as a
//! one-room building is. The rest are laid out only so the packer can see
//! what they join: two streets whose one link runs through a building are
//! still packed side by side (`packer::add_bridged_edges`), and then the
//! hidden rooms are dropped from the layout.
//!
//! Measured on gs.map with the gate (`--quality-check`), events aside,
//! 31,787 rooms drawn before and 18,209 after: exits against their
//! direction 454 -> 180, lines through rooms 597 -> 48, building rooms
//! under a line not theirs 291 -> 32, directionless lines crossing another
//! 1,876 -> 674 (18% of them -> 12%).

use std::collections::HashSet;

use cena_map::{Map, Room, RoomId};

use crate::classifier::{Sense, room_sense};
use crate::regions::is_passage;

/// Indoors with no terrain: drawn only as its doorway, if it is one.
fn indoors_bare(room: &Room) -> bool {
    room_sense(room) == Sense::Indoor && matches!(room.terrain.as_deref(), None | Some("" | "none"))
}

/// The rooms of `map` left off the sheet: indoors with no terrain, and no
/// exit to or from a room drawn in its own right, or out of the selection
/// (another area's street). A building reached only by a teleport or a
/// routine has no doorway and is hidden whole. None at all when the
/// selection has no outdoor room: a guild, a castle, Zul Logoth, all
/// indoors, is drawn whole.
#[must_use]
pub fn hidden_rooms(map: &Map) -> HashSet<RoomId> {
    if !map.rooms().iter().any(|r| room_sense(r) == Sense::Outdoor) {
        return HashSet::new();
    }
    let mut doorways: HashSet<RoomId> = HashSet::new();
    for room in map.rooms() {
        let bare = indoors_bare(room);
        for exit in room.exits.iter().filter(|e| is_passage(e)) {
            // A door onto a room outside the selection is a door onto
            // another area's street: this one cannot see the street, but
            // the building is still entered from it.
            let Some(to) = map.room(exit.to) else {
                if bare {
                    doorways.insert(room.id);
                }
                continue;
            };
            match (bare, indoors_bare(to)) {
                (true, false) => {
                    doorways.insert(room.id);
                }
                // A door in from the street that leaves by another way.
                (false, true) => {
                    doorways.insert(to.id);
                }
                _ => {}
            }
        }
    }
    map.rooms()
        .iter()
        .filter(|r| indoors_bare(r) && !doorways.contains(&r.id))
        .map(|r| r.id)
        .collect()
}

/// `map`'s rooms split into those drawn and those hidden, as two maps.
/// Exits between the two stay on the rooms; each map reads an exit to a
/// room it does not hold as leaving the selection.
#[must_use]
pub fn split(map: &Map, hidden: &HashSet<RoomId>) -> (Map, Map) {
    let (drawn, left): (Vec<Room>, Vec<Room>) = map
        .rooms()
        .iter()
        .cloned()
        .partition(|r| !hidden.contains(&r.id));
    let whole =
        |rooms| Map::from_rooms(rooms).unwrap_or_else(|_| unreachable!("a subset of unique ids"));
    (whole(drawn), whole(left))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Cost, Crossing, Exit, ExitKind};

    fn room(id: u32, paths: &str, terrain: Option<&str>, exits: &[(u32, &str)]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![],
            description: vec![],
            paths: vec![paths.to_owned()],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: terrain.map(str::to_owned),
            tags: vec![],
            meta: vec![],
            image: None,
            exits: exits
                .iter()
                .map(|&(to, cmd)| Exit {
                    to: RoomId(to),
                    kind: ExitKind::Other,
                    crossing: Crossing::Command(cmd.to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    /// The Hinterwilds: a street, the pits' doorway off it and a pit behind
    /// that, a hut with terrain, and a door in from the street whose room
    /// leaves by another way. The pit is hidden; the doorways, the hut and
    /// the street are drawn.
    #[test]
    fn only_the_rooms_behind_a_doorway_are_hidden() {
        let map = Map::from_rooms(vec![
            room(
                1,
                "Obvious paths: east",
                Some("barren scrub"),
                &[(2, "go steps"), (4, "go hut"), (5, "go gash")],
            ),
            room(
                2,
                "Obvious exits: north",
                None,
                &[(1, "go steps"), (3, "north")],
            ),
            room(
                3,
                "Obvious exits: south",
                Some("none"),
                &[(2, "south"), (5, "east")],
            ),
            room(4, "Obvious exits: out", Some("hard, flat"), &[(1, "out")]),
            room(5, "Obvious exits: west", None, &[(3, "west")]),
        ])
        .expect("no duplicate ids");

        let hidden = hidden_rooms(&map);
        assert_eq!(hidden, HashSet::from([RoomId(3)]));
    }

    /// A guild's own area is all indoors: nothing is hidden, or there is
    /// nothing to draw.
    #[test]
    fn a_selection_all_indoors_is_drawn_whole() {
        let map = Map::from_rooms(vec![
            room(1, "Obvious exits: north", None, &[(2, "north")]),
            room(2, "Obvious exits: south", None, &[(1, "south")]),
        ])
        .expect("no duplicate ids");

        assert!(hidden_rooms(&map).is_empty());
    }
}
