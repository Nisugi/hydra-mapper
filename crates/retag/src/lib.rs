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

/// Mark rooms whose **only** way out leads into a room already gone.
///
/// Derived rather than listed, because what strands a room is a fact
/// about the graph and a hand-written list of 89 ids would be a snapshot
/// of one run of it. The rules in `status.toml` name places; this names
/// the consequence of those names.
///
/// The seed is the settled set: `map:status:gone`, unreachable from any
/// town centre, and never numbered by the game. A room is added when
/// every one of its exits lands in that set and it is itself unreachable
/// and unnumbered -- then the set grows and it runs again, because taking
/// a shop can strand the room behind it.
///
/// In gs.map this converges after one round and finds 89 rooms, every one
/// a single-exit festival shop opening onto a Feywrot Mire clearing, a
/// Pinefar stall, or the like: `[Zombie Snack Shack]`, `[Bog Botanicals]`,
/// `[Birds of Paradise, Parlour]`. They were missed because the curation
/// names places by location or title and these have neither -- 67 of the
/// 89 have no `location` at all, and no title pattern covers
/// `[Crazy Gravy]`.
///
/// # What it will not take
///
/// A room reachable from a town centre, whatever its exits say. A room
/// the game still numbers. A room with no exits at all, which says
/// nothing either way and is left to `deletable_stubs` and the
/// `is_connected` test that already handle it. The walkability veto is
/// the same one every other pass answers to, and it is checked here
/// directly rather than reported as a violation, because this pass
/// generates its own matches and a rule that cannot be written wrong
/// needs no complaint about it.
fn strand_orphans(plan: &mut Plan, rooms: &[Room], reachable: &BTreeSet<u32>) {
    let mut dead: BTreeSet<u32> = rooms
        .iter()
        .filter(|r| {
            r.meta.iter().any(|m| m == "map:status:gone")
                && !reachable.contains(&r.id.0)
                && r.uid.is_empty()
        })
        .map(|r| r.id.0)
        .collect();

    let mut added: Vec<u32> = Vec::new();
    loop {
        let round: Vec<u32> = rooms
            .iter()
            .filter(|r| {
                !dead.contains(&r.id.0)
                    && !reachable.contains(&r.id.0)
                    && r.uid.is_empty()
                    && !r.exits.is_empty()
                    && r.exits.iter().all(|e| dead.contains(&e.to.0))
            })
            .map(|r| r.id.0)
            .collect();
        if round.is_empty() {
            break;
        }
        for id in &round {
            dead.insert(*id);
        }
        added.extend(round);
    }

    for id in added {
        plan.changes.push(Change::AddMeta {
            id,
            meta: "map:status:gone".to_owned(),
        });
    }
}

/// Write `meta:region:<name>` from the official mapdb's `loc` field.
///
/// Joined by **uid only**. A title join would invent agreement -- 20,128
/// mapdb titles against ours, with `[Shop]` and `[Second Floor]` repeating
/// town to town -- and the point of this pass is to compare two sources,
/// which is worthless if one is derived from a guess about the other.
///
/// Additive: `location` is not touched. The two fields answer different
/// questions, and which one a consumer should want is exactly what this
/// pass exists to let someone measure.
fn tag_regions(plan: &mut Plan, rooms: &[Room], curation: &Curation) {
    let mut region_of: BTreeMap<i64, &str> = BTreeMap::new();
    for region in &curation.regions.regions {
        for uid in &region.uids {
            region_of.insert(*uid, region.name.as_str());
        }
    }
    // Rooms the mapdb holds with no `loc`, named by id because the
    // evidence is which official area they are in. Seeded before the uid
    // pass so a room that HAS a region keeps it: the check below refuses
    // to write a second one, and this is the weaker claim of the two.
    let mut by_id: BTreeMap<u32, &str> = BTreeMap::new();
    for block in &curation.regions.unclassified {
        for id in &block.ids {
            by_id.insert(*id, block.region.as_str());
        }
    }

    for room in rooms {
        // A room the game numbers once belongs where that number says.
        // A room it numbers NINE times, once per town, is a plane the
        // towns each open onto -- and `find_map` would take whichever
        // instance happened to be listed first.
        let mut found: BTreeSet<&str> = BTreeSet::new();
        for uid in &room.uid {
            if let Some(name) = region_of.get(&uid.0) {
                found.insert(name);
            }
        }
        let name = match found.len() {
            0 => match by_id.get(&room.id.0) {
                Some(region) => (*region).to_owned(),
                None => continue,
            },
            1 => (*found.iter().next().unwrap_or(&"")).to_owned(),
            // Named for itself. It is one place in our map and nine in
            // theirs, so no town's name is more true than the others'.
            _ => curation
                .regions
                .planes
                .iter()
                .find(|p| room.title.iter().any(|t| t.contains(&p.title)))
                .map_or_else(String::new, |p| p.region.clone()),
        };
        if name.is_empty() {
            continue;
        }
        // A region that is an area of another answers with the other's
        // name. See `[[fold]]` in regions.toml for why each one is one.
        let name = fold_of(curation, &name);
        let meta = format!("region:{name}");
        // A plane's region changed once the plane rule existed, so the
        // stale one has to go: `AddMeta` alone would leave a room
        // claiming two regions.
        for old in room.meta.iter().filter(|m| m.starts_with("region:")) {
            if *old != meta {
                plan.changes.push(Change::DropMeta {
                    id: room.id.0,
                    meta: old.clone(),
                });
            }
        }
        if room.meta.contains(&meta) {
            continue;
        }
        plan.changes.push(Change::AddMeta {
            id: room.id.0,
            meta,
        });
    }
}

