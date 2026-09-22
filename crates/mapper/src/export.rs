//! Exporting corrections for the map combiner.
//!
//! The combiner merges what this writes into the `.map` file that Hydra
//! and Vellum load. **Those clients run the layout engine themselves**, so
//! what they need is not positions -- it is the corrected *inputs* a
//! layout is derived from, which is why this exports edge corrections and
//! plate membership rather than a grid of cells.
//!
//! # `dirto`, the field upstream already has
//!
//! The mapdb room record carries `dirto`: a map from destination room to a
//! curated direction string, read before the movement command when the
//! layout engine resolves an edge (`reference/VellumFE/src/core/
//! layout_engine/direction.rs`, `direction_for_connection` step 1). Its
//! vocabulary is exactly what this editor produces:
//!
//! | Saved here | `dirto` value | What the engine does |
//! |---|---|---|
//! | [`EdgeAction::Direction`] | `"north"`, `"southwest"`, ... | Positions by that bearing |
//! | [`EdgeAction::Connector`] | `"cross-group"` | Does not use the edge for positioning |
//!
//! (`"none"` and `"skip"` also exist upstream and mean "fall through to
//! the command text", which is the same as having no correction at all, so
//! this never writes them.)
//!
//! Exporting into a field the engine already reads means the combiner has
//! nothing to invent and the clients need no new code: a corrected map
//! simply lays out correctly.
//!
//! # Plates
//!
//! Plate membership has no upstream field, so it is exported beside the
//! `dirto` patch as its own section, in the `map` slug terms `plan/21` §3f
//! already specifies for a room ("one map per room"). An area's interiors
//! used to export as a separate slug, being packed as their own grid;
//! they are laid out on the area's one sheet now, beside the streets they
//! open off, and travel under no slug of their own.

use std::collections::BTreeMap;

use cena_map::{Map, RoomId};
use cena_map_layout::EdgeAction;
use serde::{Deserialize, Serialize};

use crate::overrides::{MapOverrides, RoomKey};
use crate::placement::{self, Placement, Resolved};

/// The suffix of an area's second picture: the one with every building in
/// focus. Once a map slug of its own, when the interiors were a separate
/// grid; now only a picture's name.
pub const INTERIORS_SUFFIX: &str = ".interiors";

/// The correction format's current version.
///
/// A consumer that does not know this number should refuse the file rather
/// than guess at it.
///
/// Version 1 was: uid-keyed `dirto`, `map_membership`, `maps` and `area`.
/// Version 2 adds `placement`, and bumped the number although the field is
/// optional -- a v1 reader would ignore it and silently drop corrections a
/// person made, which is worse than refusing the file outright.
pub const FORMAT_VERSION: u32 = 2;

/// What a file with no `version` is assumed to be.
///
/// The pilot exports written before the field existed are version 1 in
/// every respect but saying so, so reading them as 1 is correct rather
/// than merely convenient -- and is **not** the current version, which
/// carries a field those files could not have had.
const fn default_version() -> u32 {
    1
}

