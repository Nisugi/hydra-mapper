//! The window's state and its `eframe::App` implementation.

use std::path::Path;

use cena_map::Map;
use cena_map_layout::scene::Sheet;
use cena_map_layout::{Layout, MapScene, build_scene, generate_layout};

use crate::areas::{AreaKind, Areas};
use crate::camera::Camera;
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

/// The generated layout for one area, kept alongside its scene so
/// switching areas does not require holding every area's scene at once.
struct Shown {
    name: String,
    #[allow(dead_code)] // read by a future inspector panel; kept for now
    layout: Layout,
    scene: MapScene,
    /// Set when the area has just changed, so the next frame -- the first
    /// one that knows how big the canvas is -- fits the camera to it.
    needs_fit: bool,
}

/// Which list, and which entry in it, the person has picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selection {
    kind: AreaKind,
    index: usize,
}

pub struct MapperApp {
    /// `Err` once, at startup, and shown instead of a window full of
    /// nothing; loading never happens again from inside the app (v1 is a
    /// one-shot viewer, not a file-open dialog -- `plan/26` names that as
    /// later work, not this one).
    map: Result<Map, LoadProblem>,
    /// The two cross-cutting groupings a person can browse by; see
    /// [`crate::areas`] for why there are two.
    areas: Areas,
    /// Which list the picker is showing.
    tab: AreaKind,
    /// Case-insensitive filter over the shown list. The mapdb list runs to
    /// hundreds of entries, which is more than a person scrolls through.
    filter: String,
    selected: Option<Selection>,
    /// Outdoor sheet or the interiors shelf. Both are computed; v1 drew
    /// only the outdoor one because there was no way to ask for the other.
    sheet: Sheet,
    camera: Camera,
    shown: Option<Shown>,
}

impl MapperApp {
    #[must_use]
    pub fn load(path: Option<&Path>) -> MapperApp {
        let map = load_map(path);
        let areas = match &map {
            Ok(map) => Areas::build(map),
            Err(_) => Areas {
                official: Vec::new(),
                mapdb: Vec::new(),
            },
        };
        MapperApp {
            map,
            areas,
            tab: AreaKind::Mapdb,
            filter: String::new(),
            selected: None,
            sheet: Sheet::Outdoor,
            camera: Camera::default(),
            shown: None,
        }
    }

    /// Compute (or recompute) the layout and scene for the selected area.
    /// Cheap enough to redo on every selection change at the sizes this
    /// tool has been run against so far (`plan/26` §5 step 4 still owes a
    /// real measurement at Wehnimer's-Landing scale).
    fn show_selected(&mut self) {
        let Ok(map) = &self.map else {
            return;
        };
        let Some(selection) = self.selected else {
            self.shown = None;
            return;
        };
        let Some(area) = self.areas.list(selection.kind).get(selection.index) else {
            self.shown = None;
            return;
        };
        let rooms: Vec<cena_map::Room> = area
            .rooms
            .iter()
            .filter_map(|&id| map.room(id).cloned())
            .collect();
        let Ok(subset) = Map::from_rooms(rooms) else {
            // Two rooms sharing an id within one area cannot happen -- the
            // whole map already rejected duplicate ids on load -- but a
            // filtered subset never panics over it either way.
            self.shown = None;
            return;
        };
        let layout = generate_layout(&subset);
        let scene = build_scene(&area.name, &layout, &subset);
        self.shown = Some(Shown {
            name: area.name.clone(),
            layout,
            scene,
            needs_fit: true,
        });
    }

    /// The left panel: the two list tabs, a filter box, and the list.
    fn picker(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        ui.heading("Areas");
        ui.horizontal(|ui| {
            for kind in [AreaKind::Official, AreaKind::Mapdb] {
                let label = format!("{} ({})", kind.title(), self.areas.list(kind).len());
                if ui.selectable_label(self.tab == kind, label).clicked() {
                    self.tab = kind;
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter"));
        ui.separator();

        let needle = self.filter.to_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, area) in self.areas.list(self.tab).iter().enumerate() {
                if !needle.is_empty() && !area.name.to_lowercase().contains(&needle) {
                    continue;
                }
                let selection = Selection {
                    kind: self.tab,
                    index,
                };
                let selected = self.selected == Some(selection);
                let label = format!("{}  ({})", area.name, area.rooms.len());
                if ui.selectable_label(selected, label).clicked() {
                    self.selected = Some(selection);
                    changed = true;
                }
            }
        });
        changed
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
        egui::Panel::left("areas").show(ui, |ui| {
            changed = self.picker(ui);
        });
        if changed {
            self.show_selected();
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(shown) = &mut self.shown else {
                ui.heading("Hydra Mapper");
                ui.label("Choose an area on the left.");
                return;
            };

            ui.horizontal(|ui| {
                ui.heading(&shown.name);
                ui.separator();
                for (sheet, label) in [(Sheet::Outdoor, "Outdoor"), (Sheet::Interiors, "Interiors")]
                {
                    let count = shown.scene.sheet(sheet).rooms.len();
                    let chosen = self.sheet == sheet;
                    // An empty sheet stays visible but unclickable, so it
                    // is clear the location simply has no interiors rather
                    // than the toggle having gone missing.
                    ui.add_enabled_ui(count > 0, |ui| {
                        if ui
                            .selectable_label(chosen, format!("{label} ({count})"))
                            .clicked()
                        {
                            self.sheet = sheet;
                            shown.needs_fit = true;
                        }
                    });
                }
                ui.separator();
                if ui.button("Fit").clicked() {
                    shown.needs_fit = true;
                }
                ui.label("drag to pan, wheel to zoom");
            });

            // Fit on the first frame after a change, when the canvas size
            // is finally known -- `load` has no window to measure.
            if shown.needs_fit {
                let sheet = shown.scene.sheet(self.sheet);
                let canvas = ui.available_rect_before_wrap();
                if canvas.width() > 0.0 && canvas.height() > 0.0 {
                    self.camera.fit(sheet.min, sheet.max, canvas);
                    shown.needs_fit = false;
                }
            }

            draw::scene(ui, &shown.scene, self.sheet, &mut self.camera);
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
