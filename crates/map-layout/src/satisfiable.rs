//! Whether a component's stated directions can be satisfied at all.
//!
//! A direction violation says the layout disagrees with an exit. It does
//! **not** say whose fault that is, and the two answers want opposite
//! responses: a contradiction in the map is for a person to correct,
//! while a solver that failed on good data is for us to fix. Until this
//! existed the panel presented both the same way, and the README asserted
//! the first -- wrongly, for most of them.
//!
//! # Every bearing is two constraints, and one of them is often equality
//!
//! The engine's sign rules make each compass direction a pair of
//! constraints, one per axis, and the axes are independent:
//!
//! ```text
//! b east of a       =>  x_a < x_b  and  y_a = y_b
//! b northeast of a  =>  x_a < x_b  and  y_b < y_a
//! b north of a      =>  x_a = x_b  and  y_b < y_a
//! ```
//!
//! **The equalities matter as much as the inequalities.** An earlier
//! version of this module dropped them -- "north says nothing about x" --
//! which made it answer a weaker question than the engine asks, and miss
//! every contradiction that runs through an alignment. `a` north of `b`,
//! `b` north of `c` and `a` east of `c` is unsatisfiable, and without the
//! equalities nothing here would have said so. (Found by ATARI, who also
//! supplied the four-room fixture the tests below pin.)
//!
//! So each axis is judged in two steps:
//!
//! 1. **Contract** the rooms an equality binds into one class. Two rooms
//!    on the same axis-class must share a coordinate, so they stand or
//!    fall together.
//! 2. **Check the classes** for a strict-order cycle. `x_a < x_b < x_a`
//!    cannot hold for any numbers, and neither can a cycle that passes
//!    through an alignment. A strict order *within* one class is the same
//!    contradiction: a room cannot be strictly east of something it must
//!    share an x with.
//!
//! A graph with no cycle can always be satisfied, by numbering the classes
//! in topological order -- so the whole question is union-find plus cycle
//! detection, O(V+E) either way, with no search and no arithmetic.
//!
//! # Forced overlap
//!
//! Satisfying every constraint is not enough on its own: two rooms forced
//! into the same class on **both** axes must occupy the same cell, and a
//! layout cannot draw that however the constraints are honoured. That is
//! its own kind of unsatisfiable, reported separately because the fix is
//! different -- an exit claiming a room is in two places at once, rather
//! than a loop of bearings.
//!
//! Up and down are excluded, as they are from validation: they borrow the
//! north/south offsets as a placement convenience and are not 2D geometry
//! (`plan/26` §0).

use std::collections::{BTreeMap, HashMap, HashSet};

use cena_map::{Map, RoomId};

use crate::direction::DirectionMap;
use crate::positioner::Cell;

/// Which axis a constraint lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// East and west: the x ordering.
    EastWest,
    /// North and south: the y ordering.
    NorthSouth,
}

impl Axis {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Axis::EastWest => "east/west",
            Axis::NorthSouth => "north/south",
        }
    }
}

/// Why a component's stated directions cannot all hold at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A loop of strict orderings on one axis: each room must be strictly
    /// one side of the next, all the way round.
    ///
    /// The loop may run through an alignment -- `a` north of `b`, `b`
    /// north of `c`, `a` east of `c` -- which is why equalities are
    /// contracted before the cycle is looked for.
    Cycle {
        axis: Axis,
        /// The rooms of the loop, first repeated at the end so it reads
        /// as a loop.
        cycle: Vec<RoomId>,
    },
    /// One room must be strictly east (or north) of another it is also
    /// aligned with: `a` east of `b` while something else holds their x
    /// equal.
    OrderWithinAlignment {
        axis: Axis,
        from: RoomId,
        to: RoomId,
    },
    /// Two rooms forced onto the same cell -- aligned on both axes, with
    /// nothing separating them.
    ForcedOverlap { a: RoomId, b: RoomId },
}

impl Problem {
    /// The rooms this problem implicates, for a panel to name.
    #[must_use]
    pub fn rooms(&self) -> Vec<RoomId> {
        match self {
            Problem::Cycle { cycle, .. } => cycle.clone(),
            Problem::OrderWithinAlignment { from, to, .. } => vec![*from, *to],
            Problem::ForcedOverlap { a, b } => vec![*a, *b],
        }
    }
}