/// What the combiner is given.
///
/// Room references are **uids**, the game's own numbers, because the
/// combiner merges into a map whose room ids it assigns itself. A room
/// with no uid cannot be named in a way that survives that, so its
/// corrections are reported as skipped rather than written under an id
/// that will mean a different room next build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    /// The format's version, so a consumer can refuse what it does not
    /// understand rather than half-reading it.
    ///
    /// Bumped when a field changes meaning, when a required one is added,
    /// or when an optional one carries corrections an older reader would
    /// drop rather than merely not understand -- silently losing an edit
    /// is worse than refusing the file.
    #[serde(default = "default_version")]
    pub version: u32,
    /// What produced this, so a file found later explains itself.
    pub generator: String,
    /// The map file the corrections were made against, for provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_map: Option<String>,
    /// `dirto` patches: room uid -> destination uid -> direction string.
    /// Merged into the room record's own `dirto` field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dirto: BTreeMap<i64, BTreeMap<i64, String>>,
    /// Room uid -> map slug: the plates a person made.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub map_membership: BTreeMap<i64, String>,
    /// Map slug -> the plate it names.
    ///
    /// Each carries the area it is a sheet of, so the combiner knows
    /// where a plate attaches rather than treating it as a map adrift.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub maps: BTreeMap<String, ExportedPlate>,
    /// Room uid -> the area it belongs to, for every room the export
    /// puts on a plate.
    ///
    /// A plate is a **grid**, not a place: the Town Well is drawn on
    /// `landing.well` and is still a room of Wehnimer's Landing. Without
    /// this, a consumer reading only `map_membership` would lose the area
    /// -- which is the one fact travel and every area-grouping need.
    /// `plan/21` §3f keeps the same pair apart, as `location` and `map`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub area: BTreeMap<i64, String>,
    /// Room uid -> where it sits relative to a room that did not move.
    ///
    /// A drag, stated as a relationship rather than as the cell it was
    /// recorded in. The editor works the offset out against the solve in
    /// front of it; a consumer applies it after its own solve, exactly as
    /// the editor does, which is why no coordinate ever travels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placement: BTreeMap<i64, Placement>,
    /// Map slug -> an SVG drawing of that sheet.
    ///
    /// Review evidence, not part of the correction: nothing here is
    /// merged into a map, and a submission is complete without it. It
    /// rides inside the file because the issue form takes one
    /// attachment, and gathering a folder of loose pictures to submit
    /// alongside is work a person should not have to do.
    ///
    /// A reader that wants the files back writes each value out under
    /// its slug; they are plain SVG text, not encoded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pictures: BTreeMap<String, String>,
}

/// Written by hand rather than derived, because a derived `Default` would
/// give `version: 0` -- a number no reader should ever see.
impl Default for Export {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            generator: String::new(),
            source_map: None,
            dirto: BTreeMap::new(),
            map_membership: BTreeMap::new(),
            maps: BTreeMap::new(),
            area: BTreeMap::new(),
            placement: BTreeMap::new(),
            pictures: BTreeMap::new(),
        }
    }
}

/// A plate as the combiner receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedPlate {
    /// What to show it as.
    pub name: String,
    /// The area it is a sheet of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
}

/// A correction that could not be exported, and why -- reported rather
/// than dropped, so a person can see that their edit did not travel.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub what: String,
    pub why: &'static str,
}

/// Why a correction cannot cross into the combiner's terms.
const NO_UID: &str = "room has no uid, so it cannot be named across a map rebuild";
const NOT_IN_MAP: &str = "room is not in this map";

/// Build the export for `store` against `map`.
///
/// Returns the export and whatever had to be left out.
#[must_use]
pub fn build(
    store: &MapOverrides,
    map: &Map,
    source_map: Option<String>,
    area_of: &dyn Fn(RoomId) -> Option<String>,
    placements: &[Resolved],
    pictures: BTreeMap<String, String>,
) -> (Export, Vec<Skipped>) {
    let mut export = Export {
        version: FORMAT_VERSION,
        generator: concat!("hydra-mapper ", env!("CARGO_PKG_VERSION")).to_owned(),
        source_map,
        pictures,
        ..Export::default()
    };
    let mut skipped = Vec::new();

    for (area, location) in &store.locations {
        for edge in &location.edges {
            let (Some(a), Some(b)) = (uid_of(edge.a, map), uid_of(edge.b, map)) else {
                skipped.push(Skipped {
                    what: format!("{area}: edge {} <-> {}", edge.a, edge.b),
                    why: reason(edge.a, edge.b, map),
                });
                continue;
            };
            // Written both ways: `dirto` is per room, and the engine reads
            // the entry on whichever room it is resolving an edge from.
            let value = dirto_value(edge.action);
            export.dirto.entry(a).or_default().insert(b, value.clone());
            let back = match edge.action {
                EdgeAction::Direction(dir) => dir.opposite().name().to_owned(),
                EdgeAction::Connector => value,
            };
            export.dirto.entry(b).or_default().insert(a, back);
        }
    }

    for (key, plate) in &store.membership_moves {
        match uid_of(*key, map) {
            Some(uid) => {
                export.map_membership.insert(uid, plate.clone());
                // The area the room still belongs to, recorded because
                // the plate replaced its grid, not its place.
                if let Some(id) = room_id_of(*key, map)
                    && let Some(area) = area_of(id)
                {
                    export.area.insert(uid, area);
                }
            }
            None => skipped.push(Skipped {
                what: format!("plate move of {key} to {plate}"),
                why: reason(*key, *key, map),
            }),
        }
    }

    let (placed, unnameable) = placement::for_export(placements, map);
    export.placement = placed;
    for r in unnameable {
        skipped.push(Skipped {
            what: format!("placement of room {} from {}", r.room.0, r.anchor.0),
            why: NO_UID,
        });
    }

    // Only the plates the export actually references, so a plate that was
    // minted and left empty does not travel as a map with no rooms.
    for plate in export.map_membership.values() {
        if let Some(entry) = store.custom_maps.get(plate) {
            export.maps.insert(
                plate.clone(),
                ExportedPlate {
                    name: entry.name.clone(),
                    area: entry.area.clone(),
                },
            );
        }
    }

    (export, skipped)
}

