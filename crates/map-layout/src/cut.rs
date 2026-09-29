//! The author, 2026-09-29: *"If I'm in the middle of a town and I need a
//! long yellow line to go to an island group, then should that group be
//! it's own "area" and get the dot for a building treatment?"*, and *"We
//! can try 3 steps"* ([`STEPS`]).
//!
//! After the outdoor sheet is packed, a group joined to the rest only by
//! lines with no direction, the nearest of them longer than the steps set,
//! is cut: it goes to its own sheet as a building does, and each of its
//! links draws as a mark at both ends rather than a line across the town.
//! The area's biggest group is never cut.

use std::collections::HashSet;

use cena_map::Map;

use crate::direction::DirectionMap;
use crate::packer::{GROUP_PADDING, chebyshev, collect_connector_edges};
use crate::positioner::{Cell, Group, PackMethod};

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

/// The packed groups of `outdoor` to cut, by the rule above.
pub(crate) fn islands(
    groups: &[Group],
    outdoor: &[usize],
    map: &Map,
    dirs: &DirectionMap,
    steps: i32,
) -> HashSet<usize> {
    let Some(&biggest) = outdoor.iter().max_by_key(|&&i| groups[i].room_ids.len()) else {
        return HashSet::new();
    };
    let edges = collect_connector_edges(groups, outdoor, map);
    let mut cut = HashSet::new();
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
            cut.insert(idx);
        }
    }
    cut
}
