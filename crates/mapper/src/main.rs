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
mod draw;
mod export;
mod focus;
mod inspect;
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
    // `--quality [map]`: every area laid out, measured by the four rules
    // and timed, into `<map>.quality.tsv`. `--quality-check [map]`: the
    // same, refused where an area is worse than that table.
    if let Some(check) = match flag.as_deref() {
        Some("--quality") => Some(false),
        Some("--quality-check") => Some(true),
        _ => None,
    } {
        let path = std::env::args()
            .nth(2)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(MAP_ENV).map(PathBuf::from))
            .or_else(bundle::default_path);
        match quality_report::quality_headless(path.as_deref(), check) {
            Ok(note) => println!("{note}"),
            Err(problem) => {
                eprintln!("{problem}");
                std::process::exit(1);
            }
        }
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
