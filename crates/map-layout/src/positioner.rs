//! Room positioning — ported from `reference/VellumFE/src/core/
//! layout_engine/positioner.rs` (`room-positioner.js` upstream of that).
//!
//! Builds connected components over directional edges with BFS, resolving
//! collisions by grid rips, then hill-climbs each component to shorten
//! stretched edges and compacts empty rows/columns (spec §4-5).

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use cena_map::{Map, RoomId};

use crate::direction::{Dir, DirectionMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub from: RoomId,
    pub to: RoomId,
    pub direction: Dir,
    pub actual: Cell,
}

/// How a group was placed on the shared sheet by the cluster packer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PackMethod {
    Image,
    Connector,
    Seed,
    Strip,
    InteriorShelf,
    /// Interior building seated on the outdoor sheet by the try-inline pass.
    InteriorInline,
}

impl PackMethod {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            PackMethod::Image => "image",
            PackMethod::Connector => "connector",
            PackMethod::Seed => "seed",
            PackMethod::Strip => "strip",
            PackMethod::InteriorShelf => "interior-shelf",
            PackMethod::InteriorInline => "interior-inline",
        }
    }
}

/// One connected component: rooms in placement order, their internal grid
/// positions, and (after packing) the offset onto the shared sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub index: usize,
    /// Room ids in BFS placement order (the reference's `componentRooms`).
    pub room_ids: Vec<RoomId>,
    /// Internal coordinates, before `base_offset` is applied.
    pub positions: HashMap<RoomId, Cell>,
    /// Compass edges whose placed geometry contradicts their stated
    /// direction -- genuine data conflicts, kept visible, never fixed
    /// silently.
    pub violations: Vec<Violation>,
    pub base_offset: Option<Cell>,
    pub packing: Option<PackMethod>,
    /// Building name for interior groups (majority `[Prefix, ...]` title).
    pub name: Option<String>,
}

impl Group {
    /// Internal position plus the group's sheet offset. Only valid after the
    /// packer has placed the group.
    ///
    /// # Panics
    ///
    /// If `room_id` was never placed in this group, or the group has not
    /// been packed yet.
    #[must_use]
    pub fn final_cell(&self, room_id: RoomId) -> Cell {
        let internal = self.positions[&room_id];
        let off = self
            .base_offset
            .unwrap_or_else(|| unreachable!("final_cell requires a packed group"));
        Cell {
            x: internal.x + off.x,
            y: internal.y + off.y,
        }
    }

    #[must_use]
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds {
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_y: i32::MAX,
            max_y: i32::MIN,
        };
        for p in self.positions.values() {
            b.min_x = b.min_x.min(p.x);
            b.max_x = b.max_x.max(p.x);
            b.min_y = b.min_y.min(p.y);
            b.max_y = b.max_y.max(p.y);
        }
        b
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}

impl Bounds {
    #[must_use]
    pub const fn width(&self) -> i32 {
        self.max_x - self.min_x + 1
    }

    #[must_use]
    pub const fn height(&self) -> i32 {
        self.max_y - self.min_y + 1
    }
}

/// Port of `calculateRoomPositionsWithGroups`: repeat BFS component builds
/// until every room is placed.
#[must_use]
pub fn position_rooms(map: &Map, dirs: &DirectionMap) -> Vec<Group> {
    let rooms = map.rooms();
    let mut groups: Vec<Group> = Vec::new();
    let mut unpositioned: HashSet<RoomId> = rooms.iter().map(|r| r.id).collect();

    // Which outdoor rooms are the world outside, rather than a courtyard
    // a building encloses. Computed once; see `open_air`.
    let air = open_air(map);

    // Directional-edge counts are a pure function of the selection, so they
    // are computed once instead of per component start.
    let connection_counts: Vec<usize> = rooms
        .iter()
        .map(|room| {
            room.exits
                .iter()
                .filter(|exit| dirs.get(room.id, exit.to).is_some())
                .count()
        })
        .collect();

    while !unpositioned.is_empty() {
        // Start room: the unplaced room with the most directional edges into
        // the selection. Strict `>` keeps the first encountered on ties;
        // iterating `rooms` (ascending id) matches the reference's
        // insertion order.
        let mut next_start: Option<RoomId> = None;
        let mut best_connections = 0usize;
        for (room, &valid) in rooms.iter().zip(&connection_counts) {
            if !unpositioned.contains(&room.id) {
                continue;
            }
            if valid > best_connections {
                best_connections = valid;
                next_start = Some(room.id);
            }
        }
        let start_id = next_start.unwrap_or_else(|| {
            // No connected rooms left; take the first remaining in room
            // order. `unpositioned` is non-empty (the loop guard), so this
            // always finds one.
            rooms
                .iter()
                .map(|r| r.id)
                .find(|id| unpositioned.contains(id))
                .unwrap_or_else(|| unreachable!("unpositioned is non-empty"))
        });

        let mut positions: HashMap<RoomId, Cell> = HashMap::new();
        let mut occupied: HashSet<Cell> = HashSet::new();
        let mut room_order: Vec<RoomId> = Vec::new();

        let mut queue: VecDeque<RoomId> = VecDeque::new();
        // Edges that join the component but cannot place a room, drained
        // after the directional BFS runs dry.
        let mut pending_connectors: Vec<(RoomId, RoomId)> = Vec::new();
        queue.push_back(start_id);
        positions.insert(start_id, Cell { x: 0, y: 0 });
        occupied.insert(Cell { x: 0, y: 0 });
        room_order.push(start_id);
        unpositioned.remove(&start_id);

        directional_bfs(
            map,
            dirs,
            &air,
            &mut queue,
            &mut pending_connectors,
            &mut positions,
            &mut occupied,
            &mut room_order,
            &mut unpositioned,
        );

        drain_connectors(
            map,
            dirs,
            &air,
            &mut pending_connectors,
            &mut positions,
            &mut occupied,
            &mut room_order,
            &mut unpositioned,
        );

        optimize_component(&room_order, &mut positions, map, dirs);
        let violations = validate_component(&room_order, &positions, map, dirs);

        let violations = repair_component(&room_order, &mut positions, violations, map, dirs);

        groups.push(Group {
            index: groups.len(),
            room_ids: room_order,
            positions,
            violations,
            base_offset: None,
            packing: None,
            name: None,
        });
    }

    groups
}

