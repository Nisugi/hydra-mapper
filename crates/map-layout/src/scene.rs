//! Presentation model (spec §8): the drawable form of a generated layout.
//! Ported from `reference/VellumFE/src/core/layout_engine/scene.rs`.
//!
//! Pure data -- no rendering toolkit types -- so `cena-mapper`'s window and
//! any later embedder both draw from the same scene. Rooms carry final
//! sheet cells; edges are pre-classified: solid directional edges, stubs
//! for directional edges stretched past `LONG_EDGE_CELLS`, and dashed
//! labeled connectors (skipped past `CONNECTOR_MAX_CELLS`).
//!
//! **One sheet, and a focus.** Every room of an area has one cell, in one
//! frame: the streets at [`OUTDOOR_SCALE`], each building hung beside the
//! street it opens off. What a renderer draws as squares is its choice --
//! a [`Unit`]: the streets, or one building -- and everything else is
//! drawn as dots on the same roads, so the area reads as one continuous
//! map whichever part of it is in focus. Nothing is laid out twice and
//! nothing is echoed; changing focus moves no room.
//!
//! **v1 has no override source** (`plan/26` §0: no editor yet), so the edge
//! restyling Vellum's `build_scene` takes (`Hide`/`Dash`/`Dots`/`Connector`
//! overrides) is not ported: every edge draws as the solver and classifier
//! decided. The seam is `crate::overrides`, not this module, when that
//! system exists.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use cena_map::{Map, RoomId};

use crate::Layout;
use crate::classifier::{building_name, interior_clusters};
use crate::direction::{Dir, DirectionMap};
use crate::positioner::Cell;

/// Directional edges longer than this render as stubs, not lines.
pub const LONG_EDGE_CELLS: i32 = 8;
/// Connectors longer than this are not drawn at all.
pub const CONNECTOR_MAX_CELLS: i32 = 30;

/// Outdoor groups are drawn at this many cells per solver cell. The
/// interiors are laid out in that frame already -- each building hung
/// beside its street at [`crate::interior_shelf::TOWN_SCALE`] -- so
/// scaling the streets to match puts every room of the area on one
/// sheet, in one frame, with the buildings in the gaps between the
/// streets. Anything that turns a drawn distance on an outdoor group
/// back into solver cells -- a drag -- divides by this.
pub const OUTDOOR_SCALE: i32 = crate::interior_shelf::TOWN_SCALE;

/// Room tags that mark a service worth a map marker (`;go2` destinations).
/// Everything else on a room's tag list (forage names, `meta:` tags) is not
/// marker material. Kept local to this crate rather than shared with
/// `cena_behavior::travel::itinerary::PLACES`: layering runs one way
/// (`cena-behavior` depends on `cena-map`, never the reverse), and this is
/// a presentation concern -- what earns a marker -- distinct from travel's
/// routing targets.
pub const SERVICE_TAGS: &[&str] = &[
    "advguard",
    "advguard2",
    "advguild",
    "advpickup",
    "alchemist",
    "armorshop",
    "bakery",
    "bank",
    "bardguild",
    "boutique",
    "chronomage",
    "clericguild",
    "clericshop",
    "cobbling",
    "collectibles",
    "consignment",
    "empathguild",
    "exchange",
    "fletcher",
    "forge",
    "furrier",
    "gemshop",
    "general store",
    "grocer",
    "herbalist",
    "inn",
    "locksmith",
    "mail",
    "movers",
    "npccleric",
    "npchealer",
    "pawnshop",
    "portmaster",
    "postoffice",
    "rangerguild",
    "smokeshop",
    "sorcererguild",
    "sunfist",
    "town",
    "treasuremaster",
];

