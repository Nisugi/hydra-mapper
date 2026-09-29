//! Lines that go around rooms instead of through them.
//!
//! A straight line between two rooms can pass over a third -- a road from
//! A to C along a row with B in the middle, a door line across a corner
//! of a building, a passage inside a building cutting past its other
//! rooms. On `gs.map` 3,583 drawn lines did that, and a room under a line
//! reads as a stop on it. This bends such a line around what it would
//! cross, and leaves every other line straight.
//!
//! **A link is never hidden for want of a clean route.** Where no bend
//! clears, the straight line stays: a line through a room is a worse
//! drawing, a missing line is a wrong map.
//!
//! Then a line with no direction that crosses another is curved round the
//! crossing (the author, 2026-09-29: *"yellow connectors should also curve
//! around to find empty space"*), where a bend crosses fewer lines and
//! passes no room. A compass line keeps its straight run: its angle is its
//! direction.

use std::collections::{HashMap, HashSet};

use cena_map::RoomId;

use crate::positioner::Cell;
use crate::scene::{Point, SceneEdgeKind, SceneRoom, SheetScene};

/// How close, in sheet cells, a line may pass a room's centre before it
/// reads as running through the room. A drawn room is 18 px in a 28 px
/// cell: 0.32 of a cell each side of centre, and the line has width.
pub const CLEARANCE: f32 = 0.4;

/// Every full line on the sheet that crosses a room it does not connect,
/// rerouted around it where a bend can clear. Stubs are left alone: they
/// are drawn as ticks at their ends, not as lines.
pub fn route_edges(sheet: &mut SheetScene) {
    let rooms = Obstacles::of(&sheet.rooms);
    for edge in &mut sheet.edges {
        if edge.kind == SceneEdgeKind::Stub {
            continue;
        }
        let ends = [edge.a_room, edge.b_room];
        let (a, b) = (point(edge.a), point(edge.b));
        let blockers = rooms.near(&[a, b], &ends);
        if blockers.is_empty() {
            continue;
        }
        if let Some(via) = best_detour(a, b, &blockers, &rooms, &ends) {
            edge.via = via;
        }
    }
    curve_connectors(sheet, &rooms);
}

/// Each connector that crosses another line, bent to cross fewer where a
/// bend can without passing a room; the fewest crossings, then the
/// shortest, wins. Taken in order, each seeing the bends made before it.
fn curve_connectors(sheet: &mut SheetScene, rooms: &Obstacles) {
    let mut paths: Vec<Vec<Point>> = sheet.edges.iter().map(path_of).collect();
    for i in 0..sheet.edges.len() {
        let edge = &sheet.edges[i];
        if edge.kind != SceneEdgeKind::Connector {
            continue;
        }
        let ends = [edge.a_room, edge.b_room];
        let others: Vec<usize> = (0..sheet.edges.len())
            .filter(|&j| {
                let o = &sheet.edges[j];
                j != i
                    && o.kind != SceneEdgeKind::Stub
                    && !ends.contains(&o.a_room)
                    && !ends.contains(&o.b_room)
            })
            .collect();
        let crossed_at = |path: &[Point]| -> Vec<Point> {
            let mut at = Vec::new();
            for &j in &others {
                for s in path.windows(2) {
                    for t in paths[j].windows(2) {
                        if let Some(p) = crossing(s[0], s[1], t[0], t[1]) {
                            at.push(p);
                        }
                    }
                }
            }
            at
        };
        let now = crossed_at(&paths[i]);
        if now.is_empty() {
            continue;
        }
        let (a, b) = (point(edge.a), point(edge.b));
        let mut best: Option<(usize, f32, Vec<Point>)> = None;
        for via in bends(
            a,
            b,
            &now,
            &[1.0, 2.0, 3.0, 4.0, 6.0],
            &[0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0],
        ) {
            if via.iter().any(|p| same(*p, a) || same(*p, b)) {
                continue;
            }
            let mut path = vec![a];
            path.extend_from_slice(&via);
            path.push(b);
            let n = crossed_at(&path).len();
            let c = cost(&path);
            if n >= now.len()
                || best
                    .as_ref()
                    .is_some_and(|(bn, bc, _)| (*bn, *bc) <= (n, c))
            {
                continue;
            }
            if rooms.near(&path, &ends).is_empty() {
                best = Some((n, c, via));
            }
        }
        if let Some((_, _, via)) = best {
            sheet.edges[i].via.clone_from(&via);
            paths[i] = path_of(&sheet.edges[i]);
        }
    }
}

