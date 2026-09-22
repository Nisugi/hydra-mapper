//! Apply `curation/*.toml` to `gs.map`.
//!
//! The order is fixed and the reason is not stylistic:
//!
//! 1. **Locations.** Disposition keys on where a room *is*. The Arena of
//!    the Abyss records `Caligos Isle` but the game moved it to Evermore
//!    Hollow, so a `gone` rule on Caligos would hide eight live rooms
//!    unless the location is corrected first.
//! 2. **Disposition.** `meta:map:status:*` and `meta:event:*`.
//! 3. **Duplicate stubs.** Deleting rooms renumbers nothing -- ids are
//!    kept -- but it does remove edge targets, so it runs after the passes
//!    that read the graph.
//!
//! Every pass records what it did rather than mutating silently, because
//! `plan` and `apply` must compute the same thing: a report you can review
//! and a write you cannot are different tools, and only one of them is
//! trustworthy.

pub mod reach;
pub mod rules;

use std::collections::{BTreeMap, BTreeSet};

use cena_map::{Map, Room};

pub use rules::{Curation, LoadError, Rule, Verdict};

/// One change to one room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `location` replaced, because the recorded one is stale.
    Location {
        id: u32,
        from: String,
        to: String,
    },
    /// A `meta` entry added.
    AddMeta { id: u32, meta: String },
    /// A `tag` removed, because `meta` now carries the same fact.
    DropTag { id: u32, tag: String },
    /// A tag renamed to the canonical spelling of its family.
    RenameTag {
        id: u32,
        from: String,
        to: String,
    },
    /// A bookkeeping stub removed.
    DeleteRoom { id: u32, title: String },
}

impl Change {
    #[must_use]
    pub fn id(&self) -> u32 {
        match self {
            Change::Location { id, .. }
            | Change::AddMeta { id, .. }
            | Change::DropTag { id, .. }
            | Change::RenameTag { id, .. }
            | Change::DeleteRoom { id, .. } => *id,
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Location { id, from, to } => {
                write!(f, "{id}\tlocation\t{from:?} -> {to:?}")
            }
            Change::AddMeta { id, meta } => write!(f, "{id}\t+meta\t{meta}"),
            Change::DropTag { id, tag } => write!(f, "{id}\t-tag\t{tag}"),
            Change::RenameTag { id, from, to } => write!(f, "{id}\ttag\t{from} -> {to}"),
            Change::DeleteRoom { id, title } => write!(f, "{id}\tDELETE\t{title}"),
        }
    }
}

/// A rule that would mark a walkable room `closed` or `gone`.
///
/// The one hard error in the tool. Someone can reach these rooms right
/// now, so whatever the rule says, they are live -- and the rule is
/// describing a different place than its author believed. This is what
/// caught the Arena of the Abyss inside Caligos Isle; the same check would
/// have caught a `location` rule on Briarmoon Cove taking the live Pinefar
/// Trading Post with it.
#[derive(Debug, Clone)]
pub struct Violation {
    pub rule: String,
    pub verdict: Verdict,
    pub matched: usize,
    pub reachable: Vec<(u32, String)>,
}

/// What a run would do.
#[derive(Debug, Default)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub violations: Vec<Violation>,
    /// Rules that matched nothing: a typo, or a place the map no longer
    /// has. Not fatal, but always worth printing.
    pub empty_rules: Vec<String>,
    pub verdicts: BTreeMap<Verdict, usize>,
}

impl Plan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Changes grouped by kind, for the summary line.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for change in &self.changes {
            let key = match change {
                Change::Location { .. } => "location",
                Change::AddMeta { .. } => "meta added",
                Change::DropTag { .. } => "tag dropped",
                Change::RenameTag { .. } => "tag renamed",
                Change::DeleteRoom { .. } => "room deleted",
            };
            *counts.entry(key).or_default() += 1;
        }
        counts
    }
}

