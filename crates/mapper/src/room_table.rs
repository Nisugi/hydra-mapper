//! One row per room: the area and the region it is assigned to.
//!
//! The export a person sorting the map wants to read or hand on: which
//! place each room is in, by the curation as it stands in the store,
//! without opening the mapper. Tab-separated, because titles hold commas
//! and quotes.
//!
//! **Every room is listed**, gone and virtual ones included, with a
//! `status` column, so a room missing from an area is visibly missing
//! rather than silently absent. Empty means unassigned.

use std::fmt::Write as _;

use cena_map::Map;

use crate::overrides::{Baseline, MapOverrides, RoomKey};

const HEADER: &str = "room_id\tuid\ttitle\tlocation\tarea\tarea_key\tregion\tregion_src\tstatus";

/// The table, header first, in map order.
///
/// - `area` is the room's curated area -- the store's word where it has
///   one, else the one baked into the map -- by name; `area_key` is its
///   key.
/// - `region` is a person's assignment when the store has one
///   (`region_src` = `assigned`), else the map's `meta region:`
///   (`mapdb`, or `inferred` where `retag` spread it into unregioned
///   ground).
/// - `status` is `live`, or the map's verdict: `gone`, `closed`, `virtual`.
#[must_use]
pub fn areas_tsv(map: &Map, store: &MapOverrides, baseline: &Baseline) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for room in map.rooms() {
        let key = RoomKey::of(room.id, map);
        let (area, area_key) = store
            .area_of(key, baseline)
            .map_or(("", ""), |k| (store.area_title(k, baseline), k));
        let meta = |prefix: &str| room.meta.iter().find_map(|m| m.strip_prefix(prefix));
        let (region, region_src) = match store.region_of(key) {
            Some(region) => (region, "assigned"),
            None => match meta("region:") {
                Some(region) if room.meta.iter().any(|m| m == "map:region-inferred") => {
                    (region, "inferred")
                }
                Some(region) => (region, "mapdb"),
                None => ("", ""),
            },
        };
        let status = if room.meta.iter().any(|m| m == "map:virtual room") {
            "virtual"
        } else {
            meta("map:status:").unwrap_or("live")
        };
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            room.id.0,
            room.uid.first().map_or(String::new(), |u| u.0.to_string()),
            clean(room.title.first().map_or("", String::as_str)),
            clean(room.location.as_deref().unwrap_or("")),
            clean(area),
            clean(area_key),
            clean(region),
            region_src,
            status,
        );
    }
    out
}

/// A field with no tab or newline in it, so every row stays one row.
fn clean(field: &str) -> String {
    field.replace(['\t', '\n', '\r'], " ")
}

