//! A place opened where its dot is (the author, 2026-09-29: *"If I'm in a
//! bank that is 2 rooms, I don't want a 2 room minimap, I want entering the
//! bank to make it's two rooms become squares and show on the minimap"*;
//! Hydra's `plan/53` §8a). The area's sheet stays as it is; the place,
//! laid out on its own, is drawn at [`HALF`] the streets' scale beside the
//! street room it is entered from, joined to it by a short line where the
//! dot was.
//!
//! Where: its first room one half step from the door's street room, on the
//! dot's side, then the other sides, then further out, up to
//! [`REACH_STEPS`] street steps; the first spot where it breaks none of the
//! rules (no room on or beside another, no line through a room, none of
//! its lines through the street's rooms) wins. Where no spot is clean, the
//! one breaking fewest is taken, and [`Opened::fitted`] says so.

use std::collections::HashSet;

use cena_map::RoomId;

use crate::positioner::Cell;
use crate::scene::{MapScene, Point, SceneEdge, SceneEdgeKind, Unit, UnitKind};

/// How many times smaller than the streets a place is drawn.
pub const HALF: i32 = 2;

/// How far from its door, in street steps, a place is looked for a spot.
pub const REACH_STEPS: i32 = 4;

/// A room nearer than this, in cells, to another is on it or crowding it.
const CROWD: f32 = 1.5;

/// A line nearer than this, in cells, to a room's centre passes through it.
const THROUGH: f32 = 0.5;

/// An area's sheet with one place opened on it.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The sheet, the place a building of it.
    pub scene: MapScene,
    /// Whether the spot broke no rule.
    pub fitted: bool,
}

/// Open on `area`'s sheet (laid out at `scale` cells a street step) the
/// place laid out alone as `place`, entered by `area`'s dots into its
/// rooms. `None` when no dot of `area` leads into it.
#[must_use]
pub fn open_place(area: &MapScene, place: &MapScene, scale: i32) -> Option<Opened> {
    let inside: HashSet<RoomId> = place.sheet.rooms.iter().map(|r| r.id).collect();
    let mut doors: Vec<_> = area
        .sheet
        .doors
        .iter()
        .filter(|d| inside.contains(&d.inside) && area.room(d.street).is_some())
        .collect();
    doors.sort_by_key(|d| (d.street, d.inside));
    let door = *doors.first()?;
    let street = area.room(door.street)?.cell;
    let entry = place.room(door.inside)?.cell;
    // Each room of the place from its first, at half scale.
    let half = |c: Cell| Cell {
        x: div_round(c.x - entry.x, place_step(place) * HALF / scale.max(1)),
        y: div_round(c.y - entry.y, place_step(place) * HALF / scale.max(1)),
    };
    let rel: Vec<(RoomId, Cell)> = place
        .sheet
        .rooms
        .iter()
        .map(|r| (r.id, half(r.cell)))
        .collect();
    let near = side(street, door.at);

    let mut best: Option<(usize, Cell)> = None;
    'search: for k in 1..=(REACH_STEPS * HALF) {
        for dir in sides_from(near) {
            let at = Cell {
                x: street.x + dir.0 * k * scale / HALF,
                y: street.y + dir.1 * k * scale / HALF,
            };
            let broken = broken(area, place, &rel, at, door.street, &doors);
            if best.is_none_or(|(least, _)| broken < least) {
                best = Some((broken, at));
            }
            if broken == 0 {
                break 'search;
            }
        }
    }
    let (broken, at) = best?;
    Some(Opened {
        scene: opened(area, place, &rel, at, &doors),
        fitted: broken == 0,
    })
}

/// Cells a step between two of the place's own rooms: the least distance
/// along a line of its, or `scale`'s 4 when it has none.
fn place_step(place: &MapScene) -> i32 {
    place
        .sheet
        .edges
        .iter()
        .map(|e| (e.a.x - e.b.x).abs().max((e.a.y - e.b.y).abs()))
        .filter(|&d| d > 0)
        .min()
        .unwrap_or(crate::interior_shelf::TOWN_SCALE)
}

