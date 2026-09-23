//! What would derive_areas' regions look like if they came from
//! `meta:region:` instead of munging `location`?
#![allow(clippy::expect_used, reason = "hand-run probe")]
use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let bytes = std::fs::read("gs.map").expect("gs.map");
    let map = cena_map::binary::decode(&bytes).expect("decode");

    let meta_region = |r: &cena_map::Room| {
        r.meta.iter().find_map(|m| m.strip_prefix("region:").map(str::to_owned))
    };

    let (mut has_meta, mut has_loc, mut gone) = (0, 0, 0);
    let mut by_meta: BTreeMap<String, usize> = BTreeMap::new();
    for r in map.rooms() {
        if meta_region(r).is_some() { has_meta += 1; }
        if r.location.is_some() { has_loc += 1; }
        if r.meta.iter().any(|m| m == "map:status:gone") { gone += 1; }
        if let Some(m) = meta_region(r) { *by_meta.entry(m).or_default() += 1; }
    }
    println!("rooms {}  with meta:region {}  with location {}  gone {}",
             map.rooms().len(), has_meta, has_loc, gone);

    // What derive_areas produces today.
    let areas = cena_map_layout::regions::derive_areas(&map);
    println!("\nderive_areas today: {} areas", areas.len());
    let mut by_derived: BTreeMap<String, usize> = BTreeMap::new();
    for a in &areas {
        *by_derived.entry(a.region.clone().unwrap_or_else(|| "(none)".into())).or_default() += a.rooms.len();
    }
    let mut v: Vec<_> = by_derived.iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!("its biggest regions:");
    for (r, n) in v.iter().take(12) { println!("  {n:6}  {r}"); }

    println!("\nmeta:region biggest:");
    let mut v: Vec<_> = by_meta.iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (r, n) in v.iter().take(12) { println!("  {n:6}  {r}"); }

    // The specific bug: does Wehnimer's still contain Talador rooms?
    println!("\n=== Wehnimer's Landing, as each source sees it ===");
    for a in &areas {
        if a.region.as_deref() == Some("Wehnimer's Landing") || a.name.contains("Wehnimer") {
            let mut locs: BTreeSet<&str> = BTreeSet::new();
            let mut g = 0;
            for id in &a.rooms {
                if let Some(r) = map.room(*id) {
                    if let Some(l) = &r.location { locs.insert(l); }
                    if r.meta.iter().any(|m| m == "map:status:gone") { g += 1; }
                }
            }
            println!("  derived area {:?}: {} rooms, {} locations, {} GONE",
                     a.name, a.rooms.len(), locs.len(), g);
            if locs.iter().any(|l| l.contains("Talador")) {
                println!("     ** still contains Talador-located rooms **");
            }
        }
    }
    let n: usize = map.rooms().iter()
        .filter(|r| meta_region(r).as_deref() == Some("Wehnimer's Landing")).count();
    println!("  meta:region Wehnimer's Landing: {n} rooms");
}
