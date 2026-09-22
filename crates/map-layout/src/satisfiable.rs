//! Whether a component's stated directions can be satisfied at all.
//!
//! A direction violation says the layout disagrees with an exit. It does
//! **not** say whose fault that is, and the two answers want opposite
//! responses: a contradiction in the map is for a person to correct,
//! while a solver that failed on good data is for us to fix. Until this
//! existed the panel presented both the same way, and the README asserted
//! the first -- wrongly, for most of them.
//!
//! # Ordering, not geometry
//!
//! Every compass direction is a pair of **strict inequalities**, one per
//! axis, and the axes are independent:
//!
//! ```text
//! b east of a       =>  x_a < x_b
//! b northeast of a  =>  x_a < x_b  and  y_b < y_a
//! b north of a      =>              y_b < y_a
//! ```
//!
//! Nothing here cares how *far*, which is the point: a stretched edge
//! satisfies the same inequality a unit one does. A set of strict
//! inequalities is satisfiable exactly when its constraint graph has no
//! cycle -- `x_a < x_b < x_c < x_a` cannot hold for any numbers at all --
//! and a graph with no cycle can always be satisfied, by numbering it in
//! topological order.
//!
//! So the question "is this component satisfiable?" is two cycle
//! detections, in O(V+E), with no search and no arithmetic.
//!
//! Up and down are excluded, as they are from validation: they borrow the
//! north/south offsets as a placement convenience and are not 2D geometry
//! (`plan/26` §0).

use std::collections::{HashMap, HashSet};

use cena_map::{Map, RoomId};

use crate::direction::DirectionMap;

/// Which axis a constraint cycle lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// East and west: the cycle is in the x ordering.
    EastWest,
    /// North and south: the cycle is in the y ordering.
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

/// A set of rooms whose stated directions contradict each other.
///
/// The rooms form a loop on one axis -- each claims to be strictly one
/// side of the next, all the way round -- which no arrangement can
/// satisfy, however it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    pub axis: Axis,
    /// The rooms of the cycle, in the order the constraints chain, with
    /// the first repeated at the end so the loop reads as a loop.
    pub cycle: Vec<RoomId>,
}

/// Every contradiction among a component's rooms, or an empty list when
/// its directions can all be satisfied at once.
///
/// **An empty list is a strong claim**: it means an arrangement exists
/// that violates nothing. If the layout still has violations, they are
/// the solver's, not the map's.
#[must_use]
pub fn contradictions(rooms: &[RoomId], map: &Map, dirs: &DirectionMap) -> Vec<Contradiction> {
    let present: HashSet<RoomId> = rooms.iter().copied().collect();
    let mut out = Vec::new();
    for axis in [Axis::EastWest, Axis::NorthSouth] {
        let graph = constraints(rooms, map, dirs, &present, axis);
        out.extend(cycles(rooms, &graph, axis));
    }
    out
}

/// Whether every stated direction among these rooms can hold at once.
#[must_use]
pub fn is_satisfiable(rooms: &[RoomId], map: &Map, dirs: &DirectionMap) -> bool {
    contradictions(rooms, map, dirs).is_empty()
}

/// `a -> b` for every constraint reading "a is strictly before b" on this
/// axis: west of, for x; north of, for y.
fn constraints(
    rooms: &[RoomId],
    map: &Map,
    dirs: &DirectionMap,
    present: &HashSet<RoomId>,
    axis: Axis,
) -> HashMap<RoomId, Vec<RoomId>> {
    let mut graph: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for &from in rooms {
        let Some(room) = map.room(from) else {
            continue;
        };
        for exit in &room.exits {
            let to = exit.to;
            if !present.contains(&to) {
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
            // A bearing with no component on this axis constrains nothing
            // here -- "north" says nothing about x.
            match step {
                1 => graph.entry(from).or_default().push(to),
                -1 => graph.entry(to).or_default().push(from),
                _ => {}
            }
        }
    }
    // Deterministic order, so the cycle reported for a given map is the
    // same one every run.
    for targets in graph.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }
    graph
}

