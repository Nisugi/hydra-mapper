//! The interior half of cluster packing: try-inline (an interior building
//! that seats cleanly beside its doorway joins the outdoor sheet) and the
//! interiors shelf itself, for whatever does not (spec §7). Split out of
//! `packer` under `plan/05` Rule 4.1 (move code down, do not raise the
//! cap) -- the outdoor connector-packing pipeline and this half are each
//! coherent on their own, and `packer` re-exports its geometry primitives
//! (`Edge`, `Segment`, `BBox`, `AnchorLine`, `chebyshev`, `uid_delta`,
//! `find_free_offset`, `find_best_connector_offset`) for this module to
//! share rather than duplicate.

use std::collections::{BTreeMap, HashMap, HashSet};

use cena_map::{Map, RoomId};

use crate::classifier::Entrance;
use crate::packer::{Edge, GROUP_PADDING, find_free_offset, uid_delta};
use crate::positioner::{Cell, Group, PackMethod};

/// One shelved building: member group -> cluster-local frame offset, and
/// the merged footprint's size. Module scope: [`pack_interior_shelf`]
/// builds one of these per cluster before laying the shelf's wrapped rows.
struct Item {
    local: Vec<(usize, Cell)>,
    width: i32,
    height: i32,
    /// Where this building's doorway is on the outdoor sheet, so the shelf
    /// can be laid out in the town's own order. `None` for a building with
    /// no outdoor entrance, which sorts last.
    door: Option<Cell>,
    /// The doorway's room, and the lowest group index, breaking ties so the
    /// order is stable between runs rather than following hash iteration.
    tie: (u32, usize),
    /// Set when this is the town frame: its `local` cells are absolute --
    /// the outdoor sheet's own cells times [`TOWN_SCALE`], with the
    /// buildings hung beside their streets -- and these are its bounds.
    /// Everything else shelves below it.
    town: Option<(Cell, Cell)>,
}

/// One shelvable unit per building cluster: its merged floor plan
/// normalised to a (0,0) top-left, its size, and where its doorway sits on
/// the outdoor sheet -- which is what [`pack_interior_shelf`] orders by.
fn shelf_items(
    groups: &mut [Group],
    interior: &[usize],
    clusters: &HashMap<usize, usize>,
    map: &Map,
    entrances: &HashMap<usize, Vec<Entrance>>,
    outdoor: &[usize],
) -> Vec<Item> {
    // Where each outdoor room sits, so a building can be shelved near the
    // others off the same street rather than at its group index.
    let mut outdoor_cell: HashMap<RoomId, Cell> = HashMap::new();
    for &idx in outdoor {
        if groups[idx].base_offset.is_some() {
            for &id in &groups[idx].room_ids {
                outdoor_cell.insert(id, groups[idx].final_cell(id));
            }
        }
    }

    // Cluster membership, canonical order (BTreeMap keys + sorted members).
    let mut members_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &idx in interior {
        members_of
            .entry(clusters.get(&idx).copied().unwrap_or(idx))
            .or_default()
            .push(idx);
    }
    for members in members_of.values_mut() {
        members.sort_unstable();
    }

    let (frames, claimed) = frames(groups, interior, clusters, &members_of, map, &outdoor_cell);
    let mut items: Vec<Item> = Vec::new();
    for frame in &frames {
        items.push(shelf_item(
            groups,
            &frame.members,
            &frame.streets,
            map,
            entrances,
            &outdoor_cell,
        ));
    }
    for (&cluster, members) in &members_of {
        if !claimed.contains(&cluster) {
            items.push(shelf_item(
                groups,
                members,
                &[],
                map,
                entrances,
                &outdoor_cell,
            ));
        }
    }
    items
}

