//! The dots for the ways in. The author, 2026-09-29: *"their room that
//! leads to an outside room, shows up on the map as a dot"*. A place left
//! off the sheet (`hidden`) is not laid out; each way into it is a dot
//! drawn beside the street room it is entered from, on the side its lines
//! leave most free, so a building reads as a building and a grotto as a
//! grotto without its rooms among the streets. A big place (Angargreft's
//! 56 rooms under the Hinterwilds) carries its name.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use cena_map::RoomId;

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
}
