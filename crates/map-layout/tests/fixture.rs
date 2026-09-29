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

/// The bank's way in is a dot beside the square, named for the bank.
#[test]
fn the_bank_door_is_a_dot_beside_the_square() {
    let map = town();
    let layout = generate_layout(&map);
    let scene = cena_map_layout::build_scene("Test Town", &layout, &map);

    let square = scene.room(RoomId(1)).expect("the square is drawn").cell;
    let door = scene
        .sheet
        .doors
        .iter()
        .find(|d| d.street == RoomId(1))
        .expect("the square has a dot for the bank");
    assert_eq!(door.inside, RoomId(3));
    assert_eq!(door.place, "Trader's Bank");
    assert_eq!(door.rooms, 2);
    #[allow(clippy::cast_precision_loss)]
    let off = (door.at.x - square.x as f32).hypot(door.at.y - square.y as f32);
    assert!(
        off > 0.0 && off < 1.0,
        "the dot is {off} cells off the square"
    );
}

/// A group cut to a sheet of its own gets its line back when it is drawn
/// beside its link (a hand's move puts it there): marks only for a link
/// drawn long.
#[test]
fn a_cut_group_drawn_beside_its_link_keeps_its_line() {
    use cena_map_layout::scene::SceneEdgeKind;
    let map = town();
    let mut layout = generate_layout(&map);
    let loft = layout
        .groups
        .iter()
        .find(|g| g.room_ids.contains(&RoomId(8)))
        .expect("the loft is placed")
        .index;
    layout.cut = vec![loft];
    let kind = |layout: &cena_map_layout::Layout| {
        let scene = cena_map_layout::build_scene("Test Town", layout, &map);
        scene
            .sheet
            .edges
            .iter()
            .find(|e| [e.a_room, e.b_room].contains(&RoomId(8)))
            .expect("the rope is drawn")
            .kind
    };
    assert_eq!(kind(&layout), SceneEdgeKind::Connector);
    // Moved far off, the same cut group's rope is marks.
    if let Some(offset) = &mut layout.groups[loft].base_offset {
        offset.x += 40;
    }
    assert_eq!(kind(&layout), SceneEdgeKind::Stub);
}

/// A room placed by up from one whose north is already taken goes to a
/// free cell beside it, not a second step north past the room there with
/// its line drawn back over that room's (the Long Snow's Encampment).
/// Up is listed first, so the order exits are placed in is tested too.
/// The Long Snow's own shape, and its numbering, which starts the
/// placement at the expanse the encampment is up from: the river's edge,
/// south, southeast, south, then the encampment up.
#[test]
fn up_takes_a_free_side_when_north_is_taken() {
    // As numbered on gs.map, 29860 to 29864, less 29850.
    let map = Map::from_rooms(vec![
        outdoor(10, "[Long Snow, Encampment]", vec![cardinal(12, "down")]),
        outdoor(11, "[Long Snow, River's Edge]", vec![cardinal(14, "south")]),
        outdoor(
            12,
            "[Long Snow, Expanse]",
            vec![cardinal(10, "up"), cardinal(13, "north")],
        ),
        outdoor(
            13,
            "[Long Snow, Expanse]",
            vec![cardinal(12, "south"), cardinal(14, "northwest")],
        ),
        outdoor(
            14,
            "[Long Snow, Expanse]",
            vec![cardinal(11, "north"), cardinal(13, "southeast")],
        ),
    ])
    .expect("distinct ids");
    let layout = generate_layout(&map);
    let group = layout
        .groups
        .iter()
        .find(|g| g.room_ids.contains(&RoomId(12)))
        .expect("placed");
    let at = |id| group.positions[&RoomId(id)];
    let (expanse, camp) = (at(12), at(10));
    assert_eq!(
        (camp.x - expanse.x).abs().max((camp.y - expanse.y).abs()),
        1,
        "the encampment is beside the expanse, at {camp:?} from {expanse:?}"
    );
    assert!(
        camp.y < expanse.y,
        "up is drawn going down: {camp:?} from {expanse:?}"
    );
    for id in [11, 13, 14] {
        let r = at(id);
        let (dx, dy) = (camp.x - expanse.x, camp.y - expanse.y);
        let (rx, ry) = (r.x - expanse.x, r.y - expanse.y);
        assert!(
            !(rx * dy == ry * dx && rx.signum() == dx.signum() && ry.signum() == dy.signum()),
            "room {id} at {r:?} is on the way up from {expanse:?} to {camp:?}"
        );
    }
}