/// A drawn edge as the polyline it is drawn along.
fn path_of(edge: &crate::scene::SceneEdge) -> Vec<Point> {
    let mut path = vec![point(edge.a)];
    path.extend(edge.via.iter().copied());
    path.push(point(edge.b));
    path
}

/// Where segments `p1`-`p2` and `q1`-`q2` cross, if they cross inside both.
fn crossing(p1: Point, p2: Point, q1: Point, q2: Point) -> Option<Point> {
    if !crate::quality::segments_cross(p1, p2, q1, q2) {
        return None;
    }
    let (rx, ry) = (p2.x - p1.x, p2.y - p1.y);
    let (sx, sy) = (q2.x - q1.x, q2.y - q1.y);
    let denom = rx.mul_add(sy, -(ry * sx));
    let t = (q1.x - p1.x).mul_add(sy, -((q1.y - p1.y) * sx)) / denom;
    Some(Point {
        x: t.mul_add(rx, p1.x),
        y: t.mul_add(ry, p1.y),
    })
}

/// A path's length, and half a cell for each bend, so a straighter route
/// wins a near tie.
fn cost(path: &[Point]) -> f32 {
    const BEND_COST: f32 = 0.5;
    #[allow(clippy::cast_precision_loss)]
    let bends = path.len().saturating_sub(2) as f32;
    path.windows(2)
        .map(|s| (s[1].x - s[0].x).hypot(s[1].y - s[0].y))
        .sum::<f32>()
        + bends * BEND_COST
}

/// Room centres on the sheet, by cell, for asking what a line passes near.
/// Also what [`crate::quality`] measures a drawn sheet with, so the count of
/// lines through rooms is the one this pass works to lower.
pub(crate) struct Obstacles {
    at: HashMap<(i32, i32), Vec<RoomId>>,
}

impl Obstacles {
    /// The room centres of `rooms`.
    pub(crate) fn of(rooms: &[SceneRoom]) -> Obstacles {
        Obstacles::of_cells(rooms.iter().map(|room| (room.id, room.cell)))
    }

    /// The room centres at `cells`.
    pub(crate) fn of_cells(cells: impl IntoIterator<Item = (RoomId, Cell)>) -> Obstacles {
        let mut at: HashMap<(i32, i32), Vec<RoomId>> = HashMap::new();
        for (id, cell) in cells {
            at.entry((cell.x, cell.y)).or_default().push(id);
        }
        Obstacles { at }
    }

    /// The rooms, other than `ends`, that the polyline `path` passes within
    /// [`CLEARANCE`] of.
    pub(crate) fn rooms_near(&self, path: &[Point], ends: &[RoomId]) -> Vec<RoomId> {
        let mut out = Vec::new();
        for p in self.near(path, ends) {
            #[allow(clippy::cast_possible_truncation)]
            let cell = (p.x.round() as i32, p.y.round() as i32);
            if let Some(ids) = self.at.get(&cell) {
                out.extend(ids.iter().filter(|id| !ends.contains(id)));
            }
        }
        out
    }

