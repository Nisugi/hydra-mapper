//! The areas the map is laid out by, and the rooms each is laid out from:
//! what the mapper's window and gate use, and what Hydra lays out at launch
//! (`plan/53` §7). An area is the one baked into the map as `meta:area:`
//! by the mapper's `retag`.

use std::collections::{BTreeMap, HashSet};

use cena_map::{Map, RoomId};

/// Every area baked into `map`, by name, with its own rooms.
#[must_use]
pub fn baked(map: &Map) -> BTreeMap<String, Vec<RoomId>> {
    let mut by_area: BTreeMap<String, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        if let Some(area) = room.meta.iter().find_map(|m| m.strip_prefix("area:")) {
            by_area.entry(area.to_owned()).or_default().push(room.id);
        }
    }
    by_area
}

/// The rooms to lay an area out from: its own, plus the neighbours a
/// stranded room needs to stay attached to its building.
///
/// **An area filter should not strand a room from the building it opens
/// off.** Measured on `gs.map`: 361 rooms lay out as single-room groups
/// with no connection at all inside their own area, purely because the
/// boundary cut them from their doorway -- `[Haegan's Weaponry]` is in
/// "Cysaegir" while its street is in "the village of Cysaegir", and
/// `[Ebonstone Manor, Lockers]` sits in "CHE Central" with its door in
/// Wehnimer's Landing. 335 of those have every neighbour in one other
/// area, and 309 have exactly one neighbour.
///
/// Those neighbours are pulled in **for the layout only**. The room still
/// belongs to its own area: the lists, the counts and the export are
/// unchanged, because `location` is not being second-guessed here. A
/// mapdb location that disagrees with a doorway is a fact about the data,
/// and changing it is a correction a person makes deliberately -- this
/// just stops the solver being lied to about what connects to what.
///
/// Only rooms that would otherwise be **wholly cut off** pull anything in,
/// so an area that is already whole is laid out from exactly its own
/// rooms, as before.
///
/// **Only a place is laid out**, own or pulled: `placeable` is
/// [`regions::placeable_rooms`], so a removed room, an urchin hideout
/// and a room nothing reaches are left off whichever list named them. And
/// only a walk attaches: a teleport says nothing about where two rooms
/// sit, so it pulls nothing in.
///
/// [`regions::placeable_rooms`]: crate::regions::placeable_rooms
#[must_use]
pub fn layout_rooms(
    area_rooms: &[RoomId],
    map: &Map,
    placeable: &HashSet<RoomId>,
) -> Vec<cena_map::Room> {
    let is_passage = crate::regions::is_passage;
    let area_rooms: Vec<RoomId> = area_rooms
        .iter()
        .copied()
        .filter(|id| placeable.contains(id))
        .collect();
    let area_rooms = area_rooms.as_slice();
    let own: HashSet<RoomId> = area_rooms.iter().copied().collect();

    // Who points at a room, so a one-way door inward still counts as an
    // attachment -- a shop entered from the street but leaving by another
    // exit is still that street's shop.
    let mut inbound: BTreeMap<RoomId, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        for exit in &room.exits {
            if own.contains(&exit.to) && !own.contains(&room.id) && is_passage(exit) {
                inbound.entry(exit.to).or_default().push(room.id);
            }
        }
    }

    let mut pulled: HashSet<RoomId> = HashSet::new();
    for &id in area_rooms {
        let Some(room) = map.room(id) else {
            continue;
        };
        let neighbours: Vec<RoomId> = room
            .exits
            .iter()
            .filter(|e| is_passage(e))
            .map(|e| e.to)
            .chain(inbound.get(&id).into_iter().flatten().copied())
            .filter(|n| placeable.contains(n))
            .collect();
        // A room with a neighbour of its own is attached already; only one
        // with none is stranded by the boundary.
        if neighbours.is_empty() || neighbours.iter().any(|n| own.contains(n)) {
            continue;
        }
        pulled.extend(neighbours);
    }

    area_rooms
        .iter()
        .copied()
        .chain(pulled.into_iter().filter(|id| !own.contains(id)))
        .filter_map(|id| map.room(id).cloned())
        .collect()
}