/// `curation/assignments.toml`: every curated area with its rooms, and
/// every region a person assigned, for `retag` to bake into the map.
///
/// **The whole curation, not the store alone.** A room's area is what the
/// map carries, overridden by the store: so the file stays complete from
/// a store holding only the latest edits, and a room taken out of its area
/// is left out of every block, which is what makes `retag` drop it.
///
/// The whole file, every time, in a stable order -- areas by key, rooms
/// by uid then id -- so a re-export that changes nothing changes no line,
/// and a diff shows exactly the rooms that moved.
#[must_use]
pub fn assignments_toml(map: &Map, store: &MapOverrides, baseline: &Baseline) -> String {
    use std::collections::BTreeMap;

    const PREAMBLE: &str = "\
# Written by the mapper (Export to curation). Do not edit by hand:
# the next export replaces this file from the store.
#
# `retag` bakes it into gs.map: each room here gets `meta:area:<name>`,
# and `meta:mapname:<map>` when its area is on a map; each [[assign]]
# region outranks the mapdb join, the folds and the spread. Rooms are named by uid, or by id where the game never
# numbered them; ids do not survive a map rebuild.

";
    let mut out = String::from(PREAMBLE);
    let split = |keys: &mut Vec<RoomKey>| {
        keys.sort_unstable();
        let uids: Vec<i64> = keys
            .iter()
            .filter_map(|k| match k {
                RoomKey::Uid(u) => Some(*u),
                RoomKey::Id(_) => None,
            })
            .collect();
        let ids: Vec<u32> = keys
            .iter()
            .filter_map(|k| match k {
                RoomKey::Id(i) => Some(*i),
                RoomKey::Uid(_) => None,
            })
            .collect();
        (uids, ids)
    };
    let list = |out: &mut String, field: &str, values: Vec<String>| {
        if values.is_empty() {
            return;
        }
        let _ = writeln!(out, "{field} = [");
        for chunk in values.chunks(12) {
            let _ = writeln!(out, "  {},", chunk.join(", "));
        }
        let _ = writeln!(out, "]");
    };

    let mut by_area: BTreeMap<&str, Vec<RoomKey>> = BTreeMap::new();
    let mut seen: std::collections::HashSet<RoomKey> = std::collections::HashSet::new();
    for room in map.rooms() {
        let key = RoomKey::of(room.id, map);
        if !seen.insert(key) {
            continue;
        }
        if let Some(area) = store.area_of(key, baseline) {
            by_area.entry(area).or_default().push(key);
        }
    }
    for (area, keys) in &mut by_area {
        let (uids, ids) = split(keys);
        let _ = writeln!(out, "[[area]]");
        let _ = writeln!(out, "key = {area:?}");
        let _ = writeln!(out, "name = {:?}", store.area_title(area, baseline));
        if let Some(on) = store.map_of_area(area, baseline) {
            let _ = writeln!(out, "map = {on:?}");
        }
        let _ = writeln!(out, "# {} rooms", keys.len());
        list(
            &mut out,
            "uids",
            uids.iter().map(ToString::to_string).collect(),
        );
        list(
            &mut out,
            "ids",
            ids.iter().map(ToString::to_string).collect(),
        );
        out.push('\n');
    }

    let mut by_region: BTreeMap<&str, Vec<RoomKey>> = BTreeMap::new();
    for (key, region) in &store.region_moves {
        by_region.entry(region.as_str()).or_default().push(*key);
    }
    for (region, keys) in &mut by_region {
        let (uids, ids) = split(keys);
        let _ = writeln!(out, "[[assign]]");
        let _ = writeln!(out, "region = {region:?}");
        let _ = writeln!(out, "# {} rooms", keys.len());
        list(
            &mut out,
            "uids",
            uids.iter().map(ToString::to_string).collect(),
        );
        list(
            &mut out,
            "ids",
            ids.iter().map(ToString::to_string).collect(),
        );
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Room, RoomId, Uid};

    fn room(id: u32, uid: Option<i64>, meta: &[&str]) -> Room {
        Room {
            id: RoomId(id),
            uid: uid.map(Uid).into_iter().collect(),
            title: vec![format!("[Room {id}]")],
            description: vec![],
            paths: vec![],
            location: Some("the town of Somewhere".to_owned()),
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: meta.iter().map(|m| (*m).to_owned()).collect(),
            image: None,
            exits: vec![],
        }
    }

    #[test]
    fn a_row_says_the_area_and_where_the_region_came_from() {
        let map = Map::from_rooms(vec![
            room(1, Some(101), &["region:Wehnimer's Landing"]),
            room(
                2,
                Some(102),
                &["region:Wehnimer's Landing", "map:region-inferred"],
            ),
            room(3, None, &["map:status:gone"]),
            room(4, Some(104), &["region:Icemule Trace"]),
        ])
        .expect("no duplicate ids");
        let mut store = MapOverrides::default();
        store.set_area(RoomKey::Uid(101), Some("landing.town"));
        store.custom_areas.insert(
            "landing.town".to_owned(),
            crate::overrides::CuratedArea {
                name: "Landing Town".to_owned(),
            },
        );
        store.set_region(RoomKey::Uid(104), Some("Wehnimer's Landing"));

        let tsv = areas_tsv(&map, &store, &Baseline::of(&map));
        let rows: Vec<Vec<&str>> = tsv.lines().map(|l| l.split('\t').collect()).collect();
        assert_eq!(rows[0].join("\t"), HEADER);
        assert_eq!(rows.len(), 5, "one row per room, gone ones too");
        assert_eq!(
            rows[1][4..],
            [
                "Landing Town",
                "landing.town",
                "Wehnimer's Landing",
                "mapdb",
                "live"
            ]
        );
        assert_eq!(rows[2][6..], ["Wehnimer's Landing", "inferred", "live"]);
        assert_eq!(rows[3][4..], ["", "", "", "", "gone"]);
        assert_eq!(rows[4][6..8], ["Wehnimer's Landing", "assigned"]);
    }
}
