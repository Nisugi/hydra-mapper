//! The pipeline itself: direction analysis -> BFS placement -> hill-climb +
//! compaction -> interior classification -> cluster packing + interior
//! shelf. `lib.rs` stays a facade (`plan/05` Rule 4.4); this is where the
//! steps documented there actually run.

use serde::{Deserialize, Serialize};

use cena_map::Map;

use crate::classifier::Classification;
use crate::overrides::EdgeOverride;
use crate::packer::PackInfo;
use crate::positioner::Group;
use crate::{classifier, direction, interior_shelf, outdoor_packing, positioner, regions};

/// A generated layout: every component with internal positions and sheet
/// offsets, plus the interior/outdoor split and packing debug info.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub groups: Vec<Group>,
    /// Indices (into `groups`) packed onto the shared outdoor sheet.
    pub outdoor: Vec<usize>,
    /// Indices placed on the separate interiors shelf sheet.
    pub interiors: Vec<usize>,
    pub classification: Classification,
    pub pack_info: PackInfo,
    /// The edge corrections this layout was solved with, so whatever
    /// draws it reads the same directions the solver placed by: a
    /// forced bearing draws as a line, an un-welded edge as a connector.
    #[serde(default)]
    pub edges: Vec<EdgeOverride>,
    /// Sheet cells per outdoor solver cell this layout was built at
    /// ([`LayoutParams::town_scale`]), so the scene draws it at the same.
    #[serde(default = "default_town_scale")]
    pub town_scale: i32,
}

/// The knobs a layout is built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutParams {
    /// Sheet cells per outdoor solver cell: the streets are laid at this
    /// scale, leaving `town_scale - 1` free cells between neighbouring
    /// street rooms for the buildings. Clamped to at least 1.
    pub town_scale: i32,
}

impl Default for LayoutParams {
    fn default() -> LayoutParams {
        LayoutParams {
            town_scale: interior_shelf::TOWN_SCALE,
        }
    }
}

fn default_town_scale() -> i32 {
    interior_shelf::TOWN_SCALE
}

/// Run the full pipeline over one location's rooms, already filtered to
/// that location (`map` may hold rooms from elsewhere; nothing here checks
/// `Room::location` -- the caller decides the selection, same contract as
/// Vellum's `generate_layout`).
#[must_use]
pub fn generate_layout(map: &Map) -> Layout {
    generate_layout_impl(map, &[], LayoutParams::default())
}

/// As [`generate_layout`], with curated edge corrections applied before
/// positioning, so the rooms are laid out *by* the corrected geometry
/// rather than nudged afterwards.
#[must_use]
pub fn generate_layout_with(map: &Map, edges: &[EdgeOverride]) -> Layout {
    generate_layout_impl(map, edges, LayoutParams::default())
}

/// As [`generate_layout_with`], with the layout's knobs set.
#[must_use]
pub fn generate_layout_tuned(map: &Map, edges: &[EdgeOverride], params: LayoutParams) -> Layout {
    generate_layout_impl(map, edges, params)
}

/// The pipeline as the reference runs it. Exists so fixture parity tests
/// keep validating the ported core against reference-exported numbers;
/// the two are the same pipeline now that every building shelves beside
/// its street rather than some being seated among the streets.
#[must_use]
pub fn generate_layout_reference(map: &Map) -> Layout {
    generate_layout_impl(map, &[], LayoutParams::default())
}

fn generate_layout_impl(map: &Map, edges: &[EdgeOverride], params: LayoutParams) -> Layout {
    let town_scale = params.town_scale.max(1);
    // A removed room and an urchin hideout are not places, whoever chose
    // the selection: they get no cell, and exits into them are exits out
    // of the selection. (Rooms nothing reaches need the whole map to see;
    // that is `regions::placeable_rooms`, the caller's filter.)
    let real;
    let map = if map.rooms().iter().all(regions::is_real_room) {
        map
    } else {
        let rooms = map
            .rooms()
            .iter()
            .filter(|r| regions::is_real_room(r))
            .cloned()
            .collect();
        real = Map::from_rooms(rooms).unwrap_or_else(|_| unreachable!("a subset of unique ids"));
        &real
    };

    let mut dirs = direction::DirectionMap::build(map);
    // Before positioning: a correction is an input to the solve, not a
    // patch on its result.
    dirs.apply_edge_overrides(map, edges);
    let dirs = dirs;

    let mut groups = positioner::position_rooms(map, &dirs);
    let classification = classifier::classify(&groups, map);

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

    // After the outdoor pass, which is what gives the doorway rooms the
    // cells the shelf is ordered by.
    interior_shelf::pack_interior_shelf(
        &mut groups,
        &interiors,
        &clusters,
        map,
        &classification.entrances,
        &outdoor,
        town_scale,
    );

    // Building names for interior groups (assigned after shelf packing so
    // the shelf order matches the reference, which sorts unnamed groups).
    // Inlined buildings keep theirs too -- the scene labels them outdoors.
    for &idx in &interiors {
        groups[idx].name = classifier::building_name(&groups[idx], map);
    }

    Layout {
        groups,
        outdoor,
        interiors,
        classification,
        pack_info,
        edges: edges.to_vec(),
        town_scale,
    }
}
