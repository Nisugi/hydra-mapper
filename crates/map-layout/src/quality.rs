//! How well a drawn sheet reads, by the author's four rules for a tuned map
//! (Hydra's `plan/53` §6 item 1): *"exits drawn right not against their
//! direction, lines not drawn through rooms, buildings sitting under the
//! right lines, exits without a direction being placed in a smart way."*
//!
//! Measured on the scene as it is drawn, after the overrides and the
//! routing, because that is what a reader sees:
//!
//! 1. **Against their bearing**: an exit whose compass direction the drawn
//!    offset contradicts, by the positioner's own test (signs, so a
//!    stretched edge passes; `positioner.rs`, `validate_component`).
//! 2. **Through rooms**: a drawn line passing within
//!    [`CLEARANCE`](crate::routing::CLEARANCE) of a room it does not join,
//!    by the routing pass's own test. Stubs are ticks, not lines.
//! 3. **Under a line not theirs**: a building's room that a line passes
//!    over, when neither end of that line is in the building.
//! 4. **Exits with no direction**: a pair of rooms joined only by exits
//!    with no bearing either way (`go door`, `out`, a climb). How they are
//!    drawn: as a line of some length, as a stub, or not at all; and how
//!    many of those lines cross another line, and how many lie along
//!    one: near parallel and closer than [`ALONG_GAP`] for more than
//!    [`ALONG_RUN`], which reads as one line, or one hidden under another.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use cena_map::{Map, RoomId};

use crate::Layout;
use crate::direction::DirectionMap;
use crate::routing::{Obstacles, point};
use crate::scene::{MapScene, Point, STREETS, SceneEdge, SceneEdgeKind};

/// One sheet's counts, or several summed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Quality {
    /// Rooms drawn.
    pub rooms: usize,
    /// Rule 1: exits drawn against their compass direction.
    pub against_bearing: usize,
    /// ... of those, between two rooms the solver placed in different
    /// groups: where the groups were put, not how a group was solved.
    pub against_bearing_across: usize,
    /// Rule 2: drawn lines that pass over a room they do not join.
    pub lines_through_rooms: usize,
    /// Rule 3: building rooms under a line whose ends are both outside the
    /// building.
    pub rooms_under_foreign_lines: usize,
    /// Rule 4: pairs of rooms joined only by exits with no direction.
    pub directionless: usize,
    /// ... of those, drawn as a line.
    pub directionless_lines: usize,
    /// ... drawn as a stub, the line too long to draw.
    pub directionless_stubs: usize,
    /// ... not drawn at all.
    pub directionless_undrawn: usize,
    /// ... drawn as a line that crosses another line.
    pub directionless_crossing: usize,
    /// ... drawn as a line that lies along another line ([`lies_along`]).
    pub directionless_along: usize,
    /// The drawn lengths of the directionless lines, in sheet cells.
    #[serde(skip)]
    pub directionless_lengths: Vec<f32>,
    /// What each rule counted, to look at: the exits against their
    /// direction, the lines through rooms with the rooms they cross, and the
    /// building rooms under a foreign line.
    #[serde(skip)]
    pub detail: Detail,
}

/// What a [`Quality`] counted, by room.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    /// Exits drawn against their direction: from, to.
    pub against: Vec<(RoomId, RoomId)>,
    /// Lines through rooms: its two ends, and the rooms it crosses.
    pub through: Vec<(RoomId, RoomId, Vec<RoomId>)>,
    /// Building rooms under a line not theirs.
    pub under: Vec<RoomId>,
}

impl Quality {
    /// Add `other`'s counts to these.
    pub fn add(&mut self, other: &Quality) {
        self.rooms += other.rooms;
        self.against_bearing += other.against_bearing;
        self.against_bearing_across += other.against_bearing_across;
        self.lines_through_rooms += other.lines_through_rooms;
        self.rooms_under_foreign_lines += other.rooms_under_foreign_lines;
        self.directionless += other.directionless;
        self.directionless_lines += other.directionless_lines;
        self.directionless_stubs += other.directionless_stubs;
        self.directionless_undrawn += other.directionless_undrawn;
        self.directionless_crossing += other.directionless_crossing;
        self.directionless_along += other.directionless_along;
        self.directionless_lengths
            .extend_from_slice(&other.directionless_lengths);
    }