/// Whether an exit is a walk, and so whether it says the two rooms are
/// on the same side of a region boundary.
///
/// Only a plain command or a list of steps. A routine is a travel puzzle
/// whose destination shuffles, an urchin pass-through is a teleport, and
/// an unported or unknown crossing cannot be walked at all -- none of
/// them means the rooms are near each other. The premium halls are the
/// worked example: every one has a pass-through to its town's urchin
/// hideout, and following those would make nine towns adjacent.
///
/// Deliberately stricter than [`reach::walkable`], which asks a
/// different question -- whether a person can get there by any means --
/// and follows every exit that is not seasonal.
fn is_passage(exit: &cena_map::Exit) -> bool {
    matches!(
        exit.crossing,
        cena_map::Crossing::Command(_) | cena_map::Crossing::Steps(_)
    )
}

/// Whether a room is somewhere, rather than a fake room the game uses
/// to hold data. The sixteen urchin hideouts carry `map:virtual room`
/// and their own descriptions say so: "This is a fake room used to hold
/// wayto/timeto for urchin guide".
fn is_a_place(room: &Room) -> bool {
    !room.meta.iter().any(|m| m == "map:virtual room")
}

/// The region a `loc` value ends up as, following the `[[fold]]` chain:
/// `The Rift` -> `Pinefar / Aenatumgana` -> `Icemule Trace`.
fn fold_of(curation: &Curation, name: &str) -> String {
    let mut current = name.to_owned();
    // Bounded rather than trusting the file to be acyclic.
    for _ in 0..8 {
        // A fold names one value; a region's `members` names several.
        // Both say "this `loc` value is not a region, that one is", so
        // both resolve here and a chain may run through either -- `The
        // Rift` folds into Pinefar, which folds into Icemule Trace.
        if let Some(f) = curation
            .decisions
            .folds
            .iter()
            .find(|f| f.region == current)
        {
            current = f.into.clone();
            continue;
        }
        if let Some(r) = curation
            .decisions
            .regions
            .iter()
            .find(|r| r.members.contains(&current))
        {
            current = r.name.clone();
            continue;
        }
        break;
    }
    current
}

