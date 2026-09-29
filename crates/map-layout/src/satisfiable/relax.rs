//! The least wrong a contradicting group can be drawn.
//!
//! A group whose bearings contradict (a loop of orderings, an order inside
//! an alignment) has no arrangement that honours them all, and before this
//! the solver's own was kept, wrong wherever it happened to be: on `gs.map`
//! 42 groups, holding 433 of the exits drawn against their direction.
//!
//! This sets aside one exit of each contradiction -- demoted to a connector,
//! which joins its rooms and constrains nothing, as the editor's own
//! [`EdgeAction::Connector`] does -- until what is left can be honoured, so
//! that [`super::place_near`] can draw every other exit right. Only the
//! exits set aside can then be drawn wrong. The exit chosen in a loop is the
//! least trusted one in it: two exits that disagree about their own bearing
//! first, then a bearing only inferred from the way back, then the first.

use cena_map::{Map, RoomId};

use crate::direction::{Dir, DirectionMap};
use crate::overrides::{EdgeAction, EdgeOverride};

use super::{Problem, problems};

/// How many exits one group may have set aside before it is left as the
/// solver drew it: a group needing more is contradicted everywhere, and
/// what is left of it would not read as the place.
const MOST_SET_ASIDE: usize = 64;

/// `dirs` with the fewest exits of `rooms` set aside, found greedily, for
/// what is left to be honoured; and the exits set aside. `None` when a
/// contradiction has no exit to set aside, or it takes more than
/// [`MOST_SET_ASIDE`].
#[must_use]
pub fn relax(
    rooms: &[RoomId],
    map: &Map,
    dirs: &DirectionMap,
) -> Option<(DirectionMap, Vec<(RoomId, RoomId)>)> {
    let mut relaxed = dirs.clone();
    let mut aside = Vec::new();
    while aside.len() <= MOST_SET_ASIDE {
        let found = problems(rooms, map, &relaxed);
        let Some(problem) = found.first() else {
            return Some((relaxed, aside));
        };
        let (a, b) = least_trusted(problem, &relaxed)?;
        relaxed.apply_edge_overrides(
            map,
            &[EdgeOverride {
                a,
                b,
                action: EdgeAction::Connector,
            }],
        );
        aside.push((a, b));
    }
    None
}

/// The exit of `problem` least worth keeping, as a pair of rooms.
fn least_trusted(problem: &Problem, dirs: &DirectionMap) -> Option<(RoomId, RoomId)> {
    let pairs: Vec<(RoomId, RoomId)> = match problem {
        Problem::Cycle { cycle, .. } => cycle.windows(2).map(|w| (w[0], w[1])).collect(),
        Problem::OrderWithinAlignment { from, to, .. } => vec![(*from, *to)],
        Problem::ForcedOverlap { a, b } => vec![(*a, *b)],
    };
    let joined =
        |a: RoomId, b: RoomId| compass(dirs, a, b).is_some() || compass(dirs, b, a).is_some();
    let candidates: Vec<(RoomId, RoomId)> =
        pairs.into_iter().filter(|&(a, b)| joined(a, b)).collect();
    // Two exits that disagree about their own bearing.
    let disagree = candidates.iter().copied().find(|&(a, b)| {
        matches!(
            (compass(dirs, a, b), compass(dirs, b, a)),
            (Some(there), Some(back)) if back != there.opposite()
        )
    });
    // A bearing one way only: the other is inferred, or missing.
    let one_way = || {
        candidates
            .iter()
            .copied()
            .find(|&(a, b)| compass(dirs, a, b).is_none() || compass(dirs, b, a).is_none())
    };
    disagree
        .or_else(one_way)
        .or_else(|| candidates.first().copied())
}

/// The compass bearing from `a` to `b`, if the exit has one.
fn compass(dirs: &DirectionMap, a: RoomId, b: RoomId) -> Option<Dir> {
    dirs.get(a, b).filter(|d| d.is_compass())
}
