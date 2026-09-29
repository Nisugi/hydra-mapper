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

use std::collections::HashSet;

use crate::direction::DirectionMap;
use crate::packer::{AnchorLine, Segment, chebyshev, for_ring};
use crate::positioner::{Bounds, Cell, Group};

/// How far out a group is moved looking for a clear spot, in cells.
pub(crate) const REACH: i32 = 60;

/// Rings searched past the first with a clear spot, for a shorter one.
const LOOK_FURTHER: i32 = 2;

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