/// Work out every change, without making any.
///
/// `plan` and `apply` share this so the report cannot drift from the
/// write: `apply` is `plan` plus `Change::apply_all`.
#[must_use]
pub fn plan(map: &Map, curation: &Curation) -> Plan {
    let mut plan = Plan::default();
    let rooms = map.rooms();
    let location_of = |r: &Room| curation.location_of(r);

    // Pass 1: locations.
    for room in rooms {
        let corrected = curation.location_of(room);
        let current = room.location.clone().unwrap_or_default();
        if corrected != current {
            plan.changes.push(Change::Location {
                id: room.id.0,
                from: current,
                to: corrected,
            });
        }
    }

    // Reachability, computed once against the corrected locations. Gates
    // are ignored and seasonal edges are not followed; see `reach`.
    let reachable = reach::walkable(map);

    // Pass 2: disposition.
    let mut assigned: BTreeMap<u32, (Verdict, Option<String>)> = BTreeMap::new();
    for rule in &curation.status.rules {
        let matched: Vec<&Room> = rooms
            .iter()
            .filter(|r| rule.matches(r, &location_of))
            .collect();
        if matched.is_empty() {
            plan.empty_rules.push(rule.describe());
            continue;
        }
        if rule.verdict != Verdict::Live {
            let walkable: Vec<(u32, String)> = matched
                .iter()
                .filter(|r| reachable.contains(&r.id.0))
                .map(|r| (r.id.0, r.title.first().cloned().unwrap_or_default()))
                .collect();
            if !walkable.is_empty() {
                plan.violations.push(Violation {
                    rule: rule.describe(),
                    verdict: rule.verdict,
                    matched: matched.len(),
                    reachable: walkable,
                });
                continue;
            }
        }
        // Later rules win, so a broad area rule can be narrowed by a
        // specific one written after it.
        for room in matched {
            assigned.insert(room.id.0, (rule.verdict, rule.event.clone()));
        }
    }

    let by_id: BTreeMap<u32, &Room> = rooms.iter().map(|r| (r.id.0, r)).collect();
    for (id, (verdict, event)) in &assigned {
        *plan.verdicts.entry(*verdict).or_default() += 1;
        let Some(room) = by_id.get(id) else { continue };
        if let Some(meta) = verdict.meta()
            && !room.meta.iter().any(|m| m == meta)
        {
            plan.changes.push(Change::AddMeta {
                id: *id,
                meta: meta.to_owned(),
            });
        }
        if let Some(event) = event {
            let meta = format!("event:{event}");
            if !room.meta.contains(&meta) {
                plan.changes.push(Change::AddMeta { id: *id, meta });
            }
        }
        // The tags this subsumes. `rewritten` is deliberately not here:
        // Cairnfang Manor is rewritten and live, so it is not a
        // disposition and keeps its tag.
        for tag in ["gone", "closed", "missing"] {
            if room.tags.iter().any(|t| t == tag) {
                plan.changes.push(Change::DropTag {
                    id: *id,
                    tag: tag.to_owned(),
                });
            }
        }
    }

    // Pass 2b: the disposition tags no area rule reached.
    convert_loose_tags(&mut plan, rooms, curation, &assigned, &reachable);

    // Pass 2c: tag spellings.
    normalise_spellings(&mut plan, rooms, curation);

    // `urchin-hideout` duplicates `meta:map:virtual room` exactly -- the
    // same 16 rooms, verified, not assumed.
    for room in rooms {
        if room.tags.iter().any(|t| t == "urchin-hideout")
            && room.meta.iter().any(|m| m == "map:virtual room")
        {
            plan.changes.push(Change::DropTag {
                id: room.id.0,
                tag: "urchin-hideout".to_owned(),
            });
        }
    }

    // Pass 3: duplicate stubs.
    for id in deletable_stubs(map) {
        if let Some(room) = by_id.get(&id) {
            plan.changes.push(Change::DeleteRoom {
                id,
                title: room.title.first().cloned().unwrap_or_default(),
            });
        }
    }

    plan.changes.sort_by_key(Change::id);
    plan
}

/// Rewrite tag spellings to one form per family.
///
/// Mostly hygiene: measured, most of the 44 families are the same rooms
/// carrying both spellings rather than divided between them, so a
/// consumer filtering on either already finds everything. `cleric shop`
/// is the exception that fixes a real lookup failure -- six rooms carry
/// one spelling only.
fn normalise_spellings(plan: &mut Plan, rooms: &[Room], curation: &Curation) {
    for rename in &curation.spellings.renames {
        for room in rooms {
            for from in &rename.from {
                if room.tags.contains(from) {
                    plan.changes.push(Change::RenameTag {
                        id: room.id.0,
                        from: from.clone(),
                        to: rename.to.clone(),
                    });
                }
            }
        }
    }
}

