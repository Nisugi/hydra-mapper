//! Direction analysis — ported from `reference/VellumFE/src/core/
//! layout_engine/direction.rs` (`connection-analyzer.js` upstream of that),
//! adapted to `cena_map::Exit`'s already-typed crossing (`plan/26` §3).
//!
//! Resolves the direction of an exit from its `ExitKind`/`Crossing` text, or
//! the reverse edge. Curated corrections
//! ([`DirectionMap::apply_edge_overrides`]) are layered on top afterwards,
//! the same seam Vellum's `dirto` check occupies, so a hand-fixed bearing
//! is what the solver positions by.

use std::collections::HashMap;

use cena_map::step::{Action, Step};
use cena_map::{Crossing, ExitKind, Map, RoomId};

use serde::{Deserialize, Serialize};

use crate::overrides::{EdgeAction, EdgeOverride};

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

    /// The abbreviations the game accepts as whole commands: `n`, `ne`,
    /// `sw`, `u`, `d`.
    ///
    /// Deliberately separate from [`Dir::from_exact`], which also backs
    /// the word-boundary scan over free text -- a bare `n` matched there
    /// would fire on any command containing the letter as a word, and
    /// `go n gate` is not a northward exit. These are only ever matched
    /// against a command in its entirety.
    #[must_use]
    pub fn from_abbreviation(s: &str) -> Option<Dir> {
        Some(match s {
            "n" => Dir::North,
            "s" => Dir::South,
            "e" => Dir::East,
            "w" => Dir::West,
            "ne" => Dir::Northeast,
            "nw" => Dir::Northwest,
            "se" => Dir::Southeast,
            "sw" => Dir::Southwest,
            "u" => Dir::Up,
            "d" => Dir::Down,
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
    if let Some(d) = Dir::from_exact(&cmd).or_else(|| Dir::from_abbreviation(&cmd)) {
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
        // A ported script carries its movement as text, so a scripted
        // exit that is really just `southwest` resolves like one. Routine,
        // pass-through, unported and unknown name no single movement.
        if let Crossing::Steps(steps) = crossing {
            return direction_from_steps(steps);
        }
        return None;
    };
    match kind {
        ExitKind::Cardinal => direction_from_command(command),
        ExitKind::Vertical => {
            let cmd = lower_trim(command);
            Dir::from_exact(&cmd).or_else(|| Dir::from_abbreviation(&cmd))
        }
        ExitKind::Out | ExitKind::Go | ExitKind::Climb | ExitKind::Other => {
            direction_from_command(command)
        }
        ExitKind::Scripted => None,
    }
}

/// The bearing one movement command names, reading its **last** word.
///
/// `swim west` and `west` are the same bearing: the verb says how the
/// walker crosses, the last word says which way. A command whose last
/// word is not a bearing (`go oak door`) names none.
fn movement_bearing(command: &str) -> Option<Dir> {
    let cmd = lower_trim(command);
    let last = cmd.split_whitespace().next_back()?;
    Dir::from_exact(last).or_else(|| Dir::from_abbreviation(last))
}

/// The bearing every movement in a ported script agrees on, if they all
/// name the same one.
///
/// The old format flattened a string proc to an opaque blob, so every
/// scripted exit was directionless by necessity. The ported steps carry
/// their movement as text, and 946 edges of `gs.map` turn out to name an
/// ordinary bearing.
///
/// **Several movements are usually one movement, branched.** The common
/// shape is a pair guarded on opposite conditions:
///
/// ```text
/// Move("west") [when ...]  |  Move("swim west") [when ...]
/// ```
///
/// Walk west if you can, swim west if the water is up. Only one fires,
/// and either way the room lies one step west -- so a script is read as
/// directional when **every** movement it contains names the same
/// bearing, however many there are and whatever guards they carry.
///
/// **Guards are ignored deliberately.** A guard asks whether the exit can
/// be *walked*, not which way it *points*; the room lies west whether or
/// not the tide is in. Walkability is the walker's question, and
/// answering it here would throw away geometry that is not in doubt.
///
/// What still resolves to nothing:
///
/// - **Movements that disagree.** `[southwest, northwest, northwest]` is
///   a walk through several rooms, not a bearing. 10 edges of `gs.map`.
/// - **Any movement with no bearing at all** -- `go door`, or a
///   `KeepMoving`/`MoveUntilThere` that names a heading but no distance
///   (rowing a boat, or fog that turns the walker round). 1,766 edges.
fn direction_from_steps(steps: &[Step]) -> Option<Dir> {
    let mut agreed: Option<Dir> = None;
    for step in steps {
        let Some(command) = moves_by(&step.action) else {
            continue;
        };
        // One movement that names no bearing makes the whole script
        // unreadable: wherever it goes is not a direction away.
        let dir = movement_bearing(command?)?;
        match agreed {
            None => agreed = Some(dir),
            Some(seen) if seen == dir => {}
            Some(_) => return None,
        }
    }
    agreed
}

/// The command a room-changing action sends, if it sends exactly one.
///
/// `Some(Some(cmd))` is a movement with a command to read; `Some(None)` is
/// one that changes rooms without a single command to name -- several
/// commands, a repeat, or a wait -- which no bearing can describe.
/// `None` is an action that does not leave the room at all.
///
/// The room-changing set mirrors `cena_map::step::moves_whatever_is_known`.
fn moves_by(action: &Action) -> Option<Option<&str>> {
    match action {
        Action::Move(c) | Action::TryMove(c) => Some(Some(c)),
        Action::KeepMoving(_)
        | Action::MoveUntilThere(_)
        | Action::Moves(_)
        | Action::KeepMovingAny(_)
        | Action::CastAt(..)
        | Action::MovesFromSetting(_)
        | Action::MoveWhile(..)
        | Action::MoveAnyWhile(..)
        | Action::WanderWhile(_)
        | Action::RoundWhile(..)
        | Action::MoveByAnyExitBut(_)
        | Action::AwaitArrival
        | Action::AwaitAny(_)
        | Action::Await(_) => Some(None),
        _ => None,
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

    /// Apply curated edge corrections, before positioning.
    ///
    /// This is the seam Vellum's own `apply_edge_overrides` occupies, and
    /// the reason corrections land here rather than on the finished
    /// layout: the solver reads this map, so a correction applied now is
    /// one the rooms are *placed by*, not one they are shoved into
    /// afterwards.
    ///
    /// [`EdgeAction::Connector`] forgets the edge's direction in both
    /// senses, leaving it a passage that constrains nothing.
    /// [`EdgeAction::Direction`] sets it, and sets the reverse to the
    /// opposite -- but only for the senses the map actually has an exit
    /// for, so a one-way exit does not gain a return the game does not
    /// offer.
    pub fn apply_edge_overrides(&mut self, map: &Map, edges: &[EdgeOverride]) {
        for edge in edges {
            let has = |from: RoomId, to: RoomId| {
                map.room(from)
                    .is_some_and(|r| r.exits.iter().any(|e| e.to == to))
            };
            match edge.action {
                EdgeAction::Connector => {
                    self.map.remove(&(edge.a, edge.b));
                    self.map.remove(&(edge.b, edge.a));
                }
                EdgeAction::Direction(dir) => {
                    if has(edge.a, edge.b) {
                        self.map.insert((edge.a, edge.b), dir);
                    }
                    if has(edge.b, edge.a) {
                        self.map.insert((edge.b, edge.a), dir.opposite());
                    }
                }
            }
        }
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
    let cmd = lower_trim(command);
    Dir::from_exact(&cmd)
        .or_else(|| Dir::from_abbreviation(&cmd))
        .map(Dir::opposite)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(action: Action) -> Step {
        Step { action, when: None }
    }

    /// A ported script whose movement is a plain bearing is an ordinary
    /// directional exit. The old format flattened these to an opaque
    /// blob, so all of them were placed with no direction; 244 edges of
    /// `gs.map` are one.
    #[test]
    fn a_script_that_only_moves_names_its_direction() {
        assert_eq!(
            direction_from_steps(&[step(Action::Move("southwest".to_owned()))]),
            Some(Dir::Southwest)
        );
        // The game's short forms too, same as a plain command.
        assert_eq!(
            direction_from_steps(&[step(Action::Move("ne".to_owned()))]),
            Some(Dir::Northeast)
        );
    }

    /// A guard asks whether the exit can be *walked*, not which way it
    /// points: the room lies north whether or not the gate is shut, and
    /// dropping the bearing would throw away geometry that is not in
    /// doubt.
    #[test]
    fn a_guarded_move_still_names_its_direction() {
        let guarded = Step {
            action: Action::Move("north".to_owned()),
            when: Some(cena_map::cond::Cond::StillHere),
        };
        assert_eq!(direction_from_steps(&[guarded]), Some(Dir::North));
    }

    /// A second movement means the destination is not one bearing away,
    /// whatever the first step said -- so the whole script names nothing.
    #[test]
    fn a_script_that_moves_twice_names_nothing() {
        assert_eq!(
            direction_from_steps(&[
                step(Action::Move("north".to_owned())),
                step(Action::Move("east".to_owned())),
            ]),
            None,
            "a two-step walk was read as a single bearing"
        );
    }

    /// The commonest scripted shape by far: one movement, branched on
    /// opposite conditions. Walk west if you can, swim west if the water
    /// is up -- only one fires, and the room lies west either way. 702
    /// edges of `gs.map` are this, and reading only the first step left
    /// every one of them unplaced.
    #[test]
    fn branches_that_go_the_same_way_name_that_way() {
        let branch = |cmd: &str| Step {
            action: Action::Move(cmd.to_owned()),
            when: Some(cena_map::cond::Cond::StillHere),
        };
        assert_eq!(
            direction_from_steps(&[branch("west"), branch("swim west")]),
            Some(Dir::West),
            "a walk-or-swim pair was not read as one bearing"
        );
        assert_eq!(
            direction_from_steps(&[branch("northeast"), branch("swim northeast")]),
            Some(Dir::Northeast)
        );
    }

    /// The bearing is the command's last word, so the verb a walker needs
    /// does not change which way the room sits.
    #[test]
    fn the_verb_does_not_change_the_bearing() {
        assert_eq!(movement_bearing("swim west"), Some(Dir::West));
        assert_eq!(movement_bearing("climb up"), Some(Dir::Up));
        assert_eq!(movement_bearing("west"), Some(Dir::West));
        assert_eq!(movement_bearing("sw"), Some(Dir::Southwest));
        // A door is not a bearing, however it is opened.
        assert_eq!(movement_bearing("go oak door"), None);
        assert_eq!(movement_bearing(""), None);
    }

    /// Movements that disagree are a walk through several rooms, not a
    /// bearing -- 10 edges of `gs.map`, and placing by the first would put
    /// the room somewhere it is not.
    #[test]
    fn movements_that_disagree_name_nothing() {
        assert_eq!(
            direction_from_steps(&[
                step(Action::Move("southwest".to_owned())),
                step(Action::Move("northwest".to_owned())),
                step(Action::Move("northwest".to_owned())),
            ]),
            None,
            "a multi-room walk was read as a single bearing"
        );
    }

    /// One movement with no bearing makes the whole script unreadable,
    /// even when the others agree: wherever `go door` leads is not a
    /// direction away.
    #[test]
    fn one_bearingless_movement_spoils_the_script() {
        assert_eq!(
            direction_from_steps(&[
                step(Action::Move("west".to_owned())),
                step(Action::Move("go oak door".to_owned())),
            ]),
            None
        );
        assert_eq!(
            direction_from_steps(&[
                step(Action::Move("west".to_owned())),
                step(Action::KeepMoving("west".to_owned())),
            ]),
            None,
            "a repeat names a heading but no distance"
        );
    }

    /// Steps that do not move the walker are not read at all: a pause or
    /// a stance says nothing about geometry either way.
    #[test]
    fn steps_that_do_not_move_are_ignored() {
        assert_eq!(
            direction_from_steps(&[
                step(Action::EmptyHands),
                step(Action::Move("north".to_owned())),
            ]),
            Some(Dir::North)
        );
    }

    /// Steps that are not a plain move name a heading but no distance --
    /// rowing a boat, or fog that turns the walker round -- so they place
    /// nothing.
    #[test]
    fn only_a_plain_move_counts() {
        assert_eq!(
            direction_from_steps(&[step(Action::KeepMoving("south".to_owned()))]),
            None
        );
        assert_eq!(
            direction_from_steps(&[step(Action::MoveUntilThere("west".to_owned()))]),
            None
        );
        assert_eq!(direction_from_steps(&[]), None);
    }

    /// A script whose move is a door rather than a bearing stays
    /// directionless, exactly as `go door` does.
    #[test]
    fn a_scripted_door_names_no_direction() {
        assert_eq!(
            direction_from_steps(&[step(Action::Move("go oak door".to_owned()))]),
            None
        );
    }

    /// The game's own short forms are real movement commands and the
    /// converter classifies them `Cardinal`. Measured on `gs.map`: ~360
    /// edges use one, including rooms whose own title names the bearing
    /// ("[The Annex, Northeast]" exits `sw` back to the entry), and every
    /// one of them was being placed with no direction at all.
    #[test]
    fn abbreviations_are_directions() {
        for (short, long) in [
            ("n", Dir::North),
            ("s", Dir::South),
            ("e", Dir::East),
            ("w", Dir::West),
            ("ne", Dir::Northeast),
            ("nw", Dir::Northwest),
            ("se", Dir::Southeast),
            ("sw", Dir::Southwest),
            ("u", Dir::Up),
            ("d", Dir::Down),
        ] {
            assert_eq!(
                direction_from_command(short),
                Some(long),
                "{short:?} did not resolve"
            );
        }
    }

    /// An abbreviation is a whole command, never a word inside one: `go n
    /// gate` is a gate, not a northward exit. This is why they are kept
    /// out of the word-boundary scan.
    #[test]
    fn an_abbreviation_inside_a_command_is_not_a_direction() {
        assert_eq!(direction_from_command("go n gate"), None);
        assert_eq!(direction_from_command("go e door"), None);
        // ...but a spelled-out one still is, as it always was.
        assert_eq!(
            direction_from_command("go northeast gate"),
            Some(Dir::Northeast)
        );
    }

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
