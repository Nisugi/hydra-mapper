//! `cena-mapper --compare <area> [map]`: an area as the engine lays it out
//! against the same area with the store's hand corrections, and what each
//! correction changed, exit by exit. The author, 2026-09-29: *"Can you
//! compare my layout to what the layout engine would produce and deduce the
//! differences and why they occurred?"*
//!
//! Both are measured by the four rules ([`cena_map_layout::quality`]), and
//! every room the corrections moved is read for what its move did to its
//! own exits: a bearing put right or made wrong, a stretched line made one
//! step or the reverse, a doorway shortened or lengthened, a line through a
//! room taken away or added. Both are drawn beside the map as
//! `<map>.<area>.engine.svg` and `<map>.<area>.hand.svg`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use cena_map::{Map, RoomId};
use cena_map_layout::quality::{self, Quality};
use cena_map_layout::scene::STREETS;
use cena_map_layout::{
    DirectionMap, Layout, LayoutParams, MapScene, build_scene, generate_layout_tuned,
};

use crate::areas::{self, Areas};
use crate::overrides::{self, MapOverrides};

/// One way of drawing the area.
struct Drawn {
    layout: Layout,
    scene: MapScene,
    quality: Quality,
    /// Each room's cell on the sheet, and its group's scale (cells per step).
    at: HashMap<RoomId, (i32, i32, i32)>,
}

impl Drawn {
    fn of(
        subset: &Map,
        name: &str,
        edges: &[cena_map_layout::EdgeOverride],
        hand: Option<&overrides::LocationOverrides>,
    ) -> Drawn {
        let mut layout = generate_layout_tuned(subset, edges, LayoutParams::default());
        if let Some(hand) = hand {
            overrides::apply(&mut layout, subset, hand);
        }
        let scene = build_scene(name, &layout, subset);
        let quality = quality::measure(&scene, &layout, subset);
        let at = scene
            .sheet
            .rooms
            .iter()
            .map(|r| (r.id, (r.cell.x, r.cell.y, scene.scale_of(r.group).max(1))))
            .collect();
        Drawn {
            layout,
            scene,
            quality,
            at,
        }
    }

    /// The line from `a` to `b` in steps: Chebyshev length over the scale.
    fn steps(&self, a: RoomId, b: RoomId) -> Option<i32> {
        let (&(ax, ay, s), &(bx, by, _)) = (self.at.get(&a)?, self.at.get(&b)?);
        Some((ax - bx).abs().max((ay - by).abs()) / s)
    }

    /// Whether the exit `a` to `b` is drawn against `dir`'s signs.
    fn against(&self, a: RoomId, b: RoomId, dirs: &DirectionMap) -> Option<bool> {
        let dir = dirs.get(a, b).filter(|d| d.is_compass())?;
        let (&(ax, ay, _), &(bx, by, _)) = (self.at.get(&a)?, self.at.get(&b)?);
        let (ex, ey) = dir.offset();
        Some((bx - ax).signum() != ex.signum() || (by - ay).signum() != ey.signum())
    }
}

/// What moving a room did to one of its exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Effect {
    BearingPutRight,
    BearingMadeWrong,
    StretchedMadeOneStep,
    OneStepStretched,
    DoorwayShortened,
    DoorwayLengthened,
    LineOffARoom,
    LineOverARoom,
}

impl Effect {
    const fn words(self) -> &'static str {
        match self {
            Effect::BearingPutRight => "a direction put right",
            Effect::BearingMadeWrong => "a direction made wrong",
            Effect::StretchedMadeOneStep => "a stretched line made one step",
            Effect::OneStepStretched => "a one-step line stretched",
            Effect::DoorwayShortened => "a doorway shortened",
            Effect::DoorwayLengthened => "a doorway lengthened",
            Effect::LineOffARoom => "a line taken off a room",
            Effect::LineOverARoom => "a line put over a room",
        }
    }
}

