//! Regenerate the `official layout` rows of `crates/mapper/data/areas.tsv`
//! from Simutronics' own `layouts.json`.
//!
//! # Why this exists
//!
//! `areas.tsv` was produced once by `research/jev-trial/areas.py`, which
//! is not in this repo, and has been the authority for what an official
//! area contains ever since. It is not the authority: it is a JOIN of
//! `layouts.json` onto a gs.map that has since changed.
//!
//! The two are in different id spaces, which is the whole reason the
//! file exists. `layouts.json` positions rooms by the game's uid;
//! `areas.tsv` lists gs.map ids. Regenerating means redoing that join
//! against the map as it stands now.
//!
//! What that fixes:
//!
//!   * 123 layouts against 117 rows. Five of the six missing ones join
//!     and are added: `icemule-trace-ranger-guild`,
//!     `open-sea-glaeveln`, `special-night-at-the-academy`,
//!     `special-rumor-woods` and `special-troubled-waters`. The sixth,
//!     `reim-settlement`, cannot -- see below.
//!   * 82 rooms had been added to 13 existing areas, and nothing was
//!     removed from any.
//!
//! # What is NOT touched
//!
//! The 191 `mapdb location` rows. Those are grouped by OUR `location`
//! field and have nothing to do with `layouts.json`; they are copied
//! through unchanged.
//!
//! # The dropped columns
//!
//! `floors`, `floor_span`, `worst_confidence` and the five confidence
//! counts came from the Python script and nothing in this repo reads
//! them -- only `area`, `source` and `room_ids`. They are written empty
//! rather than removed, so the header stays the shape every existing
//! reader expects.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "a one-shot generator run by hand from the repo root"
)]

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

const LAYOUTS: &str = "E:/Cena/reference/mapdb/map-data/prime/layouts.json";
const TSV: &str = "crates/mapper/data/areas.tsv";

#[derive(serde::Deserialize)]
struct Layouts {
    layouts: BTreeMap<String, Layout>,
}

#[derive(serde::Deserialize)]
struct Layout {
    #[serde(default)]
    pos: Vec<(i64, i64, i64)>,
}

fn main() {
    let raw = std::fs::read_to_string(LAYOUTS).expect("layouts.json");
    let layouts: Layouts = serde_json::from_str(&raw).expect("parse layouts");

    let bytes = std::fs::read("gs.map").expect("gs.map");
    let map = cena_map::binary::decode(&bytes).expect("decode");
    // uid -> our id. A room carries several uids (the Confluence carries
    // nine), so every one of them is a way in.
    let mut ours: HashMap<i64, u32> = HashMap::new();
    for room in map.rooms() {
        for uid in &room.uid {
            ours.insert(uid.0, room.id.0);
        }
    }

    // The name is everything before the `||` in the key; the rest is the
    // uid-range spec the game uses to decide membership, which we do not
    // need because `pos` already enumerates the rooms.
    let mut rows: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let (mut positioned, mut joined) = (0usize, 0usize);
    for (key, layout) in &layouts.layouts {
        let name = key.split("||").next().unwrap_or(key).to_owned();
        let ids = rows.entry(name).or_default();
        for (uid, _, _) in &layout.pos {
            positioned += 1;
            if let Some(id) = ours.get(uid) {
                joined += 1;
                ids.push(*id);
            }
        }
    }
    for ids in rows.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    // A layout whose every room is gone from gs.map would write a row
    // naming nothing, which reads as an area that exists and is empty.
    //
    // One does: `reim-settlement`. Its layout positions 147 rooms at
    // uids 7105604-7105790, and gs.map holds 155 Reim rooms numbered
    // -7105029, -7105031 and so on -- NEGATIVE, which `cena-map`
    // documents as the game's marker for a generated area. Simutronics
    // laid out the template; we recorded the instances. A uid join
    // cannot bridge that, and pretending otherwise would put an area in
    // the tree whose rooms nothing can find.
    rows.retain(|_, ids| !ids.is_empty());

    let old = std::fs::read_to_string(TSV).expect("areas.tsv");
    let mut lines = old.lines();
    let header = lines.next().expect("header");
    let columns: Vec<&str> = header.split('\t').collect();
    let at = |name: &str| columns.iter().position(|c| *c == name).expect("column");
    let (name_at, source_at, rooms_at, ids_at) =
        (at("area"), at("source"), at("rooms"), at("room_ids"));

    let mut out = String::new();
    let _ = writeln!(out, "{header}");
    let mut kept = 0usize;
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.get(source_at) == Some(&"official layout") {
            continue; // regenerated below
        }
        kept += 1;
        let _ = writeln!(out, "{line}");
    }
    for (name, ids) in &rows {
        let mut fields = vec![String::new(); columns.len()];
        fields[name_at] = name.clone();
        fields[source_at] = "official layout".to_owned();
        fields[rooms_at] = ids.len().to_string();
        fields[ids_at] = ids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "{}", fields.join("\t"));
    }
    std::fs::write(TSV, &out).expect("write areas.tsv");

    println!(
        "{} layouts, {positioned} positioned rooms, {joined} joined to gs.map",
        layouts.layouts.len()
    );
    println!("{} official rows written, {kept} other rows kept", rows.len());
}