/// **Frames.** A cluster with a street door and the street room it
/// opens onto belong together; so do that street room's other
/// buildings, and their other street rooms, and so on. Walking the
/// door edges outward from any cluster gives a connected component of
/// buildings and street rooms, and each component is merged as one
/// frame with every one of its street rooms echoed inside it.
///
/// Components, not islands: seeding a frame from a single street room
/// and claiming each building for the lowest-id one left a building
/// with five doors attached to one of them and stretched across the
/// sheet to the other four. In Wehnimer's Landing that was 134 of 421
/// door edges. Here the five street rooms are placed around the
/// building instead.
///
/// Whole clusters, never single groups: a back room is usually its own
/// group behind the shopfront, and seating the shopfront alone would
/// sever a building the clustering already keeps together. A cluster
/// with no street door at all shelves on its own as before.
///
/// Returns each frame's member groups and street rooms, and the set of
/// clusters that landed in one -- the rest shelve alone.
fn frames(
    groups: &[Group],
    interior: &[usize],
    clusters: &HashMap<usize, usize>,
    members_of: &BTreeMap<usize, Vec<usize>>,
    map: &Map,
    outdoor_cell: &HashMap<RoomId, Cell>,
) -> (Vec<Frame>, HashSet<usize>) {
    let (cluster_doors, door_clusters, street_adj) =
        doors_and_streets(groups, interior, clusters, members_of, map, outdoor_cell);
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut seen_streets: HashSet<RoomId> = HashSet::new();
    let mut frames: Vec<Frame> = Vec::new();
    for &cluster in members_of.keys() {
        if claimed.contains(&cluster) || !cluster_doors.contains_key(&cluster) {
            continue;
        }
        // Flood doors and street adjacency together from here.
        let mut frame_clusters: Vec<usize> = Vec::new();
        let mut frame_streets: Vec<RoomId> = Vec::new();
        let mut clusters_todo: Vec<usize> = vec![cluster];
        let mut streets_todo: Vec<RoomId> = Vec::new();
        claimed.insert(cluster);
        loop {
            if let Some(c) = clusters_todo.pop() {
                frame_clusters.push(c);
                for &street in cluster_doors.get(&c).map_or(&[] as &[_], Vec::as_slice) {
                    if seen_streets.insert(street) {
                        streets_todo.push(street);
                    }
                }
            } else if let Some(street) = streets_todo.pop() {
                frame_streets.push(street);
                for &other in door_clusters
                    .get(&street)
                    .map_or(&[] as &[_], Vec::as_slice)
                {
                    if claimed.insert(other) {
                        clusters_todo.push(other);
                    }
                }
                for &next in street_adj.get(&street).map_or(&[] as &[_], Vec::as_slice) {
                    if seen_streets.insert(next) {
                        streets_todo.push(next);
                    }
                }
            } else {
                break;
            }
        }
        frame_clusters.sort_unstable();
        frame_streets.sort_unstable();
        let mut members: Vec<usize> = frame_clusters
            .iter()
            .flat_map(|c| members_of[c].iter().copied())
            .collect();
        members.sort_unstable();
        frames.push(Frame {
            members,
            streets: frame_streets,
        });
    }
    // **One frame: the town.** Every frame shares the same skeleton -- the
    // outdoor sheet, scaled up -- so they are merged into one, and every
    // outdoor room is echoed, doors or not, so the skeleton is the whole
    // town and not just the streets the buildings happen to open onto.
    if !frames.is_empty() {
        let mut members: Vec<usize> = frames
            .iter()
            .flat_map(|f| f.members.iter().copied())
            .collect();
        members.sort_unstable();
        members.dedup();
        // Every outdoor room, not only the streets the doors reach: the
        // skeleton is the whole outdoor sheet, so the two sheets are one
        // picture and a road on one is a road on the other.
        let mut streets: Vec<RoomId> = outdoor_cell.keys().copied().collect();
        streets.sort_unstable();
        frames = vec![Frame { members, streets }];
    }
    (frames, claimed)
}

/// Every door between a cluster and a street room, from both sides,
/// and the street's own adjacency.
#[allow(clippy::type_complexity)]
fn doors_and_streets(
    groups: &[Group],
    interior: &[usize],
    clusters: &HashMap<usize, usize>,
    members_of: &BTreeMap<usize, Vec<usize>>,
    map: &Map,
    outdoor_cell: &HashMap<RoomId, Cell>,
) -> (
    BTreeMap<usize, Vec<RoomId>>,
    BTreeMap<RoomId, Vec<usize>>,
    HashMap<RoomId, Vec<RoomId>>,
) {
    // Doors, read from the map in **both** directions. `entrances` only
    // knows outdoor -> interior exits, and a shop whose one link to the
    // street is its own `out` is not in it -- 110 of the 120 door edges
    // still stretched across Wehnimer's Landing after the first frames
    // were built were exactly those, attached to nothing.
    let mut group_of_room: HashMap<RoomId, usize> = HashMap::new();
    for &idx in interior {
        for &id in &groups[idx].room_ids {
            group_of_room.insert(id, idx);
        }
    }
    let mut cluster_doors: BTreeMap<usize, Vec<RoomId>> = BTreeMap::new();
    let mut door_clusters: BTreeMap<RoomId, Vec<usize>> = BTreeMap::new();
    let mut note = |cluster: usize, street: RoomId| {
        cluster_doors.entry(cluster).or_default().push(street);
        door_clusters.entry(street).or_default().push(cluster);
    };
    for (&cluster, members) in members_of {
        for &m in members {
            for &id in &groups[m].room_ids {
                let Some(room) = map.room(id) else {
                    continue;
                };
                for exit in room.exits.iter().filter(|e| crate::regions::is_passage(e)) {
                    if outdoor_cell.contains_key(&exit.to) {
                        note(cluster, exit.to);
                    }
                }
            }
        }
    }
    for &street in outdoor_cell.keys() {
        let Some(room) = map.room(street) else {
            continue;
        };
        for exit in room.exits.iter().filter(|e| crate::regions::is_passage(e)) {
            if let Some(&g) = group_of_room.get(&exit.to) {
                note(clusters.get(&g).copied().unwrap_or(g), street);
            }
        }
    }
    for list in cluster_doors.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    for list in door_clusters.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    // The street itself joins frames too. Two street rooms adjacent
    // outdoors belong to one frame whether or not a building spans them,
    // and a street room with no door at all is carried along so the road
    // is continuous -- a fixture street of twelve rooms with a shop on
    // each came out as three frames on three rows before this, because
    // frames only reached along doors.
    let mut street_adj: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for &street in outdoor_cell.keys() {
        let Some(room) = map.room(street) else {
            continue;
        };
        for exit in room.exits.iter().filter(|e| crate::regions::is_passage(e)) {
            if outdoor_cell.contains_key(&exit.to) && exit.to != street {
                street_adj.entry(street).or_default().push(exit.to);
                street_adj.entry(exit.to).or_default().push(street);
            }
        }
    }

    (cluster_doors, door_clusters, street_adj)
}

