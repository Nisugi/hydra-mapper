//! The pipeline itself: direction analysis -> BFS placement -> hill-climb +
//! compaction -> interior classification -> cluster packing + interior
//! shelf. `lib.rs` stays a facade (`plan/05` Rule 4.4); this is where the
//! steps documented there actually run.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use cena_map::Map;

use crate::classifier::Classification;
use crate::overrides::EdgeOverride;
use crate::packer::PackInfo;
use crate::positioner::Group;
use crate::{classifier, direction, interior_shelf, outdoor_packing, positioner};

/// A generated layout: every component with internal positions and sheet
/// offsets, plus the interior/outdoor split and packing debug info.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub groups: Vec<Group>,
    /// Indices (into `groups`) packed onto the shared outdoor sheet.
    pub outdoor: Vec<usize>,
    /// Indices placed on the separate interiors shelf sheet.
    pub interiors: Vec<usize>,
    /// Interior groups the try-inline pass seated on the outdoor sheet
    /// (subset of `outdoor`); they keep their building names so the scene
    /// can label them.
    #[serde(default)]
    pub inlined: Vec<usize>,
    pub classification: Classification,
    pub pack_info: PackInfo,
}

/// Run the full pipeline over one location's rooms, already filtered to
/// that location (`map` may hold rooms from elsewhere; nothing here checks
/// `Room::location` -- the caller decides the selection, same contract as
/// Vellum's `generate_layout`).
#[must_use]
pub fn generate_layout(map: &Map) -> Layout {
    generate_layout_impl(map, true, &[])
}

/// As [`generate_layout`], with curated edge corrections applied before
/// positioning, so the rooms are laid out *by* the corrected geometry
/// rather than nudged afterwards.
#[must_use]
pub fn generate_layout_with(map: &Map, edges: &[EdgeOverride]) -> Layout {
    generate_layout_impl(map, true, edges)
}

/// The pipeline exactly as the reference runs it -- no try-inline pass.
/// Exists so fixture parity tests keep validating the ported core against
/// reference-exported numbers; production always inlines.
#[must_use]
pub fn generate_layout_reference(map: &Map) -> Layout {
    generate_layout_impl(map, false, &[])
}

fn generate_layout_impl(map: &Map, inline_interiors: bool, edges: &[EdgeOverride]) -> Layout {
    let mut dirs = direction::DirectionMap::build(map);
    // Before positioning: a correction is an input to the solve, not a
    // patch on its result.
    dirs.apply_edge_overrides(map, edges);
    let dirs = dirs;

    let mut groups = positioner::position_rooms(map, &dirs);
    let mut classification = classifier::classify(&groups, map);

    let mut outdoor: Vec<usize> = groups
        .iter()
        .map(|g| g.index)
        .filter(|i| !classification.interior_groups.contains(i))
        .collect();
    let mut interiors: Vec<usize> = groups
        .iter()
        .map(|g| g.index)
        .filter(|i| classification.interior_groups.contains(i))
        .collect();
    // A selection that is entirely interiors skips the split entirely.
    if outdoor.is_empty() {
        outdoor = groups.iter().map(|g| g.index).collect();
        interiors.clear();
    }

    let pack_info = outdoor_packing::pack_groups(&mut groups, &outdoor, map, &dirs);
    let clusters = classifier::interior_clusters(&groups, &classification.interior_groups, map);

    // Try-inline pass: an interior building that seats cleanly beside its
    // doorway -- short connectors, no crossings, free ground -- joins the
    // outdoor sheet, so lone caves and grottos stay discoverable. Dense
    // shop districts fail the budget and keep the shelf. No curated
    // Interior flips exist yet to exempt (`plan/26` §0), so `forced_shelf`
    // is always empty.
    let mut inlined: Vec<usize> = Vec::new();
    if inline_interiors {
        let forced_shelf: HashSet<usize> = HashSet::new();
        inlined = interior_shelf::inline_interior_clusters(
            &mut groups,
            &outdoor,
            &interiors,
            &clusters,
            &classification.entrances,
            &forced_shelf,
            map,
            &dirs,
        );
        if !inlined.is_empty() {
            let inlined_set: HashSet<usize> = inlined.iter().copied().collect();
            interiors.retain(|idx| !inlined_set.contains(idx));
            outdoor.extend(inlined.iter().copied());
            for &idx in &inlined {
                classification.interior_groups.remove(&idx);
            }
            classifier::recompute_entrances(&mut classification, &groups, map);
        }
    }

    interior_shelf::pack_interior_shelf(&mut groups, &interiors, &clusters, map);

    // Building names for interior groups (assigned after shelf packing so
    // the shelf order matches the reference, which sorts unnamed groups).
    // Inlined buildings keep theirs too -- the scene labels them outdoors.
    for &idx in interiors.iter().chain(&inlined) {
        groups[idx].name = classifier::building_name(&groups[idx], map);
    }

    Layout {
        groups,
        outdoor,
        interiors,
        inlined,
        classification,
        pack_info,
    }
}