/// Fill a room's region where every way out of it leads to one region.
///
/// **A region boundary has to be crossed somewhere.** Walk outward from
/// an unregioned room through other unregioned rooms until you reach
/// ground that has a region. If everything you can reach that way is one
/// region, there is no path out of here that does not go through it, and
/// the room is inside it. That is a fact about where the graph lets you
/// walk, not an inference from a name, a location string or a title
/// prefix, all of which have been wrong here before.
///
/// Simutronics fills `loc` in for towns, quests and transport routes and
/// leaves wilderness out, so this fills a blank rather than overruling
/// anybody: [`tag_regions`] runs first and a room it placed is never
/// touched.
///
/// The unit is the **pocket**, not the room: the whole connected run of
/// unregioned ground, judged by everything on its frontier at once.
/// Deciding room by room would be weaker and order-dependent -- a room
/// beside Wehnimer's would take it before the pocket behind it revealed
/// a second way out into Shadow Valley, and the boundary would land
/// wherever the two advancing sides happened to meet rather than where
/// the map puts it.
///
/// Measured on `gs.map`, of 9,666 unregioned rooms:
///
/// - **7,161** are in a pocket with one region on its frontier, and fill.
/// - **713** are in a pocket reaching several, and are left alone. These
///   are real: 192 rooms of wilderness between Icemule, the Isle of
///   Ornath, Solhaven and Wehnimer's; the Graveyard's 163 between
///   Wehnimer's and Shadow Valley; 38 and 33 more against what is left
///   of Talador. A boundary genuinely runs through them and nothing here
///   can say where.
/// - **1,792** reach no regioned room at all, so there is nothing to
///   infer from.
fn spread_regions(plan: &mut Plan, rooms: &[Room], curation: &Curation) {
    // Start from what the map says plus what this run has already
    // planned, so the mapdb's answer always wins over an inferred one.
    let mut region: BTreeMap<u32, String> = BTreeMap::new();
    for room in rooms {
        if let Some(name) = room.meta.iter().find_map(|m| m.strip_prefix("region:")) {
            region.insert(room.id.0, name.to_owned());
        }
    }
    for change in &plan.changes {
        match change {
            Change::AddMeta { id, meta } => {
                if let Some(name) = meta.strip_prefix("region:") {
                    region.insert(*id, name.to_owned());
                }
            }
            Change::DropMeta { id, meta } if meta.starts_with("region:") => {
                region.remove(id);
            }
            _ => {}
        }
    }
    // **Resolve AFTER the planned changes, not before.** The map holds
    // whatever the last run wrote, which for a folded value is its old
    // name, and `tag_regions` only rewrites rooms whose uid the mapdb
    // lists -- so a room it skipped keeps the stale name and spreads it.
    // Resolving first did not help: the loop above then overwrote the
    // corrected names with the planned ones, which are themselves
    // resolved, except where a `DropMeta` later in file order removed
    // the entry again. 101 rooms came out carrying `Cysaegir`,
    // `Kraken's Fall` and `Old Ta'Faendryl` after those had folded.
    for name in region.values_mut() {
        *name = fold_of(curation, name);
    }

    // Walkable adjacency, both ways: a one-way door still says the two
    // rooms are on the same side of a boundary.
    let present: BTreeSet<u32> = rooms.iter().map(|r| r.id.0).collect();
    let mut adj: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for room in rooms {
        for exit in room.exits.iter().filter(|e| is_passage(e)) {
            let to = exit.to.0;
            if to != room.id.0 && present.contains(&to) {
                adj.entry(room.id.0).or_default().insert(to);
                adj.entry(to).or_default().insert(room.id.0);
            }
        }
    }

    // A room that is not a place gets no region, however enclosed it is.
    // The urchin hideouts are the case: sixteen fake rooms holding
    // `wayto` entries for the guide, marked `map:virtual room`, each
    // reached only by a pass-through. `derive_areas` drops them, but the
    // Region list is built from `meta:region:` rather than from areas,
    // so filling one here is what put a hideout in the tree.
    let unreal: BTreeSet<u32> = rooms
        .iter()
        .filter(|r| !is_a_place(r))
        .map(|r| r.id.0)
        .collect();
    for id in &unreal {
        region.remove(id);
    }
    // And take back any a previous run wrote. `AddMeta` alone would
    // leave the stale one in place, because this pass only ever adds.
    for room in rooms.iter().filter(|r| unreal.contains(&r.id.0)) {
        for stale in room
            .meta
            .iter()
            .filter(|m| m.starts_with("region:") || *m == "map:region-inferred")
        {
            plan.changes.push(Change::DropMeta {
                id: room.id.0,
                meta: stale.clone(),
            });
        }
    }
    let blank: Vec<u32> = rooms
        .iter()
        .map(|r| r.id.0)
        .filter(|id| !region.contains_key(id) && !unreal.contains(id))
        .collect();
    let blank_set: BTreeSet<u32> = blank.iter().copied().collect();

    let mut seen: BTreeSet<u32> = BTreeSet::new();
    for &start in &blank {
        if !seen.insert(start) {
            continue;
        }
        // The pocket, and every region on its frontier.
        let mut stack = vec![start];
        let mut members = vec![start];
        let mut frontier: BTreeSet<&str> = BTreeSet::new();
        while let Some(here) = stack.pop() {
            let Some(neighbours) = adj.get(&here) else {
                continue;
            };
            for n in neighbours {
                if let Some(name) = region.get(n) {
                    frontier.insert(name.as_str());
                } else if blank_set.contains(n) && seen.insert(*n) {
                    members.push(*n);
                    stack.push(*n);
                }
            }
        }
        if frontier.len() != 1 {
            continue;
        }
        let Some(name) = frontier.iter().next() else {
            continue;
        };
        for id in members {
            plan.changes.push(Change::AddMeta {
                id,
                meta: format!("region:{name}"),
            });
            // **Say that this one was inferred.** A region from the
            // mapdb is Simutronics' answer; this one is the graph's. A
            // separate namespace, not a `region:` value: anything
            // reading `strip_prefix("region:")` would otherwise find a
            // region named `from:spread`.
            plan.changes.push(Change::AddMeta {
                id,
                meta: "map:region-inferred".to_owned(),
            });
        }
    }
}

