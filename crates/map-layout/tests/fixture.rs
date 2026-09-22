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
    let repeat_stats = LayoutStats::compute(&again, &map);
    assert_eq!(stats, repeat_stats, "same rooms in, same layout out");
}

/// The bank is a real building: two indoor rooms, reached by one doorway
/// from the square, forming one cluster -- not two, and not merged with
/// the outdoor square.
#[test]
fn the_bank_is_one_building_with_one_doorway() {
    let map = town();
    let layout = generate_layout(&map);

    let lobby_group = layout
        .groups
        .iter()
        .find(|g| g.room_ids.contains(&RoomId(3)))
        .expect("lobby was placed");
    let vault_group = layout
        .groups
        .iter()
        .find(|g| g.room_ids.contains(&RoomId(4)))
        .expect("vault was placed");

    assert!(
        layout
            .classification
            .interior_groups
            .contains(&lobby_group.index),
        "the bank lobby is interior, not left outdoors"
    );

    // Room 3 and room 4 share a directional edge (up/down), so BFS places
    // them in the same component; the bank is one group, one cluster.
    assert_eq!(
        lobby_group.index, vault_group.index,
        "up/down keeps the lobby and vault in one component"
    );

    assert_eq!(
        layout.classification.entrance_room_ids.len(),
        1,
        "exactly one outdoor room hosts the bank's doorway"
    );
    assert!(
        layout.classification.entrance_room_ids.contains(&RoomId(1)),
        "room 1's door is the bank's only entrance"
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

/// The generated scene draws every room once, makes the bank a unit of
/// its own, and gives it a name from its title's bracketed prefix.
#[test]
fn the_scene_draws_every_room_and_names_the_bank() {
    let map = town();
    let layout = generate_layout(&map);
    let scene = cena_map_layout::build_scene("Test Town", &layout, &map);

    assert_eq!(
        scene.sheet.rooms.len(),
        7,
        "every room in the fixture is drawn exactly once"
    );

    let lobby = scene.room(RoomId(3)).expect("the lobby is in the scene");
    let labels = scene.sheet.labels.clone();
    assert!(
        labels.iter().any(|l| l.text == "Trader's Bank"),
        "the bank's building name comes from its bracketed title prefix: {labels:?}"
    );
    assert_eq!(lobby.title, "[Trader's Bank, Lobby]");
    assert_eq!(scene.units[lobby.unit].name, "Trader's Bank");
    assert!(scene.units[lobby.unit].door_rooms.contains(&RoomId(3)));
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