    /// The `q`th quantile (0 to 1) of the directionless lines' lengths.
    #[must_use]
    pub fn directionless_length(&self, q: f32) -> Option<f32> {
        let mut lengths = self.directionless_lengths.clone();
        if lengths.is_empty() {
            return None;
        }
        lengths.sort_by(f32::total_cmp);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let at = ((lengths.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
        lengths.get(at).copied()
    }
}

/// `scene`'s counts: `layout` and `map` are what it was built from.
#[must_use]
pub fn measure(scene: &MapScene, layout: &Layout, map: &Map) -> Quality {
    // The directions the scene was drawn by, corrections included, as
    // `build_scene` takes them.
    let mut dirs = DirectionMap::build(map);
    dirs.apply_edge_overrides(map, &layout.edges);
    let sheet = &scene.sheet;
    let cell_of: HashMap<RoomId, (i32, i32, usize)> = sheet
        .rooms
        .iter()
        .map(|room| (room.id, (room.cell.x, room.cell.y, room.group)))
        .collect();
    let mut quality = Quality {
        rooms: sheet.rooms.len(),
        ..Quality::default()
    };

    // Rule 1, and the pairs rule 4 is about.
    let mut directionless: HashSet<(RoomId, RoomId)> = HashSet::new();
    for room in map.rooms() {
        let Some(&(ax, ay, ag)) = cell_of.get(&room.id) else {
            continue;
        };
        for exit in &room.exits {
            let Some(&(bx, by, bg)) = cell_of.get(&exit.to) else {
                continue;
            };
            match dirs.get(room.id, exit.to) {
                Some(dir) if dir.is_compass() => {
                    let (ex, ey) = dir.offset();
                    if (bx - ax).signum() != ex.signum() || (by - ay).signum() != ey.signum() {
                        quality.against_bearing += 1;
                        quality.detail.against.push((room.id, exit.to));
                        if ag != bg {
                            quality.against_bearing_across += 1;
                        }
                    }
                }
                None if dirs.get(exit.to, room.id).is_none()
                    && crate::regions::is_passage(exit)
                    && room.id != exit.to =>
                {
                    directionless.insert(pair(room.id, exit.to));
                }
                // Up and down, or a bearing only one way: not directionless.
                _ => {}
            }
        }
    }

    // Rules 2 and 3.
    let obstacles = Obstacles::of(&sheet.rooms);
    let mut under: HashSet<RoomId> = HashSet::new();
    for edge in sheet.edges.iter().filter(|e| e.kind != SceneEdgeKind::Stub) {
        let ends = [edge.a_room, edge.b_room];
        let crossed = obstacles.rooms_near(&path(edge), &ends);
        if crossed.is_empty() {
            continue;
        }
        quality.lines_through_rooms += 1;
        quality
            .detail
            .through
            .push((edge.a_room, edge.b_room, crossed.clone()));
        let end_units = [scene.unit_of(edge.a_room), scene.unit_of(edge.b_room)];
        for room in crossed {
            match scene.unit_of(room) {
                Some(unit) if unit != STREETS && !end_units.contains(&Some(unit)) => {
                    under.insert(room);
                }
                _ => {}
            }
        }
    }
    quality.rooms_under_foreign_lines = under.len();
    quality.detail.under = under.iter().copied().collect();
    quality.detail.under.sort_unstable();

    rule_four(&mut quality, sheet, &directionless);
    quality
}

/// Rule 4 over the pairs joined only with no direction: how each is drawn,
/// how long, and whether it crosses or lies along another line.
fn rule_four(
    quality: &mut Quality,
    sheet: &crate::scene::SheetScene,
    directionless: &HashSet<(RoomId, RoomId)>,
) {
    let drawn: HashMap<(RoomId, RoomId), &SceneEdge> = sheet
        .edges
        .iter()
        .map(|edge| (pair(edge.a_room, edge.b_room), edge))
        .collect();
    let lines: Vec<(Vec<Point>, [RoomId; 2])> = sheet
        .edges
        .iter()
        .filter(|e| e.kind != SceneEdgeKind::Stub)
        .map(|e| (path(e), [e.a_room, e.b_room]))
        .collect();
    quality.directionless = directionless.len();
    for ends in directionless {
        match drawn.get(ends) {
            None => quality.directionless_undrawn += 1,
            Some(edge) if edge.kind == SceneEdgeKind::Stub => quality.directionless_stubs += 1,
            Some(edge) => {
                quality.directionless_lines += 1;
                let own = path(edge);
                quality.directionless_lengths.push(length(&own));
                let crosses = lines.iter().any(|(other, other_ends)| {
                    !other_ends.contains(&edge.a_room)
                        && !other_ends.contains(&edge.b_room)
                        && paths_cross(&own, other)
                });
                if crosses {
                    quality.directionless_crossing += 1;
                }
                let along = lines.iter().any(|(other, other_ends)| {
                    *other_ends != [edge.a_room, edge.b_room]
                        && *other_ends != [edge.b_room, edge.a_room]
                        && lies_along(&own, other)
                });
                if along {
                    quality.directionless_along += 1;
                }
            }
        }
    }
}

/// An unordered pair, smaller id first.
fn pair(a: RoomId, b: RoomId) -> (RoomId, RoomId) {
    if a.0 <= b.0 { (a, b) } else { (b, a) }
}

/// A drawn edge as the polyline it is drawn along.
fn path(edge: &SceneEdge) -> Vec<Point> {
    let mut points = vec![point(edge.a)];
    points.extend(edge.via.iter().copied());
    points.push(point(edge.b));
    points
}

fn length(path: &[Point]) -> f32 {
    path.windows(2)
        .map(|s| (s[1].x - s[0].x).hypot(s[1].y - s[0].y))
        .sum()
}

/// How close, in sheet cells, two near-parallel lines run before they
/// read as one.
pub(crate) const ALONG_GAP: f32 = 1.0;

/// How far, in sheet cells, two lines must run that close to lie along
/// each other rather than meet.
pub(crate) const ALONG_RUN: f32 = 1.5;

/// Whether some segment of `a` lies along some segment of `b`: within
/// about ten degrees of parallel, both of `b`'s ends of that stretch
/// within [`ALONG_GAP`] of `a`'s line, and the two overlapping for more
/// than [`ALONG_RUN`]. Two lines leaving one room the same way count: from
/// a little way out they are drawn on top of each other.
pub(crate) fn lies_along(a: &[Point], b: &[Point]) -> bool {
    a.windows(2)
        .any(|s| b.windows(2).any(|t| segments_along(s[0], s[1], t[0], t[1])))
}

fn segments_along(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
    let (dx, dy) = (p2.x - p1.x, p2.y - p1.y);
    let (ex, ey) = (q2.x - q1.x, q2.y - q1.y);
    let (lp, lq) = (dx.hypot(dy), ex.hypot(ey));
    if lp < f32::EPSILON || lq < f32::EPSILON {
        return false;
    }
    // Near parallel: the sine of the angle between them under ten degrees.
    if (dx * ey - dy * ex).abs() / (lp * lq) > 0.17 {
        return false;
    }
    let (ux, uy) = (dx / lp, dy / lp);
    let off = |q: Point| ((q.x - p1.x) * uy - (q.y - p1.y) * ux).abs();
    if off(q1) > ALONG_GAP || off(q2) > ALONG_GAP {
        return false;
    }
    let along = |q: Point| (q.x - p1.x) * ux + (q.y - p1.y) * uy;
    let (a, b) = (along(q1).min(along(q2)), along(q1).max(along(q2)));
    b.min(lp) - a.max(0.0) > ALONG_RUN
}

/// Whether any segment of `a` properly crosses any segment of `b`.
fn paths_cross(a: &[Point], b: &[Point]) -> bool {
    a.windows(2)
        .any(|s| b.windows(2).any(|t| segments_cross(s[0], s[1], t[0], t[1])))
}

/// Whether segments `p1`-`p2` and `q1`-`q2` cross at a point inside both;
/// touching at an end, or lying along each other, is not a crossing.
pub(crate) fn segments_cross(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
    let cross =
        |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let d1 = cross(q1, q2, p1);
    let d2 = cross(q1, q2, p2);
    let d3 = cross(p1, p2, q1);
    let d4 = cross(p1, p2, q2);
    let eps = 1e-4;
    ((d1 > eps && d2 < -eps) || (d1 < -eps && d2 > eps))
        && ((d3 > eps && d4 < -eps) || (d3 < -eps && d4 > eps))
}

#[cfg(test)]
mod tests {
    use super::{Point, segments_cross};

    fn p(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    /// An X crosses; lines that meet at an end, run along each other, or
    /// miss do not.
    #[test]
    fn a_crossing_is_one_inside_both_segments() {
        assert!(segments_cross(
            p(0.0, 0.0),
            p(2.0, 2.0),
            p(0.0, 2.0),
            p(2.0, 0.0)
        ));
        assert!(!segments_cross(
            p(0.0, 0.0),
            p(1.0, 1.0),
            p(1.0, 1.0),
            p(2.0, 0.0)
        ));
        assert!(!segments_cross(
            p(0.0, 0.0),
            p(2.0, 0.0),
            p(1.0, 0.0),
            p(3.0, 0.0)
        ));
        assert!(!segments_cross(
            p(0.0, 0.0),
            p(1.0, 0.0),
            p(0.0, 1.0),
            p(1.0, 1.0)
        ));
    }
}
