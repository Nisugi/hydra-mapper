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
use cena_map_layout::{Cell, EdgeAction, EdgeOverride, Group};
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
    /// Edge corrections, keyed by the unordered room pair they join.
    ///
    /// Unlike the other two, these are **inputs to generation**: they
    /// change what the solver does rather than adjusting its result, so a
    /// change here re-runs the layout.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<EdgeCorrection>,
}

/// A plate: a sheet of one area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plate {
    /// What the picker shows.
    pub name: String,
    /// The area this plate is a sheet of, by the name the picker uses.
    ///
    /// Optional because a plate can outlive the area it was made under --
    /// a map rebuild can rename one -- and an orphaned plate is still
    /// browsable rather than lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
}

/// One saved edge correction.
///
/// Keyed by [`RoomKey`] rather than the room ids
/// [`cena_map_layout::EdgeOverride`] uses, because this is the form that
/// goes to disk and must survive a map rebuild. [`LocationOverrides::
/// edge_overrides`] resolves it to ids for the layout crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeCorrection {
    pub a: RoomKey,
    pub b: RoomKey,
    pub action: EdgeAction,
}

impl LocationOverrides {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.group_offsets.is_empty() && self.room_pins.is_empty() && self.edges.is_empty()
    }

    /// How many corrections this area carries, for the "Reset (n)" button.
    #[must_use]
    pub fn len(&self) -> usize {
        self.group_offsets.len() + self.room_pins.len() + self.edges.len()
    }

    /// The edge corrections as the layout crate takes them, resolved from
    /// saved keys to this map's room ids.
    ///
    /// A correction naming a room this map does not have is dropped: a
    /// saved file outlives the map it was written against, and the layout
    /// crate skips what it cannot resolve anyway.
    #[must_use]
    pub fn edge_overrides(&self, map: &Map) -> Vec<EdgeOverride> {
        let ids: BTreeMap<RoomKey, RoomId> = map
            .rooms()
            .iter()
            .map(|r| (RoomKey::of(r.id, map), r.id))
            .collect();
        self.edges
            .iter()
            .filter_map(|edge| {
                Some(EdgeOverride {
                    a: *ids.get(&edge.a)?,
                    b: *ids.get(&edge.b)?,
                    action: edge.action,
                })
            })
            .collect()
    }

    /// The action saved for the edge between `a` and `b`, either way
    /// round, so the combo shows the current setting from either end.
    #[must_use]
    pub fn edge_action(&self, a: RoomKey, b: RoomKey) -> Option<EdgeAction> {
        let (lo, hi) = ordered(a, b);
        self.edges
            .iter()
            .find(|e| e.a == lo && e.b == hi)
            .map(|e| e.action)
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
    /// Plate key -> the plate.
    ///
    /// A plate is a **sheet of one area**, not a free-floating map: it is
    /// created while looking at an area, shown beside that area's Outdoor
    /// and Interiors sheets, and exported saying which area it hangs off.
    /// That is what lets a town's satellites -- the well, the treehouse,
    /// a shop's back rooms -- be drawn apart from its main sheet while
    /// still belonging to it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_maps: BTreeMap<String, Plate>,
    /// Room -> curated area key.
    ///
    /// Separate from [`Self::membership_moves`] because an area and a
    /// plate are different things that happen to share a mechanism. A
    /// plate is a SHEET: a drawing surface carved off an area to keep its
    /// satellites off the main map. An area is a PLACE. A room belongs to
    /// one place and may be drawn on any number of sheets, so folding
    /// them into one map would make "which area is this room in" depend
    /// on where someone chose to draw it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub area_moves: BTreeMap<RoomKey, String>,
    /// Area key -> the area.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_areas: BTreeMap<String, CuratedArea>,
}

