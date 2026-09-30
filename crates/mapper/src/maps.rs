//! Maps: curated groups of areas, each laid out as one sheet (the author,
//! 2026-09-29: *"I think the solution is for cold river to end up in the
//! hinterwilds area, and not be a separate area. Areas have sub areas
//! hmm?"*). The areas are Simutronics' own splits and stay as they are,
//! the map's sub-areas; a map is only a name each of its areas carries
//! ([`MapOverrides::area_maps`]), baked into `gs.map` as `meta:hydramap:` by
//! `retag`, and read back by [`cena_map_layout::areas::baked`].
//!
//! No field in the data says which areas make one map. The region spans
//! caravans (Icemule Trace holds the Hinterwilds and Icemule itself), and
//! `location` is noise across areas: 89 of 318 locations in
//! `gs.overrides.areas.tsv` span more than one. So maps are chosen by a
//! person, and [`suggest`] offers the candidates: areas a walk joins whose
//! rooms mostly name the same location.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use cena_map::{Map, RoomId};

use crate::areas::{Area, AreaKind};
use crate::overrides::{Baseline, MapOverrides, RoomKey};

/// Every map with rooms on it, by name: the rooms of all its areas.
pub fn map_areas(map: &Map, store: &MapOverrides, baseline: &Baseline) -> Vec<Area> {
    let mut by_map: BTreeMap<&str, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        let on = store
            .area_of(RoomKey::of(room.id, map), baseline)
            .and_then(|area| store.map_of_area(area, baseline));
        if let Some(on) = on {
            by_map.entry(on).or_default().push(room.id);
        }
    }
    by_map
        .into_iter()
        .map(|(name, rooms)| Area {
            name: name.to_owned(),
            kind: AreaKind::Maps,
            parent: None,
            contested: None,
            rooms,
        })
        .collect()
}

/// Areas worth putting on one map: the name offered, and the areas' keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub areas: Vec<String>,
}

/// Areas on no map yet that a walk joins and whose rooms mostly name one
/// location, each group offered under that location's name. Cold River
/// and the Hinterwilds are the case it was made for: two official areas,
/// every room of both saying *the Hinterwilds*, joined at the Long Snow.
pub fn suggest(map: &Map, store: &MapOverrides, baseline: &Baseline) -> Vec<Suggestion> {
    let area_of = |id: RoomId| {
        store
            .area_of(RoomKey::of(id, map), baseline)
            .filter(|area| store.map_of_area(area, baseline).is_none())
    };
    // Each area's commonest location.
    let mut votes: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
    for room in map.rooms() {
        if let (Some(area), Some(location)) = (area_of(room.id), room.location.as_deref()) {
            *votes.entry(area).or_default().entry(location).or_default() += 1;
        }
    }
    let location: HashMap<&str, &str> = votes
        .into_iter()
        .filter_map(|(area, counts)| {
            let top = counts
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))?;
            Some((area, top.0))
        })
        .collect();

    // Areas a walk joins, one location between them.
    let mut parent: HashMap<&str, &str> = location.keys().map(|&a| (a, a)).collect();
    for room in map.rooms() {
        let Some(here) = area_of(room.id) else {
            continue;
        };
        for exit in room
            .exits
            .iter()
            .filter(|e| cena_map_layout::regions::is_passage(e))
        {
            let Some(there) = area_of(exit.to) else {
                continue;
            };
            if here != there && location.get(here) == location.get(there) {
                let (a, b) = (root(&mut parent, here), root(&mut parent, there));
                if a != b {
                    parent.insert(a.max(b), a.min(b));
                }
            }
        }
    }
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let areas: Vec<&str> = parent.keys().copied().collect();
    for area in areas {
        let top = root(&mut parent, area);
        groups.entry(top).or_default().push(area.to_owned());
    }
    let mut out: Vec<Suggestion> = groups
        .into_values()
        .filter(|areas| areas.len() > 1)
        .map(|mut areas| {
            areas.sort();
            Suggestion {
                name: slug(location[areas[0].as_str()]),
                areas,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.areas.cmp(&b.areas)));
    out
}

/// The group `at` is in, halving the path as it climbs.
fn root<'a>(parent: &mut HashMap<&'a str, &'a str>, mut at: &'a str) -> &'a str {
    while parent[at] != at {
        let up = parent[parent[at]];
        parent.insert(at, up);
        at = up;
    }
    at
}

/// `--suggest-maps`: every suggestion, a line each. `--accept-map`
/// (`accept` named): that one put on its map in the store beside the map
/// file, the store backed up first.
///
/// # Errors
///
/// When the map or the store will not load, no suggestion has that name,
/// or the store cannot be written.
pub fn suggest_headless(
    path: Option<&std::path::Path>,
    accept: Option<&str>,
) -> Result<String, String> {
    let map = crate::app::load_map(path).map_err(|p| p.to_string())?;
    let store_path = crate::overrides::store_path(path.ok_or("no map path")?);
    let mut store =
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?;
    let baseline = Baseline::of(&map);
    let offered = suggest(&map, &store, &baseline);
    let Some(name) = accept else {
        let mut out = format!("{} suggested\n", offered.len());
        for s in &offered {
            let titles: Vec<&str> = s
                .areas
                .iter()
                .map(|a| store.area_title(a, &baseline))
                .collect();
            let _ = writeln!(out, "{}: {}", s.name, titles.join(", "));
        }
        return Ok(out);
    };
    let chosen = offered
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("no suggestion is named {name}"))?;
    if store_path.exists() {
        std::fs::copy(&store_path, store_path.with_extension("json.bak"))
            .map_err(|e| format!("Could not back up {}: {e}", store_path.display()))?;
    }
    for area in &chosen.areas {
        store.set_area_map(area, Some(name));
    }
    store
        .save(&store_path)
        .map_err(|e| format!("Could not write {}: {e}", store_path.display()))?;
    Ok(format!(
        "{name}: {} areas put on it, in {}",
        chosen.areas.len(),
        store_path.display()
    ))
}

