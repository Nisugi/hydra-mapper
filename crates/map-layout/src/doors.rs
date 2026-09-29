//! The dots for the ways in. The author, 2026-09-29: *"their room that
//! leads to an outside room, shows up on the map as a dot"*. A place left
//! off the sheet (`hidden`) is not laid out; each way into it is a dot
//! drawn beside the street room it is entered from, on the side its lines
//! leave most free, so a building reads as a building and a grotto as a
//! grotto without its rooms among the streets. A big place (Angargreft's
//! 56 rooms under the Hinterwilds) carries its name.
//!
//! A room laid out whose lines with no direction reach far across the
//! sheet is folded the same way ([`fold_fans`]): the Issenflow's current,
//! entered by `go river` from six rooms along the banks, drew six lines up
//! to 85 cells long to wherever it sat. It becomes a dot at each bank
//! (the author, 2026-09-29: *"I can accept dots"*).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use cena_map::{Map, RoomId};

use crate::hidden::WayIn;
use crate::scene::{Point, SheetScene};

/// How far from its street room's centre a dot sits, in cells.
const BESIDE: f32 = 0.45;

/// A place of this many rooms or more is named beside its dot.
pub const NAMED_ROOMS: usize = 10;

/// One way in, drawn: a dot beside its street room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDoor {
    /// The street room it is entered from.
    pub street: RoomId,
    /// The hidden room a walk leads to.
    pub inside: RoomId,
    /// Where the dot is drawn, in sheet cells.
    pub at: Point,
    /// The place behind it, and how many rooms it holds.
    pub place: String,
    pub rooms: usize,
}

impl SceneDoor {
    /// Whether the place behind it is big enough to be named on the sheet.
    #[must_use]
    pub const fn named(&self) -> bool {
        self.rooms >= NAMED_ROOMS
    }
}

/// A room with this many lines with no direction longer than
/// [`FAN_STEPS`] is folded to dots.
pub const FAN_LINES: usize = 3;

/// How long a line with no direction may run, in steps between street
/// rooms, before it counts toward [`FAN_LINES`]. Measured over gs.map, laid
/// out whole: the Miasmal Forest's paths run to 30 cells (7 steps) and
/// stay lines; the Issenflow's current (four over 35 cells, the longest
/// 85) and Black Swan Castle's drawbridge (three over 100) fold.
pub const FAN_STEPS: i32 = 10;

/// Take off `sheet` each room with [`FAN_LINES`] lines with no direction
/// longer than [`FAN_STEPS`] street steps (`scale` cells each), with every
/// line it had, and give back a way into it from each room those lines
/// reached, for [`place`] to dot.
pub(crate) fn fold_fans(sheet: &mut SheetScene, map: &Map, scale: i32) -> Vec<WayIn> {
    #[allow(clippy::cast_precision_loss)]
    let far = (FAN_STEPS * scale) as f32;
    let mut long: HashMap<RoomId, usize> = HashMap::new();
    for edge in &sheet.edges {
        if edge.kind != crate::scene::SceneEdgeKind::Connector {
            continue;
        }
        let path = crate::routing::path_of(edge);
        let length: f32 = path
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .sum();
        if length > far {
            *long.entry(edge.a_room).or_default() += 1;
            *long.entry(edge.b_room).or_default() += 1;
        }
    }
    let folded: HashSet<RoomId> = long
        .into_iter()
        .filter(|&(_, n)| n >= FAN_LINES)
        .map(|(room, _)| room)
        .collect();
    if folded.is_empty() {
        return Vec::new();
    }
    let mut ways: Vec<WayIn> = Vec::new();
    for edge in &sheet.edges {
        for (inside, street) in [(edge.a_room, edge.b_room), (edge.b_room, edge.a_room)] {
            if folded.contains(&inside)
                && !folded.contains(&street)
                && !ways
                    .iter()
                    .any(|w| w.street == street && w.inside == inside)
            {
                ways.push(WayIn {
                    street,
                    inside,
                    place: crate::hidden::place_name(map, &[inside]),
                    rooms: 1,
                });
            }
        }
    }
    sheet
        .edges
        .retain(|e| !folded.contains(&e.a_room) && !folded.contains(&e.b_room));
    sheet.rooms.retain(|r| !folded.contains(&r.id));
    ways.sort_by_key(|w| (w.street, w.inside));
    ways
}

/// The eight sides a dot may sit on, as unit steps: straight ones first.
const SIDES: [(f32, f32); 8] = [
    (0.0, -1.0),
    (1.0, 0.0),
    (0.0, 1.0),
    (-1.0, 0.0),
    (1.0, -1.0),
    (1.0, 1.0),
    (-1.0, 1.0),
    (-1.0, -1.0),
];

