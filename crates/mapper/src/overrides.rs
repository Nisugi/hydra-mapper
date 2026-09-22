//! Hand-curated corrections: a sparse diff saved beside the map and
//! applied after generation, so the solver's own output stays pristine and
//! a correction survives the next map build.
//!
//! Ported from `reference/VellumFE/src/core/layout_engine/overrides.rs`,
//! which `plan/26` §0 named as the reference for this work. Two of its
//! capabilities are built here, the two that were asked for:
//!
//! - **Moving things.** [`LocationOverrides::group_offsets`] shifts a whole
//!   group; [`LocationOverrides::room_pins`] places one room inside its
//!   group's frame.
//! - **Reassigning an area.** [`MapOverrides::membership_moves`] moves a
//!   room to another plate, and [`MapOverrides::custom_maps`] mints the
//!   plates to move it to -- `landing.well`, `landing.treehouse` and so
//!   on, for satellites that crowd a town's own sheet.
//!
//! Vellum's other override kinds (edge restyling, forced sheets, room data
//! edits) are deliberately not ported yet; the shape here leaves room for
//! them without a format change, since every field is `#[serde(default)]`.
//!
//! # Room keys, and why they are not Vellum's
//!
//! Vellum keys everything by game uid, falling back to the map's own room
//! id for the rooms that have none, and states the two spaces cannot
//! collide: *"uids are >= 7 digits, so the key spaces never collide"*.
//!
//! **That is not true of this map.** Measured against `gs.map`: 9,542 uids
//! are below 1,000,000 and 228 are negative, which produces 160 keys that
//! two different rooms both claim -- room 1207 has uid 17127, and room
//! 17127 has no uid, so both want the key `17127`. Porting the scheme
//! unchanged would let a correction on one room silently move another.
//!
//! So a [`RoomKey`] names its space instead of relying on the numbers not
//! meeting. It costs a tagged enum in the JSON and removes the whole class
//! of bug.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cena_map::{Map, RoomId};
use cena_map_layout::{Cell, Group};
use serde::{Deserialize, Serialize};

/// A room's stable identity across map builds.
///
/// Uid where the room has one, because the map's own ids are renumbered
/// when it is rebuilt and uids are not. The id is the fallback for the
/// 21% of rooms with no uid, and the variant tag keeps the two spaces
/// apart (see the module docs).
///
/// Serialized as a **string** -- `"uid:7120"`, `"id:4242"` -- because
/// every one of these lives as a JSON object key, and JSON keys are
/// strings. A derived enum representation cannot be a key at all; that is
/// a save failure on the first correction, not a formatting preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoomKey {
    /// The game's own room number: stable across map rebuilds.
    Uid(i64),
    /// This map's room id, for a room the game has never numbered. Not
    /// stable across a rebuild, and the price of being able to correct a
    /// room the game will not name.
    Id(u32),
}

impl std::fmt::Display for RoomKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoomKey::Uid(uid) => write!(f, "uid:{uid}"),
            RoomKey::Id(id) => write!(f, "id:{id}"),
        }
    }
}

impl std::str::FromStr for RoomKey {
    type Err = BadRoomKey;

    fn from_str(s: &str) -> Result<RoomKey, BadRoomKey> {
        let bad = || BadRoomKey(s.to_owned());
        match s.split_once(':') {
            Some(("uid", rest)) => rest.parse().map(RoomKey::Uid).map_err(|_| bad()),
            Some(("id", rest)) => rest.parse().map(RoomKey::Id).map_err(|_| bad()),
            _ => Err(bad()),
        }
    }
}

/// A room key that could not be parsed back out of a saved file.
#[derive(Debug)]
pub struct BadRoomKey(String);

impl std::fmt::Display for BadRoomKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} is not a room key (expected \"uid:N\" or \"id:N\")",
            self.0
        )
    }
}

impl std::error::Error for BadRoomKey {}

impl Serialize for RoomKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RoomKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<RoomKey, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl RoomKey {
    /// The key for `id` in `map`: its first uid, or the id itself.
    #[must_use]
    pub fn of(id: RoomId, map: &Map) -> RoomKey {
        match map.room(id).and_then(|r| r.uid.first()) {
            Some(uid) => RoomKey::Uid(uid.0),
            None => RoomKey::Id(id.0),
        }
    }

