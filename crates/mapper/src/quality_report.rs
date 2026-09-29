//! `cena-mapper --quality [map]`: every area of the map laid out as the
//! window draws it, measured by the author's four rules
//! ([`cena_map_layout::quality`]), and timed. Written beside the map as
//! `<map>.quality.tsv`, one row per area and a total, and summed on
//! standard output: the baseline Hydra's `plan/53` Stage 1 tunes against.
//!
//! `cena-mapper --quality-check [map]` is the gate: the same measure,
//! compared with that table, area by area, and refused when any area is
//! worse on any rule; the table is left as it is, and the new one written
//! beside it as `<map>.quality.new.tsv`. `--quality` again accepts it.
//! `gs.map` is not in the repository, so the gate is this command rather
//! than a `cargo test`.
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

/// The columns the gate holds, by name and index among a row's counts (the
/// area's name is not one of them): each may not grow.
const GATED: [(&str, usize); 5] = [
    ("exits against their direction", 2),
    ("lines through rooms", 3),
    ("building rooms under a line not theirs", 4),
    ("directionless exits not drawn", 8),
    ("directionless lines crossing another", 9),
];

const HEADER: &str = "area\trooms\tms\tagainst_bearing\tlines_through_rooms\t\
                      rooms_under_foreign_lines\tdirectionless\tlines\tstubs\tundrawn\t\
                      crossing\tlen_median\tlen_p90\n";

/// Measure every area of the map at `path`. With `check`, compare with the
/// table beside the map and refuse a worse area; otherwise write the table.
///
/// # Errors
///
/// The map or its store cannot be read, the table cannot be written, or,
/// with `check`, an area is worse than the table says.
pub fn quality_headless(path: Option<&Path>, check: bool) -> Result<String, String> {
    let path = path.ok_or("no map path: give one, or set CENA_MAP")?;
    let started = Instant::now();
    let rows = measure_all(path)?;
    let wall = started.elapsed();
    let mut total = Quality::default();
    let mut laying = Duration::ZERO;
    for row in &rows {
        total.add(&row.quality);
        laying += row.time;
    }
    let text = table(&rows, laying, &total);
    let summary = summary(&rows, &total, laying, wall);
    let baseline = path.with_extension("quality.tsv");
    if !check {
        std::fs::write(&baseline, text).map_err(|e| format!("{} {e}", baseline.display()))?;
        return Ok(format!("{summary}\nwritten to {}", baseline.display()));
    }
    let old = std::fs::read_to_string(&baseline).map_err(|e| {
        format!(
            "{} {e}: run --quality first, to have a table to check against",
            baseline.display()
        )
    })?;
    let new_table = path.with_extension("quality.new.tsv");
    std::fs::write(&new_table, &text).map_err(|e| format!("{} {e}", new_table.display()))?;
    let (worse, better) = compare(&parse(&old), &parse(&text));
    let mut out = format!("{summary}\nwritten to {}\n", new_table.display());
    for (rule, index) in GATED {
        let was = parse(&old).get("TOTAL").map_or(0, |c| c[index]);
        let now = parse(&text).get("TOTAL").map_or(0, |c| c[index]);
        let _ = writeln!(out, "  {rule}: {was} -> {now}");
    }
    if worse.is_empty() {
        let _ = write!(out, "no area worse on any rule; better in {better} places");
        Ok(out)
    } else {
        let _ = writeln!(out, "WORSE, in {} places:", worse.len());
        for line in worse {
            let _ = writeln!(out, "  {line}");
        }
        Err(out)
    }
}

/// Every area of the map at `path`, laid out and measured.
fn measure_all(path: &Path) -> Result<Vec<Row>, String> {
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
    Ok(rows)
}

/// The table: a row per area, then the total.
fn table(rows: &[Row], laying: Duration, total: &Quality) -> String {
    let mut text = String::from(HEADER);
    for row in rows {
        line(&mut text, &row.name, row.time, &row.quality);
    }
    line(&mut text, "TOTAL", laying, total);
    text
}

fn summary(rows: &[Row], total: &Quality, laying: Duration, wall: Duration) -> String {
    let slowest = rows.iter().max_by_key(|row| row.time);
    format!(
        "{} areas, {} rooms, laid out in {:.1} s ({:.1} s with measuring); slowest {}\n\
         rule 1, exits against their direction: {}\n\
         rule 2, lines through rooms: {}\n\
         rule 3, building rooms under a line not theirs: {}\n\
         rule 4, pairs joined only without a direction: {} -- {} lines ({} crossing another), \
         {} stubs, {} not drawn; line length median {}, p90 {}",
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
    )
}

/// A table's counts by area: every column read as a whole number, a length
/// or a `-` as 0 (the gate reads only counts).
fn parse(text: &str) -> BTreeMap<String, Vec<u64>> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_owned();
            let counts = fields.map(|f| f.parse().unwrap_or(0)).collect();
            Some((name, counts))
        })
        .collect()
}

/// Where `new` is worse than `old` on a gated rule, as lines to show, and
/// how many places it is better. An area only one of them has is neither.
fn compare(
    old: &BTreeMap<String, Vec<u64>>,
    new: &BTreeMap<String, Vec<u64>>,
) -> (Vec<String>, usize) {
    let mut worse = Vec::new();
    let mut better = 0;
    for (area, now) in new {
        let Some(was) = old.get(area) else {
            continue;
        };
        for (rule, index) in GATED {
            let (was, now) = (
                was.get(index).copied().unwrap_or(0),
                now.get(index).copied().unwrap_or(0),
            );
            if now > was {
                worse.push(format!("{area}: {rule} {was} -> {now}"));
            } else if now < was {
                better += 1;
            }
        }
    }
    (worse, better)
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

#[cfg(test)]
mod tests {
    use super::{compare, parse};

    const OLD: &str = "area\trooms\tms\tagainst\tthrough\tunder\td\tl\ts\tundrawn\tcrossing\tm\tp\n\
                       a\t10\t5\t3\t2\t1\t4\t4\t0\t0\t1\t1.4\t2.0\n\
                       b\t10\t5\t0\t0\t0\t0\t0\t0\t0\t0\t-\t-\n";

    /// A rule that grows in an area is worse, whatever the others did;
    /// one that shrinks is better; a new area is neither.
    #[test]
    fn an_area_worse_on_one_rule_is_refused() {
        let new = OLD
            .replace("a\t10\t5\t3\t2\t1", "a\t10\t9\t2\t2\t1")
            .replace("b\t10\t5\t0\t0\t0", "b\t10\t5\t0\t1\t0")
            + "c\t5\t1\t9\t9\t9\t0\t0\t0\t0\t0\t-\t-\n";
        let (worse, better) = compare(&parse(OLD), &parse(&new));
        assert_eq!(worse, ["b: lines through rooms 0 -> 1"]);
        assert_eq!(better, 1, "a's exits against their direction, 3 -> 2");
        let (worse, better) = compare(&parse(OLD), &parse(OLD));
        assert!(worse.is_empty() && better == 0, "the same table passes");
    }
}