/// One change to one room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `location` replaced, because the recorded one is stale.
    Location { id: u32, from: String, to: String },
    /// A `meta` entry added.
    AddMeta { id: u32, meta: String },
    /// A `tag` removed, because `meta` now carries the same fact.
    DropTag { id: u32, tag: String },
    /// A `meta` entry removed, superseded by a consolidated one.
    DropMeta { id: u32, meta: String },
    /// A tag renamed to the canonical spelling of its family.
    RenameTag { id: u32, from: String, to: String },
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
            | Change::DropMeta { id, .. }
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
            Change::DropMeta { id, meta } => write!(f, "{id}	-meta	{meta}"),
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
                Change::DropMeta { .. } => "meta dropped",
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
            // A rule whose rooms were deleted did its job; only an
            // unmarked empty rule is a mistake worth reporting.
            if !rule.spent {
                plan.empty_rules.push(rule.describe());
            }
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

    // Pass 2d: lockers.
    consolidate_lockers(&mut plan, rooms, curation);

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

    // Pass 2d2: rooms stranded behind a gone area.
    strand_orphans(&mut plan, rooms, &reachable);

    // Pass 2e: regions from the official mapdb.
    tag_regions(&mut plan, rooms, curation);

    // Pass 2f: regions spread into the ground between them.
    spread_regions(&mut plan, rooms, curation);

    // Pass 3: rooms the game no longer has.
    for id in removed_rooms(rooms, &reachable) {
        if let Some(room) = by_id.get(&id) {
            plan.changes.push(Change::DeleteRoom {
                id,
                title: room.title.first().cloned().unwrap_or_default(),
            });
        }
    }

    // Pass 3b: duplicate stubs.
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
        // A prefix rewrite moves a whole namespace and has nothing to do
        // with tags or with walkability.
        if let (Some(from), Some(to)) = (&conversion.from_meta_prefix, &conversion.to_meta_prefix) {
            for room in rooms {
                for meta in &room.meta {
                    if let Some(rest) = meta.strip_prefix(from.as_str()) {
                        plan.changes.push(Change::DropMeta {
                            id: room.id.0,
                            meta: meta.clone(),
                        });
                        let moved = format!("{to}{rest}");
                        if !room.meta.contains(&moved) {
                            plan.changes.push(Change::AddMeta {
                                id: room.id.0,
                                meta: moved,
                            });
                        }
                    }
                }
            }
            continue;
        }
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
            if let Some(meta) = &conversion.to_meta
                && !room.meta.contains(meta)
            {
                plan.changes.push(Change::AddMeta {
                    id: room.id.0,
                    meta: meta.clone(),
                });
            }
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

