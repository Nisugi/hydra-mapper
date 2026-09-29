//! The standalone map explorer window (`plan/26`).
//!
//! Loads a combined `.map` file, lets a person pick a location, and draws
//! the layout `cena-map-layout` computes for it. Read-only: v1 is an
//! explorer, not an editor (`plan/26` §0) -- no write-back, no override
//! system, nothing here changes the map file.
//!
//! This binary is thin on purpose. The layout algorithm lives entirely in
//! `cena-map-layout`, which knows nothing about `egui`; a future Hydra GUI
//! panel can depend on that crate directly and never need this one.

mod app;
mod area_fill;
mod areas;
mod bundle;
mod camera;
mod compare;
mod draw;
mod export;
mod focus;
mod inspect;
mod maps;
mod overrides;
mod placement;
mod quality_report;
mod room_table;
mod svg;

use std::path::PathBuf;

use app::MapperApp;

/// Same env var `cena`'s own `travel.rs` uses, so a person who has already
/// set it up for the client does not need a second one for this tool.
///
/// ```text
/// $env:CENA_MAP = "E:\Gemstone\data\cena_data\gs.map"
/// ```
const MAP_ENV: &str = "CENA_MAP";

fn main() -> eframe::Result {
    // `cena-mapper --export-areas [map]`: write the room/area/region table
    // beside the store and exit, no window. `--export-curation [map]`:
    // write curation/assignments.toml for retag, likewise.
    let flag = std::env::args().nth(1);
    // `cena-mapper --import <changes.json> [map]`: merge a contributor's
    // changes into the store beside the map.
    if flag.as_deref() == Some("--import") {
        let Some(changes) = std::env::args().nth(2).map(PathBuf::from) else {
            eprintln!("usage: cena-mapper --import <changes.json> [map]");
            std::process::exit(2);
        };
        let path = std::env::args()
            .nth(3)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(MAP_ENV).map(PathBuf::from))
            .or_else(bundle::default_path);
        match app::import_changes_headless(&changes, path.as_deref()) {
            Ok(note) => println!("{note}"),
            Err(problem) => {
                eprintln!("{problem}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    // `--compare <area> [map]`: the area as the engine draws it, against
    // the same with the store's hand corrections, and what each did.
    if flag.as_deref() == Some("--compare") {
        let Some(area) = std::env::args().nth(2) else {
            eprintln!("usage: cena-mapper --compare <area> [map]");
            std::process::exit(2);
        };
        finish(compare::compare_headless(&area, map_at(3).as_deref()));
        return Ok(());
    }
    // `--quality [map]`: every area laid out, measured by the four rules
    // and timed, into `<map>.quality.tsv`. `--quality-check [map]`: the
    // same, refused where an area is worse than that table.
    if let Some(check) = match flag.as_deref() {
        Some("--quality") => Some(false),
        Some("--quality-check") => Some(true),
        _ => None,
    } {
        finish(quality_report::quality_headless(
            map_at(2).as_deref(),
            check,
        ));
        return Ok(());
    }
    // `--suggest-maps [map]`: the areas worth putting on one map.
    // `--accept-map <name> [map]`: put the suggestion of that name on it,
    // in the store, as the window's Accept does.
    if flag.as_deref() == Some("--suggest-maps") {
        finish(maps::suggest_headless(map_at(2).as_deref(), None));
        return Ok(());
    }
    if flag.as_deref() == Some("--accept-map") {
        let Some(name) = std::env::args().nth(2) else {
            eprintln!("usage: cena-mapper --accept-map <name> [map]");
            std::process::exit(2);
        };
        finish(maps::suggest_headless(map_at(3).as_deref(), Some(&name)));
        return Ok(());
    }
    // `--plan-areas [map]`: say where the rooms with no area would go.
    // `--fill-areas [map]`: put them there, in the store.
    if let Some(write) = match flag.as_deref() {
        Some("--plan-areas") => Some(false),
        Some("--fill-areas") => Some(true),
        _ => None,
    } {
        let path = std::env::args()
            .nth(2)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(MAP_ENV).map(PathBuf::from))
            .or_else(bundle::default_path);
        match app::fill_areas_headless(path.as_deref(), write) {
            Ok(note) => println!("{note}"),
            Err(problem) => {
                eprintln!("{problem}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if let Some(flag) = flag.as_deref().filter(|f| f.starts_with("--export-")) {
        let path = std::env::args()
            .nth(2)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(MAP_ENV).map(PathBuf::from))
            .or_else(bundle::default_path);
        let result = match flag {
            "--export-areas" => app::export_areas_headless(path.as_deref()),
            "--export-curation" => app::export_curation_headless(path.as_deref()),
            other => Err(format!("unknown flag {other}")),
        };
        match result {
            Ok(note) => println!("{note}"),
            Err(problem) => {
                eprintln!("{problem}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let path = map_path();
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Hydra Mapper",
        options,
        Box::new(move |_cc| Ok(Box::new(MapperApp::load(path.as_deref())))),
    )
}

/// The map named by the `n`th argument, or `CENA_MAP`, or the bundled one.
fn map_at(n: usize) -> Option<PathBuf> {
    std::env::args()
        .nth(n)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(MAP_ENV).map(PathBuf::from))
        .or_else(bundle::default_path)
}

/// A headless mode's answer on standard output, or its problem on standard
/// error and exit 1.
fn finish(result: Result<String, String>) {
    match result {
        Ok(note) => println!("{note}"),
        Err(problem) => {
            eprintln!("{problem}");
            std::process::exit(1);
        }
    }
}

/// The map file to open: `CENA_MAP`, or the first command-line argument, so
/// `cena-mapper path\to\hydra.map` also works without setting an env var
/// first. `None` when neither is given; the window still opens and says so.
fn map_path() -> Option<PathBuf> {
    std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(MAP_ENV).map(PathBuf::from))
        // A bundled build with neither uses its embedded map (see `bundle`).
        .or_else(bundle::default_path)
}
