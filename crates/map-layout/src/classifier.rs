//! Interior classification -- ported from `reference/VellumFE/src/core/
//! layout_engine/classifier.rs` (`interior-classifier.js` upstream of that,
//! spec §6).
//!
//! Primary signal: indoor rooms print "Obvious exits", outdoor rooms print
//! "Obvious paths" (99.5% coverage); a strict majority decides. Fallbacks: a
//! literal `out` exit leaving the component, weatherless rooms, and
//! propagation (a component reachable only through interiors is interior).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use cena_map::{Crossing, Map, Room, RoomId};

use crate::positioner::Group;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entrance {
    pub outdoor_room_id: RoomId,
    pub interior_room_id: RoomId,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub interior_groups: HashSet<usize>,
    /// interior group index -> doorway edges into it.
    pub entrances: HashMap<usize, Vec<Entrance>>,
    /// Outdoor rooms that host a doorway (get door markers).
    pub entrance_room_ids: HashSet<RoomId>,
}

#[must_use]
pub fn classify(groups: &[Group], map: &Map) -> Classification {
    let mut component_of: HashMap<RoomId, usize> = HashMap::new();
    for group in groups {
        for &id in &group.room_ids {
            component_of.insert(id, group.index);
        }
    }

    let mut interior: HashSet<usize> = HashSet::new();
    for group in groups {
        if is_interior_component(group, &component_of, map) {
            interior.insert(group.index);
        }
    }

    // Propagate interiority to a fixed point: rooms behind a second door
    // inside a building form their own component with no `out` of their
    // own.
    //
    // Propagation exists to fill in for components with NO paths/exits
    // signal of their own -- it must never overrule a decisive outdoor
    // majority. Player-shop boutique streets are the canonical case: 17
    // rooms all printing "Obvious paths", reachable only through the shops
    // they serve, are still streets.
    let mut decisive_outdoor: HashSet<usize> = HashSet::new();
    for group in groups {
        let mut indoor = 0usize;
        let mut outdoor = 0usize;
        for &room_id in &group.room_ids {
            if let Some(room) = map.room(room_id) {
                match room_sense(room) {
                    Sense::Indoor => indoor += 1,
                    Sense::Outdoor => outdoor += 1,
                    Sense::Unknown => {}
                }
            }
        }
        if outdoor > indoor && group.room_ids.len() > COURTYARD_ROOMS {
            decisive_outdoor.insert(group.index);
        }
    }
    let mut neighbor_sets: HashMap<usize, HashSet<usize>> = HashMap::new();
    for group in groups {
        let mut neighbors = HashSet::new();
        for &room_id in &group.room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                if let Some(&target_group) = component_of.get(&exit.to)
                    && target_group != group.index
                {
                    neighbors.insert(target_group);
                }
            }
        }
        neighbor_sets.insert(group.index, neighbors);
    }
    let mut changed = true;
    while changed {
        changed = false;
        for group in groups {
            if interior.contains(&group.index) || decisive_outdoor.contains(&group.index) {
                continue;
            }
            let neighbors = &neighbor_sets[&group.index];
            if neighbors.is_empty() {
                continue;
            }
            if neighbors.iter().all(|n| interior.contains(n)) {
                interior.insert(group.index);
                changed = true;
            }
        }
    }

    let (entrances, entrance_room_ids) = compute_entrances(groups, map, &interior);

    Classification {
        interior_groups: interior,
        entrances,
        entrance_room_ids,
    }
}

/// Entrances: every edge from an outdoor room into an interior component.
/// Factored out so classification overrides can recompute after flipping
/// groups between sheets.
fn compute_entrances(
    groups: &[Group],
    map: &Map,
    interior: &HashSet<usize>,
) -> (HashMap<usize, Vec<Entrance>>, HashSet<RoomId>) {
    let mut component_of: HashMap<RoomId, usize> = HashMap::new();
    for group in groups {
        for &id in &group.room_ids {
            component_of.insert(id, group.index);
        }
    }
    let mut entrances: HashMap<usize, Vec<Entrance>> = HashMap::new();
    let mut entrance_room_ids: HashSet<RoomId> = HashSet::new();
    for group in groups {
        if interior.contains(&group.index) {
            continue;
        }
        for &room_id in &group.room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                let Some(&target_group) = component_of.get(&exit.to) else {
                    continue;
                };
                if !interior.contains(&target_group) {
                    continue;
                }
                entrances.entry(target_group).or_default().push(Entrance {
                    outdoor_room_id: room_id,
                    interior_room_id: exit.to,
                });
                entrance_room_ids.insert(room_id);
            }
        }
    }
    (entrances, entrance_room_ids)
}