/// Compare the area `area` of the map at `path`, drawn by the engine and by
/// hand.
///
/// # Errors
///
/// The map or its store cannot be read, the area has no rooms, or no hand
/// corrections, or a picture cannot be written.
pub fn compare_headless(area: &str, path: Option<&Path>) -> Result<String, String> {
    let path = path.ok_or("no map path: give one, or set CENA_MAP")?;
    let map = crate::app::load_map(Some(path)).map_err(|p| p.to_string())?;
    let store_path = overrides::store_path(path);
    let store =
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?;
    // The area's rooms as the window has them: the store's curated area,
    // with every room moved into it since the map was baked, and only then
    // the map's own `meta:area:`. The two differ, and the pins were made on
    // the window's.
    let built = Areas::build(&map, &store);
    let placeable = built.placeable.clone();
    let own: Vec<RoomId> = areas::AreaKind::ALL
        .iter()
        .flat_map(|&kind| built.list(kind))
        .find(|a| a.name == area && (a.parent.is_some() || a.kind == areas::AreaKind::Maps))
        .map_or_else(
            || {
                map.rooms()
                    .iter()
                    .filter(|r| r.meta.iter().any(|m| m.strip_prefix("area:") == Some(area)))
                    .map(|r| r.id)
                    .collect()
            },
            |a| a.rooms.clone(),
        );
    if own.is_empty() {
        return Err(format!("no room of the map is in the area {area}"));
    }
    let subset = Map::from_rooms(areas::layout_rooms(&own, &map, &placeable))
        .map_err(|e| format!("{e:?}"))?;
    // An area with no hand corrections is drawn by the engine twice: the
    // pictures and the rules still say how it reads.
    let none = overrides::LocationOverrides::default();
    let hand = store
        .location(&format!("map:{area}"))
        .or_else(|| store.location(&format!("curated:{area}")))
        .or_else(|| store.location(area))
        .unwrap_or(&none);
    let edges = hand.edge_overrides(&subset);
    let engine = Drawn::of(&subset, area, &edges, None);
    let yours = Drawn::of(&subset, area, &edges, Some(hand));
    let mut dirs = DirectionMap::build(&subset);
    dirs.apply_edge_overrides(&subset, &engine.layout.edges);

    let mut out = String::new();
    rules(&mut out, &engine.quality, &yours.quality);
    groups(&mut out, &subset, &engine, &yours, hand);
    let effects = moves(&mut out, &subset, &engine, &yours, &dirs);
    let _ = writeln!(out, "\nwhat the moves did to the moved rooms' exits:");
    for (effect, n) in &effects {
        let _ = writeln!(out, "  {:>4}  {}", n, effect.words());
    }
    shape(&mut out, &engine, &yours);
    geography(&mut out, &subset, &engine, &yours);
    for (drawn, which) in [(&engine, "engine"), (&yours, "hand")] {
        let file = path.with_extension(format!("{area}.{which}.svg"));
        picture(drawn, &format!("{area} ({which})"), &file)?;
        let _ = writeln!(out, "drawn: {}", file.display());
    }
    Ok(out)
}

fn rules(out: &mut String, engine: &Quality, yours: &Quality) {
    let _ = writeln!(out, "the four rules, engine -> by hand:");
    for (rule, a, b) in [
        (
            "exits against their direction",
            engine.against_bearing,
            yours.against_bearing,
        ),
        (
            "lines through rooms",
            engine.lines_through_rooms,
            yours.lines_through_rooms,
        ),
        (
            "building rooms under a line not theirs",
            engine.rooms_under_foreign_lines,
            yours.rooms_under_foreign_lines,
        ),
        (
            "directionless lines crossing another",
            engine.directionless_crossing,
            yours.directionless_crossing,
        ),
        (
            "directionless exits not drawn",
            engine.directionless_undrawn,
            yours.directionless_undrawn,
        ),
    ] {
        let _ = writeln!(out, "  {rule}: {a} -> {b}");
    }
}