/// A location as a map's name, in the areas' own style:
/// `the Hinterwilds` -> `the-hinterwilds`.
fn slug(location: &str) -> String {
    let mut out = String::new();
    for ch in location.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') && ch != '\'' {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Cost, Crossing, Exit, ExitKind, Room};

    fn room(id: u32, area: &str, location: &str, to: &[u32]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![format!("[Room {id}]")],
            description: vec![],
            paths: vec!["Obvious paths: north".to_owned()],
            location: Some(location.to_owned()),
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![format!("area:{area}")],
            image: None,
            exits: to
                .iter()
                .map(|&t| Exit {
                    to: RoomId(t),
                    kind: ExitKind::Other,
                    crossing: Crossing::Command("north".to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    /// Cold River and the Hinterwilds, joined by a walk and both named
    /// the Hinterwilds, are offered as one map; Icemule, joined to Cold
    /// River only by a walk from a room of another name, is not.
    #[test]
    fn areas_a_walk_joins_under_one_location_are_offered() {
        let map = Map::from_rooms(vec![
            room(1, "cold-river", "the Hinterwilds", &[2]),
            room(2, "cold-river", "the Hinterwilds", &[3]),
            room(3, "hinterwilds", "the Hinterwilds", &[2]),
            room(4, "icemule", "the town of Icemule Trace", &[1]),
        ])
        .expect("unique ids");
        let store = MapOverrides::default();
        let baseline = Baseline::of(&map);
        let offered = suggest(&map, &store, &baseline);
        assert_eq!(
            offered,
            vec![Suggestion {
                name: "the-hinterwilds".to_owned(),
                areas: vec!["cold.river".to_owned(), "hinterwilds".to_owned()],
            }]
        );
    }

    /// Accepted, the two are one map with both areas' rooms, and are no
    /// longer offered.
    #[test]
    fn an_accepted_map_lists_its_areas_rooms_and_is_not_offered_again() {
        let map = Map::from_rooms(vec![
            room(1, "cold-river", "the Hinterwilds", &[2]),
            room(2, "hinterwilds", "the Hinterwilds", &[1]),
        ])
        .expect("unique ids");
        let mut store = MapOverrides::default();
        let baseline = Baseline::of(&map);
        for area in ["cold.river", "hinterwilds"] {
            store.set_area_map(area, Some("the-hinterwilds"));
        }
        let maps = map_areas(&map, &store, &baseline);
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].name, "the-hinterwilds");
        assert_eq!(maps[0].rooms, vec![RoomId(1), RoomId(2)]);
        assert!(suggest(&map, &store, &baseline).is_empty());
    }

    /// Simutronics' own `mapname:`, on 228 rooms of gs.map, is not a map
    /// of ours: an area whose rooms carry it is on no map.
    #[test]
    fn simutronics_mapname_is_not_a_map() {
        let mut r = room(1, "icemule", "the town of Icemule Trace", &[]);
        r.meta.push("mapname:Icemule Trace".to_owned());
        let map = Map::from_rooms(vec![r]).expect("unique ids");
        let baseline = Baseline::of(&map);
        assert!(map_areas(&map, &MapOverrides::default(), &baseline).is_empty());
    }

    /// On the Hinterwilds, whose river current is folded to dots, every
    /// room drawn is found by its number: the fold indexes them again.
    #[test]
    fn the_hinterwilds_rooms_are_found_after_the_fold() {
        let map = crate::app::load_map(Some(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../gs.map"
        ))))
        .expect("gs.map loads");
        let placeable = cena_map_layout::regions::placeable_rooms(&map);
        let rooms = cena_map_layout::areas::layout_rooms(
            &cena_map_layout::areas::baked(&map)["the-hinterwilds"],
            &map,
            &placeable,
        );
        let subset = Map::from_rooms(rooms).expect("a subset");
        let layout = cena_map_layout::generate_layout(&subset);
        let scene = cena_map_layout::build_scene("the-hinterwilds", &layout, &subset);
        assert!(scene.room(RoomId(30115)).is_none(), "the current is folded");
        for room in &scene.sheet.rooms {
            assert_eq!(scene.room(room.id).map(|r| r.id), Some(room.id));
        }
    }

    #[test]
    fn a_location_is_slugged_as_the_areas_are() {
        assert_eq!(slug("the Hinterwilds"), "the-hinterwilds");
        assert_eq!(slug("Wehnimer's Landing"), "wehnimers-landing");
    }
}
