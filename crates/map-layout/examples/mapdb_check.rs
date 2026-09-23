//! Crosscheck the area-triage groups against the official mapdb.
//!
//! `E:/Cena/reference/mapdb/map-data/prime/rooms.json` is Simutronics' own
//! room database, about a year old, keyed by the game's uid. A gs.map room
//! with a uid that mapdb does not know, or with no uid at all, is a room
//! the game had stopped numbering -- the same evidence that settled
//! Talador, but from an independent source rather than from absence.
//!
//! Titles are matched too, because a room can be live under a uid we never
//! recorded.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "a one-shot probe run by hand from the repo root"
)]

use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(serde::Deserialize)]
struct Db {
    rooms: Vec<DbRoom>,
}

#[derive(serde::Deserialize)]
struct DbRoom {
    id: i64,
    t: String,
    #[serde(default)]
    loc: Option<String>,
}

fn main() {
    let raw = std::fs::read_to_string("E:/Cena/reference/mapdb/map-data/prime/rooms.json")
        .expect("rooms.json");
    let db: Db = serde_json::from_str(&raw).expect("parse mapdb");
    let db_uids: HashSet<i64> = db.rooms.iter().map(|r| r.id).collect();
    // Title -> how many mapdb rooms carry it. Titles in mapdb have no
    // brackets; gs.map titles do.
    let mut db_titles: HashMap<String, usize> = HashMap::new();
    for r in &db.rooms {
        *db_titles.entry(r.t.clone()).or_default() += 1;
    }
    let mut db_loc: HashMap<String, usize> = HashMap::new();
    for r in &db.rooms {
        if let Some(l) = &r.loc {
            *db_loc.entry(l.clone()).or_default() += 1;
        }
    }
    eprintln!(
        "mapdb: {} rooms, {} distinct titles, {} locations",
        db.rooms.len(),
        db_titles.len(),
        db_loc.len()
    );

    let raw = std::fs::read_to_string("C:/tmp/official.json").expect("official.json");
    let official: HashMap<String, Vec<u32>> = serde_json::from_str(&raw).expect("parse");
    let mut area_of: HashMap<u32, &str> = HashMap::new();
    for (name, ids) in &official {
        for id in ids {
            area_of.insert(*id, name.as_str());
        }
    }

    let bytes = std::fs::read("gs.map").expect("gs.map");
    let map = cena_map::binary::decode(&bytes).expect("decode");
    let dirs = cena_map_layout::DirectionMap::build(&map);
    let groups = cena_map_layout::positioner::position_rooms(&map, &dirs);

    let bare = |t: &str| t.trim_matches(['[', ']']).to_string();
    let prefix = |r: &cena_map::Room| {
        let t = r.title.first().cloned().unwrap_or_default();
        t.strip_prefix('[')
            .and_then(|x| x.split_once(", ").map(|(a, _)| a.to_string()))
            .unwrap_or_else(|| t.trim_matches(['[', ']']).to_string())
    };

    let mut out = String::from(HEADER);
    let mut rows: Vec<(usize, String)> = Vec::new();

    // Whole-map totals, for the denominator.
    let (mut tot, mut tot_nouid, mut tot_unknown) = (0usize, 0usize, 0usize);
    for r in map.rooms() {
        tot += 1;
        if r.uid.is_empty() {
            tot_nouid += 1;
        } else if !r.uid.iter().any(|u| db_uids.contains(&u.0)) {
            tot_unknown += 1;
        }
    }
    eprintln!(
        "gs.map: {tot} rooms, {tot_nouid} with no uid ({:.0}%), {tot_unknown} with a uid mapdb does not know ({:.0}%)",
        100.0 * tot_nouid as f64 / tot as f64,
        100.0 * tot_unknown as f64 / tot as f64
    );

    for g in &groups {
        if g.room_ids.iter().any(|id| area_of.contains_key(&id.0)) {
            continue;
        }
        let n = g.room_ids.len();
        if n < 6 {
            continue;
        }

        let (mut nouid, mut known, mut unknown) = (0, 0, 0);
        let (mut t_hit, mut t_miss) = (0, 0);
        let mut gone = 0;
        let mut pre: BTreeMap<String, usize> = BTreeMap::new();
        let mut loc: BTreeMap<String, usize> = BTreeMap::new();
        for id in &g.room_ids {
            let Some(r) = map.room(*id) else { continue };
            *pre.entry(prefix(r)).or_default() += 1;
            if let Some(l) = &r.location {
                *loc.entry(l.clone()).or_default() += 1;
            }
            if r.meta.iter().any(|m| m == "map:status:gone") {
                gone += 1;
            }
            if r.uid.is_empty() {
                nouid += 1;
            } else if r.uid.iter().any(|u| db_uids.contains(&u.0)) {
                known += 1;
            } else {
                unknown += 1;
            }
            // Title evidence, independent of uid.
            let t = r.title.first().cloned().unwrap_or_default();
            if db_titles.contains_key(&bare(&t)) {
                t_hit += 1;
            } else {
                t_miss += 1;
            }
        }

        let mut pv: Vec<_> = pre.iter().collect();
        pv.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let mut lv: Vec<_> = loc.iter().collect();
        lv.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let top = |v: &[(&String, &usize)], k: usize| {
            v.iter()
                .take(k)
                .map(|(a, c)| format!("{a} ({c})"))
                .collect::<Vec<_>>()
                .join("; ")
        };

        // The verdict this probe offers, on mapdb evidence ALONE.
        //
        // `absent` = the mapdb knows none of these rooms, by uid or by
        // title. A year-old official database that never heard of a group
        // is strong evidence the group is not in the game.
        //
        // `present` = the mapdb knows most of them: live, whatever else
        // the sheet guessed.
        // Two OPPOSITE things look alike if you only ask "does the mapdb
        // know this group": it knows neither a removed area nor one added
        // after the snapshot. The uid column tells them apart.
        //
        //   no uid at all  -> the game never numbered it. We map
        //                     exhaustively and repeatedly, so absence is a
        //                     FAILED REACH, not an unsampled one. Gone.
        //   uid the mapdb  -> the game numbered it and the database is
        //   does not know     simply older. Newer than the snapshot.
        //
        // Skyship is the case that forced the split: 72 rooms across five
        // groups, every one carrying a uid, not one of them in the mapdb.
        // Spitfire is the mirror -- 170 rooms, no uid anywhere, no title
        // hit -- and it is a flying ship that was removed.
        let verdict = if t_hit == 0 && known == 0 {
            if unknown > 0 && nouid == 0 {
                "newer"
            } else {
                "absent"
            }
        } else if known * 2 >= n || t_hit * 2 >= n {
            "present"
        } else {
            "partial"
        };

        rows.push((
            n,
            format!(
                "{verdict}\t{n}\t{gone}\t{known}\t{unknown}\t{nouid}\t{t_hit}\t{t_miss}\t{}\t{}\n",
                top(&lv, 2),
                top(&pv, 4)
            ),
        ));
    }

    rows.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    for (_, l) in &rows {
        out.push_str(l);
    }
    std::fs::write("analysis/mapdb-crosscheck.tsv", &out).expect("write");

    let mut by: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (n, l) in &rows {
        let v = l.split('\t').next().unwrap_or("");
        let e = by
            .entry(match v {
                "absent" => "absent",
                "newer" => "newer",
                "present" => "present",
                _ => "partial",
            })
            .or_default();
        e.0 += 1;
        e.1 += n;
    }
    println!(
        "\n{} groups, {} rooms",
        rows.len(),
        rows.iter().map(|(n, _)| n).sum::<usize>()
    );
    for (k, (c, r)) in &by {
        println!("  {k:8}  {c:4} groups  {r:6} rooms");
    }
}