/// One shelf item: the members merged into a frame normalised to a
/// (0,0) top-left, with every echo -- if this is a frame with street
/// rooms in it -- kept in that frame too.
fn shelf_item(
    groups: &[Group],
    members: &[usize],
    anchors: &[RoomId],
    map: &Map,
    entrances: &HashMap<usize, Vec<Entrance>>,
    outdoor_cell: &HashMap<RoomId, Cell>,
) -> Item {
    let mut local: HashMap<usize, Cell> = HashMap::new();
    if anchors.is_empty() && members.len() == 1 {
        local.insert(members[0], Cell::default());
    } else {
        let mut all: Vec<usize> = members.to_vec();
        all.extend((0..anchors.len()).map(anchor_index));
        // The street's own shape: each echo's outdoor cell. Every anchor
        // has one, because a frame only gathers street rooms that do.
        let skeleton: Vec<Cell> = anchors
            .iter()
            .filter_map(|a| outdoor_cell.get(a).copied())
            .collect();
        let skeleton = if skeleton.len() == anchors.len() {
            skeleton
        } else {
            Vec::new()
        };
        merge_cluster_members(groups, &all, map, &mut local, anchors, &skeleton);
    }
    // Normalize the frame to a (0,0) top-left, the echoes included.
    let mut min = Cell {
        x: i32::MAX,
        y: i32::MAX,
    };
    let mut max = Cell {
        x: i32::MIN,
        y: i32::MIN,
    };
    for (&idx, off) in &local {
        if anchor_at(idx).is_some() {
            min.x = min.x.min(off.x);
            min.y = min.y.min(off.y);
            max.x = max.x.max(off.x);
            max.y = max.y.max(off.y);
            continue;
        }
        let b = groups[idx].bounds();
        min.x = min.x.min(b.min_x + off.x);
        min.y = min.y.min(b.min_y + off.y);
        max.x = max.x.max(b.max_x + off.x);
        max.y = max.y.max(b.max_y + off.y);
    }
    // **The town frame is not normalised.** Its echoes sit at the outdoor
    // cells times the scale, and the buildings beside them, so an interior
    // group's offset is already a cell of the one shared frame the whole
    // area is drawn in. Anything without a street is normalised to (0,0)
    // and shelved below the town.
    let is_town = !anchors.is_empty();
    let shift = if is_town { Cell::default() } else { min };
    let mut ordered: Vec<(usize, Cell)> = members
        .iter()
        .map(|&idx| {
            let off = local[&idx];
            (
                idx,
                Cell {
                    x: off.x - shift.x,
                    y: off.y - shift.y,
                },
            )
        })
        .collect();
    ordered.sort_unstable_by_key(|&(idx, _)| idx);
    // Where to shelve it: a frame by its lowest street room, a lone
    // building by whichever door it has, if any.
    let door_room = anchors.iter().copied().min_by_key(|r| r.0).or_else(|| {
        members
            .iter()
            .filter_map(|m| entrances.get(m))
            .flatten()
            .map(|e| e.outdoor_room_id)
            .min_by_key(|r| r.0)
    });
    Item {
        local: ordered,
        town: is_town.then_some((min, max)),
        width: max.x - min.x + 1,
        height: max.y - min.y + 1,
        door: door_room.and_then(|r| outdoor_cell.get(&r).copied()),
        tie: (
            door_room.map_or(u32::MAX, |r| r.0),
            members.first().copied().unwrap_or(usize::MAX),
        ),
    }
}

/// Interiors sheet: wrapped shelf rows in an independent coordinate space.
/// A cluster (one walkable building) is merged into a single floor plan
/// first -- members are placed beside the rooms their passages connect to,
/// like the outdoor connector pass but cluster-local -- and then each
/// merged building is shelved as one unit (spec §7).
pub fn pack_interior_shelf(
    groups: &mut [Group],
    interior: &[usize],
    clusters: &HashMap<usize, usize>,
    map: &Map,
    entrances: &HashMap<usize, Vec<Entrance>>,
    outdoor: &[usize],
) {
    if interior.is_empty() {
        return;
    }

    let mut items = shelf_items(groups, interior, clusters, map, entrances, outdoor);

    // The town frame goes where it is: its cells are the outdoor sheet's
    // own, scaled, and moving it would break the one picture. Everything
    // else shelves in rows below it -- and below the streets themselves,
    // which are there whether or not a building hangs off them. (Reading
    // only the town frame put the rows at the origin, on top of the
    // streets, in an area where no building opens onto a street.)
    // `GROUP_PADDING` empty rows between: the last occupied row is
    // inclusive.
    let mut below = outdoor
        .iter()
        .flat_map(|&o| {
            let g = &groups[o];
            g.room_ids
                .iter()
                .map(move |&id| g.final_cell(id).y * TOWN_SCALE)
        })
        .max()
        .map_or(0, |y| y + 1 + GROUP_PADDING);
    items.retain(|item| {
        let Some((_, max)) = item.town else {
            return true;
        };
        for (idx, cell) in &item.local {
            groups[*idx].base_offset = Some(*cell);
            groups[*idx].packing = Some(PackMethod::InteriorShelf);
        }
        below = below.max(max.y + 1 + GROUP_PADDING);
        false
    });

    // **The shelf is laid out in the town's own order.** Buildings were
    // previously emitted by group index -- a number with no geographic
    // meaning -- which put two shops off the same street corner a median
    // of 42 cells apart on the real map's largest town, in different rows.
    // Sorting by where each doorway sits outdoors brings that to 8, for
    // about 10% more shelf area: the rows no longer pack in whatever order
    // the indices fell in, so buildings of unlike size sit together and
    // leave more ragged gaps. Finding a shop where the town says it should
    // be is worth the cells.
    //
    // A building with no outdoor doorway -- reachable only through other
    // interiors -- has nothing to sort by and goes last.
    items.sort_by_key(|i| {
        (
            i.door.map_or(i32::MAX, |c| c.y),
            i.door.map_or(i32::MAX, |c| c.x),
            i.tie,
        )
    });

    // Padded area, so the sheet comes out roughly square even when padding
    // dwarfs the mostly tiny buildings.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let row_width = {
        let total_area: i64 = items
            .iter()
            .map(|i| i64::from(i.width + GROUP_PADDING) * i64::from(i.height + GROUP_PADDING))
            .sum();
        ((total_area as f64).sqrt().ceil() as i32).max(20)
    };

    let mut cursor_x = 0;
    let mut cursor_y = below;
    let mut row_height = 0;
    for item in items {
        if cursor_x > 0 && cursor_x + item.width > row_width {
            cursor_y += row_height + GROUP_PADDING;
            cursor_x = 0;
            row_height = 0;
        }
        for (idx, local) in &item.local {
            groups[*idx].base_offset = Some(Cell {
                x: cursor_x + local.x,
                y: cursor_y + local.y,
            });
            groups[*idx].packing = Some(PackMethod::InteriorShelf);
        }
        cursor_x += item.width + GROUP_PADDING;
        row_height = row_height.max(item.height);
    }
}

