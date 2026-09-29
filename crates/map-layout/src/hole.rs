//! Room made inside a ring. The author, 2026-09-29, of Kraken's Fall's
//! Atoll: *"why doesn't the path above stretch out a little so the ruins
//! can fit inside it?"* The ruins hang by a barrier off the Inner Edge,
//! a room at the end of a spoke inside the atoll's ring path; the ring is
//! too tight for them, so they were packed below everything and cut, their
//! barrier drawn as marks.
//!
//! Before packing, a group joined only by lines with no direction whose
//! link ends on a room another group encloses ([`ring_box`]) is tried in
//! that group grown about the room ([`grow`]): as many empty columns on
//! each side of the room's column, the same each side, so the ring grows
//! evenly (the author: *"it should squish a group symmetrically not from
//! one side to the other"*), and rows above or below its row as the place
//! needs (the Atoll's ruins hang below its Inner Edge). The least
//! growth in which it fits beside its link, as the cut would have it put
//! down (`stretch::beside`), is kept; the packer and the cut's pull-in then
//! find the room that was made. (Doubling the ring and then squishing back
//! what was not needed came out lopsided: a greedy squish takes one side
//! first.)
//!
//! The columns are opened only across the ring's own rows where that
//! leaves every exit pointing the way it did, so what hangs above or below
//! the ring keeps its width; otherwise down the whole group. Doubling the
//! ring's box instead made it twice what the ruins needed, and opening
//! columns on the ring's rows regardless leaned 102 exits against their
//! direction over the map, in towns most.

use std::collections::HashSet;

use cena_map::{Map, RoomId};

use crate::direction::DirectionMap;
use crate::packer::{AnchorLine, Edge, collect_connector_edges};
use crate::positioner::{Cell, Group};
use crate::scene::{LONG_EDGE_CELLS, Point};

/// The most a ring grows, columns and rows together, each side.
const MOST_GROWTH: i32 = 8;

/// Grow each group that encloses the link of a group that cannot fit
/// inside it as it is, by the least that makes it fit.
pub(crate) fn make_room(groups: &mut [Group], outdoor: &[usize], map: &Map, dirs: &DirectionMap) {
    let edges = collect_connector_edges(groups, outdoor, map);
    let mut spread: HashSet<usize> = HashSet::new();
    for &inner in outdoor {
        let Some(links) = edges.get(&inner) else {
            continue;
        };
        if links.iter().any(|e| {
            dirs.get(e.room_id, e.other_room_id).is_some()
                || dirs.get(e.other_room_id, e.room_id).is_some()
        }) {
            continue;
        }
        for link in links {
            let host = link.other_group;
            if spread.contains(&host) || groups[host].room_ids.len() <= groups[inner].room_ids.len()
            {
                continue;
            }
            let Some(ring) = ring_box(&groups[host], link.other_room_id, map) else {
                continue;
            };
            if fits_inside(&groups[inner], &groups[host], link, map, dirs) {
                break;
            }
            let centre = groups[host].positions[&link.other_room_id];
            let grown = (1..=MOST_GROWTH)
                .flat_map(|total| {
                    (0..=total).flat_map(move |across| {
                        (0..=total - across).map(move |up| (across, up, total - across - up))
                    })
                })
                .find_map(|(across, up, down)| {
                    [true, false].into_iter().find_map(|band| {
                        let rows = (up, down);
                        let wide = grow(&groups[host], across, rows, centre, band.then_some(ring));
                        (keeps_directions(&groups[host], &wide, map)
                            && longest_step(&wide, map) <= LONG_EDGE_CELLS
                            && fits_inside(&groups[inner], &wide, link, map, dirs))
                        .then_some(wide)
                    })
                });
            if let Some(wide) = grown {
                groups[host] = wide;
                spread.insert(host);
                break;
            }
        }
    }
}

/// A ring's box: the least and greatest cell across and down.
#[derive(Debug, Clone, Copy)]
struct RingBox {
    min: Cell,
    max: Cell,
}

/// `group` grown about `centre`: `across` empty columns opened on each
/// side of its column, only on the ring's rows when `band` is given, and
/// `rows` empty rows opened above its row and below it. Every cell moves
/// away from the centre or stays, never past another, so no exit's step
/// turns round.
fn grow(
    group: &Group,
    across: i32,
    (up, down): (i32, i32),
    centre: Cell,
    band: Option<RingBox>,
) -> Group {
    let mut wide = group.clone();
    for cell in wide.positions.values_mut() {
        let opened = band.is_none_or(|b| (b.min.y..=b.max.y).contains(&cell.y));
        *cell = Cell {
            x: if opened {
                cell.x + across * (cell.x - centre.x).signum()
            } else {
                cell.x
            },
            y: match cell.y.cmp(&centre.y) {
                std::cmp::Ordering::Less => cell.y - up,
                std::cmp::Ordering::Equal => cell.y,
                std::cmp::Ordering::Greater => cell.y + down,
            },
        };
    }
    wide
}