const HEADER: &str = "\
# gs.map groups crosschecked against the official mapdb
# (E:/Cena/reference/mapdb/map-data/prime/rooms.json, ~1 year old).
# Regenerate: cargo run -p cena-map-layout --example mapdb_check
#
# Same groups as area-triage.tsv. `id` in the mapdb IS the game's uid.
#
#   absent  = mapdb knows none of these rooms AND the game never numbered
#             them (no uid). We map exhaustively and repeatedly, so a
#             missing uid is a FAILED REACH, not an unsampled room. Two
#             independent witnesses that the group is not in the game.
#   newer   = mapdb knows none of them, but every room HAS a uid the mapdb
#             lacks. The game numbered it; the database is just older.
#             Added after the snapshot. LIVE -- do not sweep these.
#   present = mapdb knows most of them. Live.
#   partial = mixed; read the columns.
#
# `absent` and `newer` are opposite findings that look identical if you
# only ask whether the mapdb knows the group. Skyship forced the
# distinction: 72 rooms, every one with a uid, none in the mapdb.
#
# `known`   uid is in the mapdb          `unknown` uid is NOT in the mapdb
# `no_uid`  gs.map has no uid at all     `t_hit`/`t_miss` title match
#
# A title match is weaker evidence than a uid -- titles repeat across
# towns -- but it catches a live room under a uid we never recorded.
#
verdict\trooms\tgone\tknown\tunknown\tno_uid\tt_hit\tt_miss\tlocations\tprefixes
";