/// The directional BFS: place every room a stated bearing can reach.
///
/// Bearingless doorways are collected in `pending` rather than followed,
/// unless they are a building's front door (see [`is_building_entrance`]),
/// in which case they are dropped entirely -- the room behind them starts
/// a separate group the interior shelf will place.
#[allow(
    clippy::too_many_arguments,
    reason = "one BFS's working state,     split out only to keep `position_rooms` readable; bundling it into a     struct would hide that these are all one loop's locals"
)]
fn directional_bfs(
    map: &Map,
    dirs: &DirectionMap,
    air: &HashSet<RoomId>,
    queue: &mut VecDeque<RoomId>,
    pending: &mut Vec<(RoomId, RoomId)>,
    positions: &mut HashMap<RoomId, Cell>,
    occupied: &mut HashSet<Cell>,
    room_order: &mut Vec<RoomId>,
    unpositioned: &mut HashSet<RoomId>,
) {
    // BFS. The queue holds ids only: grid rips move already-placed
    // rooms, so the parent position is re-read at processing time.
    while let Some(room_id) = queue.pop_front() {
        let Some(room) = map.room(room_id) else {
            continue;
        };
        for exit in &room.exits {
            let target_id = exit.to;
            if map.room(target_id).is_none() || !unpositioned.contains(&target_id) {
                continue;
            }
            // A routine or a teleport is not a walk: it says nothing about
            // where its far end sits, so it neither places a room nor
            // joins one to this group. Without this the Rift was drawn
            // beside the Birthing Sands as one place.
            if !crate::regions::is_passage(exit) {
                continue;
            }
            let Some(direction) = dirs.get(room_id, target_id) else {
                // A `go door`, `go archway`, `go yett`. It states no
                // bearing, but it may still say these two rooms are
                // one place -- and grouping on direction alone
                // shattered 71% of buildings, the Bard Guild into 97
                // pieces and the Temple of Tonis into 11 across two
                // sheets, because a doorway is how a building is
                // joined to itself.
                //
                // The distinction is what the doorway crosses. Inside
                // to inside is a building's own structure: the
                // Temple's `go archway` from its Hall of Spring to its
                // Garden Bower. Outside to inside is a front door, and
                // the room behind it is a separate building that the
                // interior shelf exists to place. Following those too
                // welds every shop onto the street and makes one
                // 27,959-room group of the world.
                if !is_building_entrance(map, air, room_id, target_id) {
                    pending.push((room_id, target_id));
                }
                continue;
            };
            let (dx, dy) = direction.offset();

            let pos = (*positions)[&room_id];
            let mut target = Cell {
                x: pos.x + dx,
                y: pos.y + dy,
            };

            if occupied.contains(&target) {
                // Grid rip: shift a half-plane one cell so the occupant
                // slides off the target cell and the stated direction
                // stays true. The parent is never inside the
                // half-plane.
                rip_grid(positions, pos, (dx, dy));
                *occupied = positions.values().copied().collect();
                let fresh = (*positions)[&room_id];
                target = Cell {
                    x: fresh.x + dx,
                    y: fresh.y + dy,
                };
            }

            if !occupied.contains(&target) {
                positions.insert(target_id, target);
                occupied.insert(target);
                room_order.push(target_id);
                unpositioned.remove(&target_id);
                queue.push_back(target_id);
            }
        }
    }
}

/// Place the rooms joined only by a bearingless doorway.
///
/// Runs after the directional BFS has run dry, so every room that CAN be
/// placed by a stated bearing already is and these only fill the gaps.
/// Each round may open the way for the next -- a doorway into a wing
/// places that wing's first room, whose own compass exits then place the
/// rest -- so it repeats until a pass places nothing.
#[allow(
    clippy::too_many_arguments,
    reason = "one BFS's working state,     split out only to keep `position_rooms` readable; bundling it into a     struct would hide that these are all one loop's locals"
)]
fn drain_connectors(
    map: &Map,
    dirs: &DirectionMap,
    air: &HashSet<RoomId>,
    pending: &mut Vec<(RoomId, RoomId)>,
    positions: &mut HashMap<RoomId, Cell>,
    occupied: &mut HashSet<Cell>,
    room_order: &mut Vec<RoomId>,
    unpositioned: &mut HashSet<RoomId>,
) {
    // Drain the connectors. Each round places what it can and may
    // open the way for the next, so it repeats until a pass places
    // nothing. A room already placed by a bearing is left alone.
    loop {
        let mut placed_any = false;
        let mut still_pending: Vec<(RoomId, RoomId)> = Vec::new();
        for &(from_id, target_id) in &*pending {
            if !unpositioned.contains(&target_id) || !positions.contains_key(&from_id) {
                continue;
            }
            let Some(spot) = free_cell_near((*positions)[&from_id], occupied) else {
                still_pending.push((from_id, target_id));
                continue;
            };
            positions.insert(target_id, spot);
            occupied.insert(spot);
            room_order.push(target_id);
            unpositioned.remove(&target_id);
            placed_any = true;

            // Its own exits rejoin the directional BFS, so a doorway
            // into a wing places that whole wing by its bearings.
            let mut wave: VecDeque<RoomId> = VecDeque::from([target_id]);
            while let Some(id) = wave.pop_front() {
                let Some(room) = map.room(id) else { continue };
                for exit in &room.exits {
                    let next = exit.to;
                    if map.room(next).is_none() || !unpositioned.contains(&next) {
                        continue;
                    }
                    let Some(direction) = dirs.get(id, next) else {
                        if !is_building_entrance(map, air, id, next) {
                            still_pending.push((id, next));
                        }
                        continue;
                    };
                    let (dx, dy) = direction.offset();
                    let pos = (*positions)[&id];
                    let mut cell = Cell {
                        x: pos.x + dx,
                        y: pos.y + dy,
                    };
                    if occupied.contains(&cell) {
                        rip_grid(positions, pos, (dx, dy));
                        *occupied = positions.values().copied().collect();
                        let fresh = (*positions)[&id];
                        cell = Cell {
                            x: fresh.x + dx,
                            y: fresh.y + dy,
                        };
                    }
                    if !occupied.contains(&cell) {
                        positions.insert(next, cell);
                        occupied.insert(cell);
                        room_order.push(next);
                        unpositioned.remove(&next);
                        wave.push_back(next);
                    }
                }
            }
        }
        *pending = still_pending;
        if !placed_any {
            break;
        }
    }
}

