//! Areas derived from the room graph, with mapdb's names as labels.
//!
//! **`location` is not a partition of the map.** Measured on `gs.map`
//! (36,838 rooms): 344 location names, but 2,672 connected pieces.
//! "Wehnimer's Landing" alone is 3,229 rooms in 274 pieces, because the
//! streets are labelled "the town of Wehnimer's Landing" while each
//! building on them -- Silvergate, Helden Hall, the Museum -- carries the
//! hub name and opens onto the streets through a door or three. Grouping
//! by location severs every building from its street, and 1,608 rooms
//! have no location at all. The official layout areas cover 37% of the
//! map and stop at the town gates.
//!
//! So the grouping here is read off the graph, and the names are only
//! used to say which pieces belong to the same place:
//!
//! - A **region** is the hub a location names: "the town of X", "the city
//!   of X" and "Y, inside the frontier town of X" are all region X.
//! - A **unit** is a connected run of rooms sharing one location and one
//!   sense (indoor or outdoor): an outdoor unit is a stretch of street or
//!   country, an indoor unit is a building or part of one.
//! - An **area** is one connected run of a region's units, whichever
//!   label each stretch happens to carry -- the streets, and the
//!   buildings and yards between them -- plus every building of another
//!   or no label that opens onto it, every courtyard those buildings
//!   wall in, and every unlabelled room hanging off any of that. An
//!   indoor unit of [`ZONE_ROOMS`] or more under a name of its own (the
//!   catacombs, the sewers, Zul Logoth, which is a tunnel city and reads
//!   as one building) is a place of its own.
//!
//! Simulated on the whole map before being built: the Landing comes out
//! as one 3,149-room area instead of 274 pieces plus a separate "town
//! of"; River's Rest is one town with its Citadel (810) instead of 26
//! fragments; the guild courtyards join their guilds; Zul Logoth keeps
//! its 670 rooms to itself.

use std::collections::{BTreeMap, HashMap, HashSet};

use cena_map::{Crossing, Map, Room, RoomId};

use crate::classifier::{Sense, room_sense};

/// An indoor unit this large is a zone -- a dungeon, a sewer, an
/// underground town -- and is an area of its own rather than a building
/// on somebody's street. The same line [`crate::classifier`] draws for
/// clustering, for the same reason.
pub const ZONE_ROOMS: usize = 50;

/// A runner-up with this share (percent) of a unit's doors, against the
/// winner's, makes the attachment contested.
pub const CONTESTED_SHARE: usize = 40;

/// An outdoor unit smaller than this is a scrap -- a porch, a yard, three
/// rooms of road with their own label -- and joins whatever it opens onto.
pub const TINY_ROOMS: usize = 6;

/// The tag on a room that is a menu rather than a place.
///
/// An urchin hideout has no geography: all 16 of them are a single room
/// whose exits are `urchin guide <somewhere>` commands -- 60 of them from
/// the Landing's, reaching every shop and gate in town. Drawing it puts a
/// room on the sheet that nobody can walk to and lines across the map
/// that nobody can walk along.
///
/// The premium halls' `premium:transport`, `premium supernode` and
/// `premium teleportation jewelry` rooms are **not** this: Zephyr Hall's
/// Common Room and Seamist Hall's Central Lounge are places, with
/// ordinary doors to the rest of their building. They carry a
/// [`Crossing::PassThrough`] to a hideout as well, which
/// [`is_passage`] already declines to follow. The room stays; the
/// teleport does not.
pub const VIRTUAL_ROOM_TAG: &str = "urchin-hideout";

/// Whether a room is a place at all, or a menu wearing a room's clothes.
#[must_use]
pub fn is_real_room(room: &Room) -> bool {
    !room.tags.iter().any(|t| t == VIRTUAL_ROOM_TAG)
}

/// Whether an exit is something a person can walk along, and so whether
/// it says anything about where two rooms are in relation to each other.
///
/// Routines (the Elemental Confluence, the Rift -- travel puzzles whose
/// destinations shuffle) and urchin pass-throughs are not walks, and an
/// unported or unknown crossing cannot be walked at all. None of them is
/// a passage, for grouping *or* for drawing: a line on the map is a claim
/// that you can get there that way.
#[must_use]
pub fn is_passage(exit: &cena_map::Exit) -> bool {
    matches!(exit.crossing, Crossing::Command(_) | Crossing::Steps(_))
}

