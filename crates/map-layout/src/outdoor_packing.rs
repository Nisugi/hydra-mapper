//! `pack_groups` and the outdoor connector-packing pipeline: the three
//! passes documented in `packer` (image-anchored, connector BFS, strip
//! fallback), split out under `plan/05` Rule 4.1 once `packer.rs` itself
//! passed 800 lines. Shares `packer`'s geometry primitives rather than
//! duplicating them.

use std::collections::{BTreeMap, HashMap, HashSet};

use cena_map::Map;

use crate::direction::DirectionMap;
use crate::packer::{
    Anchor, AnchorLine, BBox, Edge, GROUP_PADDING, PackInfo, Segment, add_bridged_edges,
    collect_connector_edges, commit_segments, estimate_scale, find_best_connector_offset,
    find_free_offset, find_primary_image, js_round, occupied_bounds, place_group,
};
use crate::positioner::{Cell, Group, PackMethod};

/// Port of `ClusterPacker.packGroups`: place the packed subset (typically
/// the outdoor components) onto one shared sheet. `packed` lists indices
/// into `groups`; all of `groups` is consulted for bridged virtual edges.
#[must_use]
pub fn pack_groups(
    groups: &mut [Group],
    packed: &[usize],
    map: &Map,
    dirs: &DirectionMap,
) -> PackInfo {
    if packed.is_empty() {
        return PackInfo::default();
    }
    let packed_set: HashSet<usize> = packed.iter().copied().collect();

    let mut edges = collect_connector_edges(groups, packed, map);
    if groups.len() > packed.len() {
        add_bridged_edges(&mut edges, groups, &packed_set, map);
    }

    let anchors = collect_image_anchors(groups, packed, map);
    let primary_image = find_primary_image(groups, packed, &anchors);
    let scale = estimate_scale(groups, packed, &anchors, primary_image.as_deref());

    let mut state = PackingState::default();

    pack_image_anchored(
        groups,
        packed,
        &edges,
        &packed_set,
        map,
        dirs,
        &anchors,
        primary_image.as_deref(),
        scale,
        &mut state,
    );
    pack_by_connectors(groups, packed, &edges, &packed_set, map, dirs, &mut state);
    pack_strip_fallback(groups, packed, &mut state);

    let mut methods: BTreeMap<String, usize> = BTreeMap::new();
    for &idx in packed {
        let name = groups[idx].packing.map_or("none", PackMethod::name);
        *methods.entry(name.to_owned()).or_insert(0) += 1;
    }
    PackInfo {
        primary_image,
        methods,
        scale,
    }
}

/// Rooms carrying `image` + coordinates, per group in room order --
/// [`pack_groups`]' input to its image-anchored pass.
fn collect_image_anchors(
    groups: &[Group],
    packed: &[usize],
    map: &Map,
) -> HashMap<usize, Vec<Anchor>> {
    let mut anchors: HashMap<usize, Vec<Anchor>> = HashMap::new();
    for &idx in packed {
        let mut list = Vec::new();
        for &room_id in &groups[idx].room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            if let Some(image) = &room.image {
                let [x1, y1, x2, y2] = image.rect;
                list.push(Anchor {
                    room_id,
                    image: image.file.clone(),
                    px: f64::from(x1 + x2) / 2.0,
                    py: f64::from(y1 + y2) / 2.0,
                });
            }
        }
        if !list.is_empty() {
            anchors.insert(idx, list);
        }
    }
    anchors
}

/// What every pass of [`pack_groups`] shares and adds to: the occupied
/// cells, which groups have a sheet offset, and the obstacles later passes
/// route around.
#[derive(Default)]
struct PackingState {
    occupied: HashSet<Cell>,
    placed: HashSet<usize>,
    placed_segments: Vec<Segment>,
    placed_boxes: Vec<BBox>,
}