/// The outdoor rooms that are the open air: the big outdoor networks a
/// building's front door opens onto.
///
/// Computed once per layout. Outdoor rooms are linked to each other by
/// EVERY edge between them -- a `go gate` between two streets is still
/// the open air -- and the runs that come out are split by size. The
/// large ones are the world outside; the small ones are pockets, and a
/// pocket reached only through a building is that building's courtyard.
///
/// # Why reachability and not a room count alone
///
/// A doorway between an indoor room and an outdoor one is either a front
/// door or a garden gate, and the two are identical locally: the Temple
/// of Tonis's `go archway` from its Hall of Spring to its Garden Bower
/// looks exactly like a shop's `go out` to the street. Blocking both
/// splits the temple from its gardens; following both welds every shop
/// onto its street and makes one 27,959-room group of the world.
///
/// What separates them is whether the outdoor side is part of the world's
/// outdoor network or a pocket only the building reaches. That is a
/// property of the graph, so the graph is asked rather than a room count
/// guessed at.
///
/// Measured on `gs.map` the two are not close. The outdoor runs are one
/// of 11,740 rooms, then 308, 242, 211, 158 and down: the world outside
/// is three orders of magnitude bigger than the next thing, and every
/// courtyard is far below that. A threshold anywhere from 5 to 100 picks
/// out the same handful of networks, which is what makes this safe where
/// a bare room count was not -- Vellum's notes record boutique streets of
/// 17 rooms that really are streets, and those sit inside the 11,740
/// because a street is joined to its town.
fn open_air(map: &Map) -> HashSet<RoomId> {
    use crate::classifier::{Sense, room_sense};

    let outdoor: HashSet<RoomId> = map
        .rooms()
        .iter()
        .filter(|room| room_sense(room) == Sense::Outdoor)
        .map(|room| room.id)
        .collect();

    // Walk-linked runs of outdoor rooms. A teleport between two outdoor
    // places does not make them one run.
    let mut adjacent: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for room in map.rooms() {
        if !outdoor.contains(&room.id) {
            continue;
        }
        for exit in &room.exits {
            if !outdoor.contains(&exit.to) || !crate::regions::is_passage(exit) {
                continue;
            }
            adjacent.entry(room.id).or_default().push(exit.to);
            adjacent.entry(exit.to).or_default().push(room.id);
        }
    }

    let mut runs: Vec<Vec<RoomId>> = Vec::new();
    let mut seen: HashSet<RoomId> = HashSet::new();
    for &start in &outdoor {
        if !seen.insert(start) {
            continue;
        }
        let mut run = vec![start];
        let mut queue = VecDeque::from([start]);
        while let Some(id) = queue.pop_front() {
            for &next in adjacent.get(&id).into_iter().flatten() {
                if seen.insert(next) {
                    run.push(next);
                    queue.push_back(next);
                }
            }
        }
        runs.push(run);
    }

    // The line is drawn relative to the biggest run rather than at a fixed
    // count, so it does not assume a map the size of `gs.map`. A courtyard
    // is small *compared to the world it sits in*; in a ten-room test
    // fixture a four-room square IS the world, and a fixed threshold would
    // call it a courtyard and weld the bank onto it.
    let biggest = runs.iter().map(Vec::len).max().unwrap_or(0);
    let floor = biggest / OPEN_AIR_RATIO;

    let mut air: HashSet<RoomId> = HashSet::new();
    for run in runs {
        if run.len() >= floor.max(1) {
            air.extend(run);
        }
    }
    air
}

/// How much smaller than the largest outdoor run a run may be and still
/// count as the open air.
///
/// The gap this has to straddle is enormous, so the exact figure hardly
/// matters. Measured on `gs.map`, the outdoor runs are 11,740 rooms, then
/// 308, 242, 211, 158, and a long tail of yards and gardens: the world
/// outside is nearly forty times the next thing down. Anything from 20 to
/// 500 picks out the same networks.
///
/// Twenty is chosen so a map with several real towns, none dominant,
/// still reads all of them as open air.
const OPEN_AIR_RATIO: usize = 20;

/// Whether a bearingless doorway is a building's front door rather than
/// its own internal structure.
///
/// A `go door` states no bearing, so it cannot place a room -- but it
/// still says whether two rooms are one place, and grouping on direction
/// alone shattered 71% of buildings, the Bard Guild into 97 pieces.
///
/// A doorway is a front door when it joins a room to the open air (see
/// [`open_air`]): the interior behind it is a building the shelf places,
/// and following it welds every shop onto its street. Every other
/// bearingless doorway is a building's own structure -- indoor to indoor,
/// or indoor to a courtyard nothing else reaches.
fn is_building_entrance(map: &Map, air: &HashSet<RoomId>, from: RoomId, to: RoomId) -> bool {
    use crate::classifier::{Sense, room_sense};
    let (Some(a), Some(b)) = (map.room(from), map.room(to)) else {
        return false;
    };
    let (sa, sb) = (room_sense(a), room_sense(b));
    // One end indoors, the other in the open air.
    (sa == Sense::Indoor && air.contains(&to)) || (sb == Sense::Indoor && air.contains(&from))
}

