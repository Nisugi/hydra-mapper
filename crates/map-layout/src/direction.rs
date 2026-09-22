//! Direction analysis — ported from `reference/VellumFE/src/core/
//! layout_engine/direction.rs` (`connection-analyzer.js` upstream of that),
//! adapted to `cena_map::Exit`'s already-typed crossing (`plan/26` §3).
//!
//! Resolves the direction of an exit from its `ExitKind`/`Crossing` text, or
//! the reverse edge. There is no `dirto`-equivalent override on `cena_map::
//! Exit` yet (the override system is out of scope for v1, `plan/26` §0), so
//! curated overrides are not read here; when they exist, they plug in ahead
//! of step 1 exactly where Vellum's `dirto` check sits today.

use std::collections::HashMap;

use cena_map::{Crossing, ExitKind, Map, RoomId};

use serde::{Deserialize, Serialize};

use crate::overrides::EdgeOverride;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    North,
    South,
    East,
    West,
    Northeast,
    Northwest,
    Southeast,
    Southwest,
    Up,
    Down,
}

impl Dir {
    /// The 10 recognized cardinal names (note: `out` is NOT one of them).
    #[must_use]
    pub fn from_exact(s: &str) -> Option<Dir> {
        Some(match s {
            "north" => Dir::North,
            "south" => Dir::South,
            "east" => Dir::East,
            "west" => Dir::West,
            "northeast" => Dir::Northeast,
            "northwest" => Dir::Northwest,
            "southeast" => Dir::Southeast,
            "southwest" => Dir::Southwest,
            "up" => Dir::Up,
            "down" => Dir::Down,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Dir::North => "north",
            Dir::South => "south",
            Dir::East => "east",
            Dir::West => "west",
            Dir::Northeast => "northeast",
            Dir::Northwest => "northwest",
            Dir::Southeast => "southeast",
            Dir::Southwest => "southwest",
            Dir::Up => "up",
            Dir::Down => "down",
        }
    }

    #[must_use]
    pub const fn opposite(self) -> Dir {
        match self {
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::East => Dir::West,
            Dir::West => Dir::East,
            Dir::Northeast => Dir::Southwest,
            Dir::Southwest => Dir::Northeast,
            Dir::Northwest => Dir::Southeast,
            Dir::Southeast => Dir::Northwest,
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
        }
    }

    /// Grid offset. Up/down borrow the N/S offsets as placement conveniences
    /// (`plan/26` §0: world elevation is out of scope; this is render-plane
    /// placement only, matching Vellum's own "up/down layering: out of
    /// scope" note).
    #[must_use]
    pub const fn offset(self) -> (i32, i32) {
        match self {
            Dir::North | Dir::Up => (0, -1),
            Dir::South | Dir::Down => (0, 1),
            Dir::East => (1, 0),
            Dir::West => (-1, 0),
            Dir::Northeast => (1, -1),
            Dir::Northwest => (-1, -1),
            Dir::Southeast => (1, 1),
            Dir::Southwest => (-1, 1),
        }
    }

    /// True 2D geometry only — up/down are excluded from validation and
    /// optimization.
    #[must_use]
    pub const fn is_compass(self) -> bool {
        !matches!(self, Dir::Up | Dir::Down)
    }
}

/// Scan order for extracting a direction from command text: longest names
/// first so "northeast" wins over "north".
const SCAN_ORDER: [Dir; 10] = [
    Dir::Northeast,
    Dir::Northwest,
    Dir::Southeast,
    Dir::Southwest,
    Dir::North,
    Dir::South,
    Dir::East,
    Dir::West,
    Dir::Down,
    Dir::Up,
];

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `\b word \b` containment, ASCII word semantics like JS regex `\b`.
fn contains_word(haystack: &str, word: &str) -> bool {
    let hay = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(word) {
        let i = start + pos;
        let end = i + word.len();
        let before_ok = i == 0 || !is_word_byte(hay[i - 1]);
        let after_ok = end >= hay.len() || !is_word_byte(hay[end]);
        if before_ok && after_ok {
            return true;
        }
        start = i + 1;
    }
    false
}

fn lower_trim(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The direction a plain command names, if any: an exact cardinal, else a
/// word-boundary scan (`"go northeast gate"` → northeast, `"go upper
/// hallway"` → nothing).
fn direction_from_command(command: &str) -> Option<Dir> {
    let cmd = lower_trim(command);
    if let Some(d) = Dir::from_exact(&cmd) {
        return Some(d);
    }
    SCAN_ORDER
        .into_iter()
        .find(|&d| contains_word(&cmd, d.name()))
}

/// One exit's direction, read from its own kind and text first, then a
/// fast path for what the converter already classified, then the text scan.
/// `ExitKind::Cardinal` names no direction of its own (VERIFIED:
/// `cena_map::exit::ExitKind::Cardinal` carries no payload) so the plain
/// command's text is still read to learn which of the eight it is — but a
/// `Vertical`/`Out` exit is never sent through the compass scan the way
/// Vellum's string-sniffing always runs, because the converter already
/// settled that question once.
fn direction_from_exit(kind: ExitKind, crossing: &Crossing) -> Option<Dir> {
    let Crossing::Command(command) = crossing else {
        // Scripted, routine, pass-through, unported, unknown: none of these
        // name a single command to scan, matching Vellum's treatment of a
        // stringproc it cannot resolve without a dirto override.
        return None;
    };
    match kind {
        ExitKind::Cardinal => direction_from_command(command),
        ExitKind::Vertical => Dir::from_exact(&lower_trim(command)),
        ExitKind::Out | ExitKind::Go | ExitKind::Climb | ExitKind::Other => {
            direction_from_command(command)
        }
        ExitKind::Scripted => None,
    }
}

/// Every edge's resolved direction, computed once up front. Direction
/// analysis is a pure function of the map, and every pipeline stage queries
/// it along exits whose target is inside the selection, so one pass covers
/// all later lookups (a performance cache only; semantics match resolving
/// each edge on demand).
pub struct DirectionMap {
    map: HashMap<(RoomId, RoomId), Dir>,
}

impl DirectionMap {
    /// Resolve every exit's direction from a room already in `map` to
    /// another room already in `map`. An exit whose destination is outside
    /// the given map (a location's rooms, filtered) is skipped, same as
    /// Vellum's `lookup.contains(target_id)` guard.
    #[must_use]
    pub fn build(map: &Map) -> DirectionMap {
        let mut resolved = HashMap::new();
        for room in map.rooms() {
            for exit in &room.exits {
                if map.room(exit.to).is_none() {
                    continue;
                }
                if let Some(dir) = direction_from_exit(exit.kind, &exit.crossing) {
                    resolved.insert((room.id, exit.to), dir);
                    continue;
                }
                if let Some(dir) = infer_from_reverse(map, room.id, exit.to) {
                    resolved.insert((room.id, exit.to), dir);
                }
            }
        }
        DirectionMap { map: resolved }
    }

    #[must_use]
    pub fn get(&self, from: RoomId, to: RoomId) -> Option<Dir> {
        self.map.get(&(from, to)).copied()
    }

    /// Apply curation edge overrides (spec §8) before positioning. Not
    /// wired to anything yet (`plan/26` §0: no editor in v1); kept as the
    /// seam Vellum's own `apply_edge_overrides` occupies, so the future
    /// override system has one obvious place to land.
    pub fn apply_edge_overrides(&mut self, _map: &Map, edges: &[EdgeOverride]) {
        debug_assert!(edges.is_empty(), "no override source exists yet");
    }
}

/// A bare cardinal on the reverse edge reverses. Extracted word-boundary
/// hints are too weak to reverse (matches Vellum: only an exact cardinal on
/// the way back is trusted).
fn infer_from_reverse(map: &Map, room: RoomId, target: RoomId) -> Option<Dir> {
    let back = map.room(target)?;
    let back_exit = back.exits.iter().find(|exit| exit.to == room)?;
    let Crossing::Command(command) = &back_exit.crossing else {
        return None;
    };
    Dir::from_exact(&lower_trim(command)).map(Dir::opposite)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundaries() {
        assert!(contains_word("go northeast gate", "northeast"));
        assert!(!contains_word("go upper hallway", "up"));
        assert!(contains_word("climb up", "up"));
        assert!(!contains_word("go soupbone", "up"));
    }

    #[test]
    fn longest_direction_wins() {
        assert!(!contains_word("northeast", "north"));
    }

    #[test]
    fn a_cardinal_kind_still_needs_its_command_text() {
        assert_eq!(direction_from_command("north"), Some(Dir::North));
        assert_eq!(
            direction_from_command("go northeast gate"),
            Some(Dir::Northeast)
        );
        assert_eq!(direction_from_command("go upper hallway"), None);
    }
}
