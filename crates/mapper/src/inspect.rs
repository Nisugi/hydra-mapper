//! What the inspector panel says about one room.
//!
//! Three sources meet here, and keeping them apart is the point of the
//! module:
//!
//! - the **room record** ([`cena_map::Room`]) -- what the game says the
//!   place is;
//! - its **exits** -- where they go and, as importantly, *how* they are
//!   crossed, since 7,400 of the map's 84,867 exits are scripted rather
//!   than a plain command and the canvas draws them identically;
//! - the **layout** ([`cena_map_layout::Layout`]) -- which group the room
//!   landed in, how that group was packed, and any direction violations
//!   naming it.
//!
//! That last source is why this exists at all. A [`Violation`] records an
//! edge whose placed geometry contradicts its stated direction -- 955 of
//! them across the real map, in 108 areas -- and until now nothing read
//! them, so a layout that had gone wrong looked exactly like one that had
//! not.
//!
//! Pure: this module decides *what* to say, never how to paint it.

use cena_map::{Crossing, Map, Room, RoomId};
use cena_map_layout::{Layout, PackMethod, Violation};

/// How an exit is crossed, reduced to the one word the panel shows.
///
/// The map's own [`Crossing`] carries the ported script; the inspector
/// only needs to say which kind it is, and whether it can be walked at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crossed {
    /// A plain command: `north`, `go door`.
    Command,
    /// Ported from an upstream script: a guarded step list or a routine.
    Scripted,
    /// Crossed as part of a neighbouring hop; nothing is sent here.
    PassThrough,
    /// Scripted upstream and **not** ported, or a kind this build does not
    /// know. Impassable either way, which is worth saying out loud.
    Impassable,
}

impl Crossed {
    /// The word shown beside an exit.
    pub const fn label(self) -> &'static str {
        match self {
            Crossed::Command => "",
            Crossed::Scripted => "scripted",
            Crossed::PassThrough => "pass-through",
            Crossed::Impassable => "IMPASSABLE",
        }
    }

    fn of(crossing: &Crossing) -> Crossed {
        match crossing {
            Crossing::Command(_) => Crossed::Command,
            Crossing::Steps(_) | Crossing::Routine(_) => Crossed::Scripted,
            Crossing::PassThrough(_) => Crossed::PassThrough,
            Crossing::Unported(_) | Crossing::Unknown(_) => Crossed::Impassable,
        }
    }
}

/// One exit, as the panel lists it.
pub struct ExitLine {
    pub to: RoomId,
    /// The movement command, when the crossing is a plain one. Scripted
    /// crossings have no single command to show.
    pub command: Option<String>,
    pub crossed: Crossed,
    /// The destination's title.
    pub to_title: Option<String>,
    /// The exit leaves this area: the destination is a room of the wider
    /// map, not of the subset being laid out.
    ///
    /// Worth saying, and worth being able to act on. An official area
    /// excludes its own interiors -- 292 rooms hang off Wehnimer's
    /// Landing alone -- so the well and the treehouse a town square opens
    /// onto are reached only through one of these.
    pub outside: bool,
}

/// A direction violation touching the inspected room, phrased from that
/// room's point of view.
pub struct ViolationLine {
    /// The room at the other end.
    pub other: RoomId,
    /// The direction the exit claims.
    pub stated: &'static str,
    /// Where the other room actually sits, relative to this one, in cells.
    pub dx: i32,
    pub dy: i32,
}

/// Everything the panel shows for one room.
pub struct RoomFacts {
    pub id: RoomId,
    pub uids: Vec<i64>,
    /// Every title variant; rooms have day/night and seasonal forms, and
    /// 2.5% of the map carries more than one.
    pub titles: Vec<String>,
    pub description: Option<String>,
    pub paths: Option<String>,
    pub location: Option<String>,
    pub terrain: Option<String>,
    pub climate: Option<String>,
    pub tags: Vec<String>,
    pub exits: Vec<ExitLine>,

    // --- layout diagnostics ---
    /// Which component the positioner put this room in.
    pub group: usize,
    /// How that group was seated on its sheet.
    pub packing: Option<PackMethod>,
    /// The group's building name, for an interior.
    pub group_name: Option<String>,
    /// Final cell on its sheet.
    pub cell: cena_map_layout::Cell,
    /// Whether the room hosts a doorway into an interior.
    pub entrance: bool,
    /// Violations naming this room. Empty for the great majority.
    pub violations: Vec<ViolationLine>,
}

impl RoomFacts {
    /// Gather the facts for `id`, or `None` when the area does not hold it.
    ///
    /// `map` is the area's own subset, so `to_title` resolves only within
    /// the area -- an exit leading elsewhere shows as a bare id, which is
    /// the honest answer for a view that holds one area at a time.
    #[must_use]
    pub fn gather(id: RoomId, map: &Map, layout: &Layout) -> Option<RoomFacts> {
        RoomFacts::gather_in(id, map, layout, None)
    }