/// Every cycle in the constraint graph, found by depth-first search.
///
/// One cycle is reported per entry point rather than every distinct cycle
/// through the same rooms: a person fixing the loop breaks all of them,
/// and enumerating them all is exponential for no gain.
fn cycles(
    rooms: &[RoomId],
    graph: &HashMap<RoomId, Vec<RoomId>>,
    axis: Axis,
) -> Vec<Contradiction> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done,
    }
    let mut mark: HashMap<RoomId, Mark> = HashMap::new();
    let mut out = Vec::new();
    // Iterative DFS, so a long chain cannot overflow the stack: the real
    // map has components of thousands of rooms.
    for &start in rooms {
        if mark.contains_key(&start) {
            continue;
        }
        let mut path: Vec<RoomId> = Vec::new();
        let mut next: Vec<usize> = Vec::new();
        path.push(start);
        next.push(0);
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
                // Back to a room still on the path: the constraints from
                // there onward form a loop.
                Some(Mark::Open) => {
                    if let Some(at) = path.iter().position(|&r| r == target) {
                        let mut cycle: Vec<RoomId> = path[at..].to_vec();
                        cycle.push(target);
                        out.push(Contradiction { axis, cycle });
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

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Cost, Crossing, Exit, ExitKind, Room};

    fn exit(to: u32, command: &str) -> Exit {
        Exit {
            to: RoomId(to),
            kind: ExitKind::Cardinal,
            crossing: Crossing::Command(command.to_owned()),
            cost: Some(Cost::Fixed(1.0)),
        }
    }

    fn room(id: u32, exits: Vec<Exit>) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![format!("[R{id}]")],
            description: vec![],
            paths: vec![],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits,
        }
    }

    fn check(rooms: Vec<Room>) -> Vec<Contradiction> {
        let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let dirs = DirectionMap::build(&map);
        contradictions(&ids, &map, &dirs)
    }

    /// The fixture ATARI supplied: the engine reports two violations on
    /// it, and this says the data is fine -- which is the whole point of
    /// the module. Every stated direction holds at
    /// `0=(0,2) 1=(1,0) 2=(2,1) 3=(0,0)`.
    #[test]
    fn the_four_room_counterexample_is_satisfiable() {
        let found = check(vec![
            room(
                0,
                vec![exit(1, "northeast"), exit(2, "northeast"), exit(3, "north")],
            ),
            room(1, vec![exit(0, "southwest"), exit(2, "southeast")]),
            room(2, vec![exit(0, "southwest"), exit(1, "northwest")]),
            room(3, vec![exit(0, "south")]),
        ]);
        assert!(
            found.is_empty(),
            "called satisfiable data contradictory: {found:?}"
        );
    }

    /// Two rooms each claiming the other is east. No arrangement holds.
    #[test]
    fn mutual_east_is_a_contradiction() {
        let found = check(vec![
            room(1, vec![exit(2, "east")]),
            room(2, vec![exit(1, "east")]),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].axis, Axis::EastWest);
    }

    /// A longer loop: a < b < c < a on one axis. Each edge is reasonable
    /// alone, which is why this needs finding rather than eyeballing.
    #[test]
    fn a_three_room_loop_is_a_contradiction() {
        let found = check(vec![
            room(1, vec![exit(2, "east")]),
            room(2, vec![exit(3, "east")]),
            room(3, vec![exit(1, "east")]),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].axis, Axis::EastWest);
        // The loop is reported as a loop: first room repeated at the end.
        let cycle = &found[0].cycle;
        assert_eq!(cycle.first(), cycle.last());
    }

    /// A contradiction on one axis does not condemn the other: these
    /// rooms disagree about north/south while their x ordering is fine.
    #[test]
    fn each_axis_is_judged_on_its_own() {
        let found = check(vec![
            room(1, vec![exit(2, "north")]),
            room(2, vec![exit(1, "north")]),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].axis, Axis::NorthSouth);
    }

    /// An ordinary square is satisfiable, and so is a long chain -- the
    /// check must not cry contradiction over normal maps.
    #[test]
    fn ordinary_shapes_are_satisfiable() {
        assert!(
            check(vec![
                room(1, vec![exit(2, "east"), exit(3, "south")]),
                room(2, vec![exit(1, "west"), exit(4, "south")]),
                room(3, vec![exit(1, "north"), exit(4, "east")]),
                room(4, vec![exit(2, "north"), exit(3, "west")]),
            ])
            .is_empty()
        );

        // A hundred rooms in a line: deep enough to overflow a recursive
        // search, which is why the walk is iterative.
        let long: Vec<Room> = (1..=100u32)
            .map(|id| {
                let mut e = vec![];
                if id > 1 {
                    e.push(exit(id - 1, "west"));
                }
                if id < 100 {
                    e.push(exit(id + 1, "east"));
                }
                room(id, e)
            })
            .collect();
        assert!(check(long).is_empty());
    }

    /// Stretched edges are ordering, not distance: three rooms in a row
    /// where the far pair also names a bearing directly.
    #[test]
    fn a_stretched_edge_is_no_contradiction() {
        assert!(
            check(vec![
                room(1, vec![exit(2, "east"), exit(3, "east")]),
                room(2, vec![exit(1, "west"), exit(3, "east")]),
                room(3, vec![exit(1, "west"), exit(2, "west")]),
            ])
            .is_empty()
        );
    }
}
