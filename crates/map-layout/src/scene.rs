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
use crate::classifier::interior_clusters;
use crate::direction::{Dir, DirectionMap};
use crate::positioner::Cell;

/// Directional edges longer than this render as stubs, not lines.
pub const LONG_EDGE_CELLS: i32 = 8;
/// Connectors longer than this are not drawn at all.
pub const CONNECTOR_MAX_CELLS: i32 = 30;
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    pub min: Cell,
    pub max: Cell,
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
    for anchor in &layout.anchors {
        let Some(room) = map.room(anchor.room) else {
            continue;
        };
        scene.interiors.anchors.push(SceneAnchor {
            id: anchor.room,
            cell: anchor.cell,
            title: room.title.first().cloned().unwrap_or_default(),
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
        }
    }
}

/// Each sheet's drawn bounds, from its placed rooms' cells and echoes.
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
    }
}
