//! Stretchy connectors. The author, 2026-09-29: *"yellow connectors should
//! be stretchy, they should stretch until their whole group is free and
//! clear of any of rooms and overlaps/edge crossing."*
//!
//! A group the connector pass places hangs on lines with no direction, so
//! nothing says how far from its neighbour it sits. It is moved outward
//! from beside the neighbour, ring by ring, to the nearest spot where it is
//! clear ([`clear`]): its rooms on free cells, no room and no line already
//! placed inside the box around its rooms, none of its connector lines
//! crossing a placed line or passing a room, and any of them that has a
//! compass direction still pointing its way. Of the clear spots on the
//! first ring that has one and the next [`LOOK_FURTHER`], the one whose
//! connectors are shortest wins. Where nothing within [`REACH`] is clear,
//! the connector pass's own scoring decides, as it did before.

use std::collections::{HashMap, HashSet};

use cena_map::{Map, RoomId};

use crate::direction::DirectionMap;
use crate::packer::{AnchorLine, Edge, Segment, chebyshev, for_ring};
use crate::positioner::{Bounds, Cell, Group};

/// How far out a group is moved looking for a clear spot, in cells.
pub(crate) const REACH: i32 = 60;

/// Rings searched past the first with a clear spot, for a shorter one.
const LOOK_FURTHER: i32 = 2;

/// Rounds of [`shrink`]: each group picked up and put back, until a round
/// moves nothing.
const SHRINK_ROUNDS: usize = 3;

/// The yellow lines shortened (the author, 2026-09-29: *"the next stage
/// there would be to shrink the yellow lines without introducing overlap
/// or crossing"*). Once everything is placed, each group but the biggest
/// is picked up and put back as near the middle of its links as it is
/// clear ([`find_clear_offset`]), where its links are shorter in total
/// than where it was; round after round, so a group moved can make room
/// for one placed before it.
pub(crate) fn shrink(
    groups: &mut [Group],
    packed: &[usize],
    edges: &HashMap<usize, Vec<Edge>>,
    map: &Map,
    dirs: &DirectionMap,
    occupied: &mut HashSet<Cell>,
    segments: &mut Vec<Segment>,
) {
    let Some(&biggest) = packed.iter().max_by_key(|&&i| groups[i].room_ids.len()) else {
        return;
    };
    for _ in 0..SHRINK_ROUNDS {
        let mut moved = false;
        for &idx in packed {
            let Some(was) = groups[idx].base_offset else {
                continue;
            };
            if idx == biggest {
                continue;
            }
            let anchors: Vec<AnchorLine> = edges
                .get(&idx)
                .map(|l| {
                    l.iter()
                        .filter(|e| groups[e.other_group].base_offset.is_some())
                        .map(|e| AnchorLine {
                            internal: groups[idx].positions[&e.room_id],
                            target: groups[e.other_group].final_cell(e.other_room_id),
                            room_id: e.room_id,
                            other_room_id: e.other_room_id,
                        })
                        .collect()
                })
                .unwrap_or_default();
            if anchors.is_empty() {
                continue;
            }
            let length = |at: Cell| -> i32 {
                anchors
                    .iter()
                    .map(|a| chebyshev(shift(a.internal, at), a.target))
                    .sum()
            };
            // Picked up: its rooms and lines off the sheet.
            let mine: HashSet<RoomId> = groups[idx].room_ids.iter().copied().collect();
            for &r in &groups[idx].room_ids {
                occupied.remove(&groups[idx].final_cell(r));
            }
            let others: Vec<Segment> = segments
                .iter()
                .filter(|s| !mine.contains(&s.ra) && !mine.contains(&s.rb))
                .copied()
                .collect();
            let n = i32::try_from(anchors.len()).unwrap_or(1).max(1);
            let middle = Cell {
                x: anchors
                    .iter()
                    .map(|a| a.target.x - a.internal.x)
                    .sum::<i32>()
                    / n,
                y: anchors
                    .iter()
                    .map(|a| a.target.y - a.internal.y)
                    .sum::<i32>()
                    / n,
            };
            let better = find_clear_offset(&groups[idx], middle, occupied, &anchors, &others, dirs)
                .filter(|&at| length(at) < length(was));
            let at = better.unwrap_or(was);
            groups[idx].base_offset = Some(at);
            for &r in &groups[idx].room_ids {
                occupied.insert(groups[idx].final_cell(r));
            }
            if better.is_some() {
                moved = true;
                *segments = others;
                lines_of(&groups[idx], &anchors, map, segments);
            }
        }
        if !moved {
            break;
        }
    }
}

/// A placed group's own lines and its links, as segments for what comes
/// after to steer clear of.
fn lines_of(group: &Group, anchors: &[AnchorLine], map: &Map, segments: &mut Vec<Segment>) {
    let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
    for &room_id in &group.room_ids {
        let Some(room) = map.room(room_id) else {
            continue;
        };
        for exit in room.exits.iter().filter(|e| crate::regions::is_passage(e)) {
            if group.positions.contains_key(&exit.to)
                && seen.insert((room_id.min(exit.to), room_id.max(exit.to)))
            {
                segments.push(Segment {
                    a: group.final_cell(room_id),
                    b: group.final_cell(exit.to),
                    ra: room_id,
                    rb: exit.to,
                });
            }
        }
    }
    for a in anchors {
        segments.push(Segment {
            a: group.final_cell(a.room_id),
            b: a.target,
            ra: a.room_id,
            rb: a.other_room_id,
        });
    }
}

