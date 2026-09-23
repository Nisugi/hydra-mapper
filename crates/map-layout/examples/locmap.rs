//! For each gs.map `location`, which official region do its uid-joined
//! rooms actually carry?
//!
//! # Its `CLEAR` verdict is NOT safe to apply
//!
//! Kept as a measurement, not a proposal. The verdict counts agreement
//! among rooms that HAVE a region and cannot see that those rooms may be
//! a different kind of room from the ones that do not.
//!
//! The Upper Trollfang is the case that proves it. 120 of our rooms there
//! join to `QUEST: Black Swan Castle` and agree unanimously, so this
//! prints CLEAR and offers to fill 293 more. But the mapdb ALSO holds 286
//! of those rooms with `loc` deliberately empty -- uids 15001-15036,
//! titled `[Upper Trollfang]` -- while the 120 are uids 55001+, titled
//! `[Meadow, Ruins]` and the like. Two uid blocks: a quest staged inside a
//! hunting ground. Filling would have relabelled the hunting ground as
//! event content and destroyed exactly the distinction the region tag was
//! added to make visible.
//!
//! rooms.json populates `loc` on only 37,372 of its 47,956 rooms. An
//! empty `loc` is frequently an answer -- "this is wilderness, it has no
//! administrative region" -- and reading it as a gap to be filled is the
//! mistake this file is a record of.
//!
//! What it is still good for: seeing which of our location names
//! correspond to which official region, which is how `Kharam-Dzu` and
//! `Teras / Kharam Dzu` were shown to be the same place.
#![allow(clippy::expect_used, reason = "hand-run probe")]
use std::collections::BTreeMap;

fn main() {
    let bytes = std::fs::read("gs.map").expect("gs.map");
    let map = cena_map::binary::decode(&bytes).expect("decode");

    // location -> region -> count, over rooms that have both.
    let mut votes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    // location -> how many rooms have NO region (the ones a fill would reach)
    let mut unfilled: BTreeMap<String, usize> = BTreeMap::new();

    for r in map.rooms() {
        let Some(loc) = &r.location else { continue };
        match r.meta.iter().find_map(|m| m.strip_prefix("region:")) {
            Some(reg) => {
                *votes
                    .entry(loc.clone())
                    .or_default()
                    .entry(reg.to_owned())
                    .or_default() += 1
            }
            None => *unfilled.entry(loc.clone()).or_default() += 1,
        }
    }

    let (mut unanimous, mut split, mut noevidence) = (0usize, 0usize, 0usize);
    let (mut fill_u, mut fill_s, mut fill_n) = (0usize, 0usize, 0usize);
    let mut rows: Vec<(usize, String)> = Vec::new();

    for (loc, need) in &unfilled {
        match votes.get(loc) {
            None => {
                noevidence += 1;
                fill_n += need;
                rows.push((*need, format!("NONE\t{need}\t{loc}\t")));
            }
            Some(v) => {
                let total: usize = v.values().sum();
                let mut best: Vec<_> = v.iter().collect();
                best.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
                let (top, n) = (best[0].0, *best[0].1);
                let share = 100 * n / total;
                if v.len() == 1 || share >= 90 {
                    unanimous += 1;
                    fill_u += need;
                    rows.push((
                        *need,
                        format!("CLEAR\t{need}\t{loc}\t{top} ({share}% of {total})"),
                    ));
                } else {
                    split += 1;
                    fill_s += need;
                    let alts: Vec<String> = best
                        .iter()
                        .take(3)
                        .map(|(r, c)| format!("{r} ({c})"))
                        .collect();
                    rows.push((*need, format!("SPLIT\t{need}\t{loc}\t{}", alts.join("; "))));
                }
            }
        }
    }
    println!("locations with rooms lacking a region: {}", unfilled.len());
    println!(
        "  CLEAR  (one region, or >=90%) : {unanimous:4} locations, would fill {fill_u} rooms"
    );
    println!("  SPLIT  (genuinely mixed)      : {split:4} locations, {fill_s} rooms");
    println!("  NONE   (no joined room at all): {noevidence:4} locations, {fill_n} rooms");

    rows.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    let mut out = String::from("verdict\trooms\tlocation\tevidence\n");
    for (_, l) in &rows {
        out.push_str(l);
        out.push('\n');
    }
    std::fs::write("analysis/location-to-region.tsv", &out).expect("write");
    println!("\ntop 30:");
    for (_, l) in rows.iter().take(30) {
        println!("  {l}");
    }
}