/// A free cell beside `from`, for a room joined by a doorway that states
/// no bearing. Compass neighbours first, then diagonals, then outward --
/// the room has to go somewhere, and beside the room you walk in from is
/// the honest guess. The hill climb refines it afterwards.
fn free_cell_near(from: Cell, occupied: &HashSet<Cell>) -> Option<Cell> {
    const NEAR: [(i32, i32); 8] = [
        (0, -1),
        (0, 1),
        (1, 0),
        (-1, 0),
        (1, -1),
        (1, 1),
        (-1, -1),
        (-1, 1),
    ];
    for (dx, dy) in NEAR {
        let cell = Cell {
            x: from.x + dx,
            y: from.y + dy,
        };
        if !occupied.contains(&cell) {
            return Some(cell);
        }
    }
    for radius in 2i32..=6 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs() != radius && dy.abs() != radius {
                    continue;
                }
                let cell = Cell {
                    x: from.x + dx,
                    y: from.y + dy,
                };
                if !occupied.contains(&cell) {
                    return Some(cell);
                }
            }
        }
    }
    None
}

fn rip_grid(positions: &mut HashMap<RoomId, Cell>, parent: Cell, (dx, dy): (i32, i32)) {
    let target_x = parent.x + dx;
    let target_y = parent.y + dy;
    if dx > 0 {
        for p in positions.values_mut() {
            if p.x >= target_x {
                p.x += 1;
            }
        }
    } else if dx < 0 {
        for p in positions.values_mut() {
            if p.x <= target_x {
                p.x -= 1;
            }
        }
    } else if dy > 0 {
        for p in positions.values_mut() {
            if p.y >= target_y {
                p.y += 1;
            }
        }
    } else if dy < 0 {
        for p in positions.values_mut() {
            if p.y <= target_y {
                p.y -= 1;
            }
        }
    }
}

/// An expected sign of `(other - this)` for one compass edge -- the
/// direction a room's neighbor must stay in, regardless of how far the
/// hill climb stretches the edge. Module scope: built once per component in
/// [`optimize_component`] and read throughout its hill climb and re-weld
/// passes.
#[derive(Clone, Copy)]
struct EdgeSign {
    other: RoomId,
    sx: i32,
    sy: i32,
}

/// Hill climb (<=12 passes) then re-weld (<=8 rounds), each room tried
/// against its directional neighbors; a move is accepted only when every
/// compass edge keeps correct signs and total edge length strictly
/// decreases, so both loops terminate.
fn optimize_component(
    room_order: &[RoomId],
    positions: &mut HashMap<RoomId, Cell>,
    map: &Map,
    dirs: &DirectionMap,
) {
    if room_order.len() < 3 {
        compact_component(positions);
        return;
    }

    let adjacency = build_compass_adjacency(room_order, positions, map, dirs);
    hill_climb(room_order, positions, &adjacency);
    if move_lines(room_order, positions, &adjacency) {
        hill_climb(room_order, positions, &adjacency);
    }
    reweld_violations(room_order, positions, &adjacency);
    compact_component(positions);
}

/// room id -> expected sign of `(other - this)` per compass edge, for every
/// room in `room_order` whose neighbor is already placed.
fn build_compass_adjacency(
    room_order: &[RoomId],
    positions: &HashMap<RoomId, Cell>,
    map: &Map,
    dirs: &DirectionMap,
) -> HashMap<RoomId, Vec<EdgeSign>> {
    let mut adjacency: HashMap<RoomId, Vec<EdgeSign>> = HashMap::new();
    for &room_id in room_order {
        let Some(room) = map.room(room_id) else {
            continue;
        };
        for exit in &room.exits {
            let target_id = exit.to;
            if !positions.contains_key(&target_id) {
                continue;
            }
            let Some(direction) = dirs.get(room_id, target_id) else {
                continue;
            };
            if !direction.is_compass() {
                continue;
            }
            let (dx, dy) = direction.offset();
            adjacency.entry(room_id).or_default().push(EdgeSign {
                other: target_id,
                sx: dx.signum(),
                sy: dy.signum(),
            });
            adjacency.entry(target_id).or_default().push(EdgeSign {
                other: room_id,
                sx: -dx.signum(),
                sy: -dy.signum(),
            });
        }
    }
    adjacency
}

/// Candidate cells worth trying for one room: the ideal cell beside each
/// neighbor +/- 1 ring, plus the current spot +/- 1 ring (so rooms drift
/// stepwise toward distant neighbors across passes). Insertion order
/// preserved, duplicates keep their first slot, like a JS Set.
fn hill_climb_candidates(
    current: Cell,
    edges: &[EdgeSign],
    positions: &HashMap<RoomId, Cell>,
) -> Vec<Cell> {
    let mut candidates: Vec<Cell> = Vec::new();
    let mut seen: HashSet<Cell> = HashSet::new();
    let mut add = |c: Cell| {
        if seen.insert(c) {
            candidates.push(c);
        }
    };
    for e in edges {
        let other = positions[&e.other];
        let ix = other.x - e.sx;
        let iy = other.y - e.sy;
        for dx in -1..=1 {
            for dy in -1..=1 {
                add(Cell {
                    x: ix + dx,
                    y: iy + dy,
                });
            }
        }
    }
    for dx in -1..=1 {
        for dy in -1..=1 {
            add(Cell {
                x: current.x + dx,
                y: current.y + dy,
            });
        }
    }
    candidates
}

/// Total cost of placing a room's edges at `at`: 1000 per sign-violated
/// edge (repairing a direction is always worth stretching for -- without
/// this, a BFS grid-rip can strand a room on the wrong side of a neighbor
/// and a length-only cost keeps it there) plus the Chebyshev length of
/// every edge.
fn edge_cost(at: Cell, edges: &[EdgeSign], positions: &HashMap<RoomId, Cell>) -> i64 {
    let mut cost = 0i64;
    for e in edges {
        let other = positions[&e.other];
        let dx = other.x - at.x;
        let dy = other.y - at.y;
        if dx.signum() != e.sx || dy.signum() != e.sy {
            cost += 1000;
        }
        cost += i64::from(dx.abs().max(dy.abs()));
    }
    cost
}

/// Whether every edge keeps its sign at `at` -- a candidate that violates
/// even one is never accepted, however short it is (a sign-violated edge
/// cannot be traded off against length; see [`edge_cost`]).
fn all_signs_hold(at: Cell, edges: &[EdgeSign], positions: &HashMap<RoomId, Cell>) -> bool {
    edges.iter().all(|e| {
        let other = positions[&e.other];
        let dx = other.x - at.x;
        let dy = other.y - at.y;
        dx.signum() == e.sx && dy.signum() == e.sy
    })
}