/// The groups moved whole, and what the move did to the doorways out of
/// each.
fn groups(
    out: &mut String,
    map: &Map,
    engine: &Drawn,
    yours: &Drawn,
    hand: &overrides::LocationOverrides,
) {
    let _ = writeln!(out, "\ngroups moved whole ({}):", hand.group_offsets.len());
    for (anchor, delta) in &hand.group_offsets {
        let Some(group) = engine
            .layout
            .groups
            .iter()
            .find(|g| overrides::RoomKey::anchor(g, map).as_ref() == Some(anchor))
        else {
            let _ = writeln!(out, "  {anchor:?}: not a group of this layout");
            continue;
        };
        let members: HashSet<RoomId> = group.room_ids.iter().copied().collect();
        // Every exit from the group to a room outside it: how long it is
        // drawn, in steps, before and after.
        let (mut before, mut after, mut n) = (0, 0, 0);
        for &id in &group.room_ids {
            for exit in map.room(id).map_or(&[][..], |r| &r.exits) {
                if members.contains(&exit.to) {
                    continue;
                }
                if let (Some(b), Some(a)) = (engine.steps(id, exit.to), yours.steps(id, exit.to)) {
                    before += b;
                    after += a;
                    n += 1;
                }
            }
        }
        let title = map
            .room(group.room_ids[0])
            .and_then(|r| r.title.first().cloned())
            .unwrap_or_default();
        let (mut pairs, mut eng, mut hand) = (0, 0, 0);
        for [_, p, e, h] in agreement(map, engine, yours, Some(&members)).into_values() {
            pairs += p;
            eng += e;
            hand += h;
        }
        let pictured = if pairs == 0 {
            "not on a picture".to_owned()
        } else {
            format!(
                "with the picture {}% -> {}%",
                percent(eng, pairs),
                percent(hand, pairs)
            )
        };
        let _ = writeln!(
            out,
            "  {} rooms around {title}, moved ({}, {}): its {n} exits out total {before} -> {after} steps; {pictured}",
            group.room_ids.len(),
            delta.x,
            delta.y
        );
    }
}

/// Every room drawn somewhere else by hand, and what the move did to its
/// exits, summed.
fn moves(
    out: &mut String,
    map: &Map,
    engine: &Drawn,
    yours: &Drawn,
    dirs: &DirectionMap,
) -> BTreeMap<Effect, usize> {
    let through = |q: &Quality| -> HashSet<(RoomId, RoomId)> {
        q.detail
            .through
            .iter()
            .map(|(a, b, _)| if a.0 <= b.0 { (*a, *b) } else { (*b, *a) })
            .collect()
    };
    let (through_engine, through_yours) = (through(&engine.quality), through(&yours.quality));
    // A room moved relative to the rooms around it, not merely with its
    // group: its offsets to its neighbours changed.
    let mut moved: Vec<RoomId> = Vec::new();
    for room in map.rooms() {
        let changed = room.exits.iter().any(|e| {
            let offset = |d: &Drawn| {
                let (&(ax, ay, _), &(bx, by, _)) = (d.at.get(&room.id)?, d.at.get(&e.to)?);
                Some((bx - ax, by - ay))
            };
            offset(engine) != offset(yours)
        });
        if changed && engine.at.contains_key(&room.id) {
            moved.push(room.id);
        }
    }
    let mut effects: BTreeMap<Effect, usize> = BTreeMap::new();
    let mut seen: HashSet<(RoomId, RoomId)> = HashSet::new();
    for &id in &moved {
        for exit in map.room(id).map_or(&[][..], |r| &r.exits) {
            let key = if id.0 <= exit.to.0 {
                (id, exit.to)
            } else {
                (exit.to, id)
            };
            if !seen.insert(key) {
                continue;
            }
            let mut note = |e: Effect| *effects.entry(e).or_default() += 1;
            match (
                engine.against(id, exit.to, dirs),
                yours.against(id, exit.to, dirs),
            ) {
                (Some(true), Some(false)) => note(Effect::BearingPutRight),
                (Some(false), Some(true)) => note(Effect::BearingMadeWrong),
                _ => {}
            }
            let bearing = dirs.get(id, exit.to).or_else(|| dirs.get(exit.to, id));
            if let (Some(b), Some(a)) = (engine.steps(id, exit.to), yours.steps(id, exit.to)) {
                match bearing {
                    Some(_) if b > 1 && a == 1 => note(Effect::StretchedMadeOneStep),
                    Some(_) if b == 1 && a > 1 => note(Effect::OneStepStretched),
                    None if a < b => note(Effect::DoorwayShortened),
                    None if a > b => note(Effect::DoorwayLengthened),
                    _ => {}
                }
            }
            match (through_engine.contains(&key), through_yours.contains(&key)) {
                (true, false) => note(Effect::LineOffARoom),
                (false, true) => note(Effect::LineOverARoom),
                _ => {}
            }
        }
    }
    let _ = writeln!(
        out,
        "\nrooms drawn in a different place relative to their neighbours: {} of {}",
        moved.len(),
        engine.at.len()
    );
    effects
}

