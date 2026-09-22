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
mod areas;
mod camera;
mod draw;
mod export;
mod inspect;
mod overrides;

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
}