/// Virtual member indices for a frame's street-room echoes. Not indices
/// into `groups` -- every access that would be is branched on
/// [`anchor_at`] instead -- but they let the merge treat "the street
/// room" as one more member, one cell in size, placed beside the door
/// room it opens onto like anything else.
///
/// Echo `i` is `ANCHOR_BASE - i`, so any number of them fit in the same
/// member list as the real groups without colliding with a real index.
const ANCHOR_BASE: usize = usize::MAX;

/// Which echo a member index stands for, if it is one.
const fn anchor_at(idx: usize) -> Option<usize> {
    // Real group indices are small; anything in the top half is an echo.
    if idx > usize::MAX / 2 {
        Some(ANCHOR_BASE - idx)
    } else {
        None
    }
}

const fn anchor_index(i: usize) -> usize {
    ANCHOR_BASE - i
}

/// Passages between one building's own members: member -> the edges its
/// rooms have into another member of the same set. Only edges within
/// `members` count; a passage out to a non-member group is not this
/// building's problem to place.
fn passages_within(
    groups: &[Group],
    members: &[usize],
    map: &Map,
    anchors: &[RoomId],
) -> HashMap<usize, Vec<Edge>> {
    let member_set: HashSet<usize> = members.iter().copied().collect();
    let mut group_of: HashMap<RoomId, usize> = HashMap::new();
    for &idx in members {
        if anchor_at(idx).is_some() {
            continue;
        }
        for &id in &groups[idx].room_ids {
            group_of.insert(id, idx);
        }
    }
    for (i, &a) in anchors.iter().enumerate() {
        group_of.insert(a, anchor_index(i));
    }
    let mut edges: HashMap<usize, Vec<Edge>> = HashMap::new();
    for &idx in members {
        // A member's rooms: one, for an echo.
        let rooms: Vec<RoomId> = match anchor_at(idx) {
            Some(i) => vec![anchors[i]],
            None => groups[idx].room_ids.clone(),
        };
        for room_id in rooms {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in room.exits.iter().filter(|e| crate::regions::is_passage(e)) {
                let target_id = exit.to;
                let Some(&other) = group_of.get(&target_id) else {
                    continue;
                };
                if other == idx || !member_set.contains(&other) {
                    continue;
                }
                edges.entry(idx).or_default().push(Edge {
                    other_group: other,
                    room_id,
                    other_room_id: target_id,
                    uid_delta: uid_delta(Some(room), map.room(target_id)),
                });
            }
        }
    }
    // Every passage is known from both ends, so a one-way door still
    // seats whichever side is placed second.
    let keys: Vec<usize> = edges.keys().copied().collect();
    for idx in keys {
        let list = edges[&idx].clone();
        for e in list {
            let back = Edge {
                other_group: idx,
                room_id: e.other_room_id,
                other_room_id: e.room_id,
                uid_delta: e.uid_delta,
            };
            let mine = edges.entry(e.other_group).or_default();
            if !mine
                .iter()
                .any(|m| m.other_group == idx && m.room_id == back.room_id)
            {
                mine.push(back);
            }
        }
    }
    edges
}