/// Fold the disposition tags no area rule reached into `meta:map:status`.
///
/// These are the scattered dead shops in live towns, not whole areas. The
/// pass runs after the area rules and skips any room they settled, so a
/// curated judgement always beats a tag of unknown age.
///
/// Dropping a tag is worthwhile even when no status replaces it:
/// `missing` was never a disposition, just a stale note that the mapper
/// could not place a room it plainly can.
fn convert_loose_tags(
    plan: &mut Plan,
    rooms: &[Room],
    curation: &Curation,
    assigned: &BTreeMap<u32, (Verdict, Option<String>)>,
    reachable: &BTreeSet<u32>,
) {
    for conversion in curation.tags.conversions.values() {
        for room in rooms {
            if assigned.contains_key(&room.id.0) {
                continue;
            }
            if !conversion.applies(room, reachable.contains(&room.id.0)) {
                continue;
            }
            plan.changes.push(Change::DropTag {
                id: room.id.0,
                tag: conversion.from_tag.clone(),
            });
            if let Some(verdict) = conversion.verdict()
                && let Some(meta) = verdict.meta()
                && !room.meta.iter().any(|m| m == meta)
            {
                *plan.verdicts.entry(verdict).or_default() += 1;
                plan.changes.push(Change::AddMeta {
                    id: room.id.0,
                    meta: meta.to_owned(),
                });
            }
        }
    }
}

/// Stubs safe to remove: titled `duplicate of NNNN`, carrying no
/// description, with no inbound edge from a room that is not itself a stub.
///
/// **The description test is not a refinement, it is the whole rule.** Ten
/// rooms share the title prefix but have a description, and comparing it
/// with the room they name shows they are day/night variants of the same
/// street -- "nearly deserted" against "teems with people" -- recorded
/// twice and mislabelled. They are live rooms, and deleting them on the
/// title alone would take ten Ta'Vaalor streets with them. A room with a
/// body of text is a room someone visited.
///
/// The inbound condition is likewise not "no inbound edge at all". Five
/// stubs are pointed at, and every one of those edges comes from another
/// stub -- they refer to each other inside an island nothing live touches.
/// Requiring total isolation would strand 140 rooms for no gain; requiring
/// no *live* inbound is the property that actually matters.
#[must_use]
pub fn deletable_stubs(map: &Map) -> BTreeSet<u32> {
    const PREFIX: &str = "duplicate of ";
    let is_stub = |r: &Room| {
        r.title.iter().any(|t| t.starts_with(PREFIX)) && r.description.is_empty()
    };

    let stubs: BTreeSet<u32> = map
        .rooms()
        .iter()
        .filter(|r| is_stub(r))
        .map(|r| r.id.0)
        .collect();

    // "Live" here excludes the day/night variants too. They carry the
    // same title prefix and point at each other, so counting them as live
    // would make five stubs look attached to something real and leave
    // them behind.
    let labelled: BTreeSet<u32> = map
        .rooms()
        .iter()
        .filter(|r| r.title.iter().any(|t| t.starts_with(PREFIX)))
        .map(|r| r.id.0)
        .collect();

    let mut live_inbound: BTreeSet<u32> = BTreeSet::new();
    for room in map.rooms() {
        if labelled.contains(&room.id.0) {
            continue;
        }
        for exit in &room.exits {
            if stubs.contains(&exit.to.0) {
                live_inbound.insert(exit.to.0);
            }
        }
    }
    stubs.difference(&live_inbound).copied().collect()
}

/// Apply a plan, returning the new room list.
///
/// Takes the rooms rather than the `Map` because `Map` is immutable after
/// construction: the caller rebuilds one with `Map::from_rooms`.
#[must_use]
pub fn apply(rooms: &[Room], plan: &Plan) -> Vec<Room> {
    let mut rooms: Vec<Room> = rooms.to_vec();
    let mut deleted: BTreeSet<u32> = BTreeSet::new();
    let index: BTreeMap<u32, usize> =
        rooms.iter().enumerate().map(|(i, r)| (r.id.0, i)).collect();

    for change in &plan.changes {
        let Some(&i) = index.get(&change.id()) else {
            continue;
        };
        match change {
            Change::Location { to, .. } => rooms[i].location = Some(to.clone()),
            Change::AddMeta { meta, .. } => {
                if !rooms[i].meta.iter().any(|m| m == meta) {
                    rooms[i].meta.push(meta.clone());
                    rooms[i].meta.sort();
                }
            }
            Change::DropTag { tag, .. } => rooms[i].tags.retain(|t| t != tag),
            Change::RenameTag { from, to, .. } => {
                for tag in &mut rooms[i].tags {
                    if tag == from {
                        tag.clone_from(to);
                    }
                }
                rooms[i].tags.sort();
                rooms[i].tags.dedup();
            }
            Change::DeleteRoom { id, .. } => {
                deleted.insert(*id);
            }
        }
    }

    if !deleted.is_empty() {
        rooms.retain(|r| !deleted.contains(&r.id.0));
        // An exit to a deleted room would dangle. Nothing live points at
        // a deletable stub, so this only prunes stub-to-stub edges, but
        // leaving a dangling target in a binary format is how a loader
        // starts returning rooms that are not there.
        for room in &mut rooms {
            room.exits.retain(|e| !deleted.contains(&e.to.0));
        }
    }
    rooms
}