/// Everything that stops a component's directions being satisfied at
/// once, or an empty list when they can be.
///
/// **An empty list is a strong claim**: an arrangement exists that
/// satisfies every stated bearing's signs. If the layout still reports
/// violations, they are the solver's, not the map's.
#[must_use]
pub fn problems(rooms: &[RoomId], map: &Map, dirs: &DirectionMap) -> Vec<Problem> {
    let present: HashSet<RoomId> = rooms.iter().copied().collect();
    let mut out = Vec::new();

    let mut classes: Vec<Union> = Vec::new();
    for axis in [Axis::EastWest, Axis::NorthSouth] {
        let edges = axis_edges(rooms, map, dirs, &present, axis);
        let mut union = Union::new(rooms);
        for e in edges.iter().filter(|e| e.equal) {
            union.join(e.from, e.to);
        }
        // A strict order inside one alignment class is a contradiction on
        // its own: the rooms must share a coordinate and differ in it.
        for e in edges.iter().filter(|e| !e.equal) {
            if union.find(e.from) == union.find(e.to) {
                out.push(Problem::OrderWithinAlignment {
                    axis,
                    from: e.from,
                    to: e.to,
                });
            }
        }
        let graph = contracted(&edges, &mut union);
        out.extend(cycles(rooms, &graph, &mut union, axis));
        classes.push(union);
    }

    // Aligned on both axes: the same cell, which cannot be drawn.
    if let [x, y] = classes.as_mut_slice() {
        let mut seen: BTreeMap<(RoomId, RoomId), RoomId> = BTreeMap::new();
        for &room in rooms {
            let key = (x.find(room), y.find(room));
            if let Some(&first) = seen.get(&key) {
                out.push(Problem::ForcedOverlap { a: first, b: room });
            } else {
                seen.insert(key, room);
            }
        }
    }

    out
}

/// Whether every stated direction among these rooms can hold at once.
#[must_use]
pub fn is_satisfiable(rooms: &[RoomId], map: &Map, dirs: &DirectionMap) -> bool {
    problems(rooms, map, dirs).is_empty()
}

/// One axis constraint between two rooms: either they must be equal on
/// this axis, or `from` must come strictly before `to`.
struct AxisEdge {
    from: RoomId,
    to: RoomId,
    equal: bool,
}

/// Every constraint the stated bearings place on one axis, equalities
/// included.
fn axis_edges(
    rooms: &[RoomId],
    map: &Map,
    dirs: &DirectionMap,
    present: &HashSet<RoomId>,
    axis: Axis,
) -> Vec<AxisEdge> {
    let mut edges = Vec::new();
    for &from in rooms {
        let Some(room) = map.room(from) else {
            continue;
        };
        for exit in &room.exits {
            let to = exit.to;
            if !present.contains(&to) || from == to {
                continue;
            }
            let Some(dir) = dirs.get(from, to) else {
                continue;
            };
            if !dir.is_compass() {
                continue;
            }
            let (dx, dy) = dir.offset();
            let step = match axis {
                Axis::EastWest => dx,
                Axis::NorthSouth => dy,
            };
            edges.push(match step {
                1 => AxisEdge {
                    from,
                    to,
                    equal: false,
                },
                -1 => AxisEdge {
                    from: to,
                    to: from,
                    equal: false,
                },
                // No component on this axis: the engine's sign rules make
                // that an alignment, not an absence of information.
                _ => AxisEdge {
                    from,
                    to,
                    equal: true,
                },
            });
        }
    }
    edges
}

/// The strict-order graph over alignment classes, deduplicated and in a
/// stable order so the same map reports the same cycle every run.
fn contracted(edges: &[AxisEdge], union: &mut Union) -> BTreeMap<RoomId, Vec<RoomId>> {
    let mut graph: BTreeMap<RoomId, Vec<RoomId>> = BTreeMap::new();
    for e in edges.iter().filter(|e| !e.equal) {
        let (a, b) = (union.find(e.from), union.find(e.to));
        if a != b {
            graph.entry(a).or_default().push(b);
        }
    }
    for targets in graph.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }
    graph
}

