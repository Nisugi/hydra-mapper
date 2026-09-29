//! `cena-mapper --quality [map]`: every area of the map laid out as the
//! window draws it, measured by the author's four rules
//! ([`cena_map_layout::quality`]), and timed. Written beside the map as
//! `<map>.quality.tsv`, one row per area and a total, and summed on
//! standard output: the baseline Hydra's `plan/53` Stage 1 tunes against.
//!
//! An area is the one baked into the map (`meta:area:<name>`, by `retag`),
//! the unit Hydra lays out by. Each is laid out with the rooms the window
//! pulls in with it (`areas::layout_rooms`), the store's corrections for it
//! and its hand pins, then built into a scene and measured.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use cena_map::{Map, RoomId};
use cena_map_layout::{LayoutParams, Quality, build_scene, generate_layout_tuned, quality};

use crate::areas::{self, Areas};
use crate::overrides::{self, MapOverrides};

/// One area's measure.
struct Row {
    name: String,
    time: Duration,
    quality: Quality,
}

/// Measure every area of the map at `path`, write the table beside it, and
/// say the totals.
///
/// # Errors
///
/// The map or its store cannot be read.
pub fn quality_headless(path: Option<&Path>) -> Result<String, String> {
    let path = path.ok_or("no map path: give one, or set CENA_MAP")?;
    let map = crate::app::load_map(Some(path)).map_err(|p| p.to_string())?;
    let store_path = overrides::store_path(path);
    let store = if store_path.exists() {
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?
    } else {
        MapOverrides::default()
    };
    let placeable = Areas::build(&map, &store).placeable;

    let mut by_area: BTreeMap<String, Vec<RoomId>> = BTreeMap::new();
    for room in map.rooms() {
        if let Some(area) = room.meta.iter().find_map(|m| m.strip_prefix("area:")) {
            by_area.entry(area.to_owned()).or_default().push(room.id);
        }
    }

    let started = Instant::now();
    let mut rows = Vec::new();
    for (name, rooms) in &by_area {
        let rooms = areas::layout_rooms(rooms, &map, &placeable);
        let Ok(subset) = Map::from_rooms(rooms) else {
            continue;
        };
        let location = store
            .location(&format!("curated:{name}"))
            .or_else(|| store.location(name));
        let edges = location
            .map(|l| l.edge_overrides(&subset))
            .unwrap_or_default();
        let clock = Instant::now();
        let mut layout = generate_layout_tuned(&subset, &edges, LayoutParams::default());
        if let Some(location) = location {
            overrides::apply(&mut layout, &subset, location);
        }
        let scene = build_scene(name, &layout, &subset);
        let time = clock.elapsed();
        rows.push(Row {
            name: name.clone(),
            time,
            quality: quality::measure(&scene, &layout, &subset),
        });
    }
    let wall = started.elapsed();

    let mut total = Quality::default();
    let mut laying = Duration::ZERO;
    for row in &rows {
        total.add(&row.quality);
        laying += row.time;
    }
    let table = path.with_extension("quality.tsv");
    let mut text = String::from(
        "area\trooms\tms\tagainst_bearing\tlines_through_rooms\trooms_under_foreign_lines\t\
         directionless\tlines\tstubs\tundrawn\tcrossing\tlen_median\tlen_p90\n",
    );
    for row in &rows {
        line(&mut text, &row.name, row.time, &row.quality);
    }
    line(&mut text, "TOTAL", laying, &total);
    std::fs::write(&table, text).map_err(|e| format!("{} {e}", table.display()))?;

    let slowest = rows.iter().max_by_key(|row| row.time);
    Ok(format!(
        "{} areas, {} rooms, laid out in {:.1} s ({:.1} s with measuring); slowest {}\n\
         rule 1, exits against their direction: {}\n\
         rule 2, lines through rooms: {}\n\
         rule 3, building rooms under a line not theirs: {}\n\
         rule 4, pairs joined only without a direction: {} -- {} lines ({} crossing another), \
         {} stubs, {} not drawn; line length median {}, p90 {}\n\
         written to {}",
        rows.len(),
        total.rooms,
        laying.as_secs_f32(),
        wall.as_secs_f32(),
        slowest.map_or_else(String::new, |row| format!(
            "{} ({} ms, {} rooms)",
            row.name,
            row.time.as_millis(),
            row.quality.rooms
        )),
        total.against_bearing,
        total.lines_through_rooms,
        total.rooms_under_foreign_lines,
        total.directionless,
        total.directionless_lines,
        total.directionless_crossing,
        total.directionless_stubs,
        total.directionless_undrawn,
        cells(total.directionless_length(0.5)),
        cells(total.directionless_length(0.9)),
        table.display(),
    ))
}

fn line(text: &mut String, name: &str, time: Duration, q: &Quality) {
    let _ = writeln!(
        text,
        "{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        q.rooms,
        time.as_millis(),
        q.against_bearing,
        q.lines_through_rooms,
        q.rooms_under_foreign_lines,
        q.directionless,
        q.directionless_lines,
        q.directionless_stubs,
        q.directionless_undrawn,
        q.directionless_crossing,
        cells(q.directionless_length(0.5)),
        cells(q.directionless_length(0.9)),
    );
}

fn cells(length: Option<f32>) -> String {
    length.map_or_else(|| "-".to_owned(), |l| format!("{l:.1}"))
}
