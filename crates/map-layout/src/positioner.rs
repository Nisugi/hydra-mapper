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
        let mut violations = validate_component(&room_order, &positions, map, dirs);

        // A violation on satisfiable data is the solver's, not the map's,
        // and both repair passes above are local: the hill climb moves one
        // room among its neighbours, and the re-weld cascades outward but
        // will not move the anchor, so neither can make the coordinated
        // shift some arrangements need. An arrangement exists in that
        // case, so take it.
        if !violations.is_empty()
            && let Some(placed) = crate::satisfiable::place_by_order(&room_order, map, dirs)
        {
            let fixed = validate_component(&room_order, &placed, map, dirs);
            // Only if it is actually better. The ordering pass satisfies
            // every direction it knows about, but `validate_component`
            // reads exits it does not constrain -- a one-way edge whose
            // reverse disagrees -- so this is checked rather than assumed.
            // Two rooms on one cell count against it as a violation
            // would: it satisfies every bearing and draws as nothing.
            if fixed.len() + stacked(&placed) < violations.len() + stacked(&positions) {
                positions = placed;
                compact_component(&mut positions);
                violations = validate_component(&room_order, &positions, map, dirs);
            }
        }

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

    // Compass-linked runs of outdoor rooms.
    let mut adjacent: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for room in map.rooms() {
        if !outdoor.contains(&room.id) {
            continue;
        }
        for exit in &room.exits {
            if !outdoor.contains(&exit.to) {
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

/// Collapse fully-empty rows and columns by rank-mapping the distinct x and
/// y values to 0..n-1. Relative order is preserved, so every edge keeps its
/// direction signs.
/// How many rooms share a cell with an earlier one.
fn stacked(positions: &HashMap<RoomId, Cell>) -> usize {
    let mut seen: HashSet<Cell> = HashSet::new();
    positions.values().filter(|&&c| !seen.insert(c)).count()
}

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
