//! An arrangement satisfying every stated bearing, found by moving the
//! solver's own as little as the bearings force.
//!
//! [`super::place_by_order`] numbers every alignment class from scratch:
//! correct, but it spreads a component out, and taken whole it swapped the
//! Landing's wing columns and tangled Mist Harbor, so the positioner keeps
//! it only when edges grow no longer. The violations it could have fixed
//! stay: 664 of the 720 on `gs.map` are inside one group (Hydra's `plan/53`
//! §3, Stage 1).
//!
//! This keeps the solver's placement as the starting point. Along each
//! axis, the classes are visited in the order the bearings impose, and each
//! sits where its rooms already are (their median) **unless** a class that
//! must come before it is at or past that, in which case it moves just one
//! step past it -- and so, in turn, does whatever must come after. A room
//! caught in no contradiction does not move; the ones that must move go
//! together, which is the coordinated shift the local repairs cannot make.
//! A room nothing constrains on an axis keeps its coordinate there.

use std::collections::{BTreeSet, HashMap, HashSet};

use cena_map::{Map, RoomId};

use crate::direction::DirectionMap;
use crate::positioner::Cell;

use super::{Axis, Ranked, problems, push_apart, ranks, separate_pieces, unstack};

/// An arrangement near `current` that satisfies every stated bearing's
/// signs, or `None` when the rooms cannot be satisfied.
#[must_use]
pub fn place_near(
    rooms: &[RoomId],
    current: &HashMap<RoomId, Cell>,
    map: &Map,
    dirs: &DirectionMap,
) -> Option<HashMap<RoomId, Cell>> {
    if !problems(rooms, map, dirs).is_empty() {
        return None;
    }
    let present: HashSet<RoomId> = rooms.iter().copied().collect();
    let x = ranks(rooms, map, dirs, &present, Axis::EastWest)?;
    let y = ranks(rooms, map, dirs, &present, Axis::NorthSouth)?;
    let xs = settle(&x, rooms, current, |c| c.x);
    let ys = settle(&y, rooms, current, |c| c.y);

    let mut out: HashMap<RoomId, Cell> = rooms
        .iter()
        .map(|&r| {
            (
                r,
                Cell {
                    x: xs.get(&r).copied().unwrap_or(0),
                    y: ys.get(&r).copied().unwrap_or(0),
                },
            )
        })
        .collect();
    unstack(&mut out, rooms, &x, &y);
    separate_pieces(&mut out, rooms, map, dirs, &present);
    push_apart(&mut out, rooms, &x, &y);
    Some(out)
}

/// Each room's coordinate on one axis: its class at its members' median
/// now, or one past its latest predecessor if that is further on; a room
/// nothing constrains where it is.
fn settle(
    ranked: &Ranked,
    rooms: &[RoomId],
    current: &HashMap<RoomId, Cell>,
    coord: fn(&Cell) -> i32,
) -> HashMap<RoomId, i32> {
    let now = |r: RoomId| current.get(&r).map_or(0, coord);

    // Predecessors, from the successor lists.
    let mut preds: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for (&from, targets) in &ranked.succ {
        for &to in targets {
            preds.entry(to).or_default().push(from);
        }
    }
    // Classes in the order the bearings impose: the rank is a topological
    // numbering, ties being unrelated.
    let mut classes: BTreeSet<(i32, RoomId)> = BTreeSet::new();
    for &r in rooms {
        let class = ranked.class_of(r);
        classes.insert((ranked.at.get(&r).copied().unwrap_or(0), class));
    }

    let mut at: HashMap<RoomId, i32> = HashMap::new();
    for &(_, class) in &classes {
        if at.contains_key(&class) {
            continue;
        }
        let members = ranked
            .members
            .get(&class)
            .map_or_else(|| vec![class], Clone::clone);
        let mut here: Vec<i32> = members.iter().map(|&m| now(m)).collect();
        here.sort_unstable();
        let seed = here.get(here.len() / 2).copied().unwrap_or(0);
        let floor = preds
            .get(&class)
            .into_iter()
            .flatten()
            .filter_map(|p| at.get(p))
            .map(|v| v + 1)
            .max();
        at.insert(class, floor.map_or(seed, |f| f.max(seed)));
    }

    rooms
        .iter()
        .map(|&r| {
            let v = if ranked.free.contains(&r) {
                now(r)
            } else {
                at.get(&ranked.class_of(r))
                    .copied()
                    .unwrap_or_else(|| now(r))
            };
            (r, v)
        })
        .collect()
}