/// Rooms to remove outright: the game does not have them any more.
///
/// Three independent facts have to agree, and each covers a different way
/// of being wrong about the other two:
///
/// 1. **`map:status:gone`** -- a curated verdict, argued in
///    `curation/status.toml` against the wiki and the game's own history.
/// 2. **Unreachable** from any town centre, gates ignored and seasonal
///    edges not followed. The same veto every other pass answers to.
/// 3. **No uid.** The game numbers the rooms it has. We map exhaustively
///    and repeatedly, so a room with no number is one we could not reach
///    despite trying, not one nobody got round to.
///
/// In gs.map that is 2,629 rooms, 94% of them in four areas settled
/// separately: Caligos Isle (666, sunk), the Miasmic Verge (644), the
/// Feywrot Mire (628) and Talador (438, destroyed in 5116).
///
/// # Why deleting rather than leaving them gone
///
/// `gone` already means never drawn, so this buys no correctness. It
/// buys the map not carrying 2,629 rooms that no longer describe
/// anything, and every consumer not having to learn the distinction.
///
/// # What makes it safe to do at all
///
/// **No live room points into the set.** Measured, not assumed: of the
/// 247 rooms with an exit into it, zero are reachable. The 89 whose every
/// exit led inside were folded in by `strand_orphans` first, so the edges
/// that would have dangled belong to rooms that are going too, and
/// `apply` prunes what remains.
///
/// The three tests are checked here rather than trusted from a rule,
/// because unlike a `status.toml` entry there is no violation to report:
/// a deleted room cannot be un-deleted by fixing a file and re-running.
fn removed_rooms(rooms: &[Room], reachable: &BTreeSet<u32>) -> BTreeSet<u32> {
    rooms
        .iter()
        .filter(|r| {
            r.meta.iter().any(|m| m == "map:status:gone")
                && !reachable.contains(&r.id.0)
                && r.uid.is_empty()
        })
        .map(|r| r.id.0)
        .collect()
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
    let is_stub =
        |r: &Room| r.title.iter().any(|t| t.starts_with(PREFIX)) && r.description.is_empty();

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
    let index: BTreeMap<u32, usize> = rooms.iter().enumerate().map(|(i, r)| (r.id.0, i)).collect();

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
            Change::DropMeta { meta, .. } => rooms[i].meta.retain(|m| m != meta),
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

/// Display name -> `che:` key, learned from the map and topped up from
/// curation.
///
/// A room carrying both `locker annex:House of Paupers` and
/// `che:paupers:entrance_annex` states the pair outright, and eleven of
/// the fourteen houses have such a room. Reading them beats hand-listing
/// them: the map is the thing being changed, so it is the thing that
/// should say how its own keys line up. `curation/lockers.toml` supplies
/// only the four no room pairs up, each with a reason.
#[must_use]
pub fn house_keys(rooms: &[Room], curation: &Curation) -> BTreeMap<String, String> {
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    for room in rooms {
        let displays: Vec<&str> = room
            .meta
            .iter()
            .filter_map(|m| m.strip_prefix("locker annex:"))
            .collect();
        let snake: Vec<&str> = room
            .meta
            .iter()
            .filter_map(|m| m.strip_suffix(":entrance_annex"))
            .filter_map(|m| m.strip_prefix("che:"))
            .collect();
        if let (Some(display), Some(key)) = (displays.first(), snake.first()) {
            keys.insert((*display).to_owned(), (*key).to_owned());
        }
    }
    for (display, key) in &curation.lockers.house_keys {
        keys.insert(display.clone(), key.clone());
    }
    keys
}

/// Fold four locker schemes and 19 tags into `meta:locker:*`.
///
/// The consolidation exists because `publiclockers` (89 rooms) and bare
/// `meta:locker` (164 rooms) overlap on **zero** rooms: two disjoint sets
/// naming one concept in two fields, so neither field alone answers
/// "where are the lockers".
///
/// Sources are dropped only where `drop_source` says so. `che:*` keys
/// stay: they carry the access fact -- who may open the door -- which is
/// a different question from whose locker it is, and not this pass's to
/// delete.
fn consolidate_lockers(plan: &mut Plan, rooms: &[Room], curation: &Curation) {
    let keys = house_keys(rooms, curation);
    let add = |plan: &mut Plan, room: &Room, meta: String| {
        if !room.meta.contains(&meta) {
            plan.changes.push(Change::AddMeta {
                id: room.id.0,
                meta,
            });
        }
    };

    for rule in &curation.lockers.rules {
        for room in rooms {
            let mut matched = false;

            for tag in &rule.from_tags {
                if room.tags.contains(tag) {
                    matched = true;
                    if rule.drop_source {
                        plan.changes.push(Change::DropTag {
                            id: room.id.0,
                            tag: tag.clone(),
                        });
                    }
                }
            }
            let che_free = !room.meta.iter().any(|m| m.starts_with("che:"));
            for meta in &rule.from_meta {
                if room.meta.contains(meta) && (che_free || !rule.from_meta_requires_no_che) {
                    matched = true;
                    if rule.drop_source {
                        plan.changes.push(Change::DropMeta {
                            id: room.id.0,
                            meta: meta.clone(),
                        });
                    }
                }
            }
            if matched && let Some(to) = &rule.to_meta {
                add(plan, room, to.clone());
            }

            // Patterned sources: the house is a wildcard in the middle.
            if let Some(required) = &rule.require_meta
                && !room.meta.contains(required)
            {
                continue;
            }
            for pattern in [&rule.from_meta_pattern, &rule.from_meta_pattern_alt]
                .into_iter()
                .flatten()
            {
                for source in &room.meta {
                    let Some(house) = match_house(pattern, source) else {
                        continue;
                    };
                    // A display name needs translating; a snake key is
                    // already the form the target wants.
                    let key = keys.get(&house).cloned().unwrap_or(house);
                    if let Some(to) = &rule.to_meta_pattern {
                        add(plan, room, to.replace("{house}", &key));
                    }
                    if rule.drop_source {
                        plan.changes.push(Change::DropMeta {
                            id: room.id.0,
                            meta: source.clone(),
                        });
                    }
                }
            }
        }
    }

    apply_house_tags(plan, rooms, curation);

    // A room the CHE rules claimed is not public, whatever a tag said.
    // Three Beacon Hall annex lockers carry both a house tag and a public
    // one; the house is the truth and `locker:public` is the mistake.
    let che_owned: BTreeSet<u32> = plan
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::AddMeta { id, meta } if meta.starts_with("locker:che:") => Some(*id),
            _ => None,
        })
        .collect();
    plan.changes.retain(|c| {
        !matches!(
            c,
            Change::AddMeta { id, meta } if meta == "locker:public" && che_owned.contains(id)
        )
    });

    // Bare `meta:locker` on a room the consolidation has now described
    // properly is redundant. It is dropped here rather than in the public
    // rule because only now is it known that something replaced it.
    for room in rooms {
        if room.meta.iter().any(|m| m == "locker") && che_owned.contains(&room.id.0) {
            plan.changes.push(Change::DropMeta {
                id: room.id.0,
                meta: "locker".to_owned(),
            });
        }
    }
}

