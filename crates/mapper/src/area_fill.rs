//! Putting the rooms nobody has assigned into areas, from what the map
//! already says about them.
//!
//! Only rooms that have a region and no area are touched, and every answer
//! lands in the store as an ordinary edit -- reviewable in the mapper,
//! undoable room by room, and exported to curation like any other. The
//! clues, strongest first:
//!
//! 1. **Title.** `[Halcyon Hills, Meadow]` is named for its place. When
//!    most of the rooms already in an area that share that prefix, in the
//!    same region, sit in one area, this room joins it.
//! 2. **Location.** The same vote over the mapdb `location`.
//! 3. **Pockets.** A run of area-less rooms whose every way out leads into
//!    one area is in that area: a building off a town's street is in the
//!    town. Repeated, since a filled pocket can close off the next one.
//! 4. **New areas.** What is left, grouped by title prefix: a connected run
//!    of [`NEW_AREA_MIN`] or more rooms sharing one becomes an area named
//!    by it. Shorter runs are a shop or a house, not a place of their own.
//!
//! Then pockets once more, behind the new areas.
//!
//! Buildings go in their town by design. An area that turns out to hold
//! two places is split by hand later; that is cheaper than inventing one
//! area per shop.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use cena_map::{Map, Room, RoomId};
use cena_map_layout::regions::{is_passage, is_real_room};

use crate::overrides::{Baseline, MapOverrides, RoomKey, plate_key};

/// How many of the already-placed rooms sharing a title prefix or location
/// must agree on one area before a new room follows them.
pub const AGREE: f64 = 0.8;

/// The shortest run of rooms sharing a title prefix that becomes an area
/// of its own. Player shops run one to three rooms and belong to their
/// town.
pub const NEW_AREA_MIN: usize = 6;

/// Which clue placed a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    Title,
    Location,
    Pocket,
    NewArea,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Reason::Title => "title",
            Reason::Location => "location",
            Reason::Pocket => "pocket",
            Reason::NewArea => "new area",
        }
    }
}

/// What a fill would do. Nothing is changed until [`apply`].
#[derive(Debug, Default)]
pub struct Fill {
    /// Room -> (area key, the clue that placed it).
    pub rooms: BTreeMap<RoomId, (String, Reason)>,
    /// Areas to mint: key -> name.
    pub new_areas: BTreeMap<String, String>,
    /// How many rooms had a region and no area to begin with.
    pub targets: usize,
}

impl std::fmt::Display for Fill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = |reason| self.rooms.values().filter(|(_, r)| *r == reason).count();
        write!(
            f,
            "{} of {} rooms with a region and no area placed: title {}, location {}, \
             pocket {}, {} in {} new area(s); {} left",
            self.rooms.len(),
            self.targets,
            count(Reason::Title),
            count(Reason::Location),
            count(Reason::Pocket),
            count(Reason::NewArea),
            self.new_areas.len(),
            self.targets - self.rooms.len()
        )
    }
}

/// Work out where every region-but-no-area room goes.
#[must_use]
pub fn propose(map: &Map, store: &MapOverrides, baseline: &Baseline) -> Fill {
    let real: HashSet<RoomId> = map
        .rooms()
        .iter()
        .filter(|r| is_real_room(r))
        .map(|r| r.id)
        .collect();
    let walks = walks(map, &real);
    let key = |id: RoomId| RoomKey::of(id, map);
    let mut area: HashMap<RoomId, String> = map
        .rooms()
        .iter()
        .filter_map(|r| Some((r.id, store.area_of(key(r.id), baseline)?.to_owned())))
        .collect();
    let region: HashMap<RoomId, &str> = map
        .rooms()
        .iter()
        .filter_map(|r| Some((r.id, store.region_in(key(r.id), baseline)?)))
        .collect();
    let targets: Vec<&Room> = map
        .rooms()
        .iter()
        .filter(|r| real.contains(&r.id) && region.contains_key(&r.id))
        .filter(|r| !area.contains_key(&r.id))
        .collect();
    let mut fill = Fill {
        targets: targets.len(),
        ..Fill::default()
    };

    // Both votes are taken over the areas as they stand, so neither clue
    // is voting on the other's guesses.
    let by_title = votes(map, &area, &region, title_key);
    let by_location = votes(map, &area, &region, location_key);
    for room in &targets {
        let answer = winner(&by_title, &region, room, title_key)
            .map(|a| (a, Reason::Title))
            .or_else(|| {
                winner(&by_location, &region, room, location_key).map(|a| (a, Reason::Location))
            });
        if let Some(answer) = answer {
            fill.rooms.insert(room.id, answer);
        }
    }
    area.extend(fill.rooms.iter().map(|(id, (a, _))| (*id, a.clone())));
    fill_pockets(&targets, &walks, &region, &mut area, &mut fill);

    let taken: HashSet<String> = store.known_areas(baseline).into_keys().collect();
    new_areas(&targets, &region, &taken, &mut area, &mut fill);
    fill_pockets(&targets, &walks, &region, &mut area, &mut fill);
    fill
}