/// Merge a frame's members into one floor plan: each is placed beside
/// the room its passage connects to, shortest-uid-delta passage first.
///
/// `members` may include echoes -- street rooms, one cell each, indexed
/// by [`anchor_index`] -- and they are placed exactly like buildings: an
/// echo lands beside the door room it opens onto, a building's door room
/// lands on the echo it opens off, and whichever of the two is placed
/// second is the one that moves. A building with five street doors gets
/// its five echoes placed around *it*, which is what keeps every door
/// short; seeding from a single echo and claiming the building for it
/// left the other four doors stretched across the sheet.
fn merge_cluster_members(
    groups: &[Group],
    members: &[usize],
    map: &Map,
    local: &mut HashMap<usize, Cell>,
    anchors: &[RoomId],
    skeleton: &[Cell],
) {
    let edges = passages_within(groups, members, map, anchors);

    let mut occupied: HashSet<Cell> = HashSet::new();
    let place = |idx: usize, off: Cell, occupied: &mut HashSet<Cell>| {
        if anchor_at(idx).is_some() {
            occupied.insert(off);
            return;
        }
        for p in groups[idx].positions.values() {
            occupied.insert(Cell {
                x: p.x + off.x,
                y: p.y + off.y,
            });
        }
    };
    // Where a member's room sits in the frame; an echo is its own offset.
    let room_at = |idx: usize, room: RoomId, local: &HashMap<usize, Cell>| -> Cell {
        let off = local[&idx];
        if anchor_at(idx).is_some() {
            return off;
        }
        let p = groups[idx].positions[&room];
        Cell {
            x: p.x + off.x,
            y: p.y + off.y,
        }
    };
    // A member's own room's position in its own frame; an echo's is the
    // origin.
    let internal_of = |idx: usize, room: RoomId| -> Cell {
        if anchor_at(idx).is_some() {
            Cell::default()
        } else {
            groups[idx].positions[&room]
        }
    };
    let min_x_of = |idx: usize| {
        if anchor_at(idx).is_some() {
            0
        } else {
            groups[idx].bounds().min_x
        }
    };
    let free_for = |idx: usize, proposed: Cell, occupied: &HashSet<Cell>| {
        member_free_offset(groups, idx, proposed, occupied)
    };

    if skeleton.is_empty() {
        // Seed on the largest building, so echoes gather round it rather
        // than it being dragged to one of them; a frame with no building
        // at all seeds on its first echo.
        let seed = *members
            .iter()
            .filter(|&&idx| anchor_at(idx).is_none())
            .max_by_key(|&&idx| (groups[idx].room_ids.len(), std::cmp::Reverse(idx)))
            .or_else(|| members.first())
            .unwrap_or_else(|| unreachable!("members is non-empty"));
        local.insert(seed, Cell::default());
        place(seed, Cell::default(), &mut occupied);
    } else {
        let cells = lay_street(groups, anchors, skeleton, &edges);
        for (i, &c) in cells.iter().enumerate() {
            let idx = anchor_index(i);
            local.insert(idx, c);
            place(idx, c, &mut occupied);
        }
        reserve_roads(skeleton, &cells, &edges, &mut occupied);
    }

    loop {
        let best = next_to_place(members, local, &edges);
        let Some((idx, placed_edges)) = best else {
            // Stragglers (shouldn't happen: frames are connected).
            let Some(&idx) = members.iter().find(|&&m| !local.contains_key(&m)) else {
                return;
            };
            let off = straggler_offset(groups, idx, &occupied, &free_for);
            local.insert(idx, off);
            place(idx, off, &mut occupied);
            continue;
        };

        // The door with the nearest uids is the one this building is
        // entered by (the reference's rule). Between doors the uids cannot
        // tell apart -- no uids at all, most often -- the one whose landing
        // stretches the building's other placed doors least; then room
        // ids. Never list order: the lists come from a hash map, and a shop
        // entered from two street rooms hung beside either one, run to run.
        let nearest = placed_edges
            .iter()
            .map(|e| e.uid_delta)
            .min()
            .unwrap_or_else(|| unreachable!("placed_edges is non-empty"));
        let land = |edge: &Edge, occupied: &HashSet<Cell>| -> Cell {
            let neighbor_room = room_at(edge.other_group, edge.other_room_id, local);
            let internal = internal_of(idx, edge.room_id);
            // Land the passage endpoints as close together as the frame
            // allows.
            let proposed = Cell {
                x: neighbor_room.x - internal.x,
                y: neighbor_room.y - internal.y,
            };
            // Nothing free near: past everything placed so far, at the
            // passage's height -- stretched, but on cells of its own. (It
            // was one width to the right, unchecked, and landed on street
            // rooms in a dense grid.)
            free_for(idx, proposed, occupied).unwrap_or_else(|| {
                let max_x = occupied.iter().map(|c| c.x).max().unwrap_or(0);
                Cell {
                    x: max_x + 2 - min_x_of(idx),
                    y: proposed.y,
                }
            })
        };
        let stretch = |off: Cell| -> i32 {
            placed_edges
                .iter()
                .map(|e| {
                    let there = room_at(e.other_group, e.other_room_id, local);
                    let here = internal_of(idx, e.room_id);
                    (here.x + off.x - there.x)
                        .abs()
                        .max((here.y + off.y - there.y).abs())
                })
                .sum()
        };
        let mut tied: Vec<&Edge> = placed_edges
            .iter()
            .filter(|e| e.uid_delta == nearest)
            .collect();
        tied.sort_by_key(|e| (e.other_room_id, e.room_id));
        tied.dedup_by_key(|e| (e.other_room_id, e.room_id));
        let off = tied
            .iter()
            .map(|e| land(e, &occupied))
            .min_by_key(|&off| stretch(off))
            .unwrap_or_else(|| unreachable!("placed_edges is non-empty"));
        local.insert(idx, off);
        place(idx, off, &mut occupied);
    }
}

/// One connected set of buildings and the street rooms they open onto,
/// merged and shelved as a single unit.
struct Frame {
    /// Every group of every cluster in the frame.
    members: Vec<usize>,
    /// The street rooms echoed inside it.
    streets: Vec<RoomId>,
}

