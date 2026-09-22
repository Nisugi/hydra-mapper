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
use serde::{Deserialize, Serialize};

use crate::classifier::Entrance;
use crate::direction::DirectionMap;
use crate::packer::{
    AnchorLine, BBox, CONNECTOR_COMMIT_CAP, DIRECTIONAL_COMMIT_CAP, Edge, GROUP_PADDING,
    INLINE_BUDGET_PER_DOOR, INLINE_CROWD_MAX, INLINE_CROWD_RADIUS, INLINE_DOOR_MAX_CELLS,
    INLINE_MAX_BUILDINGS, Segment, chebyshev, find_best_connector_offset, find_free_offset,
    uid_delta,
};
use crate::positioner::{Cell, Group, PackMethod};

/// Obstacles the packed outdoor sheet already presents: every final room
/// cell, drawable segments (connectors <= 30 cells, intra-group
/// directional edges <= 8, deduped by unordered pair), group bounding
/// boxes, and the final cell of every outdoor room (doorway targets).
fn outdoor_obstacles(
    groups: &[Group],
    outdoor: &[usize],
    map: &Map,
    dirs: &DirectionMap,
) -> (
    HashSet<Cell>,
    Vec<Segment>,
    Vec<BBox>,
    HashMap<RoomId, Cell>,
) {
    let mut occupied: HashSet<Cell> = HashSet::new();
    let mut cell_of: HashMap<RoomId, Cell> = HashMap::new();
    let mut component_of: HashMap<RoomId, usize> = HashMap::new();
    for &idx in outdoor {
        for &id in &groups[idx].room_ids {
            let c = groups[idx].final_cell(id);
            occupied.insert(c);
            cell_of.insert(id, c);
            component_of.insert(id, idx);
        }
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
    for &idx in outdoor {
        for &room_id in &groups[idx].room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                let target_id = exit.to;
                let Some(&b) = cell_of.get(&target_id) else {
                    continue;
                };
                let key = (room_id.min(target_id), room_id.max(target_id));
                if seen.contains(&key) {
                    continue;
                }
                let a = cell_of[&room_id];
                let cap = if component_of[&target_id] == idx {
                    // Directional check before the dedup mark, so an edge
                    // stored one-way gets its chance from the other side.
                    if dirs.get(room_id, target_id).is_none() {
                        continue;
                    }
                    DIRECTIONAL_COMMIT_CAP
                } else {
                    CONNECTOR_COMMIT_CAP
                };
                seen.insert(key);
                if chebyshev(a, b) > cap {
                    continue;
                }
                segments.push(Segment {
                    a,
                    b,
                    ra: room_id,
                    rb: target_id,
                });
            }
        }
    }

    let boxes: Vec<BBox> = outdoor
        .iter()
        .map(|&idx| {
            let b = groups[idx].bounds();
            let off = groups[idx]
                .base_offset
                .unwrap_or_else(|| unreachable!("outdoor groups are packed"));
            BBox {
                min_x: b.min_x + off.x,
                max_x: b.max_x + off.x,
                min_y: b.min_y + off.y,
                max_y: b.max_y + off.y,
            }
        })
        .collect();

    (occupied, segments, boxes, cell_of)
}

/// One candidate placement for a building cluster's try-inline seat. Module
/// scope: [`inline_interior_clusters`] builds one of these per building
/// cluster before greedily placing them best-score-first.
struct Candidate {
    members: Vec<usize>,
    local: HashMap<usize, Cell>,
    merged: Group,
    anchor_lines: Vec<AnchorLine>,
    proposed: Cell,
    /// Outdoor rooms hosting this building's doorways -- exempt from the
    /// crowding count (the building necessarily sits beside them).
    hosts: HashSet<RoomId>,
}