fn div_round(v: i32, by: i32) -> i32 {
    let by = by.max(1);
    #[allow(clippy::cast_possible_truncation)]
    let r = (f64::from(v) / f64::from(by)).round() as i32;
    r
}

/// The compass side of `from` that `to` lies on, as a unit step.
fn side(from: Cell, to: Point) -> (i32, i32) {
    #[allow(clippy::cast_precision_loss)]
    let (dx, dy) = (to.x - from.x as f32, to.y - from.y as f32);
    let sign = |v: f32| {
        if v > 0.2 {
            1
        } else if v < -0.2 {
            -1
        } else {
            0
        }
    };
    match (sign(dx), sign(dy)) {
        (0, 0) => (0, 1),
        s => s,
    }
}

/// The eight sides, `first` then the rest nearest it first.
fn sides_from(first: (i32, i32)) -> Vec<(i32, i32)> {
    let ring = [
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
    ];
    let start = ring.iter().position(|&s| s == first).unwrap_or(4);
    let mut out = vec![ring[start]];
    for d in 1..=4 {
        out.push(ring[(start + d) % 8]);
        if d < 4 {
            out.push(ring[(start + 8 - d) % 8]);
        }
    }
    out
}

fn point(c: Cell) -> Point {
    #[allow(clippy::cast_precision_loss)]
    Point {
        x: c.x as f32,
        y: c.y as f32,
    }
}

fn distance(a: Point, b: Point) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// How far `p` is from the segment `a`-`b`.
fn to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = dx.mul_add(dx, dy * dy);
    if length == 0.0 {
        return distance(p, a);
    }
    let t = ((p.x - a.x).mul_add(dx, (p.y - a.y) * dy) / length).clamp(0.0, 1.0);
    distance(
        p,
        Point {
            x: t.mul_add(dx, a.x),
            y: t.mul_add(dy, a.y),
        },
    )
}

/// Whether the polyline `path` passes through `p`.
fn crosses(path: &[Point], p: Point) -> bool {
    path.windows(2).any(|w| to_segment(p, w[0], w[1]) < THROUGH)
}

/// The place's rooms at `at` and their lines, and each door's line from
/// its street room, as points.
fn drawn(
    place: &MapScene,
    rel: &[(RoomId, Cell)],
    at: Cell,
    doors: &[&crate::doors::SceneDoor],
    street_of: impl Fn(RoomId) -> Option<Cell>,
) -> (Vec<(RoomId, Point)>, Vec<Vec<Point>>) {
    let cell_of = |id: RoomId| {
        rel.iter().find(|(r, _)| *r == id).map(|(_, c)| Cell {
            x: at.x + c.x,
            y: at.y + c.y,
        })
    };
    let rooms = rel
        .iter()
        .map(|&(id, c)| {
            (
                id,
                point(Cell {
                    x: at.x + c.x,
                    y: at.y + c.y,
                }),
            )
        })
        .collect();
    let mut lines: Vec<Vec<Point>> = place
        .sheet
        .edges
        .iter()
        .filter_map(|e| Some(vec![point(cell_of(e.a_room)?), point(cell_of(e.b_room)?)]))
        .collect();
    for door in doors {
        if let (Some(s), Some(i)) = (street_of(door.street), cell_of(door.inside)) {
            lines.push(vec![point(s), point(i)]);
        }
    }
    (rooms, lines)
}

/// How many rules the place at `at` breaks against `area`.
fn broken(
    area: &MapScene,
    place: &MapScene,
    rel: &[(RoomId, Cell)],
    at: Cell,
    street_room: RoomId,
    doors: &[&crate::doors::SceneDoor],
) -> usize {
    let (rooms, lines) = drawn(place, rel, at, doors, |id| area.room(id).map(|r| r.cell));
    let area_paths: Vec<Vec<Point>> = area
        .sheet
        .edges
        .iter()
        .map(crate::routing::path_of)
        .collect();
    let mut count = 0;
    for &(_, p) in &rooms {
        // On or beside a room of the area.
        count += area
            .sheet
            .rooms
            .iter()
            .filter(|r| distance(point(r.cell), p) < CROWD)
            .count();
        // Under a line of the area's.
        count += area_paths.iter().filter(|path| crosses(path, p)).count();
    }
    // Its lines through the area's rooms, but for the ends they join.
    let doors_streets: HashSet<RoomId> = doors.iter().map(|d| d.street).collect();
    for line in &lines {
        count += area
            .sheet
            .rooms
            .iter()
            .filter(|r| r.id != street_room && !doors_streets.contains(&r.id))
            .filter(|r| crosses(line, point(r.cell)))
            .count();
    }
    count
}