/// The 19 tags spelling "this house's lockers" three different ways.
fn apply_house_tags(plan: &mut Plan, rooms: &[Room], curation: &Curation) {
    for entry in &curation.lockers.house_tags {
        let kind = entry.kind.as_deref().unwrap_or("house");
        for room in rooms {
            for tag in &entry.tags {
                if room.tags.contains(tag) {
                    let meta = format!("locker:che:{kind}:{}", entry.house);
                    if !room.meta.contains(&meta) {
                        plan.changes.push(Change::AddMeta {
                            id: room.id.0,
                            meta,
                        });
                    }
                    plan.changes.push(Change::DropTag {
                        id: room.id.0,
                        tag: tag.clone(),
                    });
                }
            }
        }
    }
}

/// Match `che:{house}:locker` or `locker annex:{House}` against one meta
/// string, returning the house part.
fn match_house(pattern: &str, source: &str) -> Option<String> {
    let token = if pattern.contains("{house}") {
        "{house}"
    } else {
        "{House}"
    };
    let (before, after) = pattern.split_once(token)?;
    let rest = source.strip_prefix(before)?;
    let house = if after.is_empty() {
        rest
    } else {
        rest.strip_suffix(after)?
    };
    // The house part is always ONE segment. Without this,
    // `che:{House}` happily matches `che:paupers:locker` and calls the
    // house "paupers:locker", producing `locker:che:house:paupers:locker`.
    if house.is_empty() || house.contains(':') {
        return None;
    }
    Some(house.to_owned())
}