/// One derived area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedArea {
    /// The hub the area belongs to, or `None` for an area made only of
    /// unlabelled rooms.
    pub region: Option<String>,
    /// A name for the list: the location most of its rooms carry, or the
    /// region, or the title prefix its rooms share.
    pub name: String,
    pub kind: AreaKind,
    /// In map order.
    pub rooms: Vec<RoomId>,
    /// Rooms that joined this area over a close alternative: a building
    /// with doors on two areas' streets, a yard between two places. Here
    /// so a person can be shown the call rather than have it made
    /// silently.
    pub contested: Vec<RoomId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaKind {
    /// An outdoor network with its buildings.
    Streets,
    /// A zone-sized interior: its own area.
    Zone,
    /// A building or building group that reaches no street at all.
    Interior,
    /// Rooms with no exits in or out; nothing to group them by.
    Isolated,
}

/// The hub a location names. "the town of Wehnimer's Landing", "the city
/// of Ta'Vaalor", "Kodos' Cottage, outside the frontier town of
/// Wehnimer's Landing" and "Oteska's Den, inside the island town of
/// River's Rest" each name the town after "of"; anything else is its own
/// region.
#[must_use]
pub fn region_of(location: &str) -> &str {
    const SETTLEMENTS: [&str; 7] = [
        "town", "city", "village", "port", "hamlet", "outpost", "isle",
    ];
    let lower = location.to_ascii_lowercase();
    // "... <settlement> of <X>" -- the last such phrase names the hub.
    let mut best: Option<usize> = None;
    let mut from = 0;
    while let Some(at) = lower[from..].find(" of ") {
        let at = from + at;
        let before = &lower[..at];
        let word = before.rsplit(' ').next().unwrap_or("");
        if SETTLEMENTS.contains(&word) {
            best = Some(at + " of ".len());
        }
        from = at + 1;
    }
    match best {
        Some(start) => location[start..].trim(),
        None => location,
    }
}

/// Every area the map's rooms fall into, largest first.
#[must_use]
pub fn derive_areas(map: &Map) -> Vec<DerivedArea> {
    // A menu is not a place: the hideouts never reach an area at all, so
    // no consumer downstream has to know they exist.
    let rooms: Vec<Room> = map
        .rooms()
        .iter()
        .filter(|r| is_real_room(r))
        .cloned()
        .collect();
    let rooms = rooms.as_slice();
    let sense: Vec<Sense> = rooms.iter().map(room_sense).collect();
    let adj = adjacency(rooms);

    let unit_of = units(rooms, &sense, &adj);
    let unit_count = unit_of.iter().max().map_or(0, |m| m + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); unit_count];
    for (i, &u) in unit_of.iter().enumerate() {
        members[u].push(i);
    }
    let unit_sense: Vec<Sense> = members.iter().map(|m| majority(m, &sense)).collect();
    let unit_region: Vec<Option<String>> = members
        .iter()
        .map(|m| {
            rooms[m[0]]
                .location
                .as_deref()
                .map(|l| region_of(l).to_owned())
        })
        .collect();

    let mut parent: Vec<usize> = (0..unit_count).collect();
    // Adjacent units of one region are one place, indoors or out: the
    // town's streets, and Moot Hall that two of them connect through.
    // Only adjacency *within* the region counts, so the catacombs, with
    // their own name, stay their own place under the town.
    for u in 0..unit_count {
        if unit_region[u].is_none() {
            continue;
        }
        for &i in &members[u] {
            for &j in &adj[i] {
                let v = unit_of[j];
                if v != u && unit_region[v] == unit_region[u] {
                    union(&mut parent, u, v);
                }
            }
        }
    }
    let contested = attach(&mut parent, &members, &unit_of, &unit_sense, &sense, &adj);

    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for u in 0..unit_count {
        let root = find(&mut parent, u);
        by_root.entry(root).or_default().push(u);
    }
    let mut areas: Vec<DerivedArea> = by_root
        .into_values()
        .map(|units| {
            let mut ids: Vec<usize> = units
                .iter()
                .flat_map(|&u| members[u].iter().copied())
                .collect();
            ids.sort_unstable();
            let kind = if ids.len() == 1 && adj[ids[0]].is_empty() {
                AreaKind::Isolated
            } else if ids.iter().filter(|&&i| sense[i] == Sense::Outdoor).count() >= TINY_ROOMS {
                AreaKind::Streets
            } else if ids.len() >= ZONE_ROOMS {
                AreaKind::Zone
            } else {
                AreaKind::Interior
            };
            let region = commonest(
                ids.iter()
                    .filter_map(|&i| rooms[i].location.as_deref().map(region_of)),
            );
            let name = commonest(ids.iter().filter_map(|&i| rooms[i].location.as_deref()))
                .or_else(|| commonest(ids.iter().filter_map(|&i| title_prefix(&rooms[i]))))
                .unwrap_or_else(|| "(unnamed)".to_owned());
            let mut contested_here: Vec<usize> = units
                .iter()
                .filter(|u| contested.contains(u))
                .flat_map(|&u| members[u].iter().copied())
                .collect();
            contested_here.sort_unstable();
            DerivedArea {
                region,
                name,
                kind,
                rooms: ids.into_iter().map(|i| rooms[i].id).collect(),
                contested: contested_here.into_iter().map(|i| rooms[i].id).collect(),
            }
        })
        .collect();
    areas.sort_by(|a, b| {
        b.rooms
            .len()
            .cmp(&a.rooms.len())
            .then_with(|| a.name.cmp(&b.name))
    });
    gather_the_unwalkable(&mut areas);
    name_satellites(&mut areas, map);
    areas
}

