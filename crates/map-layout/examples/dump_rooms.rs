//! One row per room: where it is, what it is, and where that answer came
//! from. The export an outside consumer wants instead of a 21 MB binary.
//!
//! ```text
//! cargo run -p cena-map-layout --example dump_rooms > analysis/rooms.tsv
//! ```
//!
//! # Why a room table rather than `areas.tsv`
//!
//! `areas.tsv` is one row per AREA with a `room_ids` blob, carries no
//! region, and lists an area twice when two sources name it. A consumer
//! asking "what is room 8686" has to invert it first. This is the
//! transpose, with the fields a consumer actually asked for.
//!
//! # Provenance is a column, not a footnote
//!
//! Three of these fields are Simutronics' answers and two are ours, and
//! a reader who cannot tell them apart will treat a guess as a fact:
//!
//! - `region` is the official mapdb's `loc`, joined by uid.
//! - `region_src` says `mapdb` for those, and `inferred` for the 7,161
//!   rooms filled by the pass that reaches into unregioned ground. That
//!   pass is sound -- every path out of the room leads to one region --
//!   but it is still our inference and it is marked.
//! - `area` is Simutronics' own layout area where one covers the room,
//!   and `area_src` says which.
//! - `location` is the game's answer to the LOCATION verb, unedited
//!   except where `curation/locations.toml` records that the recorded
//!   one is stale.
//! - `status` is a curation verdict: absent means live.
//!
//! Empty means empty. A room with no region gets an empty column rather
//! than a guess, because 2,505 of them genuinely have none and that is
//! the honest answer.

use std::collections::{BTreeMap, HashMap};

use cena_map::RoomId;

/// Tab-separated, because a room title contains commas and quotes and a
/// consumer should not have to guess at an escaping convention.
const HEADER: &str = "room_id\tuids\ttitle\tlocation\tregion\tregion_src\tarea\tarea_src\tstatus\tevent\tsense\tdoors";

fn main() {
    let path = std::env::var("CENA_MAP").unwrap_or_else(|_| "gs.map".to_owned());
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("could not read {path}");
        return;
    };
    let Ok(map) = cena_map::binary::decode(&bytes) else {
        eprintln!("{path} is not a map");
        return;
    };

    // Official areas, from the file that joins layouts.json to our ids.
    let mut area_of: HashMap<u32, (String, String)> = HashMap::new();
    if let Ok(tsv) = std::fs::read_to_string("crates/mapper/data/areas.tsv") {
        for line in tsv.lines().skip(1) {
            let fields: Vec<&str> = line.split('\t').collect();
            let (Some(name), Some(source), Some(ids)) =
                (fields.first(), fields.get(1), fields.last())
            else {
                continue;
            };
            // An official layout area is hand-drawn and wins over a
            // location grouping that happens to cover the same room.
            let better = *source == "official layout";
            for id in ids.split_whitespace().filter_map(|x| x.parse::<u32>().ok()) {
                match area_of.get(&id) {
                    Some((_, existing)) if existing == "official layout" && !better => {}
                    _ => {
                        area_of.insert(id, ((*name).to_owned(), (*source).to_owned()));
                    }
                }
            }
        }
    }

    // Which rooms a door leads into, so a consumer can see a building's
    // entrances without re-deriving the classifier.
    let mut doors: BTreeMap<RoomId, usize> = BTreeMap::new();
    for room in map.rooms() {
        for exit in &room.exits {
            if let cena_map::Crossing::Command(cmd) = &exit.crossing
                && (cmd.starts_with("go ") || cmd == "out" || cmd == "in")
            {
                *doors.entry(exit.to).or_default() += 1;
            }
        }
    }

    println!("{HEADER}");
    for room in map.rooms() {
        let meta = |prefix: &str| -> String {
            room.meta
                .iter()
                .find_map(|m| m.strip_prefix(prefix))
                .unwrap_or("")
                .to_owned()
        };
        let region = meta("region:");
        let region_src = if region.is_empty() {
            String::new()
        } else if room.meta.iter().any(|m| m == "map:region-inferred") {
            "inferred".to_owned()
        } else {
            "mapdb".to_owned()
        };
        let (area, area_src) = area_of
            .get(&room.id.0)
            .cloned()
            .unwrap_or_else(|| (String::new(), String::new()));
        // Absent status means live; say so rather than leaving a reader
        // to wonder whether the column failed.
        let status = room
            .meta
            .iter()
            .find_map(|m| m.strip_prefix("map:status:"))
            .unwrap_or("live");
        let sense = match room.paths.first().map(String::as_str) {
            Some(p) if p.starts_with("Obvious exits") => "indoor",
            Some(p) if p.starts_with("Obvious paths") => "outdoor",
            _ => "",
        };
        println!(
            "{}\t{}\t{}\t{}\t{region}\t{region_src}\t{area}\t{area_src}\t{status}\t{}\t{sense}\t{}",
            room.id.0,
            room.uid
                .iter()
                .map(|u| u.0.to_string())
                .collect::<Vec<_>>()
                .join(","),
            room.title.first().map_or("", String::as_str),
            room.location.as_deref().unwrap_or(""),
            meta("event:"),
            doors.get(&room.id).copied().unwrap_or(0),
        );
    }
}