fn hill_climb(
    room_order: &[RoomId],
    positions: &mut HashMap<RoomId, Cell>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
) {
    let mut occupied: HashSet<Cell> = positions.values().copied().collect();

    let mut improved = true;
    let mut passes = 0;
    while improved && passes < 12 {
        improved = false;
        passes += 1;

        for &room_id in room_order {
            let Some(edges) = adjacency.get(&room_id) else {
                continue;
            };
            if edges.is_empty() {
                continue;
            }
            let current = positions[&room_id];
            let current_cost = edge_cost(current, edges, positions);

            let candidates = hill_climb_candidates(current, edges, positions);
            let mut best: Option<Cell> = None;
            let mut best_cost = current_cost;
            for &cand in &candidates {
                if occupied.contains(&cand) || !all_signs_hold(cand, edges, positions) {
                    continue;
                }
                let cost = edge_cost(cand, edges, positions);
                if cost < best_cost {
                    best_cost = cost;
                    best = Some(cand);
                }
            }

            if let Some(best) = best {
                occupied.remove(&current);
                occupied.insert(best);
                positions.insert(room_id, best);
                improved = true;
            }
        }
    }
}

/// How far, in cells, a whole column or row is tried away from where it is.
const LINE_REACH: i32 = 3;

/// Move whole columns and rows, where the hill climb can only move rooms.
///
/// **A column is stuck where one room cannot move.** Rooms joined north
/// and south must share an x, so neither can step sideways alone, and the
/// climb never tries both at once. The Kraken's Fall atoll and a wing of
/// the Landing came out with the column that belongs beside the middle
/// placed outside the one that belongs on the rim: every bearing true,
/// every diagonal crossing, and no single-room move out of it.
///
/// So here each alignment class -- the rooms bearings tie to one x (or one
/// y) -- moves as one: swapped with a class on a nearby line, or shifted
/// to a nearby free line. A move is taken only when nothing collides, no
/// bearing that held breaks, and the edges it touches get strictly
/// shorter in total, so this ends. Returns whether anything moved.
fn move_lines(
    room_order: &[RoomId],
    positions: &mut HashMap<RoomId, Cell>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
) -> bool {
    let mut moved = false;
    for _ in 0..6 {
        let mut any = false;
        for axis_x in [true, false] {
            any |= move_lines_on(room_order, positions, adjacency, axis_x);
        }
        moved |= any;
        if !any {
            break;
        }
    }
    moved
}

/// One axis of [`move_lines`]: columns when `axis_x`, rows otherwise.
fn move_lines_on(
    room_order: &[RoomId],
    positions: &mut HashMap<RoomId, Cell>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
    axis_x: bool,
) -> bool {
    let coord = move |c: Cell| if axis_x { c.x } else { c.y };
    let with = move |c: Cell, v: i32| {
        if axis_x {
            Cell { x: v, y: c.y }
        } else {
            Cell { x: c.x, y: v }
        }
    };
    let classes = line_classes(room_order, positions, adjacency, axis_x);
    let class_of: HashMap<RoomId, usize> = classes
        .iter()
        .enumerate()
        .flat_map(|(i, m)| m.iter().map(move |&r| (r, i)))
        .collect();
    let mut occupied: HashMap<Cell, RoomId> = positions.iter().map(|(&r, &c)| (c, r)).collect();

    let mut changed = false;
    for a in 0..classes.len() {
        let Some(va) = line_of(&classes[a], positions, coord) else {
            continue;
        };
        // Which classes sit on the lines within reach, read now: earlier
        // moves in this pass may have shifted them.
        let mut options: Vec<(i32, Option<usize>)> = Vec::new();
        for delta in -LINE_REACH..=LINE_REACH {
            if delta == 0 {
                continue;
            }
            let target = va + delta;
            let mut partners: Vec<usize> = classes[a]
                .iter()
                .filter_map(|r| occupied.get(&with(positions[r], target)))
                .filter_map(|o| class_of.get(o).copied())
                .collect();
            partners.sort_unstable();
            partners.dedup();
            match partners.as_slice() {
                [] => options.push((target, None)),
                [b] if line_of(&classes[*b], positions, coord) == Some(target) => {
                    options.push((target, Some(*b)));
                }
                _ => {}
            }
        }
        let mut best: Option<(i64, HashMap<RoomId, Cell>)> = None;
        for (target, partner) in options {
            let mut proposal: HashMap<RoomId, Cell> = HashMap::new();
            for r in &classes[a] {
                proposal.insert(*r, with(positions[r], target));
            }
            if let Some(b) = partner {
                for r in &classes[b] {
                    proposal.insert(*r, with(positions[r], va));
                }
            }
            let Some(gain) = line_move_gain(&proposal, positions, &occupied, adjacency) else {
                continue;
            };
            if gain > 0 && best.as_ref().is_none_or(|(g, _)| gain > *g) {
                best = Some((gain, proposal));
            }
        }
        if let Some((_, proposal)) = best {
            for r in proposal.keys() {
                occupied.remove(&positions[r]);
            }
            for (r, c) in &proposal {
                occupied.insert(*c, *r);
            }
            positions.extend(proposal);
            changed = true;
        }
    }
    changed
}

/// The alignment classes on one axis: rooms joined by an edge that takes
/// no step along it (north/south for x, east/west for y).
fn line_classes(
    room_order: &[RoomId],
    positions: &HashMap<RoomId, Cell>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
    axis_x: bool,
) -> Vec<Vec<RoomId>> {
    let mut seen: HashSet<RoomId> = HashSet::new();
    let mut classes: Vec<Vec<RoomId>> = Vec::new();
    for &start in room_order {
        if !positions.contains_key(&start) || !seen.insert(start) {
            continue;
        }
        let mut members = vec![start];
        let mut i = 0;
        while i < members.len() {
            let r = members[i];
            i += 1;
            for e in adjacency.get(&r).map_or(&[] as &[_], Vec::as_slice) {
                let flat = if axis_x { e.sx == 0 } else { e.sy == 0 };
                if flat && positions.contains_key(&e.other) && seen.insert(e.other) {
                    members.push(e.other);
                }
            }
        }
        classes.push(members);
    }
    classes
}