/// Rooms with no passage in or out, sharing a location, are one place:
/// the Elemental Confluence and the Rift are reached by routines whose
/// destinations shuffle, so their rooms are joined by nothing that counts
/// as adjacency here, yet they are plainly one area each. Rooms with no
/// location stay isolated -- there is nothing to gather them by.
fn gather_the_unwalkable(areas: &mut Vec<DerivedArea>) {
    let mut gathered: BTreeMap<String, DerivedArea> = BTreeMap::new();
    let mut keep: Vec<DerivedArea> = Vec::new();
    for area in areas.drain(..) {
        if area.kind != AreaKind::Isolated {
            keep.push(area);
            continue;
        }
        let Some(name) = area.name.strip_suffix("").filter(|_| area.region.is_some()) else {
            keep.push(area);
            continue;
        };
        let name = name.to_owned();
        gathered
            .entry(name.clone())
            .and_modify(|g| g.rooms.extend(area.rooms.iter().copied()))
            .or_insert(area);
    }
    for (_, mut area) in gathered {
        area.rooms.sort_unstable();
        area.kind = if area.rooms.len() == 1 {
            AreaKind::Isolated
        } else {
            AreaKind::Zone
        };
        keep.push(area);
    }
    keep.sort_by(|a, b| {
        b.rooms
            .len()
            .cmp(&a.rooms.len())
            .then_with(|| a.name.cmp(&b.name))
    });
    *areas = keep;
}

/// A region's largest area keeps its name; the rest -- the
/// Spitfire, Halcyon Hills, the Carnival, each a run of "Wehnimer's
/// Landing" rooms with no road to the town -- take the title prefix
/// their rooms share, so the list does not read as eleven areas all
/// called Wehnimer's Landing. Any name still repeated gets its room count.
fn name_satellites(areas: &mut [DerivedArea], map: &Map) {
    let mut seen_region: HashMap<String, usize> = HashMap::new();
    let mut principal: HashSet<usize> = HashSet::new();
    for (at, area) in areas.iter_mut().enumerate() {
        let Some(region) = area.region.clone() else {
            continue;
        };
        let nth = seen_region.entry(region.clone()).or_insert(0);
        *nth += 1;
        if *nth == 1 || area.kind == AreaKind::Isolated {
            principal.insert(at);
            continue;
        }
        let titled = area.rooms.iter().filter_map(|&id| map.room(id));
        // "[Icemule Trace, Exterior]" is prefixed with the town's own name,
        // which says nothing; the whole title does.
        let name = commonest(titled.clone().filter_map(title_prefix))
            .filter(|p| *p != region)
            .or_else(|| commonest(titled.filter_map(title_inner)));
        if let Some(name) = name {
            area.name = name;
        }
    }
    // A satellite that still shares a name -- with the principal, or
    // with another satellite -- says how big it is. The principal never
    // changes: it is the name a person looks for.
    let mut seen_name: HashMap<String, usize> = HashMap::new();
    for area in areas.iter().filter(|a| a.kind != AreaKind::Isolated) {
        *seen_name.entry(area.name.clone()).or_default() += 1;
    }
    for (at, area) in areas.iter_mut().enumerate() {
        if !principal.contains(&at) && area.kind != AreaKind::Isolated && seen_name[&area.name] > 1
        {
            area.name = format!("{} ({} rooms)", area.name, area.rooms.len());
        }
    }
}

