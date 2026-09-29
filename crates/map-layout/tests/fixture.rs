//! End-to-end fixture: a small hand-built town, run through the whole
//! pipeline (`generate_layout` -> `build_scene`), checked against the hard
//! invariants the port must uphold (spec §9) and against numbers worth
//! pinning so a future change to the pipeline shows its effect here.
//!
//! Shape, all one location:
//!
//! ```text
//!            [10] Square, North
//!              |
//! [8] Loft --- [1] Square, Center --- [2] Square, East
//!  (climb rope)  |         \
//!              [3] Bank Lobby (go door)
//!                |
//!              [4] Bank Vault
//!
//! [2] --- [5] Gemshop Counter (go arch, no return leg)
//! ```
//!
//! Room 1 is the busiest outdoor room (4 directional edges), so BFS starts
//! there. Rooms 3-4 are the bank, indoor by "Obvious exits" and reached
//! from the square by a plain door -- a real building, one cluster, one
//! doorway. Room 8 is a directionless climb off the square, its own
//! component, outdoors (a treetop platform). Room 5 is reached only by "go
//! arch" from room 2, with no way back recorded -- an edge case the
//! classifier and scene must not choke on.

use cena_map::{Cost, Crossing, Exit, ExitKind, Image, Map, Room, RoomId};
use cena_map_layout::{LayoutStats, generate_layout};

fn outdoor(id: u32, title: &str, exits: Vec<Exit>) -> Room {
    Room {
        id: RoomId(id),
        uid: vec![],
        title: vec![title.to_owned()],
        description: vec![],
        paths: vec!["Obvious paths: see below".to_owned()],
        location: None,
        location_unknowable: false,
        check_location: false,
        unique_loot: vec![],
        climate: Some("clear".to_owned()),
        terrain: Some("hard, flat".to_owned()),
        tags: vec![],
        meta: vec![],
        image: None,
        exits,
    }
}

fn indoor(id: u32, title: &str, exits: Vec<Exit>) -> Room {
    Room {
        id: RoomId(id),
        uid: vec![],
        title: vec![title.to_owned()],
        description: vec![],
        paths: vec!["Obvious exits: see below".to_owned()],
        location: None,
        location_unknowable: false,
        check_location: false,
        unique_loot: vec![],
        climate: Some("none".to_owned()),
        terrain: Some("none".to_owned()),
        tags: vec![],
        meta: vec![],
        image: None,
        exits,
    }
}

fn cardinal(to: u32, command: &str) -> Exit {
    Exit {
        to: RoomId(to),
        kind: ExitKind::Cardinal,
        crossing: Crossing::Command(command.to_owned()),
        cost: Some(Cost::Fixed(1.0)),
    }
}

fn plain(to: u32, kind: ExitKind, command: &str) -> Exit {
    Exit {
        to: RoomId(to),
        kind,
        crossing: Crossing::Command(command.to_owned()),
        cost: Some(Cost::Fixed(1.0)),
    }
}

fn town() -> Map {
    let rooms = vec![
        outdoor(
            1,
            "[Town Square, Center]",
            vec![
                cardinal(10, "north"),
                cardinal(2, "east"),
                plain(8, ExitKind::Climb, "climb rope"),
                plain(3, ExitKind::Go, "go door"),
            ],
        ),
        outdoor(
            2,
            "[Town Square, East]",
            vec![cardinal(1, "west"), plain(5, ExitKind::Go, "go arch")],
        ),
        indoor(
            3,
            "[Trader's Bank, Lobby]",
            vec![plain(1, ExitKind::Go, "go door"), cardinal(4, "down")],
        ),
        indoor(4, "[Trader's Bank, Vault]", vec![cardinal(3, "up")]),
        outdoor(5, "[Gemshop Counter]", vec![]),
        outdoor(
            8,
            "[Town Square, Treetop Loft]",
            vec![plain(1, ExitKind::Climb, "climb down")],
        ),
        outdoor(10, "[Town Square, North]", vec![cardinal(1, "south")]),
    ];
    Map::from_rooms(rooms)
        .unwrap_or_else(|e| unreachable!("the fixture's ids are hand-written and distinct: {e}"))
}

