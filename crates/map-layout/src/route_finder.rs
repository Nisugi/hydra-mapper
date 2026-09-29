//! A way round for a line with no direction: a search over the sheet's
//! grid, step by step between the rooms, that may turn as often and go as
//! far out of its way as it needs (the author, 2026-09-29: *"those yellow
//! lines can bend a lot more and go out of their way a bit to fit between
//! places"*). A step into a cell another line runs through costs
//! [`LINE_COST`], far more than the length it saves, so a route goes round
//! a line where it can. `routing::curve_connectors` keeps what this finds
//! only when it crosses fewer lines than what was drawn.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::positioner::Cell;
use crate::scene::Point;

/// What a step into a cell another line runs through costs, in cells.
const LINE_COST: f32 = 30.0;

/// What a turn costs, in cells, so a straighter route wins a near tie.
const TURN_COST: f32 = 0.5;

/// How far past the box around the two ends a route may wander, in cells.
const WANDER: i32 = 12;

/// A cell and the step that reached it (8: none yet).
type State = ((i32, i32), usize);

/// The eight steps, straight ones first.
const STEPS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// How near a drawn line a route's cell may be before stepping into it
/// costs [`LINE_COST`]: beyond a cell, so a route keeps its distance
/// rather than running alongside (`quality::ALONG_GAP`).
const NEAR_LINE: f32 = 1.2;

/// The cells a drawn line runs through or passes within [`NEAR_LINE`] of.
pub(crate) fn cells_of(path: &[Point]) -> Vec<(i32, i32)> {
    let mut out: HashSet<(i32, i32)> = HashSet::new();
    for s in path.windows(2) {
        let (dx, dy) = (s[1].x - s[0].x, s[1].y - s[0].y);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let samples = (dx.hypot(dy) * 4.0).ceil().max(1.0) as u32;
        for k in 0..=samples {
            #[allow(clippy::cast_precision_loss)]
            let t = k as f32 / samples as f32;
            let (x, y) = (t.mul_add(dx, s[0].x), t.mul_add(dy, s[0].y));
            #[allow(clippy::cast_possible_truncation)]
            for cx in (x.floor() as i32 - 1)..=(x.ceil() as i32 + 1) {
                for cy in (y.floor() as i32 - 1)..=(y.ceil() as i32 + 1) {
                    #[allow(clippy::cast_precision_loss)]
                    if (cx as f32 - x).hypot(cy as f32 - y) < NEAR_LINE {
                        out.insert((cx, cy));
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

/// A route from `a` to `b`, as its bends (neither end included), through
/// no cell of `rooms` but the two ends, paying [`LINE_COST`] for each cell
/// `lined` says another line runs through. `None` when the ends cannot
/// reach each other within [`WANDER`].
pub(crate) fn find(
    a: Cell,
    b: Cell,
    rooms: &HashSet<(i32, i32)>,
    lined: impl Fn((i32, i32)) -> bool,
) -> Option<Vec<Point>> {
    let (lo_x, hi_x) = (a.x.min(b.x) - WANDER, a.x.max(b.x) + WANDER);
    let (lo_y, hi_y) = (a.y.min(b.y) - WANDER, a.y.max(b.y) + WANDER);
    let start = (a.x, a.y);
    let goal = (b.x, b.y);
    let guess = |(x, y): (i32, i32)| {
        let (dx, dy) = ((x - goal.0).abs(), (y - goal.1).abs());
        #[allow(clippy::cast_precision_loss)]
        let (lo, hi) = (dx.min(dy) as f32, dx.max(dy) as f32);
        lo.mul_add(std::f32::consts::SQRT_2, hi - lo)
    };
    // State: a cell and the step that reached it (8 = none yet).
    let mut best: HashMap<State, f32> = HashMap::new();
    let mut from: HashMap<State, State> = HashMap::new();
    let mut open = BinaryHeap::new();
    best.insert((start, 8), 0.0);
    open.push(Open {
        priority: guess(start),
        cost: 0.0,
        at: start,
        step: 8,
    });
    while let Some(Open { cost, at, step, .. }) = open.pop() {
        if at == goal {
            return Some(bends_of(&from, (at, step), start));
        }
        if best.get(&(at, step)).is_some_and(|&c| c < cost) {
            continue;
        }
        for (k, &(sx, sy)) in STEPS.iter().enumerate() {
            let next = (at.0 + sx, at.1 + sy);
            if next.0 < lo_x || next.0 > hi_x || next.1 < lo_y || next.1 > hi_y {
                continue;
            }
            if next != goal && rooms.contains(&next) {
                continue;
            }
            let mut c = cost + if k < 4 { 1.0 } else { std::f32::consts::SQRT_2 };
            if step != 8 && step != k {
                c += TURN_COST;
            }
            if next != goal && lined(next) {
                c += LINE_COST;
            }
            if best.get(&(next, k)).is_none_or(|&old| c < old) {
                best.insert((next, k), c);
                from.insert((next, k), (at, step));
                open.push(Open {
                    priority: c + guess(next),
                    cost: c,
                    at: next,
                    step: k,
                });
            }
        }
    }
    None
}

/// The route walked back from `end`, as the cells where it turns.
fn bends_of(from: &HashMap<State, State>, end: State, start: (i32, i32)) -> Vec<Point> {
    let mut cells: Vec<State> = vec![end];
    let mut at = end;
    while at.0 != start {
        at = from[&at];
        cells.push(at);
    }
    cells.reverse();
    let mut bends = Vec::new();
    for w in cells.windows(2) {
        // A turn at w[0]: the step into it differs from the step out.
        if w[0].1 != 8 && w[0].1 != w[1].1 {
            #[allow(clippy::cast_precision_loss)]
            bends.push(Point {
                x: w[0].0.0 as f32,
                y: w[0].0.1 as f32,
            });
        }
    }
    bends
}

/// A state waiting in the search, cheapest first.
struct Open {
    priority: f32,
    cost: f32,
    at: (i32, i32),
    step: usize,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.priority.total_cmp(&other.priority) == Ordering::Equal
    }
}

impl Eq for Open {}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .total_cmp(&self.priority)
            .then_with(|| other.at.cmp(&self.at))
            .then_with(|| other.step.cmp(&self.step))
    }
}