/// "[Moonglae Inn, Atrium]" -> "Moonglae Inn, Atrium".
fn title_inner(room: &Room) -> Option<&str> {
    let title = room.title.first()?;
    let inner = title.strip_prefix('[')?.split(']').next()?.trim();
    (!inner.is_empty()).then_some(inner)
}

/// A room with exits into this many *other* regions is a transport hub --
/// the wagon at Bloodriven that goes to twelve towns, a portmaster, a
/// caravan master, an urchin hideout -- and none of its exits say two
/// places are adjacent. The street's own exit *into* it still counts, so
/// a hideout joins its town rather than fusing every town it reaches.
pub const HUB_REGIONS: usize = 3;

/// Adjacency in both directions: a one-way door still says the two rooms
/// are one place.
///
/// **Not every exit is a passage.** Measured on gs.map, the exits that
/// cross a region boundary: 1,866 plain commands, 719 routines (477 of
/// them the Elemental Confluence, 165 the Rift -- travel puzzles whose
/// destinations shuffle), 217 ported scripts, 111 urchin pass-throughs.
/// Routines and pass-throughs never count; an unported or unknown
/// crossing is impassable and never counts; and a hub's exits never count
/// (see [`HUB_REGIONS`]).
fn adjacency(rooms: &[Room]) -> Vec<Vec<usize>> {
    let index: HashMap<RoomId, usize> = rooms.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let region: Vec<Option<&str>> = rooms
        .iter()
        .map(|r| r.location.as_deref().map(region_of))
        .collect();
    let passage = |exit: &cena_map::Exit| is_passage(exit);
    let is_hub: Vec<bool> = rooms
        .iter()
        .enumerate()
        .map(|(i, room)| {
            let mut others: HashSet<&str> = HashSet::new();
            for exit in room.exits.iter().filter(|e| passage(e)) {
                if let Some(&j) = index.get(&exit.to)
                    && let Some(there) = region[j]
                    && region[i] != Some(there)
                {
                    others.insert(there);
                }
            }
            others.len() >= HUB_REGIONS
        })
        .collect();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); rooms.len()];
    for (i, room) in rooms.iter().enumerate() {
        if is_hub[i] {
            continue;
        }
        for exit in room.exits.iter().filter(|e| passage(e)) {
            if let Some(&j) = index.get(&exit.to)
                && j != i
            {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
    }
    adj
}

/// Buildings, scraps, courtyards and the unlabelled join whatever they
/// open onto most, smallest first so a back room follows its shop before
/// the shop follows its street. Repeated until nothing moves: a building
/// behind a building finds the street through it.
///
/// Returns the units whose choice was contested -- a runner-up took at
/// least [`CONTESTED_SHARE`] of their doors -- with the rooms of each,
/// because silently handing a building with doors on two streets to
/// whichever has one more is exactly the kind of decision a person
/// should be shown.
fn attach(
    parent: &mut [usize],
    members: &[Vec<usize>],
    unit_of: &[usize],
    unit_sense: &[Sense],
    sense: &[Sense],
    adj: &[Vec<usize>],
) -> Vec<usize> {
    let mut contested: Vec<usize> = Vec::new();
    let mut order: Vec<usize> = (0..members.len()).collect();
    order.sort_by_key(|&u| members[u].len());
    // Buildings and scraps first; courtyards only once the buildings
    // have found their streets, or a small town whose neighbours are
    // still its own shops and porches reads as walled in by them.
    for courtyards in [false, true] {
        loop {
            let mut moved = false;
            for &u in &order {
                if find(parent, u) != u {
                    continue;
                }
                // The whole set rooted here, not this one unit of it: the
                // town is not a scrap because one stretch of street is.
                let set: Vec<usize> = (0..members.len())
                    .filter(|&v| find(parent, v) == u)
                    .flat_map(|v| members[v].iter().copied())
                    .collect();
                let size = set.len();
                let mut into: HashMap<usize, usize> = HashMap::new();
                let (mut to_indoor, mut to_outdoor) = (0usize, 0usize);
                for &i in &set {
                    for &j in &adj[i] {
                        let v = find(parent, unit_of[j]);
                        if v == u {
                            continue;
                        }
                        *into.entry(v).or_default() += 1;
                        if sense[j] == Sense::Indoor {
                            to_indoor += 1;
                        } else {
                            to_outdoor += 1;
                        }
                    }
                }
                let mut ranked: Vec<(usize, usize)> = into.into_iter().collect();
                ranked.sort_by_key(|&(v, n)| (std::cmp::Reverse(n), v));
                let Some(&(target, best)) = ranked.first() else {
                    continue;
                };
                let stays = match unit_sense[u] {
                    Sense::Indoor => size >= ZONE_ROOMS,
                    // A street network stays, unless it is a scrap or a
                    // courtyard walled in by buildings.
                    Sense::Outdoor => size >= TINY_ROOMS && !(courtyards && to_indoor > to_outdoor),
                    Sense::Unknown => false,
                };
                if stays {
                    continue;
                }
                if let Some(&(_, second)) = ranked.get(1)
                    && second * 100 >= (best + second) * CONTESTED_SHARE
                {
                    contested.push(u);
                }
                parent[u] = target;
                moved = true;
            }
            if !moved {
                break;
            }
        }
    }
    contested
}

/// Unit index per room: connected runs of one location and one sense.
fn units(rooms: &[Room], sense: &[Sense], adj: &[Vec<usize>]) -> Vec<usize> {
    let mut unit_of: Vec<usize> = vec![usize::MAX; rooms.len()];
    let mut next = 0;
    for start in 0..rooms.len() {
        if unit_of[start] != usize::MAX {
            continue;
        }
        unit_of[start] = next;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            for &j in &adj[i] {
                if unit_of[j] == usize::MAX
                    && rooms[j].location == rooms[i].location
                    && sense[j] == sense[i]
                {
                    unit_of[j] = next;
                    stack.push(j);
                }
            }
        }
        next += 1;
    }
    unit_of
}