/// One drawable room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneRoom {
    pub id: RoomId,
    pub uid: Option<i64>,
    /// Its one cell on the sheet.
    pub cell: Cell,
    pub group: usize,
    /// The unit this room belongs to first: its building, or the streets.
    /// Index into [`MapScene::units`].
    pub unit: usize,
    /// Outdoor room hosting a doorway into an interior (gets a door
    /// marker).
    pub entrance: bool,
    /// First room title, for hover text.
    pub title: String,
    /// mapdb terrain string ("deciduous forest", "hard, flat", ...) for
    /// terrain-tinted room fills. None/empty = theme fill.
    #[serde(default)]
    pub terrain: Option<String>,
    /// Service tags this room carries (intersection of the room's tags
    /// with [`SERVICE_TAGS`]) -- marker material.
    #[serde(default)]
    pub service_tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SceneEdgeKind {
    /// Solid line: a compass-true edge within a group.
    Directional,
    /// Stretched directional edge: draw short dashed arrows at both ends,
    /// each labeled with the partner's room id, instead of a long line.
    Stub,
    /// Dashed inter-group connector ("go door" adjacency, no direction).
    Connector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEdge {
    pub a: Cell,
    pub b: Cell,
    pub a_room: RoomId,
    pub b_room: RoomId,
    /// Group of the `a` endpoint.
    pub group: usize,
    pub kind: SceneEdgeKind,
    /// Movement label for connectors ("dock", "gate") when one is worth
    /// showing.
    pub label: Option<String>,
    /// The building this edge is inside, if it is inside one: both ends in
    /// the same building unit. `None` for a road, and for a door -- an
    /// edge between units -- which a renderer draws whatever is in focus,
    /// where a building's inside is drawn only when the building is.
    #[serde(default)]
    pub unit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupLabel {
    pub text: String,
    /// Top-left cell of the group's bounds; render above it.
    pub cell: Cell,
    pub group: usize,
    /// The unit the label names, when it names one.
    #[serde(default)]
    pub unit: Option<usize>,
}

/// The sheet: every room of the area, every drawable edge, and the labels.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SheetScene {
    pub rooms: Vec<SceneRoom>,
    pub edges: Vec<SceneEdge>,
    pub labels: Vec<GroupLabel>,
    pub min: Cell,
    pub max: Cell,
}

/// What a set of rooms is, for a renderer choosing what to put in focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitKind {
    /// The outdoor rooms: the streets and the country between them.
    Streets,
    /// One walkable building -- an interior cluster.
    Building,
}

/// A set of rooms a renderer can put in focus: drawn as squares, with
/// everything outside it as dots. The streets are unit 0; every building
/// is a unit of its own. A room is in exactly one of these; a renderer
/// with other groupings (a hunting area, a plate) layers them on top.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub kind: UnitKind,
    /// The building's name, or "Streets".
    pub name: String,
    /// Its rooms, in id order.
    pub rooms: Vec<RoomId>,
    /// The rooms a door leads into from outside the unit -- a building's
    /// door rooms. Drawn larger than the other dots when the unit is out
    /// of focus, so the way in is visible. Empty for the streets.
    pub door_rooms: Vec<RoomId>,
}

/// The unit index of the streets, always present.
pub const STREETS: usize = 0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MapScene {
    pub location: String,
    pub sheet: SheetScene,
    /// [`STREETS`] first, then one per building.
    pub units: Vec<Unit>,
    /// room id -> index into `sheet.rooms`.
    pub room_index: HashMap<RoomId, usize>,
    /// Group index -> its frame offset (`base_offset`), for translating
    /// final cells back into group-relative coordinates when editing. In
    /// solver cells for outdoor groups, in sheet cells for interiors.
    pub group_offsets: HashMap<usize, Cell>,
    /// Interior group index -> cluster id: groups of one walkable
    /// building.
    pub group_cluster: HashMap<usize, usize>,
    /// Group index -> cells per solver cell: [`OUTDOOR_SCALE`] for an
    /// outdoor group, 1 for an interior one.
    pub group_scale: HashMap<usize, i32>,
}

impl MapScene {
    #[must_use]
    pub fn room(&self, id: RoomId) -> Option<&SceneRoom> {
        let &idx = self.room_index.get(&id)?;
        Some(&self.sheet.rooms[idx])
    }