/// Recompute doorway markers from the current interior set -- for callers
/// that move groups between sheets after classification (the packer's
/// try-inline pass), mirroring what `apply_sheet_overrides` does for
/// curated flips.
pub fn recompute_entrances(classification: &mut Classification, groups: &[Group], map: &Map) {
    let (entrances, entrance_room_ids) =
        compute_entrances(groups, map, &classification.interior_groups);
    classification.entrances = entrances;
    classification.entrance_room_ids = entrance_room_ids;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sense {
    Indoor,
    Outdoor,
    Unknown,
}

/// "Obvious exits" = indoor, "Obvious paths" = outdoor. `cena_map::Room`
/// keeps every variant of the paths line (day/night, seasonal), not
/// Vellum's single joined string, so any variant naming one settles it; a
/// room whose variants disagree is `Unknown`, same as a room with neither
/// wording at all.
pub(crate) fn room_sense(room: &Room) -> Sense {
    let (mut indoor, mut outdoor) = (false, false);
    for line in &room.paths {
        let lowered = line.to_lowercase();
        indoor |= lowered.contains("obvious exits");
        outdoor |= lowered.contains("obvious paths");
    }
    match (indoor, outdoor) {
        (true, false) => Sense::Indoor,
        (false, true) => Sense::Outdoor,
        _ => Sense::Unknown,
    }
}

fn is_interior_component(group: &Group, component_of: &HashMap<RoomId, usize>, map: &Map) -> bool {
    let mut indoor = 0usize;
    let mut outdoor = 0usize;
    for &room_id in &group.room_ids {
        if let Some(room) = map.room(room_id) {
            match room_sense(room) {
                Sense::Indoor => indoor += 1,
                Sense::Outdoor => outdoor += 1,
                Sense::Unknown => {}
            }
        }
    }
    if indoor != outdoor && indoor + outdoor > 0 {
        return indoor > outdoor;
    }

    // No usable paths data -- structural fallbacks.
    let mut weatherless = 0usize;
    for &room_id in &group.room_ids {
        let Some(room) = map.room(room_id) else {
            continue;
        };
        for exit in &room.exits {
            let Crossing::Command(command) = &exit.crossing else {
                continue;
            };
            if command.trim().to_lowercase() != "out" {
                continue;
            }
            // `out` is only a doorway when it LEAVES this component. An
            // `out` that stays inside means the component contains its own
            // outdoors (grottos off a beach) -- not a building.
            if component_of.get(&exit.to) != Some(&group.index) {
                return true;
            }
        }
        if room.climate.as_deref() == Some("none") && room.terrain.as_deref() == Some("none") {
            weatherless += 1;
        }
    }
    weatherless > 0 && weatherless == group.room_ids.len()
}

/// A component this large is a zone (catacombs, sewers, castle floors), not
/// a room of somebody's building: it gets its own cluster and never welds
/// its neighbors (Wehnimer's underground touches half the town's cellars --
/// without this line every shop with a trapdoor merges into one monster
/// "building"). Spec §10 lists proper large-interior splitting as future
/// work.
pub const ZONE_COMPONENT_ROOMS: usize = 50;

/// An outdoor component this small does not get to veto propagation: if
/// every way out of it leads into one building, it is that building's
/// courtyard -- a roofless room of it -- not a street.
///
/// **Why there is a size line at all.** Propagation must not overrule a
/// decisive outdoor majority (see `classify`), because player-shop
/// boutique streets are 17 rooms of "Obvious paths" reachable only
/// through the shops they serve, and they are streets. But the same veto
/// stranded the Dragonfly Den's Rainfall -- one open-air bath, reached
/// only by `go stairs` from the Den's Chamber and `go portal` from its
/// Cookery, printing "Obvious paths" because you can see the sky. Being
/// outdoor it never joined the Den's cluster; being a street with no
/// street it shelved alone, 87 cells from the building it belongs to.
///
/// A courtyard is small and enclosed; a street is neither. Two rooms is
/// the line: it takes the baths and the light wells, and leaves anything
/// with a shape of its own to the outdoor sheet.
pub const COURTYARD_ROOMS: usize = 2;

/// Interior clusters: interior groups connected by ANY exit between interior
/// rooms form one walkable interior space (one building) -- "go arch" joins
/// as surely as "north". Only edges that lead outdoors (or into a
/// zone-sized component) separate. Returns interior group index -> cluster
/// id (the smallest group index in the cluster), so ids are stable for a
/// given map build.
#[must_use]
pub fn interior_clusters(
    groups: &[Group],
    interior: &HashSet<usize>,
    map: &Map,
) -> HashMap<usize, usize> {
    let is_zone = |idx: usize| groups[idx].room_ids.len() > ZONE_COMPONENT_ROOMS;
    let mut group_of: HashMap<RoomId, usize> = HashMap::new();
    for group in groups {
        if interior.contains(&group.index) {
            for &id in &group.room_ids {
                group_of.insert(id, group.index);
            }
        }
    }

    // Union-find over interior group indices.
    let mut parent: HashMap<usize, usize> = interior.iter().map(|&g| (g, g)).collect();
    for group in groups {
        if !interior.contains(&group.index) || is_zone(group.index) {
            continue;
        }
        for &room_id in &group.room_ids {
            let Some(room) = map.room(room_id) else {
                continue;
            };
            for exit in &room.exits {
                let Some(&other) = group_of.get(&exit.to) else {
                    continue; // outdoors or outside the selection: a boundary
                };
                if other == group.index || is_zone(other) {
                    continue; // a zone is a neighbor, not a wing
                }
                let a = find(&mut parent, group.index);
                let b = find(&mut parent, other);
                if a != b {
                    // Root at the smaller index so cluster ids are
                    // canonical.
                    let (lo, hi) = (a.min(b), a.max(b));
                    parent.insert(hi, lo);
                }
            }
        }
    }

    let keys: Vec<usize> = parent.keys().copied().collect();
    keys.into_iter()
        .map(|g| {
            let root = find(&mut parent, g);
            (g, root)
        })
        .collect()
}

/// Union-find root, with path compression. Module-scope because
/// `interior_clusters` calls it repeatedly on a shared `parent` map.
fn find(parent: &mut HashMap<usize, usize>, mut g: usize) -> usize {
    while parent[&g] != g {
        let up = parent[&parent[&g]];
        parent.insert(g, up);
        g = up;
    }
    g
}

/// "[Hamehela's Magic Shoppe]" / "[Manor House, Foyer]" -> building name:
/// the most common bracketed prefix among the group's room titles.
#[must_use]
pub fn building_name(group: &Group, map: &Map) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for &room_id in &group.room_ids {
        let title = map
            .room(room_id)
            .and_then(|r| r.title.first())
            .map_or("", String::as_str);
        let Some(rest) = title.strip_prefix('[') else {
            continue;
        };
        let end = rest.find([',', ']']).unwrap_or(rest.len());
        let name = rest[..end].trim();
        if name.is_empty() {
            continue;
        }
        if let Some(entry) = counts.iter_mut().find(|(n, _)| n == name) {
            entry.1 += 1;
        } else {
            counts.push((name.to_owned(), 1));
        }
    }
    let mut best: Option<(String, usize)> = None;
    for (name, count) in counts {
        if best.as_ref().is_none_or(|(_, c)| count > *c) {
            best = Some((name, count));
        }
    }
    best.map(|(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Cost, Exit, ExitKind};

    fn room(id: u32, exits: &[(u32, &str)]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![],
            description: vec![],
            paths: vec![],
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
                    kind: ExitKind::Other,
                    crossing: Crossing::Command(cmd.to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    fn group(index: usize, room_ids: &[u32]) -> Group {
        Group {
            index,
            room_ids: room_ids.iter().copied().map(RoomId).collect(),
            positions: room_ids
                .iter()
                .enumerate()
                .map(|(i, &id)| {
                    (
                        RoomId(id),
                        crate::positioner::Cell {
                            x: i32::try_from(i).unwrap_or(i32::MAX),
                            y: 0,
                        },
                    )
                })
                .collect(),
            violations: vec![],
            base_offset: None,
            packing: None,
            name: None,
        }
    }

    fn sensed(id: u32, paths: &str, exits: &[(u32, &str)]) -> Room {
        Room {
            paths: vec![paths.to_owned()],
            ..room(id, exits)
        }
    }

    /// The Dragonfly Den's Rainfall is an open-air bath inside a
    /// building: "Obvious paths" because you can see the sky, but the
    /// only ways out are `go stairs` into the Den's Chamber and
    /// `go portal` into its Cookery. A roofless room of the building is
    /// still a room of the building -- it must not be left outdoors,
    /// with no street to sit on.
    ///
    /// Two rooms of boutique street are still street, though: that is
    /// what the size line protects.
    #[test]
    fn a_courtyard_a_building_encloses_is_part_of_it() {
        let rooms = vec![
            sensed(1, "Obvious exits: none", &[(2, "go stairs")]),
            sensed(
                2,
                "Obvious paths: none",
                &[(1, "go stairs"), (3, "go portal")],
            ),
            sensed(3, "Obvious exits: none", &[(2, "go portal")]),
        ];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let groups = vec![group(0, &[1]), group(1, &[2]), group(2, &[3])];

        let out = classify(&groups, &map);
        assert!(
            out.interior_groups.contains(&1),
            "the bath was left outdoors, with no street to sit on"
        );
    }

    /// The veto the size line protects: a boutique street is reachable
    /// only through the shops it serves and prints "Obvious paths", but
    /// it is a street and belongs on the outdoor sheet.
    #[test]
    fn a_boutique_street_is_still_a_street() {
        let mut rooms = vec![sensed(1, "Obvious exits: none", &[(2, "go door")])];
        // Four rooms of street, past COURTYARD_ROOMS, reached only
        // through the shop.
        for i in 2..=5u32 {
            let mut exits: Vec<(u32, &str)> = vec![];
            if i > 2 {
                exits.push((i - 1, "west"));
            }
            if i < 5 {
                exits.push((i + 1, "east"));
            }
            if i == 2 {
                exits.push((1, "go door"));
            }
            rooms.push(sensed(i, "Obvious paths: east, west", &exits));
        }
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let groups = vec![group(0, &[1]), group(1, &[2, 3, 4, 5])];

        let out = classify(&groups, &map);
        assert!(
            !out.interior_groups.contains(&1),
            "a boutique street was swallowed by the shop it serves"
        );
    }

    /// The bank: 3850+3672 are one directional component, 3670 hangs off
    /// 3672 via "go arch" (directionless, so its own component), and 3672
    /// leads outside via "out" to 3669. All three interior rooms are ONE
    /// cluster; the outdoor room never joins.
    #[test]
    fn go_arch_joins_a_building_but_out_does_not() {
        let rooms = vec![
            room(3669, &[(3672, "go bank")]), // outdoors
            room(3670, &[(3672, "go arch")]), // teller cage
            room(3672, &[(3850, "north"), (3670, "go arch"), (3669, "out")]),
            room(3850, &[(3672, "south")]),
        ];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let groups = vec![
            group(0, &[3672, 3850]), // directional pair
            group(1, &[3670]),       // reached only via "go arch"
            group(2, &[3669]),       // the street outside
        ];
        let interior: HashSet<usize> = [0usize, 1].into_iter().collect();

        let clusters = interior_clusters(&groups, &interior, &map);
        assert_eq!(clusters.len(), 2, "both interior groups get a cluster id");
        assert_eq!(
            clusters[&0], clusters[&1],
            "'go arch' must join the bank's sub-groups into one building"
        );
        assert!(
            !clusters.contains_key(&2),
            "the outdoor group is not part of any interior cluster"
        );
    }
}
