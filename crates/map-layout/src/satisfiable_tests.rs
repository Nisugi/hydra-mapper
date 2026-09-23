//! Tests for [`crate::satisfiable`], in their own file only because the
//! module they cover is long enough already.

use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId};

use crate::direction::DirectionMap;
use crate::satisfiable::{Axis, Problem, place_by_order, problems};

fn exit(to: u32, command: &str) -> Exit {
    Exit {
        to: RoomId(to),
        kind: ExitKind::Cardinal,
        crossing: Crossing::Command(command.to_owned()),
        cost: Some(Cost::Fixed(1.0)),
    }
}

fn room(id: u32, exits: Vec<Exit>) -> Room {
    Room {
        id: RoomId(id),
        uid: vec![],
        title: vec![format!("[R{id}]")],
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

fn check(rooms: Vec<Room>) -> Vec<Problem> {
    let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
    let map = Map::from_rooms(rooms).expect("no duplicate ids");
    let dirs = DirectionMap::build(&map);
    problems(&ids, &map, &dirs)
}

/// The fixture ATARI supplied: the engine reported two violations on it,
/// and the data is fine. Every stated direction holds at
/// `0=(0,2) 1=(1,0) 2=(2,1) 3=(0,0)`.
#[test]
fn the_four_room_counterexample_is_satisfiable() {
    let found = check(vec![
        room(
            0,
            vec![exit(1, "northeast"), exit(2, "northeast"), exit(3, "north")],
        ),
        room(1, vec![exit(0, "southwest"), exit(2, "southeast")]),
        room(2, vec![exit(0, "southwest"), exit(1, "northwest")]),
        room(3, vec![exit(0, "south")]),
    ]);
    assert!(found.is_empty(), "called satisfiable data bad: {found:?}");
}

/// **The case the equalities exist for.** Room 1 north of 2 and 2 north
/// of 3 put all three on one x; 1 east of 3 then demands a difference in
/// the x they must share. Dropping the equalities -- as an earlier
/// version did -- finds nothing here, because neither axis holds a cycle
/// on its own.
#[test]
fn a_contradiction_through_an_alignment_is_found() {
    let found = check(vec![
        room(1, vec![exit(2, "north"), exit(3, "east")]),
        room(2, vec![exit(3, "north")]),
        room(3, vec![]),
    ]);
    assert!(
        found.iter().any(|p| matches!(
            p,
            Problem::OrderWithinAlignment {
                axis: Axis::EastWest,
                ..
            }
        )),
        "missed a contradiction running through an alignment: {found:?}"
    );
}

/// Rooms aligned on both axes with nothing separating them must share a
/// cell, which no layout can draw. 9 is south of 1 and 2 north of 9, so 2
/// shares 1's x; 5 is east of 1 and 2 west of 5, so 2 shares 1's y. No
/// cycle anywhere -- only the overlap.
#[test]
fn rooms_forced_onto_one_cell_are_reported() {
    let found = check(vec![
        room(1, vec![exit(9, "south"), exit(5, "east")]),
        room(9, vec![exit(2, "north")]),
        room(5, vec![exit(2, "west")]),
        room(2, vec![]),
    ]);
    assert_eq!(
        found,
        vec![Problem::ForcedOverlap {
            a: RoomId(1),
            b: RoomId(2)
        }],
        "rooms pinned to one cell went unreported"
    );
}

/// Two rooms each claiming the other is east. No arrangement holds.
#[test]
fn mutual_east_is_a_contradiction() {
    assert!(
        !check(vec![
            room(1, vec![exit(2, "east")]),
            room(2, vec![exit(1, "east")]),
        ])
        .is_empty()
    );
}

/// A longer loop: a < b < c < a on one axis. Each edge is reasonable
/// alone, which is why it needs finding rather than eyeballing.
#[test]
fn a_three_room_loop_is_a_contradiction() {
    let found = check(vec![
        room(1, vec![exit(2, "east")]),
        room(2, vec![exit(3, "east")]),
        room(3, vec![exit(1, "east")]),
    ]);
    assert!(found.iter().any(|p| matches!(
        p,
        Problem::Cycle {
            axis: Axis::EastWest,
            ..
        }
    )));
}

/// An ordinary square is satisfiable, and so is a long chain -- the check
/// must not cry contradiction over normal maps.
#[test]
fn ordinary_shapes_are_satisfiable() {
    assert!(
        check(vec![
            room(1, vec![exit(2, "east"), exit(3, "south")]),
            room(2, vec![exit(1, "west"), exit(4, "south")]),
            room(3, vec![exit(1, "north"), exit(4, "east")]),
            room(4, vec![exit(2, "north"), exit(3, "west")]),
        ])
        .is_empty()
    );

    // A hundred rooms in a line: deep enough to overflow a recursive
    // search, which is why the walk is iterative.
    let long: Vec<Room> = (1..=100u32)
        .map(|id| {
            let mut e = vec![];
            if id > 1 {
                e.push(exit(id - 1, "west"));
            }
            if id < 100 {
                e.push(exit(id + 1, "east"));
            }
            room(id, e)
        })
        .collect();
    assert!(check(long).is_empty());
}

/// Stretched edges are ordering, not distance: three rooms in a row where
/// the far pair also names a bearing directly.
#[test]
fn a_stretched_edge_is_no_contradiction() {
    assert!(
        check(vec![
            room(1, vec![exit(2, "east"), exit(3, "east")]),
            room(2, vec![exit(1, "west"), exit(3, "east")]),
            room(3, vec![exit(1, "west"), exit(2, "west")]),
        ])
        .is_empty()
    );
}

/// What the ordering placer produces must satisfy every sign it was built
/// from -- the alignments included, which is the half that was missing.
#[test]
fn the_ordering_placement_satisfies_every_sign() {
    let rooms = vec![
        room(1, vec![exit(2, "east"), exit(3, "south")]),
        room(2, vec![exit(1, "west"), exit(4, "south")]),
        room(3, vec![exit(1, "north"), exit(4, "east")]),
        room(4, vec![exit(2, "north"), exit(3, "west")]),
    ];
    let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
    let map = Map::from_rooms(rooms).expect("ok");
    let dirs = DirectionMap::build(&map);
    let placed = place_by_order(&ids, &map, &dirs).expect("satisfiable");

    for &from in &ids {
        let Some(r) = map.room(from) else { continue };
        for e in &r.exits {
            let Some(dir) = dirs.get(from, e.to) else {
                continue;
            };
            let (ex, ey) = dir.offset();
            let (a, b) = (placed[&from], placed[&e.to]);
            assert_eq!(
                (b.x - a.x).signum(),
                ex,
                "x sign wrong for {from:?} -> {:?}",
                e.to
            );
            assert_eq!(
                (b.y - a.y).signum(),
                ey,
                "y sign wrong for {from:?} -> {:?}",
                e.to
            );
        }
    }
}

/// Contradictory rooms get no arrangement rather than a wrong one.
#[test]
fn contradictory_rooms_place_to_nothing() {
    let rooms = vec![
        room(1, vec![exit(2, "east")]),
        room(2, vec![exit(1, "east")]),
    ];
    let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
    let map = Map::from_rooms(rooms).expect("ok");
    let dirs = DirectionMap::build(&map);
    assert!(place_by_order(&ids, &map, &dirs).is_none());
}

/// A street of three, a tower reached from its middle by "go steps" and
/// then up and up, and a yard reached by "out": nothing orders the tower
/// or the yard east-west or north-south, so ranking put all four at the
/// origin, on one cell, with the street's own west end. They belong
/// beside the room they are reached from, each on a cell of its own,
/// and the street's order is not disturbed by making room for them.
#[test]
fn rooms_nothing_orders_sit_beside_their_neighbour_not_stacked_at_the_origin() {
    let rooms = vec![
        room(1, vec![exit(2, "east")]),
        room(
            2,
            vec![
                exit(1, "west"),
                exit(3, "east"),
                exit(10, "go steps"),
                exit(20, "out"),
            ],
        ),
        room(3, vec![exit(2, "west")]),
        room(10, vec![exit(2, "down"), exit(11, "up")]),
        room(11, vec![exit(10, "down"), exit(12, "up")]),
        room(12, vec![exit(11, "down")]),
        room(20, vec![exit(2, "go gate")]),
    ];
    let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
    let map = Map::from_rooms(rooms).expect("ok");
    let dirs = DirectionMap::build(&map);
    let placed = place_by_order(&ids, &map, &dirs).expect("satisfiable");

    let mut cells: Vec<_> = placed.values().copied().collect();
    cells.sort_unstable_by_key(|c| (c.x, c.y));
    let before = cells.len();
    cells.dedup();
    assert_eq!(cells.len(), before, "two rooms share a cell: {placed:?}");

    let at = |id: u32| placed[&RoomId(id)];
    assert!(
        at(1).x < at(2).x && at(2).x < at(3).x,
        "the street lost its order: {placed:?}"
    );
    assert_eq!(at(1).y, at(2).y);
    assert_eq!(at(2).y, at(3).y);
    let apart = |a: crate::positioner::Cell, b: crate::positioner::Cell| {
        (a.x - b.x).abs().max((a.y - b.y).abs())
    };
    for id in [10, 20] {
        assert!(
            apart(at(id), at(2)) <= 2,
            "room {id} is {} cells from the room it is reached from: {placed:?}",
            apart(at(id), at(2))
        );
    }
}

/// Every stated planar bearing among `placed` holds by its signs, and no
/// two rooms share a cell.
fn assert_drawable(
    placed: &std::collections::HashMap<RoomId, crate::positioner::Cell>,
    map: &Map,
    dirs: &DirectionMap,
) {
    for room in map.rooms() {
        for e in &room.exits {
            let Some(d) = dirs.get(room.id, e.to) else {
                continue;
            };
            if matches!(d, crate::direction::Dir::Up | crate::direction::Dir::Down) {
                continue;
            }
            let (a, b) = (placed[&room.id], placed[&e.to]);
            let (dx, dy) = d.offset();
            assert!(
                (b.x - a.x).signum() == dx.signum() && (b.y - a.y).signum() == dy.signum(),
                "{} -> {} is {d:?} but sits at {a:?} -> {b:?}",
                room.id.0,
                e.to.0
            );
        }
    }
    let mut cells: Vec<_> = placed.values().copied().collect();
    cells.sort_unstable_by_key(|c| (c.x, c.y));
    let before = cells.len();
    cells.dedup();
    assert_eq!(cells.len(), before, "two rooms share a cell: {placed:?}");
}

fn place(rooms: Vec<Room>) {
    let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
    let map = Map::from_rooms(rooms).expect("ok");
    let dirs = DirectionMap::build(&map);
    let placed = place_by_order(&ids, &map, &dirs).expect("satisfiable");
    assert_drawable(&placed, &map, &dirs);
}

/// Two rooms both north of one room: both must share its x and sit above
/// it, and nothing says which is higher -- so ranking put them on one
/// cell, and the nudge, which only ever moved a room upward, could not
/// part them. One must step further north.
#[test]
fn two_rooms_north_of_one_do_not_share_a_cell() {
    place(vec![
        room(1, vec![exit(2, "north"), exit(3, "north")]),
        room(2, vec![]),
        room(3, vec![]),
    ]);
}

/// Two grids joined only by a bearingless doorway: Henty's Depot, whose
/// halves were drawn on top of each other because nothing orders one
/// against the other and both were ranked from the origin.
#[test]
fn two_grids_a_doorway_joins_are_drawn_apart() {
    place(vec![
        room(
            1,
            vec![exit(2, "east"), exit(3, "south"), exit(11, "go arch")],
        ),
        room(2, vec![exit(1, "west"), exit(4, "south")]),
        room(3, vec![exit(1, "north"), exit(4, "east")]),
        room(4, vec![exit(2, "north"), exit(3, "west")]),
        room(
            11,
            vec![exit(12, "east"), exit(13, "south"), exit(1, "go arch")],
        ),
        room(12, vec![exit(11, "west"), exit(14, "south")]),
        room(13, vec![exit(11, "north"), exit(14, "east")]),
        room(14, vec![exit(12, "north"), exit(13, "west")]),
    ]);
}

/// Two roads, each with a field one step east, where the ranking gave
/// both fields the same coordinate on both axes although they are
/// different classes on each: neither can be nudged alone, so one field's
/// class is pushed, and whatever is east of it follows.
#[test]
fn classes_ranked_onto_one_cell_are_pushed_apart() {
    place(vec![
        // Road A runs north-south; its field is east of its top room.
        room(1, vec![exit(2, "south"), exit(5, "east")]),
        room(2, vec![exit(1, "north")]),
        room(5, vec![exit(1, "west"), exit(6, "east")]),
        room(6, vec![exit(5, "west")]),
        // Road B, joined to A only by a bearingless path, the same shape.
        room(
            3,
            vec![exit(4, "south"), exit(7, "east"), exit(1, "go path")],
        ),
        room(4, vec![exit(3, "north")]),
        room(7, vec![exit(3, "west"), exit(8, "east")]),
        room(8, vec![exit(7, "west")]),
    ]);
}