/// Write a fill into the store: mint its areas, then place its rooms.
pub fn apply(fill: &Fill, map: &Map, store: &mut MapOverrides) {
    for name in fill.new_areas.values() {
        store.create_area(name);
    }
    for (id, (area, _)) in &fill.rooms {
        store.set_area(RoomKey::of(*id, map), Some(area));
    }
}

/// One row per room placed, for a person to read down: which area, and
/// which clue put it there.
#[must_use]
pub fn review_tsv(fill: &Fill, map: &Map, store: &MapOverrides, baseline: &Baseline) -> String {
    let mut out = String::from("room_id\tuid\ttitle\tregion\tarea\treason\n");
    for (id, (area, reason)) in &fill.rooms {
        let Some(room) = map.room(*id) else {
            continue;
        };
        let key = RoomKey::of(*id, map);
        let name = fill
            .new_areas
            .get(area)
            .map_or_else(|| store.area_title(area, baseline), String::as_str);
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            id.0,
            room.uid
                .first()
                .map(|u| u.0.to_string())
                .unwrap_or_default(),
            room.title.first().map_or("", String::as_str),
            store.region_in(key, baseline).unwrap_or(""),
            name,
            reason.label()
        );
    }
    out
}

/// Walkable neighbours, both ways, among real rooms.
fn walks(map: &Map, real: &HashSet<RoomId>) -> HashMap<RoomId, Vec<RoomId>> {
    let mut out: HashMap<RoomId, Vec<RoomId>> = HashMap::new();
    for room in map.rooms().iter().filter(|r| real.contains(&r.id)) {
        for exit in &room.exits {
            if is_passage(exit) && exit.to != room.id && real.contains(&exit.to) {
                out.entry(room.id).or_default().push(exit.to);
                out.entry(exit.to).or_default().push(room.id);
            }
        }
    }
    for list in out.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    out
}

/// `[Halcyon Hills, Meadow]` -> `Halcyon Hills`, as written.
fn title_name(room: &Room) -> Option<&str> {
    let title = room.title.first()?.trim();
    let inner = title.strip_prefix('[')?.trim_end_matches(']');
    let head = inner.split(',').next()?.trim();
    (!head.is_empty()).then_some(head)
}

fn title_key(room: &Room) -> Option<String> {
    title_name(room).map(str::to_lowercase)
}

fn location_key(room: &Room) -> Option<String> {
    let location = room.location.as_deref()?.trim().to_lowercase();
    let location = location.strip_prefix("the ").unwrap_or(&location);
    (!location.is_empty()).then(|| location.to_owned())
}

type Clue = fn(&Room) -> Option<String>;
type Votes = HashMap<(String, String), HashMap<String, usize>>;

/// For each (region, clue value): how many placed rooms sit in each area.
fn votes(
    map: &Map,
    area: &HashMap<RoomId, String>,
    region: &HashMap<RoomId, &str>,
    clue: Clue,
) -> Votes {
    let mut out: Votes = HashMap::new();
    for room in map.rooms() {
        let (Some(a), Some(r), Some(value)) =
            (area.get(&room.id), region.get(&room.id), clue(room))
        else {
            continue;
        };
        *out.entry(((*r).to_owned(), value))
            .or_default()
            .entry(a.clone())
            .or_default() += 1;
    }
    out
}

/// The area [`AGREE`] of the room's fellows are in, if there is one.
fn winner(
    votes: &Votes,
    region: &HashMap<RoomId, &str>,
    room: &Room,
    clue: Clue,
) -> Option<String> {
    let counts = votes.get(&((*region.get(&room.id)?).to_owned(), clue(room)?))?;
    let total: usize = counts.values().sum();
    let (area, n) = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))?;
    #[allow(clippy::cast_precision_loss)] // room counts, far below 2^52
    let share = *n as f64 / total as f64;
    (share >= AGREE).then(|| area.clone())
}

