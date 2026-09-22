//! The window's state and its `eframe::App` implementation.

use std::collections::BTreeSet;
use std::path::Path;

use cena_map::Map;
use cena_map_layout::{Layout, MapScene, build_scene, generate_layout};

use crate::draw;

/// What went wrong loading the map file, said to the player in the window
/// rather than only on stderr -- a tool that cannot show a map should say
/// why, not open blank.
#[derive(Debug)]
enum LoadProblem {
    NoPath,
    CouldNotRead {
        path: String,
        error: std::io::Error,
    },
    NotAMap {
        path: String,
        error: cena_map::binary::LoadError,
    },
}

impl std::fmt::Display for LoadProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadProblem::NoPath => write!(
                f,
                "No map file given. Set CENA_MAP or pass a path as the first argument."
            ),
            LoadProblem::CouldNotRead { path, error } => write!(f, "Cannot read {path}: {error}"),
            LoadProblem::NotAMap { path, error } => {
                write!(f, "{path} is not a map this build can read: {error}")
            }
        }
    }
}

/// The generated layout for one location, kept alongside its scene so
/// switching locations does not require holding every location's scene at
/// once.
struct Shown {
    location: String,
    #[allow(dead_code)] // read by a future inspector panel; kept for now
    layout: Layout,
    scene: MapScene,
}

pub struct MapperApp {
    /// `Err` once, at startup, and shown instead of a window full of
    /// nothing; loading never happens again from inside the app (v1 is a
    /// one-shot viewer, not a file-open dialog -- `plan/26` names that as
    /// later work, not this one).
    map: Result<Map, LoadProblem>,
    /// Every distinct `Room::location`, sorted, for the picker. Rooms with
    /// no location are grouped under an empty string, which sorts first.
    locations: Vec<String>,
    selected: Option<usize>,
    shown: Option<Shown>,
}

impl MapperApp {
    #[must_use]
    pub fn load(path: Option<&Path>) -> MapperApp {
        let map = load_map(path);
        let locations = map.as_ref().map(distinct_locations).unwrap_or_default();
        MapperApp {
            map,
            locations,
            selected: None,
            shown: None,
        }
    }

    /// Compute (or recompute) the layout and scene for the selected
    /// location. Cheap enough to redo on every selection change at the
    /// sizes this tool has been run against so far (`plan/26` §5 step 4
    /// still owes a real measurement at Wehnimer's-Landing scale).
    fn show_selected(&mut self) {
        let Ok(map) = &self.map else {
            return;
        };
        let Some(idx) = self.selected else {
            self.shown = None;
            return;
        };
        let Some(location) = self.locations.get(idx).cloned() else {
            return;
        };
        let rooms: Vec<cena_map::Room> = map
            .rooms()
            .iter()
            .filter(|r| room_location(r) == location)
            .cloned()
            .collect();
        let Ok(subset) = Map::from_rooms(rooms) else {
            // Two rooms sharing an id within one location cannot happen --
            // the whole map already rejected duplicate ids on load -- but a
            // filtered subset never panics over it either way.
            self.shown = None;
            return;
        };
        let layout = generate_layout(&subset);
        let scene = build_scene(&location, &layout, &subset);
        self.shown = Some(Shown {
            location,
            layout,
            scene,
        });
    }
}

impl eframe::App for MapperApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Err(problem) = &self.map {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading("Hydra Mapper");
                ui.colored_label(egui::Color32::from_rgb(200, 80, 80), problem.to_string());
            });
            return;
        }

        let mut changed = false;
        egui::Panel::left("locations").show(ui, |ui| {
            ui.heading("Locations");
            ui.label(format!("{} places", self.locations.len()));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (idx, location) in self.locations.iter().enumerate() {
                    let label = if location.is_empty() {
                        "(no location)"
                    } else {
                        location.as_str()
                    };
                    let selected = self.selected == Some(idx);
                    if ui.selectable_label(selected, label).clicked() {
                        self.selected = Some(idx);
                        changed = true;
                    }
                }
            });
        });
        if changed {
            self.show_selected();
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(shown) = &self.shown {
                ui.heading(&shown.location);
                draw::scene(ui, &shown.scene);
            } else {
                ui.heading("Hydra Mapper");
                ui.label("Choose a location on the left.");
            }
        });
    }
}

fn load_map(path: Option<&Path>) -> Result<Map, LoadProblem> {
    let Some(path) = path else {
        return Err(LoadProblem::NoPath);
    };
    let path_str = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|error| LoadProblem::CouldNotRead {
        path: path_str.clone(),
        error,
    })?;
    cena_map::binary::decode(&bytes).map_err(|error| LoadProblem::NotAMap {
        path: path_str,
        error,
    })
}

/// A room's location, or `""` for one with none -- the same bucket the
/// picker groups them under.
fn room_location(room: &cena_map::Room) -> &str {
    room.location.as_deref().unwrap_or("")
}

fn distinct_locations(map: &Map) -> Vec<String> {
    let set: BTreeSet<&str> = map.rooms().iter().map(room_location).collect();
    set.into_iter().map(str::to_owned).collect()
}