/// The `dirto` string for one action, in the vocabulary
/// `direction_for_connection` already reads.
fn dirto_value(action: EdgeAction) -> String {
    match action {
        EdgeAction::Direction(dir) => dir.name().to_owned(),
        // Upstream's own word for "connected, but do not position by it".
        EdgeAction::Connector => "cross-group".to_owned(),
    }
}

/// A room's uid, or `None` when it has none or is not in this map.
fn uid_of(key: RoomKey, map: &Map) -> Option<i64> {
    match key {
        RoomKey::Uid(uid) => Some(uid),
        // An id-keyed correction is only exportable if that room turns out
        // to have a uid after all, which it does not: the key is an id
        // precisely because it had none.
        RoomKey::Id(id) => map
            .room(RoomId(id))
            .and_then(|r| r.uid.first())
            .map(|u| u.0),
    }
}

/// The room a key names in this map.
fn room_id_of(key: RoomKey, map: &Map) -> Option<RoomId> {
    match key {
        RoomKey::Uid(uid) => map
            .rooms()
            .iter()
            .find(|r| r.uid.iter().any(|u| u.0 == uid))
            .map(|r| r.id),
        RoomKey::Id(id) => map.room(RoomId(id)).map(|r| r.id),
    }
}

/// Which of the two problems stopped a correction being exported.
fn reason(a: RoomKey, b: RoomKey, map: &Map) -> &'static str {
    let missing = |key: RoomKey| match key {
        RoomKey::Uid(uid) => !map.rooms().iter().any(|r| r.uid.iter().any(|u| u.0 == uid)),
        RoomKey::Id(id) => map.room(RoomId(id)).is_none(),
    };
    if missing(a) || missing(b) {
        NOT_IN_MAP
    } else {
        NO_UID
    }
}

impl Export {
    /// Write the export as pretty JSON, atomically.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// Whether there is anything to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dirto.is_empty() && self.map_membership.is_empty() && self.placement.is_empty()
    }

    /// How many corrections this carries, for the button's label.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dirto.values().map(BTreeMap::len).sum::<usize>()
            + self.map_membership.len()
            + self.placement.len()
    }
}