/// Build one try-inline [`Candidate`] per building cluster: its merged
/// floor plan, doorways, and the proposed seat beside the most-trusted one
/// (lowest uid delta, as pass 2). Clusters with no doorway onto the outdoor
/// sheet, or pinned to the shelf by a curated flip, are skipped.
#[allow(clippy::too_many_arguments)]
fn build_inline_candidates(
    groups: &mut [Group],
    interiors: &[usize],
    clusters: &HashMap<usize, usize>,
    entrances: &HashMap<usize, Vec<Entrance>>,
    forced_shelf: &HashSet<usize>,
    outdoor_cell_of: &HashMap<RoomId, Cell>,
    map: &Map,
) -> Vec<Candidate> {
    // Cluster membership in canonical order.
    let mut members_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &idx in interiors {
        members_of
            .entry(clusters.get(&idx).copied().unwrap_or(idx))
            .or_default()
            .push(idx);
    }
    // Town check counts every building, curated pins included.
    if members_of.len() > INLINE_MAX_BUILDINGS {
        return Vec::new();
    }
    members_of.retain(|_, members| members.iter().all(|m| !forced_shelf.contains(m)));

    let mut candidates: Vec<Candidate> = Vec::new();
    for (&cluster, members) in &mut members_of {
        members.sort_unstable();
        // Merged building frame, exactly as the shelf builds it.
        let mut local: HashMap<usize, Cell> = HashMap::new();
        if members.len() == 1 {
            local.insert(members[0], Cell::default());
        } else {
            merge_cluster_members(groups, members, map, &mut local, None);
        }
        let mut positions: HashMap<RoomId, Cell> = HashMap::new();
        let mut room_ids: Vec<RoomId> = Vec::new();
        for &m in members.iter() {
            let off = local[&m];
            for &id in &groups[m].room_ids {
                let p = groups[m].positions[&id];
                positions.insert(
                    id,
                    Cell {
                        x: p.x + off.x,
                        y: p.y + off.y,
                    },
                );
                room_ids.push(id);
            }
        }

        // Doorways into this building, deduped by room pair.
        let mut doors: Vec<Entrance> = Vec::new();
        for &m in members.iter() {
            for e in entrances.get(&m).map_or(&[] as &[_], Vec::as_slice) {
                if outdoor_cell_of.contains_key(&e.outdoor_room_id) && !doors.contains(e) {
                    doors.push(*e);
                }
            }
        }
        if doors.is_empty() {
            continue; // nothing to seat it against
        }
        let anchor_lines: Vec<AnchorLine> = doors
            .iter()
            .map(|e| AnchorLine {
                internal: positions[&e.interior_room_id],
                target: outdoor_cell_of[&e.outdoor_room_id],
                room_id: e.interior_room_id,
                other_room_id: e.outdoor_room_id,
            })
            .collect();

        let seat = doors
            .iter()
            .min_by_key(|e| uid_delta(map.room(e.interior_room_id), map.room(e.outdoor_room_id)))
            .unwrap_or_else(|| unreachable!("doors is non-empty"));
        let target = outdoor_cell_of[&seat.outdoor_room_id];
        let internal = positions[&seat.interior_room_id];
        let proposed = Cell {
            x: target.x - internal.x,
            y: target.y - internal.y,
        };

        candidates.push(Candidate {
            members: members.clone(),
            local,
            merged: Group {
                index: cluster,
                room_ids,
                positions,
                violations: Vec::new(),
                base_offset: None,
                packing: None,
                name: None,
            },
            anchor_lines,
            proposed,
            hosts: doors.iter().map(|e| e.outdoor_room_id).collect(),
        });
    }
    candidates
}

/// Whether seating `merged` at `offset` would crowd genuinely open ground:
/// more than [`INLINE_CROWD_MAX`] distinct outdoor rooms (doorway hosts
/// exempted) within [`INLINE_CROWD_RADIUS`] of any of its cells.
fn crowds_open_ground(
    merged: &Group,
    offset: Cell,
    hosts: &HashSet<RoomId>,
    room_at: &HashMap<Cell, RoomId>,
) -> bool {
    let mut nearby: HashSet<RoomId> = HashSet::new();
    for p in merged.positions.values() {
        for dx in -INLINE_CROWD_RADIUS..=INLINE_CROWD_RADIUS {
            for dy in -INLINE_CROWD_RADIUS..=INLINE_CROWD_RADIUS {
                let c = Cell {
                    x: p.x + offset.x + dx,
                    y: p.y + offset.y + dy,
                };
                let Some(&id) = room_at.get(&c) else {
                    continue;
                };
                if !hosts.contains(&id) && nearby.insert(id) && nearby.len() > INLINE_CROWD_MAX {
                    return true;
                }
            }
        }
    }
    false
}

