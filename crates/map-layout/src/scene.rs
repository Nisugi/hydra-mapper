//! Presentation model (spec §8): the drawable form of a generated layout.
//! Ported from `reference/VellumFE/src/core/layout_engine/scene.rs`.
//!
//! Pure data -- no rendering toolkit types -- so `cena-mapper`'s window and
//! any later embedder both draw from the same scene. Rooms carry final
//! sheet cells; edges are pre-classified: solid directional edges, stubs
//! for directional edges stretched past `LONG_EDGE_CELLS`, and dashed
//! labeled connectors (skipped past `CONNECTOR_MAX_CELLS`).
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

/// The outdoor sheet is drawn at the interiors sheet's scale, so the two
/// are one picture: the same street is the same length on both, and a
/// building's doorway dot on one sits where the building is on the
/// other. Measured on gs.map for the doorway dots alone, twice the
/// solver's spacing would do (442 of the Landing's 470 fit beside their
/// street; at four times, all of them); the rest of the scale is for the
/// match.
pub const OUTDOOR_SCALE: i32 = crate::interior_shelf::TOWN_SCALE;
/// On the interiors sheet, passages draw only when the rooms sit close
/// together. Merged placement seats each component beside the room its
/// anchoring passage connects to, so genuine doorways stay short; in huge
/// complexes (player-shop districts) the remaining long cross-links are
/// noise that reads as spaghetti.
pub const INTERIOR_CONNECTOR_MAX_CELLS: i32 = 4;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sheet {
    Outdoor,
    Interiors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneRoom {
    pub id: RoomId,
    pub uid: Option<i64>,
    pub cell: Cell,
    pub group: usize,
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
    /// Group of the `a` endpoint (directional/stub edges are intra-group;
    /// connectors only exist on the outdoor sheet).
    pub group: usize,
    pub kind: SceneEdgeKind,
    /// Movement label for connectors ("dock", "gate") when one is worth
    /// showing.
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupLabel {
    pub text: String,
    /// Top-left cell of the group's bounds; render above it.
    pub cell: Cell,
    pub group: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetScene {
    pub rooms: Vec<SceneRoom>,
    pub edges: Vec<SceneEdge>,
    pub labels: Vec<GroupLabel>,
    /// Street rooms echoed onto this sheet at the heart of their island of
    /// buildings. Only the interiors sheet has any. Drawn distinctly: an
    /// echo is a signpost saying "these doors open onto here", not a room
    /// the sheet can walk to.
    #[serde(default)]
    pub anchors: Vec<SceneAnchor>,
    /// Scene cells per layout cell. The outdoor sheet is drawn at
    /// [`OUTDOOR_SCALE`] so the doorway echoes fit beside the streets;
    /// anything that turns a drawn distance back into layout cells -- a
    /// drag -- divides by this.
    #[serde(default = "one")]
    pub scale: i32,
    pub min: Cell,
    pub max: Cell,
}

const fn one() -> i32 {
    1
}

impl Default for SheetScene {
    fn default() -> Self {
        SheetScene {
            rooms: Vec::new(),
            edges: Vec::new(),
            labels: Vec::new(),
            anchors: Vec::new(),
            scale: 1,
            min: Cell::default(),
            max: Cell::default(),
        }
    }
}

/// A street room drawn a second time, among the buildings that open off
/// it. The room's one true [`SceneRoom`] is on the outdoor sheet; this is
/// where it is *also* drawn, so a door edge can be a line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneAnchor {
    pub id: RoomId,
    pub cell: Cell,
    /// First room title, for the label and hover text.
    pub title: String,
    /// Whether any building opens off this street room. Most echoes are
    /// street rooms carried along so the road stays continuous, and they
    /// should read as road -- a dot -- not as another doorway.
    #[serde(default)]
    pub has_door: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MapScene {
    pub location: String,
    pub outdoor: SheetScene,
    pub interiors: SheetScene,
    /// room id -> (sheet, index into that sheet's `rooms`).
    pub room_index: HashMap<RoomId, (Sheet, usize)>,
    /// Group index -> its sheet frame offset (`base_offset`), for
    /// translating final cells back into group-relative coordinates when
    /// editing.
    pub group_offsets: HashMap<usize, Cell>,
    /// Interior group index -> cluster id: groups of one walkable
    /// building.
    pub group_cluster: HashMap<usize, usize>,
}

impl MapScene {
    #[must_use]
    pub fn sheet(&self, sheet: Sheet) -> &SheetScene {
        match sheet {
            Sheet::Outdoor => &self.outdoor,
            Sheet::Interiors => &self.interiors,
        }
    }

    #[must_use]
    pub fn room(&self, id: RoomId) -> Option<(Sheet, &SceneRoom)> {
        let &(sheet, idx) = self.room_index.get(&id)?;
        Some((sheet, &self.sheet(sheet).rooms[idx]))
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

/// Which sheet a group belongs to, as a closure callers pass around: an
/// interior-group index draws on the interiors shelf, everything else on
/// the shared outdoor sheet.
fn sheet_of_fn(interiors: &HashSet<usize>) -> impl Fn(usize) -> Sheet + '_ {
    move |group: usize| {
        if interiors.contains(&group) {
            Sheet::Interiors
        } else {
            Sheet::Outdoor
        }
    }
}

/// One [`SceneRoom`] per placed room, filed into its sheet and into
/// `scene.room_index`, plus each group's sheet offset.
fn populate_rooms(
    scene: &mut MapScene,
    layout: &Layout,
    map: &Map,
    sheet_of: &impl Fn(usize) -> Sheet,
) {
    for group in &layout.groups {
        let sheet = sheet_of(group.index);
        scene
            .group_offsets
            .insert(group.index, group.base_offset.unwrap_or_default());
        for &id in &group.room_ids {
            let room = map.room(id);
            let scene_room = SceneRoom {
                id,
                uid: room.and_then(|r| r.uid.first()).map(|u| u.0),
                cell: group.final_cell(id),
                group: group.index,
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
            let target = match sheet {
                Sheet::Outdoor => &mut scene.outdoor,
                Sheet::Interiors => &mut scene.interiors,
            };
            scene.room_index.insert(id, (sheet, target.rooms.len()));
            target.rooms.push(scene_room);
        }
    }
}

/// Every drawable edge: directional within a group; connectors across
/// groups on the same sheet. Deduped by unordered pair, direction checked
/// either way.
fn populate_edges(
    scene: &mut MapScene,
    layout: &Layout,
    map: &Map,
    dirs: &DirectionMap,
    sheet_of: &impl Fn(usize) -> Sheet,
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
            let target_id = exit.to;
            let Some(&target_group) = group_of.get(&target_id) else {
                continue;
            };
            let cmd = match &exit.crossing {
                cena_map::Crossing::Command(cmd) => cmd.as_str(),
                _ => "",
            };
            let key = (room.id.min(target_id), room.id.max(target_id));
            let a_sheet = sheet_of(room_group);
            if room_group == target_group {
                push_directional_edge(
                    scene, layout, dirs, room.id, target_id, room_group, a_sheet, &mut seen, key,
                );
            } else {
                push_connector_edge(
                    scene,
                    layout,
                    room.id,
                    target_id,
                    room_group,
                    target_group,
                    a_sheet,
                    sheet_of,
                    cmd,
                    &mut seen,
                    key,
                );
            }
        }
    }
}

/// A same-group edge: solid when its stated direction resolves, a stub
/// when it stretches past [`LONG_EDGE_CELLS`], nothing when direction
/// analysis found none (matches Vellum's own edge classification, spec
/// §8).
#[allow(clippy::too_many_arguments)]
fn push_directional_edge(
    scene: &mut MapScene,
    layout: &Layout,
    dirs: &DirectionMap,
    room_id: RoomId,
    target_id: RoomId,
    room_group: usize,
    a_sheet: Sheet,
    seen: &mut HashSet<(RoomId, RoomId)>,
    key: (RoomId, RoomId),
) {
    if dirs.get(room_id, target_id).is_none() {
        return;
    }
    if !seen.insert(key) {
        return;
    }
    let group = &layout.groups[room_group];
    let a = group.final_cell(room_id);
    let b = group.final_cell(target_id);
    let len = (a.x - b.x).abs().max((a.y - b.y).abs());
    push_edge(
        scene,
        a_sheet,
        SceneEdge {
            a,
            b,
            a_room: room_id,
            b_room: target_id,
            group: room_group,
            kind: if len > LONG_EDGE_CELLS {
                SceneEdgeKind::Stub
            } else {
                SceneEdgeKind::Directional
            },
            label: None,
        },
    );
}

/// A cross-group edge. Outdoors: any same-sheet pair draws (geographic
/// adjacency). Indoors: only within one cluster -- passages like "go arch"
/// inside a building; between unrelated buildings on the shelf, nothing
/// draws (doorways show as door markers outdoors instead).
#[allow(clippy::too_many_arguments)]
fn push_connector_edge(
    scene: &mut MapScene,
    layout: &Layout,
    room_id: RoomId,
    target_id: RoomId,
    room_group: usize,
    target_group: usize,
    a_sheet: Sheet,
    sheet_of: &impl Fn(usize) -> Sheet,
    cmd: &str,
    seen: &mut HashSet<(RoomId, RoomId)>,
    key: (RoomId, RoomId),
) {
    let b_sheet = sheet_of(target_group);
    if a_sheet != b_sheet {
        return;
    }
    if a_sheet == Sheet::Interiors
        && scene.group_cluster.get(&room_group) != scene.group_cluster.get(&target_group)
    {
        return;
    }
    if !seen.insert(key) {
        return;
    }
    let a = layout.groups[room_group].final_cell(room_id);
    let b = layout.groups[target_group].final_cell(target_id);
    let len = (a.x - b.x).abs().max((a.y - b.y).abs());
    let cap = if a_sheet == Sheet::Interiors {
        INTERIOR_CONNECTOR_MAX_CELLS
    } else {
        CONNECTOR_MAX_CELLS
    };
    if len > cap {
        return;
    }
    push_edge(
        scene,
        a_sheet,
        SceneEdge {
            a,
            b,
            a_room: room_id,
            b_room: target_id,
            group: room_group,
            kind: SceneEdgeKind::Connector,
            label: connector_label(cmd),
        },
    );
}

/// Building labels: one per CLUSTER (one building, one label), named by
/// the majority group name among its members, anchored at the combined
/// bounds' top-left. The cluster id is its smallest member group index, so
/// it always passes a cluster-set group filter. Shelved buildings label
/// the interiors sheet; try-inlined buildings label the outdoor sheet the
/// same way.
fn populate_labels(scene: &mut MapScene, layout: &Layout, map: &Map) {
    scene.interiors.labels = cluster_labels(&scene.group_cluster, &layout.groups);
    let inlined_set: HashSet<usize> = layout.inlined.iter().copied().collect();
    if !inlined_set.is_empty() {
        let inlined_clusters = interior_clusters(&layout.groups, &inlined_set, map);
        scene.outdoor.labels = cluster_labels(&inlined_clusters, &layout.groups);
    }
}

/// The street-room echoes on the interiors sheet, and the door edges they
/// make drawable.
///
/// A shop's door edge used to have its two ends on different sheets, in
/// different coordinate spaces, and so was never drawn -- 1,116 such edges
/// in Wehnimer's Landing alone, which is why a shelved shop looked
/// unrelated to anything. With the street room echoed beside its
/// buildings, both ends share a sheet and the edge is an ordinary
/// connector line.
fn populate_anchors(scene: &mut MapScene, layout: &Layout, map: &Map) {
    // The road itself: the outdoor sheet's own edges, each one drawn
    // again between the echoes of its two rooms, the same kind and the
    // same label. Not re-derived from the exits -- the two sheets are
    // one picture, and a road that is solid on one is solid on the
    // other. Ordinary outdoor edges are one cell long, so at the
    // interiors' scale they are `TOWN_SCALE`; a stub stays a stub.
    let echo_cell: HashMap<RoomId, Cell> =
        layout.anchors.iter().map(|a| (a.room, a.cell)).collect();
    for edge in &scene.outdoor.edges {
        let (Some(&a), Some(&b)) = (echo_cell.get(&edge.a_room), echo_cell.get(&edge.b_room))
        else {
            continue;
        };
        scene.interiors.edges.push(SceneEdge {
            a,
            b,
            a_room: edge.a_room,
            b_room: edge.b_room,
            group: usize::MAX,
            kind: edge.kind,
            label: edge.label.clone(),
        });
    }
    for anchor in &layout.anchors {
        let Some(room) = map.room(anchor.room) else {
            continue;
        };
        let echo_at = scene.interiors.anchors.len();
        scene.interiors.anchors.push(SceneAnchor {
            id: anchor.room,
            cell: anchor.cell,
            title: room.title.first().cloned().unwrap_or_default(),
            has_door: false,
        });
        // Every door from this street room to a room on the interiors
        // sheet, read from both ends so a one-way door still draws.
        let mut doors: Vec<RoomId> = room.exits.iter().map(|e| e.to).collect();
        for other in map.rooms() {
            if other.exits.iter().any(|e| e.to == anchor.room) {
                doors.push(other.id);
            }
        }
        doors.sort_unstable();
        doors.dedup();
        for door in doors {
            let Some(&(Sheet::Interiors, at)) = scene.room_index.get(&door) else {
                continue;
            };
            let target = &scene.interiors.rooms[at];
            // Every street room a building opens onto is in its frame,
            // so a door edge is short by construction -- max 13 cells
            // across the real map's towns, median 1. The cap is the
            // outdoor connector's, generous: a door this long is a frame
            // that failed, and hiding it would hide the failure.
            let len = (anchor.cell.x - target.cell.x)
                .abs()
                .max((anchor.cell.y - target.cell.y).abs());
            if len > CONNECTOR_MAX_CELLS {
                continue;
            }
            let cmd = room
                .exits
                .iter()
                .find(|e| e.to == door)
                .and_then(|e| match &e.crossing {
                    cena_map::Crossing::Command(c) => Some(c.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            scene.interiors.edges.push(SceneEdge {
                a: anchor.cell,
                b: target.cell,
                a_room: anchor.room,
                b_room: door,
                group: target.group,
                kind: SceneEdgeKind::Connector,
                label: connector_label(cmd),
            });
            scene.interiors.anchors[echo_at].has_door = true;
        }
    }
}

/// Each sheet's drawn bounds, from its placed rooms' cells and echoes.
/// Spread a sheet out: every drawn cell times `scale`. Rooms keep their
/// solver cells for everything that is not drawing.
fn scale_sheet(sheet: &mut SheetScene, scale: i32) {
    let up = |c: Cell| Cell {
        x: c.x * scale,
        y: c.y * scale,
    };
    for room in &mut sheet.rooms {
        room.cell = up(room.cell);
    }
    for edge in &mut sheet.edges {
        edge.a = up(edge.a);
        edge.b = up(edge.b);
    }
    for label in &mut sheet.labels {
        label.cell = up(label.cell);
    }
    for anchor in &mut sheet.anchors {
        anchor.cell = up(anchor.cell);
    }
    sheet.scale = scale;
}

/// **The interiors, echoed onto the outdoor sheet.** Beside each street
/// room, one cell per building that opens off it: the building's door
/// room, drawn as a doorway and named for the building, joined to the
/// street by a short connector. The mirror of the street echoes on the
/// interiors sheet, and the thing a person reads a town map for -- what
/// is here -- without leaving the streets.
///
/// A doorway takes the nearest free cell around its street room, one
/// ring out and then two; the cells under the roads between adjacent
/// street rooms are kept clear. A doorway with no free cell within two
/// rings is left out rather than dropped somewhere misleading; the
/// street room's own door marker still says a building is there.
fn populate_doorways(scene: &mut MapScene, layout: &Layout, map: &Map) {
    let scale = scene.outdoor.scale;
    let street_cell: HashMap<RoomId, Cell> =
        scene.outdoor.rooms.iter().map(|r| (r.id, r.cell)).collect();
    let mut occupied: HashSet<Cell> = street_cell.values().copied().collect();
    // Road cells: the line between two adjacent street rooms.
    for edge in &scene.outdoor.edges {
        let (dx, dy) = (edge.b.x - edge.a.x, edge.b.y - edge.a.y);
        if dx.abs().max(dy.abs()) != scale {
            continue;
        }
        for step in 1..scale {
            occupied.insert(Cell {
                x: edge.a.x + dx.signum() * step,
                y: edge.a.y + dy.signum() * step,
            });
        }
    }
    // One doorway per (street room, building), in a stable order.
    let mut doorways: Vec<(RoomId, RoomId, usize)> = Vec::new();
    let mut seen: HashSet<(RoomId, usize)> = HashSet::new();
    let mut entries: Vec<(&usize, &Vec<crate::classifier::Entrance>)> =
        layout.classification.entrances.iter().collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (&idx, list) in entries {
        let cluster = scene.group_cluster.get(&idx).copied().unwrap_or(idx);
        for e in list {
            if street_cell.contains_key(&e.outdoor_room_id)
                && seen.insert((e.outdoor_room_id, cluster))
            {
                doorways.push((e.outdoor_room_id, e.interior_room_id, idx));
            }
        }
    }
    doorways.sort_by_key(|&(street, door, _)| (street, door));
    for (street, door, idx) in doorways {
        let at = street_cell[&street];
        let mut found: Option<Cell> = None;
        'rings: for r in 1..=2 {
            let mut ring: Vec<Cell> = Vec::new();
            crate::packer::for_ring(at, r, |c| ring.push(c));
            for c in ring {
                if !occupied.contains(&c) {
                    found = Some(c);
                    break 'rings;
                }
            }
        }
        let Some(cell) = found else {
            continue;
        };
        occupied.insert(cell);
        let title = building_name(&layout.groups[idx], map)
            .or_else(|| map.room(door).and_then(|r| r.title.first().cloned()))
            .unwrap_or_default();
        scene.outdoor.anchors.push(SceneAnchor {
            id: door,
            cell,
            title,
            has_door: true,
        });
        scene.outdoor.edges.push(SceneEdge {
            a: at,
            b: cell,
            a_room: street,
            b_room: door,
            group: idx,
            kind: SceneEdgeKind::Connector,
            label: None,
        });
    }
}

fn compute_sheet_bounds(scene: &mut MapScene) {
    for sheet in [&mut scene.outdoor, &mut scene.interiors] {
        let mut min = Cell {
            x: i32::MAX,
            y: i32::MAX,
        };
        let mut max = Cell {
            x: i32::MIN,
            y: i32::MIN,
        };
        for cell in sheet
            .rooms
            .iter()
            .map(|r| r.cell)
            .chain(sheet.anchors.iter().map(|a| a.cell))
        {
            min.x = min.x.min(cell.x);
            min.y = min.y.min(cell.y);
            max.x = max.x.max(cell.x);
            max.y = max.y.max(cell.y);
        }
        if sheet.rooms.is_empty() && sheet.anchors.is_empty() {
            min = Cell::default();
            max = Cell::default();
        }
        sheet.min = min;
        sheet.max = max;
    }
}

#[must_use]
pub fn build_scene(location: &str, layout: &Layout, map: &Map) -> MapScene {
    let dirs = DirectionMap::build(map);

    let mut scene = MapScene {
        location: location.to_owned(),
        ..Default::default()
    };

    let interiors: HashSet<usize> = layout.interiors.iter().copied().collect();
    scene.group_cluster = interior_clusters(&layout.groups, &interiors, map);
    let sheet_of = sheet_of_fn(&interiors);

    populate_rooms(&mut scene, layout, map, &sheet_of);
    populate_edges(&mut scene, layout, map, &dirs, &sheet_of);
    populate_anchors(&mut scene, layout, map);
    populate_labels(&mut scene, layout, map);
    scale_sheet(&mut scene.outdoor, OUTDOOR_SCALE);
    populate_doorways(&mut scene, layout, map);
    compute_sheet_bounds(&mut scene);

    scene
}

/// One label per cluster: majority group name among the members, anchored
/// at the combined bounds' top-left. Unlabeled when no name has a true
/// majority -- a 300-shop district must not wear one shop's name.
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
        });
    }
    labels
}

fn push_edge(scene: &mut MapScene, sheet: Sheet, edge: SceneEdge) {
    match sheet {
        Sheet::Outdoor => scene.outdoor.edges.push(edge),
        Sheet::Interiors => scene.interiors.edges.push(edge),
    }
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

        assert_eq!(scene.outdoor.edges.len(), 1);
        assert_eq!(scene.outdoor.edges[0].kind, SceneEdgeKind::Directional);
        assert_eq!(scene.outdoor.rooms.len(), 2);
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
    /// doors. The door edge from a street room to its shop used to have
    /// its ends on different sheets and was never drawn; with the street
    /// room echoed among its shops, it is a connector line on the
    /// interiors sheet, and the echo sits within the sheet's bounds.
    #[test]
    fn a_door_edge_is_drawn_on_the_interiors_sheet_beside_its_street_rooms_echo() {
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

        // The first corner's echo is there, and both its shops' door
        // edges reach it.
        let echo = scene
            .interiors
            .anchors
            .iter()
            .find(|a| a.id == RoomId(1000))
            .expect("street room 1000 is echoed on the interiors sheet");
        for shop in [0u32, 1] {
            let drawn = scene.interiors.edges.iter().any(|e| {
                e.kind == SceneEdgeKind::Connector
                    && ((e.a_room == RoomId(1000) && e.b_room == RoomId(shop))
                        || (e.b_room == RoomId(1000) && e.a_room == RoomId(shop)))
                    && (e.a == echo.cell || e.b == echo.cell)
            });
            assert!(
                drawn,
                "no door edge drawn from the echo of 1000 to shop {shop}"
            );
        }
        // Still exactly one SceneRoom per room: the echo is not a room.
        assert_eq!(
            scene.outdoor.rooms.len() + scene.interiors.rooms.len(),
            map.rooms().len()
        );
        assert!(scene.room_index.len() == map.rooms().len());
        // And the echo lies inside the sheet's drawn bounds.
        let b = &scene.interiors;
        assert!(
            (b.min.x..=b.max.x).contains(&echo.cell.x)
                && (b.min.y..=b.max.y).contains(&echo.cell.y),
            "echo at {:?} is outside bounds {:?}..{:?}",
            echo.cell,
            b.min,
            b.max
        );

        the_mirror(&scene, &map);
    }

    /// **And the mirror.** Both of the corner's shops are echoed on the
    /// outdoor sheet as doorways beside the corner, named for the shop,
    /// each joined to the corner by a connector; the corner's own cell
    /// is a multiple of the sheet's scale, so the street is spread out
    /// enough for them to fit.
    fn the_mirror(scene: &MapScene, map: &Map) {
        // Both of the corner's shops are echoed on the
        // outdoor sheet as doorways beside the corner, named for the
        // shop, each joined to the corner by a connector; the corner's
        // own cell is a multiple of the sheet's scale, so the street is
        // spread out enough for them to fit.
        let corner = scene
            .outdoor
            .rooms
            .iter()
            .find(|r| r.id == RoomId(1000))
            .expect("street room 1000 is on the outdoor sheet");
        assert_eq!(scene.outdoor.scale, OUTDOOR_SCALE);
        assert_eq!(corner.cell.x % OUTDOOR_SCALE, 0);
        assert_eq!(corner.cell.y % OUTDOOR_SCALE, 0);
        for shop in [0u32, 1] {
            let doorway = scene
                .outdoor
                .anchors
                .iter()
                .find(|a| a.id == RoomId(shop))
                .unwrap_or_else(|| panic!("shop {shop} has no doorway echo on the outdoor sheet"));
            let apart = (doorway.cell.x - corner.cell.x)
                .abs()
                .max((doorway.cell.y - corner.cell.y).abs());
            assert!(
                apart <= 2,
                "shop {shop}'s doorway is {apart} cells from its street"
            );
            assert!(doorway.has_door);
            assert_eq!(doorway.title, format!("Shop {shop}"));
            assert!(
                scene
                    .outdoor
                    .edges
                    .iter()
                    .any(|e| e.kind == SceneEdgeKind::Connector
                        && e.a_room == RoomId(1000)
                        && e.b_room == RoomId(shop)
                        && e.a == corner.cell
                        && e.b == doorway.cell),
                "no connector from the corner to shop {shop}'s doorway"
            );
        }
        // A doorway is not a room either.
        assert!(scene.room_index.len() == map.rooms().len());
    }
}
