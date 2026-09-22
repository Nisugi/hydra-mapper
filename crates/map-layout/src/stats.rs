//! Statistical summary of a generated layout, the same shape Vellum's
//! fixture export uses (`docs/layout-fixtures.json`), so the porting
//! ratchet (`plan/26` §5 step 4) can diff against real zone numbers.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use cena_map::{Map, RoomId};

use crate::pipeline::Layout;
use crate::positioner::Cell;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutStats {
    pub rooms: usize,
    pub components: usize,
    pub outdoor_components: usize,
    pub outdoor_rooms: usize,
    pub interior_components: usize,
    pub interior_rooms: usize,
    pub direction_violations: usize,
    pub entrance_rooms: usize,
    pub cell_overlaps: usize,
    pub inter_group_connectors: usize,
    pub connector_len_median: Option<i32>,
    pub connector_len_p90: Option<i32>,
    pub pack_methods: BTreeMap<String, usize>,
    pub primary_image: Option<String>,
}

impl LayoutStats {
    #[must_use]
    pub fn compute(layout: &Layout, map: &Map) -> LayoutStats {
        // Final outdoor-sheet cells and component membership per room.
        let mut fin: HashMap<RoomId, Cell> = HashMap::new();
        let mut comp_of: HashMap<RoomId, usize> = HashMap::new();
        for &idx in &layout.outdoor {
            let group = &layout.groups[idx];
            for &id in &group.room_ids {
                fin.insert(id, group.final_cell(id));
                comp_of.insert(id, idx);
            }
        }

        let mut cells: HashSet<Cell> = HashSet::new();
        let mut overlaps = 0;
        for c in fin.values() {
            if !cells.insert(*c) {
                overlaps += 1;
            }
        }

        // Inter-group connectors: cross-component exits on the outdoor
        // sheet, deduped by unordered pair, measured in Chebyshev cells.
        let mut connector_lens: Vec<i32> = Vec::new();
        let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
        for room in map.rooms() {
            for exit in &room.exits {
                let target = exit.to;
                let (Some(&a), Some(&b)) = (fin.get(&room.id), fin.get(&target)) else {
                    continue;
                };
                if comp_of[&room.id] == comp_of[&target] {
                    continue;
                }
                let key = (room.id.min(target), room.id.max(target));
                if !seen.insert(key) {
                    continue;
                }
                connector_lens.push((a.x - b.x).abs().max((a.y - b.y).abs()));
            }
        }
        connector_lens.sort_unstable();
        let quantile = |f: f64| -> Option<i32> {
            if connector_lens.is_empty() {
                return None;
            }
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let idx = (connector_lens.len() as f64 * f).floor() as usize;
            Some(
                connector_lens
                    .get(idx)
                    .copied()
                    .unwrap_or(*connector_lens.last().unwrap_or(&0)),
            )
        };

        LayoutStats {
            rooms: map.rooms().len(),
            components: layout.groups.len(),
            outdoor_components: layout.outdoor.len(),
            outdoor_rooms: layout
                .outdoor
                .iter()
                .map(|&i| layout.groups[i].room_ids.len())
                .sum(),
            interior_components: layout.interiors.len(),
            interior_rooms: layout
                .interiors
                .iter()
                .map(|&i| layout.groups[i].room_ids.len())
                .sum(),
            direction_violations: layout.groups.iter().map(|g| g.violations.len()).sum(),
            entrance_rooms: layout.classification.entrance_room_ids.len(),
            cell_overlaps: overlaps,
            inter_group_connectors: connector_lens.len(),
            connector_len_median: quantile(0.5),
            connector_len_p90: quantile(0.9),
            pack_methods: layout.pack_info.methods.clone(),
            primary_image: layout.pack_info.primary_image.clone(),
        }
    }
}