    /// The unit a room belongs to first.
    #[must_use]
    pub fn unit_of(&self, id: RoomId) -> Option<usize> {
        self.room(id).map(|r| r.unit)
    }

    /// Cells per solver cell for a group's rooms.
    #[must_use]
    pub fn scale_of(&self, group: usize) -> i32 {
        self.group_scale.get(&group).copied().unwrap_or(1)
    }

    /// Every group sharing a building with `group` (including itself) --
    /// the set a mini map draws when the character is indoors.
    #[must_use]
    pub fn cluster_groups(&self, group: usize) -> HashSet<usize> {
        match self.group_cluster.get(&group) {
            Some(&cluster) => self
                .group_cluster
                .iter()
                .filter(|&(_, &c)| c == cluster)
                .map(|(&g, _)| g)
                .collect(),
            None => std::iter::once(group).collect(),
        }
    }
}

/// A connector label worth drawing: the movement command when it's short,
/// not a cardinal, and not scripted (a simplified port of the reference's
/// `getConnectionLabel`).
fn connector_label(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    if Dir::from_exact(&lowered).is_some() || lowered == "out" {
        return None;
    }
    let rest = lowered
        .strip_prefix("go ")
        .or_else(|| lowered.strip_prefix("climb "))
        .or_else(|| lowered.strip_prefix("move "))
        .unwrap_or(&lowered);
    if rest.len() <= 20 {
        Some(rest.to_owned())
    } else {
        None
    }
}

/// Every room of the area on one sheet, its units, and its edges.
#[must_use]
pub fn build_scene(location: &str, layout: &Layout, map: &Map) -> MapScene {
    let dirs = DirectionMap::build(map);

    let mut scene = MapScene {
        location: location.to_owned(),
        ..Default::default()
    };

    let interiors: HashSet<usize> = layout.interiors.iter().copied().collect();
    scene.group_cluster = interior_clusters(&layout.groups, &interiors, map);
    for group in &layout.groups {
        let scale = if interiors.contains(&group.index) {
            1
        } else {
            OUTDOOR_SCALE
        };
        scene.group_scale.insert(group.index, scale);
    }

    let unit_of_group = populate_units(&mut scene, layout, map);
    populate_rooms(&mut scene, layout, map, &unit_of_group);
    populate_edges(&mut scene, layout, map, &dirs, &unit_of_group);
    populate_labels(&mut scene, layout, &unit_of_group);
    compute_sheet_bounds(&mut scene);

    scene
}

/// The streets, then one unit per building cluster; returns group index
/// -> unit index.
fn populate_units(scene: &mut MapScene, layout: &Layout, map: &Map) -> HashMap<usize, usize> {
    let mut unit_of_group: HashMap<usize, usize> = HashMap::new();
    let mut streets = Unit {
        kind: UnitKind::Streets,
        name: "Streets".to_owned(),
        rooms: Vec::new(),
        door_rooms: Vec::new(),
    };
    for &idx in &layout.outdoor {
        streets
            .rooms
            .extend(layout.groups[idx].room_ids.iter().copied());
        unit_of_group.insert(idx, STREETS);
    }
    streets.rooms.sort_unstable();
    scene.units.push(streets);

    // Clusters in a stable order: by cluster id, which is the smallest
    // member group index.
    let mut clusters: Vec<usize> = scene.group_cluster.values().copied().collect();
    clusters.sort_unstable();
    clusters.dedup();
    for cluster in clusters {
        let mut members: Vec<usize> = scene
            .group_cluster
            .iter()
            .filter(|&(_, &c)| c == cluster)
            .map(|(&g, _)| g)
            .collect();
        members.sort_unstable();
        let unit = scene.units.len();
        let mut rooms: Vec<RoomId> = Vec::new();
        let mut door_rooms: Vec<RoomId> = Vec::new();
        let mut votes: HashMap<String, usize> = HashMap::new();
        for &idx in &members {
            unit_of_group.insert(idx, unit);
            rooms.extend(layout.groups[idx].room_ids.iter().copied());
            if let Some(name) = layout.groups[idx]
                .name
                .clone()
                .or_else(|| building_name(&layout.groups[idx], map))
            {
                *votes.entry(name).or_default() += 1;
            }
            for e in layout
                .classification
                .entrances
                .get(&idx)
                .map_or(&[] as &[_], Vec::as_slice)
            {
                door_rooms.push(e.interior_room_id);
            }
        }
        rooms.sort_unstable();
        door_rooms.sort_unstable();
        door_rooms.dedup();
        let name = votes
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map_or_else(|| "Building".to_owned(), |(n, _)| n);
        scene.units.push(Unit {
            kind: UnitKind::Building,
            name,
            rooms,
            door_rooms,
        });
    }
    // An interior group in no cluster would be a building of its own.
    // There are none -- every interior group is at least its own cluster
    // -- but a room must have a unit, so this is not left to chance.
    for &idx in &layout.interiors {
        if let std::collections::hash_map::Entry::Vacant(slot) = unit_of_group.entry(idx) {
            let unit = scene.units.len();
            slot.insert(unit);
            scene.units.push(Unit {
                kind: UnitKind::Building,
                name: layout.groups[idx]
                    .name
                    .clone()
                    .unwrap_or_else(|| "Building".to_owned()),
                rooms: layout.groups[idx].room_ids.clone(),
                door_rooms: Vec::new(),
            });
        }
    }
    unit_of_group
}

