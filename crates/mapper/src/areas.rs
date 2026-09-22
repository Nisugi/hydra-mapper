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
    Mapdb,
    /// Plates a person made, and moved rooms onto, to keep satellites off
    /// a town's own sheet. See [`crate::overrides`].
    Plates,
}

impl AreaKind {
    /// The tab label, and the word for a count beneath it.
    pub const fn title(self) -> &'static str {
        match self {
            AreaKind::Official => "Official",
            AreaKind::Mapdb => "Mapdb",
            AreaKind::Plates => "Plates",
        }
    }
}

/// One browsable area: a name and the rooms that make it up.
pub struct Area {
    pub name: String,
    /// Members, in map order. Official areas carry them from `areas.tsv`;
    /// mapdb areas collect every room sharing a `location`; plates collect
    /// whatever was moved onto them.
    pub rooms: Vec<RoomId>,
}

impl Area {
    /// This area without the rooms in `taken`, or `None` when that leaves
    /// nothing -- an area emptied onto plates stops being listed rather
    /// than offering a name that draws a blank sheet.
    fn without(mut self, taken: &HashSet<RoomId>) -> Option<Area> {
        self.rooms.retain(|id| !taken.contains(id));
        (!self.rooms.is_empty()).then_some(self)
    }
}

/// All three lists, each sorted by name, rebuilt whenever a room is moved
/// between plates.
pub struct Areas {
    pub official: Vec<Area>,
    pub mapdb: Vec<Area>,
    pub plates: Vec<Area>,
}

impl Areas {
    /// Build the lists for `map`, honouring the plates and membership
    /// moves in `store`.
    ///
    /// **A moved room leaves its old area.** Membership moves are applied
    /// first and win over everything: that is the whole point of moving a
    /// room onto its own plate -- the Town Well stops crowding the
    /// Landing's sheet because it is no longer *in* the Landing's list.
    ///
    /// After that, **official layout is truth, and the lists do not
    /// overlap.** A room an official area claims is not listed a second
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
        let plates = plate_areas(map, store);
        let mut claimed: HashSet<RoomId> = plates
            .iter()
            .flat_map(|a| a.rooms.iter().copied())
            .collect();

        let official: Vec<Area> = official_areas(map)
            .into_iter()
            .filter_map(|area| area.without(&claimed))
            .collect();
        claimed.extend(official.iter().flat_map(|a| a.rooms.iter().copied()));

        Areas {
            official,
            mapdb: mapdb_areas(map, &claimed),
            plates,
        }
    }

    /// The list for one kind, so the picker can hold a kind rather than
    /// branching at every use.
    #[must_use]
    pub fn list(&self, kind: AreaKind) -> &[Area] {
        match kind {
            AreaKind::Official => &self.official,
            AreaKind::Mapdb => &self.mapdb,
            AreaKind::Plates => &self.plates,
        }
    }
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
            name: store
                .custom_maps
                .get(plate)
                .cloned()
                .unwrap_or_else(|| plate.to_owned()),
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
            rooms,
        });
    }
    areas.sort_by(|a, b| a.name.cmp(&b.name));
    areas
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
fn mapdb_areas(map: &Map, claimed: &HashSet<RoomId>) -> Vec<Area> {
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
            rooms,
        })
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

    /// The rule the lists exist to keep: official layout is truth, so a
    /// room it claims is never listed again under its mapdb location.
    #[test]
    fn a_claimed_room_is_not_listed_twice() {
        let claimed = RoomId(7);
        let rooms = vec![
            room_in(claimed, "Some Shop"),
            room_in(RoomId(8), "Some Shop"),
            room_in(RoomId(9), "Elsewhere"),
        ];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");

        let mapdb = mapdb_areas(&map, &std::iter::once(claimed).collect());
        let listed: Vec<RoomId> = mapdb.iter().flat_map(|a| a.rooms.iter().copied()).collect();

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
    /// mapdb list rather than lingering as an empty name.
    #[test]
    fn a_wholly_claimed_location_disappears() {
        let rooms = vec![room_in(RoomId(1), "Inside An Official Area")];
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let claimed = std::iter::once(RoomId(1)).collect();
        assert!(mapdb_areas(&map, &claimed).is_empty());
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
}
