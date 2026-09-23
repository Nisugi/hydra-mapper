//! The two area lists a person can browse the map by.
//!
//! Two lists, from two sources, over one map -- and **no room appears in
//! both**:
//!
//! - **Official** -- Simutronics' own layout areas, the unit their
//!   hand-drawn artwork is positioned in. Only some of the map has them.
//! - **Mapdb** -- what the game's `location` verb answers, carried on
//!   [`cena_map::Room::location`]. Covers everything the official layout
//!   does not.
//!
//! **Official layout is truth.** Where it covers a room, it wins, and that
//! room is not listed again under its mapdb location. The reason is
//! curation: the official areas are clean, deliberate splits, whereas
//! mapdb `location` routinely cuts a building into areas of one or two
//! rooms, so browsing by it alone buries a town under its own shopfronts.
//! `research/jev-trial/areas.py` applies exactly this precedence when it
//! assigns each room a single area.
//!
//! The two groupings genuinely cross-cut -- `areas.py` measured it: *"66
//! official areas span several locations and 53 locations span several
//! official areas."* That is why the split is done per room rather than
//! per name: a location partly inside an official area keeps only its
//! unclaimed rooms, and one wholly inside it drops out of the mapdb list
//! altogether.
//!
//! Only the official list needs a data file. `gs.map` cannot supply it:
//! the map carries `image.file`, but those are raw artwork filenames, both
//! inconsistent (`JourneysEnd.jpg` beside `Journeys_End.jpg`) and not the
//! area names -- the clean slugs come from mapdb's `layouts.json` matched
//! to rooms by uid, which `areas.py` does upstream. Rather than depend on
//! that pipeline at runtime, its output ships with this crate, so the
//! explorer stays standalone as `plan/26` requires.

use std::collections::{BTreeMap, HashSet};

use cena_map::{Map, RoomId};

use crate::overrides::{MapOverrides, RoomKey};

/// `areas.tsv` as produced by `research/jev-trial/areas.py`, compiled in so
/// the binary needs no data file beside it.
///
/// A frozen snapshot, and the one real cost of bundling: areas added to the
/// map after this file was generated fall back to their mapdb location and
/// simply do not appear in the official list. Refreshing it is a file copy,
/// not a code change.
const AREAS_TSV: &str = include_str!("../data/areas.tsv");

/// Which of the three groupings a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaKind {
    /// Simutronics' own layout areas (`source` = `official layout`).
    Official,
    /// The game's `location` verb (`source` = `mapdb location`).
    ///
    /// Named for the FIELD, not the file. `location` is what the game
    /// answers where you stand, at whatever granularity the verb uses:
    /// `the Upper Trollfang`, `the town of Wehnimer's Landing`,
    /// `Greystorm Manor, inside the frontier town of Wehnimer's Landing`.
    Location,
    /// The official mapdb's `loc` field, written to `meta:region:` by
    /// `retag`.
    ///
    /// A different question from [`AreaKind::Location`], not a better
    /// answer to the same one. `loc` is an administrative grouping a GM
    /// assigned -- 50 of them in this map, against 344 location strings --
    /// and Simutronics fills it in for towns, quests and transport routes
    /// and leaves wilderness blank on purpose. 11,994 rooms have none, and
    /// for those our `location` is the only field that names the place.
    ///
    /// Worth having beside `Location` because the two disagree in a way
    /// that is information: the Upper Trollfang is one location of 413
    /// rooms, of which 120 are `QUEST: Black Swan Castle` -- a quest
    /// staged inside a hunting ground, which only the region field
    /// separates.
    Region,
    /// Areas read off the room graph, with mapdb's names as labels. See
    /// [`cena_map_layout::regions`]. Beside the other two rather than
    /// replacing them yet; its corrections are stored under their own
    /// key (see [`Area::store_key`]).
    Derived,
    /// Plates a person made, and moved rooms onto, to keep satellites off
    /// a town's own sheet. See [`crate::overrides`].
    Plates,
}

impl AreaKind {
    /// Every list, in tab order.
    pub const ALL: [AreaKind; 5] = [
        AreaKind::Official,
        AreaKind::Region,
        AreaKind::Location,
        AreaKind::Derived,
        AreaKind::Plates,
    ];

    /// The tab label, and the word for a count beneath it.
    pub const fn title(self) -> &'static str {
        match self {
            AreaKind::Official => "Official",
            AreaKind::Region => "Region",
            AreaKind::Location => "Location",
            AreaKind::Derived => "Derived",
            AreaKind::Plates => "Plates",
        }
    }
}