    /// The stable identity of a whole group: the lowest key among its
    /// rooms, uid-keyed ones first so a group keeps its anchor when a
    /// uid-less room joins it.
    #[must_use]
    pub fn anchor(group: &Group, map: &Map) -> Option<RoomKey> {
        group.room_ids.iter().map(|&id| RoomKey::of(id, map)).min()
    }
}

/// Corrections for one area's layout, keyed by the area name the picker
/// shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LocationOverrides {
    /// Cell delta applied to a whole group's placement, keyed by the
    /// group's anchor. Additive: dragging twice sums, and dragging back to
    /// zero removes the entry rather than recording a no-op.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub group_offsets: BTreeMap<RoomKey, Cell>,
    /// One room's position within its group's own frame. Absolute, not a
    /// delta: the last drag wins.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub room_pins: BTreeMap<RoomKey, Cell>,
}

impl LocationOverrides {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.group_offsets.is_empty() && self.room_pins.is_empty()
    }

    /// How many corrections this area carries, for the "Reset (n)" button.
    #[must_use]
    pub fn len(&self) -> usize {
        self.group_offsets.len() + self.room_pins.len()
    }
}

/// The whole override store, as saved to disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapOverrides {
    /// Per-area layout corrections.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locations: BTreeMap<String, LocationOverrides>,
    /// Room -> plate key. A room named here is taken out of whatever area
    /// would otherwise hold it and listed under this plate instead.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub membership_moves: BTreeMap<RoomKey, String>,
    /// Plate key -> the name shown in the picker. These are the areas a
    /// person makes: `landing.well`, `landing.treehouse`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_maps: BTreeMap<String, String>,
}

impl MapOverrides {
    /// Read the store, or start an empty one.
    ///
    /// An unreadable file is reported rather than silently discarded:
    /// Vellum warns and starts fresh, which quietly loses hand curation if
    /// a write ever half-lands. The caller decides what to do.
    pub fn load(path: &Path) -> Result<MapOverrides, LoadError> {
        let json = match std::fs::read_to_string(path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MapOverrides::default());
            }
            Err(error) => return Err(LoadError::Unreadable(error)),
        };
        serde_json::from_str(&json).map_err(LoadError::Malformed)
    }

    /// Write the store, atomically: a temporary file and a rename, so an
    /// interrupted save cannot truncate the corrections already made.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// The corrections for one area, if it has any.
    #[must_use]
    pub fn location(&self, area: &str) -> Option<&LocationOverrides> {
        self.locations.get(area)
    }

    /// Add `delta` to a group's offset, dropping the entry when the sum
    /// returns to zero so an undone drag leaves no trace.
    pub fn nudge_group(&mut self, area: &str, anchor: RoomKey, delta: Cell) {
        let location = self.locations.entry(area.to_owned()).or_default();
        let total = location.group_offsets.entry(anchor).or_default();
        total.x += delta.x;
        total.y += delta.y;
        if total.x == 0 && total.y == 0 {
            location.group_offsets.remove(&anchor);
        }
        self.prune(area);
    }

    /// Pin one room within its group's frame, or remove its pin.
    pub fn pin_room(&mut self, area: &str, key: RoomKey, pin: Option<Cell>) {
        let location = self.locations.entry(area.to_owned()).or_default();
        match pin {
            Some(cell) => location.room_pins.insert(key, cell),
            None => location.room_pins.remove(&key),
        };
        self.prune(area);
    }

    /// Forget every correction for one area.
    pub fn reset_location(&mut self, area: &str) {
        self.locations.remove(area);
    }

    /// Mint a plate. Returns its key, which is derived from the name so
    /// the same name always names the same plate.
    pub fn create_map(&mut self, name: &str) -> String {
        let key = plate_key(name);
        self.custom_maps.insert(key.clone(), name.trim().to_owned());
        key
    }

    /// Move a room to a plate, or (with `None`) return it to whichever
    /// area would hold it naturally.
    pub fn move_room(&mut self, key: RoomKey, to: Option<&str>) {
        match to {
            Some(plate) => self.membership_moves.insert(key, plate.to_owned()),
            None => self.membership_moves.remove(&key),
        };
    }

    /// Delete a plate and return every room that was in it.
    pub fn delete_map(&mut self, plate: &str) {
        self.custom_maps.remove(plate);
        self.membership_moves.retain(|_, to| to != plate);
    }

    /// Drop an area's section once it holds nothing, so the saved file
    /// does not accumulate empty objects.
    fn prune(&mut self, area: &str) {
        if self
            .locations
            .get(area)
            .is_some_and(LocationOverrides::is_empty)
        {
            self.locations.remove(area);
        }
    }
}