/// Area-less rooms connected within one region, each run with every area
/// it opens onto. A way into area-less ground of another region counts as
/// a way out to somewhere unknown.
fn runs(
    targets: &[&Room],
    walks: &HashMap<RoomId, Vec<RoomId>>,
    region: &HashMap<RoomId, &str>,
    area: &HashMap<RoomId, String>,
) -> Vec<(Vec<RoomId>, HashSet<Option<String>>)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for room in targets {
        if area.contains_key(&room.id) || !seen.insert(room.id) {
            continue;
        }
        let mut run = Vec::new();
        let mut exits = HashSet::new();
        let mut stack = vec![room.id];
        while let Some(id) = stack.pop() {
            run.push(id);
            for &next in walks.get(&id).into_iter().flatten() {
                if let Some(a) = area.get(&next) {
                    exits.insert(Some(a.clone()));
                } else if region.get(&next) == region.get(&id) {
                    if seen.insert(next) {
                        stack.push(next);
                    }
                } else {
                    exits.insert(None);
                }
            }
        }
        run.sort_unstable();
        out.push((run, exits));
    }
    out
}

/// Place every pocket that opens onto exactly one area, until none does.
fn fill_pockets(
    targets: &[&Room],
    walks: &HashMap<RoomId, Vec<RoomId>>,
    region: &HashMap<RoomId, &str>,
    area: &mut HashMap<RoomId, String>,
    fill: &mut Fill,
) {
    loop {
        let mut placed = false;
        for (run, exits) in runs(targets, walks, region, area) {
            let mut exits = exits.into_iter();
            let (Some(Some(to)), None) = (exits.next(), exits.next()) else {
                continue;
            };
            for id in run {
                area.insert(id, to.clone());
                fill.rooms.insert(id, (to.clone(), Reason::Pocket));
            }
            placed = true;
        }
        if !placed {
            return;
        }
    }
}