impl Area {
    /// The key this area's corrections are stored under. A derived area
    /// often shares a mapdb area's name over a different set of rooms with
    /// different group numbers, and a correction keyed to the bare name
    /// would land on the wrong grid, so its key is prefixed. The prefix
    /// is not shown anywhere: the list, the header and the export say the
    /// name.
    #[must_use]
    pub fn store_key(&self) -> String {
        match self.kind {
            AreaKind::Derived => format!("derived:{}", self.name),
            // Same reason as `derived:`. A region and a location share a
            // name often -- `Ta'Illistim` is both -- over different room
            // sets, so an unprefixed key would put one list's corrections
            // on the other's grid.
            AreaKind::Region => format!("region:{}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// One browsable area: a name and the rooms that make it up.
pub struct Area {
    pub name: String,
    /// Which kind of list this came from, so a caller can tell a plate
    /// (a grid) from an area (a place).
    pub kind: AreaKind,
    /// Members, in map order. Official areas carry them from `areas.tsv`;
    /// mapdb areas collect every room sharing a `location`; plates collect
    /// whatever was moved onto them.
    pub rooms: Vec<RoomId>,
}

/// All three lists, each sorted by name, rebuilt whenever a room is moved
/// between plates.
pub struct Areas {
    pub official: Vec<Area>,
    pub region: Vec<Area>,
    pub location: Vec<Area>,
    pub derived: Vec<Area>,
    pub plates: Vec<Area>,
}

impl Areas {
    /// Build the lists for `map`, plus the plates in `store`.
    ///
    /// **A plate is a grid, not an area.** Moving a room onto one changes
    /// where it is *laid out*, not where it *is*: the Town Well sits on
    /// `landing.well` and remains a room of Wehnimer's Landing, so it
    /// appears in both lists. `plan/21` §3f keeps the same two facts
    /// apart, as `location` (the area) and `map` (the grid), and
    /// collapsing them would lose the area for anything that groups by
    /// it.
    ///
    /// The Official and Mapdb lists still do not overlap each other:
    /// **official layout is truth.** A room an official area claims is not listed a second
    /// time under its mapdb location: the official layout is the curated
    /// split, whereas mapdb `location` frequently cuts a building into
    /// areas of one or two rooms. So the official list is taken first and
    /// the mapdb list covers only what is left -- the same precedence
    /// `areas.py` applies when it assigns each room exactly one area.
    ///
    /// The official list is intersected with the rooms actually present:
    /// `areas.tsv` was generated against one map file, and a room it names
    /// that this map does not have would otherwise produce an area that
    /// draws as a hole.
    #[must_use]
    pub fn build(map: &Map, store: &MapOverrides) -> Areas {
        let official = official_areas(map);
        let claimed: HashSet<RoomId> = official
            .iter()
            .flat_map(|a| a.rooms.iter().copied())
            .collect();
        Areas {
            official,
            region: region_areas(map),
            location: location_areas(map, &claimed),
            derived: derived_areas(map),
            plates: plate_areas(map, store),
        }
    }

    /// The list for one kind, so the picker can hold a kind rather than
    /// branching at every use.
    #[must_use]
    pub fn list(&self, kind: AreaKind) -> &[Area] {
        match kind {
            AreaKind::Official => &self.official,
            AreaKind::Region => &self.region,
            AreaKind::Location => &self.location,
            AreaKind::Derived => &self.derived,
            AreaKind::Plates => &self.plates,
        }
    }
}

/// The graph's own grouping, by name. Rooms with no exits at all are left
/// out: there is nothing to draw and nothing to group them by.
fn derived_areas(map: &Map) -> Vec<Area> {
    let mut areas: Vec<Area> = cena_map_layout::regions::derive_areas(map)
        .into_iter()
        .filter(|a| a.kind != cena_map_layout::regions::AreaKind::Isolated)
        .map(|a| Area {
            name: a.name,
            kind: AreaKind::Derived,
            rooms: a.rooms,
        })
        .collect();
    areas.sort_by(|a, b| a.name.cmp(&b.name));
    areas
}

/// One area per plate that has rooms on it, named as the person named it.
///
/// A plate with no rooms left is not listed: it would draw as an empty
/// sheet. The plate itself survives in the store, so it stays available as
/// a move target.
fn plate_areas(map: &Map, store: &MapOverrides) -> Vec<Area> {
    let mut by_plate: BTreeMap<&str, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        let key = RoomKey::of(room.id, map);
        if let Some(plate) = store.membership_moves.get(&key) {
            by_plate.entry(plate.as_str()).or_default().push(room.id);
        }
    }
    by_plate
        .into_iter()
        .map(|(plate, rooms)| Area {
            // The display name if the plate was minted here; the key
            // itself for one that arrived in a hand-edited file.
            name: store.plate_name(plate).to_owned(),
            kind: AreaKind::Plates,
            rooms,
        })
        .collect()
}

/// Parse the bundled TSV, keeping `official layout` rows whose rooms this
/// map has.
fn official_areas(map: &Map) -> Vec<Area> {
    let mut lines = AREAS_TSV.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let columns: Vec<&str> = header.split('\t').collect();
    let (Some(name_at), Some(source_at), Some(rooms_at)) = (
        columns.iter().position(|c| *c == "area"),
        columns.iter().position(|c| *c == "source"),
        columns.iter().position(|c| *c == "room_ids"),
    ) else {
        return Vec::new();
    };

    let mut areas: Vec<Area> = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let (Some(name), Some(source), Some(ids)) = (
            fields.get(name_at),
            fields.get(source_at),
            fields.get(rooms_at),
        ) else {
            continue;
        };
        if *source != "official layout" {
            continue;
        }
        let rooms: Vec<RoomId> = ids
            .split_whitespace()
            .filter_map(|id| id.parse().ok())
            .map(RoomId)
            .filter(|&id| map.room(id).is_some())
            .collect();
        if rooms.is_empty() {
            continue;
        }
        areas.push(Area {
            name: (*name).to_owned(),
            kind: AreaKind::Official,
            rooms,
        });
    }
    areas.sort_by(|a, b| a.name.cmp(&b.name));
    areas
}

/// Every distinct `meta:region:` value, with its rooms.
///
/// Unlike the location list this claims nothing and excludes nothing. It
/// is a second opinion on the whole map, and the rooms an official area
/// already covers are exactly the ones worth seeing under both names.
///
/// Rooms with no region are collected under one entry rather than
/// dropped. 11,994 of them is a third of the map -- the hunting grounds
/// Simutronics does not classify -- and "which rooms does the official
/// grouping not reach" is a question worth being able to click on.
fn region_areas(map: &Map) -> Vec<Area> {
    let mut by_region: BTreeMap<&str, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        let name = room
            .meta
            .iter()
            .find_map(|m| m.strip_prefix("region:"))
            .unwrap_or("");
        by_region.entry(name).or_default().push(room.id);
    }
    by_region
        .into_iter()
        .map(|(name, rooms)| Area {
            name: if name.is_empty() {
                "(no region)".to_owned()
            } else {
                name.to_owned()
            },
            kind: AreaKind::Region,
            rooms,
        })
        .collect()
}

/// Every distinct [`cena_map::Room::location`] among the rooms no official
/// area `claimed`, with its rooms. Rooms with no location group under one
/// bucket rather than vanishing -- a good part of the map has none, and it
/// is still worth being able to look at.
///
/// A location whose rooms are *all* claimed disappears from this list
/// entirely, which is the point: the official layout already shows those
/// rooms under a curated name, and mapdb would re-list them split into
/// ones and twos.
fn location_areas(map: &Map, claimed: &HashSet<RoomId>) -> Vec<Area> {
    let mut by_location: BTreeMap<&str, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        if claimed.contains(&room.id) {
            continue;
        }
        by_location
            .entry(room.location.as_deref().unwrap_or(""))
            .or_default()
            .push(room.id);
    }
    by_location
        .into_iter()
        .map(|(name, rooms)| Area {
            name: if name.is_empty() {
                "(no location)".to_owned()
            } else {
                name.to_owned()
            },
            kind: AreaKind::Location,
            rooms,
        })
        .collect()
}