/// The line a class sits on now, if all of it sits on one.
fn line_of(
    members: &[RoomId],
    positions: &HashMap<RoomId, Cell>,
    coord: impl Fn(Cell) -> i32,
) -> Option<i32> {
    let v = coord(positions[&members[0]]);
    members
        .iter()
        .all(|r| coord(positions[r]) == v)
        .then_some(v)
}

/// How much shorter the edges touching `proposal`'s rooms get if it is
/// applied, or `None` if it lands on a room it does not move or breaks a
/// bearing that held.
fn line_move_gain(
    proposal: &HashMap<RoomId, Cell>,
    positions: &HashMap<RoomId, Cell>,
    occupied: &HashMap<Cell, RoomId>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
) -> Option<i64> {
    let mut taken: HashSet<Cell> = HashSet::new();
    for cell in proposal.values() {
        if !taken.insert(*cell) {
            return None;
        }
        if occupied
            .get(cell)
            .is_some_and(|holder| !proposal.contains_key(holder))
        {
            return None;
        }
    }
    let at = |r: &RoomId| proposal.get(r).or_else(|| positions.get(r)).copied();
    let (mut before, mut after) = (0i64, 0i64);
    for (r, &p1) in proposal {
        let p0 = positions[r];
        for e in adjacency.get(r).map_or(&[] as &[_], Vec::as_slice) {
            let (Some(o0), Some(o1)) = (positions.get(&e.other).copied(), at(&e.other)) else {
                continue;
            };
            if !violated_at(p0, o0, e.sx, e.sy) && violated_at(p1, o1, e.sx, e.sy) {
                return None;
            }
            before += i64::from((o0.x - p0.x).abs().max((o0.y - p0.y).abs()));
            after += i64::from((o1.x - p1.x).abs().max((o1.y - p1.y).abs()));
        }
    }
    Some(before - after)
}

fn violated_at(r: Cell, o: Cell, sx: i32, sy: i32) -> bool {
    (o.x - r.x).signum() != sx || (o.y - r.y).signum() != sy
}

/// Try to repair one violated edge by placing the wrong-side room at its
/// ideal cell beside its neighbor, then cascading: any neighbor whose edge
/// turns sign-wrong is pulled to ITS ideal cell in turn (the chain moves
/// non-rigidly). Commits only when nothing collides and the total
/// violation count strictly drops; returns whether it committed.
fn attempt_reweld(
    anchor_id: RoomId,
    edge: EdgeSign,
    positions: &mut HashMap<RoomId, Cell>,
    occupied_by: &mut HashMap<Cell, RoomId>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
    room_order: &[RoomId],
) -> bool {
    let mut moved: HashMap<RoomId, Cell> = HashMap::new();
    let anchor_pos = positions[&anchor_id];
    moved.insert(
        edge.other,
        Cell {
            x: anchor_pos.x + edge.sx,
            y: anchor_pos.y + edge.sy,
        },
    );
    let mut queue = VecDeque::from([edge.other]);
    while let Some(r) = queue.pop_front() {
        let r_pos = moved[&r];
        for e in adjacency.get(&r).map_or(&[] as &[_], Vec::as_slice) {
            let o_pos = moved.get(&e.other).copied().unwrap_or(positions[&e.other]);
            if !violated_at(r_pos, o_pos, e.sx, e.sy) {
                continue; // fine as-is (stretch allowed)
            }
            if moved.contains_key(&e.other) || e.other == anchor_id {
                return false; // moved-and-still-wrong, or would drag the anchor
            }
            let ideal = Cell {
                x: r_pos.x + e.sx,
                y: r_pos.y + e.sy,
            };
            moved.insert(e.other, ideal);
            queue.push_back(e.other);
        }
    }
    // Collisions: targets must be unique and free of every unmoved room.
    let mut targets: HashSet<Cell> = HashSet::new();
    for &p in moved.values() {
        if !targets.insert(p) {
            return false;
        }
        if let Some(&holder) = occupied_by.get(&p)
            && !moved.contains_key(&holder)
        {
            return false;
        }
    }
    // Net effect on every edge touching a moved room; untouched edges are
    // unchanged, so strictly-fewer here is strictly fewer overall.
    let mut before = 0usize;
    let mut after = 0usize;
    for &room_id in room_order {
        for e in adjacency.get(&room_id).map_or(&[] as &[_], Vec::as_slice) {
            if !moved.contains_key(&room_id) && !moved.contains_key(&e.other) {
                continue;
            }
            let cur_r = positions[&room_id];
            let cur_o = positions[&e.other];
            if violated_at(cur_r, cur_o, e.sx, e.sy) {
                before += 1;
            }
            let new_r = moved.get(&room_id).copied().unwrap_or(cur_r);
            let new_o = moved.get(&e.other).copied().unwrap_or(cur_o);
            if violated_at(new_r, new_o, e.sx, e.sy) {
                after += 1;
            }
        }
    }
    if after >= before {
        return false;
    }
    // Commit in two phases so rooms can move into vacated cells.
    for &id in moved.keys() {
        occupied_by.remove(&positions[&id]);
    }
    for (&id, &p) in &moved {
        positions.insert(id, p);
        occupied_by.insert(p, id);
    }
    true
}

/// Grid rips can strand a welded RUN of rooms sideways: every single room's
/// legal cell is then blocked until its neighbor moves first, so
/// [`hill_climb`]'s single-room moves can never repair it. This re-welds
/// via [`attempt_reweld`], repeatedly, until nothing more repairs or the
/// round cap is hit.
fn reweld_violations(
    room_order: &[RoomId],
    positions: &mut HashMap<RoomId, Cell>,
    adjacency: &HashMap<RoomId, Vec<EdgeSign>>,
) {
    let mut occupied_by: HashMap<Cell, RoomId> =
        positions.iter().map(|(&id, &p)| (p, id)).collect();

    let mut repaired = true;
    let mut rounds = 0;
    while repaired && rounds < 8 {
        repaired = false;
        rounds += 1;
        let mut violated: Vec<(RoomId, EdgeSign)> = Vec::new();
        for &room_id in room_order {
            for e in adjacency.get(&room_id).map_or(&[] as &[_], Vec::as_slice) {
                if violated_at(positions[&room_id], positions[&e.other], e.sx, e.sy) {
                    violated.push((room_id, *e));
                }
            }
        }
        violated.sort_by_key(|(from, e)| (*from, e.other));
        for (anchor_id, edge) in violated {
            if attempt_reweld(
                anchor_id,
                edge,
                positions,
                &mut occupied_by,
                adjacency,
                room_order,
            ) {
                repaired = true;
                break; // positions changed: rescan
            }
        }
    }
}