    /// The cells of rooms, other than `ends`, that the polyline `path`
    /// passes within [`CLEARANCE`] of. Walked in half-cell steps, looking at
    /// the cells around each step, so the cost is the line's length and not
    /// the size of its bounding box.
    fn near(&self, path: &[Point], ends: &[RoomId]) -> Vec<Point> {
        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        let mut out = Vec::new();
        for pair in path.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let length = (to.x - from.x).hypot(to.y - from.y);
            // A segment over a hundred thousand cells long is not one this
            // map draws; the cap keeps a corrupt coordinate from hanging.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let steps = ((length * 2.0).ceil() as usize).clamp(1, 200_000);
            #[allow(clippy::cast_precision_loss)]
            for i in 0..=steps {
                let along = i as f32 / steps as f32;
                let (at_x, at_y) = (
                    from.x + (to.x - from.x) * along,
                    from.y + (to.y - from.y) * along,
                );
                #[allow(clippy::cast_possible_truncation)]
                let (cx, cy) = (at_x.round() as i32, at_y.round() as i32);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        let cell = (cx + dx, cy + dy);
                        if !seen.insert(cell) {
                            continue;
                        }
                        let Some(ids) = self.at.get(&cell) else {
                            continue;
                        };
                        if ids.iter().all(|id| ends.contains(id)) {
                            continue;
                        }
                        #[allow(clippy::cast_precision_loss)]
                        let p = Point {
                            x: cell.0 as f32,
                            y: cell.1 as f32,
                        };
                        if path
                            .windows(2)
                            .any(|s| segment_distance(p, s[0], s[1]) < CLEARANCE)
                        {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }
}

/// The shortest bend that clears every room: an L either way round, a
/// side-step to either side at a few distances, or a bow past each room
/// in the way. Each bend costs a little on top of its length, so a
/// straighter route wins a near tie. `None` when nothing clears.
fn best_detour(
    a: Point,
    b: Point,
    blockers: &[Point],
    rooms: &Obstacles,
    ends: &[RoomId],
) -> Option<Vec<Point>> {
    let candidates = bends(a, b, blockers, &[0.6, 1.0], &[0.5, 1.0, 1.5, 2.0, 3.0]);
    let price = |via: &[Point]| {
        let mut path = vec![a];
        path.extend_from_slice(via);
        path.push(b);
        cost(&path)
    };
    let mut best: Option<(f32, Vec<Point>)> = None;
    for via in candidates {
        // A corner on an end is no bend at all.
        if via.iter().any(|p| same(*p, a) || same(*p, b)) {
            continue;
        }
        let c = price(&via);
        if best.as_ref().is_some_and(|(bc, _)| *bc <= c) {
            continue;
        }
        let mut path = vec![a];
        path.extend_from_slice(&via);
        path.push(b);
        if rooms.near(&path, ends).is_empty() {
            best = Some((c, via));
        }
    }
    best.map(|(_, via)| via)
}

/// The bends tried for a line from `a` to `b`: an L either way round, a
/// side-step to either side at each of `steps`, and a bow past each of
/// `around` at each of `reaches`, either side.
fn bends(a: Point, b: Point, around: &[Point], reaches: &[f32], steps: &[f32]) -> Vec<Vec<Point>> {
    let mut candidates: Vec<Vec<Point>> = vec![
        vec![Point { x: a.x, y: b.y }],
        vec![Point { x: b.x, y: a.y }],
    ];
    // Side-steps perpendicular to the line's main axis.
    let along_x = (b.x - a.x).abs() >= (b.y - a.y).abs();
    for &k in steps {
        for sign in [1.0_f32, -1.0] {
            let (ox, oy) = if along_x {
                (0.0, k * sign)
            } else {
                (k * sign, 0.0)
            };
            candidates.push(vec![
                Point {
                    x: a.x + ox,
                    y: a.y + oy,
                },
                Point {
                    x: b.x + ox,
                    y: b.y + oy,
                },
            ]);
        }
    }
    // A bow past each point in the way, either side.
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = dx.hypot(dy).max(f32::EPSILON);
    let (nx, ny) = (-dy / length, dx / length);
    for p in around {
        for &reach in reaches {
            for sign in [1.0_f32, -1.0] {
                candidates.push(vec![Point {
                    x: p.x + nx * reach * sign,
                    y: p.y + ny * reach * sign,
                }]);
            }
        }
    }
    candidates
}

fn same(p: Point, q: Point) -> bool {
    (p.x - q.x).abs() < 1e-4 && (p.y - q.y).abs() < 1e-4
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn point(c: Cell) -> Point {
    Point {
        x: c.x as f32,
        y: c.y as f32,
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0)
    };
    (p.x - a.x - t * dx).hypot(p.y - a.y - t * dy)
}

#[cfg(test)]
mod tests {
    use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId};

    use super::{CLEARANCE, Point, path_of, segment_distance};
    use crate::scene::SceneEdgeKind;

    fn room(id: u32, exits: &[(u32, &str)]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![cena_map::Uid(i64::from(id))],
            title: vec![format!("[Road {id}]")],
            description: vec![],
            paths: vec!["Obvious paths: east, west".to_owned()],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits: exits
                .iter()
                .map(|&(to, cmd)| Exit {
                    to: RoomId(to),
                    kind: ExitKind::Go,
                    crossing: Crossing::Command(cmd.to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    /// Three rooms in a row, and a path from the first to the third: drawn
    /// straight it runs over the middle room, which then reads as a stop
    /// on it. It bends round, and the road between neighbours stays
    /// straight.
    #[test]
    fn a_line_bends_round_a_room_it_would_cross() {
        let map = Map::from_rooms(vec![
            room(1, &[(2, "east"), (3, "go path")]),
            room(2, &[(1, "west"), (3, "east")]),
            room(3, &[(2, "west"), (1, "go path")]),
        ])
        .expect("no duplicate ids");
        let layout = crate::generate_layout(&map);
        let scene = crate::build_scene("Test", &layout, &map);
        let (a, m, c) = (
            scene.room(RoomId(1)).expect("drawn").cell,
            scene.room(RoomId(2)).expect("drawn").cell,
            scene.room(RoomId(3)).expect("drawn").cell,
        );
        assert_eq!((a.y, m.y), (m.y, c.y), "the fixture is not a row");

        let path_edge = scene
            .sheet
            .edges
            .iter()
            .find(|e| e.a_room.0 + e.b_room.0 == 4)
            .expect("the path is drawn");
        assert_eq!(path_edge.kind, SceneEdgeKind::Connector);
        assert!(!path_edge.via.is_empty(), "the path was not routed");
        #[allow(clippy::cast_precision_loss)]
        let pt = |cell: crate::positioner::Cell| Point {
            x: cell.x as f32,
            y: cell.y as f32,
        };
        let mut path = vec![pt(path_edge.a)];
        path.extend_from_slice(&path_edge.via);
        path.push(pt(path_edge.b));
        assert!(
            path.windows(2)
                .all(|s| segment_distance(pt(m), s[0], s[1]) >= CLEARANCE),
            "the routed path still crosses the middle room: {path:?}"
        );
        let road = scene
            .sheet
            .edges
            .iter()
            .find(|e| e.a_room.0 + e.b_room.0 == 3)
            .expect("the road is drawn");
        assert!(road.via.is_empty(), "a clear road was bent");
    }

    /// A square of four rooms, joined round by compass exits, a compass
    /// line across one diagonal and a `go path` across the other. Drawn
    /// straight, the path crosses the diagonal; it curves round outside the
    /// square instead, past no room.
    #[test]
    fn a_connector_curves_round_a_line_it_would_cross() {
        let map = Map::from_rooms(vec![
            room(1, &[(2, "east"), (4, "south"), (3, "go path")]),
            room(2, &[(1, "west"), (3, "south"), (4, "southwest")]),
            room(3, &[(2, "north"), (4, "west"), (1, "go path")]),
            room(4, &[(3, "east"), (1, "north"), (2, "northeast")]),
        ])
        .expect("no duplicate ids");
        let layout = crate::generate_layout(&map);
        let scene = crate::build_scene("Test", &layout, &map);
        let edge = |a: u32, b: u32| {
            scene
                .sheet
                .edges
                .iter()
                .find(|e| {
                    (e.a_room, e.b_room) == (RoomId(a), RoomId(b))
                        || (e.a_room, e.b_room) == (RoomId(b), RoomId(a))
                })
                .expect("drawn")
        };
        let (path, diagonal) = (path_of(edge(1, 3)), path_of(edge(2, 4)));
        assert!(
            !path.windows(2).any(|s| diagonal
                .windows(2)
                .any(|d| crate::quality::segments_cross(s[0], s[1], d[0], d[1]))),
            "the path still crosses the diagonal: {path:?}"
        );
        for id in [2, 4] {
            let at = super::point(scene.room(RoomId(id)).expect("drawn").cell);
            assert!(
                path.windows(2)
                    .all(|s| segment_distance(at, s[0], s[1]) >= CLEARANCE),
                "the curved path runs over room {id}: {path:?}"
            );
        }
    }
}