/// Cycles in the contracted graph, by iterative depth-first search.
///
/// One cycle per entry point rather than every distinct loop through the
/// same rooms: breaking one breaks them all, and enumerating them is
/// exponential for no gain.
fn cycles(
    rooms: &[RoomId],
    graph: &BTreeMap<RoomId, Vec<RoomId>>,
    union: &mut Union,
    axis: Axis,
) -> Vec<Problem> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done,
    }
    let mut mark: HashMap<RoomId, Mark> = HashMap::new();
    let mut out = Vec::new();
    // Iterative, because the real map has components of thousands of
    // rooms and a recursive walk would overflow the stack.
    let mut starts: Vec<RoomId> = rooms.iter().map(|&r| union.find(r)).collect();
    starts.sort_unstable();
    starts.dedup();
    for start in starts {
        if mark.contains_key(&start) {
            continue;
        }
        let mut path = vec![start];
        let mut next = vec![0usize];
        mark.insert(start, Mark::Open);
        while let Some(&room) = path.last() {
            let index = next
                .last_mut()
                .unwrap_or_else(|| unreachable!("path and next stay the same length"));
            let Some(&target) = graph.get(&room).and_then(|t| t.get(*index)) else {
                mark.insert(room, Mark::Done);
                path.pop();
                next.pop();
                continue;
            };
            *index += 1;
            match mark.get(&target) {
                Some(Mark::Done) => {}
                Some(Mark::Open) => {
                    if let Some(at) = path.iter().position(|&r| r == target) {
                        let mut cycle: Vec<RoomId> = path[at..].to_vec();
                        cycle.push(target);
                        out.push(Problem::Cycle { axis, cycle });
                    }
                }
                None => {
                    mark.insert(target, Mark::Open);
                    path.push(target);
                    next.push(0);
                }
            }
        }
    }
    out
}

/// Disjoint sets over rooms, for the alignment classes.
struct Union {
    parent: HashMap<RoomId, RoomId>,
}

impl Union {
    fn new(rooms: &[RoomId]) -> Union {
        Union {
            parent: rooms.iter().map(|&r| (r, r)).collect(),
        }
    }

    fn find(&mut self, of: RoomId) -> RoomId {
        let mut root = of;
        while let Some(&up) = self.parent.get(&root) {
            if up == root {
                break;
            }
            root = up;
        }
        // Path compression, so repeated lookups over a long chain stay
        // cheap.
        let mut at = of;
        while let Some(&up) = self.parent.get(&at) {
            if up == root {
                break;
            }
            self.parent.insert(at, root);
            at = up;
        }
        root
    }

    fn join(&mut self, a: RoomId, b: RoomId) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Lower id wins, so the class representative -- and every
            // report naming it -- is the same on every run.
            let (keep, drop) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent.insert(drop, keep);
        }
    }
}

/// An arrangement satisfying every stated bearing's signs, or `None` when
/// the rooms cannot be satisfied.
///
/// **This cannot fail on satisfiable data**: each axis numbers its
/// alignment classes in topological order, so every strict constraint
/// `a < b` holds because `a`'s class was numbered first, and every
/// equality holds because both rooms share a class. No search, no
/// backtracking, nothing to get stuck in.
///
/// The result is *correct*, not *pretty*: a class sits one rank past its
/// predecessors, which spreads a component out more than BFS placement
/// does. It is meant as the answer when the ordinary passes have left a
/// violation behind, with compaction tidying afterwards.
#[must_use]
pub fn place_by_order(
    rooms: &[RoomId],
    map: &Map,
    dirs: &DirectionMap,
) -> Option<HashMap<RoomId, Cell>> {
    if !problems(rooms, map, dirs).is_empty() {
        return None;
    }
    let present: HashSet<RoomId> = rooms.iter().copied().collect();
    let x = ranks(rooms, map, dirs, &present, Axis::EastWest)?;
    let y = ranks(rooms, map, dirs, &present, Axis::NorthSouth)?;

    let mut out: HashMap<RoomId, Cell> = HashMap::new();
    for &room in rooms {
        out.insert(
            room,
            Cell {
                x: x.at.get(&room).copied().unwrap_or(0),
                y: y.at.get(&room).copied().unwrap_or(0),
            },
        );
    }
    // **A room nothing constrains on an axis goes where its neighbours
    // are.** Ranking gives it 0 -- the origin -- and every such room in
    // a component lands on one cell there: a tower reached by up and
    // down, a yard reached by "out", eight of them stacked at (0,0) in
    // Wehnimer's Landing. The map still says what they are next to, so
    // they take a neighbour's coordinate on the free axis, and any two
    // still sharing a cell are nudged apart along an axis that is free
    // for one of them. Nothing constrained is moved.
    let neighbours = neighbours_within(rooms, map, &present);
    settle_free(&mut out, rooms, &neighbours, &x.free, &y.free);
    unstack(&mut out, rooms, &x, &y);
    Some(out)
}

/// Each room's neighbours by any exit, within the component, both ways.
fn neighbours_within(
    rooms: &[RoomId],
    map: &Map,
    present: &HashSet<RoomId>,
) -> HashMap<RoomId, Vec<RoomId>> {
    let mut out: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for &room in rooms {
        let Some(r) = map.room(room) else {
            continue;
        };
        for exit in r.exits.iter().filter(|e| crate::regions::is_passage(e)) {
            if exit.to != room && present.contains(&exit.to) {
                out.entry(room).or_default().push(exit.to);
                out.entry(exit.to).or_default().push(room);
            }
        }
    }
    out
}

