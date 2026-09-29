//! The author, 2026-09-29: *"If I'm in the middle of a town and I need a
//! long yellow line to go to an island group, then should that group be
//! it's own "area" and get the dot for a building treatment?"*, and *"We
//! can try 3 steps"* ([`STEPS`]).
//!
//! After the outdoor sheet is packed, a group joined to the rest only by
//! lines with no direction, the nearest of them longer than the steps set,
//! is cut: it goes to its own sheet as a building does, and each of its
//! links draws as a mark at both ends rather than a line across the town.
//! The area's biggest group is never cut, and none is cut that can be put
//! down beside its link ([`pull_in`]): Hinterwilds' Fjallarhaart was cut
//! where the packer had left it, with the ground beside its arch empty
//! (the author, 2026-09-29: *"There's literally nothing in the way of it
//! going and attaching to the room above"*).

use std::collections::{HashMap, HashSet};

use cena_map::Map;

use crate::direction::DirectionMap;
use crate::packer::{Edge, GROUP_PADDING, Segment, chebyshev, collect_connector_edges};
use crate::positioner::{Cell, Group, PackMethod};
use crate::stretch::{anchors_of, beside, lines_of};

/// Lay the `cut` groups in a row below every room of `placed`, left to
/// right, [`GROUP_PADDING`] cells apart.
pub(crate) fn below(groups: &mut [Group], placed: &[usize], cut: &[usize]) {
    let cells: Vec<Cell> = placed
        .iter()
        .filter(|&&i| groups[i].base_offset.is_some())
        .flat_map(|&i| {
            let g = &groups[i];
            g.room_ids.iter().map(move |&r| g.final_cell(r))
        })
        .collect();
    let bottom = cells.iter().map(|c| c.y).max().unwrap_or(0);
    let mut x = cells.iter().map(|c| c.x).min().unwrap_or(0);
    for &c in cut {
        let b = groups[c].bounds();
        groups[c].base_offset = Some(Cell {
            x: x - b.min_x,
            y: bottom + GROUP_PADDING - b.min_y,
        });
        groups[c].packing = Some(PackMethod::Strip);
        x += b.max_x - b.min_x + 1 + GROUP_PADDING;
    }
}

/// How far, in steps, a group's nearest link may run before the group is
/// cut. On gs.map, with the rest of the pipeline as it is, directionless
/// lines crossing another 202 against 263 with no cut at all.
pub(crate) const STEPS: i32 = 3;

/// Each group that [`islands`] would cut, put down beside its links where
/// it can be ([`beside`]): within `steps` of one of them, clear of
/// everything placed, its links bending round what is in their way.
pub(crate) fn pull_in(
    groups: &mut [Group],
    outdoor: &[usize],
    map: &Map,
    dirs: &DirectionMap,
    steps: i32,
) {
    let edges = collect_connector_edges(groups, outdoor, map);
    let placed: Vec<usize> = outdoor
        .iter()
        .copied()
        .filter(|&i| groups[i].base_offset.is_some())
        .collect();
    let mut occupied: HashSet<Cell> = HashSet::new();
    let mut segments: Vec<Segment> = Vec::new();
    for &i in &placed {
        occupied.extend(groups[i].room_ids.iter().map(|&r| groups[i].final_cell(r)));
        lines_of(
            &groups[i],
            &anchors_of(groups, i, &edges),
            map,
            &mut segments,
        );
    }
    for idx in candidates(groups, outdoor, &edges, dirs, steps) {
        let anchors = anchors_of(groups, idx, &edges);
        let mine: HashSet<cena_map::RoomId> = groups[idx].room_ids.iter().copied().collect();
        let cells: Vec<Cell> = groups[idx]
            .room_ids
            .iter()
            .map(|&r| groups[idx].final_cell(r))
            .collect();
        for c in &cells {
            occupied.remove(c);
        }
        let others: Vec<Segment> = segments
            .iter()
            .filter(|s| !mine.contains(&s.ra) && !mine.contains(&s.rb))
            .copied()
            .collect();
        if let Some(at) = beside(&groups[idx], &anchors, &occupied, &others, dirs, steps) {
            groups[idx].base_offset = Some(at);
            segments = others;
            lines_of(&groups[idx], &anchors, map, &mut segments);
            occupied.extend(
                groups[idx]
                    .room_ids
                    .iter()
                    .map(|&r| groups[idx].final_cell(r)),
            );
        } else {
            occupied.extend(cells);
        }
    }
}

/// The packed groups of `outdoor` to cut, by the rule above.
pub(crate) fn islands(
    groups: &[Group],
    outdoor: &[usize],
    map: &Map,
    dirs: &DirectionMap,
    steps: i32,
) -> HashSet<usize> {
    let edges = collect_connector_edges(groups, outdoor, map);
    candidates(groups, outdoor, &edges, dirs, steps)
        .into_iter()
        .collect()
}

/// The placed groups of `outdoor`, but the biggest, whose links all have no
/// direction and the nearest of them runs more than `steps`.
fn candidates(
    groups: &[Group],
    outdoor: &[usize],
    edges: &HashMap<usize, Vec<Edge>>,
    dirs: &DirectionMap,
    steps: i32,
) -> Vec<usize> {
    let Some(&biggest) = outdoor.iter().max_by_key(|&&i| groups[i].room_ids.len()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for &idx in outdoor {
        if idx == biggest || groups[idx].base_offset.is_none() {
            continue;
        }
        let Some(links) = edges.get(&idx) else {
            continue;
        };
        let bearing = links.iter().any(|e| {
            dirs.get(e.room_id, e.other_room_id).is_some()
                || dirs.get(e.other_room_id, e.room_id).is_some()
        });
        if bearing {
            continue;
        }
        let nearest = links
            .iter()
            .filter(|e| groups[e.other_group].base_offset.is_some())
            .map(|e| {
                chebyshev(
                    groups[idx].final_cell(e.room_id),
                    groups[e.other_group].final_cell(e.other_room_id),
                )
            })
            .min();
        if nearest.is_some_and(|n| n > steps) {
            out.push(idx);
        }
    }
    out
}