/// The rooms to lay an area out from: its own, plus the neighbours a
/// stranded room needs to stay attached to its building.
///
/// **An area filter should not strand a room from the building it opens
/// off.** Measured on `gs.map`: 361 rooms lay out as single-room groups
/// with no connection at all inside their own area, purely because the
/// boundary cut them from their doorway -- `[Haegan's Weaponry]` is in
/// "Cysaegir" while its street is in "the village of Cysaegir", and
/// `[Ebonstone Manor, Lockers]` sits in "CHE Central" with its door in
/// Wehnimer's Landing. 335 of those have every neighbour in one other
/// area, and 309 have exactly one neighbour.
///
/// Those neighbours are pulled in **for the layout only**. The room still
/// belongs to its own area: the lists, the counts and the export are
/// unchanged, because `location` is not being second-guessed here. A
/// mapdb location that disagrees with a doorway is a fact about the data,
/// and changing it is a correction a person makes deliberately -- this
/// just stops the solver being lied to about what connects to what.
///
/// Only rooms that would otherwise be **wholly cut off** pull anything in,
/// so an area that is already whole is laid out from exactly its own
/// rooms, as before.
#[must_use]
pub fn layout_rooms(area_rooms: &[RoomId], map: &Map) -> Vec<cena_map::Room> {
    let own: HashSet<RoomId> = area_rooms.iter().copied().collect();

    // Who points at a room, so a one-way door inward still counts as an
    // attachment -- a shop entered from the street but leaving by another
    // exit is still that street's shop.
    let mut inbound: BTreeMap<RoomId, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        for exit in &room.exits {
            if own.contains(&exit.to) && !own.contains(&room.id) {
                inbound.entry(exit.to).or_default().push(room.id);
            }
        }
    }

    let mut pulled: HashSet<RoomId> = HashSet::new();
    for &id in area_rooms {
        let Some(room) = map.room(id) else {
            continue;
        };
        let neighbours: Vec<RoomId> = room
            .exits
            .iter()
            .map(|e| e.to)
            .chain(inbound.get(&id).into_iter().flatten().copied())
            .filter(|n| map.room(*n).is_some())
            .collect();
        // A room with a neighbour of its own is attached already; only one
        // with none is stranded by the boundary.
        if neighbours.is_empty() || neighbours.iter().any(|n| own.contains(n)) {
            continue;
        }
        pulled.extend(neighbours);
    }

    area_rooms
        .iter()
        .copied()
        .chain(pulled.into_iter().filter(|id| !own.contains(id)))
        .filter_map(|id| map.room(id).cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled file parses and both kinds of row are present, so a
    /// mangled copy fails here rather than as an empty list in the window.
    #[test]
    fn the_bundled_tsv_has_both_sources() {
        let mut official = 0;
        let mut mapdb = 0;
        for line in AREAS_TSV.lines().skip(1) {
            match line.split('\t').nth(1) {
                Some("official layout") => official += 1,
                Some("mapdb location") => mapdb += 1,
                _ => {}
            }
        }
        assert!(
            official > 100,
            "expected the official areas, got {official}"
        );
        assert!(mapdb > 100, "expected the mapdb areas, got {mapdb}");
    }

    /// An area whose rooms this map does not have is dropped, not offered
    /// as a name that draws nothing.
    #[test]
    fn official_areas_are_filtered_to_the_map() {
        let map = Map::from_rooms(Vec::new()).expect("an empty map is a map");
        assert!(official_areas(&map).is_empty());
    }

    /// The rule the location list exists to keep: official layout is
    /// truth, so a room it claims is never listed again under its
    /// `location`.
    #[test]
    fn a_claimed_room_is_not_listed_twice() {
        let claimed = RoomId(7);
        let rooms = vec![
            room_in(claimed, "Some Shop"),
            room_in(RoomId(8), "Some Shop"),
            room_in(RoomId(9), "Elsewhere"),
        ];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");

        let by_location = location_areas(&map, &std::iter::once(claimed).collect());
        let listed: Vec<RoomId> = by_location.iter().flat_map(|a| a.rooms.iter().copied()).collect();

        assert!(
            !listed.contains(&claimed),
            "a claimed room was listed again"
        );
        // Its unclaimed neighbour still appears, under the same name: the
        // split is per room, not per location.
        assert!(listed.contains(&RoomId(8)));
        assert!(listed.contains(&RoomId(9)));
    }

    /// A location every one of whose rooms is claimed drops out of the
    /// location list rather than lingering as an empty name.
    #[test]
    fn a_wholly_claimed_location_disappears() {
        let rooms = vec![room_in(RoomId(1), "Inside An Official Area")];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let claimed = std::iter::once(RoomId(1)).collect();
        assert!(location_areas(&map, &claimed).is_empty());
    }

    /// The region list claims nothing and drops nothing: unlike the
    /// location list it is a second opinion on the whole map, and a room
    /// an official area already covers is exactly the one worth seeing
    /// under both names.
    #[test]
    fn the_region_list_covers_every_room_including_claimed_ones() {
        let mut a = room_in(RoomId(1), "the Upper Trollfang");
        a.meta = vec!["region:QUEST: Black Swan Castle".to_owned()];
        let b = room_in(RoomId(2), "the Upper Trollfang");
        let map = Map::from_rooms(vec![a, b]).expect("no duplicate ids");

        let regions = region_areas(&map);
        let listed: Vec<RoomId> = regions.iter().flat_map(|r| r.rooms.iter().copied()).collect();
        assert!(listed.contains(&RoomId(1)) && listed.contains(&RoomId(2)));

        // One location, two regions: the quest separates from the
        // hunting ground, which is the whole reason the list exists.
        let quest = regions
            .iter()
            .find(|r| r.name == "QUEST: Black Swan Castle")
            .expect("the quest is its own entry");
        assert_eq!(quest.rooms, vec![RoomId(1)]);
        let none = regions
            .iter()
            .find(|r| r.name == "(no region)")
            .expect("unclassified rooms are kept, not dropped");
        assert_eq!(none.rooms, vec![RoomId(2)]);
    }

    /// A region and a location sharing a name keep separate corrections.
    #[test]
    fn a_region_and_a_location_of_one_name_do_not_share_a_store_key() {
        let region = Area {
            name: "Ta'Illistim".to_owned(),
            kind: AreaKind::Region,
            rooms: vec![],
        };
        let location = Area {
            name: "Ta'Illistim".to_owned(),
            kind: AreaKind::Location,
            rooms: vec![],
        };
        assert_ne!(region.store_key(), location.store_key());
    }

    /// A room with just enough filled in to carry an id and a location.
    fn room_in(id: RoomId, location: &str) -> cena_map::Room {
        cena_map::Room {
            id,
            uid: vec![],
            title: vec![],
            description: vec![],
            paths: vec![],
            location: Some(location.to_owned()),
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits: vec![],
        }
    }

    /// A room whose only doorway is in another area is drawn with that
    /// doorway, so the boundary does not leave it floating alone.
    ///
    /// Measured on `gs.map`: 361 rooms lay out as single-room groups with
    /// no connection inside their own area at all, because the filter cut
    /// them from their street -- `[Haegan's Weaponry]` is in "Cysaegir"
    /// while its door is in "the village of Cysaegir".
    #[test]
    fn a_stranded_room_brings_its_doorway_with_it() {
        let shop = RoomId(1);
        let street = RoomId(2);
        let map = Map::from_rooms(vec![
            room_linked(shop, "Cysaegir", &[street]),
            room_linked(street, "the village of Cysaegir", &[shop]),
        ])
        .expect("no duplicate ids");

        let rooms = layout_rooms(&[shop], &map);
        let ids: Vec<RoomId> = rooms.iter().map(|r| r.id).collect();
        assert!(ids.contains(&shop));
        assert!(
            ids.contains(&street),
            "the shop was laid out without the street it opens onto"
        );
    }

    /// An area that is already whole pulls nothing in: every room has a
    /// neighbour of its own, so there is nothing for the boundary to have
    /// broken.
    #[test]
    fn a_whole_area_pulls_nothing_in() {
        let (a, b, outside) = (RoomId(1), RoomId(2), RoomId(3));
        let map = Map::from_rooms(vec![
            room_linked(a, "town", &[b, outside]),
            room_linked(b, "town", &[a]),
            room_linked(outside, "elsewhere", &[a]),
        ])
        .expect("no duplicate ids");

        let ids: Vec<RoomId> = layout_rooms(&[a, b], &map).iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 2, "pulled in a neighbour that was not needed");
        assert!(!ids.contains(&outside));
    }

    /// A one-way door inward still attaches the room: a shop entered from
    /// the street but leaving by another exit is still that street's shop.
    #[test]
    fn an_inbound_only_doorway_still_counts() {
        let shop = RoomId(1);
        let street = RoomId(2);
        let map = Map::from_rooms(vec![
            room_linked(shop, "shops", &[]),
            room_linked(street, "streets", &[shop]),
        ])
        .expect("no duplicate ids");

        let ids: Vec<RoomId> = layout_rooms(&[shop], &map).iter().map(|r| r.id).collect();
        assert!(
            ids.contains(&street),
            "a room reachable only one way was left stranded"
        );
    }

    /// A room with no exits at all has nothing to pull in -- 27 rooms of
    /// `gs.map` are dead records, and inventing a neighbour for them would
    /// be worse than leaving them alone.
    #[test]
    fn a_room_with_no_exits_pulls_nothing() {
        let lone = RoomId(1);
        let map = Map::from_rooms(vec![room_linked(lone, "nowhere", &[])]).expect("one room");
        assert_eq!(layout_rooms(&[lone], &map).len(), 1);
    }

    /// A room with just enough filled in to carry an id, a location and
    /// some exits.
    fn room_linked(id: RoomId, location: &str, to: &[RoomId]) -> cena_map::Room {
        let mut room = room_in(id, location);
        room.exits = to
            .iter()
            .map(|&t| cena_map::Exit {
                to: t,
                kind: cena_map::ExitKind::Go,
                crossing: cena_map::Crossing::Command("go door".to_owned()),
                cost: Some(cena_map::Cost::Fixed(1.0)),
            })
            .collect();
        room
    }
}
