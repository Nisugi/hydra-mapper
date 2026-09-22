//! Curated corrections that change what the solver does.
//!
//! Ported from `reference/VellumFE/src/core/layout_engine/overrides.rs`,
//! narrowed to the two edge actions that affect **geometry**. Vellum's
//! other three (`Hide`, `Dash`, `Dots`) only restyle a drawn line, which
//! is a renderer's concern and no business of this crate.
//!
//! Both actions here are inputs to generation, not adjustments after it:
//! they are applied to the [`crate::direction::DirectionMap`] before
//! `position_rooms` runs, so the solver lays the rooms out knowing the
//! correction. That is the whole point -- a room placed wrongly is not
//! nudged back afterwards, it is placed correctly the first time, and
//! everything downstream (packing, classification, violation counts)
//! follows from the corrected geometry.
//!
//! Keyed by [`cena_map::RoomId`], the id the solver itself speaks. The
//! stable, save-to-disk identity of a room is a caller's problem: the
//! mapper keys its store by game uid and resolves to ids when it builds
//! this list, which keeps file formats out of a crate that has no file
//! I/O.

use cena_map::RoomId;

use serde::{Deserialize, Serialize};

use crate::direction::Dir;

/// What to do with the edge between two rooms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeAction {
    /// Treat the edge as a directionless passage: it still connects the
    /// rooms, but constrains nothing about where they sit.
    ///
    /// The fix for two rooms the solver welded together on bad data -- a
    /// `go door` the direction analyser guessed a compass bearing for, or
    /// a pair of exits that disagree. Un-welding lets each room be placed
    /// by its other, better-evidenced exits.
    Connector,
    /// Force this direction from `a` to `b`; the reverse edge, if the map
    /// has one, takes the opposite.
    ///
    /// The fix for an edge whose stated direction is wrong or missing.
    /// This is what answers a direction violation: the exit claims north,
    /// the room actually sits south, and one of those is the truth.
    Direction(Dir),
}