/// A ring too tight for what hangs inside it grows about the room it hangs
/// from until it fits, rather than the island being cut and drawn below
/// (the Atoll's ruins, off its Inner Edge).
///
/// ```text
/// 11 - 12 - 13
///  |    |    |
/// 18   20    14      20 hangs south of 12; 30-31-32 hang off 20
///  |         |       by `go barrier`, three rooms wide
/// 17 - 16 - 15
/// ```
#[test]
fn a_ring_grows_to_hold_what_hangs_inside_it() {
    let map = Map::from_rooms(vec![
        outdoor(
            11,
            "[Atoll, Path]",
            vec![cardinal(12, "east"), cardinal(18, "south")],
        ),
        outdoor(
            12,
            "[Atoll, Path]",
            vec![
                cardinal(11, "west"),
                cardinal(13, "east"),
                cardinal(20, "south"),
            ],
        ),
        outdoor(
            13,
            "[Atoll, Path]",
            vec![cardinal(12, "west"), cardinal(14, "south")],
        ),
        outdoor(
            14,
            "[Atoll, Path]",
            vec![cardinal(13, "north"), cardinal(15, "south")],
        ),
        outdoor(
            15,
            "[Atoll, Path]",
            vec![cardinal(14, "north"), cardinal(16, "west")],
        ),
        outdoor(
            16,
            "[Atoll, Path]",
            vec![cardinal(15, "east"), cardinal(17, "west")],
        ),
        outdoor(
            17,
            "[Atoll, Path]",
            vec![cardinal(16, "east"), cardinal(18, "north")],
        ),
        outdoor(
            18,
            "[Atoll, Path]",
            vec![cardinal(17, "south"), cardinal(11, "north")],
        ),
        outdoor(
            20,
            "[Atoll, Inner Edge]",
            vec![cardinal(12, "north"), plain(31, ExitKind::Go, "go barrier")],
        ),
        outdoor(30, "[Ruins, Platform]", vec![cardinal(31, "east")]),
        outdoor(
            31,
            "[Ruins, Dais]",
            vec![
                cardinal(30, "west"),
                cardinal(32, "east"),
                plain(20, ExitKind::Go, "go barrier"),
            ],
        ),
        outdoor(32, "[Ruins, Platform]", vec![cardinal(31, "west")]),
    ])
    .expect("distinct ids");
    let layout = generate_layout(&map);
    assert!(
        layout.cut.is_empty(),
        "the ruins were cut: {:?}",
        layout.cut
    );
    let cell = |id| {
        layout
            .groups
            .iter()
            .find(|g| g.room_ids.contains(&RoomId(id)))
            .map(|g| g.final_cell(RoomId(id)))
            .expect("placed")
    };
    let ring: Vec<_> = (11..=18).map(cell).collect();
    let (lo_x, hi_x) = (
        ring.iter().map(|c| c.x).min().unwrap_or(0),
        ring.iter().map(|c| c.x).max().unwrap_or(0),
    );
    let (lo_y, hi_y) = (
        ring.iter().map(|c| c.y).min().unwrap_or(0),
        ring.iter().map(|c| c.y).max().unwrap_or(0),
    );
    for id in 30..=32 {
        let c = cell(id);
        assert!(
            c.x > lo_x && c.x < hi_x && c.y > lo_y && c.y < hi_y,
            "ruin {id} at {c:?} is not inside the ring ({lo_x}..{hi_x}, {lo_y}..{hi_y})"
        );
    }
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