/// Apply one area's corrections to a freshly generated layout.
///
/// After generation, deliberately: the solver's own output is what gets
/// cached and reused, and a correction is a diff on top of it. A
/// correction whose anchor no longer resolves -- a group the map no longer
/// forms -- is skipped rather than guessed at, which is what makes the
/// store safe to keep across a map rebuild.
///
/// Returns how many corrections actually landed, so the window can say
/// when some did not.
pub fn apply(layout: &mut cena_map_layout::Layout, map: &Map, ov: &LocationOverrides) -> usize {
    if ov.is_empty() {
        return 0;
    }
    let mut applied = 0;

    let anchors: BTreeMap<RoomKey, usize> = layout
        .groups
        .iter()
        .filter_map(|g| RoomKey::anchor(g, map).map(|k| (k, g.index)))
        .collect();
    for (anchor, delta) in &ov.group_offsets {
        let Some(&idx) = anchors.get(anchor) else {
            continue;
        };
        // A group with no offset was never packed, so there is nothing to
        // shift; leaving it alone beats inventing a placement.
        if let Some(offset) = &mut layout.groups[idx].base_offset {
            offset.x += delta.x;
            offset.y += delta.y;
            applied += 1;
        }
    }

    if !ov.room_pins.is_empty() {
        let rooms: BTreeMap<RoomKey, (usize, RoomId)> = layout
            .groups
            .iter()
            .flat_map(|g| {
                g.room_ids
                    .iter()
                    .map(move |&id| (RoomKey::of(id, map), (g.index, id)))
            })
            .collect();
        for (key, pin) in &ov.room_pins {
            let Some(&(idx, id)) = rooms.get(key) else {
                continue;
            };
            layout.groups[idx].positions.insert(id, *pin);
            applied += 1;
        }
    }

    applied
}

/// Why a store could not be read.
#[derive(Debug)]
pub enum LoadError {
    Unreadable(std::io::Error),
    Malformed(serde_json::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Unreadable(error) => write!(f, "cannot be read: {error}"),
            LoadError::Malformed(error) => write!(f, "is not valid override JSON: {error}"),
        }
    }
}

/// A plate's key, derived from its display name: lowercased, with runs of
/// anything else collapsed to a single dot.
///
/// Dots rather than Vellum's dashes because the names asked for are
/// already dotted -- `landing.well`, `landing.treehouse`. The key is flat
/// either way: the dot is a naming convention that sorts related plates
/// together in the picker, not a hierarchy the code understands.
#[must_use]
pub fn plate_key(name: &str) -> String {
    let mut key = String::new();
    let mut pending_dot = false;
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() {
            if pending_dot && !key.is_empty() {
                key.push('.');
            }
            pending_dot = false;
            key.push(ch);
        } else {
            pending_dot = true;
        }
    }
    key
}

