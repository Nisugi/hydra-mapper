//! Cluster packing, outdoor half -- ported from `reference/VellumFE/src/
//! core/layout_engine/packer.rs` (`cluster-packer.js` upstream of that,
//! spec §7).
//!
//! Places connected components relative to each other by, in priority
//! order: `image_coords` anchors (hand-drawn map overlays), connector
//! edges (edges between components prove physical adjacency; smallest uid
//! delta wins ties), and a strip fallback.
//!
//! Interiors go on their own shelf, in `crate::interior_shelf` -- split out
//! under `plan/05` Rule 4.1 (move code down, do not raise the cap) once
//! this file passed 800 lines. That module re-uses this one's geometry
//! primitives (`Edge`, `Segment`, `BBox`, `AnchorLine`, `chebyshev`,
//! `uid_delta`, `find_free_offset`, `find_best_connector_offset`), `pub(
//! crate)` here rather than duplicated there.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use cena_map::{Map, RoomId};

use crate::direction::DirectionMap;
use crate::positioner::{Cell, Group, PackMethod};

pub(crate) const GROUP_PADDING: i32 = 3; // cells between strip-placed groups
pub(crate) const SEARCH_RADIUS: i32 = 30; // max spiral distance resolving collisions
const DEFAULT_SCALE: f64 = 30.0; // px per grid cell when estimation has no data
const SCALE_MIN: f64 = 5.0;
const SCALE_MAX: f64 = 300.0;
const ANCHOR_PAIR_CAP: usize = 20; // caps the O(n^2) scale-estimation pair walk
const GRID_DELTA_MIN: i32 = 1;
const GRID_DELTA_MAX: i32 = 50;
const CROSSING_PENALTY: i64 = 1000;
const COURTYARD_PENALTY: i64 = 4; // per cell inside another group's bbox
pub(crate) const CONNECTOR_COMMIT_CAP: i32 = 30; // committed connector max length
pub(crate) const DIRECTIONAL_COMMIT_CAP: i32 = 8; // committed intra-group edge max length
const BRIDGED_CONTACT_CAP: usize = 10; // contact pairs per excluded component
const UID_DELTA_MISSING: u64 = u64::MAX;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Edge {
    pub(crate) other_group: usize,
    pub(crate) room_id: RoomId,
    pub(crate) other_room_id: RoomId,
    pub(crate) uid_delta: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Anchor {
    pub(crate) room_id: RoomId,
    pub(crate) image: String,
    pub(crate) px: f64,
    pub(crate) py: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Segment {
    pub(crate) a: Cell,
    pub(crate) b: Cell,
    pub(crate) ra: RoomId,
    pub(crate) rb: RoomId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BBox {
    pub(crate) min_x: i32,
    pub(crate) max_x: i32,
    pub(crate) min_y: i32,
    pub(crate) max_y: i32,
}

/// A connector line the candidate placement will create: the group-internal
/// endpoint and the already-final cell it must reach.
pub(crate) struct AnchorLine {
    pub(crate) internal: Cell,
    pub(crate) target: Cell,
    pub(crate) room_id: RoomId,
    pub(crate) other_room_id: RoomId,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackInfo {
    pub primary_image: Option<String>,
    /// pack method name -> group count (over the packed subset).
    pub methods: BTreeMap<String, usize>,
    pub scale: f64,
}

pub(crate) fn chebyshev(a: Cell, b: Cell) -> i32 {
    (a.x - b.x).abs().max((a.y - b.y).abs())
}

/// JS `Math.round`: half-up toward +infinity (Rust's `round` is half away
/// from zero).
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn js_round(x: f64) -> i32 {
    (x + 0.5).floor() as i32
}

pub(crate) fn uid_delta(a: Option<&cena_map::Room>, b: Option<&cena_map::Room>) -> u64 {
    match (a.and_then(|r| r.uid.first()), b.and_then(|r| r.uid.first())) {
        (Some(&ua), Some(&ub)) => ua.0.abs_diff(ub.0),
        _ => UID_DELTA_MISSING,
    }
}

/// Walk ring `r` around `center` in the reference order: dx from -r to r; at
/// the vertical edges (|dx| == r) every dy, elsewhere only dy = +/-r.
pub(crate) fn for_ring(center: Cell, r: i32, mut f: impl FnMut(Cell)) {
    for dx in -r..=r {
        if dx.abs() == r {
            for dy in -r..=r {
                f(Cell {
                    x: center.x + dx,
                    y: center.y + dy,
                });
            }
        } else {
            for dy in [-r, r] {
                f(Cell {
                    x: center.x + dx,
                    y: center.y + dy,
                });
            }
        }
    }
}

fn fits(group: &Group, offset: Cell, occupied: &HashSet<Cell>) -> bool {
    group.positions.values().all(|p| {
        !occupied.contains(&Cell {
            x: p.x + offset.x,
            y: p.y + offset.y,
        })
    })
}

/// Nearest collision-free offset to the proposed one, spiraling outward.
pub(crate) fn find_free_offset(
    group: &Group,
    proposed: Cell,
    occupied: &HashSet<Cell>,
) -> Option<Cell> {
    if fits(group, proposed, occupied) {
        return Some(proposed);
    }
    for r in 1..=SEARCH_RADIUS {
        let mut found: Option<Cell> = None;
        for_ring(proposed, r, |cand| {
            if found.is_none() && fits(group, cand, occupied) {
                found = Some(cand);
            }
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Strict segment crossing (shared endpoints and collinear touches
/// excluded).
fn segments_cross(a1: Cell, b1: Cell, a2: Cell, b2: Cell) -> bool {
    fn orient(a: Cell, b: Cell, c: Cell) -> i64 {
        let v = (i64::from(b.x) - i64::from(a.x)) * (i64::from(c.y) - i64::from(a.y))
            - (i64::from(b.y) - i64::from(a.y)) * (i64::from(c.x) - i64::from(a.x));
        v.signum()
    }
    let o1 = orient(a1, b1, a2);
    let o2 = orient(a1, b1, b2);
    let o3 = orient(a2, b2, a1);
    let o4 = orient(a2, b2, b1);
    o1 != o2 && o3 != o4 && o1 != 0 && o2 != 0 && o3 != 0 && o4 != 0
}

pub(crate) fn place_group(
    groups: &mut [Group],
    idx: usize,
    offset: Cell,
    method: PackMethod,
    occupied: &mut HashSet<Cell>,
    placed: &mut HashSet<usize>,
) {
    groups[idx].base_offset = Some(offset);
    groups[idx].packing = Some(method);
    placed.insert(idx);
    for p in groups[idx].positions.values() {
        occupied.insert(Cell {
            x: p.x + offset.x,
            y: p.y + offset.y,
        });
    }
}

/// Commit the just-placed group's lines as obstacles for later placements:
/// its connectors to already-placed groups (<= 30 cells) and its
/// intra-group directional edges (<= 8 cells), plus its bounding box.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_segments(
    groups: &[Group],
    idx: usize,
    edges: &HashMap<usize, Vec<Edge>>,
    packed_set: &HashSet<usize>,
    placed: &HashSet<usize>,
    map: &Map,
    dirs: &DirectionMap,
    placed_segments: &mut Vec<Segment>,
    placed_boxes: &mut Vec<BBox>,
) {
    let group = &groups[idx];
    if let Some(group_edges) = edges.get(&idx) {
        for e in group_edges {
            if !placed.contains(&e.other_group) || !packed_set.contains(&e.other_group) {
                continue;
            }
            let a = group.final_cell(e.room_id);
            let b = groups[e.other_group].final_cell(e.other_room_id);
            if chebyshev(a, b) > CONNECTOR_COMMIT_CAP {
                continue;
            }
            placed_segments.push(Segment {
                a,
                b,
                ra: e.room_id,
                rb: e.other_room_id,
            });
        }
    }

    let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
    for &room_id in &group.room_ids {
        let Some(room) = map.room(room_id) else {
            continue;
        };
        for exit in &room.exits {
            let target_id = exit.to;
            if !group.positions.contains_key(&target_id) {
                continue;
            }
            let key = (room_id.min(target_id), room_id.max(target_id));
            if seen.contains(&key) {
                continue;
            }
            if dirs.get(room_id, target_id).is_none() {
                continue;
            }
            seen.insert(key);
            let a = group.final_cell(room_id);
            let b = group.final_cell(target_id);
            if chebyshev(a, b) > DIRECTIONAL_COMMIT_CAP {
                continue;
            }
            placed_segments.push(Segment {
                a,
                b,
                ra: room_id,
                rb: target_id,
            });
        }
    }

    let bounds = group.bounds();
    let off = groups[idx]
        .base_offset
        .unwrap_or_else(|| unreachable!("group was just placed"));
    placed_boxes.push(BBox {
        min_x: bounds.min_x + off.x,
        max_x: bounds.max_x + off.x,
        min_y: bounds.min_y + off.y,
        max_y: bounds.max_y + off.y,
    });
}

/// Connector edges: any exit between rooms of different packed components.
/// These carry no direction but prove adjacency.
pub(crate) fn collect_connector_edges(
    groups: &[Group],
    packed: &[usize],
    map: &Map,
) -> HashMap<usize, Vec<Edge>> {
    let mut component_of: HashMap<RoomId, usize> = HashMap::new();
    for &idx in packed {
        for &id in &groups[idx].room_ids {
            component_of.insert(id, idx);
        }
    }

    let mut edges: HashMap<usize, Vec<Edge>> = HashMap::new();
    for &idx in packed {
        for &room_id in &groups[idx].room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                let target_id = exit.to;
                let Some(&other) = component_of.get(&target_id) else {
                    continue;
                };
                if other == idx {
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
    edges
}

/// A packed-side room touching an excluded (interior) component, and which
/// packed group it belongs to. Module scope: [`add_bridged_edges`] collects
/// these per excluded component before turning contact pairs into bridged
/// edges.
#[derive(Clone, Copy, PartialEq)]
struct Contact {
    group: usize,
    room_id: RoomId,
}

/// Virtual edges between packed groups whose only link runs through an
/// excluded (interior) component -- e.g. two shores of a ferry interior.
pub(crate) fn add_bridged_edges(
    edges: &mut HashMap<usize, Vec<Edge>>,
    groups: &[Group],
    packed_set: &HashSet<usize>,
    map: &Map,
) {
    let mut component_of_all: HashMap<RoomId, usize> = HashMap::new();
    for group in groups {
        for &id in &group.room_ids {
            component_of_all.insert(id, group.index);
        }
    }

    // Excluded component -> packed-side contacts, in first-encounter order.
    let mut contact_order: Vec<usize> = Vec::new();
    let mut contacts: HashMap<usize, Vec<Contact>> = HashMap::new();
    let add_contact = |excluded: usize,
                       packed_idx: usize,
                       room_id: RoomId,
                       contact_order: &mut Vec<usize>,
                       contacts: &mut HashMap<usize, Vec<Contact>>| {
        let list = contacts.entry(excluded).or_insert_with(|| {
            contact_order.push(excluded);
            Vec::new()
        });
        let c = Contact {
            group: packed_idx,
            room_id,
        };
        if !list.contains(&c) {
            list.push(c);
        }
    };

    for group in groups {
        for &room_id in &group.room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                let target_id = exit.to;
                let Some(&target_group) = component_of_all.get(&target_id) else {
                    continue;
                };
                if target_group == group.index {
                    continue;
                }
                let group_packed = packed_set.contains(&group.index);
                let target_packed = packed_set.contains(&target_group);
                if group_packed && !target_packed {
                    add_contact(
                        target_group,
                        group.index,
                        room_id,
                        &mut contact_order,
                        &mut contacts,
                    );
                } else if !group_packed && target_packed {
                    add_contact(
                        group.index,
                        target_group,
                        target_id,
                        &mut contact_order,
                        &mut contacts,
                    );
                }
            }
        }
    }

    for excluded in contact_order {
        let list = &contacts[&excluded];
        let cap = list.len().min(BRIDGED_CONTACT_CAP);
        for i in 0..cap {
            for j in (i + 1)..cap {
                if list[i].group == list[j].group {
                    continue;
                }
                let delta = uid_delta(map.room(list[i].room_id), map.room(list[j].room_id));
                edges.entry(list[i].group).or_default().push(Edge {
                    other_group: list[j].group,
                    room_id: list[i].room_id,
                    other_room_id: list[j].room_id,
                    uid_delta: delta,
                });
                edges.entry(list[j].group).or_default().push(Edge {
                    other_group: list[i].group,
                    room_id: list[j].room_id,
                    other_room_id: list[i].room_id,
                    uid_delta: delta,
                });
            }
        }
    }
}

/// The geographic base map is whatever image anchors the largest component.
/// Raw per-room counts can be fooled by collage overlays.
pub(crate) fn find_primary_image(
    groups: &[Group],
    packed: &[usize],
    anchors: &HashMap<usize, Vec<Anchor>>,
) -> Option<String> {
    let mut largest: Option<usize> = None;
    let mut largest_rooms = 0usize;
    for &idx in packed {
        if anchors.get(&idx).map_or(0, Vec::len) == 0 {
            continue;
        }
        let n = groups[idx].room_ids.len();
        if n > largest_rooms {
            largest_rooms = n;
            largest = Some(idx);
        }
    }
    let largest = largest?;

    let mut counts: Vec<(&str, usize)> = Vec::new();
    for a in &anchors[&largest] {
        if let Some(entry) = counts.iter_mut().find(|(img, _)| *img == a.image) {
            entry.1 += 1;
        } else {
            counts.push((&a.image, 1));
        }
    }
    let mut best: Option<&str> = None;
    let mut best_count = 0usize;
    for (image, count) in counts {
        if count > best_count {
            best_count = count;
            best = Some(image);
        }
    }
    best.map(str::to_owned)
}

/// Pixels per grid cell: median ratio of pixel delta to grid delta over
/// anchored room pairs WITHIN components (grid positions are
/// solver-trusted, pixel positions cartographer-trusted).
pub(crate) fn estimate_scale(
    groups: &[Group],
    packed: &[usize],
    anchors: &HashMap<usize, Vec<Anchor>>,
    primary_image: Option<&str>,
) -> f64 {
    let Some(primary) = primary_image else {
        return DEFAULT_SCALE;
    };
    let mut ratios: Vec<f64> = Vec::new();

    for &idx in packed {
        let list: Vec<&Anchor> = anchors
            .get(&idx)
            .map(|l| l.iter().filter(|a| a.image == primary).collect())
            .unwrap_or_default();
        if list.len() < 2 {
            continue;
        }
        let limit = list.len().min(ANCHOR_PAIR_CAP);
        for i in 0..limit {
            for j in (i + 1)..limit {
                let pa = groups[idx].positions[&list[i].room_id];
                let pb = groups[idx].positions[&list[j].room_id];
                let cells_across = (pa.x - pb.x).abs();
                let cells_down = (pa.y - pb.y).abs();
                let pixels_across = (list[i].px - list[j].px).abs();
                let pixels_down = (list[i].py - list[j].py).abs();
                if (GRID_DELTA_MIN..=GRID_DELTA_MAX).contains(&cells_across) {
                    ratios.push(pixels_across / f64::from(cells_across));
                }
                if (GRID_DELTA_MIN..=GRID_DELTA_MAX).contains(&cells_down) {
                    ratios.push(pixels_down / f64::from(cells_down));
                }
            }
        }
    }

    if ratios.is_empty() {
        return DEFAULT_SCALE;
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = ratios[ratios.len() / 2];
    median.clamp(SCALE_MIN, SCALE_MAX)
}

/// Bounds of the occupied set as the reference computes them: minX and maxY
/// only, both biased toward 0 by their initial values.
pub(crate) fn occupied_bounds(occupied: &HashSet<Cell>) -> (i32, i32) {
    let mut min_x = 0;
    let mut max_y = 0;
    for c in occupied {
        if c.x < min_x {
            min_x = c.x;
        }
        if c.y > max_y {
            max_y = c.y;
        }
    }
    (min_x, max_y)
}

/// Connector-aware placement: among collision-free offsets near the
/// proposed one, prefer short connector lines that cross as few existing
/// lines as possible and avoid other groups' bounding boxes. Explores two
/// rings past the nearest fit so a clean spot can beat a marginally closer
/// tangled one.
pub(crate) fn find_best_connector_offset(
    group: &Group,
    proposed: Cell,
    occupied: &HashSet<Cell>,
    anchor_lines: &[AnchorLine],
    placed_segments: &[Segment],
    placed_boxes: &[BBox],
) -> Option<(Cell, i64)> {
    let reach = SEARCH_RADIUS + 4;
    let mut win_min_x = proposed.x;
    let mut win_max_x = proposed.x;
    let mut win_min_y = proposed.y;
    let mut win_max_y = proposed.y;
    for a in anchor_lines {
        win_min_x = win_min_x.min(a.target.x);
        win_max_x = win_max_x.max(a.target.x);
        win_min_y = win_min_y.min(a.target.y);
        win_max_y = win_max_y.max(a.target.y);
    }
    let local_segments: Vec<&Segment> = placed_segments
        .iter()
        .filter(|seg| {
            seg.a.x.max(seg.b.x) >= win_min_x - reach
                && seg.a.x.min(seg.b.x) <= win_max_x + reach
                && seg.a.y.max(seg.b.y) >= win_min_y - reach
                && seg.a.y.min(seg.b.y) <= win_max_y + reach
        })
        .collect();
    let local_boxes: Vec<&BBox> = placed_boxes
        .iter()
        .filter(|b| {
            b.max_x >= win_min_x - reach
                && b.min_x <= win_max_x + reach
                && b.max_y >= win_min_y - reach
                && b.min_y <= win_max_y + reach
        })
        .collect();

    let group_cells: Vec<Cell> = group.positions.values().copied().collect();
    let mut best: Option<Cell> = None;
    let mut best_score = i64::MAX;

    let consider = |candidate: Cell, best: &mut Option<Cell>, best_score: &mut i64| {
        if !fits(group, candidate, occupied) {
            return;
        }
        let mut score = 0i64;
        for anchor in anchor_lines {
            let endpoint = Cell {
                x: anchor.internal.x + candidate.x,
                y: anchor.internal.y + candidate.y,
            };
            score += i64::from(chebyshev(endpoint, anchor.target));
            for seg in &local_segments {
                if seg.ra == anchor.room_id
                    || seg.rb == anchor.room_id
                    || seg.ra == anchor.other_room_id
                    || seg.rb == anchor.other_room_id
                {
                    continue;
                }
                if segments_cross(endpoint, anchor.target, seg.a, seg.b) {
                    score += CROSSING_PENALTY;
                }
            }
        }
        // Discourage landing inside another group's footprint (courtyards).
        for cell in &group_cells {
            let cx = cell.x + candidate.x;
            let cy = cell.y + candidate.y;
            for b in &local_boxes {
                if cx >= b.min_x && cx <= b.max_x && cy >= b.min_y && cy <= b.max_y {
                    score += COURTYARD_PENALTY;
                    break;
                }
            }
        }
        if score < *best_score {
            *best_score = score;
            *best = Some(candidate);
        }
    };

    consider(proposed, &mut best, &mut best_score);
    let mut first_fit_radius: Option<i32> = None;
    for r in 1..=SEARCH_RADIUS {
        if best.is_some() && first_fit_radius.is_none() {
            first_fit_radius = Some(r - 1);
        }
        if let Some(f) = first_fit_radius
            && r > f + 2
        {
            break;
        }
        for_ring(proposed, r, |cand| {
            consider(cand, &mut best, &mut best_score);
        });
    }
    best.map(|cell| (cell, best_score))
}