/// Try-inline pass: interior buildings that seat cleanly beside their
/// doorways join the outdoor sheet instead of the shelf, so a lone grotto
/// stays discoverable on the main map. Each cluster is merged into one
/// floor plan, seated by the same connector-aware scorer as pass 2, and
/// accepted only within the inline budget: short doorway connectors, no
/// crossings, essentially no courtyard intrusion. Whatever fails keeps
/// today's shelf behavior. Placement is greedy best-score-first so
/// contending buildings resolve deterministically; forced-shelf groups
/// (curated Interior flips) pin their whole building to the shelf.
///
/// Returns the group indices that moved, ascending.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn inline_interior_clusters(
    groups: &mut [Group],
    outdoor: &[usize],
    interiors: &[usize],
    clusters: &HashMap<usize, usize>,
    entrances: &HashMap<usize, Vec<Entrance>>,
    forced_shelf: &HashSet<usize>,
    map: &Map,
    dirs: &DirectionMap,
) -> Vec<usize> {
    if interiors.is_empty() || outdoor.is_empty() {
        return Vec::new();
    }
    let (mut occupied, mut segments, mut boxes, outdoor_cell_of) =
        outdoor_obstacles(groups, outdoor, map, dirs);
    let mut candidates = build_inline_candidates(
        groups,
        interiors,
        clusters,
        entrances,
        forced_shelf,
        &outdoor_cell_of,
        map,
    );

    // Outdoor room per final cell, kept current as buildings place, so the
    // crowding gate sees earlier inlines as neighbors too.
    let mut room_at: HashMap<Cell, RoomId> =
        outdoor_cell_of.iter().map(|(&id, &c)| (c, id)).collect();

    // Greedy best-score-first; every placement changes the obstacles, so
    // the survivors are re-scored each round. Ties keep the earlier
    // candidate (ascending cluster id -- deterministic).
    let mut inlined: Vec<usize> = Vec::new();
    loop {
        let mut best: Option<(usize, Cell, i64)> = None;
        for (ci, cand) in candidates.iter().enumerate() {
            let Some((offset, score)) = find_best_connector_offset(
                &cand.merged,
                cand.proposed,
                &occupied,
                &cand.anchor_lines,
                &segments,
                &boxes,
            ) else {
                continue;
            };
            if score
                > i64::try_from(cand.anchor_lines.len()).unwrap_or(i64::MAX)
                    * INLINE_BUDGET_PER_DOOR
            {
                continue;
            }
            let doors_short = cand.anchor_lines.iter().all(|a| {
                let endpoint = Cell {
                    x: a.internal.x + offset.x,
                    y: a.internal.y + offset.y,
                };
                chebyshev(endpoint, a.target) <= INLINE_DOOR_MAX_CELLS
            });
            if !doors_short {
                continue;
            }
            if crowds_open_ground(&cand.merged, offset, &cand.hosts, &room_at) {
                continue;
            }
            if best.is_none_or(|(_, _, s)| score < s) {
                best = Some((ci, offset, score));
            }
        }
        let Some((ci, offset, _)) = best else {
            break;
        };
        let cand = candidates.remove(ci);
        for &m in &cand.members {
            let off = cand.local[&m];
            groups[m].base_offset = Some(Cell {
                x: offset.x + off.x,
                y: offset.y + off.y,
            });
            groups[m].packing = Some(PackMethod::InteriorInline);
        }
        for (&id, p) in &cand.merged.positions {
            let cell = Cell {
                x: p.x + offset.x,
                y: p.y + offset.y,
            };
            occupied.insert(cell);
            room_at.insert(cell, id);
        }
        for a in &cand.anchor_lines {
            let endpoint = Cell {
                x: a.internal.x + offset.x,
                y: a.internal.y + offset.y,
            };
            segments.push(Segment {
                a: endpoint,
                b: a.target,
                ra: a.room_id,
                rb: a.other_room_id,
            });
        }
        let b = cand.merged.bounds();
        boxes.push(BBox {
            min_x: b.min_x + offset.x,
            max_x: b.max_x + offset.x,
            min_y: b.min_y + offset.y,
            max_y: b.max_y + offset.y,
        });
        inlined.extend(cand.members.iter().copied());
    }
    inlined.sort_unstable();
    inlined
}

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
    /// For an island, the street room drawn among its buildings: which
    /// room, and where in this item's frame.
    anchor: Option<(RoomId, Cell)>,
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

    // **Islands.** Every cluster with a street door is claimed by that
    // door's outdoor room -- the lowest-id one, when a building has
    // several -- and all the clusters off one street room become a
    // single item with that room drawn at its heart. A cluster with no
    // street door at all (reached only through other interiors, or cut
    // off) shelves on its own as before.
    //
    // Whole clusters, never single groups: a back room is usually its own
    // group behind the shopfront, and seating the shopfront while leaving
    // the back room would sever a building the clustering already keeps
    // together.
    let mut islands: BTreeMap<RoomId, Vec<usize>> = BTreeMap::new();
    let mut alone: Vec<Vec<usize>> = Vec::new();
    for members in members_of.values() {
        let door_room = members
            .iter()
            .filter_map(|m| entrances.get(m))
            .flatten()
            .map(|e| e.outdoor_room_id)
            .filter(|r| outdoor_cell.contains_key(r))
            .min_by_key(|r| r.0);
        match door_room {
            Some(a) => islands
                .entry(a)
                .or_default()
                .extend(members.iter().copied()),
            None => alone.push(members.clone()),
        }
    }

    let mut items: Vec<Item> = Vec::new();
    for (anchor, mut members) in islands {
        members.sort_unstable();
        items.push(shelf_item(
            groups,
            &members,
            Some(anchor),
            map,
            entrances,
            &outdoor_cell,
        ));
    }
    for members in &alone {
        items.push(shelf_item(
            groups,
            members,
            None,
            map,
            entrances,
            &outdoor_cell,
        ));
    }
    items
}