/// A group's room, on the sheet: solver cells times the group's scale.
fn sheet_cell(scene: &MapScene, layout: &Layout, group: usize, id: RoomId) -> Cell {
    let c = layout.groups[group].final_cell(id);
    let scale = scene.scale_of(group);
    Cell {
        x: c.x * scale,
        y: c.y * scale,
    }
}

/// One [`SceneRoom`] per placed room, plus each group's frame offset.
fn populate_rooms(
    scene: &mut MapScene,
    layout: &Layout,
    map: &Map,
    unit_of_group: &HashMap<usize, usize>,
) {
    for group in &layout.groups {
        scene
            .group_offsets
            .insert(group.index, group.base_offset.unwrap_or_default());
        for &id in &group.room_ids {
            let room = map.room(id);
            let scene_room = SceneRoom {
                id,
                uid: room.and_then(|r| r.uid.first()).map(|u| u.0),
                cell: sheet_cell(scene, layout, group.index, id),
                group: group.index,
                unit: unit_of_group.get(&group.index).copied().unwrap_or(STREETS),
                entrance: layout.classification.entrance_room_ids.contains(&id),
                title: room
                    .and_then(|r| r.title.first())
                    .cloned()
                    .unwrap_or_default(),
                terrain: room.and_then(|r| r.terrain.clone()),
                service_tags: room
                    .map(|r| {
                        r.tags
                            .iter()
                            .filter(|t| SERVICE_TAGS.contains(&t.as_str()))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            scene.room_index.insert(id, scene.sheet.rooms.len());
            scene.sheet.rooms.push(scene_room);
        }
    }
}

/// Every drawable edge: directional within a group; connectors across
/// groups -- roads between outdoor groups, passages within a building,
/// and doors between a street and a building. Deduped by unordered pair,
/// direction checked either way.
fn populate_edges(
    scene: &mut MapScene,
    layout: &Layout,
    map: &Map,
    dirs: &DirectionMap,
    unit_of_group: &HashMap<usize, usize>,
) {
    let group_of: HashMap<RoomId, usize> = layout
        .groups
        .iter()
        .flat_map(|g| g.room_ids.iter().map(move |&id| (id, g.index)))
        .collect();
    let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
    for room in map.rooms() {
        let Some(&room_group) = group_of.get(&room.id) else {
            continue;
        };
        for exit in &room.exits {
            // A line is a claim that you can walk there that way. A
            // teleport, a routine, an unported crossing: no line.
            if !crate::regions::is_passage(exit) {
                continue;
            }
            let target_id = exit.to;
            let Some(&target_group) = group_of.get(&target_id) else {
                continue;
            };
            let key = (room.id.min(target_id), room.id.max(target_id));
            let a = sheet_cell(scene, layout, room_group, room.id);
            let b = sheet_cell(scene, layout, target_group, target_id);
            let len = (a.x - b.x).abs().max((a.y - b.y).abs());
            let unit_a = unit_of_group.get(&room_group).copied().unwrap_or(STREETS);
            let unit_b = unit_of_group.get(&target_group).copied().unwrap_or(STREETS);
            let inside = (unit_a == unit_b && unit_a != STREETS).then_some(unit_a);
            let (kind, label) = if room_group == target_group {
                // A same-group edge: solid when its stated direction
                // resolves, a stub when it stretches past
                // LONG_EDGE_CELLS solver cells, nothing when direction
                // analysis found none.
                if dirs.get(room.id, target_id).is_none() {
                    continue;
                }
                let kind = if len > LONG_EDGE_CELLS * scene.scale_of(room_group) {
                    SceneEdgeKind::Stub
                } else {
                    SceneEdgeKind::Directional
                };
                (kind, None)
            } else {
                // A cross-group edge. Between two buildings that are not
                // one cluster, nothing draws: that is not a passage a
                // person walks. Anything else draws unless it is absurdly
                // long: a road, a passage, a door.
                if unit_a != STREETS && unit_b != STREETS && unit_a != unit_b {
                    continue;
                }
                if len > CONNECTOR_MAX_CELLS * OUTDOOR_SCALE {
                    continue;
                }
                let cmd = match &exit.crossing {
                    cena_map::Crossing::Command(cmd) => cmd.as_str(),
                    _ => "",
                };
                (SceneEdgeKind::Connector, connector_label(cmd))
            };
            if !seen.insert(key) {
                continue;
            }
            scene.sheet.edges.push(SceneEdge {
                a,
                b,
                a_room: room.id,
                b_room: target_id,
                group: room_group,
                kind,
                label,
                unit: inside,
            });
        }
    }
}

/// Building labels: one per unit, named as the unit is, anchored at the
/// building's top-left.
fn populate_labels(scene: &mut MapScene, layout: &Layout, unit_of_group: &HashMap<usize, usize>) {
    let mut labels = cluster_labels(&scene.group_cluster, &layout.groups);
    for label in &mut labels {
        label.unit = unit_of_group.get(&label.group).copied();
        if let Some(unit) = label.unit {
            label.text.clone_from(&scene.units[unit].name);
        }
    }
    scene.sheet.labels = labels;
}

/// Building labels: one per cluster, named by the majority group name
/// among its members, anchored at the combined bounds' top-left.
fn cluster_labels(
    cluster_map: &HashMap<usize, usize>,
    groups: &[crate::positioner::Group],
) -> Vec<GroupLabel> {
    let mut clusters: Vec<usize> = cluster_map.values().copied().collect();
    clusters.sort_unstable();
    clusters.dedup();
    let mut labels = Vec::new();
    for cluster in clusters {
        let mut name_votes: Vec<(&str, usize)> = Vec::new();
        let mut min = Cell {
            x: i32::MAX,
            y: i32::MAX,
        };
        for (&idx, &c) in cluster_map {
            if c != cluster {
                continue;
            }
            let group = &groups[idx];
            let bounds = group.bounds();
            let off = group.base_offset.unwrap_or_default();
            min.x = min.x.min(bounds.min_x + off.x);
            min.y = min.y.min(bounds.min_y + off.y);
            if let Some(name) = group.name.as_deref() {
                match name_votes.iter_mut().find(|(n, _)| *n == name) {
                    Some(entry) => entry.1 += 1,
                    None => name_votes.push((name, 1)),
                }
            }
        }
        let named_total: usize = name_votes.iter().map(|(_, c)| c).sum();
        let mut best: Option<(&str, usize)> = None;
        for (name, count) in name_votes {
            if best.is_none_or(|(_, c)| count > c) {
                best = Some((name, count));
            }
        }
        let Some((name, count)) = best else {
            continue;
        };
        if count * 2 <= named_total {
            continue;
        }
        labels.push(GroupLabel {
            text: name.to_owned(),
            cell: min,
            group: cluster,
            unit: None,
        });
    }
    labels
}

fn compute_sheet_bounds(scene: &mut MapScene) {
    let sheet = &mut scene.sheet;
    let mut min = Cell {
        x: i32::MAX,
        y: i32::MAX,
    };
    let mut max = Cell {
        x: i32::MIN,
        y: i32::MIN,
    };
    for cell in sheet.rooms.iter().map(|r| r.cell) {
        min.x = min.x.min(cell.x);
        min.y = min.y.min(cell.y);
        max.x = max.x.max(cell.x);
        max.y = max.y.max(cell.y);
    }
    if sheet.rooms.is_empty() {
        min = Cell::default();
        max = Cell::default();
    }
    sheet.min = min;
    sheet.max = max;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Cost, Exit, ExitKind, Room};

    fn room(id: u32, uid: i64, exits: &[(u32, &str)]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![cena_map::Uid(uid)],
            title: vec![format!("[Room {id}]")],
            description: vec![],
            paths: vec!["Obvious paths: north, south".to_owned()],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits: exits
                .iter()
                .map(|&(to, cmd)| Exit {
                    to: RoomId(to),
                    kind: ExitKind::Cardinal,
                    crossing: cena_map::Crossing::Command(cmd.to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    #[test]
    fn a_simple_pair_draws_one_directional_edge() {
        let rooms = vec![
            room(1, 9_000_001, &[(2, "north")]),
            room(2, 9_000_002, &[(1, "south")]),
        ];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let layout = crate::generate_layout(&map);
        let scene = build_scene("Test", &layout, &map);

        assert_eq!(scene.sheet.edges.len(), 1);
        assert_eq!(scene.sheet.edges[0].kind, SceneEdgeKind::Directional);
        assert_eq!(scene.sheet.rooms.len(), 2);
        assert_eq!(scene.units.len(), 1);
        assert_eq!(scene.units[STREETS].kind, UnitKind::Streets);
    }

    /// A teleport is not a walk, so it is not a line. The premium halls
    /// carry a pass-through to their town's urchin hideout beside their
    /// ordinary doors, and the hideout is nowhere near them: drawing it
    /// ran a connector clear across the sheet to a room nobody can walk
    /// to. Two separate streets here, so the teleport between them is a
    /// cross-group edge -- the case a connector is drawn for.
    #[test]
    fn a_teleport_draws_no_line() {
        let mut rooms = vec![
            room(1, 9_000_001, &[(2, "north")]),
            room(2, 9_000_002, &[(1, "south")]),
            room(3, 9_000_003, &[(4, "north")]),
            room(4, 9_000_004, &[(3, "south")]),
        ];
        let teleport = Exit {
            to: RoomId(3),
            kind: ExitKind::Cardinal,
            crossing: cena_map::Crossing::PassThrough(cena_map::Pass),
            cost: Some(Cost::Fixed(1.0)),
        };
        rooms[0].exits.push(teleport);
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let layout = crate::generate_layout(&map);
        let scene = build_scene("Test", &layout, &map);

        assert!(
            !scene
                .sheet
                .edges
                .iter()
                .any(|e| (e.a_room, e.b_room) == (RoomId(1), RoomId(3))
                    || (e.a_room, e.b_room) == (RoomId(3), RoomId(1))),
            "the teleport drew a line"
        );
    }

    #[test]
    fn connector_labels() {
        assert_eq!(connector_label("go dock"), Some("dock".into()));
        assert_eq!(
            connector_label("climb rope ladder"),
            Some("rope ladder".into())
        );
        assert_eq!(connector_label("north"), None);
        assert_eq!(connector_label("out"), None);
        assert_eq!(
            connector_label("go some extremely long movement command"),
            None
        );
    }

    /// A street of enough corners to be a town, each with shops behind
    /// doors. **One sheet, one frame:** every room has one cell, the
    /// street rooms at the outdoor scale, each shop beside its own
    /// corner, joined to it by a door edge; the shops are building units
    /// whose door rooms are the shops, and the corners are the streets.
    #[test]
    fn a_town_is_one_sheet_with_the_shops_beside_their_corners() {
        const CORNERS: u32 = 12;
        let shop = |id: u32, street: u32| Room {
            id: RoomId(id),
            uid: vec![cena_map::Uid(i64::from(id))],
            title: vec![format!("[Shop {id}]")],
            description: vec![],
            paths: vec!["Obvious exits: out".to_owned()],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits: vec![Exit {
                to: RoomId(street),
                kind: ExitKind::Out,
                crossing: cena_map::Crossing::Command("out".to_owned()),
                cost: Some(Cost::Fixed(1.0)),
            }],
        };
        let mut rooms = Vec::new();
        for i in 0..CORNERS {
            let street = 1000 + i;
            rooms.push(shop(i * 2, street));
            rooms.push(shop(i * 2 + 1, street));
            let mut exits: Vec<(u32, &str)> = vec![(i * 2, "go shop"), (i * 2 + 1, "go shop")];
            if i > 0 {
                exits.push((street - 1, "west"));
            }
            if i + 1 < CORNERS {
                exits.push((street + 1, "east"));
            }
            rooms.push(room(street, i64::from(street), &exits));
        }
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let layout = crate::generate_layout(&map);
        assert!(!layout.interiors.is_empty(), "the shops did not shelve");
        let scene = build_scene("street", &layout, &map);

        // One room, one cell, and no two rooms on one cell.
        assert_eq!(scene.sheet.rooms.len(), map.rooms().len());
        assert_eq!(scene.room_index.len(), map.rooms().len());
        let mut cells: Vec<Cell> = scene.sheet.rooms.iter().map(|r| r.cell).collect();
        cells.sort_unstable_by_key(|c| (c.x, c.y));
        let before = cells.len();
        cells.dedup();
        assert_eq!(cells.len(), before, "two rooms share a cell");

        // The street is at the outdoor scale, in the streets unit.
        let corner = scene
            .room(RoomId(1000))
            .expect("corner 1000 is on the sheet");
        assert_eq!(corner.cell.x % OUTDOOR_SCALE, 0);
        assert_eq!(corner.cell.y % OUTDOOR_SCALE, 0);
        assert_eq!(corner.unit, STREETS);
        let next = scene
            .room(RoomId(1001))
            .expect("corner 1001 is on the sheet");
        assert_eq!(
            (next.cell.x - corner.cell.x)
                .abs()
                .max((next.cell.y - corner.cell.y).abs()),
            OUTDOOR_SCALE,
            "adjacent corners are not one scaled cell apart"
        );

        // Each shop beside its corner, joined by a door edge, in a
        // building unit that names it as its door room.
        for shop in [0u32, 1] {
            let r = scene.room(RoomId(shop)).expect("shop is on the sheet");
            let apart = (r.cell.x - corner.cell.x)
                .abs()
                .max((r.cell.y - corner.cell.y).abs());
            assert!(apart <= 2, "shop {shop} is {apart} cells from its corner");
            assert!(
                scene
                    .sheet
                    .edges
                    .iter()
                    .any(|e| e.kind == SceneEdgeKind::Connector
                        && e.unit.is_none()
                        && ((e.a_room == RoomId(1000) && e.b_room == RoomId(shop))
                            || (e.b_room == RoomId(1000) && e.a_room == RoomId(shop)))),
                "no door edge between corner 1000 and shop {shop}"
            );
            let unit = &scene.units[r.unit];
            assert_eq!(unit.kind, UnitKind::Building);
            assert!(unit.rooms.contains(&RoomId(shop)));
            assert!(
                unit.door_rooms.contains(&RoomId(shop)),
                "{:?}",
                unit.door_rooms
            );
            assert_eq!(unit.name, format!("Shop {shop}"));
        }
        // Every room is in the unit it says it is.
        for r in &scene.sheet.rooms {
            assert!(scene.units[r.unit].rooms.contains(&r.id));
        }
    }
}