/// Where a member can go, nearest `proposed`: a building by the shared
/// search, an echo by a one-cell ring walk.
fn member_free_offset(
    groups: &[Group],
    idx: usize,
    proposed: Cell,
    occupied: &HashSet<Cell>,
) -> Option<Cell> {
    if anchor_at(idx).is_none() {
        return find_free_offset(&groups[idx], proposed, occupied);
    }
    if !occupied.contains(&proposed) {
        return Some(proposed);
    }
    for r in 1..=crate::packer::SEARCH_RADIUS {
        let mut found = None;
        crate::packer::for_ring(proposed, r, |c| {
            if found.is_none() && !occupied.contains(&c) {
                found = Some(c);
            }
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Where a member can go, nearest a proposed offset, or nowhere.
type FreeFor<'a> = dyn Fn(usize, Cell, &HashSet<Cell>) -> Option<Cell> + 'a;

/// The road between two adjacent echoes is drawn as a line; keep the
/// cells under it clear so no building sits on the street.
fn reserve_roads(
    skeleton: &[Cell],
    cells: &[Cell],
    edges: &HashMap<usize, Vec<Edge>>,
    occupied: &mut HashSet<Cell>,
) {
    for (i, &a) in skeleton.iter().enumerate() {
        for e in edges
            .get(&anchor_index(i))
            .map_or(&[] as &[_], Vec::as_slice)
        {
            let Some(j) = anchor_at(e.other_group) else {
                continue;
            };
            let b = skeleton[j];
            if (a.x - b.x).abs().max((a.y - b.y).abs()) != 1 {
                continue;
            }
            let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
            for step in 1..TOWN_SCALE {
                occupied.insert(Cell {
                    x: cells[i].x + dx * step,
                    y: cells[i].y + dy * step,
                });
            }
        }
    }
}

/// How many cells one outdoor cell becomes on the interiors sheet.
pub const TOWN_SCALE: i32 = 4;

/// The town laid down in its own shape, scaled so the buildings fit
/// between its rooms. See the comments inside for how.
fn lay_street(
    groups: &[Group],
    anchors: &[RoomId],
    skeleton: &[Cell],
    edges: &HashMap<usize, Vec<Edge>>,
) -> Vec<Cell> {
    // **The town goes down first, at scale.** Every echo is placed at its
    // outdoor cell times `TOWN_SCALE`: exact directions, exact adjacency,
    // the outdoor sheet's own shape, with `TOWN_SCALE - 1` free cells
    // between neighbours for the buildings to sit in. Scaling uniformly
    // rather than widening each gap by demand keeps the two sheets the
    // same picture -- a player can carry the town's shape from one to the
    // other -- and a building with doors on two streets lands between
    // them because the streets are where they were.
    let _ = (groups, anchors, edges);
    let cells: Vec<Cell> = skeleton
        .iter()
        .map(|c| Cell {
            x: c.x * TOWN_SCALE,
            y: c.y * TOWN_SCALE,
        })
        .collect();
    cells
}

/// The unplaced member with the most passages to placed ones, and those
/// passages; ties go to the lowest member index.
fn next_to_place(
    members: &[usize],
    local: &HashMap<usize, Cell>,
    edges: &HashMap<usize, Vec<Edge>>,
) -> Option<(usize, Vec<Edge>)> {
    let mut best: Option<(usize, Vec<Edge>)> = None;
    for &idx in members {
        if local.contains_key(&idx) {
            continue;
        }
        let placed_edges: Vec<Edge> = edges
            .get(&idx)
            .map(|l| {
                l.iter()
                    .filter(|e| local.contains_key(&e.other_group))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        if placed_edges.is_empty() {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(_, b)| placed_edges.len() > b.len())
        {
            best = Some((idx, placed_edges));
        }
    }
    best
}

/// Where a member no passage reaches goes: off the right-hand end.
fn straggler_offset(
    groups: &[Group],
    idx: usize,
    occupied: &HashSet<Cell>,
    free_for: &FreeFor<'_>,
) -> Cell {
    let max_x = occupied.iter().map(|c| c.x).max().unwrap_or(0);
    let min_x = if anchor_at(idx).is_some() {
        0
    } else {
        groups[idx].bounds().min_x
    };
    let proposed = Cell {
        x: max_x + 2 - min_x,
        y: 0,
    };
    free_for(idx, proposed, occupied).unwrap_or(proposed)
}

#[cfg(test)]
mod tests {
    use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId};

    use super::TOWN_SCALE;
    use crate::generate_layout;
    use crate::positioner::Cell;

    fn room(id: u32, title: &str, paths: &str, exits: Vec<Exit>) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![title.to_owned()],
            description: vec![],
            paths: vec![paths.to_owned()],
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

    fn exit(to: u32, command: &str) -> Exit {
        Exit {
            to: RoomId(to),
            kind: ExitKind::Cardinal,
            crossing: Crossing::Command(command.to_owned()),
            cost: Some(Cost::Fixed(1.0)),
        }
    }

    fn door(to: u32, what: &str) -> Exit {
        Exit {
            to: RoomId(to),
            kind: ExitKind::Go,
            crossing: Crossing::Command(format!("go {what}")),
            cost: Some(Cost::Fixed(1.0)),
        }
    }

    /// A street of six corners, each with two shops behind doors, built so
    /// that group index and geography disagree: the shops are declared
    /// east-to-west while the street runs west-to-east.
    ///
    /// More than `INLINE_MAX_BUILDINGS` buildings makes this a town, so the
    /// shelf is what places them -- which is the path under test. The shelf
    /// must follow the street: two shops off one corner belong nearer each
    /// other than shops from opposite ends of the road. Ordering by index
    /// put same-doorway rooms a median of 42 cells apart in Wehnimer's
    /// Landing, in different rows of the sheet.
    #[test]
    fn the_shelf_follows_the_street_not_the_indices() {
        const OUT: &str = "Obvious paths: east, west";
        const IN: &str = "Obvious exits: out";
        const CORNERS: u32 = 6;

        let mut rooms = Vec::new();
        // Shops first and in reverse, so low ids sit at the east end. Each
        // corner's shops are a different depth from the next corner's, so
        // the rows pack differently under a different order -- with every
        // building the same size, any order gives the same sheet and the
        // assertion below would hold vacuously.
        for i in (0..CORNERS).rev() {
            let street = 1000 + i;
            for s in 0..2u32 {
                let head = i * 20 + s * 10;
                let depth = i + 1;
                let mut back: Vec<Exit> = vec![door(street, "out")];
                if depth > 1 {
                    back.push(exit(head + 1, "north"));
                }
                rooms.push(room(head, "[Shop]", IN, back));
                for d in 1..depth {
                    let mut e = vec![exit(head + d - 1, "south")];
                    if d + 1 < depth {
                        e.push(exit(head + d + 1, "north"));
                    }
                    rooms.push(room(head + d, "[Shop Back]", IN, e));
                }
            }
        }
        for i in 0..CORNERS {
            let id = 1000 + i;
            let mut exits = vec![door(i * 20, "shop"), door(i * 20 + 10, "shop")];
            if i > 0 {
                exits.push(exit(id - 1, "west"));
            }
            if i + 1 < CORNERS {
                exits.push(exit(id + 1, "east"));
            }
            rooms.push(room(id, "[Street]", OUT, exits));
        }
        let map = Map::from_rooms(rooms).expect("no duplicate ids");

        let layout = generate_layout(&map);
        assert!(
            !layout.interiors.is_empty(),
            "fixture inlined instead of shelving; the shelf is untested"
        );
        let cell_of = |id: u32| {
            layout
                .groups
                .iter()
                .find(|g| g.room_ids.contains(&RoomId(id)))
                .map(|g| g.final_cell(RoomId(id)))
                .expect("room is placed")
        };

        // Each corner's shops sit pressed around that corner's echo on the
        // shelf -- within two cells of it, which is what the real map
        // measures at (median 1, max 2). And the echo of one corner is
        // nowhere near the shops of another, so the corners read as
        // separate islands rather than one run of rooms.
        let apart = |a: Cell, b: Cell| (a.x - b.x).abs().max((a.y - b.y).abs());
        // A street room's place in the shared frame: its outdoor cell at
        // the town scale, which is where the shelf hangs its shops.
        let street_at = |street: u32| {
            let c = cell_of(street);
            Cell {
                x: c.x * TOWN_SCALE,
                y: c.y * TOWN_SCALE,
            }
        };
        for corner in 0..CORNERS {
            let here = street_at(1000 + corner);
            for shop in [corner * 20, corner * 20 + 10] {
                let d = apart(cell_of(shop), here);
                assert!(d <= 2, "shop {shop} sits {d} cells from its street room");
            }
            for other in 0..CORNERS {
                if other == corner {
                    continue;
                }
                assert!(
                    apart(here, street_at(1000 + other)) > 2,
                    "two street rooms ({} and {}) sit on top of each other",
                    1000 + corner,
                    1000 + other
                );
            }
        }
    }

    /// A street of twelve rooms with a shop on each, and a guild hall
    /// fronting five of them. On the interiors sheet the street keeps its
    /// shape -- one row, rooms in order, nothing folded or scattered --
    /// the shops sit beside their street rooms, and the hall sits on the
    /// street between the streets it fronts.
    ///
    /// Before the street was laid down first, every echo was slid in
    /// beside whatever building it opened onto, and a town core came out
    /// as a solid mass of rooms with the street rooms buried in it.
    #[test]
    fn the_street_keeps_its_shape_and_the_buildings_hang_off_it() {
        const OUT: &str = "Obvious paths: east, west";
        const IN: &str = "Obvious exits: out";
        const STREETS: u32 = 12;
        const HALL_STREETS: [u32; 5] = [1000, 1003, 1006, 1009, 1011];

        let mut rooms = Vec::new();
        for (i, street) in (0u32..).zip(HALL_STREETS) {
            let id = 500 + i;
            let mut exits = vec![door(street, "out")];
            if i > 0 {
                exits.push(exit(id - 1, "west"));
            }
            if usize::try_from(i + 1).is_ok_and(|n| n < HALL_STREETS.len()) {
                exits.push(exit(id + 1, "east"));
            }
            rooms.push(room(id, "[Guild Hall]", IN, exits));
        }
        for i in 0..STREETS {
            let id = 1000 + i;
            let mut exits = vec![door(2000 + i, "shop")];
            if let Some(k) = (0u32..).zip(HALL_STREETS).find(|&(_, s)| s == id) {
                exits.push(door(500 + k.0, "hall"));
            }
            if i > 0 {
                exits.push(exit(id - 1, "west"));
            }
            if i + 1 < STREETS {
                exits.push(exit(id + 1, "east"));
            }
            rooms.push(room(id, "[Street]", OUT, exits));
            rooms.push(room(2000 + i, "[Shop]", IN, vec![door(id, "out")]));
        }
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let layout = generate_layout(&map);
        assert!(
            !layout.interiors.is_empty(),
            "fixture inlined; the shelf is untested"
        );

        let cell_of = |id: u32| {
            layout
                .groups
                .iter()
                .find(|g| g.room_ids.contains(&RoomId(id)))
                .map(|g| g.final_cell(RoomId(id)))
                .expect("room is placed")
        };
        // A street room's place in the shared frame: its outdoor cell at
        // the town scale.
        let echo_of = |street: u32| {
            let c = cell_of(street);
            Cell {
                x: c.x * TOWN_SCALE,
                y: c.y * TOWN_SCALE,
            }
        };
        let apart = |a: Cell, b: Cell| (a.x - b.x).abs().max((a.y - b.y).abs());

        // The street: one row, in order, every room echoed.
        let echoes: Vec<Cell> = (0..STREETS).map(|i| echo_of(1000 + i)).collect();
        assert!(
            echoes.windows(2).all(|w| w[0].y == w[1].y),
            "the street did not stay on one row: {echoes:?}"
        );
        assert!(
            echoes.windows(2).all(|w| w[0].x < w[1].x),
            "the street's rooms came out of order: {echoes:?}"
        );
        // And at one scale throughout: the same picture as outdoors, only
        // larger, not a street stretched where the buildings are and
        // squeezed where they are not.
        assert!(
            echoes.windows(2).all(|w| w[1].x - w[0].x == TOWN_SCALE),
            "the street is not uniformly scaled: {echoes:?}"
        );

        // Every shop beside its own street room.
        for i in 0..STREETS {
            let d = apart(cell_of(2000 + i), echo_of(1000 + i));
            assert!(
                d <= 2,
                "shop {} sits {d} cells from its street room",
                2000 + i
            );
        }

        // The hall on the street, between the streets it fronts: every
        // hall room within the span of those echoes along the street, and
        // no further from the street row than its own depth.
        let (first, last) = (echo_of(HALL_STREETS[0]), echo_of(HALL_STREETS[4]));
        for i in 0..5u32 {
            let c = cell_of(500 + i);
            assert!(
                (first.x..=last.x).contains(&c.x),
                "hall room {} at x={} is outside its streets' span {}..={}",
                500 + i,
                c.x,
                first.x,
                last.x
            );
            assert!(
                (c.y - first.y).abs() <= 2,
                "hall room {} sits {} rows off the street",
                500 + i,
                (c.y - first.y).abs()
            );
        }
    }

    const OUT: &str = "Obvious paths: east, west";
    const IN: &str = "Obvious exits: out";

    /// Every room on a cell of its own, on the one sheet.
    fn assert_one_room_per_cell(map: &Map) {
        let layout = generate_layout(map);
        let scene = crate::build_scene("Test", &layout, map);
        let mut cells: Vec<Cell> = scene.sheet.rooms.iter().map(|r| r.cell).collect();
        cells.sort_unstable_by_key(|c| (c.x, c.y));
        let before = cells.len();
        cells.dedup();
        assert_eq!(cells.len(), before, "two rooms share a cell");
    }

    /// A building whose door leads out of the selection has no street to
    /// hang beside, so it shelves in the rows below the town -- below the
    /// streets, not at the origin on top of them. ("A Hide and Leather
    /// Tent": its interior and a street room were both drawn at (0,0).)
    #[test]
    fn a_building_with_no_street_shelves_below_the_streets() {
        let mut rooms = Vec::new();
        for i in 0..5u32 {
            let mut exits = Vec::new();
            if i > 0 {
                exits.push(exit(i - 1, "west"));
            }
            if i < 4 {
                exits.push(exit(i + 1, "east"));
            }
            rooms.push(room(i, "[Street]", OUT, exits));
        }
        // Its only door is to room 999, which is not in the selection.
        rooms.push(room(
            100,
            "[Tent]",
            IN,
            vec![door(999, "flap"), exit(101, "north")],
        ));
        rooms.push(room(
            101,
            "[Tent]",
            IN,
            vec![exit(100, "south"), exit(102, "north")],
        ));
        rooms.push(room(102, "[Tent]", IN, vec![exit(101, "south")]));
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        assert_one_room_per_cell(&map);

        let layout = generate_layout(&map);
        let street_bottom = (0..5u32)
            .map(|i| {
                let g = layout
                    .groups
                    .iter()
                    .find(|g| g.room_ids.contains(&RoomId(i)));
                g.expect("placed").final_cell(RoomId(i)).y * TOWN_SCALE
            })
            .max()
            .expect("five streets");
        for id in 100..=102u32 {
            let g = layout
                .groups
                .iter()
                .find(|g| g.room_ids.contains(&RoomId(id)));
            let y = g.expect("placed").final_cell(RoomId(id)).y;
            assert!(
                y > street_bottom,
                "tent room {id} at y={y} is not below the street"
            );
        }
    }

    /// A shop entered from two street rooms, with no uids to say which is
    /// its front door, hangs beside the same one every time. Its door
    /// edges came out of a hash map, and the first of a tie won: 105 of
    /// 200 runs put it by one street room and 95 by the other.
    #[test]
    fn a_shop_with_two_street_doors_hangs_in_the_same_place_every_run() {
        let mut rooms = Vec::new();
        for i in 0..8u32 {
            let mut exits = Vec::new();
            if i > 0 {
                exits.push(exit(i - 1, "west"));
            }
            if i < 7 {
                exits.push(exit(i + 1, "east"));
            }
            if i == 1 || i == 6 {
                exits.push(door(100, "shop"));
            }
            rooms.push(room(i, "[Street]", OUT, exits));
        }
        rooms.push(room(100, "[Shop]", IN, vec![door(999, "back")]));
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let first = generate_layout(&map);
        for _ in 0..50 {
            assert_eq!(
                generate_layout(&map),
                first,
                "the layout changed between runs"
            );
        }
    }
}