    /// As [`RoomFacts::gather`], with `whole` consulted for the titles of
    /// rooms outside this area, so an exit leading out of it can still be
    /// named and acted on.
    #[must_use]
    pub fn gather_in(
        id: RoomId,
        map: &Map,
        layout: &Layout,
        whole: Option<&Map>,
    ) -> Option<RoomFacts> {
        let room = map.room(id)?;
        let group = layout.groups.iter().find(|g| g.room_ids.contains(&id))?;

        Some(RoomFacts {
            id,
            uids: room.uid.iter().map(|u| u.0).collect(),
            titles: room.title.clone(),
            description: room.description.first().cloned(),
            paths: room.paths.first().cloned(),
            location: room.location.clone(),
            terrain: room.terrain.clone(),
            climate: room.climate.clone(),
            tags: room.tags.clone(),
            exits: exit_lines(room, map, whole),
            group: group.index,
            packing: group.packing,
            group_name: group.name.clone(),
            cell: group.final_cell(id),
            entrance: layout.classification.entrance_room_ids.contains(&id),
            violations: violation_lines(id, layout),
        })
    }

    /// The title to show in a heading: the first variant, or a stand-in so
    /// a titleless room still names itself by id.
    #[must_use]
    pub fn heading(&self) -> String {
        match self.titles.first() {
            Some(title) => title.clone(),
            None => format!("Room {}", self.id.0),
        }
    }
}

fn exit_lines(room: &Room, map: &Map, whole: Option<&Map>) -> Vec<ExitLine> {
    room.exits
        .iter()
        .map(|exit| {
            let inside = map.room(exit.to);
            let outside = inside.is_none();
            let title = inside
                .or_else(|| whole.and_then(|w| w.room(exit.to)))
                .and_then(|r| r.title.first())
                .cloned();
            ExitLine {
                to: exit.to,
                command: match &exit.crossing {
                    Crossing::Command(cmd) => Some(cmd.clone()),
                    _ => None,
                },
                crossed: Crossed::of(&exit.crossing),
                to_title: title,
                outside,
            }
        })
        .collect()
}

/// Every violation naming `id`, phrased from its point of view: which room
/// is at the other end, what direction the exit claimed, and the offset it
/// actually ended up at.
///
/// A violation is recorded once, on the group, with a `from` and a `to`.
/// The inspected room may be either end, so both are matched and the
/// offset is negated when it is the `to` -- "the exit into me said north"
/// still reads as a direction from me.
fn violation_lines(id: RoomId, layout: &Layout) -> Vec<ViolationLine> {
    let mut lines = Vec::new();
    for group in &layout.groups {
        for Violation {
            from,
            to,
            direction,
            actual,
        } in &group.violations
        {
            let (other, sign) = if *from == id {
                (*to, 1)
            } else if *to == id {
                (*from, -1)
            } else {
                continue;
            };
            lines.push(ViolationLine {
                other,
                stated: direction.name(),
                dx: actual.x * sign,
                dy: actual.y * sign,
            });
        }
    }
    lines
}

/// How a cell offset reads as a compass bearing, for saying where a room
/// *actually* sits when its exit claimed otherwise. Screen axes: `y` grows
/// downward, so a negative `dy` is north.
#[must_use]
pub fn bearing(dx: i32, dy: i32) -> String {
    if dx == 0 && dy == 0 {
        return "the same cell".to_owned();
    }
    let ns = match dy.cmp(&0) {
        std::cmp::Ordering::Less => "north",
        std::cmp::Ordering::Greater => "south",
        std::cmp::Ordering::Equal => "",
    };
    let ew = match dx.cmp(&0) {
        std::cmp::Ordering::Less => "west",
        std::cmp::Ordering::Greater => "east",
        std::cmp::Ordering::Equal => "",
    };
    format!("{ns}{ew}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearings_read_as_compass_points() {
        // y grows downward, so negative dy is north.
        assert_eq!(bearing(0, -3), "north");
        assert_eq!(bearing(2, 0), "east");
        assert_eq!(bearing(-1, 4), "southwest");
        assert_eq!(bearing(0, 0), "the same cell");
    }

    #[test]
    fn crossing_kinds_reduce_to_one_word() {
        assert_eq!(
            Crossed::of(&Crossing::Command("north".into())),
            Crossed::Command
        );
        assert_eq!(
            Crossed::of(&Crossing::Steps(vec![])),
            Crossed::Scripted,
            "a ported script is walkable, just not a plain command"
        );
        assert_eq!(
            Crossed::of(&Crossing::Unported(cena_map::ShapeId("abc".into()))),
            Crossed::Impassable,
            "an unported edge cannot be walked and should say so"
        );
    }
}
