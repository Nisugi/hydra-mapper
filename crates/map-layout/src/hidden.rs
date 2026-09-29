//! The rooms left off the sheet. The author, 2026-09-29: *"We leave all
//! indoor rooms without terrain off the map. their room that leads to an
//! outside room, shows up on the map as a dot like it does now, the rest of
//! them are hidden on the indoor sheet not to be shown."* Then, looking at
//! the towns: *"there should be more splitting to interior sheets"* -- so
//! the doorway goes too, and the street room it opens from carries the mark
//! of a way in ([`entrances`]) rather than a dot on a line beside it. In the
//! Landing's town the doorways were as many as the street rooms.
//!
//! Indoors is "Obvious exits" ([`room_sense`]); no terrain is none recorded,
//! or `none`. The hidden rooms are laid out only so the packer can see what
//! they join: two streets whose one link runs through a building are still
//! packed side by side (`packer::add_bridged_edges`), and then they are
//! dropped from the layout. Rooms reached only through hidden ones go with
//! them ([`reached_only_through`]).
//!
//! Measured on gs.map with the gate (`--quality-check`), events aside,
//! 31,787 rooms drawn before any of it and 18,209 with the doorways kept:
//! exits against their direction 454 -> 180, lines through rooms 597 ->
//! 48, building rooms under a line not theirs 291 -> 32, directionless
//! lines crossing another 1,876 -> 674 (18% of them -> 12%).

use std::collections::HashSet;

use cena_map::{Map, Room, RoomId};

use crate::classifier::{Sense, room_sense};
use crate::regions::is_passage;

/// Indoors with no terrain: on the indoor sheet, not this one.
fn indoors_bare(room: &Room) -> bool {
    room_sense(room) == Sense::Indoor && matches!(room.terrain.as_deref(), None | Some("" | "none"))
}

/// The rooms of `map` left off the sheet: indoors with no terrain, and what
/// is reached only through them. None at all when the selection has no
/// outdoor room: a guild, a castle, Zul Logoth, all indoors, is drawn
/// whole.
#[must_use]
pub fn hidden_rooms(map: &Map) -> HashSet<RoomId> {
    if !map.rooms().iter().any(|r| room_sense(r) == Sense::Outdoor) {
        return HashSet::new();
    }
    let mut hidden: HashSet<RoomId> = map
        .rooms()
        .iter()
        .filter(|r| indoors_bare(r))
        .map(|r| r.id)
        .collect();
    reached_only_through(map, &mut hidden);
    hidden
}

/// The drawn rooms a walk leads from into a hidden one: where a building
/// is entered, marked on the street as a way in.
#[must_use]
pub fn entrances(map: &Map, hidden: &HashSet<RoomId>) -> HashSet<RoomId> {
    map.rooms()
        .iter()
        .filter(|r| !hidden.contains(&r.id))
        .filter(|r| {
            r.exits
                .iter()
                .any(|e| is_passage(e) && hidden.contains(&e.to))
        })
        .map(|r| r.id)
        .collect()
}

/// Hide with them the rooms reached only through hidden ones: a cluster of
/// drawn rooms, not the area's biggest and not on Lich's picture, every
/// walk into or out of which is a hidden room's. Hinterwilds' Chthonian
/// Dark is entered only by `go arch` from the Pits of the Dead: part of
/// the pits' place, and in the way where it was drawn (the author, 2026-09-29).
fn reached_only_through(map: &Map, hidden: &mut HashSet<RoomId>) {
    let shown: Vec<RoomId> = map
        .rooms()
        .iter()
        .map(|r| r.id)
        .filter(|id| !hidden.contains(id))
        .collect();
    // Walks between drawn rooms, either way round.
    let mut next: std::collections::HashMap<RoomId, Vec<RoomId>> = std::collections::HashMap::new();
    for room in map.rooms() {
        for exit in room.exits.iter().filter(|e| is_passage(e)) {
            next.entry(room.id).or_default().push(exit.to);
            next.entry(exit.to).or_default().push(room.id);
        }
    }
    let mut seen: HashSet<RoomId> = HashSet::new();
    let mut clusters: Vec<Vec<RoomId>> = Vec::new();
    for &start in &shown {
        if !seen.insert(start) {
            continue;
        }
        let mut cluster = vec![start];
        let mut i = 0;
        while i < cluster.len() {
            for &n in next.get(&cluster[i]).into_iter().flatten() {
                if map.room(n).is_some() && !hidden.contains(&n) && seen.insert(n) {
                    cluster.push(n);
                }
            }
            i += 1;
        }
        clusters.push(cluster);
    }
    let biggest = clusters.iter().map(Vec::len).max().unwrap_or(0);
    for cluster in clusters.iter().filter(|c| c.len() < biggest) {
        let mine: HashSet<RoomId> = cluster.iter().copied().collect();
        let outside: Vec<RoomId> = cluster
            .iter()
            .flat_map(|id| next.get(id).into_iter().flatten().copied())
            // A way out of the selection is another area's street, not a
            // hidden room.
            .filter(|n| !mine.contains(n))
            .collect();
        let pictured = cluster
            .iter()
            .any(|&id| map.room(id).is_some_and(|r| r.image.is_some()));
        if !pictured && !outside.is_empty() && outside.iter().all(|n| hidden.contains(n)) {
            hidden.extend(mine);
        }
    }
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

    /// The Hinterwilds: a street, the pits' way in off it and a pit behind
    /// that, a hut with terrain, and a gash in from the street. Every indoor
    /// room with no terrain is hidden; the hut and the street are drawn, and
    /// the street is marked where it leads into what is hidden.
    #[test]
    fn indoors_with_no_terrain_is_hidden_and_its_way_in_marked() {
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
        assert_eq!(hidden, HashSet::from([RoomId(2), RoomId(3), RoomId(5)]));
        assert_eq!(entrances(&map, &hidden), HashSet::from([RoomId(1)]));
    }

    /// The Chthonian Dark: an arena entered only by `go arch` from a pit
    /// that is hidden. It is part of the pit's place and is hidden with
    /// it; the street and the pit's doorway are not.
    #[test]
    fn a_place_reached_only_through_hidden_rooms_is_hidden_too() {
        let map = Map::from_rooms(vec![
            room(
                1,
                "Obvious paths: east",
                Some("barren scrub"),
                &[(2, "go pit"), (6, "east")],
            ),
            room(
                6,
                "Obvious paths: west",
                Some("barren scrub"),
                &[(1, "west"), (7, "east")],
            ),
            room(
                7,
                "Obvious paths: west",
                Some("barren scrub"),
                &[(6, "west")],
            ),
            room(2, "Obvious exits: north", None, &[(1, "out"), (3, "north")]),
            room(
                3,
                "Obvious exits: south",
                None,
                &[(2, "south"), (4, "go arch")],
            ),
            room(
                4,
                "Obvious paths: east",
                None,
                &[(3, "go arch"), (5, "east")],
            ),
            room(5, "Obvious paths: west", None, &[(4, "west")]),
        ])
        .expect("no duplicate ids");

        let hidden = hidden_rooms(&map);
        assert_eq!(
            hidden,
            HashSet::from([RoomId(2), RoomId(3), RoomId(4), RoomId(5)])
        );
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