/// How much longer, in total edge length, a group put right by
/// [`crate::satisfiable::place_near`] may be drawn. Measured on gs.map,
/// events aside, exits against their direction from 649: at 5%, 521; at
/// 10%, 505; at 15%, 499; at 20%, 488 -- and lines crossing another fewest
/// at 10% (1,878, against 1,885, 1,880 and 1,882), every other rule no
/// worse at any of them. The knee.
const NEAR_STRETCH_PERCENT: i64 = 10;

/// Put right what violations the solver left on satisfiable data, when an
/// arrangement that does is a clean win; the violations that remain.
fn repair_component(
    room_order: &[RoomId],
    positions: &mut HashMap<RoomId, Cell>,
    mut violations: Vec<Violation>,
    map: &Map,
    dirs: &DirectionMap,
) -> Vec<Violation> {
    // A violation on satisfiable data is the solver's, not the map's,
    // and both repair passes above are local: the hill climb moves one
    // room among its neighbours, and the re-weld cascades outward but
    // will not move the anchor, so neither can make the coordinated
    // shift some arrangements need. An arrangement exists in that
    // case -- but it is ranked, not drawn: it satisfies every bearing
    // and can stretch the component out of shape doing it.
    //
    // Tried first: the arrangement nearest the solver's own, which moves
    // only what the bearings force (`satisfiable::place_near`), so it
    // keeps the component's shape where `place_by_order` redraws it.
    if !violations.is_empty()
        && let Some(mut placed) = crate::satisfiable::place_near(room_order, positions, map, dirs)
    {
        optimize_component(room_order, &mut placed, map, dirs);
        let fixed = validate_component(room_order, &placed, map, dirs);
        // A little longer is allowed here, as it is not for the ordering
        // pass below: this moves only what the bearings force, so it
        // cannot redraw the group. On gs.map every satisfiable group it
        // tried it put right entirely, and 37 were refused on length
        // alone, 16 of them for under 2%.
        if fixed.len() < violations.len()
            && stacked(&placed) == 0
            && edge_length(room_order, &placed, map) * 100
                <= edge_length(room_order, positions, map) * (100 + NEAR_STRETCH_PERCENT)
        {
            *positions = placed;
            violations = fixed;
        }
    }
    if !violations.is_empty()
        && let Some(mut placed) = crate::satisfiable::place_by_order(room_order, map, dirs)
    {
        compact_component(&mut placed);
        optimize_component(room_order, &mut placed, map, dirs);
        let fixed = validate_component(room_order, &placed, map, dirs);
        // **Only a clean win is taken**: fewer violations, no rooms
        // stacked, and no longer in total edge length after its own
        // hill climb. The ordering pass satisfies every direction it
        // knows about, but `validate_component` reads exits it does
        // not constrain, so fewer violations is checked, not assumed.
        // And the length test is what keeps it honest: taken on
        // violations alone it swapped the Landing's wing columns to
        // fix one bearing (edges +30%) and tangled Mist Harbor's
        // streets (a 1,063-room group, edges nearly doubled). A
        // violation is drawn and can be corrected; a tangle cannot be
        // read.
        if fixed.len() < violations.len()
            && stacked(&placed) == 0
            && edge_length(room_order, &placed, map) <= edge_length(room_order, positions, map)
        {
            *positions = placed;
            violations = fixed;
        }
    }
    // A group whose bearings contradict: drawn with the fewest of them set
    // aside, every other honoured (`satisfiable::relax`), on the same terms.
    if !violations.is_empty()
        && let Some((relaxed, _aside)) = crate::satisfiable::relax(room_order, map, dirs)
        && let Some(mut placed) =
            crate::satisfiable::place_near(room_order, positions, map, &relaxed)
    {
        optimize_component(room_order, &mut placed, map, dirs);
        let fixed = validate_component(room_order, &placed, map, dirs);
        // And its directionless lines crossing no more: freed of the exits
        // set aside, a group can put its bearings right by folding its
        // doorways across each other.
        if fixed.len() < violations.len()
            && stacked(&placed) == 0
            && edge_length(room_order, &placed, map) * 100
                <= edge_length(room_order, positions, map) * (100 + NEAR_STRETCH_PERCENT)
            && doorways_crossing(room_order, &placed, map, dirs)
                <= doorways_crossing(room_order, positions, map, dirs)
        {
            *positions = placed;
            violations = fixed;
        }
    }
    violations
}

/// How many lines between rooms of the component joined only by exits with
/// no direction cross another of its lines, by the quality measure's own
/// test (`quality::segments_cross`).
fn doorways_crossing(
    room_order: &[RoomId],
    positions: &HashMap<RoomId, Cell>,
    map: &Map,
    dirs: &DirectionMap,
) -> usize {
    let point = |c: Cell| crate::routing::point(c);
    let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
    let mut lines: Vec<(RoomId, RoomId, bool)> = Vec::new();
    for room in room_order.iter().filter_map(|&id| map.room(id)) {
        for exit in &room.exits {
            let (a, b) = (room.id, exit.to);
            if a == b || !positions.contains_key(&b) {
                continue;
            }
            let key = if a < b { (a, b) } else { (b, a) };
            if seen.insert(key) {
                let doorway = dirs.get(a, b).is_none() && dirs.get(b, a).is_none();
                lines.push((key.0, key.1, doorway));
            }
        }
    }
    lines
        .iter()
        .filter(|(_, _, doorway)| *doorway)
        .filter(|&&(a, b, _)| {
            let (p1, p2) = (point(positions[&a]), point(positions[&b]));
            lines.iter().any(|&(c, d, _)| {
                ![c, d].contains(&a)
                    && ![c, d].contains(&b)
                    && crate::quality::segments_cross(
                        p1,
                        p2,
                        point(positions[&c]),
                        point(positions[&d]),
                    )
            })
        })
        .count()
}