/// Whether every exit between two rooms of `was` steps the same way round
/// in `now` as it did: neither across nor down turned, nor made or lost.
fn keeps_directions(was: &Group, now: &Group, map: &Map) -> bool {
    was.room_ids.iter().all(|&id| {
        map.room(id).is_none_or(|room| {
            room.exits.iter().all(|e| {
                let (Some(a0), Some(b0)) = (was.positions.get(&id), was.positions.get(&e.to))
                else {
                    return true;
                };
                let (a1, b1) = (now.positions[&id], now.positions[&e.to]);
                (b0.x - a0.x).signum() == (b1.x - a1.x).signum()
                    && (b0.y - a0.y).signum() == (b1.y - a1.y).signum()
            })
        })
    })
}

/// Whether `inner` can be put down beside `link`'s end in `host`, laid at
/// the origin, with nothing else on the sheet.
fn fits_inside(inner: &Group, host: &Group, link: &Edge, map: &Map, dirs: &DirectionMap) -> bool {
    let mut host = host.clone();
    host.base_offset = Some(Cell { x: 0, y: 0 });
    let occupied: HashSet<Cell> = host.positions.values().copied().collect();
    let mut segments = Vec::new();
    crate::stretch::lines_of(&host, &[], map, &mut segments);
    let anchor = AnchorLine {
        internal: inner.positions[&link.room_id],
        target: host.positions[&link.other_room_id],
        room_id: link.room_id,
        other_room_id: link.other_room_id,
    };
    crate::stretch::beside(
        inner,
        &[anchor],
        &occupied,
        &segments,
        dirs,
        crate::cut::STEPS,
    )
    .is_some()
}

/// The longest step between two of `group`'s rooms, in cells.
fn longest_step(group: &Group, map: &Map) -> i32 {
    let mut longest = 0;
    for &id in &group.room_ids {
        let Some(room) = map.room(id) else {
            continue;
        };
        for exit in &room.exits {
            if let Some(&to) = group.positions.get(&exit.to) {
                let at = group.positions[&id];
                longest = longest.max((to.x - at.x).abs().max((to.y - at.y).abs()));
            }
        }
    }
    longest
}

/// The ring around room `id` of `group`, if the group's own lines enclose
/// it: a line of it across each of the four ways out from the room,
/// straight east, west, north and south. The ring's box is the box around
/// the nearest such line each way.
fn ring_box(group: &Group, id: RoomId, map: &Map) -> Option<RingBox> {
    let at = *group.positions.get(&id)?;
    let mut lines: Vec<(Point, Point)> = Vec::new();
    for &from in &group.room_ids {
        let Some(room) = map.room(from) else {
            continue;
        };
        for exit in room.exits.iter().filter(|e| crate::regions::is_passage(e)) {
            if exit.to == id || from == id {
                continue;
            }
            if let Some(&to) = group.positions.get(&exit.to) {
                lines.push((point(group.positions[&from]), point(to)));
            }
        }
    }
    // Off the grid a little, so a ray meets a line and never slips through
    // at one of its rooms.
    let from = Point {
        x: at_f(at.x) + 0.13,
        y: at_f(at.y) + 0.17,
    };
    let mut ring = RingBox { min: at, max: at };
    for (dx, dy) in [(1.0_f32, 0.0_f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let far = Point {
            x: dx.mul_add(10_000.0, from.x),
            y: dy.mul_add(10_000.0, from.y),
        };
        // The nearest line across this way out: least distance along it.
        let hit = lines
            .iter()
            .filter(|&&(a, b)| crate::quality::segments_cross(from, far, a, b))
            .min_by(|p, q| {
                let reach = |&(a, b): &(Point, Point)| {
                    let mid = Point {
                        x: f32::midpoint(a.x, b.x),
                        y: f32::midpoint(a.y, b.y),
                    };
                    (mid.x - from.x).mul_add(dx, (mid.y - from.y) * dy)
                };
                reach(p).total_cmp(&reach(q))
            })?;
        for p in [hit.0, hit.1] {
            #[allow(clippy::cast_possible_truncation)]
            let c = Cell {
                x: p.x as i32,
                y: p.y as i32,
            };
            ring.min = Cell {
                x: ring.min.x.min(c.x),
                y: ring.min.y.min(c.y),
            };
            ring.max = Cell {
                x: ring.max.x.max(c.x),
                y: ring.max.y.max(c.y),
            };
        }
    }
    Some(ring)
}

#[allow(clippy::cast_precision_loss)]
const fn at_f(v: i32) -> f32 {
    v as f32
}

const fn point(c: Cell) -> Point {
    Point {
        x: at_f(c.x),
        y: at_f(c.y),
    }
}