/// The nearest offset within [`REACH`] of `proposed` where `group` is
/// clear of everything placed, or `None`.
pub(crate) fn find_clear_offset(
    group: &Group,
    proposed: Cell,
    occupied: &HashSet<Cell>,
    anchor_lines: &[AnchorLine],
    placed_segments: &[Segment],
    dirs: &DirectionMap,
) -> Option<Cell> {
    let placed = Placed {
        occupied,
        segments: placed_segments,
        dirs,
    };
    let cells: Vec<Cell> = group.positions.values().copied().collect();
    let bounds = group.bounds();
    let mut best: Option<(i64, Cell)> = None;
    let mut first_clear: Option<i32> = None;
    for r in 0..=REACH {
        if first_clear.is_some_and(|f| r > f + LOOK_FURTHER) {
            break;
        }
        let mut ring = Vec::new();
        if r == 0 {
            ring.push(proposed);
        } else {
            for_ring(proposed, r, |c| ring.push(c));
        }
        for at in ring {
            if !clear(&cells, bounds, at, anchor_lines, &placed) {
                continue;
            }
            first_clear.get_or_insert(r);
            let length: i64 = anchor_lines
                .iter()
                .map(|a| i64::from(chebyshev(shift(a.internal, at), a.target)))
                .sum();
            if best.is_none_or(|(shortest, _)| length < shortest) {
                best = Some((length, at));
            }
        }
    }
    best.map(|(_, at)| at)
}

/// What is already on the sheet.
struct Placed<'a> {
    occupied: &'a HashSet<Cell>,
    segments: &'a [Segment],
    dirs: &'a DirectionMap,
}

/// Whether the group, moved by `at`, is clear of everything placed.
fn clear(
    cells: &[Cell],
    bounds: Bounds,
    at: Cell,
    anchor_lines: &[AnchorLine],
    placed: &Placed<'_>,
) -> bool {
    if cells
        .iter()
        .any(|&c| placed.occupied.contains(&shift(c, at)))
    {
        return false;
    }
    // The box around its rooms: no room and no line of anyone else's.
    let (min_x, max_x) = (bounds.min_x + at.x, bounds.max_x + at.x);
    let (min_y, max_y) = (bounds.min_y + at.y, bounds.max_y + at.y);
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            if placed.occupied.contains(&Cell { x, y }) {
                return false;
            }
        }
    }
    let footprint = [
        (min_x, min_y, max_x, min_y),
        (max_x, min_y, max_x, max_y),
        (max_x, max_y, min_x, max_y),
        (min_x, max_y, min_x, min_y),
    ];
    for seg in placed.segments {
        let inside = |c: Cell| c.x >= min_x && c.x <= max_x && c.y >= min_y && c.y <= max_y;
        if inside(seg.a)
            || inside(seg.b)
            || footprint.iter().any(|&(x1, y1, x2, y2)| {
                crosses(seg.a, seg.b, Cell { x: x1, y: y1 }, Cell { x: x2, y: y2 })
            })
        {
            return false;
        }
    }
    // Its connector lines: across no placed line, past no room, and one
    // with a compass direction pointing its way.
    for anchor in anchor_lines {
        let end = shift(anchor.internal, at);
        if let Some(dir) = placed.dirs.get(anchor.room_id, anchor.other_room_id)
            && dir.is_compass()
        {
            let (ex, ey) = dir.offset();
            if (anchor.target.x - end.x).signum() != ex.signum()
                || (anchor.target.y - end.y).signum() != ey.signum()
            {
                return false;
            }
        }
        let crossing = placed.segments.iter().any(|seg| {
            ![seg.ra, seg.rb].contains(&anchor.room_id)
                && ![seg.ra, seg.rb].contains(&anchor.other_room_id)
                && crosses(end, anchor.target, seg.a, seg.b)
        });
        if crossing || passes_a_room(end, anchor.target, placed.occupied, cells, at) {
            return false;
        }
    }
    true
}

/// Whether the straight line `a`-`b` passes within routing's clearance of
/// a room other than its two ends: one placed, or one of the group's own.
fn passes_a_room(a: Cell, b: Cell, occupied: &HashSet<Cell>, own: &[Cell], at: Cell) -> bool {
    let near =
        |c: Cell| c != a && c != b && distance_to_segment(c, a, b) < crate::routing::CLEARANCE;
    let (lo_x, hi_x) = (a.x.min(b.x), a.x.max(b.x));
    let (lo_y, hi_y) = (a.y.min(b.y), a.y.max(b.y));
    for x in lo_x..=hi_x {
        for y in lo_y..=hi_y {
            let c = Cell { x, y };
            if occupied.contains(&c) && near(c) {
                return true;
            }
        }
    }
    own.iter().any(|&c| near(shift(c, at)))
}

/// How far the centre of `cell` is from the segment `from`-`to`.
fn distance_to_segment(cell: Cell, from: Cell, to: Cell) -> f32 {
    let (at, from, to) = (
        crate::routing::point(cell),
        crate::routing::point(from),
        crate::routing::point(to),
    );
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let along = dx.mul_add(dx, dy * dy);
    let share = if along == 0.0 {
        0.0
    } else {
        ((at.x - from.x).mul_add(dx, (at.y - from.y) * dy) / along).clamp(0.0, 1.0)
    };
    (share.mul_add(dx, from.x) - at.x).hypot(share.mul_add(dy, from.y) - at.y)
}

/// Whether two segments cross at a point inside both (touching at an end,
/// or lying along each other, is not a crossing).
fn crosses(p1: Cell, p2: Cell, q1: Cell, q2: Cell) -> bool {
    use crate::routing::point;
    crate::quality::segments_cross(point(p1), point(p2), point(q1), point(q2))
}

const fn shift(c: Cell, by: Cell) -> Cell {
    Cell {
        x: c.x + by.x,
        y: c.y + by.y,
    }
}