/// Total Chebyshev length of every exit between rooms of the component:
/// how stretched it is drawn.
fn edge_length(room_order: &[RoomId], positions: &HashMap<RoomId, Cell>, map: &Map) -> i64 {
    room_order
        .iter()
        .filter_map(|&id| map.room(id))
        .flat_map(|room| {
            room.exits.iter().filter_map(move |e| {
                let (a, b) = (positions.get(&room.id)?, positions.get(&e.to)?);
                Some(i64::from((a.x - b.x).abs().max((a.y - b.y).abs())))
            })
        })
        .sum()
}

/// How many rooms share a cell with an earlier one.
fn stacked(positions: &HashMap<RoomId, Cell>) -> usize {
    let mut seen: HashSet<Cell> = HashSet::new();
    positions.values().filter(|&&c| !seen.insert(c)).count()
}

/// Collapse fully-empty rows and columns by rank-mapping the distinct x and
/// y values to 0..n-1. Relative order is preserved, so every edge keeps its
/// direction signs.
fn compact_component(positions: &mut HashMap<RoomId, Cell>) {
    if positions.is_empty() {
        return;
    }
    let mut xs: Vec<i32> = positions.values().map(|p| p.x).collect();
    let mut ys: Vec<i32> = positions.values().map(|p| p.y).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    // A distinct-coordinate count in the billions never happens on a map
    // grid; the cast is exact in practice.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let to_rank = |i: usize| i as i32;
    let x_rank: HashMap<i32, i32> = xs
        .iter()
        .enumerate()
        .map(|(i, &x)| (x, to_rank(i)))
        .collect();
    let y_rank: HashMap<i32, i32> = ys
        .iter()
        .enumerate()
        .map(|(i, &y)| (y, to_rank(i)))
        .collect();
    for p in positions.values_mut() {
        p.x = x_rank[&p.x];
        p.y = y_rank[&p.y];
    }
}

/// Check every placed compass edge against its stated direction. Stretched
/// edges pass (signs match); wrong-way edges are reported.
fn validate_component(
    room_order: &[RoomId],
    positions: &HashMap<RoomId, Cell>,
    map: &Map,
    dirs: &DirectionMap,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    for &room_id in room_order {
        let Some(room) = map.room(room_id) else {
            continue;
        };
        let Some(&pos) = positions.get(&room_id) else {
            continue;
        };
        for exit in &room.exits {
            let target_id = exit.to;
            let Some(&target_pos) = positions.get(&target_id) else {
                continue;
            };
            let Some(direction) = dirs.get(room_id, target_id) else {
                continue;
            };
            if !direction.is_compass() {
                continue;
            }
            let (ex, ey) = direction.offset();
            let actual_x = target_pos.x - pos.x;
            let actual_y = target_pos.y - pos.y;
            if actual_x.signum() != ex.signum() || actual_y.signum() != ey.signum() {
                violations.push(Violation {
                    from: room_id,
                    to: target_id,
                    direction,
                    actual: Cell {
                        x: actual_x,
                        y: actual_y,
                    },
                });
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId};

    use super::{Cell, optimize_component};
    use crate::direction::DirectionMap;

    fn room(id: u32, exits: &[(u32, &str)]) -> Room {
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
            exits: exits
                .iter()
                .map(|&(to, cmd)| Exit {
                    to: RoomId(to),
                    kind: ExitKind::Cardinal,
                    crossing: Crossing::Command(cmd.to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    /// Two columns in each other's places: M belongs beside L and P beside
    /// Q, but M sits where P should and P where M should. Every bearing
    /// holds either way, and no one room can move -- each is held to its
    /// column's x by the room north or south of it, and the cell it would
    /// step to is taken. Swapping the two columns whole is the way out.
    #[test]
    fn two_columns_in_each_others_places_are_swapped() {
        // Columns, top room then bottom: L = 1,2; M = 3,4; P = 5,6; Q = 7,8.
        // Every link both ways, as the map records them.
        let column = |top: u32, bottom: u32, east: Option<(u32, u32)>, west: Option<(u32, u32)>| {
            let mut t = vec![(bottom, "south")];
            let mut b = vec![(top, "north")];
            if let Some((et, eb)) = east {
                t.push((et, "east"));
                b.push((eb, "east"));
            }
            if let Some((wt, wb)) = west {
                t.push((wt, "west"));
                b.push((wb, "west"));
            }
            vec![room(top, &t), room(bottom, &b)]
        };
        let mut rooms = Vec::new();
        rooms.extend(column(1, 2, Some((3, 4)), None));
        rooms.extend(column(3, 4, None, Some((1, 2))));
        rooms.extend(column(5, 6, Some((7, 8)), None));
        rooms.extend(column(7, 8, None, Some((5, 6))));
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let dirs = DirectionMap::build(&map);
        let at = |x: i32, y: i32| Cell { x, y };
        // L at x=0, P at 1, M at 2, Q at 3: M and P swapped.
        let mut positions: HashMap<RoomId, Cell> = [
            (1, at(0, 0)),
            (2, at(0, 1)),
            (5, at(1, 0)),
            (6, at(1, 1)),
            (3, at(2, 0)),
            (4, at(2, 1)),
            (7, at(3, 0)),
            (8, at(3, 1)),
        ]
        .into_iter()
        .map(|(id, c)| (RoomId(id), c))
        .collect();
        let order: Vec<RoomId> = (1..=8).map(RoomId).collect();
        optimize_component(&order, &mut positions, &map, &dirs);

        let x = |id: u32| positions[&RoomId(id)].x;
        assert_eq!(x(3) - x(1), 1, "M is not beside L: {positions:?}");
        assert_eq!(x(7) - x(5), 1, "P is not beside Q: {positions:?}");
        assert_eq!((x(1), x(3)), (x(2), x(4)), "a column came apart");
    }
}
