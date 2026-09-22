//! The two area lists a person can browse the map by.
//!
//! A room belongs to **two** different groupings at once, and neither one
//! contains the other. `research/jev-trial/areas.py` measured the overlap
//! and found it genuinely cross-cutting: *"66 official areas span several
//! locations and 53 locations span several official areas."* So the window
//! offers both lists rather than picking a winner:
//!
//! - **Official** -- Simutronics' own layout areas, the unit their
//!   hand-drawn artwork is positioned in. Only some of the map has them.
//! - **Mapdb** -- what the game's `location` verb answers, carried on
//!   [`cena_map::Room::location`]. Covers the rest.
//!
//! Only the official list needs a data file. `gs.map` cannot supply it:
//! the map carries `image.file`, but those are raw artwork filenames, both
//! inconsistent (`JourneysEnd.jpg` beside `Journeys_End.jpg`) and not the
//! area names -- the clean slugs come from mapdb's `layouts.json` matched
//! to rooms by uid, which `areas.py` does upstream. Rather than depend on
//! that pipeline at runtime, its output ships with this crate, so the
//! explorer stays standalone as `plan/26` requires.

use std::collections::BTreeMap;

use cena_map::{Map, RoomId};

/// `areas.tsv` as produced by `research/jev-trial/areas.py`, compiled in so
/// the binary needs no data file beside it.
///
/// A frozen snapshot, and the one real cost of bundling: areas added to the
/// map after this file was generated fall back to their mapdb location and
/// simply do not appear in the official list. Refreshing it is a file copy,
/// not a code change.
const AREAS_TSV: &str = include_str!("../data/areas.tsv");

/// Which of the two groupings a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaKind {
    /// Simutronics' own layout areas (`source` = `official layout`).
    Official,
    /// The game's `location` verb (`source` = `mapdb location`).
    Mapdb,
}

impl AreaKind {
    /// The tab label, and the word for a count beneath it.
    pub const fn title(self) -> &'static str {
        match self {
            AreaKind::Official => "Official",
            AreaKind::Mapdb => "Mapdb",
        }
    }
}

/// One browsable area: a name and the rooms that make it up.
pub struct Area {
    pub name: String,
    /// Members, in map order. Official areas carry them from `areas.tsv`;
    /// mapdb areas collect every room sharing a `location`.
    pub rooms: Vec<RoomId>,
}

/// Both lists, each sorted by name, built once at startup.
pub struct Areas {
    pub official: Vec<Area>,
    pub mapdb: Vec<Area>,
}

impl Areas {
    /// Build both lists for `map`.
    ///
    /// The official list is intersected with the rooms actually present:
    /// `areas.tsv` was generated against one map file, and a room it names
    /// that this map does not have would otherwise produce an area that
    /// draws as a hole.
    #[must_use]
    pub fn build(map: &Map) -> Areas {
        Areas {
            official: official_areas(map),
            mapdb: mapdb_areas(map),
        }
    }

    /// The list for one kind, so the picker can hold a kind rather than
    /// branching at every use.
    #[must_use]
    pub fn list(&self, kind: AreaKind) -> &[Area] {
        match kind {
            AreaKind::Official => &self.official,
            AreaKind::Mapdb => &self.mapdb,
        }
    }
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

/// Every distinct [`cena_map::Room::location`], with its rooms. Rooms with
/// no location group under one bucket rather than vanishing -- a third of
/// the map has none, and it is still worth being able to look at.
fn mapdb_areas(map: &Map) -> Vec<Area> {
    let mut by_location: BTreeMap<&str, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
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
}
