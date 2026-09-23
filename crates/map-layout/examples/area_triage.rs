//! Regenerate `analysis/area-triage.tsv` and the generated half of
//! `curation/areas.toml`.
//!
//! Kept in the tree rather than thrown away: the verdicts depend on
//! curation that keeps changing -- a room marked `gone` leaves the sheet,
//! a new Official area removes a whole group -- so this has to be re-run,
//! and a sheet nobody can reproduce is one nobody can trust.
//!
//! Expects `C:/tmp/official.json`: the `official layout` rows of
//! `crates/mapper/data/areas.tsv` as `{area: [room_id, ...]}`.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::format_push_string,
    reason = "a one-shot generator run by hand from the repo root: a               missing gs.map or official.json is a mistyped command, and               the panic says so more usefully than a Result would"
)]

use std::collections::{BTreeMap, HashMap, HashSet};

fn main() {
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

    let indoor = |r: &cena_map::Room| {
        r.paths
            .iter()
            .any(|p| p.to_lowercase().contains("obvious exits"))
    };
    let prefix = |r: &cena_map::Room| {
        let t = r.title.first().cloned().unwrap_or_default();
        t.strip_prefix('[')
            .and_then(|x| x.split_once(", ").map(|(a, _)| a.to_string()))
            .unwrap_or_else(|| t.trim_matches(['[', ']']).to_string())
    };

    let mut out = String::from(HEADER);
    let mut rows: Vec<(usize, String)> = Vec::new();
    let mut places = String::new();
    let mut builds = String::new();

    for g in &groups {
        if g.room_ids.iter().any(|id| area_of.contains_key(&id.0)) {
            continue;
        }
        let n = g.room_ids.len();
        if n < 6 {
            continue;
        }

        let (mut ind, mut outd) = (0, 0);
        let mut pre: BTreeMap<String, usize> = BTreeMap::new();
        let mut loc: BTreeMap<String, usize> = BTreeMap::new();
        let mut gone = 0;
        for id in &g.room_ids {
            if let Some(r) = map.room(*id) {
                if indoor(r) {
                    ind += 1;
                } else {
                    outd += 1;
                }
                *pre.entry(prefix(r)).or_default() += 1;
                if let Some(l) = &r.location {
                    *loc.entry(l.clone()).or_default() += 1;
                }
                if r.meta.iter().any(|m| m == "map:status:gone") {
                    gone += 1;
                }
            }
        }

        let inside: HashSet<u32> = g.room_ids.iter().map(|i| i.0).collect();
        let mut touch: BTreeMap<&str, usize> = BTreeMap::new();
        for id in &g.room_ids {
            if let Some(r) = map.room(*id) {
                for e in &r.exits {
                    if !inside.contains(&e.to.0)
                        && let Some(a) = area_of.get(&e.to.0)
                    {
                        *touch.entry(a).or_default() += 1;
                    }
                }
            }
        }
        let mut tv: Vec<_> = touch.into_iter().collect();
        tv.sort_by_key(|(_, c)| std::cmp::Reverse(*c));

        // A real front door: an indoor room here opening onto an outdoor
        // room of the area. Adjacency alone is not belonging.
        let mut doorways = 0;
        if let Some((parent, _)) = tv.first() {
            for id in &g.room_ids {
                let Some(r) = map.room(*id) else { continue };
                if !indoor(r) {
                    continue;
                }
                for e in &r.exits {
                    if inside.contains(&e.to.0) || area_of.get(&e.to.0) != Some(parent) {
                        continue;
                    }
                    if map.room(e.to).is_some_and(|t| !indoor(t)) {
                        doorways += 1;
                    }
                }
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
        let touches = tv
            .iter()
            .take(2)
            .map(|(a, c)| format!("{a} ({c})"))
            .collect::<Vec<_>>()
            .join("; ");

        // Prefixes of one or two rooms are furniture, not places: a CHE
        // house names every table in its lounge, so Cairnfang Manor is 64
        // rooms of 67 beside a Forsythia Table of one.
        let real = pv.iter().filter(|(_, c)| **c >= 3).count();
        let dominant = pv.first().is_some_and(|(_, c)| **c * 2 >= n);

        // ONLY `gone` is skipped, and only when MOST of the group is
        // gone. `closed` draws -- that is the whole distinction the
        // status carries -- so Rumor Woods still needs an area to be
        // drawn in when it opens in April.
        //
        // The majority matters because the grouping welds neighbours: the
        // 423-room Rumor Woods group also holds Summit Academy and
        // Briarmoon Cove, both retired, and one gone room tainting the
        // whole group hid a live festival.
        let verdict = if gone * 2 > n {
            "skip"
        } else if !tv.is_empty() && n <= 20 && doorways >= 1 {
            "building"
        } else if tv.is_empty() && n > 50 && (real < 2 || dominant) {
            "place"
        } else if real >= 3 && n > 50 && !dominant {
            "split"
        } else {
            ""
        };

        let mut ids: Vec<u32> = g.room_ids.iter().map(|i| i.0).collect();
        ids.sort_unstable();
        let idlist = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
        if verdict == "place" {
            places.push_str(&format!(
                "\n[[place]]\nname = {:?}\nrooms = {n}\nindoor = {ind}\noutdoor = {outd}\nroom_ids = \"{idlist}\"\n",
                pv[0].0
            ));
        } else if verdict == "building" {
            builds.push_str(&format!(
                "\n[[building]]\nparent = {:?}\nrooms = {n}\nnames = {:?}\nroom_ids = \"{idlist}\"\n",
                tv[0].0,
                top(&pv, 3)
            ));
        }
        rows.push((
            n,
            format!(
                "{verdict}\t{n}\t{ind}\t{outd}\t{doorways}\t{touches}\t{}\t{}\n",
                top(&lv, 2),
                top(&pv, 5)
            ),
        ));
    }

    rows.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    for (_, l) in &rows {
        out.push_str(l);
    }
    std::fs::write("analysis/area-triage.tsv", &out).expect("write sheet");
    std::fs::write("C:/tmp/places.toml", &places).expect("write places");
    std::fs::write("C:/tmp/buildings.toml", &builds).expect("write buildings");
    println!(
        "{} groups, {} rooms",
        rows.len(),
        rows.iter().map(|(n, _)| n).sum::<usize>()
    );
}

const HEADER: &str = "\
# Room groups the Official 117 areas do not cover.
# Regenerate with: cargo run -p cena-map-layout --example area_triage
#
# Fill `verdict`: place | building | split      (blank = undecided)
#   place    = ONE area, kept whole. The group is a single place, and any
#              stray prefixes are its furniture.
#   building = rooms of a building; attaches to the area named in `touches`.
#   split    = several places welded into one group by the interior-doorway
#              grouping. NOT an assignment: it needs breaking up first, and
#              the pieces are usually a MIX rather than peer areas -- the
#              192-room group holding Ta'Illistim Keep and the Moonglae Inn
#              is most likely one place and one building of taillistim-town.
#              Prefix is the visible signal but not a reliable rule: the
#              prefix-pieces were each internally connected in only 19 of
#              the 45 groups tried.
#   skip     = GONE. Removed from the game and never drawn, so it needs no
#              area at all. `closed` is NOT skipped: closed means draw but
#              do not route, so Rumor Woods still needs an area to be drawn
#              in when it opens in April.
#
# The `place` and `building` rows are already in curation/areas.toml.
#
# `doorway` counts real front doors into `touches`: an indoor room here
# opening onto an outdoor room there. A `building` needs at least one.
# Adjacency alone is not belonging -- a Krolvin Warship touches
# wehnimers-landing-danjirland by one exit, has no doorway, and is an Open
# Sea Adventures ship instance rather than a building in Danjirland.
#
# `prefixes` of one or two rooms are ignored when deciding `split`.
#
verdict\trooms\tindoor\toutdoor\tdoorway\ttouches\tlocations\tprefixes
";