/// One curated correction to one edge.
///
/// The pair is unordered -- the same correction applies whichever end it
/// is written from -- but [`EdgeAction::Direction`] reads `a -> b`, so the
/// two ends are not interchangeable for that action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeOverride {
    pub a: RoomId,
    pub b: RoomId,
    pub action: EdgeAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::direction::DirectionMap;
    use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room};

    /// Two rooms joined by `cmd`, both ways.
    fn pair(cmd: &str, back: &str) -> Map {
        let exit = |to: u32, command: &str| Exit {
            to: RoomId(to),
            kind: ExitKind::Cardinal,
            crossing: Crossing::Command(command.to_owned()),
            cost: Some(Cost::Fixed(1.0)),
        };
        let room = |id: u32, exits: Vec<Exit>| Room {
            id: RoomId(id),
            uid: vec![],
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
        };
        Map::from_rooms(vec![
            room(1, vec![exit(2, cmd)]),
            room(2, vec![exit(1, back)]),
        ])
        .expect("no duplicate ids")
    }

    /// A forced direction replaces what the command text said, and the
    /// reverse edge takes the opposite -- the fix for an exit whose stated
    /// bearing is wrong.
    #[test]
    fn a_forced_direction_overrides_the_command_text() {
        let map = pair("north", "south");
        let mut dirs = DirectionMap::build(&map);
        assert_eq!(dirs.get(RoomId(1), RoomId(2)), Some(Dir::North));

        dirs.apply_edge_overrides(
            &map,
            &[EdgeOverride {
                a: RoomId(1),
                b: RoomId(2),
                action: EdgeAction::Direction(Dir::East),
            }],
        );
        assert_eq!(dirs.get(RoomId(1), RoomId(2)), Some(Dir::East));
        assert_eq!(
            dirs.get(RoomId(2), RoomId(1)),
            Some(Dir::West),
            "the reverse edge did not take the opposite"
        );
    }

    /// A connector forgets the direction in both senses, so the edge stops
    /// welding the two rooms into a fixed relative position.
    #[test]
    fn a_connector_forgets_the_direction_both_ways() {
        let map = pair("north", "south");
        let mut dirs = DirectionMap::build(&map);
        dirs.apply_edge_overrides(
            &map,
            &[EdgeOverride {
                a: RoomId(1),
                b: RoomId(2),
                action: EdgeAction::Connector,
            }],
        );
        assert_eq!(dirs.get(RoomId(1), RoomId(2)), None);
        assert_eq!(dirs.get(RoomId(2), RoomId(1)), None);
    }

    /// A one-way exit does not gain a return the game does not offer.
    #[test]
    fn forcing_a_direction_invents_no_reverse_exit() {
        let one_way = Map::from_rooms(vec![
            Room {
                id: RoomId(1),
                uid: vec![],
                title: vec!["[One]".to_owned()],
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
                exits: vec![Exit {
                    to: RoomId(2),
                    kind: ExitKind::Cardinal,
                    crossing: Crossing::Command("north".to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                }],
            },
            Room {
                id: RoomId(2),
                uid: vec![],
                title: vec!["[Two]".to_owned()],
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
                exits: vec![],
            },
        ])
        .expect("no duplicate ids");

        let mut dirs = DirectionMap::build(&one_way);
        dirs.apply_edge_overrides(
            &one_way,
            &[EdgeOverride {
                a: RoomId(1),
                b: RoomId(2),
                action: EdgeAction::Direction(Dir::East),
            }],
        );
        assert_eq!(dirs.get(RoomId(1), RoomId(2)), Some(Dir::East));
        assert_eq!(
            dirs.get(RoomId(2), RoomId(1)),
            None,
            "a reverse direction was invented for an exit that does not exist"
        );
    }

    /// The correction reaches the solver: forcing east actually places
    /// the second room east of the first, which is the point of applying
    /// it before positioning rather than after.
    ///
    /// Asserted as a relative offset, because a group's cells are
    /// normalised -- what matters is where room 2 sits *from* room 1, not
    /// which absolute cell either landed in.
    #[test]
    fn a_forced_direction_changes_where_rooms_land() {
        let map = pair("north", "south");
        let offset = |layout: &crate::Layout| {
            let group = &layout.groups[0];
            let a = group.final_cell(RoomId(1));
            let b = group.final_cell(RoomId(2));
            (b.x - a.x, b.y - a.y)
        };

        // y grows downward, so north is a negative y.
        assert_eq!(offset(&crate::generate_layout(&map)), (0, -1));

        let fixed = crate::generate_layout_with(
            &map,
            &[EdgeOverride {
                a: RoomId(1),
                b: RoomId(2),
                action: EdgeAction::Direction(Dir::East),
            }],
        );
        assert_eq!(offset(&fixed), (1, 0), "room 2 did not land east of room 1");
    }

    /// An override naming a room the map does not have is skipped, not a
    /// panic: a saved correction outlives the map it was written against.
    #[test]
    fn an_override_for_a_missing_room_is_skipped() {
        let map = pair("north", "south");
        let mut dirs = DirectionMap::build(&map);
        dirs.apply_edge_overrides(
            &map,
            &[EdgeOverride {
                a: RoomId(1),
                b: RoomId(999),
                action: EdgeAction::Direction(Dir::East),
            }],
        );
        assert_eq!(
            dirs.get(RoomId(1), RoomId(2)),
            Some(Dir::North),
            "an unrelated edge was disturbed"
        );
    }

    /// The wire form is stable, because it is what a saved correction
    /// round-trips through.
    #[test]
    fn edge_actions_serialize_by_name() {
        let connector = serde_json::to_string(&EdgeAction::Connector).expect("serializes");
        assert_eq!(connector, "\"connector\"");
        let north = serde_json::to_string(&EdgeAction::Direction(Dir::North)).expect("serializes");
        assert_eq!(north, "{\"direction\":\"north\"}");
        let back: EdgeAction = serde_json::from_str(&north).expect("parses");
        assert_eq!(back, EdgeAction::Direction(Dir::North));
    }
}