/// A dot for each way in whose street room is drawn, on the side of it its
/// lines leave most open, and each further dot at that room on the side
/// most open from its lines and the dots before it.
pub(crate) fn place(sheet: &mut SheetScene, ways_in: &[WayIn]) {
    let cells: HashMap<RoomId, Point> = sheet
        .rooms
        .iter()
        .map(|r| (r.id, crate::routing::point(r.cell)))
        .collect();
    // The way each line leaves each room, as an angle.
    let mut leaving: HashMap<RoomId, Vec<f32>> = HashMap::new();
    for edge in &sheet.edges {
        let path = crate::routing::path_of(edge);
        for (room, from, to) in [
            (edge.a_room, path.first(), path.get(1)),
            (edge.b_room, path.last(), path.iter().rev().nth(1)),
        ] {
            if let (Some(from), Some(to)) = (from, to) {
                leaving
                    .entry(room)
                    .or_default()
                    .push((to.y - from.y).atan2(to.x - from.x));
            }
        }
    }
    for way in ways_in {
        let Some(&centre) = cells.get(&way.street) else {
            continue;
        };
        let taken = leaving.entry(way.street).or_default();
        let side = open_side(taken);
        taken.push(side.1.atan2(side.0));
        let norm = side.0.hypot(side.1);
        sheet.doors.push(SceneDoor {
            street: way.street,
            inside: way.inside,
            at: Point {
                x: (side.0 / norm).mul_add(BESIDE, centre.x),
                y: (side.1 / norm).mul_add(BESIDE, centre.y),
            },
            place: way.place.clone(),
            rooms: way.rooms,
        });
    }
}

/// Of [`SIDES`], the one furthest in angle from every one `taken`.
fn open_side(taken: &[f32]) -> (f32, f32) {
    let apart = |side: (f32, f32)| -> f32 {
        let a = side.1.atan2(side.0);
        taken
            .iter()
            .map(|&t| {
                let d = (a - t).rem_euclid(std::f32::consts::TAU);
                d.min(std::f32::consts::TAU - d)
            })
            .fold(std::f32::consts::PI, f32::min)
    };
    let mut best = SIDES[0];
    for &side in &SIDES[1..] {
        if apart(side) > apart(best) + 1e-3 {
            best = side;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room with lines east and west gets its dot north; a second dot
    /// at the same room goes south, away from the first.
    #[test]
    fn a_dot_sits_where_the_lines_are_not() {
        let e = 0.0_f32;
        let w = std::f32::consts::PI;
        assert_eq!(open_side(&[e, w]), (0.0, -1.0));
        let n = (-1.0_f32).atan2(0.0);
        assert_eq!(open_side(&[e, w, n]), (0.0, 1.0));
        // A room whose one line runs north puts the dot south.
        assert_eq!(open_side(&[n]), (0.0, 1.0));
    }

    fn room(id: u32, x: i32, y: i32, title: &str) -> crate::scene::SceneRoom {
        crate::scene::SceneRoom {
            id: RoomId(id),
            uid: None,
            cell: crate::positioner::Cell { x, y },
            group: 0,
            unit: 0,
            entrance: false,
            title: title.to_owned(),
            terrain: None,
            service_tags: Vec::new(),
        }
    }

    fn line(a: &crate::scene::SceneRoom, b: &crate::scene::SceneRoom) -> crate::scene::SceneEdge {
        crate::scene::SceneEdge {
            a: a.cell,
            b: b.cell,
            a_room: a.id,
            b_room: b.id,
            group: 0,
            kind: crate::scene::SceneEdgeKind::Connector,
            label: None,
            unit: None,
            via: Vec::new(),
        }
    }

    /// The Issenflow's current, entered by `go river` from banks far
    /// apart, is folded to a dot at each bank; a room
    /// with the same three lines, short, stays drawn.
    #[test]
    fn a_room_whose_lines_run_far_is_a_dot_at_each_end() {
        let current = room(1, 0, 0, "[The Issenflow, Currents]");
        let banks = [
            room(2, 60, 0, "[Bank]"),
            room(3, 0, 60, "[Bank]"),
            room(4, -60, 0, "[Bank]"),
        ];
        let hut = room(5, 100, 100, "[Hut]");
        let yards = [
            room(6, 104, 100, "[Yard]"),
            room(7, 100, 104, "[Yard]"),
            room(8, 96, 100, "[Yard]"),
        ];
        let mut sheet = SheetScene::default();
        sheet.rooms.push(current.clone());
        sheet.rooms.push(hut.clone());
        for bank in &banks {
            sheet.edges.push(line(&current, bank));
            sheet.rooms.push(bank.clone());
        }
        for yard in &yards {
            sheet.edges.push(line(&hut, yard));
            sheet.rooms.push(yard.clone());
        }
        let map = Map::from_rooms(Vec::new()).expect("empty");
        let ways = fold_fans(&mut sheet, &map, 4);
        assert_eq!(
            ways.iter()
                .map(|w| (w.street.0, w.inside.0))
                .collect::<Vec<_>>(),
            [(2, 1), (3, 1), (4, 1)]
        );
        assert!(sheet.rooms.iter().all(|r| r.id != RoomId(1)));
        assert!(
            sheet.rooms.iter().any(|r| r.id == RoomId(5)),
            "the hut stays"
        );
        assert_eq!(
            sheet.edges.len(),
            3,
            "the hut's lines stay, the current's go"
        );
    }
}