/// A room free on an axis takes a neighbour's coordinate there,
/// preferring a neighbour that is constrained on that axis; repeated
/// until nothing changes, so a chain of free rooms follows its one
/// anchored end.
fn settle_free(
    out: &mut HashMap<RoomId, Cell>,
    rooms: &[RoomId],
    neighbours: &HashMap<RoomId, Vec<RoomId>>,
    free_x: &HashSet<RoomId>,
    free_y: &HashSet<RoomId>,
) {
    for _ in 0..rooms.len() {
        let mut changed = false;
        for &room in rooms {
            let list = neighbours.get(&room).map_or(&[] as &[_], Vec::as_slice);
            if free_x.contains(&room)
                && let Some(&n) = list
                    .iter()
                    .find(|n| !free_x.contains(n))
                    .or_else(|| list.first())
                && out[&n].x != out[&room].x
            {
                let x = out[&n].x;
                if let Some(cell) = out.get_mut(&room) {
                    cell.x = x;
                    changed = true;
                }
            }
            if free_y.contains(&room)
                && let Some(&n) = list
                    .iter()
                    .find(|n| !free_y.contains(n))
                    .or_else(|| list.first())
                && out[&n].y != out[&room].y
            {
                let y = out[&n].y;
                if let Some(cell) = out.get_mut(&room) {
                    cell.y = y;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Two rooms on one cell: the later one steps to the nearest free cell
/// within what its constraints allow on each axis -- anywhere, on an
/// axis nothing constrains; between its nearest predecessor and
/// successor otherwise. A room with no room to move stays put.
fn unstack(out: &mut HashMap<RoomId, Cell>, rooms: &[RoomId], x: &Ranked, y: &Ranked) {
    let mut count: HashMap<Cell, usize> = HashMap::new();
    for c in out.values() {
        *count.entry(*c).or_default() += 1;
    }
    let reach = i32::try_from(rooms.len()).unwrap_or(i32::MAX);
    for &room in rooms {
        let here = out[&room];
        if count.get(&here).copied().unwrap_or(0) < 2 {
            continue;
        }
        let (xlo, xhi) = x.range(room, here.x);
        let (ylo, yhi) = y.range(room, here.y);
        let mut best: Option<(i32, Cell)> = None;
        for dx in -reach..=reach {
            let cx = here.x.saturating_add(dx);
            if cx < xlo || cx > xhi {
                continue;
            }
            for dy in -reach..=reach {
                let cy = here.y.saturating_add(dy);
                if cy < ylo || cy > yhi {
                    continue;
                }
                let cell = Cell { x: cx, y: cy };
                let cost = dx.abs().max(dy.abs());
                if best.is_some_and(|(c, _)| c <= cost) {
                    continue;
                }
                if !count.contains_key(&cell) {
                    best = Some((cost, cell));
                }
            }
        }
        if let Some((_, cell)) = best {
            *count.entry(here).or_default() -= 1;
            *count.entry(cell).or_default() += 1;
            out.insert(room, cell);
        }
    }
}

/// Which rooms nothing constrains on this axis, and, for the rest that
/// stand alone in their class, the ranks they must sit strictly between.
#[allow(clippy::type_complexity)]
fn freedom(
    rooms: &[RoomId],
    graph: &BTreeMap<RoomId, Vec<RoomId>>,
    rank: &HashMap<RoomId, i32>,
    has_pred: &HashSet<RoomId>,
    union: &mut Union,
) -> (HashSet<RoomId>, HashMap<RoomId, (Option<i32>, Option<i32>)>) {
    let mut class_size: HashMap<RoomId, usize> = HashMap::new();
    for &r in rooms {
        *class_size.entry(union.find(r)).or_default() += 1;
    }
    let free: HashSet<RoomId> = rooms
        .iter()
        .copied()
        .filter(|&r| {
            let class = union.find(r);
            !has_pred.contains(&class)
                && graph.get(&class).is_none_or(Vec::is_empty)
                && class_size.get(&class) == Some(&1)
        })
        .collect();
    // Bounds, for singleton classes only: the nearest predecessor's and
    // successor's ranks.
    let mut nearest_pred: HashMap<RoomId, i32> = HashMap::new();
    for (&from, targets) in graph {
        for &to in targets {
            let r = rank.get(&from).copied().unwrap_or(0);
            let slot = nearest_pred.entry(to).or_insert(r);
            *slot = (*slot).max(r);
        }
    }
    let bounds: HashMap<RoomId, (Option<i32>, Option<i32>)> = rooms
        .iter()
        .filter_map(|&r| {
            let class = union.find(r);
            if class_size.get(&class) != Some(&1) {
                return None;
            }
            let lo = nearest_pred.get(&class).copied();
            let hi = graph
                .get(&class)
                .and_then(|t| t.iter().filter_map(|s| rank.get(s)).min())
                .copied();
            Some((r, (lo, hi)))
        })
        .collect();
    (free, bounds)
}

/// One axis, ranked.
struct Ranked {
    at: HashMap<RoomId, i32>,
    /// Rooms no constraint touches on this axis.
    free: HashSet<RoomId>,
    /// Per singleton room: the nearest predecessor's and successor's
    /// ranks, the coordinates it must sit strictly between.
    bounds: HashMap<RoomId, (Option<i32>, Option<i32>)>,
}

impl Ranked {
    /// Where a room may sit on this axis without breaking an order,
    /// given where it is now (an equality class keeps its coordinate:
    /// moving one member alone would part it from the rest).
    fn range(&self, room: RoomId, now: i32) -> (i32, i32) {
        if self.free.contains(&room) {
            return (i32::MIN / 2, i32::MAX / 2);
        }
        // Never below where it is: a predecessor may itself have been
        // nudged up, and only a move upward is safe against every
        // successor, which stays at or above its rank.
        match self.bounds.get(&room) {
            Some(&(_, hi)) => (now, hi.map_or(i32::MAX / 2, |h| h - 1)),
            None => (now, now),
        }
    }
}

/// Longest-path rank per room on one axis: how many strict constraints
/// must come before its alignment class.
///
/// Longest path rather than any topological numbering, because a class
/// must sit strictly after **every** predecessor, not just the first one
/// reached. Then a class with successors and no predecessors is pulled
/// up to just before its nearest successor: rank 0 is the origin, and a
/// room that is merely west of something belongs beside that something,
/// not at the far edge of the component with every other such room.
fn ranks(
    rooms: &[RoomId],
    map: &Map,
    dirs: &DirectionMap,
    present: &HashSet<RoomId>,
    axis: Axis,
) -> Option<Ranked> {
    let edges = axis_edges(rooms, map, dirs, present, axis);
    let mut union = Union::new(rooms);
    for e in edges.iter().filter(|e| e.equal) {
        union.join(e.from, e.to);
    }
    let graph = contracted(&edges, &mut union);

    let mut classes: Vec<RoomId> = rooms.iter().map(|&r| union.find(r)).collect();
    classes.sort_unstable();
    classes.dedup();

    let mut incoming: HashMap<RoomId, usize> = classes.iter().map(|&c| (c, 0)).collect();
    for targets in graph.values() {
        for t in targets {
            *incoming.entry(*t).or_default() += 1;
        }
    }
    let mut ready: Vec<RoomId> = classes
        .iter()
        .copied()
        .filter(|c| incoming.get(c) == Some(&0))
        .collect();
    ready.sort_unstable();

    let mut rank: HashMap<RoomId, i32> = classes.iter().map(|&c| (c, 0)).collect();
    let mut done = 0usize;
    while let Some(class) = ready.pop() {
        done += 1;
        let here = rank.get(&class).copied().unwrap_or(0);
        for &target in graph.get(&class).map_or(&[] as &[_], Vec::as_slice) {
            let slot = rank.entry(target).or_default();
            *slot = (*slot).max(here + 1);
            let left = incoming.entry(target).or_default();
            *left = left.saturating_sub(1);
            if *left == 0 {
                ready.push(target);
                ready.sort_unstable();
            }
        }
    }
    if done != classes.len() {
        return None; // a class still waiting on itself: a cycle
    }
    // Sources pulled beside their successors.
    let mut has_pred: HashSet<RoomId> = HashSet::new();
    for targets in graph.values() {
        has_pred.extend(targets.iter().copied());
    }
    for &class in &classes {
        if has_pred.contains(&class) {
            continue;
        }
        if let Some(nearest) = graph
            .get(&class)
            .and_then(|t| t.iter().filter_map(|s| rank.get(s)).min())
        {
            rank.insert(class, nearest - 1);
        }
    }
    let (free, bounds) = freedom(rooms, &graph, &rank, &has_pred, &mut union);
    Some(Ranked {
        at: rooms
            .iter()
            .map(|&r| {
                let class = union.find(r);
                (r, rank.get(&class).copied().unwrap_or(0))
            })
            .collect(),
        free,
        bounds,
    })
}
