//! Regenerate `curation/generated/regions.toml` from the official mapdb.
//!
//! Kept in the tree because the file it writes is 345KB of extracted data
//! and a generated file nobody can reproduce is one nobody can trust. The
//! mapdb itself lives outside the repo (it is 21MB and not ours), so this
//! is the record of exactly what was taken from it.

#![allow(
    clippy::expect_used,
    reason = "a one-shot generator run by hand from the repo root: a missing rooms.json is a mistyped command"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

const SOURCE: &str = "E:/Cena/reference/mapdb/map-data/prime/rooms.json";

#[derive(serde::Deserialize)]
struct Db {
    rooms: Vec<DbRoom>,
}

#[derive(serde::Deserialize)]
struct DbRoom {
    id: i64,
    #[serde(default)]
    loc: Option<String>,
}

fn main() {
    let raw = std::fs::read_to_string(SOURCE).expect("rooms.json");
    let db: Db = serde_json::from_str(&raw).expect("parse mapdb");

    let mut by_region: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for r in &db.rooms {
        if let Some(l) = &r.loc {
            by_region.entry(l.clone()).or_default().push(r.id);
        }
    }
    let total: usize = by_region.values().map(Vec::len).sum();

    let mut out = String::new();
    let _ = write!(out, "{HEADER}");
    let _ = writeln!(out, "# {} regions, {total} uids.\n", by_region.len());

    for (name, ids) in &mut by_region {
        ids.sort_unstable();
        let _ = writeln!(out, "[[region]]");
        let _ = writeln!(out, "name = {}", toml_string(name));
        let _ = writeln!(out, "# {} rooms", ids.len());
        let _ = writeln!(out, "uids = [");
        for chunk in ids.chunks(12) {
            let line: Vec<String> = chunk.iter().map(i64::to_string).collect();
            let _ = writeln!(out, "  {},", line.join(", "));
        }
        let _ = writeln!(out, "]\n");
    }

    std::fs::write("curation/generated/regions.toml", &out).expect("write regions.toml");
    println!("{total} uids in {} regions", by_region.len());
}

/// TOML basic string. Region names carry apostrophes (`Zul Logoth /
/// Kharag 'doth Dzulthu`) and slashes, so quoting is not optional.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

const HEADER: &str = "\
# uid -> region, extracted from the official mapdb.
#
# Source: E:/Cena/reference/mapdb/map-data/prime/rooms.json,
# Simutronics' own room database, snapshot 2025-09-06.
#
# GENERATED. Do not hand-edit: regenerate with
#   cargo run -p cena-map-layout --example dump_regions
#
# This is the `loc` field, which is NOT the same question as gs.map's
# `location`. `location` is what the game's `location` verb replies -- a
# place, at whatever granularity the verb happens to use, with the prose
# the verb uses (`the town of Wehnimer's Landing`, `the free port of
# Solhaven`). `loc` is an administrative region a GM assigned, one string
# for a whole area.
#
# Measured against gs.map: of 21,837 rooms where both have a value, they
# agree on 4,481 and differ on 17,356 -- but almost none of that is a
# contradiction. It is the two fields answering different questions:
#
#   Solhaven / Vornavis  <-  Solhaven, the free port of Solhaven,
#                            the Vornavian Coast, the southern part of
#                            Solhaven, the plains of Vornavis, Vornavis,
#                            the Outlands, the town of Solhaven
#
# That fan-in is the `region` tier docs/room-classification.md wants and
# nothing in the map had.
#
# Three things to know before trusting it:
#
#   * The snapshot is a year old. It was AHEAD of us on Bayview Villa,
#     which gs.map still reaches by a `go pathway` edge the game has
#     removed -- but staleness cuts both ways.
#
#   * A bare `Teras`, `Mist Harbor`, `Solhaven`, `River's Rest` or `Zul
#     Logoth` is NOT the town. Each is exactly 64 rooms, every one titled
#     `Elemental Confluence`, in its own uid block (583xxx, 588xxx,
#     584xxx, 585xxx, 586xxx): one instanced feature replicated per town.
#     The town is in the compound name.
#
#   * The slash does NOT encode a hierarchy, and splitting on it to
#     manufacture a `region / area` tier would be wrong in both
#     directions:
#
#         Cold River / The Hinterwilds     town   / wilderness around it
#         Solhaven / Vornavis              town   / barony
#         Mist Harbor / Four Winds Isle    town   / isle
#         Teras / Kharam Dzu               island / town      <- inverted
#         Ta'Vaalor/Ta'Illistim Transport  a route BETWEEN two cities
#
#     It reads as two names for one place -- the town and the larger thing
#     it sits in, or a common name and a dwarven one (`Zul Logoth /
#     Kharag 'doth Dzulthu`) -- written together because either might be
#     what someone calls it. Only 12 of the 72 have a slash at all, and
#     the largest regions (`Ta'Illistim` 4,139, `Icemule Trace` 2,631,
#     `Wehnimer's Landing` 2,346) have none, so a split would leave them
#     with no area.
#
# What IS reliable is the prefix taxonomy: `QUEST:` (9 regions), `EVENT:`
# (4) and `... Transport` (7) are Simutronics labelling its own seasonal
# and transit content, which agrees independently with the disposition
# calls already in curation/status.toml.
#
# Written as `meta:region:<name>` while the two are compared. If `loc`
# wins it becomes a real field on Room; if it loses, deleting a meta pass
# costs nothing -- which is the whole reason it is not a field yet.
#
";