/// Pass 1: seat every group anchored to the primary image at the cell its
/// hand-drawn coordinates imply, nearest free offset.
#[allow(clippy::too_many_arguments)]
fn pack_image_anchored(
    groups: &mut [Group],
    packed: &[usize],
    edges: &HashMap<usize, Vec<Edge>>,
    packed_set: &HashSet<usize>,
    map: &Map,
    dirs: &DirectionMap,
    anchors: &HashMap<usize, Vec<Anchor>>,
    primary_image: Option<&str>,
    scale: f64,
    state: &mut PackingState,
) {
    let Some(primary) = primary_image else {
        return;
    };
    // `packed`'s own order, not the set's: `sort_by` is stable, and exact
    // ties on (anchor_count, room_ids.len()) must break the same way the
    // reference's insertion order breaks them.
    let mut anchored: Vec<usize> = packed
        .iter()
        .copied()
        .filter(|idx| {
            anchors
                .get(idx)
                .is_some_and(|l| l.iter().any(|a| a.image == primary))
        })
        .collect();
    let anchor_count = |idx: usize| {
        anchors
            .get(&idx)
            .map_or(0, |l| l.iter().filter(|a| a.image == primary).count())
    };
    anchored.sort_by(|&a, &b| {
        anchor_count(b)
            .cmp(&anchor_count(a))
            .then(groups[b].room_ids.len().cmp(&groups[a].room_ids.len()))
    });

    for idx in anchored {
        let group_anchors: Vec<&Anchor> = anchors[&idx]
            .iter()
            .filter(|a| a.image == primary)
            .collect();
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        for a in &group_anchors {
            let internal = groups[idx].positions[&a.room_id];
            sum_x += a.px / scale - f64::from(internal.x);
            sum_y += a.py / scale - f64::from(internal.y);
        }
        #[allow(clippy::cast_precision_loss)]
        let n = group_anchors.len() as f64;
        let proposed = Cell {
            x: js_round(sum_x / n),
            y: js_round(sum_y / n),
        };
        if let Some(offset) = find_free_offset(&groups[idx], proposed, &state.occupied) {
            place_group(
                groups,
                idx,
                offset,
                PackMethod::Image,
                &mut state.occupied,
                &mut state.placed,
            );
            commit_segments(
                groups,
                idx,
                edges,
                packed_set,
                &state.placed,
                map,
                dirs,
                &mut state.placed_segments,
                &mut state.placed_boxes,
            );
        }
    }
}

/// The unplaced group with the most edges to placed groups, and the
/// specific placed edges that qualify it -- [`pack_by_connectors`]' choice
/// of what to place next each round.
struct BestConnected {
    idx: usize,
    placed_edges: Vec<Edge>,
    min_uid_delta: u64,
}