/// `area`'s sheet with the place drawn at `at`: its rooms a building of
/// their own, its lines, a line from each door's street room in place of
/// the dot, and the bounds grown to hold it.
fn opened(
    area: &MapScene,
    place: &MapScene,
    rel: &[(RoomId, Cell)],
    at: Cell,
    doors: &[&crate::doors::SceneDoor],
) -> MapScene {
    let mut scene = area.clone();
    let unit = scene.units.len();
    let group = scene
        .sheet
        .rooms
        .iter()
        .map(|r| r.group)
        .chain(scene.sheet.edges.iter().map(|e| e.group))
        .max()
        .map_or(0, |g| g + 1);
    let cell_of = |id: RoomId| {
        rel.iter().find(|(r, _)| *r == id).map(|(_, c)| Cell {
            x: at.x + c.x,
            y: at.y + c.y,
        })
    };
    let entries: Vec<RoomId> = doors.iter().map(|d| d.inside).collect();
    scene.units.push(Unit {
        kind: UnitKind::Building,
        name: doors.first().map(|d| d.place.clone()).unwrap_or_default(),
        rooms: rel.iter().map(|(id, _)| *id).collect(),
        door_rooms: entries,
    });
    for room in &place.sheet.rooms {
        if let Some(cell) = cell_of(room.id) {
            let mut room = room.clone();
            room.cell = cell;
            room.group = group;
            room.unit = unit;
            scene.sheet.rooms.push(room);
        }
    }
    for edge in &place.sheet.edges {
        if let (Some(a), Some(b)) = (cell_of(edge.a_room), cell_of(edge.b_room)) {
            scene.sheet.edges.push(SceneEdge {
                a,
                b,
                group,
                unit: Some(unit),
                via: Vec::new(),
                ..edge.clone()
            });
        }
    }
    for door in doors {
        let (Some(street), Some(inside)) = (area.room(door.street), cell_of(door.inside)) else {
            continue;
        };
        scene.sheet.edges.push(SceneEdge {
            a: street.cell,
            b: inside,
            a_room: door.street,
            b_room: door.inside,
            group: street.group,
            kind: SceneEdgeKind::Connector,
            label: None,
            unit: None,
            via: Vec::new(),
        });
    }
    let opened: HashSet<RoomId> = rel.iter().map(|(id, _)| *id).collect();
    scene.sheet.doors.retain(|d| !opened.contains(&d.inside));
    scene.room_index = scene
        .sheet
        .rooms
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id, i))
        .collect();
    scene.group_scale.insert(group, 1);
    crate::scene::compute_sheet_bounds(&mut scene);
    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dots_side_first_then_its_neighbours() {
        assert_eq!(sides_from((0, 1))[..3], [(0, 1), (-1, 1), (1, 1)]);
        assert_eq!(sides_from((1, 0)).len(), 8);
        assert_eq!(side(Cell { x: 4, y: 4 }, Point { x: 4.0, y: 4.45 }), (0, 1));
        assert_eq!(
            side(Cell { x: 4, y: 4 }, Point { x: 3.6, y: 3.6 }),
            (-1, -1)
        );
    }

    #[test]
    fn a_line_through_a_room_is_seen() {
        let path = [Point { x: 0.0, y: 0.0 }, Point { x: 8.0, y: 0.0 }];
        assert!(crosses(&path, Point { x: 4.0, y: 0.2 }));
        assert!(!crosses(&path, Point { x: 4.0, y: 2.0 }));
    }
}