/// One shelf item: the members merged into a frame normalised to a
/// (0,0) top-left, with the anchor -- if this is an island -- kept in
/// that frame too.
fn shelf_item(
    groups: &[Group],
    members: &[usize],
    anchor: Option<RoomId>,
    map: &Map,
    entrances: &HashMap<usize, Vec<Entrance>>,
    outdoor_cell: &HashMap<RoomId, Cell>,
) -> Item {
    let mut local: HashMap<usize, Cell> = HashMap::new();
    if anchor.is_none() && members.len() == 1 {
        local.insert(members[0], Cell::default());
    } else {
        merge_cluster_members(groups, members, map, &mut local, anchor);
    }
    // Normalize the frame to a (0,0) top-left, the anchor included.
    let mut min = Cell {
        x: i32::MAX,
        y: i32::MAX,
    };
    let mut max = Cell {
        x: i32::MIN,
        y: i32::MIN,
    };
    for (&idx, off) in &local {
        if idx == ANCHOR {
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
    let mut ordered: Vec<(usize, Cell)> = members
        .iter()
        .map(|&idx| {
            let off = local[&idx];
            (
                idx,
                Cell {
                    x: off.x - min.x,
                    y: off.y - min.y,
                },
            )
        })
        .collect();
    ordered.sort_unstable_by_key(|&(idx, _)| idx);
    let anchor_local = anchor.and_then(|a| {
        local.get(&ANCHOR).map(|off| {
            (
                a,
                Cell {
                    x: off.x - min.x,
                    y: off.y - min.y,
                },
            )
        })
    });
    // Where to shelve it: an island by its street room, a lone
    // building by whichever door it has, if any.
    let door_room = anchor.or_else(|| {
        members
            .iter()
            .filter_map(|m| entrances.get(m))
            .flatten()
            .map(|e| e.outdoor_room_id)
            .min_by_key(|r| r.0)
    });
    Item {
        local: ordered,
        width: max.x - min.x + 1,
        height: max.y - min.y + 1,
        door: door_room.and_then(|r| outdoor_cell.get(&r).copied()),
        tie: (
            door_room.map_or(u32::MAX, |r| r.0),
            members.first().copied().unwrap_or(usize::MAX),
        ),
        anchor: anchor_local,
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
) -> Vec<Anchor> {
    if interior.is_empty() {
        return Vec::new();
    }

    let mut items = shelf_items(groups, interior, clusters, map, entrances, outdoor);

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

    let mut anchors: Vec<Anchor> = Vec::new();
    let mut cursor_x = 0;
    let mut cursor_y = 0;
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
        if let Some((room, local)) = item.anchor {
            anchors.push(Anchor {
                room,
                cell: Cell {
                    x: cursor_x + local.x,
                    y: cursor_y + local.y,
                },
            });
        }
        cursor_x += item.width + GROUP_PADDING;
        row_height = row_height.max(item.height);
    }
    anchors
}

/// Passages between one building's own members: member -> the edges its
/// rooms have into another member of the same set. Only edges within
/// `members` count; a passage out to a non-member group is not this
/// building's problem to place.
/// A street room echoed onto the interiors sheet, at the heart of the
/// island of buildings that open off it.
///
/// **An echo, not a room.** The room still belongs to its outdoor group
/// and has its one true cell there; this is a second place it is *drawn*,
/// so that a shop's door edge has both ends on one sheet and can be a
/// line. Nothing that maps a room to its group should learn about these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub room: RoomId,
    /// Its cell on the interiors sheet.
    pub cell: Cell,
}

/// The virtual member index of an island's anchor room. Not an index into
/// `groups` -- every access that would be is branched on this instead --
/// but it lets the merge treat "the street room" as one more member with
/// one room at the origin.
const ANCHOR: usize = usize::MAX;

fn passages_within(
    groups: &[Group],
    members: &[usize],
    map: &Map,
    anchor: Option<RoomId>,
) -> HashMap<usize, Vec<Edge>> {
    let member_set: HashSet<usize> = members.iter().copied().collect();
    let mut group_of: HashMap<RoomId, usize> = HashMap::new();
    for &idx in members {
        for &id in &groups[idx].room_ids {
            group_of.insert(id, idx);
        }
    }
    if let Some(a) = anchor {
        group_of.insert(a, ANCHOR);
    }
    let mut edges: HashMap<usize, Vec<Edge>> = HashMap::new();
    for &idx in members {
        for &room_id in &groups[idx].room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                let target_id = exit.to;
                let Some(&other) = group_of.get(&target_id) else {
                    continue;
                };
                if other == idx || (other != ANCHOR && !member_set.contains(&other)) {
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
    // The anchor's own exits, so a door the street room opens *into* the
    // building still seats the building even when the way back is not a
    // plain exit.
    if let Some(a) = anchor
        && let Some(room) = map.room(a)
    {
        for exit in &room.exits {
            let Some(&idx) = group_of.get(&exit.to) else {
                continue;
            };
            if idx == ANCHOR {
                continue;
            }
            edges.entry(idx).or_default().push(Edge {
                other_group: ANCHOR,
                room_id: exit.to,
                other_room_id: a,
                uid_delta: uid_delta(map.room(exit.to), Some(room)),
            });
        }
    }
    edges
}

/// Merge a cluster's members into one floor plan: each is placed beside
/// the room its passage connects to, shortest-uid-delta passage first.
///
/// With an `anchor`, this builds an **island**: the anchor -- a street
/// room, one cell at the origin -- is the seed, and every building's door
/// room is proposed *on* that cell and slid to the nearest free one. That
/// puts the buildings off one street room pressed around it, which is the
/// point of drawing the street room on the interiors sheet at all.
fn merge_cluster_members(
    groups: &[Group],
    members: &[usize],
    map: &Map,
    local: &mut HashMap<usize, Cell>,
    anchor: Option<RoomId>,
) {
    let edges = passages_within(groups, members, map, anchor);

    let mut occupied: HashSet<Cell> = HashSet::new();
    let place = |idx: usize, off: Cell, occupied: &mut HashSet<Cell>| {
        if idx == ANCHOR {
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
    // Where a member's room sits in the frame, the anchor being one room
    // at its own offset.
    let room_at = |idx: usize, room: RoomId, local: &HashMap<usize, Cell>| -> Cell {
        let off = local[&idx];
        if idx == ANCHOR {
            return off;
        }
        let p = groups[idx].positions[&room];
        Cell {
            x: p.x + off.x,
            y: p.y + off.y,
        }
    };
    let width_of = |idx: usize| {
        if idx == ANCHOR {
            1
        } else {
            groups[idx].bounds().width()
        }
    };

    let seed = if anchor.is_some() {
        ANCHOR
    } else {
        *members
            .iter()
            .max_by_key(|&&idx| (groups[idx].room_ids.len(), std::cmp::Reverse(idx)))
            .unwrap_or_else(|| unreachable!("members is non-empty"))
    };
    local.insert(seed, Cell::default());
    place(seed, Cell::default(), &mut occupied);

    loop {
        // Most placed-passages first; ties to the lowest member index.
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

        let Some((idx, placed_edges)) = best else {
            // Stragglers (shouldn't happen: clusters are connected).
            let Some(&idx) = members.iter().find(|&&m| !local.contains_key(&m)) else {
                return;
            };
            let max_x = occupied.iter().map(|c| c.x).max().unwrap_or(0);
            let bounds = groups[idx].bounds();
            let proposed = Cell {
                x: max_x + 2 - bounds.min_x,
                y: 0,
            };
            let off = find_free_offset(&groups[idx], proposed, &occupied).unwrap_or(proposed);
            local.insert(idx, off);
            place(idx, off, &mut occupied);
            continue;
        };

        let edge = *placed_edges
            .iter()
            .min_by_key(|e| e.uid_delta)
            .unwrap_or_else(|| unreachable!("placed_edges is non-empty"));
        let neighbor_room = room_at(edge.other_group, edge.other_room_id, local);
        let internal = groups[idx].positions[&edge.room_id];
        // Land the passage endpoints as close together as the frame
        // allows.
        let proposed = Cell {
            x: neighbor_room.x - internal.x,
            y: neighbor_room.y - internal.y,
        };
        let off = find_free_offset(&groups[idx], proposed, &occupied).unwrap_or(Cell {
            x: proposed.x + width_of(edge.other_group) + 1,
            y: proposed.y,
        });
        local.insert(idx, off);
        place(idx, off, &mut occupied);
    }
}

#[cfg(test)]
mod tests {
    use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId};

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
        assert_eq!(
            layout.anchors.len(),
            CORNERS as usize,
            "expected one echo per street corner: {:?}",
            layout.anchors
        );
        for anchor in &layout.anchors {
            let corner = anchor.room.0 - 1000;
            for shop in [corner * 20, corner * 20 + 10] {
                let d = apart(cell_of(shop), anchor.cell);
                assert!(
                    d <= 2,
                    "shop {shop} sits {d} cells from its street room's echo"
                );
            }
            for other in &layout.anchors {
                if other.room == anchor.room {
                    continue;
                }
                assert!(
                    apart(anchor.cell, other.cell) > 2,
                    "two street rooms' echoes ({} and {}) sit on top of each other",
                    anchor.room.0,
                    other.room.0
                );
            }
        }
    }
}