/// Area-less rooms sharing a title prefix within one region, [`NEW_AREA_MIN`]
/// or more of them, become an area named by it.
///
/// By name, not by connection: a place is often reached in stretches --
/// Sonreeva's rooms sit in three runs -- and one name in one region is one
/// place. A prefix that opens with an article (`A Dark Tunnel`) describes a
/// room rather than naming a place, and makes no area.
fn new_areas(
    targets: &[&Room],
    region: &HashMap<RoomId, &str>,
    taken: &HashSet<String>,
    area: &mut HashMap<RoomId, String>,
    fill: &mut Fill,
) {
    let mut by_name: BTreeMap<(&str, String), Vec<&Room>> = BTreeMap::new();
    for room in targets.iter().filter(|r| !area.contains_key(&r.id)) {
        let Some(prefix) = title_key(room) else {
            continue;
        };
        if prefix.starts_with("a ") || prefix.starts_with("an ") {
            continue;
        }
        by_name
            .entry((region[&room.id], prefix))
            .or_default()
            .push(room);
    }
    for ((home, _), rooms) in by_name {
        if rooms.len() < NEW_AREA_MIN {
            continue;
        }
        let Some(name) = title_name(rooms[0]) else {
            continue;
        };
        // One name can head places in several regions -- `Aradhul Road`
        // in three -- and each is its own area.
        let free = |name: &str| {
            let key = plate_key(name);
            !taken.contains(&key) && !fill.new_areas.contains_key(&key)
        };
        let mut name = name.to_owned();
        if !free(&name) {
            name = format!("{name} ({home})");
        }
        let base = name.clone();
        let mut n = 2;
        while !free(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        let key = plate_key(&name);
        for room in rooms {
            area.insert(room.id, key.clone());
            fill.rooms.insert(room.id, (key.clone(), Reason::NewArea));
        }
        fill.new_areas.insert(key, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: u32, title: &str, meta: &[&str], to: &[u32]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![title.to_owned()],
            description: vec![],
            paths: vec![],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: meta.iter().map(|m| (*m).to_owned()).collect(),
            image: None,
            exits: to
                .iter()
                .map(|&t| cena_map::Exit {
                    to: RoomId(t),
                    kind: cena_map::ExitKind::Go,
                    crossing: cena_map::Crossing::Command("go door".to_owned()),
                    cost: Some(cena_map::Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    const TOWN: &[&str] = &["area:town", "region:Landing"];
    const HILLS: &[&str] = &["area:hills", "region:Landing"];
    const LANDING: &[&str] = &["region:Landing"];

    fn run(rooms: Vec<Room>) -> (Map, Fill) {
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let fill = propose(&map, &MapOverrides::default(), &Baseline::of(&map));
        (map, fill)
    }

    fn placed(fill: &Fill, id: u32) -> Option<(&str, Reason)> {
        fill.rooms.get(&RoomId(id)).map(|(a, r)| (a.as_str(), *r))
    }

    /// A room titled like the rooms of an area joins it, wherever it is.
    #[test]
    fn a_room_titled_like_an_area_joins_it() {
        let (_, fill) = run(vec![
            room(1, "[Halcyon Hills, Meadow]", HILLS, &[]),
            room(2, "[Halcyon Hills, Brook]", HILLS, &[]),
            room(3, "[Halcyon Hills, Copse]", LANDING, &[]),
        ]);
        assert_eq!(placed(&fill, 3), Some(("hills", Reason::Title)));
    }

    /// A building off one street is in that street's town.
    #[test]
    fn a_building_off_a_town_is_in_the_town() {
        let (_, fill) = run(vec![
            room(1, "[Town Square]", TOWN, &[2]),
            room(2, "[Rone Academy, Foyer]", LANDING, &[1, 3]),
            room(3, "[Rone Academy, Library]", LANDING, &[2]),
        ]);
        assert_eq!(placed(&fill, 2), Some(("town", Reason::Pocket)));
        assert_eq!(placed(&fill, 3), Some(("town", Reason::Pocket)));
    }

    /// Ground between two areas is nobody's until a person says.
    #[test]
    fn ground_between_two_areas_is_left() {
        let (_, fill) = run(vec![
            room(1, "[Town Square]", TOWN, &[2]),
            room(2, "[A Trail]", LANDING, &[1, 3]),
            room(3, "[Halcyon Hills, Meadow]", HILLS, &[2]),
        ]);
        assert_eq!(placed(&fill, 2), None);
    }

    /// Six rooms sharing a prefix, going nowhere one area claims, make an
    /// area; three make nothing.
    #[test]
    fn a_long_run_of_one_prefix_becomes_an_area_and_a_short_one_does_not() {
        let mut rooms = vec![room(1, "[Town Square]", TOWN, &[10, 20])];
        for i in 0..6 {
            let id: u32 = 10 + i;
            let to: Vec<u32> = [id.wrapping_sub(1), id + 1]
                .into_iter()
                .filter(|t| (10..16).contains(t))
                .collect();
            rooms.push(room(id, &format!("[Spitfire, Deck {i}]"), LANDING, &to));
        }
        for i in 0..3 {
            let id: u32 = 20 + i;
            let to: Vec<u32> = [id.wrapping_sub(1), id + 1]
                .into_iter()
                .filter(|t| (20..23).contains(t))
                .collect();
            rooms.push(room(id, &format!("[Tiny Shop, Room {i}]"), LANDING, &to));
        }
        // Both runs also open onto the hills, so no pocket settles them.
        rooms.push(room(2, "[Halcyon Hills, Meadow]", HILLS, &[15, 22]));
        let (map, fill) = run(rooms);
        assert_eq!(
            fill.new_areas.get("spitfire").map(String::as_str),
            Some("Spitfire")
        );
        for id in 10..16 {
            assert_eq!(placed(&fill, id), Some(("spitfire", Reason::NewArea)));
        }
        for id in 20..23 {
            assert_eq!(placed(&fill, id), None);
        }

        let mut store = MapOverrides::default();
        apply(&fill, &map, &mut store);
        let baseline = Baseline::of(&map);
        assert_eq!(
            store.area_of(RoomKey::of(RoomId(12), &map), &baseline),
            Some("spitfire")
        );
        assert_eq!(store.area_title("spitfire", &baseline), "Spitfire");
    }

    /// One name in one region is one place, in however many stretches;
    /// a name that opens with an article is a description, not a place.
    #[test]
    fn one_name_in_stretches_is_one_area_and_a_description_is_none() {
        let mut rooms = vec![
            room(1, "[Town Square]", TOWN, &[]),
            room(2, "[Halcyon Hills, Meadow]", HILLS, &[]),
        ];
        for id in 10..16 {
            // Six one-room stretches, each opening onto both areas, so no
            // pocket takes them.
            rooms.push(room(id, "[Sonreeva, Hall]", LANDING, &[1, 2]));
        }
        for id in 20..26 {
            rooms.push(room(id, "[A Dark Tunnel]", LANDING, &[1, 2]));
        }
        let (_, fill) = run(rooms);
        assert_eq!(fill.new_areas.len(), 1);
        for id in 10..16 {
            assert_eq!(placed(&fill, id), Some(("sonreeva", Reason::NewArea)));
        }
        assert_eq!(placed(&fill, 20), None);
    }

    /// A room with no region is not this pass's to place.
    #[test]
    fn a_room_with_no_region_is_left_alone() {
        let (_, fill) = run(vec![
            room(1, "[Town Square]", TOWN, &[2]),
            room(2, "[Town Square, Alley]", &[], &[1]),
        ]);
        assert_eq!(fill.targets, 0);
        assert!(fill.rooms.is_empty());
    }
}