fn majority(members: &[usize], sense: &[Sense]) -> Sense {
    let indoor = members
        .iter()
        .filter(|&&i| sense[i] == Sense::Indoor)
        .count();
    let outdoor = members
        .iter()
        .filter(|&&i| sense[i] == Sense::Outdoor)
        .count();
    match indoor.cmp(&outdoor) {
        std::cmp::Ordering::Greater => Sense::Indoor,
        std::cmp::Ordering::Less => Sense::Outdoor,
        std::cmp::Ordering::Equal => Sense::Unknown,
    }
}

fn commonest<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for name in names {
        *counts.entry(name).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|&(name, n)| (n, std::cmp::Reverse(name)))
        .map(|(name, _)| name.to_owned())
}

/// "[Moonglae Inn, Atrium]" -> "Moonglae Inn".
fn title_prefix(room: &Room) -> Option<&str> {
    let title = room.title.first()?;
    let inner = title.strip_prefix('[')?.split(']').next()?;
    let prefix = inner.split(',').next()?.trim();
    (!prefix.is_empty()).then_some(prefix)
}

fn find(parent: &mut [usize], mut u: usize) -> usize {
    while parent[u] != u {
        parent[u] = parent[parent[u]];
        u = parent[u];
    }
    u
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        parent[a.max(b)] = a.min(b);
    }
}

#[cfg(test)]
mod tests {
    use cena_map::{Cost, Crossing, Exit, ExitKind};

    use super::*;

    const OUT: &str = "Obvious paths: north";
    const IN: &str = "Obvious exits: out";

    fn tagged(id: u32, title: &str, location: Option<&str>, tag: &str, to: &[u32]) -> Room {
        Room {
            tags: vec![tag.to_owned()],
            ..room(id, title, location, IN, to)
        }
    }

