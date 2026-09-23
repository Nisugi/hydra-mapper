//! Which rooms someone can walk to.
//!
//! The one automatic check on curation: a room you can reach is live,
//! whatever a rule claims. It is a veto, not a generator -- being
//! unreachable does not make a room `gone`, because `closed` looks
//! identical in the graph and the difference is a fact about the world.
//!
//! # "Someone", not "everyone"
//!
//! Gates are ignored: profession, race, citizenship, CHE, society,
//! premium. A room behind the Voln door is reachable, because a Voln
//! member can walk there. Access is a property of the edge and the
//! character, which `Cost::Gated` and `tags.lic` already model per walker;
//! folding it in here would answer a different question and answer it
//! identically for everyone, which is the failure the room-classification
//! doc warns about.
//!
//! A ticket price is the same kind of thing. The Chronomage transport hub
//! costs 100,000 silvers in Prime and nothing in Shattered, and is live in
//! both -- so cost cannot bear on reachability either.
//!
//! # Seasonal edges
//!
//! `event transport duskruin` and `event transport ebon gate` are in the
//! map, but the command only works while that event is running, and
//! events run in even months for about 21 days. Following them would make
//! 1,378 Duskruin, Ebon Gate and Evermore Hollow rooms look live all year
//! and would veto every correct `closed` rule written about them.
//!
//! This is the one place the tool knows something the map does not state.
//! It is spelled as a prefix test on the crossing text rather than a room
//! list, so a new event area added with the same verb is handled without
//! an edit here.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cena_map::{Map, Room};

/// Title prefixes of the rooms a walk starts from.
///
/// Town squares and equivalents. Deliberately not derived from the
/// largest connected component, which sounds more principled and is not:
/// the component is only "the live world" if you already know Caligos
/// Isle is not in it, and one relocated sub-area with a stale edge is
/// enough to pull a sunk island back in.
const TOWN_CENTRES: &[&str] = &[
    "[Town Square",
    "[Icemule Trace, Town Center",
    "[Ta'Illistim, Hanging Gardens",
    "[Ta'Vaalor, Court Plaza",
    "[Solhaven, Bay",
    "[Kharam-Dzu, Town Square",
    "[River's Rest, Town Square",
    "[Zul Logoth, Tunnel Square",
    "[Mist Harbor, Sun Dial",
    "[Cysaegir, Town",
    "[Kraken's Fall, Town Square",
];

/// An edge that only works while a paid event is running.
fn is_seasonal(crossing: &str) -> bool {
    crossing.to_lowercase().contains("event transport")
}

/// Every room reachable on foot from a town centre.
///
/// Directed: an exit is followed the way it points. A room you can only
/// walk *out* of is not reachable, which is the honest reading -- the
/// map's one-way edges are frequently stale, and treating them as
/// bidirectional is what let the Arena of the Abyss drag 674 sunk Caligos
/// rooms into the live set.
#[must_use]
pub fn walkable(map: &Map) -> BTreeSet<u32> {
    let index: BTreeMap<u32, usize> = map
        .rooms()
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.0, i))
        .collect();

    let mut seen: BTreeSet<u32> = map
        .rooms()
        .iter()
        .filter(|r| is_town_centre(r))
        .map(|r| r.id.0)
        .collect();
    let mut queue: VecDeque<u32> = seen.iter().copied().collect();

    while let Some(id) = queue.pop_front() {
        let Some(&i) = index.get(&id) else { continue };
        for exit in &map.rooms()[i].exits {
            if is_seasonal(&format!("{:?}", exit.crossing)) {
                continue;
            }
            if seen.insert(exit.to.0) {
                queue.push_back(exit.to.0);
            }
        }
    }
    seen
}

fn is_town_centre(room: &Room) -> bool {
    room.title
        .iter()
        .any(|t| TOWN_CENTRES.iter().any(|c| t.starts_with(c)))
}

/// Rooms grouped into connected components, ignoring edge direction and
/// seasonal edges.
///
/// Undirected here, unlike [`walkable`], because this answers a different
/// question: not "can I get there" but "is this one place". A component is
/// the unit curation is decided in -- `Grawood Farmstead` is 211 rooms in
/// one component, and one verdict settles all of them.
#[must_use]
pub fn components(map: &Map) -> Vec<Vec<u32>> {
    let mut adjacent: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for room in map.rooms() {
        adjacent.entry(room.id.0).or_default();
        for exit in &room.exits {
            if is_seasonal(&format!("{:?}", exit.crossing)) {
                continue;
            }
            adjacent.entry(room.id.0).or_default().insert(exit.to.0);
            adjacent.entry(exit.to.0).or_default().insert(room.id.0);
        }
    }

    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut out: Vec<Vec<u32>> = Vec::new();
    for &start in adjacent.keys() {
        if !seen.insert(start) {
            continue;
        }
        let mut group = vec![start];
        let mut queue = VecDeque::from([start]);
        while let Some(id) = queue.pop_front() {
            for &next in adjacent.get(&id).into_iter().flatten() {
                if seen.insert(next) {
                    group.push(next);
                    queue.push_back(next);
                }
            }
        }
        out.push(group);
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.len()));
    out
}
