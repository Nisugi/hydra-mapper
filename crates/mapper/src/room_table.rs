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

use crate::overrides::{MapOverrides, RoomKey};

const HEADER: &str = "room_id\tuid\ttitle\tlocation\tarea\tarea_key\tregion\tregion_src\tstatus";

/// The table, header first, in map order.
///
/// - `area` is the curated area the store puts the room in (its display
///   name; `area_key` is the store's key for it).
/// - `region` is a person's assignment when the store has one
///   (`region_src` = `assigned`), else the map's `meta region:`
///   (`mapdb`, or `inferred` where `retag` spread it into unregioned
///   ground).
/// - `status` is `live`, or the map's verdict: `gone`, `closed`, `virtual`.
#[must_use]
pub fn areas_tsv(map: &Map, store: &MapOverrides) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for room in map.rooms() {
        let key = RoomKey::of(room.id, map);
        let (area, area_key) = store
            .area_moves
            .get(&key)
            .map_or(("", ""), |k| (store.area_name(k), k.as_str()));
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

        let tsv = areas_tsv(&map, &store);
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