/// Hard invariants (spec §9), any zone: no two rooms share a sheet cell,
/// every compass edge either matches its stated direction or is reported
/// as a violation, and running it twice gives the same answer.
#[test]
fn hard_invariants_hold() {
    let map = town();
    let layout = generate_layout(&map);
    let stats = LayoutStats::compute(&layout, &map);

    assert_eq!(
        stats.cell_overlaps, 0,
        "no two rooms may share a sheet cell"
    );
    assert_eq!(
        stats.direction_violations, 0,
        "every compass edge in this fixture is genuinely satisfiable"
    );
    assert_eq!(stats.rooms, 7);

    let again = generate_layout(&map);
    assert_eq!(layout, again, "same rooms in, same layout out");
}

/// The bank is a real building: two indoor rooms with no terrain, reached
/// by one door from the square. It is on the indoor sheet, not this one
/// (`hidden`), and the square is marked as its way in.
#[test]
fn the_bank_is_hidden_and_its_door_marked() {
    let map = town();
    let layout = generate_layout(&map);

    for id in [3, 4] {
        assert!(
            !layout
                .groups
                .iter()
                .any(|g| g.room_ids.contains(&RoomId(id))),
            "bank room {id} was drawn"
        );
    }
    assert!(
        layout.classification.entrance_room_ids.contains(&RoomId(1)),
        "the square is not marked as the bank's way in"
    );
}

/// A one-way "go arch" with no return leg still gets a room and does not
/// crash the pipeline -- BFS places it as a directionless component of its
/// own, since nothing states where it sits relative to room 2.
#[test]
fn a_one_way_exit_with_no_return_still_places() {
    let map = town();
    let layout = generate_layout(&map);
    assert!(
        layout
            .groups
            .iter()
            .any(|g| g.room_ids.contains(&RoomId(5))),
        "the gemshop counter must be placed somewhere"
    );
}

/// The generated scene draws every room but the bank's once, and marks
/// the square where the bank is entered.
#[test]
fn the_scene_draws_the_town_and_marks_the_bank_door() {
    let map = town();
    let layout = generate_layout(&map);
    let scene = cena_map_layout::build_scene("Test Town", &layout, &map);

    assert_eq!(
        scene.sheet.rooms.len(),
        5,
        "every room in the fixture but the bank's is drawn exactly once"
    );
    assert!(
        scene.room(RoomId(3)).is_none(),
        "the bank's lobby was drawn"
    );
    assert!(
        scene.room(RoomId(1)).expect("the square is drawn").entrance,
        "the square is not marked as a way in"
    );
}

/// A room with an `Image` anchor is not required by this fixture (no
/// scripted overlay in the source data), but the packer must not panic
/// when none of the packed rooms carry one -- confirms `pack_groups`'
/// image-anchor pass degrades to the connector/strip passes cleanly.
#[test]
fn packing_with_no_image_anchors_falls_back_cleanly() {
    let map = town();
    let layout = generate_layout(&map);
    assert!(
        layout.pack_info.primary_image.is_none(),
        "no room in this fixture carries an image anchor"
    );
    assert!(
        !layout.pack_info.methods.is_empty(),
        "every packed group still gets a pack method"
    );
}

/// `image_coords`, when present, anchors a group at the cell its pixel
/// position implies (spec §7 pass 1) -- exercised on a second, minimal map
/// so the main fixture can stay free of image data.
#[test]
fn an_image_anchored_room_seats_by_its_pixel_position() {
    let anchored = Room {
        image: Some(Image {
            file: "town.png".to_owned(),
            rect: [100, 100, 120, 120],
        }),
        ..outdoor(1, "[Anchored Room]", vec![])
    };
    let map = Map::from_rooms(vec![anchored]).expect("single room, no duplicates");
    let layout = generate_layout(&map);

    assert_eq!(layout.pack_info.primary_image.as_deref(), Some("town.png"));
    assert_eq!(layout.pack_info.methods.get("image").copied(), Some(1));
}