fn best_connected_to_placed(
    packed: &[usize],
    edges: &HashMap<usize, Vec<Edge>>,
    placed: &HashSet<usize>,
    deferred: &HashSet<usize>,
) -> Option<BestConnected> {
    let mut best: Option<BestConnected> = None;
    for &idx in packed {
        if placed.contains(&idx) || deferred.contains(&idx) {
            continue;
        }
        let placed_edges: Vec<Edge> = edges
            .get(&idx)
            .map(|l| {
                l.iter()
                    .filter(|e| placed.contains(&e.other_group))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        if placed_edges.is_empty() {
            continue;
        }
        let min_uid_delta = placed_edges
            .iter()
            .map(|e| e.uid_delta)
            .min()
            .unwrap_or_else(|| unreachable!("placed_edges is non-empty"));
        let better = match &best {
            None => true,
            Some(b) => {
                placed_edges.len() > b.placed_edges.len()
                    || (placed_edges.len() == b.placed_edges.len()
                        && min_uid_delta < b.min_uid_delta)
            }
        };
        if better {
            best = Some(BestConnected {
                idx,
                placed_edges,
                min_uid_delta,
            });
        }
    }
    best
}

/// When nothing touches the placed set: seed the largest remaining
/// connector super-cluster below the current map, all such seeds pinned to
/// one shared left edge.
#[allow(clippy::too_many_arguments)]
fn seed_next_super_cluster(
    groups: &mut [Group],
    packed: &[usize],
    edges: &HashMap<usize, Vec<Edge>>,
    packed_set: &HashSet<usize>,
    deferred: &HashSet<usize>,
    map: &Map,
    dirs: &DirectionMap,
    seed_base_x: &mut Option<i32>,
    state: &mut PackingState,
) -> bool {
    let mut seed_group: Option<usize> = None;
    for &idx in packed {
        if state.placed.contains(&idx)
            || deferred.contains(&idx)
            || edges.get(&idx).map_or(0, Vec::len) == 0
        {
            continue;
        }
        if seed_group.is_none_or(|s| groups[idx].room_ids.len() > groups[s].room_ids.len()) {
            seed_group = Some(idx);
        }
    }
    let Some(idx) = seed_group else {
        return false;
    };
    let bounds = groups[idx].bounds();
    let (extent_min_x, extent_max_y) = occupied_bounds(&state.occupied);
    let base_x = *seed_base_x.get_or_insert(extent_min_x);
    let proposed = Cell {
        x: base_x - bounds.min_x,
        y: extent_max_y + GROUP_PADDING - bounds.min_y,
    };
    let offset = find_free_offset(&groups[idx], proposed, &state.occupied).unwrap_or(proposed);
    place_group(
        groups,
        idx,
        offset,
        PackMethod::Seed,
        &mut state.occupied,
        &mut state.placed,
    );
    commit_segments(
        groups,
        idx,
        edges,
        packed_set,
        &state.placed,
        map,
        dirs,
        &mut state.placed_segments,
        &mut state.placed_boxes,
    );
    true
}

/// Pass 2: BFS outward from a seed, landing each next group beside the
/// placed neighbor most likely to be physically adjacent (min uid delta),
/// scored by [`find_best_connector_offset`]. Whatever cannot find room
/// nearby is deferred to the strip pass.
#[allow(clippy::too_many_arguments)]
fn pack_by_connectors(
    groups: &mut [Group],
    packed: &[usize],
    edges: &HashMap<usize, Vec<Edge>>,
    packed_set: &HashSet<usize>,
    map: &Map,
    dirs: &DirectionMap,
    state: &mut PackingState,
) {
    if state.placed.is_empty() {
        let mut largest = packed[0];
        for &idx in packed {
            if groups[idx].room_ids.len() > groups[largest].room_ids.len() {
                largest = idx;
            }
        }
        // The initial seed does NOT commit its segments (reference
        // behavior).
        place_group(
            groups,
            largest,
            Cell { x: 0, y: 0 },
            PackMethod::Seed,
            &mut state.occupied,
            &mut state.placed,
        );
    }

    let mut deferred: HashSet<usize> = HashSet::new();
    let mut seed_base_x: Option<i32> = None;
    loop {
        let Some(best) = best_connected_to_placed(packed, edges, &state.placed, &deferred) else {
            if seed_next_super_cluster(
                groups,
                packed,
                edges,
                packed_set,
                &deferred,
                map,
                dirs,
                &mut seed_base_x,
                state,
            ) {
                continue;
            }
            break;
        };

        let edge = *best
            .placed_edges
            .iter()
            .min_by_key(|e| e.uid_delta)
            .unwrap_or_else(|| unreachable!("placed_edges is non-empty"));
        let neighbor_cell = groups[edge.other_group].final_cell(edge.other_room_id);
        let internal = groups[best.idx].positions[&edge.room_id];

        let anchor_lines: Vec<AnchorLine> = best
            .placed_edges
            .iter()
            .map(|e| AnchorLine {
                internal: groups[best.idx].positions[&e.room_id],
                target: groups[e.other_group].final_cell(e.other_room_id),
                room_id: e.room_id,
                other_room_id: e.other_room_id,
            })
            .collect();

        let proposed = Cell {
            x: neighbor_cell.x - internal.x,
            y: neighbor_cell.y - internal.y,
        };
        if let Some((offset, _)) = find_best_connector_offset(
            &groups[best.idx],
            proposed,
            &state.occupied,
            &anchor_lines,
            &state.placed_segments,
            &state.placed_boxes,
        ) {
            place_group(
                groups,
                best.idx,
                offset,
                PackMethod::Connector,
                &mut state.occupied,
                &mut state.placed,
            );
            commit_segments(
                groups,
                best.idx,
                edges,
                packed_set,
                &state.placed,
                map,
                dirs,
                &mut state.placed_segments,
                &mut state.placed_boxes,
            );
        } else {
            // No room nearby; leave it for the strip pass and keep going.
            deferred.insert(best.idx);
        }
    }
}

/// Pass 3: whatever is still unplaced lines up in a strip below everything
/// else, largest first.
fn pack_strip_fallback(groups: &mut [Group], packed: &[usize], state: &mut PackingState) {
    let mut leftovers: Vec<usize> = packed
        .iter()
        .copied()
        .filter(|idx| !state.placed.contains(idx))
        .collect();
    leftovers.sort_by(|&a, &b| groups[b].room_ids.len().cmp(&groups[a].room_ids.len()));
    if leftovers.is_empty() {
        return;
    }
    let mut max_y = 0;
    let mut min_x = 0;
    for c in &state.occupied {
        if c.y > max_y {
            max_y = c.y;
        }
        if c.x < min_x {
            min_x = c.x;
        }
    }
    let mut cursor_x = min_x;
    let strip_y = max_y + GROUP_PADDING;
    for idx in leftovers {
        let bounds = groups[idx].bounds();
        let proposed = Cell {
            x: cursor_x - bounds.min_x,
            y: strip_y - bounds.min_y,
        };
        let offset = find_free_offset(&groups[idx], proposed, &state.occupied).unwrap_or(proposed);
        place_group(
            groups,
            idx,
            offset,
            PackMethod::Strip,
            &mut state.occupied,
            &mut state.placed,
        );
        cursor_x += bounds.width() + GROUP_PADDING;
    }
}