    fn room(id: u32, title: &str, location: Option<&str>, paths: &str, to: &[u32]) -> Room {
        Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![title.to_owned()],
            description: vec![],
            paths: vec![paths.to_owned()],
            location: location.map(str::to_owned),
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits: to
                .iter()
                .map(|&t| Exit {
                    to: RoomId(t),
                    kind: ExitKind::Cardinal,
                    crossing: Crossing::Command("north".to_owned()),
                    cost: Some(Cost::Fixed(1.0)),
                })
                .collect(),
        }
    }

    fn area_of(areas: &[DerivedArea], id: u32) -> &DerivedArea {
        areas
            .iter()
            .find(|a| a.rooms.contains(&RoomId(id)))
            .unwrap_or_else(|| panic!("room {id} is in no area"))
    }

    /// A town whose streets carry two labels, with a shop under the hub
    /// name, a back room behind the shop, a walled yard behind that, an
    /// unlabelled porch, a zone-sized catacomb under its own name, and a
    /// road out to another town. mapdb location makes seven groups of
    /// this; the map makes three places.
    fn two_towns_and_a_catacomb() -> Map {
        let mut rooms = vec![
            // Streets: 1-2 "the town of Wehn", 3 plain "Wehn", back to 4.
            room(
                1,
                "[Wehn, North Rd.]",
                Some("the town of Wehn"),
                OUT,
                &[2, 10],
            ),
            room(
                2,
                "[Wehn, East Rd.]",
                Some("the town of Wehn"),
                OUT,
                &[1, 3, 30],
            ),
            room(3, "[Wehn, South Rd.]", Some("Wehn"), OUT, &[2, 4, 100]),
            room(
                4,
                "[Wehn, West Rd.]",
                Some("the town of Wehn"),
                OUT,
                &[3, 1, 200],
            ),
            // A shop under the hub label, its back room, and a yard walled
            // in by them (outdoor, but every door leads indoors).
            room(10, "[Shop, Front]", Some("Wehn"), IN, &[1, 11]),
            room(11, "[Shop, Back]", Some("Wehn"), IN, &[10, 12]),
            room(12, "[Shop, Yard]", Some("Wehn"), OUT, &[11, 13]),
            room(13, "[Shop, Yard]", Some("Wehn"), OUT, &[12, 11]),
            // An unlabelled porch off the street.
            room(30, "[Porch]", None, IN, &[2]),
            // A road to another town, and that town.
            room(100, "[Road]", Some("the road"), OUT, &[3, 101]),
            room(101, "[Road]", Some("the road"), OUT, &[100, 102]),
            room(102, "[Road]", Some("the road"), OUT, &[101, 103]),
            room(103, "[Road]", Some("the road"), OUT, &[102, 104]),
            room(104, "[Road]", Some("the road"), OUT, &[103, 105]),
            room(105, "[Road]", Some("the road"), OUT, &[104, 106]),
            room(
                106,
                "[Sol, Gate]",
                Some("the town of Sol"),
                OUT,
                &[105, 107],
            ),
            room(
                107,
                "[Sol, Square]",
                Some("the town of Sol"),
                OUT,
                &[106, 108],
            ),
            room(
                108,
                "[Sol, Square]",
                Some("the town of Sol"),
                OUT,
                &[107, 109],
            ),
            room(
                109,
                "[Sol, Square]",
                Some("the town of Sol"),
                OUT,
                &[108, 110],
            ),
            room(
                110,
                "[Sol, Square]",
                Some("the town of Sol"),
                OUT,
                &[109, 111],
            ),
            room(111, "[Sol, Square]", Some("the town of Sol"), OUT, &[110]),
        ];
        // The catacombs: a zone of their own under the town.
        #[allow(clippy::cast_possible_truncation)]
        let zone = ZONE_ROOMS as u32;
        for i in 0..zone {
            let id = 200 + i;
            let mut to = vec![];
            if i > 0 {
                to.push(id - 1);
            }
            if i + 1 < zone {
                to.push(id + 1);
            }
            if i == 0 {
                to.push(4);
            }
            rooms.push(room(id, "[Catacombs]", Some("the catacombs"), IN, &to));
        }
        Map::from_rooms(rooms).expect("no duplicate ids")
    }

    /// The Landing's urchin hideout has an exit to a hideout in each of
    /// three other towns; each town's street has an exit into its own
    /// hideout. A hideout is a menu, not a place: it lands in no area at
    /// all, and the towns it reaches stay apart.
    #[test]
    #[allow(clippy::cast_possible_truncation)] // fixture ids
    fn a_hideout_is_no_place_and_fuses_nothing() {
        let mut rooms = Vec::new();
        for (t, town) in ["Wehn", "Sol", "Ice", "Riv"].iter().enumerate() {
            let base = 100 * (t as u32 + 1);
            let loc = format!("the town of {town}");
            for i in 0..TINY_ROOMS as u32 {
                let id = base + i;
                let mut to = vec![];
                if i > 0 {
                    to.push(id - 1);
                }
                if i + 1 < TINY_ROOMS as u32 {
                    to.push(id + 1);
                }
                if i == 0 {
                    to.push(base + 50);
                }
                rooms.push(room(id, &format!("[{town}, Street]"), Some(&loc), OUT, &to));
            }
            // The hideout: back to its street, and on to the others'.
            let mut to: Vec<u32> = vec![base];
            to.extend((1..=4u32).map(|o| o * 100 + 50).filter(|&h| h != base + 50));
            rooms.push(tagged(
                base + 50,
                &format!("[{town} - Urchin Hideout]"),
                Some(town),
                VIRTUAL_ROOM_TAG,
                &to,
            ));
        }
        let map = Map::from_rooms(rooms).expect("no duplicate ids");
        let areas = derive_areas(&map);
        let towns: Vec<&DerivedArea> = [100, 200, 300, 400]
            .iter()
            .map(|&id| area_of(&areas, id))
            .collect();
        for pair in towns.windows(2) {
            assert!(
                pair[0].rooms != pair[1].rooms,
                "two towns fused: {:?}",
                pair[0].name
            );
        }
        for base in [100, 200, 300, 400] {
            assert!(
                !areas.iter().any(|a| a.rooms.contains(&RoomId(base + 50))),
                "a hideout reached an area"
            );
        }
    }

    #[test]
    fn a_town_is_its_streets_and_everything_that_opens_onto_them() {
        let map = two_towns_and_a_catacomb();
        let areas = derive_areas(&map);

        let town = area_of(&areas, 1);
        assert_eq!(town.region.as_deref(), Some("Wehn"));
        assert_eq!(town.kind, AreaKind::Streets);
        for id in [2, 3, 4, 10, 11, 12, 13, 30] {
            assert!(
                town.rooms.contains(&RoomId(id)),
                "room {id} is not in the town: {:?}",
                area_of(&areas, id).name
            );
        }
        let catacombs = area_of(&areas, 200);
        assert_eq!(catacombs.kind, AreaKind::Zone);
        assert_eq!(catacombs.rooms.len(), ZONE_ROOMS);
        let sol = area_of(&areas, 107);
        assert_eq!(sol.region.as_deref(), Some("Sol"));
        assert!(!sol.rooms.contains(&RoomId(1)), "the two towns merged");
        assert_eq!(
            areas
                .iter()
                .filter(|a| a.kind != AreaKind::Isolated)
                .count(),
            4,
            "{:?}",
            areas
                .iter()
                .map(|a| (&a.name, a.rooms.len()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_settlement_phrase_names_its_hub() {
        assert_eq!(
            region_of("the town of Wehnimer's Landing"),
            "Wehnimer's Landing"
        );
        assert_eq!(region_of("the city of Ta'Vaalor"), "Ta'Vaalor");
        assert_eq!(
            region_of("Kodos' Cottage, outside the frontier town of Wehnimer's Landing"),
            "Wehnimer's Landing"
        );
        assert_eq!(
            region_of("Oteska's Den, inside the island town of River's Rest"),
            "River's Rest"
        );
        assert_eq!(region_of("the free port of Solhaven"), "Solhaven");
        // No settlement word: the label is its own region. "the Temple of
        // Love" is not the region "Love".
        assert_eq!(region_of("the Temple of Love"), "the Temple of Love");
        assert_eq!(region_of("Wehnimer's Landing"), "Wehnimer's Landing");
    }
}