/// How big each is drawn.
fn shape(out: &mut String, engine: &Drawn, yours: &Drawn) {
    let size = |d: &Drawn| {
        let s = &d.scene.sheet;
        (s.max.x - s.min.x + 1, s.max.y - s.min.y + 1)
    };
    let ((ew, eh), (yw, yh)) = (size(engine), size(yours));
    let length = |d: &Drawn| -> i64 {
        d.scene
            .sheet
            .edges
            .iter()
            .map(|e| i64::from((e.a.x - e.b.x).abs().max((e.a.y - e.b.y).abs())))
            .sum()
    };
    let _ = writeln!(
        out,
        "\nthe sheet: {ew} x {eh} cells -> {yw} x {yh}; every line together {} -> {} cells long",
        length(engine),
        length(yours)
    );
}

/// Two rooms at least this many pixels apart on one of Lich's pictures
/// are a pair whose order on that axis a drawing can agree with.
const APART: i32 = 8;

/// How far each drawing agrees with Lich's pictures of the place, per
/// picture: its rooms, then of the pairs at least [`APART`] pixels apart on
/// an axis (touching a room of `only` when given), how many, and how many
/// each drawing, engine then hand, has the same way round on that axis.
fn agreement(
    map: &Map,
    engine: &Drawn,
    yours: &Drawn,
    only: Option<&HashSet<RoomId>>,
) -> BTreeMap<String, [usize; 4]> {
    let mut by_file: BTreeMap<&str, Vec<(RoomId, i32, i32)>> = BTreeMap::new();
    for room in map.rooms() {
        if let Some(image) = &room.image
            && engine.at.contains_key(&room.id)
            && yours.at.contains_key(&room.id)
        {
            let [l, t, r, b] = image.rect;
            by_file.entry(image.file.as_str()).or_default().push((
                room.id,
                i32::midpoint(l, r),
                i32::midpoint(t, b),
            ));
        }
    }
    let mut out = BTreeMap::new();
    for (file, rooms) in &by_file {
        let mut counts = [rooms.len(), 0, 0, 0];
        for (i, &(a, ax, ay)) in rooms.iter().enumerate() {
            for &(b, bx, by) in &rooms[i + 1..] {
                if only.is_some_and(|s| !s.contains(&a) && !s.contains(&b)) {
                    continue;
                }
                for (picture, along_x) in [(bx - ax, true), (by - ay, false)] {
                    if picture.abs() < APART {
                        continue;
                    }
                    let drawn = |d: &Drawn| {
                        let (&(x1, y1, _), &(x2, y2, _)) = (&d.at[&a], &d.at[&b]);
                        if along_x { x2 - x1 } else { y2 - y1 }
                    };
                    counts[1] += 1;
                    counts[2] += usize::from(drawn(engine).signum() == picture.signum());
                    counts[3] += usize::from(drawn(yours).signum() == picture.signum());
                }
            }
        }
        if counts[1] > 0 {
            out.insert((*file).to_owned(), counts);
        }
    }
    out
}

/// The per cent of `of` that `part` is.
fn percent(part: usize, of: usize) -> usize {
    (part * 100).checked_div(of).unwrap_or(0)
}

fn geography(out: &mut String, map: &Map, engine: &Drawn, yours: &Drawn) {
    let _ = writeln!(
        out,
        "\nagreeing with Lich's pictures of the place (pairs of rooms the same way round):"
    );
    let (mut n, mut e, mut h) = (0, 0, 0);
    for (file, [rooms, pairs, eng, hand]) in agreement(map, engine, yours, None) {
        let _ = writeln!(
            out,
            "  {file}: {rooms} rooms, {pairs} pairs: engine {}%, by hand {}%",
            percent(eng, pairs),
            percent(hand, pairs)
        );
        n += pairs;
        e += eng;
        h += hand;
    }
    if n > 0 {
        let _ = writeln!(
            out,
            "  every picture: engine {}%, by hand {}%",
            percent(e, n),
            percent(h, n)
        );
    }
}

fn picture(drawn: &Drawn, title: &str, file: &Path) -> Result<(), String> {
    let streets: HashSet<RoomId> = drawn
        .scene
        .units
        .get(STREETS)
        .map(|u| u.rooms.iter().copied().collect())
        .unwrap_or_default();
    let doors: HashSet<RoomId> = drawn
        .scene
        .units
        .iter()
        .flat_map(|u| u.door_rooms.iter().copied())
        .collect();
    let every: HashSet<RoomId> = drawn.scene.sheet.rooms.iter().map(|r| r.id).collect();
    let svg = crate::svg::sheet(&drawn.scene.sheet, &every, &streets, &doors, title)
        .map_err(|e| format!("{title}: {e:?}"))?;
    std::fs::write(file, svg).map_err(|e| format!("{} {e}", file.display()))
}