/// A curated area: a named place, its rooms assigned by hand.
///
/// It records no parent. An area's region is read from its ROOMS --
/// whichever `meta:region:` they carry -- so assigning a room is the only
/// act needed and the tree cannot disagree with the map. An area whose
/// rooms span regions is a fact worth seeing rather than a conflict to
/// resolve at creation time, and [`MapOverrides::area_region`] reports
/// the split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratedArea {
    /// What the picker shows.
    pub name: String,
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
        let mut store: MapOverrides = serde_json::from_str(&json).map_err(LoadError::Malformed)?;
        store.repair();
        Ok(store)
    }

    /// Fix what an older build could save.
    ///
    /// A plate that owns itself: `create_map` took whatever was on
    /// screen, so making a plate while already viewing one recorded the
    /// plate as its own area. The dropdown then offers only that plate,
    /// with no way back to the town it hangs off. Dropping the owner
    /// leaves it browsable and lets it be re-parented; guessing one would
    /// be worse.
    fn repair(&mut self) {
        // Self-owning: drop the owner. Leaving it browsable and
        // re-parentable beats guessing which area it meant.
        for (key, plate) in &mut self.custom_maps {
            if plate
                .area
                .as_deref()
                .is_some_and(|area| area == plate.name || area == key)
            {
                plate.area = None;
            }
        }

        // Parented to another plate: adopt that plate's own area, since
        // plates are sheets of an area and never of each other.
        let owner_by_name: BTreeMap<String, Option<String>> = self
            .custom_maps
            .values()
            .map(|plate| (plate.name.clone(), plate.area.clone()))
            .collect();
        let fixes: Vec<(String, Option<String>)> = self
            .custom_maps
            .iter()
            .filter_map(|(key, plate)| {
                let area = plate.area.as_deref()?;
                let grandparent = owner_by_name.get(area)?.clone();
                Some((key.clone(), grandparent))
            })
            .collect();
        for (key, area) in fixes {
            if let Some(plate) = self.custom_maps.get_mut(&key) {
                plate.area = area;
            }
        }
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

    /// Set, replace or clear the correction on one edge.
    ///
    /// The pair is stored in a canonical order so the same edge is one
    /// entry whichever room it was edited from. A
    /// [`EdgeAction::Direction`] is *read* `a -> b`, so when the canonical
    /// order reverses the pair the direction is flipped to match -- "east
    /// from here" saved from the other end still means the same geometry.
    pub fn set_edge(&mut self, area: &str, a: RoomKey, b: RoomKey, action: Option<EdgeAction>) {
        let location = self.locations.entry(area.to_owned()).or_default();
        let (lo, hi) = ordered(a, b);
        location.edges.retain(|e| !(e.a == lo && e.b == hi));
        if let Some(action) = action {
            let action = if (lo, hi) == (a, b) {
                action
            } else {
                match action {
                    // Directional: reversing the pair reverses the sense.
                    EdgeAction::Direction(dir) => EdgeAction::Direction(dir.opposite()),
                    // Symmetric: the same either way round.
                    EdgeAction::Connector => EdgeAction::Connector,
                }
            };
            location.edges.push(EdgeCorrection {
                a: lo,
                b: hi,
                action,
            });
            location.edges.sort_by_key(|e| (e.a, e.b));
        }
        self.prune(area);
    }

    /// Forget every correction for one area.
    pub fn reset_location(&mut self, area: &str) {
        self.locations.remove(area);
    }

    /// Mint a plate under `area`. Returns its key, which is derived from
    /// the name so the same name always names the same plate.
    pub fn create_map(&mut self, name: &str, area: Option<&str>) -> String {
        let key = plate_key(name);
        self.custom_maps.insert(
            key.clone(),
            Plate {
                name: name.trim().to_owned(),
                area: area.map(ToOwned::to_owned),
            },
        );
        key
    }

    /// The plates that are sheets of `area`, as (key, name).
    #[must_use]
    pub fn plates_of(&self, area: &str) -> Vec<(&str, &str)> {
        self.custom_maps
            .iter()
            .filter(|(_, plate)| plate.area.as_deref() == Some(area))
            .map(|(key, plate)| (key.as_str(), plate.name.as_str()))
            .collect()
    }

    /// Say which area a plate is a sheet of.
    pub fn set_plate_area(&mut self, plate: &str, area: Option<&str>) {
        if let Some(entry) = self.custom_maps.get_mut(plate) {
            entry.area = area.map(ToOwned::to_owned);
        }
    }

    /// The area a plate is a sheet of, given the plate's *display name*.
    ///
    /// `None` when the name is not a plate's -- an area is its own owner
    /// -- or when the plate has no owner recorded.
    #[must_use]
    pub fn owner_of(&self, plate_name: &str) -> Option<&str> {
        self.custom_maps
            .values()
            .find(|plate| plate.name == plate_name)
            .and_then(|plate| plate.area.as_deref())
    }

    /// Mint a curated area. Returns its key, derived from the name so
    /// the same name always names the same area.
    pub fn create_area(&mut self, name: &str) -> String {
        let key = plate_key(name);
        self.custom_areas.insert(
            key.clone(),
            CuratedArea {
                name: name.trim().to_owned(),
            },
        );
        key
    }

    /// Put a room in a curated area, or (with `None`) take it out.
    pub fn set_area(&mut self, key: RoomKey, area: Option<&str>) -> Option<String> {
        match area {
            Some(area) => self.area_moves.insert(key, area.to_owned()),
            None => self.area_moves.remove(&key),
        }
    }

    /// Delete a curated area, releasing its rooms.
    pub fn delete_area(&mut self, area: &str) {
        self.custom_areas.remove(area);
        self.area_moves.retain(|_, to| to != area);
    }

    /// A curated area's display name, falling back to its key.
    #[must_use]
    pub fn area_name<'a>(&'a self, key: &'a str) -> &'a str {
        self.custom_areas
            .get(key)
            .map_or(key, |area| area.name.as_str())
    }

    /// A plate's display name, falling back to its key for one that
    /// arrived in a hand-edited file.
    #[must_use]
    pub fn plate_name<'a>(&'a self, key: &'a str) -> &'a str {
        self.custom_maps
            .get(key)
            .map_or(key, |plate| plate.name.as_str())
    }

    /// Whether this room is drawn on a plate rather than with its area.
    #[must_use]
    pub fn is_plated(&self, key: RoomKey) -> bool {
        self.membership_moves.contains_key(&key)
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

/// The canonical order for an edge's two rooms, so one edge is one entry
/// however it was written.
fn ordered(a: RoomKey, b: RoomKey) -> (RoomKey, RoomKey) {
    if a <= b { (a, b) } else { (b, a) }
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
        let key = store.create_map("Landing Well", Some("town"));
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

    /// The same edge is one entry however it is written, and a direction
    /// saved from the far end means the same geometry.
    #[test]
    fn an_edge_is_one_entry_from_either_end() {
        use cena_map_layout::Dir;
        let (a, b) = (RoomKey::Uid(100), RoomKey::Uid(200));
        let mut store = MapOverrides::default();

        // "east, from a to b"
        store.set_edge("town", a, b, Some(EdgeAction::Direction(Dir::East)));
        // The same statement made from b is "west, from b to a", and must
        // not become a second entry.
        store.set_edge("town", b, a, Some(EdgeAction::Direction(Dir::West)));
        let location = store.location("town").expect("has corrections");
        assert_eq!(location.edges.len(), 1, "one edge became two entries");
        assert_eq!(
            location.edge_action(a, b),
            Some(EdgeAction::Direction(Dir::East))
        );
        // Read back from either end.
        assert_eq!(location.edge_action(b, a), location.edge_action(a, b));

        store.set_edge("town", b, a, None);
        assert!(store.location("town").is_none(), "clearing left a trace");
    }

    /// Saved keys resolve to this map's room ids, and a correction naming
    /// a room the map does not have is dropped rather than resolving to
    /// the wrong one.
    #[test]
    fn edge_overrides_resolve_to_room_ids() {
        use cena_map_layout::Dir;
        let map = Map::from_rooms(vec![room_with_uid(RoomId(5), 900_001)]).expect("one room");
        let mut store = MapOverrides::default();
        store.set_edge(
            "town",
            RoomKey::Uid(900_001),
            RoomKey::Uid(900_002),
            Some(EdgeAction::Direction(Dir::North)),
        );
        let location = store.location("town").expect("has corrections");
        assert!(
            location.edge_overrides(&map).is_empty(),
            "an override for a room this map lacks was resolved anyway"
        );
    }

    /// A room carrying a uid, for key-resolution tests.
    fn room_with_uid(id: RoomId, uid: i64) -> cena_map::Room {
        cena_map::Room {
            id,
            uid: vec![cena_map::Uid(uid)],
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

    /// A plate that owns itself is repaired on load: an older build took
    /// whatever was on screen as the owner, so making a plate while
    /// viewing one recorded the plate as its own area, leaving it a dead
    /// end with no way back to the town it hangs off.
    #[test]
    fn a_self_owning_plate_is_repaired() {
        let mut store = MapOverrides::default();
        store.custom_maps.insert(
            "landing.well".to_owned(),
            Plate {
                name: "landing.well".to_owned(),
                area: Some("landing.well".to_owned()),
            },
        );
        // And one parented to that plate rather than to a real area.
        store.custom_maps.insert(
            "landing.deeper".to_owned(),
            Plate {
                name: "landing.deeper".to_owned(),
                area: Some("landing.well".to_owned()),
            },
        );
        store.repair();

        assert_eq!(store.custom_maps["landing.well"].area, None);
        assert_eq!(
            store.custom_maps["landing.deeper"].area, None,
            "a plate parented to a plate kept a plate as its area"
        );
    }

    /// A plate belongs to the area it was made under, and only that
    /// area's sheets list it.
    #[test]
    fn plates_belong_to_their_area() {
        let mut store = MapOverrides::default();
        let well = store.create_map("landing.well", Some("wehnimers-landing-town"));
        store.create_map("elsewhere", Some("some-other-area"));
        store.create_map("orphan", None);

        let sheets = store.plates_of("wehnimers-landing-town");
        assert_eq!(sheets.len(), 1, "an unrelated plate was listed as a sheet");
        assert_eq!(sheets[0].0, well);
        assert_eq!(store.plate_name(&well), "landing.well");
        // A plate with no owner is not a sheet of anything, but is still
        // named -- it stays browsable rather than becoming unreachable.
        assert_eq!(store.plate_name("orphan"), "orphan");
    }

    /// Viewing a plate must lead back to the area it hangs off, or the
    /// plate is a dead end.
    #[test]
    fn a_plate_leads_back_to_its_area() {
        let mut store = MapOverrides::default();
        store.create_map("landing.well", Some("wehnimers-landing-town"));
        store.create_map("landing.treehouse", Some("wehnimers-landing-town"));

        assert_eq!(
            store.owner_of("landing.well"),
            Some("wehnimers-landing-town")
        );
        // From the plate, the area's other sheets are reachable: itself
        // and its sibling.
        let owner = store.owner_of("landing.well").expect("has an owner");
        let siblings: Vec<&str> = store.plates_of(owner).into_iter().map(|(_, n)| n).collect();
        assert_eq!(siblings, vec!["landing.treehouse", "landing.well"]);

        // An area is not a plate, so it has no owner of its own.
        assert_eq!(store.owner_of("wehnimers-landing-town"), None);
    }

    /// A store survives a round trip through its file format.
    #[test]
    fn a_store_round_trips() {
        let mut store = MapOverrides::default();
        let key = store.create_map("Landing Well", Some("town"));
        store.move_room(RoomKey::Uid(7120), Some(&key));
        store.move_room(RoomKey::Id(4242), Some(&key));
        store.nudge_group("town", RoomKey::Uid(500), Cell { x: 1, y: 2 });
        store.pin_room("town", RoomKey::Id(9), Some(Cell { x: -4, y: 0 }));
        store.set_edge(
            "town",
            RoomKey::Uid(11),
            RoomKey::Uid(12),
            Some(EdgeAction::Direction(cena_map_layout::Dir::Southwest)),
        );
        store.set_edge(
            "town",
            RoomKey::Uid(13),
            RoomKey::Id(14),
            Some(EdgeAction::Connector),
        );

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