/// Where the store lives: beside the map file it corrects, named after it.
///
/// One map, one override file. Keeping it beside the map rather than in a
/// config directory means a second map file gets its own corrections
/// instead of inheriting ones written against different room ids.
#[must_use]
pub fn store_path(map_path: &Path) -> PathBuf {
    let mut name = map_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    name.push_str(".overrides.json");
    map_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plate_keys_are_dotted_and_stable() {
        assert_eq!(plate_key("Landing Well"), "landing.well");
        assert_eq!(plate_key("landing.treehouse"), "landing.treehouse");
        assert_eq!(plate_key("  Town  Square--Central "), "town.square.central");
        // Same name, same key: minting twice does not make two plates.
        assert_eq!(plate_key("Landing Well"), plate_key("landing well"));
    }

    #[test]
    fn the_store_path_sits_beside_the_map() {
        let path = store_path(Path::new("/maps/gs.map"));
        assert!(path.ends_with("gs.overrides.json"));
    }

    /// The bug this port does not inherit: a uid and an id that are the
    /// same number are different keys.
    #[test]
    fn uid_and_id_keys_never_collide() {
        assert_ne!(RoomKey::Uid(17127), RoomKey::Id(17127));
        let mut store = MapOverrides::default();
        store.move_room(RoomKey::Uid(17127), Some("a"));
        store.move_room(RoomKey::Id(17127), Some("b"));
        assert_eq!(
            store.membership_moves.len(),
            2,
            "one key overwrote the other"
        );
    }

    /// Dragging a group and dragging it back leaves nothing behind.
    #[test]
    fn a_cancelled_drag_records_nothing() {
        let mut store = MapOverrides::default();
        let anchor = RoomKey::Uid(500);
        store.nudge_group("town", anchor, Cell { x: 3, y: -2 });
        assert_eq!(
            store.locations["town"].group_offsets[&anchor],
            Cell { x: 3, y: -2 }
        );
        store.nudge_group("town", anchor, Cell { x: -3, y: 2 });
        assert!(
            store.location("town").is_none(),
            "an undone drag left a trace"
        );
    }

    /// Successive drags of the same group accumulate.
    #[test]
    fn drags_accumulate() {
        let mut store = MapOverrides::default();
        let anchor = RoomKey::Uid(7);
        store.nudge_group("town", anchor, Cell { x: 2, y: 0 });
        store.nudge_group("town", anchor, Cell { x: 1, y: 5 });
        assert_eq!(
            store.locations["town"].group_offsets[&anchor],
            Cell { x: 3, y: 5 }
        );
    }

    /// Deleting a plate releases its rooms rather than stranding them
    /// under a name the picker no longer offers.
    #[test]
    fn deleting_a_plate_releases_its_rooms() {
        let mut store = MapOverrides::default();
        let key = store.create_map("Landing Well");
        store.move_room(RoomKey::Uid(7120), Some(&key));
        store.move_room(RoomKey::Uid(9999), Some("landing.treehouse"));
        store.delete_map(&key);
        assert!(store.custom_maps.is_empty());
        assert_eq!(
            store.membership_moves.len(),
            1,
            "an unrelated move was dropped"
        );
    }

    /// A store survives a round trip through its file format.
    #[test]
    fn a_store_round_trips() {
        let mut store = MapOverrides::default();
        let key = store.create_map("Landing Well");
        store.move_room(RoomKey::Uid(7120), Some(&key));
        store.move_room(RoomKey::Id(4242), Some(&key));
        store.nudge_group("town", RoomKey::Uid(500), Cell { x: 1, y: 2 });
        store.pin_room("town", RoomKey::Id(9), Some(Cell { x: -4, y: 0 }));

        let json = serde_json::to_string_pretty(&store).expect("serializes");
        // Keys must be strings to be JSON object keys at all -- a derived
        // enum representation fails to serialize outright.
        assert!(
            json.contains("\"uid:7120\""),
            "uid key not a string: {json}"
        );
        assert!(json.contains("\"id:4242\""), "id key not a string: {json}");
        let back: MapOverrides = serde_json::from_str(&json).expect("parses");
        assert_eq!(store, back);
    }

    /// A key that is not a key is reported, not silently taken as some
    /// other room.
    #[test]
    fn a_malformed_key_is_refused() {
        assert!("uid:7120".parse::<RoomKey>().is_ok());
        assert!("id:9".parse::<RoomKey>().is_ok());
        for bad in ["7120", "uid:", "uid:abc", "what:3", ""] {
            assert!(bad.parse::<RoomKey>().is_err(), "{bad:?} parsed as a key");
        }
    }

    /// A missing file is an empty store, not an error: the first run has
    /// nothing saved yet.
    #[test]
    fn a_missing_store_is_empty() {
        let missing = Path::new("./definitely-not-here.overrides.json");
        let store = MapOverrides::load(missing).expect("a missing file is not an error");
        assert_eq!(store, MapOverrides::default());
    }
}