/// Where an export is written: beside the map, named after it.
#[must_use]
pub fn export_path(map_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = map_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    name.push_str(".corrections.json");
    map_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map_layout::Dir;

    fn room(id: u32, uid: Option<i64>) -> cena_map::Room {
        cena_map::Room {
            id: RoomId(id),
            uid: uid.map(cena_map::Uid).into_iter().collect(),
            title: vec![],
            description: vec![],
            paths: vec![],
            location: None,
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

    /// A forced bearing becomes a `dirto` entry on both rooms, the far one
    /// carrying the opposite -- the engine reads whichever end it is
    /// resolving from.
    #[test]
    fn a_bearing_exports_as_dirto_both_ways() {
        let map = Map::from_rooms(vec![room(1, Some(7_000_001)), room(2, Some(7_000_002))])
            .expect("two rooms");
        let mut store = MapOverrides::default();
        store.set_edge(
            "town",
            RoomKey::Uid(7_000_001),
            RoomKey::Uid(7_000_002),
            Some(EdgeAction::Direction(Dir::East)),
        );

        let (export, skipped) = build(&store, &map, None, &|_| None, &[], BTreeMap::new());
        assert!(skipped.is_empty());
        assert_eq!(export.dirto[&7_000_001][&7_000_002], "east");
        assert_eq!(
            export.dirto[&7_000_002][&7_000_001], "west",
            "the far room did not get the opposite bearing"
        );
    }

    /// A passage exports as upstream's own `cross-group`, which the layout
    /// engine already knows not to position by.
    #[test]
    fn a_passage_exports_as_cross_group() {
        let map = Map::from_rooms(vec![room(1, Some(7_000_001)), room(2, Some(7_000_002))])
            .expect("two rooms");
        let mut store = MapOverrides::default();
        store.set_edge(
            "town",
            RoomKey::Uid(7_000_001),
            RoomKey::Uid(7_000_002),
            Some(EdgeAction::Connector),
        );

        let (export, _) = build(&store, &map, None, &|_| None, &[], BTreeMap::new());
        assert_eq!(export.dirto[&7_000_001][&7_000_002], "cross-group");
        assert_eq!(export.dirto[&7_000_002][&7_000_001], "cross-group");
    }

    /// A correction on a uid-less room is reported, not silently dropped
    /// and not written under an id that means nothing after a rebuild.
    #[test]
    fn a_uidless_room_is_reported_not_exported() {
        let map =
            Map::from_rooms(vec![room(1, Some(7_000_001)), room(2, None)]).expect("two rooms");
        let mut store = MapOverrides::default();
        store.set_edge(
            "town",
            RoomKey::Uid(7_000_001),
            RoomKey::Id(2),
            Some(EdgeAction::Connector),
        );

        let (export, skipped) = build(&store, &map, None, &|_| None, &[], BTreeMap::new());
        assert!(export.dirto.is_empty(), "exported an unusable key");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].why, NO_UID);
    }

    /// Plates travel with the rooms on them, and an empty plate does not.
    #[test]
    fn plates_export_with_their_rooms() {
        let map = Map::from_rooms(vec![room(1, Some(7120))]).expect("one room");
        let mut store = MapOverrides::default();
        let key = store.create_map("landing.well", Some("the town of Wehnimer's Landing"));
        store.move_room(RoomKey::Uid(7120), Some(&key));
        store.create_map("landing.empty", None);

        let (export, _) = build(&store, &map, None, &|_| None, &[], BTreeMap::new());
        assert_eq!(export.map_membership[&7120], "landing.well");
        assert_eq!(export.maps.len(), 1, "an empty plate travelled anyway");
        assert!(export.maps.contains_key("landing.well"));
    }

    /// A plate is a grid, not a place: the room's area travels with it,
    /// so nothing downstream loses where the room actually is.
    #[test]
    fn a_plated_room_keeps_its_area() {
        let map = Map::from_rooms(vec![room(1, Some(7122))]).expect("one room");
        let mut store = MapOverrides::default();
        let key = store.create_map("landing.well", Some("the town of Wehnimer's Landing"));
        store.move_room(RoomKey::Uid(7122), Some(&key));

        let (export, _) = build(
            &store,
            &map,
            None,
            &|_| Some("the town of Wehnimer's Landing".to_owned()),
            &[],
            BTreeMap::new(),
        );
        assert_eq!(export.map_membership[&7122], "landing.well");
        assert_eq!(
            export.area[&7122], "the town of Wehnimer's Landing",
            "the room's area did not travel with its plate"
        );
    }

    /// Every export says which format it is, so a validator can refuse a
    /// version it does not know instead of half-reading it.
    #[test]
    fn an_export_states_its_format_version() {
        let map = Map::from_rooms(vec![room(1, Some(7120))]).expect("one room");
        let (export, _) = build(
            &MapOverrides::default(),
            &map,
            None,
            &|_| None,
            &[],
            BTreeMap::new(),
        );
        assert_eq!(export.version, FORMAT_VERSION);

        let json = serde_json::to_string(&export).expect("serializes");
        assert!(
            json.contains(&format!("\"version\":{FORMAT_VERSION}")),
            "the version did not reach the file: {json}"
        );
    }

    /// The pilot exports predate the field, and are version 1 in all but
    /// saying so -- reading them as 1 is correct, not a guess.
    #[test]
    fn a_file_without_a_version_reads_as_version_one() {
        let json = r#"{"generator":"hydra-mapper 0.1.0","map_membership":{"4124007":"pilot"}}"#;
        let export: Export = serde_json::from_str(json).expect("parses");
        assert_eq!(export.version, 1);
        assert_eq!(export.map_membership[&4_124_007], "pilot");
    }

    /// A drag travels as an offset from a room that did not move, with
    /// both ends named by uid.
    #[test]
    fn a_placement_exports_against_its_anchor() {
        let map = Map::from_rooms(vec![room(1, Some(7_000_001)), room(2, Some(7_000_002))])
            .expect("two rooms");
        let placements = vec![Resolved {
            room: RoomId(2),
            anchor: RoomId(1),
            dx: 3,
            dy: -2,
        }];

        let (export, skipped) = build(
            &MapOverrides::default(),
            &map,
            None,
            &|_| None,
            &placements,
            BTreeMap::new(),
        );
        assert!(skipped.is_empty());
        assert_eq!(export.placement[&7_000_002].anchor, 7_000_001);
        assert_eq!(
            (
                export.placement[&7_000_002].dx,
                export.placement[&7_000_002].dy
            ),
            (3, -2)
        );
        assert!(!export.is_empty(), "a placement alone is still an export");
        assert_eq!(export.len(), 1);
    }

    /// A placement whose anchor has no uid is reported, not written under
    /// a room id that means something else after a rebuild.
    #[test]
    fn a_placement_with_an_unnameable_anchor_is_reported() {
        let map =
            Map::from_rooms(vec![room(1, None), room(2, Some(7_000_002))]).expect("two rooms");
        let placements = vec![Resolved {
            room: RoomId(2),
            anchor: RoomId(1),
            dx: 1,
            dy: 0,
        }];

        let (export, skipped) = build(
            &MapOverrides::default(),
            &map,
            None,
            &|_| None,
            &placements,
            BTreeMap::new(),
        );
        assert!(export.placement.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].why, NO_UID);
    }

    /// Pictures ride inside the file and come back out intact, which is
    /// the whole point of carrying them there: one attachment, and an
    /// extractor writes each value straight out as a `.svg`.
    #[test]
    fn pictures_travel_inside_the_file() {
        let map = Map::from_rooms(vec![room(1, Some(7120))]).expect("one room");
        let mut store = MapOverrides::default();
        let key = store.create_map("landing.well", Some("landing"));
        store.move_room(RoomKey::Uid(7120), Some(&key));

        let doc = "<svg xmlns=\"http://www.w3.org/2000/svg\"><title>a &amp; b</title></svg>";
        let mut pictures = BTreeMap::new();
        pictures.insert("landing.well".to_owned(), doc.to_owned());

        let (export, _) = build(&store, &map, None, &|_| None, &[], pictures);
        let json = serde_json::to_string(&export).expect("serializes");
        let back: Export = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.pictures["landing.well"], doc);
    }

    /// A picture is evidence, not a correction: it must not make an
    /// otherwise-empty export look like it has something to merge.
    #[test]
    fn pictures_alone_are_not_an_export() {
        let map = Map::from_rooms(vec![room(1, Some(7120))]).expect("one room");
        let mut pictures = BTreeMap::new();
        pictures.insert("town".to_owned(), "<svg/>".to_owned());

        let (export, _) = build(
            &MapOverrides::default(),
            &map,
            None,
            &|_| None,
            &[],
            pictures,
        );
        assert!(
            export.is_empty(),
            "pictures alone were treated as corrections to merge"
        );
        assert_eq!(export.len(), 0);
    }

    /// A default-constructed export is never version 0: that number means
    /// "nobody set this", and no reader should have to interpret it.
    #[test]
    fn a_default_export_is_not_version_zero() {
        assert_eq!(Export::default().version, FORMAT_VERSION);
    }
}
