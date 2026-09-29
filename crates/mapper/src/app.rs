//! The window's state and its `eframe::App` implementation.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use cena_map::{Map, RoomId};
use cena_map_layout::Cell;
use cena_map_layout::scene::SceneRoom;
use cena_map_layout::{
    Dir, EdgeAction, Layout, LayoutParams, MapScene, build_scene, generate_layout_tuned,
};

use crate::areas::{self, AreaKind, Areas};
use crate::camera::Camera;
use crate::draw;
use crate::export;
use crate::focus::Focus;
use crate::inspect::{Crossed, RoomFacts, bearing};
use crate::overrides::{self, Baseline, MapOverrides, RoomKey};
use crate::placement;
use crate::svg;

/// What went wrong loading the map file, said to the player in the window
/// rather than only on stderr -- a tool that cannot show a map should say
/// why, not open blank.
#[derive(Debug)]
pub(crate) enum LoadProblem {
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
    /// What is drawn as squares. See [`crate::focus`].
    focus: Focus,
    /// Where this area's corrections live in the store. See
    /// [`Area::store_key`].
    store_key: String,
    /// Read by the inspector panel, for the diagnostics the scene does not
    /// carry: group, pack method and direction violations.
    layout: Layout,
    /// The area's own rooms, kept so the inspector can read full room
    /// records -- the scene carries only what it needs to draw.
    subset: Map,
    scene: MapScene,
    /// A curated area whose rooms disagree about their region: the region
    /// most of them carry, and the drawn rooms that carry another (or
    /// none). Empty when they all agree.
    majority: String,
    off_region: HashSet<RoomId>,
    /// Set when the area has just changed, so the next frame -- the first
    /// one that knows how big the canvas is -- fits the camera to it.
    needs_fit: bool,
}

impl Shown {
    /// What the canvas draws as squares, as dots, and in red.
    fn draw_focus(&self, flag_off_region: bool) -> draw::Focus<'_> {
        draw::Focus {
            rooms: self.focus.rooms(),
            streets: self.focus.streets(),
            doors: self.focus.doors(),
            flagged: flag_off_region.then_some(&self.off_region),
        }
    }
}

/// Panel colors. The canvas keeps its own in [`crate::draw`]; these are
/// only for the inspector's text.
const ENTRANCE_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 170, 60);
const SCRIPTED_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 190, 240);
const IMPASSABLE_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 120, 110);
const VIOLATION_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 180, 90);

/// Which list, and which entry in it, the person has picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selection {
    kind: AreaKind,
    index: usize,
}

/// Whether a rebuild of this kind re-fits the camera.
const fn needs_fit_for(fit: Fit) -> bool {
    matches!(fit, Fit::Reset)
}

/// Whether rebuilding the shown area should re-fit the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// Centre and zoom to the sheet: a different area is being shown.
    Reset,
    /// Leave the camera alone: the same area, redrawn after an edit.
    Keep,
}

/// One completed edit, applied after the frame's panels have let go of
/// their borrows. Every one of these saves the store.
#[derive(Debug, Clone)]
enum EditAction {
    /// Shift a whole group by a cell delta.
    NudgeGroup { anchor: RoomKey, delta: Cell },
    /// Place rooms, each within its own group's frame: the one room
    /// Alt-dragged, or every picked room dragged together.
    PinRooms { pins: Vec<(RoomKey, Cell)> },
    /// Drop a room's pin, returning it to where the solver put it.
    UnpinRoom { key: RoomKey },
    /// Forget every correction for the shown area.
    ResetLocation,
    /// Delete a plate, releasing its rooms back to their own areas.
    DeletePlate { plate: String },
    /// Say which area a plate is a sheet of.
    SetPlateArea { plate: String, area: String },
    /// Set, replace or clear the correction on one edge.
    SetEdge {
        a: RoomKey,
        b: RoomKey,
        action: Option<EdgeAction>,
    },
    /// Move rooms onto a plate, or (with `None`) back to their own area.
    MoveRooms {
        keys: Vec<RoomKey>,
        to: Option<String>,
    },
    /// Mint a plate and move rooms onto it in one action.
    NewPlate { name: String, keys: Vec<RoomKey> },
    /// Put rooms in a curated area, or (with `None`) take them out.
    ///
    /// Distinct from [`EditAction::MoveRooms`] because a plate and an
    /// area are different things: a plate is a sheet a room is DRAWN on,
    /// an area is the place it IS. A room can be on a plate and in an
    /// area at once, and neither answer should overwrite the other.
    AssignArea {
        keys: Vec<RoomKey>,
        to: Option<String>,
    },
    /// Mint a curated area and put rooms in it in one action.
    NewArea { name: String, keys: Vec<RoomKey> },
    /// Delete a curated area, releasing its rooms.
    DeleteArea { area: String },
    /// Say which region rooms are in, or (with `None`) stop saying.
    ///
    /// The end state this is for: every room carrying its region
    /// outright, instead of the three mechanisms deriving one today --
    /// a uid join onto the official mapdb, the `[[fold]]` table that
    /// corrects it where `loc` named an area rather than a region, and
    /// an inference that spreads a region into unregioned ground. A
    /// quarter of the regioned rooms are currently a guess, marked
    /// `map:region-inferred`. An assignment here outranks all of it.
    AssignRegion {
        keys: Vec<RoomKey>,
        to: Option<String>,
    },
    /// Put areas on a map, or (with `None`) take them off it.
    MapAreas {
        areas: Vec<String>,
        map: Option<String>,
    },
}

pub struct MapperApp {
    /// The layout's knobs, set from the canvas header. A change re-solves
    /// the shown area.
    params: LayoutParams,
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
    camera: Camera,
    /// The room the inspector panel is describing, if any.
    inspected: Option<RoomId>,
    shown: Option<Shown>,

    // --- editing ---
    /// Hand-made corrections: moves, pins and plates. Saved on every edit.
    store: MapOverrides,
    /// Where the store is saved. `None` when no map path was given, which
    /// also means corrections cannot be kept.
    store_path: Option<PathBuf>,
    /// A store that would not load or save, said in the window. While this
    /// is set from a failed *load*, editing stays off.
    store_problem: Option<String>,
    /// The canvas toggles: editing, labels, every interior room as a dot.
    /// Labels and interiors start off -- a town's worth of titles, or of
    /// floor plans, is the first thing anyone turns off.
    view: draw::View,
    drag: Option<DragState>,
    /// Name being typed for a new plate.
    new_plate: String,
    /// Name being typed for a new curated area.
    new_area: String,
    /// The region box: filled from the region dropdown, applied by the
    /// + buttons.
    new_region: String,
    /// Regions whose areas are folded away in the Region tree.
    collapsed: BTreeSet<String>,
    /// Rooms picked out to be assigned together.
    ///
    /// **Rooms, not groups.** It was groups first, and that was wrong: a
    /// "group" is the solver's connected component, which on a town sheet
    /// is every outdoor room drawn -- 246 of them on Mist Harbor. Picking
    /// fifteen of those was impossible; Ctrl-click took all 246 or none.
    /// So the unit is the room, picked two ways:
    ///
    ///   Ctrl-click   one room
    ///   Ctrl-drag    every room drawn inside the box
    ///
    /// Kept to the rooms of the shown sheet: a new area empties it, and a
    /// room an edit takes off this sheet (a plate move) drops out, so
    /// nothing off screen is assigned by a panel describing what is on it.
    picked_rooms: BTreeSet<RoomId>,
    /// A room to inspect once its area is on screen, from a room-number
    /// search. Applied after `show_selected`, which clears the selection.
    pending_inspect: Option<RoomId>,
    /// The rooms walked through to reach the inspected one, most recent
    /// last. Following an exit out of an area can go several rooms deep
    /// into a building, and without this the only way back is finding the
    /// room again on the canvas.
    trail: Vec<RoomId>,
    /// What the last export did, shown until the next one.
    export_note: Option<String>,
    /// Areas ticked for an SVG export. Kept by name rather than by
    /// selection index, because the two lists are filtered independently
    /// and an index means nothing once the filter changes.
    svg_areas: BTreeSet<String>,
    /// Whether the area list is showing its ticks. Its own toggle rather
    /// than edit mode's: choosing what to draw happens while browsing,
    /// and edit mode is only reachable once an area is already on screen.
    picking_svgs: bool,
}

/// A drag in progress. Committed as one correction on release, so dragging
/// a group across the sheet is a single entry rather than one per frame.
#[derive(Debug, Clone)]
struct DragState {
    /// What the drag carries.
    moving: Moving,
    /// Pixels moved so far, converted to whole cells on release.
    accumulated: egui::Vec2,
}

/// What a drag carries, decided when it starts.
#[derive(Debug, Clone)]
enum Moving {
    /// A whole group, by shifting its offset.
    Group(usize),
    /// These rooms, each pinned in its own group's frame: the one room
    /// under an Alt-drag, or every picked room when the drag starts on
    /// one of them. Taken when the drag starts, so the pick is what moves
    /// even if it changes before the release.
    Rooms(Vec<RoomId>),
}

impl MapperApp {
    #[must_use]
    pub fn load(path: Option<&Path>) -> MapperApp {
        let map = load_map(path);
        let store_path = path.map(overrides::store_path);
        let (store, store_problem) = match store_path.as_deref() {
            Some(path) => match MapOverrides::load(path) {
                Ok(store) => (store, None),
                // A store that will not parse is NOT discarded: it is
                // hand-curated work. Editing stays off until it is fixed
                // or moved aside, so a stray save cannot overwrite it.
                Err(error) => (
                    MapOverrides::default(),
                    Some(format!("{} {error}", path.display())),
                ),
            },
            None => (MapOverrides::default(), None),
        };
        let mut store = store;
        // Seed the area tree from the official layout the first time a
        // map is opened with no areas in it. Never when the store failed
        // to parse: editing is off in that case precisely so nothing
        // overwrites hand-curated work, and a seed is an edit.
        let seeded = match (&map, store_problem.is_none()) {
            (Ok(map), true) => areas::seed_from_official(map, &mut store),
            _ => 0,
        };
        let areas = match &map {
            Ok(map) => Areas::build(map, &store),
            Err(_) => Areas {
                official: Vec::new(),
                region: Vec::new(),
                location: Vec::new(),
                derived: Vec::new(),
                plates: Vec::new(),
                maps: Vec::new(),
                placeable: HashSet::new(),
                baseline: crate::overrides::Baseline::default(),
            },
        };
        // **Regions start shut.** 52 of them with their areas open is 222
        // rows, and whichever one someone is looking for is off the
        // bottom of the screen. A region is a heading; the list of them
        // is what the tab is for, and opening one is the act of choosing
        // it. `collapsed` records what is shut rather than what is open,
        // so seeding it with every region is the whole default.
        let collapsed: BTreeSet<String> = areas
            .list(AreaKind::Region)
            .iter()
            .filter(|a| a.parent.is_none())
            .map(|a| a.name.clone())
            .collect();

        if seeded > 0
            && let Some(path) = store_path.as_deref()
            && let Err(error) = store.save(path)
        {
            eprintln!("could not save the seeded areas: {error}");
        }
        MapperApp {
            params: LayoutParams::default(),
            map,
            areas,
            tab: AreaKind::Location,
            filter: String::new(),
            selected: None,
            camera: Camera::default(),
            inspected: None,
            shown: None,
            store,
            store_path,
            store_problem,
            view: draw::View {
                edit_mode: false,
                labels: false,
                interiors: false,
                off_region: false,
            },
            drag: None,
            new_plate: String::new(),
            new_area: String::new(),
            new_region: String::new(),
            collapsed,
            picked_rooms: BTreeSet::new(),
            pending_inspect: None,
            trail: Vec::new(),
            export_note: None,
            svg_areas: BTreeSet::new(),
            picking_svgs: false,
        }
    }

    /// Which rooms of each area land on its interiors shelf.
    ///
    /// Recomputed here rather than cached: exporting is rare, and a stale
    /// answer would put rooms on the wrong grid.
    /// Every dragged room's placement, across every area that has one.
    ///
    /// Each area is re-solved and its corrections applied, because an
    /// offset is measured against the cells the person was looking at --
    /// the corrected layout, not the solver's first answer. Areas with no
    /// drags are skipped rather than solved for nothing.
    fn placements_across_areas(&self, map: &Map) -> Vec<placement::Resolved> {
        let mut out = Vec::new();
        // Region is here because a person can drag one like any other
        // area and expects the correction to survive. It is NOT in the
        // "which area holds this room" lookups below: a region covers the
        // whole map, official rooms included, so it would shadow every
        // more specific name it contains.
        for kind in [
            AreaKind::Official,
            AreaKind::Region,
            AreaKind::Maps,
            AreaKind::Location,
            AreaKind::Plates,
        ] {
            for area in self.areas.list(kind) {
                let Some(location) = self.store.location(&area.store_key()) else {
                    continue;
                };
                if location.group_offsets.is_empty() && location.room_pins.is_empty() {
                    continue;
                }
                let rooms = areas::layout_rooms(&area.rooms, map, &self.areas.placeable);
                let Ok(subset) = Map::from_rooms(rooms) else {
                    continue;
                };
                let edges = location.edge_overrides(&subset);
                let mut layout = generate_layout_tuned(&subset, &edges, self.params);
                overrides::apply(&mut layout, &subset, location);
                out.extend(placement::resolve(&layout, &subset, location));
            }
        }
        out
    }

    /// The bar above everything: the export button, and whatever the
    /// corrections as a whole have to say. Returns which export was
    /// asked for: the combiner submission, or the region assignments.
    fn corrections_bar(&mut self, ui: &mut egui::Ui, can_edit: bool) -> (bool, bool, bool) {
        // A store that will not load or save is said once, at the top,
        // because it means corrections are not being kept. The export
        // note shares the bar: both are about the corrections as a whole,
        // not about whatever area is on screen.
        let mut export_now = false;
        let mut export_regions_now = false;
        let mut export_areas_now = false;
        if self.store_problem.is_some() || self.export_note.is_some() || can_edit {
            egui::Panel::top("corrections").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let count = self.store.locations.values().map(crate::overrides::LocationOverrides::len).sum::<usize>()
                    + self.store.membership_moves.len();
                ui.add_enabled_ui(can_edit && count > 0, |ui| {
                    export_now = ui
                        .button("Export for combiner")
                        .on_hover_text(
                            "Write the corrections as a dirto patch the combiner                                  can merge into the map",
                        )
                        .clicked();
                    // Separate button because it is a separate artefact
                    // for a separate reader: the combiner takes layout
                    // corrections, `retag` takes facts about places.
                    export_regions_now = ui
                        .add_enabled(
                            !self.store.region_moves.is_empty() || !self.store.area_moves.is_empty(),
                            egui::Button::new("Export to curation"),
                        )
                        .on_hover_text(
                            "Write every area and region assignment to curation/assignments.toml for retag to bake into gs.map",
                        )
                        .clicked();
                });
                export_areas_now = ui
                    .add_enabled(self.map.is_ok(), egui::Button::new("Export areas"))
                    .on_hover_text("Write every room's area and region as a TSV beside the store")
                    .clicked();
                if ui
                    .add_enabled(
                        self.store != MapOverrides::default(),
                        egui::Button::new("Export my changes"),
                    )
                    .on_hover_text(
                        "Write every edit made here to my-map-changes.json, to send back; \
                         drop a changes file on this window to take one in",
                    )
                    .clicked()
                {
                    self.export_changes();
                }
                // Picking which areas to draw is a question asked while
                // browsing the list, so its toggle lives here rather than
                // with the canvas: the canvas header only exists once an
                // area is on screen, and that is too late to be choosing.
                ui.add_enabled_ui(can_edit, |ui| {
                    ui.toggle_value(&mut self.picking_svgs, "Pick areas to draw")
                        .on_hover_text(
                            "Tick areas in the list; their sheets and plates \
                             are drawn into the export",
                        );
                });
                let ticked = self.svg_areas.len();
                if ticked > 0 {
                    ui.weak(format!("+ {ticked} area(s) drawn"));
                    if ui.small_button("clear").clicked() {
                        self.svg_areas.clear();
                    }
                }
                if let Some(problem) = &self.store_problem {
                    ui.colored_label(IMPASSABLE_COLOR, format!("Corrections: {problem}"));
                } else if let Some(note) = &self.export_note {
                    ui.label(note);
                } else {
                    ui.weak(format!("{count} correction(s)"));
                }
            });
        });
        }
        (export_now, export_regions_now, export_areas_now)
    }

    /// Draw every ticked area, and every plate hanging off one, as SVG
    /// files in a folder beside the map.
    ///
    /// A plate is drawn alongside the area it was carved out of, because
    /// a reviewer judging "should these rooms be on their own sheet?"
    /// needs to see both halves of that question.
    /// Draw every ticked area and its plates, as slug -> SVG document.
    ///
    /// These ride inside the corrections file rather than being written
    /// beside it: the issue form takes one attachment, and a person
    /// should not have to gather a folder of loose files to submit.
    fn draw_ticked_areas(&self, map: &Map) -> (BTreeMap<String, String>, Vec<String>) {
        // Every ticked area, plus the plates that belong to one. A plate
        // is its own entry in the Plates list, so it is drawn the same
        // way as any other area once its name is known.
        let mut wanted: Vec<String> = self.svg_areas.iter().cloned().collect();
        for area in &self.svg_areas {
            // `plates_of` gives (slug, display name); the Plates list is
            // keyed by the name, which is what finds the rooms below.
            wanted.extend(
                self.store
                    .plates_of(area)
                    .into_iter()
                    .map(|(_, name)| name.to_owned()),
            );
        }
        wanted.sort();
        wanted.dedup();

        let mut drawn: BTreeMap<String, String> = BTreeMap::new();
        let mut problems: Vec<String> = Vec::new();
        for name in &wanted {
            let Some(area) = AreaKind::ALL
                .iter()
                .find_map(|&kind| self.areas.list(kind).iter().find(|a| &a.name == name))
            else {
                continue;
            };
            let rooms = areas::layout_rooms(&area.rooms, map, &self.areas.placeable);
            let Ok(subset) = Map::from_rooms(rooms) else {
                continue;
            };
            let location = self.store.location(&area.store_key());
            let edges = location
                .map(|l| l.edge_overrides(&subset))
                .unwrap_or_default();
            let mut layout = generate_layout_tuned(&subset, &edges, self.params);
            if let Some(location) = location {
                overrides::apply(&mut layout, &subset, location);
            }
            let scene = build_scene(name, &layout, &subset);

            // Two pictures of the one sheet: the streets in focus, and
            // every building in focus. Same names as when they were two
            // sheets, so the issue form's readers need not change.
            let focus = Focus::build(&scene, self.areas.list(AreaKind::Official));
            let streets = focus.units[cena_map_layout::scene::STREETS].rooms.clone();
            let buildings: HashSet<RoomId> = scene
                .sheet
                .rooms
                .iter()
                .map(|r| r.id)
                .filter(|id| !streets.contains(id))
                .collect();
            for (rooms, suffix) in [(&streets, ""), (&buildings, export::INTERIORS_SUFFIX)] {
                if rooms.is_empty() {
                    continue;
                }
                let slug = format!("{name}{suffix}");
                match svg::sheet(&scene.sheet, rooms, &streets, focus.doors(), &slug) {
                    Ok(doc) => {
                        drawn.insert(slug, doc);
                    }
                    // An empty interiors shelf is the normal case for an
                    // outdoor area, and not worth reporting.
                    Err(svg::NotDrawn::Empty) => {}
                    Err(problem) => problems.push(format!("{slug}: {problem}")),
                }
            }
        }
        (drawn, problems)
    }

    /// Walk to a room reached by following an exit, remembering where it
    /// was reached from.
    ///
    /// Following an exit back to a room already on the trail unwinds to
    /// it rather than stacking a second copy: rooms link both ways, so
    /// walking a corridor and back would otherwise grow the trail without
    /// bound and make the back button retrace a path rather than leave
    /// it.
    fn walk_to(&mut self, next: RoomId) {
        walk(&mut self.trail, self.inspected, next);
        self.inspected = Some(next);
    }

    /// The distinct areas the shown sheet's rooms belong to.
    ///
    /// For a plate this is where its rooms came from, which is what makes
    /// a sensible owner. Empty for anything else, since an area does not
    /// need adopting.
    fn areas_of_shown(&self) -> Vec<String> {
        let Some(shown) = self.shown.as_ref() else {
            return Vec::new();
        };
        if self.store.owner_of(&shown.name).is_some() {
            return Vec::new();
        }
        let mut found: Vec<String> = Vec::new();
        let note = |name: &str, found: &mut Vec<String>| {
            if !found.iter().any(|n| n == name) {
                found.push(name.to_owned());
            }
        };
        for room in shown.subset.rooms() {
            for kind in [AreaKind::Official, AreaKind::Location] {
                if let Some(area) = self
                    .areas
                    .list(kind)
                    .iter()
                    .find(|a| a.rooms.contains(&room.id))
                {
                    note(&area.name, &mut found);
                }
            }
            // The areas a room's own exits reach, too. A plate is usually
            // made *from* somewhere -- the well behind a town square --
            // and that somewhere is an official area which, by excluding
            // its interiors, does not contain the plated room at all.
            for exit in &room.exits {
                for kind in [AreaKind::Official, AreaKind::Location] {
                    if let Some(area) = self
                        .areas
                        .list(kind)
                        .iter()
                        .find(|a| a.rooms.contains(&exit.to))
                    {
                        note(&area.name, &mut found);
                    }
                }
            }
        }
        found.sort();
        found
    }

    /// Show a sheet the header's dropdown offered: a plate, by its
    /// key, or the area those plates hang off, by its name.
    fn show_sheet(&mut self, key: &str) {
        // A plate key resolves to its display name; an area's name is
        // already the name, and `plate_name` passes it through.
        let name = self.store.plate_name(key).to_owned();
        self.show_named(&name);
    }

    /// Show whichever area or plate goes by `name`, wherever it is
    /// listed. Plates are looked at first, since a plate's name is the
    /// more specific thing.
    fn show_named(&mut self, name: &str) {
        for kind in [
            AreaKind::Plates,
            AreaKind::Maps,
            AreaKind::Official,
            AreaKind::Location,
            AreaKind::Derived,
            AreaKind::Region,
        ] {
            if let Some(index) = self.areas.list(kind).iter().position(|a| a.name == name) {
                self.tab = kind;
                self.selected = Some(Selection { kind, index });
                self.show_selected();
                return;
            }
        }
    }

    /// Write the corrections out for the map combiner.
    ///
    /// Separate from the store's own save: that file is this editor's
    /// working state, keyed for its own use, whereas this is what another
    /// program consumes -- uid-keyed, in the `dirto` vocabulary the layout
    /// engine already reads.
    /// Write the store's areas and region assignments to
    /// `curation/assignments.toml`, beside the map, for `retag` to bake in.
    ///
    /// **Why not the combiner export.** That file is a submission of
    /// layout corrections -- drags, pins, plates, pictures -- and its
    /// consumer is the combiner. Which area and region a room is in is a
    /// fact about a place, and its consumer is `retag`, which writes it
    /// into `gs.map` where Hydra and every other reader will find it.
    fn export_curation(&mut self) {
        let Some(store_path) = self.store_path.as_deref() else {
            self.export_note = Some("Nothing to export: no map is loaded.".to_owned());
            return;
        };
        let dir = store_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("curation");
        let path = dir.join("assignments.toml");
        let Ok(map) = &self.map else {
            self.export_note = Some("Nothing to export: no map is loaded.".to_owned());
            return;
        };
        let text = crate::room_table::assignments_toml(map, &self.store, &self.areas.baseline);
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text));
        self.export_note = Some(match written {
            Ok(()) => format!(
                "Wrote {} area assignment(s) and {} region assignment(s) to {};                  run `retag apply` to bake them into the map",
                self.store.area_moves.len(),
                self.store.region_moves.len(),
                path.display()
            ),
            Err(error) => format!("Could not write {}: {error}", path.display()),
        });
    }

    /// Every room's area and region, as the store has them now, to
    /// `<store>.areas.tsv`. See [`crate::room_table`].
    fn export_areas(&mut self) {
        let (Ok(map), Some(store_path)) = (&self.map, self.store_path.as_deref()) else {
            self.export_note = Some("Nothing to export: no map is loaded.".to_owned());
            return;
        };
        self.export_note = Some(write_areas(map, &self.store, store_path));
    }

    /// A copy of the store -- every edit made in this mapper, and nothing
    /// the map already says -- as `my-map-changes.json` beside it, to send
    /// back. The store's own format, so importing it is a merge of two
    /// stores.
    fn export_changes(&mut self) {
        let Some(store_path) = self.store_path.as_deref() else {
            self.export_note = Some("Nothing to export: no map is loaded.".to_owned());
            return;
        };
        let path = store_path.with_file_name("my-map-changes.json");
        self.export_note = Some(match self.store.save(&path) {
            Ok(()) => format!("Wrote your changes to {} -- send that file", path.display()),
            Err(error) => format!("Could not write {}: {error}", path.display()),
        });
    }

    /// Changes files dropped on the window, merged into this store.
    fn take_dropped(&mut self, ui: &egui::Ui) {
        let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if self.store_problem.is_some() || self.store_path.is_none() {
            self.export_note =
                Some("Cannot take in changes: this store is not being saved.".to_owned());
            return;
        }
        let mut notes = Vec::new();
        for path in dropped {
            match MapOverrides::load(&path) {
                Ok(theirs) => {
                    let report = self.store.merge(&theirs);
                    notes.push(format!("{}: {report}", path.display()));
                }
                Err(error) => {
                    notes.push(format!("{}: not a changes file ({error})", path.display()));
                }
            }
        }
        self.save_store();
        self.rebuild_areas();
        self.refresh_shown();
        self.export_note = Some(format!("Took in {}", notes.join("; ")));
    }

    fn export_corrections(&mut self) {
        let (Ok(map), Some(store_path)) = (&self.map, self.store_path.as_deref()) else {
            self.export_note = Some("Nothing to export: no map is loaded.".to_owned());
            return;
        };
        let source = store_path
            .file_name()
            .map(|n| n.to_string_lossy().replace(".overrides.json", ".map"));
        // Every area's interiors shelf, so the export can give it its own
        // grid slug: the two sheets are packed independently and merging
        // them puts 632 rooms of the real map on another room's cell.
        // Which area each room belongs to, so a plated room's place
        // travels alongside the grid it is drawn on.
        let area_of = |id: RoomId| -> Option<String> {
            for kind in [AreaKind::Official, AreaKind::Location] {
                if let Some(area) = self.areas.list(kind).iter().find(|a| a.rooms.contains(&id)) {
                    return Some(area.name.clone());
                }
            }
            None
        };
        // Drags, stated as offsets from a room that did not move. Each area
        // is solved on its own, so each is re-solved here to measure
        // against the same cells the person was looking at when they
        // dragged.
        let placements = self.placements_across_areas(map);
        // Pictures of the ticked areas, carried inside the file so the
        // whole submission is one attachment.
        let (pictures, picture_problems) = self.draw_ticked_areas(map);
        let (export, skipped) =
            export::build(&self.store, map, source, &area_of, &placements, pictures);
        if export.is_empty() {
            self.export_note = Some("Nothing to export yet.".to_owned());
            return;
        }
        let path = export::export_path(store_path);
        match export.save(&path) {
            Ok(()) => {
                let mut note = format!(
                    "Exported {} correction(s) to {}",
                    export.len(),
                    path.display()
                );
                if !export.pictures.is_empty() {
                    let _ = write!(note, ", with {} picture(s)", export.pictures.len());
                }
                if !skipped.is_empty() {
                    // Said out loud: a correction that cannot be named
                    // across a map rebuild did not travel, and a person
                    // should know rather than find out downstream.
                    let _ = write!(note, "; {} skipped ({})", skipped.len(), skipped[0].why);
                }
                if let Some(problem) = picture_problems.first() {
                    let _ = write!(
                        note,
                        "; {} area(s) not drawn ({problem})",
                        picture_problems.len()
                    );
                }
                self.export_note = Some(note);
            }
            Err(error) => {
                self.export_note = Some(format!("Cannot write {}: {error}", path.display()));
            }
        }
    }

    /// Save the store, turning a write failure into a message the window
    /// shows rather than a silent loss of the correction just made.
    fn save_store(&mut self) {
        let Some(path) = self.store_path.as_deref() else {
            self.store_problem = Some("No map path, so corrections cannot be saved.".to_owned());
            return;
        };
        match self.store.save(path) {
            Ok(()) => self.store_problem = None,
            Err(error) => {
                self.store_problem = Some(format!("Cannot save {}: {error}", path.display()));
            }
        }
    }

    /// Rebuild the area lists after a membership change, keeping the
    /// selected area selected by name where it still exists.
    fn rebuild_areas(&mut self) {
        let Ok(map) = &self.map else { return };
        let was = self
            .selected
            .and_then(|s| self.areas.list(s.kind).get(s.index))
            .map(|a| (self.tab, a.name.clone()));
        self.areas = Areas::build(map, &self.store);
        self.selected = was.and_then(|(kind, name)| {
            self.areas
                .list(kind)
                .iter()
                .position(|a| a.name == name)
                .map(|index| Selection { kind, index })
        });
        // Reached from an edit, so the camera stays where the person put
        // it -- see `refresh_shown`.
        self.refresh_shown();
    }

    /// Compute (or recompute) the layout and scene for the selected area.
    ///
    /// Cheap enough to redo on every selection change: measured against
    /// the real map, the worst area (Wehnimer's Landing, 3,229 rooms)
    /// takes ~102ms in release, and every other one far less.
    fn show_selected(&mut self) {
        self.rebuild_shown(Fit::Reset);
    }

    /// Redraw the shown area after an edit to it, **without moving the
    /// camera or dropping the selected room**.
    ///
    /// An edit changes the layout under a view a person is already
    /// looking at, often mid-gesture. Re-fitting there yanks the zoom and
    /// position away from what they were aiming at, and clearing the
    /// selection shuts the inspector, which resizes the canvas and reads
    /// as a flash. Switching areas is the only time either is wanted.
    fn refresh_shown(&mut self) {
        self.rebuild_shown(Fit::Keep);
    }

    /// A knob moved last frame: re-solve what is shown, keeping focus and
    /// selection, and frame the new sheet.
    fn apply_knobs(&mut self) {
        if self
            .shown
            .as_ref()
            .is_some_and(|s| s.layout.town_scale != self.params.town_scale.max(1))
        {
            self.rebuild_shown(Fit::Keep);
            if let Some(shown) = &mut self.shown {
                shown.needs_fit = true;
            }
        }
    }

    fn rebuild_shown(&mut self, fit: Fit) {
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
        // A room on a plate is drawn there, not here -- that is what a
        // plate is for. It stays in this area's *list*, because it is
        // still a room of this place; it just lays out elsewhere.
        // A plated room is drawn on its plate, not here; the rest of the
        // area lays out with whatever neighbours it needs to stay whole.
        let own: Vec<RoomId> = area
            .rooms
            .iter()
            .copied()
            .filter(|&id| {
                area.kind == AreaKind::Plates || !self.store.is_plated(RoomKey::of(id, map))
            })
            .collect();
        let rooms = areas::layout_rooms(&own, map, &self.areas.placeable);
        if rooms.is_empty() {
            self.shown = None;
            return;
        }
        let Ok(subset) = Map::from_rooms(rooms) else {
            // Two rooms sharing an id within one area cannot happen -- the
            // whole map already rejected duplicate ids on load -- but a
            // filtered subset never panics over it either way.
            self.shown = None;
            return;
        };
        let name = area.name.clone();
        let store_key = area.store_key();
        let majority = area.parent.clone().unwrap_or_default();
        let off_region: HashSet<RoomId> = if area.contested.is_some() {
            area.rooms
                .iter()
                .copied()
                .filter(|&id| {
                    self.store
                        .region_in(RoomKey::of(id, map), &self.areas.baseline)
                        .unwrap_or(areas::NO_REGION)
                        != majority
                })
                .collect()
        } else {
            HashSet::new()
        };
        let location = self.store.location(&store_key);
        // Edge corrections go IN to the solve: they change what the solver
        // does, so the rooms are placed by the corrected geometry. Moves
        // and pins come after, as a diff on its result.
        let edges = location
            .map(|l| l.edge_overrides(&subset))
            .unwrap_or_default();
        let mut layout = generate_layout_tuned(&subset, &edges, self.params);
        if let Some(location) = location {
            overrides::apply(&mut layout, &subset, location);
        }
        let scene = build_scene(&name, &layout, &subset);
        let mut focus = Focus::build(&scene, self.areas.list(AreaKind::Official));
        if fit == Fit::Keep
            && let Some(previous) = self.shown.as_ref().filter(|s| s.name == name)
        {
            focus.carry_over(&previous.focus);
        }
        if fit == Fit::Reset {
            // A new area's selection does not carry over: the room is not
            // in it, and a stale inspector panel would describe nothing
            // visible. The trail goes with it -- walking back into the
            // area just left would be worse than no button.
            self.inspected = None;
            self.trail.clear();
            // Nor does a picked set: a region and an official area share
            // rooms, and a selection made on one would otherwise follow
            // into the other half-visible.
            self.picked_rooms.clear();
        } else if self
            .inspected
            .is_some_and(|id| scene.room(id).is_none() && subset.room(id).is_none())
        {
            // Editing can take the inspected room off this area
            // entirely -- a plate move does exactly that -- and an
            // inspector describing a room that is neither drawn nor held
            // here is worse than none. A room still in the area but off
            // the shown sheet keeps its panel: it is a visit, which is
            // the case the trail exists for.
            self.inspected = self.trail.pop();
        }
        // The picked rooms survive a re-solve of the same area -- ids do
        // not move -- but not onto a sheet that does not draw them.
        self.picked_rooms.retain(|&id| scene.room(id).is_some());
        let off_region = off_region
            .into_iter()
            .filter(|&id| scene.room(id).is_some())
            .collect();
        self.shown = Some(Shown {
            name,
            focus,
            store_key,
            layout,
            subset,
            scene,
            majority,
            off_region,
            needs_fit: needs_fit_for(fit),
        });
    }

    /// A click on a room in focus: inspect it, or close the panel if it
    /// was the inspected one already. A click on a dot -- a room out of
    /// focus -- enters the unit it belongs to, a building or a hunting
    /// area or the streets, with the room inspected and in the middle of
    /// the view.
    fn clicked(&mut self, id: RoomId) {
        let Some(shown) = &mut self.shown else {
            return;
        };
        if shown.focus.enter(id) {
            if let Some(room) = shown.scene.room(id) {
                self.camera.center_on(room.cell);
            }
            self.inspected = Some(id);
            return;
        }
        self.inspected = (self.inspected != Some(id)).then_some(id);
    }

    /// The selection as the panel needs it: how many rooms, and every key
    /// to assign.
    fn picked(&self) -> (usize, Vec<RoomKey>) {
        let keys: Vec<RoomKey> = self.map.as_ref().ok().map_or_else(Vec::new, |whole| {
            let mut keys: Vec<RoomKey> = self
                .picked_rooms
                .iter()
                .map(|&id| RoomKey::of(id, whole))
                .collect();
            keys.sort_unstable();
            keys.dedup();
            keys
        });
        (self.picked_rooms.len(), keys)
    }

    /// What a click or a box on the canvas does: inspect, or pick.
    fn pointer(&mut self, hit: &draw::Hit) {
        if let Some(id) = hit.clicked {
            if hit.ctrl {
                self.toggle_room(id);
            } else {
                self.clicked(id);
            }
        }
        // A box ADDS: sweeping a second box over part of the first should
        // grow the selection, not punch holes in it. Ctrl-click is there
        // for taking single rooms back out.
        if let Some(&first) = hit.boxed.first() {
            self.seed_with_inspected(first);
            self.picked_rooms.extend(hit.boxed.iter().copied());
        }
    }

    /// The room already clicked joins a selection being started.
    ///
    /// Click one room, Ctrl-click a second: two rooms are picked, which is
    /// what everyone expects of Ctrl-click and what the first version did
    /// not do -- the plain click inspected a room and the Ctrl-click then
    /// started a selection without it.
    fn seed_with_inspected(&mut self, clicked: RoomId) {
        if self.picked_rooms.is_empty()
            && let Some(first) = self.inspected
            && first != clicked
            && self
                .shown
                .as_ref()
                .is_some_and(|s| s.scene.room(first).is_some())
        {
            self.picked_rooms.insert(first);
        }
    }

    /// Ctrl-click: pick this one room, or unpick it.
    ///
    /// Toggling rather than adding, because the mistake someone makes
    /// while picking fifteen rooms is clicking one twice, and having to
    /// start again would be worse than the tedium it replaces.
    fn toggle_room(&mut self, id: RoomId) {
        self.seed_with_inspected(id);
        if !self.picked_rooms.remove(&id) {
            self.picked_rooms.insert(id);
        }
    }

    /// Track a drag across frames and turn a finished one into an edit.
    ///
    /// The whole drag is one correction: pixels accumulate while the mouse
    /// is down and convert to cells once, on release, so a slow drag does
    /// not record a trail of one-cell nudges.
    fn handle_drag(&mut self, hit: &draw::Hit) -> Option<EditAction> {
        if let Some((id, alt)) = hit.drag_started {
            let picked = &self.picked_rooms;
            self.drag = self.shown.as_ref().and_then(|shown| {
                let group = shown.scene.room(id)?.group;
                // A drag that starts on a picked room carries the whole
                // pick; one that starts anywhere else carries the group,
                // as it always has. Alt means this one room either way.
                let moving = if alt {
                    Moving::Rooms(vec![id])
                } else if picked.contains(&id) {
                    Moving::Rooms(
                        picked
                            .iter()
                            .copied()
                            .filter(|&r| shown.scene.room(r).is_some())
                            .collect(),
                    )
                } else {
                    Moving::Group(group)
                };
                Some(DragState {
                    moving,
                    accumulated: egui::Vec2::ZERO,
                })
            });
        }
        if let (Some(delta), Some(drag)) = (hit.dragged_by, self.drag.as_mut()) {
            drag.accumulated += delta;
        }
        if !hit.drag_stopped {
            return None;
        }

        let drag = self.drag.take()?;
        let shown = self.shown.as_ref()?;
        match drag.moving {
            Moving::Group(index) => {
                let scale = shown.scene.scale_of(index);
                let delta = cells_dragged(drag.accumulated, self.camera, scale);
                if delta.x == 0 && delta.y == 0 {
                    return None;
                }
                let group = shown.layout.groups.get(index)?;
                Some(EditAction::NudgeGroup {
                    anchor: RoomKey::anchor(group, &shown.subset)?,
                    delta,
                })
            }
            // Each room is pinned at its position within its own group's
            // frame, which is its drawn cell less the group's offset. The
            // rooms of a pick can sit in different groups, at different
            // scales, so each converts the drag at its own.
            Moving::Rooms(ids) => {
                let pins: Vec<(RoomKey, Cell)> = ids
                    .into_iter()
                    .filter_map(|id| {
                        let room = shown.scene.room(id)?;
                        let scale = shown.scene.scale_of(room.group);
                        let delta = cells_dragged(drag.accumulated, self.camera, scale);
                        if delta.x == 0 && delta.y == 0 {
                            return None;
                        }
                        let offset = shown
                            .scene
                            .group_offsets
                            .get(&room.group)
                            .copied()
                            .unwrap_or_default();
                        // The drawn cell is the solver's times the group's
                        // scale; the pin is in the solver's.
                        let pin = Cell {
                            x: room.cell.x / scale - offset.x + delta.x,
                            y: room.cell.y / scale - offset.y + delta.y,
                        };
                        Some((RoomKey::of(id, &shown.subset), pin))
                    })
                    .collect();
                (!pins.is_empty()).then_some(EditAction::PinRooms { pins })
            }
        }
    }

    /// Apply an edit, save the store, and redraw whatever it changed.
    fn commit(&mut self, edit: EditAction) {
        // Corrections are keyed by the store key; plate ownership by the
        // area's name, which is what the header and the plate list show.
        let area = self.shown.as_ref().map(|shown| shown.store_key.clone());
        // A plate made while looking at a plate belongs to that plate's
        // own area, not to the plate: plates are sheets of an area, never
        // of each other, and a plate owning itself is a dead end with no
        // way back to the town it hangs off.
        let owning_area = self.shown.as_ref().map(|shown| {
            self.store
                .owner_of(&shown.name)
                .unwrap_or(&shown.name)
                .to_owned()
        });
        let mut membership_changed = false;
        match edit {
            EditAction::NudgeGroup { anchor, delta } => {
                let Some(area) = area else { return };
                self.store.nudge_group(&area, anchor, delta);
            }
            EditAction::PinRooms { pins } => {
                let Some(area) = area else { return };
                for (key, pin) in pins {
                    self.store.pin_room(&area, key, Some(pin));
                }
            }
            EditAction::UnpinRoom { key } => {
                let Some(area) = area else { return };
                self.store.pin_room(&area, key, None);
            }
            EditAction::ResetLocation => {
                let Some(area) = area else { return };
                self.store.reset_location(&area);
            }
            EditAction::DeletePlate { plate } => {
                self.store.delete_map(&plate);
                membership_changed = true;
            }
            EditAction::SetPlateArea { plate, area } => {
                self.store.set_plate_area(&plate, Some(&area));
            }
            EditAction::SetEdge { a, b, action } => {
                let Some(area) = area else { return };
                self.store.set_edge(&area, a, b, action);
            }
            EditAction::MoveRooms { keys, to } => {
                for key in keys {
                    self.store.move_room(key, to.as_deref());
                }
                membership_changed = true;
            }
            EditAction::NewPlate { name, keys } => {
                let plate = self.store.create_map(&name, owning_area.as_deref());
                for key in keys {
                    self.store.move_room(key, Some(&plate));
                }
                membership_changed = true;
            }
            EditAction::AssignRegion { keys, to } => {
                for key in keys {
                    self.store.set_region(key, to.as_deref());
                }
                membership_changed = true;
            }
            EditAction::AssignArea { keys, to } => {
                for key in keys {
                    self.store.set_area(key, to.as_deref());
                }
                membership_changed = true;
            }
            EditAction::NewArea { name, keys } => {
                let area = self.store.create_area(&name);
                for key in keys {
                    self.store.set_area(key, Some(&area));
                }
                membership_changed = true;
            }
            EditAction::DeleteArea { area } => {
                let baked: Vec<RoomKey> = self
                    .areas
                    .baseline
                    .area
                    .iter()
                    .filter(|(_, a)| **a == area)
                    .map(|(k, _)| *k)
                    .collect();
                self.store.delete_area(&area, baked);
                membership_changed = true;
            }
            EditAction::MapAreas { areas, map } => {
                for area in areas {
                    self.store.set_area_map(&area, map.as_deref());
                }
                membership_changed = true;
            }
        }
        self.save_store();
        if membership_changed {
            // A moved room changes which areas exist and what is in them,
            // so the lists are rebuilt, not just the shown layout.
            self.rebuild_areas();
        } else {
            self.refresh_shown();
        }
    }

    /// The inspector when there is a selection but no inspected room.
    fn selection_only_panel(&mut self, ui: &mut egui::Ui, can_edit: bool) -> Option<EditAction> {
        let (picked_rooms, picked_keys) = self.picked();
        let mut edit = None;
        let mut clear_selection = false;
        let whole = self.map.as_ref().ok()?;
        let store = &self.store;
        let baseline = &self.areas.baseline;
        let mut boxes = PlaceBoxes {
            region: &mut self.new_region,
            area: &mut self.new_area,
        };
        let new_plate = &mut self.new_plate;
        let edit_out = &mut edit;
        egui::Panel::right("inspector").show(ui, |ui| {
            ui.heading("Selection");
            if !can_edit {
                ui.weak("Editing is off.");
                return;
            }
            if let Some(action) = selection_panel(
                ui,
                store,
                &Picked {
                    rooms: picked_rooms,
                    keys: &picked_keys,
                },
                new_plate,
                &mut clear_selection,
            )
            .or_else(|| place_panel(ui, store, baseline, whole, None, &picked_keys, &mut boxes))
            {
                *edit_out = Some(action);
            }
        });
        if clear_selection || edit.is_some() {
            self.picked_rooms.clear();
        }
        edit
    }

    /// What the back button returns to, named rather than numbered: "back
    /// to [Town Square Central]" says where it goes.
    fn previous_title(&self) -> Option<String> {
        self.trail.last().and_then(|&id| {
            self.map
                .as_ref()
                .ok()
                .and_then(|m| m.room(id))
                .and_then(|r| r.title.first().cloned())
                .or_else(|| Some(format!("room {}", id.0)))
        })
    }

    /// The inspector's closing, following an exit, or going back.
    fn after_inspector(&mut self, open: bool, follow: Option<RoomId>, back: bool) {
        if !open {
            self.inspected = None;
            self.trail.clear();
        } else if let Some(next) = follow {
            self.walk_to(next);
        } else if back {
            self.inspected = self.trail.pop();
        }
    }

    /// The inspector panel, drawn before the central panel so egui gives
    /// the canvas whatever space is left.
    fn inspector_panel(&mut self, ui: &mut egui::Ui, can_edit: bool) -> Option<EditAction> {
        // A selection with nothing inspected still needs somewhere to
        // act. Picking fifteen rooms and finding no panel -- because the
        // last click was a Ctrl-click, which selects rather than
        // inspects -- is the state this avoids.
        if self.inspected.is_none() && !self.picked_rooms.is_empty() {
            return self.selection_only_panel(ui, can_edit);
        }
        let (Some(shown), Some(id)) = (&self.shown, self.inspected) else {
            return None;
        };
        // Gathered before the mutable borrows below: the panel closure
        // holds `&mut self.new_area`, so nothing can call a `&self`
        // method after that point.
        let (picked_rooms, picked_keys) = self.picked();
        let previous = self.previous_title();
        let mut clear_selection = false;

        let mut edit = None;
        let edit_out = &mut edit;
        let mut open = true;
        let store = &self.store;
        let baseline = &self.areas.baseline;
        let new_plate = &mut self.new_plate;
        let mut boxes = PlaceBoxes {
            region: &mut self.new_region,
            area: &mut self.new_area,
        };
        let edit_mode = self.view.edit_mode && can_edit;

        let whole = self.map.as_ref().ok();
        let visiting = shown.subset.room(id).is_none();
        let mut follow = None;
        let mut back = false;

        egui::Panel::right("inspector").show(ui, |ui| {
            inspector_header(ui, previous.as_deref(), &mut open, &mut back);
            if visiting {
                // A room reached through an exit out of this area. It is
                // not laid out here, so there are no layout diagnostics
                // to show -- but it can still be put on one of this
                // area's plates, which is the whole point of following
                // the exit.
                let Some(whole) = whole else { return };
                visiting_room(ui, whole, id, &mut follow);
                if edit_mode
                    && let Some(action) =
                        visiting_membership(ui, shown, store, whole, id, new_plate).or_else(|| {
                            place_panel(
                                ui,
                                store,
                                baseline,
                                whole,
                                Some(id),
                                &picked_keys,
                                &mut boxes,
                            )
                        })
                {
                    *edit_out = Some(action);
                }
                return;
            }
            inspector(
                ui,
                shown,
                id,
                whole,
                store.location(&shown.store_key),
                &mut follow,
            );
            if !edit_mode {
                return;
            }
            // The selection acts on many rooms at once, so it comes
            // before the controls for the one room being inspected.
            if let Some(action) = selection_panel(
                ui,
                store,
                &Picked {
                    rooms: picked_rooms,
                    keys: &picked_keys,
                },
                new_plate,
                &mut clear_selection,
            ) {
                *edit_out = Some(action);
            }
            // Editing controls live below the facts, so the panel
            // reads the same whether or not Edit is on.
            ui.separator();
            if let Some(action) = room_edits(ui, shown, store, id) {
                *edit_out = Some(action);
            }
            if let Some(action) = whole.and_then(|w| {
                place_panel(ui, store, baseline, w, Some(id), &picked_keys, &mut boxes)
            }) {
                *edit_out = Some(action);
            }
            if let Some(action) = membership(ui, shown, store, id, new_plate) {
                *edit_out = Some(action);
            }
        });
        self.after_inspector(open, follow, back);
        // An assignment consumes the selection -- leaving it picked
        // invites assigning the same rooms twice -- but only
        // one made FROM the selection. Unpinning a room or fixing an
        // edge is unrelated and must not throw the picking away.
        let acted_on_selection = !picked_keys.is_empty()
            && match &edit {
                Some(
                    EditAction::AssignArea { keys, .. }
                    | EditAction::AssignRegion { keys, .. }
                    | EditAction::NewArea { keys, .. }
                    | EditAction::MoveRooms { keys, .. }
                    | EditAction::NewPlate { keys, .. },
                ) => *keys == picked_keys,
                _ => false,
            };
        if clear_selection || acted_on_selection {
            self.picked_rooms.clear();
        }
        edit
    }

    /// Under the Maps tab: the areas worth putting on one map
    /// ([`crate::maps::suggest`]), each with Accept, which puts them on it.
    fn map_suggestions(&self, ui: &mut egui::Ui) -> Option<EditAction> {
        let map = self.map.as_ref().ok()?;
        let offered = crate::maps::suggest(map, &self.store, &self.areas.baseline);
        if offered.is_empty() {
            return None;
        }
        ui.separator();
        ui.strong(format!("Suggested ({})", offered.len()))
            .on_hover_text("Areas a walk joins whose rooms mostly name one location");
        let mut edit = None;
        egui::ScrollArea::vertical()
            .id_salt("map_suggestions")
            .max_height(240.0)
            .show(ui, |ui| {
                for suggestion in offered {
                    let titles: Vec<&str> = suggestion
                        .areas
                        .iter()
                        .map(|a| self.store.area_title(a, &self.areas.baseline))
                        .collect();
                    ui.horizontal_top(|ui| {
                        if ui.button("Accept").clicked() {
                            edit = Some(EditAction::MapAreas {
                                areas: suggestion.areas.clone(),
                                map: Some(suggestion.name.clone()),
                            });
                        }
                        ui.add(
                            egui::Label::new(format!("{}: {}", suggestion.name, titles.join(", ")))
                                .wrap(),
                        );
                    });
                }
            });
        edit
    }

    /// The left panel: the two list tabs, a filter box, and the list.
    fn picker(&mut self, ui: &mut egui::Ui, picking: bool) -> bool {
        let mut changed = false;
        // Set when a room-number search is followed: the area to show,
        // and the room to inspect once it is on screen.
        let mut goto: Option<(AreaKind, usize, RoomId)> = None;

        ui.heading("Areas");
        ui.horizontal(|ui| {
            for kind in AreaKind::ALL {
                let label = format!("{} ({})", kind.title(), self.areas.list(kind).len());
                if ui.selectable_label(self.tab == kind, label).clicked() {
                    self.tab = kind;
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter, or a room number"));
        // A bare number is a room, not a name: with 356 areas there is
        // otherwise no way to answer "which area holds room 7562", and
        // that is exactly the question an exit leading out of an area
        // provokes.
        if let Some(found) = self.filter.trim().parse::<u32>().ok().map(RoomId) {
            goto = room_search(ui, &self.areas, found);
        }
        ui.separator();

        let needle = self.filter.to_lowercase();
        // The Region tab is a tree: regions at the top, the curated areas
        // whose rooms carry that region nested under them. Every other
        // tab is a flat list, so rows are ordered here rather than in
        // `Areas`, which has no opinion about presentation.
        //
        // A collapsed region hides its areas but is itself always shown,
        // because a region with no areas yet is exactly the one someone
        // needs to click on to start curating it.
        let order = row_order(self.areas.list(self.tab), self.tab, &self.collapsed);
        let mut toggle: Option<String> = None;
        let rows = self.areas.list(self.tab);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for index in order {
                let area = &rows[index];
                // A filter matching a region keeps its areas, and one
                // matching an area keeps it visible under its region:
                // hiding the parent would leave the child unplaceable.
                if !needle.is_empty()
                    && !area.name.to_lowercase().contains(&needle)
                    && !area
                        .parent
                        .as_deref()
                        .is_some_and(|p| p.to_lowercase().contains(&needle))
                {
                    continue;
                }
                let selection = Selection {
                    kind: self.tab,
                    index,
                };
                let selected = self.selected == Some(selection);
                let label = area_label(area, self.tab, self.map.as_ref().ok(), &self.store);
                // The tick marks an area for the SVG export, and appears
                // only while picking: a row that always carries one reads
                // as a mode the window is stuck in.
                //
                // `horizontal_top` with a truncating label, rather than
                // plain `horizontal`: a horizontal layout asks for its
                // content's full width, which would stop the panel ever
                // being dragged narrower than the longest area name.
                ui.horizontal_top(|ui| {
                    if tree_handle(ui, area, self.tab, rows, &self.collapsed) {
                        toggle = Some(area.name.clone());
                    }
                    if picking {
                        // Scoped by area name: every one of these
                        // checkboxes has an empty label, and egui derives
                        // a widget's id from its text and position, so
                        // without this they all share one id and only a
                        // single tick can be held across the whole list.
                        ui.push_id(&area.name, |ui| {
                            let mut ticked = self.svg_areas.contains(&area.name);
                            if ui
                                .checkbox(&mut ticked, "")
                                .on_hover_text("Draw this area and its plates as SVGs")
                                .changed()
                            {
                                if ticked {
                                    self.svg_areas.insert(area.name.clone());
                                } else {
                                    self.svg_areas.remove(&area.name);
                                }
                            }
                        });
                    }
                    if ui
                        .add(
                            egui::Button::selectable(
                                selected,
                                // A spacer that eats the leftover width,
                                // which pushes the label to the left
                                // edge. The button centres its content
                                // otherwise, and a centred name hides its
                                // own indentation -- the only thing
                                // saying an area sits under a region.
                                (row_text(area, self.tab, &label), egui::Atom::grow()),
                            )
                            .truncate()
                            .min_size(egui::vec2(ui.available_width(), 0.0)),
                        )
                        .on_hover_text(&label)
                        .clicked()
                    {
                        self.selected = Some(selection);
                        changed = true;
                    }
                });
            }
        });
        if self.tab == AreaKind::Maps
            && let Some(edit) = self.map_suggestions(ui)
        {
            self.commit(edit);
            changed = true;
        }
        if let Some(name) = toggle
            && !self.collapsed.remove(&name)
        {
            self.collapsed.insert(name);
        }
        if let Some((kind, index, room)) = goto {
            self.tab = kind;
            self.selected = Some(Selection { kind, index });
            self.pending_inspect = Some(room);
            changed = true;
        }
        changed
    }
}

/// The inspector panel: everything known about one room, from all three
/// sources -- the room record, its exits, and the layout that placed it.
///
/// A free function rather than a method so it borrows only the `Shown` it
/// reads, leaving the rest of the app free for the canvas beside it.
fn inspector(
    ui: &mut egui::Ui,
    shown: &Shown,
    id: RoomId,
    whole: Option<&Map>,
    location: Option<&crate::overrides::LocationOverrides>,
    follow: &mut Option<RoomId>,
) {
    let Some(facts) = RoomFacts::gather_in(id, &shown.subset, &shown.layout, whole) else {
        ui.label(format!("Room {} is not in this area.", id.0));
        return;
    };

    ui.heading(facts.heading());
    ui.label(format!("Room {}", facts.id.0));
    if !facts.uids.is_empty() {
        let uids: Vec<String> = facts.uids.iter().map(ToString::to_string).collect();
        // A room has many uids when it is instanced, and none for a fifth
        // of the map; both are normal and worth showing plainly.
        ui.label(format!("uid {}", uids.join(", ")));
    }
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        // --- what the game says this place is ---
        for (label, value) in [
            ("Location", facts.location.as_deref()),
            ("Terrain", facts.terrain.as_deref()),
            ("Climate", facts.climate.as_deref()),
        ] {
            if let Some(value) = value {
                ui.label(format!("{label}: {value}"));
            }
        }
        if !facts.tags.is_empty() {
            // Forage names make a room's tag list run to a hundred entries,
            // which pushed its exits and layout off the panel. Always
            // foldable; shut to start with once it is long.
            let count = facts.tags.len();
            egui::CollapsingHeader::new(format!("Tags ({count})"))
                .id_salt("tags_collapsed")
                .default_open(count <= TAGS_BEFORE_COLLAPSING)
                .show(ui, |ui| {
                    ui.label(facts.tags.join(", "));
                });
        }
        if facts.entrance {
            ui.colored_label(ENTRANCE_COLOR, "Hosts a doorway into an interior");
        }

        if facts.titles.len() > 1 {
            ui.add_space(4.0);
            ui.label("Other titles:");
            for title in facts.titles.iter().skip(1) {
                ui.small(title);
            }
        }
        if let Some(description) = &facts.description {
            ui.add_space(4.0);
            ui.small(description);
        }
        if let Some(paths) = &facts.paths {
            ui.add_space(4.0);
            ui.small(paths);
        }

        // --- exits ---
        ui.add_space(8.0);
        exit_list(ui, &facts, follow);

        // --- how the layout placed it ---
        ui.add_space(8.0);
        ui.strong("Layout");
        ui.label(format!("Cell: {}, {}", facts.cell.x, facts.cell.y));
        ui.label(match facts.group_name.as_deref() {
            Some(name) => format!("Group {} ({name})", facts.group),
            None => format!("Group {}", facts.group),
        });
        if let Some(packing) = facts.packing {
            ui.label(format!("Packed by: {packing:?}"));
        }

        // What a drag on this room will travel as. Shown so the arithmetic
        // can be checked by eye here, rather than trusted blind until
        // something downstream reads the export.
        if let Some(location) = location
            && let Some(placed) = placement::resolve(&shown.layout, &shown.subset, location)
                .into_iter()
                .find(|p| p.room == id)
        {
            ui.add_space(4.0);
            ui.label(format!(
                "Exports as: {} from room {}",
                offset_phrase(placed.dx, placed.dy),
                placed.anchor.0
            ));
        }

        if facts.violations.is_empty() {
            return;
        }
        // The reason the layout half of this panel exists: 955 of these
        // across the real map, and until now nothing showed them.
        ui.add_space(4.0);
        ui.colored_label(
            VIOLATION_COLOR,
            format!("{} direction violation(s)", facts.violations.len()),
        );
        for violation in &facts.violations {
            ui.small(format!(
                "exit says {}, room {} actually sits {}",
                violation.stated,
                violation.other.0,
                bearing(violation.dx, violation.dy),
            ));
        }
    });
}

/// A room with more exits than this gets a collapsing header instead of
/// the list, shut by default.
///
/// The Elemental Confluence is why. Its rooms carry up to 359 exits --
/// nine instances merged, every one of them a scripted hop to another
/// town -- and rendering them inline pushed the Area and Plate panels
/// off the bottom of the inspector, so the rooms that most need
/// curating were the ones that could not be assigned.
///
/// Twelve, because a busy town square is nine or ten and should still
/// read at a glance. Nothing is hidden: the header says how many there
/// are and opens to the same list.
const EXITS_BEFORE_COLLAPSING: usize = 12;

/// Tags past this many start folded: a room's forage names alone can run
/// to a hundred.
const TAGS_BEFORE_COLLAPSING: usize = 8;

/// The exit list, with rooms outside this area as links to follow.
fn exit_list(ui: &mut egui::Ui, facts: &RoomFacts, follow: &mut Option<RoomId>) {
    let count = facts.exits.len();
    if count > EXITS_BEFORE_COLLAPSING {
        egui::CollapsingHeader::new(format!("Exits ({count})"))
            .id_salt("exits_collapsed")
            .default_open(false)
            .show(ui, |ui| exit_rows(ui, facts, follow));
        return;
    }
    ui.strong(format!("Exits ({count})"));
    exit_rows(ui, facts, follow);
}

fn exit_rows(ui: &mut egui::Ui, facts: &RoomFacts, follow: &mut Option<RoomId>) {
    for exit in &facts.exits {
        ui.horizontal_wrapped(|ui| {
            match &exit.command {
                Some(command) => ui.label(command),
                None => ui.label("-"),
            };
            let label = match &exit.to_title {
                Some(title) => format!("-> {title}"),
                None => format!("-> room {}", exit.to.0),
            };
            if exit.outside {
                // Clickable: an official area excludes its own
                // interiors, so the only way to reach the well behind
                // a town square is through the exit that names it.
                if ui
                    .link(label)
                    .on_hover_text("Outside this area -- click to inspect it")
                    .clicked()
                {
                    *follow = Some(exit.to);
                }
            } else {
                ui.weak(label);
            }
            if exit.crossed != Crossed::Command {
                let color = if exit.crossed == Crossed::Impassable {
                    IMPASSABLE_COLOR
                } else {
                    SCRIPTED_COLOR
                };
                ui.colored_label(color, exit.crossed.label());
            }
        });
    }
}

/// A room reached by following an exit out of the shown area.
///
/// It is not laid out here, so this shows what the map knows about it and
/// its own exits -- enough to tell a well from a treehouse and to walk on
/// to the next room.
fn visiting_room(ui: &mut egui::Ui, whole: &Map, id: RoomId, follow: &mut Option<RoomId>) {
    let Some(room) = whole.room(id) else {
        ui.label(format!("Room {} is not in this map.", id.0));
        return;
    };
    ui.heading(
        room.title
            .first()
            .cloned()
            .unwrap_or_else(|| format!("Room {}", id.0)),
    );
    ui.label(format!("Room {}", id.0));
    ui.weak("Outside the area on screen");
    ui.separator();
    if let Some(location) = &room.location {
        ui.label(format!("Location: {location}"));
    }
    if let Some(description) = room.description.first() {
        ui.add_space(4.0);
        ui.small(description);
    }
    ui.add_space(8.0);
    ui.strong(format!("Exits ({})", room.exits.len()));
    for exit in &room.exits {
        ui.horizontal_wrapped(|ui| {
            match &exit.crossing {
                cena_map::Crossing::Command(cmd) => ui.label(cmd),
                _ => ui.label("-"),
            };
            let title = whole
                .room(exit.to)
                .and_then(|r| r.title.first())
                .cloned()
                .unwrap_or_else(|| format!("room {}", exit.to.0));
            if ui.link(format!("-> {title}")).clicked() {
                *follow = Some(exit.to);
            }
        });
    }
}

/// Putting a visited room -- one outside the shown area -- onto one of
/// that area's plates.
///
/// This is the case the editor could not reach before: an official area
/// excludes its own interiors, so the well and treehouse behind a town
/// square were unselectable and therefore unplateable.
fn visiting_membership(
    ui: &mut egui::Ui,
    shown: &Shown,
    store: &MapOverrides,
    whole: &Map,
    id: RoomId,
    new_plate: &mut String,
) -> Option<EditAction> {
    let mut edit = None;
    let key = RoomKey::of(id, whole);

    ui.add_space(8.0);
    ui.strong("Plate");
    if let Some(plate) = store.membership_moves.get(&key) {
        ui.label(format!("On: {}", store.plate_name(plate)));
        if ui.button("Send back").clicked() {
            edit = Some(EditAction::MoveRooms {
                keys: vec![key],
                to: None,
            });
        }
    }

    let plates = store.plates_of(&shown.name);
    if !plates.is_empty() {
        egui::ComboBox::from_id_salt("visit_move_to_plate")
            .selected_text(format!("Add to a {} plate...", shown.name))
            .show_ui(ui, |ui| {
                for (plate, name) in plates {
                    if store.membership_moves.get(&key).map(String::as_str) == Some(plate) {
                        continue;
                    }
                    if ui.selectable_label(false, name).clicked() {
                        edit = Some(EditAction::MoveRooms {
                            keys: vec![key],
                            to: Some(plate.to_owned()),
                        });
                    }
                }
            });
    }

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(new_plate)
                .hint_text("new plate name")
                .desired_width(120.0),
        );
        let named = !new_plate.trim().is_empty();
        if ui
            .add_enabled(named, egui::Button::new("+ room"))
            .on_hover_text("Make this plate under the shown area and move this room onto it")
            .clicked()
        {
            edit = Some(EditAction::NewPlate {
                name: new_plate.trim().to_owned(),
                keys: vec![key],
            });
            new_plate.clear();
        }
    });
    edit
}

/// What the selection covers.
#[derive(Clone, Copy)]
struct Picked<'a> {
    rooms: usize,
    keys: &'a [RoomKey],
}

/// The selection: what is picked, and what to do with it.
///
/// **Why this exists.** The per-room controls offer "- room" and
/// "- group", and on a town sheet the group is the whole outdoor
/// component -- 246 rooms on Mist Harbor -- so there was nothing between
/// one room and all of them. Pick exactly the rooms meant, act once.
///
/// Shown only when something is picked, so the ordinary inspector is
/// unchanged for anyone not doing a bulk pass.
fn selection_panel(
    ui: &mut egui::Ui,
    store: &MapOverrides,
    picked: &Picked<'_>,
    new_plate: &mut String,
    clear: &mut bool,
) -> Option<EditAction> {
    let Picked { rooms, keys } = *picked;
    if rooms == 0 {
        return None;
    }
    let mut edit = None;
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(format!("Selected: {rooms} room(s)"));
        if ui.button("Clear").clicked() {
            *clear = true;
        }
    });
    ui.weak("Ctrl-click: a room.  Ctrl-drag: a box.");
    let on_a_plate = keys
        .iter()
        .filter(|k| store.membership_moves.contains_key(k))
        .count();
    if on_a_plate > 0
        && ui
            .button("- from plate")
            .on_hover_text(format!(
                "Take {on_a_plate} selected room(s) off their plate"
            ))
            .clicked()
    {
        edit = Some(EditAction::MoveRooms {
            keys: keys.to_vec(),
            to: None,
        });
    }
    if let Some(action) = selection_plate(ui, store, rooms, keys, new_plate) {
        edit = Some(action);
    }
    edit
}

/// Moving a picked set onto a plate, or onto a new one.
///
/// A plate is the other place a selection wants to go: carving a town's
/// satellites off its sheet is the same act as assigning them, done
/// with the same picking.
fn selection_plate(
    ui: &mut egui::Ui,
    store: &MapOverrides,
    rooms: usize,
    keys: &[RoomKey],
    new_plate: &mut String,
) -> Option<EditAction> {
    let mut edit = None;
    if !store.custom_maps.is_empty() {
        let mut chosen: Option<String> = None;
        egui::ComboBox::from_id_salt("assign_selection_plate")
            .selected_text("Move all onto a plate...")
            .show_ui(ui, |ui| {
                for plate in store.custom_maps.keys() {
                    if ui
                        .selectable_label(
                            false,
                            format!("{} ({rooms} rooms)", store.plate_name(plate)),
                        )
                        .clicked()
                    {
                        chosen = Some(plate.clone());
                    }
                }
            });
        if let Some(plate) = chosen {
            edit = Some(EditAction::MoveRooms {
                keys: keys.to_vec(),
                to: Some(plate),
            });
        }
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(new_plate)
                .hint_text("new plate name")
                .desired_width(120.0),
        );
        if ui
            .add_enabled(!new_plate.trim().is_empty(), egui::Button::new("+ all"))
            .on_hover_text(format!(
                "Make this plate and move all {rooms} rooms onto it"
            ))
            .clicked()
        {
            edit = Some(EditAction::NewPlate {
                name: new_plate.trim().to_owned(),
                keys: keys.to_vec(),
            });
            new_plate.clear();
        }
    });

    edit
}

/// The two text boxes the place panel fills from its dropdowns.
struct PlaceBoxes<'a> {
    region: &'a mut String,
    area: &'a mut String,
}

/// Which region and which area: for the inspected room, for the
/// selection, or both.
///
/// **Pick, then apply.** Each dropdown only fills its box; the + buttons
/// apply what the box says, to this room or to every selected room. So
/// the same two buttons serve one room and five hundred, and a stray
/// click in a dropdown changes nothing.
///
/// A region is **chosen**, never minted: the thirteen are decided in
/// `curation/regions.toml`, so the box must hold one of them before +
/// will apply it. An area is minted here on purpose -- deciding the areas
/// is the work -- so a new name makes a new area.
///
/// "- " takes the room back out: out of its assigned region (back to what
/// the map says), or out of its area altogether.
fn place_panel(
    ui: &mut egui::Ui,
    store: &MapOverrides,
    baseline: &Baseline,
    whole: &Map,
    room: Option<RoomId>,
    selected: &[RoomKey],
    boxes: &mut PlaceBoxes<'_>,
) -> Option<EditAction> {
    let key = room.map(|id| RoomKey::of(id, whole));
    let mut edit = None;

    ui.separator();
    ui.strong("Region");
    if let (Some(id), Some(key)) = (room, key) {
        let inferred = whole
            .room(id)
            .is_some_and(|r| r.meta.iter().any(|m| m == "map:region-inferred"));
        match (store.region_of(key), baseline.region.get(&key)) {
            (Some(a), _) => ui.label(format!("{a} (assigned)")),
            (None, Some(d)) if inferred => ui.label(format!("{d} (inferred)")),
            (None, Some(d)) => ui.label(d.as_str()),
            (None, None) => ui.weak("no region"),
        };
    }
    let mut regions: Vec<&str> = baseline.regions.iter().map(String::as_str).collect();
    regions.extend(store.region_moves.values().map(String::as_str));
    regions.sort_unstable();
    regions.dedup();
    picker(ui, "region_pick", &regions, boxes.region);
    let region = boxes.region.trim().to_owned();
    let known = regions.contains(&region.as_str());
    let assigned = |k: &RoomKey| store.region_of(*k).is_some();
    if let Some(keys) = apply_buttons(
        ui,
        "region",
        known,
        key.is_some_and(|k| assigned(&k)),
        selected.iter().filter(|k| assigned(k)).count(),
        key,
        selected,
    ) {
        edit = Some(match keys {
            Apply::Add(keys) => EditAction::AssignRegion {
                keys,
                to: Some(region.clone()),
            },
            Apply::Remove(keys) => EditAction::AssignRegion { keys, to: None },
        });
    }
    if !region.is_empty() && !known {
        ui.weak("not a region -- pick one from the list");
    }

    ui.add_space(8.0);
    ui.strong("Area");
    if let Some(key) = key {
        match store.area_of(key, baseline) {
            Some(area) => ui.label(format!("In: {}", store.area_title(area, baseline))),
            None => ui.label("In: no area yet"),
        };
    }
    let areas = store.known_areas(baseline);
    let names: Vec<&str> = areas.values().map(String::as_str).collect();
    picker(ui, "area_pick", &names, boxes.area);
    let name = boxes.area.trim().to_owned();
    let in_area = |k: &RoomKey| store.area_of(*k, baseline).is_some();
    if let Some(keys) = apply_buttons(
        ui,
        "area",
        !name.is_empty(),
        key.is_some_and(|k| in_area(&k)),
        selected.iter().filter(|k| in_area(k)).count(),
        key,
        selected,
    ) {
        edit = Some(match keys {
            Apply::Add(keys) => match areas.iter().find(|(_, n)| **n == name) {
                Some((area, _)) => EditAction::AssignArea {
                    keys,
                    to: Some(area.clone()),
                },
                None => EditAction::NewArea {
                    name: name.clone(),
                    keys,
                },
            },
            Apply::Remove(keys) => EditAction::AssignArea { keys, to: None },
        });
    }
    if !name.is_empty() && !areas.values().any(|n| *n == name) {
        ui.weak("new area -- + makes it");
    }
    edit
}

/// A dropdown that fills `text` and a box showing it. Choosing from the
/// list applies nothing.
fn picker(ui: &mut egui::Ui, id: &str, choices: &[&str], text: &mut String) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text("\u{25be}")
            .width(24.0)
            .show_ui(ui, |ui| {
                for choice in choices {
                    if ui.selectable_label(text == choice, *choice).clicked() {
                        (*choice).clone_into(text);
                    }
                }
            });
        ui.add(egui::TextEdit::singleline(text).desired_width(170.0));
    });
}

/// What an apply row asked for.
enum Apply {
    Add(Vec<RoomKey>),
    Remove(Vec<RoomKey>),
}

/// "+ room  + selected (N)" and "- room  - selected (N)": the same four
/// buttons for region and area. `ready` is whether the box holds
/// something to apply; `room_has` and `selected_have` say whether there
/// is anything to take out.
fn apply_buttons(
    ui: &mut egui::Ui,
    what: &str,
    ready: bool,
    room_has: bool,
    selected_have: usize,
    room: Option<RoomKey>,
    selected: &[RoomKey],
) -> Option<Apply> {
    let mut out = None;
    let n = selected.len();
    ui.horizontal(|ui| {
        if let Some(key) = room
            && ui
                .add_enabled(ready, egui::Button::new("+ room"))
                .on_hover_text(format!("Put this room in the {what} in the box"))
                .clicked()
        {
            out = Some(Apply::Add(vec![key]));
        }
        if n > 0
            && ui
                .add_enabled(ready, egui::Button::new(format!("+ selected ({n})")))
                .on_hover_text(format!(
                    "Put all {n} selected rooms in the {what} in the box"
                ))
                .clicked()
        {
            out = Some(Apply::Add(selected.to_vec()));
        }
    });
    if room_has || selected_have > 0 {
        ui.horizontal(|ui| {
            if let Some(key) = room
                && ui
                    .add_enabled(room_has, egui::Button::new("- room"))
                    .on_hover_text(format!("Take this room out of its {what}"))
                    .clicked()
            {
                out = Some(Apply::Remove(vec![key]));
            }
            if n > 0
                && ui
                    .add_enabled(
                        selected_have > 0,
                        egui::Button::new(format!("- selected ({selected_have})")),
                    )
                    .on_hover_text(format!(
                        "Take the {selected_have} selected rooms that have one out of their {what}"
                    ))
                    .clicked()
            {
                out = Some(Apply::Remove(selected.to_vec()));
            }
        });
    }
    out
}

/// The ten compass bearings a person can force, in the order the combo
/// lists them.
const BEARINGS: [Dir; 10] = [
    Dir::North,
    Dir::Northeast,
    Dir::East,
    Dir::Southeast,
    Dir::South,
    Dir::Southwest,
    Dir::West,
    Dir::Northwest,
    Dir::Up,
    Dir::Down,
];

/// How an edge correction reads in the combo.
fn edge_label(action: Option<EdgeAction>) -> String {
    match action {
        None => "auto".to_owned(),
        Some(EdgeAction::Connector) => "passage".to_owned(),
        Some(EdgeAction::Direction(dir)) => dir.name().to_owned(),
    }
}

/// Per-exit edge corrections: the controls that fix a wrong layout rather
/// than tidy a drawn one.
///
/// Both actions here are inputs to the solve, so choosing one re-runs the
/// layout and the rooms move. `passage` un-welds two rooms the solver
/// placed adjacent on bad data; a bearing forces what the exit should have
/// said -- the answer to a direction violation.
fn edges_editor(
    ui: &mut egui::Ui,
    shown: &Shown,
    store: &MapOverrides,
    facts: &RoomFacts,
) -> Option<EditAction> {
    let mut edit = None;
    let here = RoomKey::of(facts.id, &shown.subset);
    let saved = store.location(&shown.store_key);

    ui.add_space(8.0);
    // Collapsed past the same length as the exit list, for the same
    // reason: an Elemental Confluence room has sixty-odd edges, and
    // listing them inline put the Area and Plate controls out of reach.
    // The header says how many carry a correction, so collapsing it
    // cannot hide that anything was changed.
    let editable = facts.exits.iter().filter(|e| e.to_title.is_some()).count();
    if editable > EXITS_BEFORE_COLLAPSING {
        let corrected = facts
            .exits
            .iter()
            .filter(|e| e.to_title.is_some())
            .filter(|e| {
                saved.is_some_and(|l| {
                    l.edge_action(here, RoomKey::of(e.to, &shown.subset))
                        .is_some()
                })
            })
            .count();
        let title = if corrected > 0 {
            format!("Edges ({editable}, {corrected} corrected)")
        } else {
            format!("Edges ({editable})")
        };
        egui::CollapsingHeader::new(title)
            .id_salt("edges_collapsed")
            .default_open(false)
            .show(ui, |ui| edge_rows(ui, shown, facts, here, saved, &mut edit));
    } else {
        ui.strong("Edges");
        edge_rows(ui, shown, facts, here, saved, &mut edit);
    }
    edit
}

/// One row per editable edge: the command, and the correction combo.
fn edge_rows(
    ui: &mut egui::Ui,
    shown: &Shown,
    facts: &RoomFacts,
    here: RoomKey,
    saved: Option<&crate::overrides::LocationOverrides>,
    edit: &mut Option<EditAction>,
) {
    for exit in &facts.exits {
        // An exit leading out of this area cannot be corrected here: the
        // solver never saw the far room, so constraining it would mean
        // nothing.
        let Some(title) = &exit.to_title else {
            continue;
        };
        let there = RoomKey::of(exit.to, &shown.subset);
        let current = saved.and_then(|l| l.edge_action(here, there));
        ui.horizontal(|ui| {
            ui.label(exit.command.as_deref().unwrap_or("-"));
            egui::ComboBox::from_id_salt(("edge", exit.to.0))
                .selected_text(edge_label(current))
                .width(110.0)
                .show_ui(ui, |ui| {
                    let mut choose = |ui: &mut egui::Ui, action: Option<EdgeAction>| {
                        if ui
                            .selectable_label(current == action, edge_label(action))
                            .clicked()
                        {
                            *edit = Some(EditAction::SetEdge {
                                a: here,
                                b: there,
                                action,
                            });
                        }
                    };
                    choose(ui, None);
                    choose(ui, Some(EdgeAction::Connector));
                    ui.separator();
                    for dir in BEARINGS {
                        choose(ui, Some(EdgeAction::Direction(dir)));
                    }
                });
        });
        ui.small(format!("   -> {title}"));
    }
}

/// The inspector's top row: close, and back to the room it came from.
fn inspector_header(ui: &mut egui::Ui, previous: Option<&str>, open: &mut bool, back: &mut bool) {
    ui.horizontal(|ui| {
        if ui
            .button("x")
            .on_hover_text("Close the inspector")
            .clicked()
        {
            *open = false;
        }
        if let Some(previous) = previous
            && ui
                .button("\u{2190}")
                .on_hover_text(format!("Back to {previous}"))
                .clicked()
        {
            *back = true;
        }
        ui.label("Inspector");
    });
    ui.separator();
}

/// The inspected room's own controls: *Unpin room*, then its edges. Both
/// are drawn; an edge's edit wins if both are made in one frame.
fn room_edits(
    ui: &mut egui::Ui,
    shown: &Shown,
    store: &MapOverrides,
    id: RoomId,
) -> Option<EditAction> {
    let unpin = unpin_button(ui, shown, store, id);
    let edges = RoomFacts::gather(id, &shown.subset, &shown.layout)
        .and_then(|facts| edges_editor(ui, shown, store, &facts));
    edges.or(unpin)
}

/// *Unpin room*, for a room with a hand pin.
fn unpin_button(
    ui: &mut egui::Ui,
    shown: &Shown,
    store: &MapOverrides,
    id: RoomId,
) -> Option<EditAction> {
    let key = RoomKey::of(id, &shown.subset);
    (store
        .location(&shown.name)
        .is_some_and(|l| l.room_pins.contains_key(&key))
        && ui
            .button("Unpin room")
            .on_hover_text("Put this room back where the solver placed it")
            .clicked())
    .then_some(EditAction::UnpinRoom { key })
}

/// The editing half of the inspector: which plate this room is on, and the
/// controls to move it to another or onto a new one.
///
/// This is the answer to a crowded town sheet. Wehnimer's Town Square
/// Central has a well and a treehouse hanging off it; moving those onto
/// `landing.well` and `landing.treehouse` takes them out of the town's own
/// list, so they stop competing for space on its sheet, while staying
/// reachable as plates of their own.
fn membership(
    ui: &mut egui::Ui,
    shown: &Shown,
    store: &MapOverrides,
    id: RoomId,
    new_plate: &mut String,
) -> Option<EditAction> {
    let mut edit = None;
    let key = RoomKey::of(id, &shown.subset);
    // Group moves are offered because a building is usually what wants
    // moving, not one room of it.
    let group_keys = |group: usize| -> Vec<RoomKey> {
        shown
            .layout
            .groups
            .get(group)
            .map(|g| {
                g.room_ids
                    .iter()
                    .map(|&rid| RoomKey::of(rid, &shown.subset))
                    .collect()
            })
            .unwrap_or_default()
    };
    let group = shown.scene.room(id).map(|room| room.group);

    ui.add_space(8.0);
    ui.strong("Plate");
    match store.membership_moves.get(&key) {
        Some(plate) => {
            ui.label(format!("On: {}", store.plate_name(plate)));
            if ui
                .button("Send back")
                .on_hover_text("Return this room to the area it came from")
                .clicked()
            {
                edit = Some(EditAction::MoveRooms {
                    keys: vec![key],
                    to: None,
                });
            }
        }
        None => {
            ui.label("On: its own area");
        }
    }

    if !store.custom_maps.is_empty() {
        egui::ComboBox::from_id_salt("move_to_plate")
            .selected_text("Move to plate...")
            .show_ui(ui, |ui| {
                for (plate, entry) in &store.custom_maps {
                    if store.membership_moves.get(&key) == Some(plate) {
                        continue;
                    }
                    if ui.selectable_label(false, &entry.name).clicked() {
                        edit = Some(EditAction::MoveRooms {
                            keys: vec![key],
                            to: Some(plate.clone()),
                        });
                    }
                }
            });
    }

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(new_plate)
                .hint_text("new plate name")
                .desired_width(120.0),
        );
        let named = !new_plate.trim().is_empty();
        if ui
            .add_enabled(named, egui::Button::new("+ room"))
            .on_hover_text("Make this plate and move this room onto it")
            .clicked()
        {
            edit = Some(EditAction::NewPlate {
                name: new_plate.trim().to_owned(),
                keys: vec![key],
            });
            new_plate.clear();
        }
        if let Some(group) = group {
            let keys = group_keys(group);
            if ui
                .add_enabled(named && !keys.is_empty(), egui::Button::new("+ group"))
                .on_hover_text(format!(
                    "Make this plate and move all {} rooms of this group onto it",
                    keys.len()
                ))
                .clicked()
            {
                edit = Some(EditAction::NewPlate {
                    name: new_plate.trim().to_owned(),
                    keys,
                });
                new_plate.clear();
            }
        }
    });
    edit
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
        self.apply_knobs();
        self.take_dropped(ui);

        let mut edit: Option<EditAction> = None;
        let edit_out = &mut edit;
        // The areas the shown sheet's rooms belong to -- the candidates
        // for adopting an orphaned plate.
        let areas_here = self.areas_of_shown();
        // A plate picked from the header's dropdown, jumped to after the
        // frame's panels have let go of their borrows.
        let mut jump: Option<String> = None;
        let go_to_plate = &mut jump;
        let can_edit = self.store_path.is_some() && self.store_problem.is_none();

        match self.corrections_bar(ui, can_edit) {
            (true, _, _) => self.export_corrections(),
            (_, true, _) => self.export_curation(),
            (_, _, true) => self.export_areas(),
            _ => {}
        }

        let mut changed = false;
        egui::Panel::left("areas").show(ui, |ui| {
            changed = self.picker(ui, self.picking_svgs && can_edit);
        });
        if changed {
            self.show_selected();
            // After, because switching areas clears the inspected room.
            if let Some(room) = self.pending_inspect.take() {
                self.inspected = Some(room);
            }
        }

        // Before the central panel, which egui gives whatever space the
        // side panels leave.
        if let Some(action) = self.inspector_panel(ui, can_edit) {
            *edit_out = Some(action);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(shown) = &mut self.shown else {
                ui.heading("Hydra Mapper");
                ui.label("Choose an area on the left.");
                return;
            };

            canvas_header(
                ui,
                shown,
                &self.store,
                &self.areas.baseline,
                self.tab,
                &mut self.view,
                &mut self.params,
                can_edit,
                edit_out,
                go_to_plate,
                &areas_here,
            );

            // Fit on the first frame after a change, when the canvas size
            // is finally known -- `load` has no window to measure.
            if shown.needs_fit {
                let sheet = &shown.scene.sheet;
                let canvas = ui.available_rect_before_wrap();
                if canvas.width() > 0.0 && canvas.height() > 0.0 {
                    self.camera.fit(sheet.min, sheet.max, canvas);
                    shown.needs_fit = false;
                }
            }

            let ghost = self
                .drag
                .as_ref()
                .map(|drag| ghost_cells(drag, &shown.scene, self.camera))
                .unwrap_or_default();
            let focus = shown.draw_focus(self.view.off_region);
            let picked: HashSet<RoomId> = self.picked_rooms.iter().copied().collect();
            let hit = draw::scene(
                ui,
                &shown.scene,
                &focus,
                &mut self.camera,
                self.inspected,
                &picked,
                self.view,
                &ghost,
            );
            self.pointer(&hit);
            if edit_out.is_none() {
                *edit_out = self.handle_drag(&hit);
            }
        });
        if let Some(sheet) = jump {
            self.show_sheet(&sheet);
        }
        if let Some(edit) = edit {
            self.commit(edit);
        }
    }
}

/// "Wrong region (N)", on an area split across regions: paints red the
/// rooms not in the region most of it carries, so they can be picked and
/// moved.
fn wrong_region_toggle(ui: &mut egui::Ui, shown: &Shown, view: &mut draw::View) {
    if shown.off_region.is_empty() {
        return;
    }
    ui.toggle_value(
        &mut view.off_region,
        egui::RichText::new(format!("Wrong region ({})", shown.off_region.len()))
            .color(egui::Color32::from_rgb(255, 90, 90)),
    )
    .on_hover_text(format!(
        "Paint red the rooms not in {}, the region most of this area is in",
        shown.majority
    ));
}

/// The bar above the canvas: the area name, the sheet toggles, Fit, and
/// the edit controls.
///
/// A free function, not a method: `shown` is already borrowed out of the
/// app, so a `&mut self` method could not also be called here.
#[allow(clippy::too_many_arguments)] // each one is a distinct piece of app state
fn canvas_header(
    ui: &mut egui::Ui,
    shown: &mut Shown,
    store: &MapOverrides,
    baseline: &Baseline,
    tab: AreaKind,
    view: &mut draw::View,
    params: &mut LayoutParams,
    can_edit: bool,
    edit_out: &mut Option<EditAction>,
    go_to_plate: &mut Option<String>,
    areas_here: &[String],
) {
    ui.horizontal(|ui| {
        ui.heading(&shown.name);
        ui.separator();
        // What is in focus, and the way back. The rest of the area is
        // always there as dots; clicking one enters its unit.
        match shown.focus.current() {
            Some(unit) => {
                ui.label(format!("{} ({})", unit.name, unit.rooms.len()))
                    .on_hover_text(
                        "In focus: drawn as squares. Click a dot to enter its building or area.",
                    );
            }
            None => {
                ui.weak("nothing in focus")
                    .on_hover_text("Click a dot to bring its building or area into focus.");
            }
        }
        ui.add_enabled_ui(shown.focus.can_go_back(), |ui| {
            if ui.button("Back").clicked() {
                shown.focus.back();
            }
        });
        // The sheets of whichever area this belongs to, beside Outdoor and
        // Interiors. Viewed from the area, that is its plates; viewed from
        // one of those plates, it is the area itself and the plate's
        // siblings -- otherwise a plate is a dead end with no way back to
        // the town it hangs off.
        let owner = store.owner_of(&shown.name).unwrap_or(&shown.name);
        let mut family: Vec<(String, &str)> = vec![(owner.to_owned(), owner)];
        family.extend(
            store
                .plates_of(owner)
                .into_iter()
                .map(|(key, name)| (key.to_owned(), name)),
        );
        if family.len() > 1 {
            egui::ComboBox::from_id_salt("area_sheets")
                .selected_text(format!("Sheets ({})", family.len()))
                .show_ui(ui, |ui| {
                    for (key, name) in family {
                        let here = name == shown.name;
                        if ui.selectable_label(here, name).clicked() && !here {
                            *go_to_plate = Some(key);
                        }
                    }
                });
        }
        wrong_region_toggle(ui, shown, view);
        ui.separator();
        view_controls(ui, shown, view, params);
        ui.separator();
        // Editing is refused outright while the store would not
        // load: the file holds hand curation, and saving over it
        // with an empty one would destroy that work silently.
        ui.add_enabled_ui(can_edit, |ui| {
            ui.toggle_value(&mut view.edit_mode, "Edit")
                .on_hover_text(
                    "Drag a group to move it; drag a picked room to move the pick; hold Alt for one room",
                );
        });
        if view.edit_mode {
            ui.label("drag a group or a pick (Alt: one room) · pick: Ctrl-click, Ctrl-drag");
            // Deleting is offered only where the plate itself is
            // on screen, so it cannot be hit while looking at a
            // town that merely lost rooms to one.
            if let Some(action) = delete_area_button(ui, store, baseline, tab, &shown.name) {
                *edit_out = Some(action);
            }
            if let Some(action) = map_controls(ui, store, baseline, tab, &shown.name) {
                *edit_out = Some(action);
            }
            if tab == AreaKind::Plates
                && let Some(plate) = plate_key_of(store, &shown.name)
            {
                if ui
                    .button("Delete plate")
                    .on_hover_text("Release every room back to its own area")
                    .clicked()
                {
                    *edit_out = Some(EditAction::DeletePlate {
                        plate: plate.clone(),
                    });
                }
                // An orphaned plate -- one whose area was never recorded,
                // or was dropped as unusable on load -- has no route back
                // to a town. Its own rooms know which areas they belong
                // to, so those are the candidates worth offering.
                if store.owner_of(&shown.name).is_none() && !areas_here.is_empty() {
                    egui::ComboBox::from_id_salt("adopt_plate")
                        .selected_text("Belongs to...")
                        .show_ui(ui, |ui| {
                            for area in areas_here {
                                if ui.selectable_label(false, area.as_str()).clicked() {
                                    *edit_out = Some(EditAction::SetPlateArea {
                                        plate: plate.clone(),
                                        area: area.clone(),
                                    });
                                }
                            }
                        });
                }
            }
            let count = store
                .location(&shown.name)
                .map_or(0, crate::overrides::LocationOverrides::len);
            if count > 0
                && ui
                    .button(format!("Reset ({count})"))
                    .on_hover_text("Forget this area's corrections")
                    .clicked()
            {
                *edit_out = Some(EditAction::ResetLocation);
            }
        } else {
            ui.label("drag to pan, wheel to zoom, click a room");
        }
    });
}

/// The plate key whose display name is `shown`, for the delete button.
fn plate_key_of(store: &MapOverrides, shown: &str) -> Option<String> {
    store
        .custom_maps
        .iter()
        .find(|(key, plate)| plate.name == shown || key.as_str() == shown)
        .map(|(key, _)| key.clone())
}

/// Record a step onto `next` in `trail`, coming from `at`.
///
/// Stepping back onto a room already on the trail unwinds to it instead
/// of stacking a second copy -- see [`MapperApp::walk_to`].
fn walk(trail: &mut Vec<RoomId>, at: Option<RoomId>, next: RoomId) {
    if let Some(index) = trail.iter().position(|&id| id == next) {
        trail.truncate(index);
    } else if let Some(from) = at {
        trail.push(from);
    }
}

/// One area's row text: its name, its room count, and how many of those
/// rooms lay out somewhere else.
///
/// The count includes rooms drawn on a plate, which is the point -- they
/// are still rooms of this place -- so the ones that lay out elsewhere are
/// called out rather than leaving the count looking wrong.
fn area_label(
    area: &crate::areas::Area,
    tab: AreaKind,
    map: Option<&Map>,
    store: &MapOverrides,
) -> String {
    let plated = if tab == AreaKind::Plates {
        0
    } else {
        area.rooms
            .iter()
            .filter(|&&id| map.is_some_and(|m| store.is_plated(RoomKey::of(id, m))))
            .count()
    };
    // A curated area whose rooms disagree about their region says so
    // rather than being silently filed under the winner. The row still
    // sits under the majority: the split is worth seeing, not worth
    // refusing to draw, and it is usually the sign of an area that wants
    // dividing rather than a mistake.
    // The queue already sits under its region, so printing the region
    // again in its label just pushes the count off the edge of a narrow
    // panel. The full name is still what the row IS -- see UNAREAED.
    if area.name.starts_with(crate::areas::UNAREAED) {
        return format!("{}  ({})", crate::areas::UNAREAED, area.rooms.len());
    }
    if let Some((top, total)) = area.contested {
        return format!(
            "{}  ({}, {top} of {total} in this region)",
            area.name,
            area.rooms.len()
        );
    }
    if plated > 0 {
        format!("{}  ({}, {plated} on plates)", area.name, area.rooms.len())
    } else {
        format!("{}  ({})", area.name, area.rooms.len())
    }
}

/// A map's and an area's choices, in edit mode: a map shown can be taken
/// apart, its areas back on no map; a curated area shown can be put on
/// any map there is, or taken off its own.
fn map_controls(
    ui: &mut egui::Ui,
    store: &MapOverrides,
    baseline: &Baseline,
    tab: AreaKind,
    shown: &str,
) -> Option<EditAction> {
    let areas = store.known_areas(baseline);
    match tab {
        AreaKind::Maps => {
            let on: Vec<String> = areas
                .keys()
                .filter(|a| store.map_of_area(a, baseline) == Some(shown))
                .cloned()
                .collect();
            ui.button("Take apart")
                .on_hover_text("Take every area off this map: each is its own sheet again")
                .clicked()
                .then_some(EditAction::MapAreas {
                    areas: on,
                    map: None,
                })
        }
        AreaKind::Region => {
            let (key, _) = areas.iter().find(|(_, name)| name.as_str() == shown)?;
            let now = store.map_of_area(key, baseline);
            let mut maps: Vec<&str> = areas
                .keys()
                .filter_map(|a| store.map_of_area(a, baseline))
                .collect();
            maps.sort_unstable();
            maps.dedup();
            let mut edit = None;
            egui::ComboBox::from_id_salt("area_map")
                .selected_text(format!("Map: {}", now.unwrap_or("none")))
                .show_ui(ui, |ui| {
                    if ui.selectable_label(now.is_none(), "none").clicked() && now.is_some() {
                        edit = Some(EditAction::MapAreas {
                            areas: vec![key.clone()],
                            map: None,
                        });
                    }
                    for map in maps {
                        if ui.selectable_label(now == Some(map), map).clicked() && now != Some(map)
                        {
                            edit = Some(EditAction::MapAreas {
                                areas: vec![key.clone()],
                                map: Some(map.to_owned()),
                            });
                        }
                    }
                });
            edit
        }
        _ => None,
    }
}

/// "Delete area", offered only while looking at the area itself.
///
/// Never from the region above it: a region row and its areas sit in one
/// list, and a delete reachable from the parent is one somebody hits
/// while meaning to tidy the child.
///
/// The rooms are untouched -- only the assignment goes -- so this is not
/// the irreversible kind of delete.
fn delete_area_button(
    ui: &mut egui::Ui,
    store: &MapOverrides,
    baseline: &Baseline,
    tab: AreaKind,
    shown: &str,
) -> Option<EditAction> {
    if tab != AreaKind::Region {
        return None;
    }
    let (key, _) = store
        .custom_areas
        .iter()
        .map(|(k, a)| (k.clone(), a.name.clone()))
        .chain(baseline.areas.iter().map(|(k, n)| (k.clone(), n.clone())))
        .find(|(_, name)| name == shown)?;
    ui.button("Delete area")
        .on_hover_text("Release every room; the rooms themselves are untouched")
        .clicked()
        .then_some(EditAction::DeleteArea { area: key })
}

/// "room 7562 is in Wehnimer's Landing", as a link that goes there.
///
/// A bare number in the filter box is a room, not a name: with hundreds
/// of areas there is otherwise no way to answer "which area holds this
/// room", and that is exactly the question an exit leading out of an area
/// provokes.
///
/// Region is not searched. It spans the whole map, so it would answer
/// every query with a region name and shadow the more specific list that
/// actually tells someone where to look.
fn room_search(
    ui: &mut egui::Ui,
    areas: &Areas,
    found: RoomId,
) -> Option<(AreaKind, usize, RoomId)> {
    let held_by = |kind: AreaKind| {
        areas
            .list(kind)
            .iter()
            .position(|a| a.rooms.contains(&found))
            .map(|index| (kind, index))
    };
    let Some((kind, index)) = held_by(AreaKind::Plates)
        .or_else(|| held_by(AreaKind::Official))
        .or_else(|| held_by(AreaKind::Location))
    else {
        ui.weak(format!("room {} is not in this map", found.0));
        return None;
    };
    let area = &areas.list(kind)[index];
    ui.link(format!("room {} is in {}", found.0, area.name))
        .on_hover_text("Show that area and inspect the room")
        .clicked()
        .then_some((kind, index, found))
}

/// A row's text, styled by what it is.
///
/// In the Region tree a region, its work queue and its areas are three
/// different kinds of row that were drawing identically, which made a
/// 215-row list unreadable. They are told apart by weight rather than by
/// a symbol, because the symbols this wanted turned out not to exist in
/// egui's font:
///
/// - a **region** is strong, so the eye can find the headings
/// - the **leftover queue** is weak, because it is a residue rather than
///   a place, and a region with a big one should not look busy
/// - an **area** is plain
///
/// Every other tab is a flat list of one kind of thing and keeps the
/// plain text it had.
fn row_text(area: &crate::areas::Area, tab: AreaKind, label: &str) -> egui::RichText {
    let text = egui::RichText::new(label);
    if tab != AreaKind::Region {
        return text;
    }
    if area.parent.is_none() {
        return text.strong();
    }
    if area.name.starts_with(crate::areas::UNAREAED) {
        return text.weak();
    }
    text
}

/// The expander in front of a Region row, and the indent in front of a
/// curated area. Returns true when the region was clicked to fold.
///
/// Every row gets 16 points at the left whether or not it has a handle,
/// so names line up instead of jittering by whether a region happens to
/// have areas yet.
fn tree_handle(
    ui: &mut egui::Ui,
    area: &crate::areas::Area,
    tab: AreaKind,
    rows: &[crate::areas::Area],
    collapsed: &BTreeSet<String>,
) -> bool {
    if area.parent.is_some() {
        ui.add_space(16.0);
        return false;
    }
    if tab != AreaKind::Region {
        return false;
    }
    let has_children = rows
        .iter()
        .any(|a| a.parent.as_deref() == Some(area.name.as_str()));
    if !has_children {
        ui.add_space(16.0);
        return false;
    }
    let shut = collapsed.contains(&area.name);
    // ASCII, not the triangles this obviously wants. egui ships a font
    // with no glyph for U+25B8 or U+25BE, so they drew as empty boxes,
    // and a caret that renders as a missing-glyph square is worse than a
    // plain one that always works.
    ui.small_button(if shut { "+" } else { "-" })
        .on_hover_text(if shut {
            "Show this region's areas"
        } else {
            "Fold this region's areas away"
        })
        .clicked()
}

/// The rows of a tab, in the order they are drawn.
///
/// The Region tab is a tree -- regions, each followed by the curated
/// areas whose rooms carry that region -- and every other tab is the list
/// as it stands. Ordering lives here rather than in `Areas`, which holds
/// what the lists ARE and has no opinion about how they are shown.
///
/// A collapsed region still appears; only its areas are folded away. A
/// region with no areas yet is exactly the one someone needs to click on
/// to start curating it.
fn row_order(
    rows: &[crate::areas::Area],
    tab: AreaKind,
    collapsed: &BTreeSet<String>,
) -> Vec<usize> {
    if tab != AreaKind::Region {
        return (0..rows.len()).collect();
    }
    let mut out = Vec::new();
    for (i, region) in rows.iter().enumerate() {
        if region.parent.is_some() {
            continue; // a curated area: drawn under its region, below
        }
        out.push(i);
        if collapsed.contains(&region.name) {
            continue;
        }
        // The leftover queue first, then the areas. "What is still
        // unsorted here" is the row someone opens a region to find.
        let mine = || {
            rows.iter()
                .enumerate()
                .filter(|(_, a)| a.parent.as_deref() == Some(region.name.as_str()))
        };
        out.extend(
            mine()
                .filter(|(_, a)| a.name.starts_with(crate::areas::UNAREAED))
                .map(|(j, _)| j),
        );
        out.extend(
            mine()
                .filter(|(_, a)| !a.name.starts_with(crate::areas::UNAREAED))
                .map(|(j, _)| j),
        );
    }
    out
}

/// A cell offset in words: "3 east, 2 north".
///
/// y grows downward on the grid, so a negative `dy` is north. Said in
/// compass terms because that is how a person reads a map, and the raw
/// signs invite exactly the wrong guess.
fn offset_phrase(dx: i32, dy: i32) -> String {
    let mut parts = Vec::new();
    if dx != 0 {
        parts.push(format!(
            "{} {}",
            dx.abs(),
            if dx > 0 { "east" } else { "west" }
        ));
    }
    if dy != 0 {
        parts.push(format!(
            "{} {}",
            dy.abs(),
            if dy > 0 { "south" } else { "north" }
        ));
    }
    if parts.is_empty() {
        "same cell".to_owned()
    } else {
        parts.join(", ")
    }
}

/// A drag's pixel travel as whole grid cells, rounded, so a move snaps to
/// the grid the layout is drawn on.
#[allow(clippy::cast_possible_truncation)]
fn cells_dragged(accumulated: egui::Vec2, camera: Camera, scale: i32) -> Cell {
    // A drawn cell is `scale` solver cells wide on a spread-out sheet;
    // the edit is in the solver's cells.
    #[allow(clippy::cast_precision_loss)] // a sheet scale is 1 or 2
    let px = camera.cell_px() * scale.max(1) as f32;
    if px <= 0.0 {
        return Cell::default();
    }
    Cell {
        x: (accumulated.x / px).round() as i32,
        y: (accumulated.y / px).round() as i32,
    }
}

/// Where every room an in-flight drag carries would land, in drawn cells,
/// for the ghost preview. Each room snaps at its own group's scale, the
/// same way the release will commit it.
fn ghost_cells(drag: &DragState, scene: &MapScene, camera: Camera) -> Vec<Cell> {
    let landing = |room: &SceneRoom| {
        let scale = scene.scale_of(room.group);
        let d = cells_dragged(drag.accumulated, camera, scale);
        // Drawn back at the group's spacing.
        Cell {
            x: room.cell.x + d.x * scale,
            y: room.cell.y + d.y * scale,
        }
    };
    match &drag.moving {
        Moving::Group(group) => scene
            .sheet
            .rooms
            .iter()
            .filter(|r| r.group == *group)
            .map(landing)
            .collect(),
        Moving::Rooms(ids) => ids
            .iter()
            .filter_map(|&id| scene.room(id))
            .map(landing)
            .collect(),
    }
}

/// Write the room table beside the store; the note says where, or why not.
fn write_areas(map: &Map, store: &MapOverrides, store_path: &Path) -> String {
    let path = store_path.with_extension("areas.tsv");
    let baseline = Baseline::of(map);
    match std::fs::write(&path, crate::room_table::areas_tsv(map, store, &baseline)) {
        Ok(()) => format!("Wrote {} rooms to {}", map.rooms().len(), path.display()),
        Err(error) => format!("Could not write {}: {error}", path.display()),
    }
}

/// `--export-curation`: `curation/assignments.toml` from the store beside
/// the map, with no window.
///
/// # Errors
///
/// When there is no map path, or the store will not parse -- an empty
/// store would write a file unassigning every room.
pub fn export_curation_headless(path: Option<&Path>) -> Result<String, String> {
    let map = load_map(path).map_err(|p| p.to_string())?;
    let store_path = overrides::store_path(path.ok_or("no map path")?);
    let store =
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?;
    let baseline = Baseline::of(&map);
    let dir = store_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("curation");
    let target = dir.join("assignments.toml");
    let text = crate::room_table::assignments_toml(&map, &store, &baseline);
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&target, text))
        .map_err(|e| format!("Could not write {}: {e}", target.display()))?;
    Ok(format!(
        "Wrote the curation ({} area edit(s) and {} region assignment(s) in the store) to {}",
        store.area_moves.len(),
        store.region_moves.len(),
        target.display()
    ))
}

/// `--import <changes> [map]`: merge a contributor's changes file into the
/// store beside the map, keeping the store as it was in `.json.bak`.
///
/// # Errors
///
/// When either file will not load, or the store cannot be written.
pub fn import_changes_headless(changes: &Path, path: Option<&Path>) -> Result<String, String> {
    let store_path = overrides::store_path(path.ok_or("no map path")?);
    let mut store =
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?;
    let theirs = MapOverrides::load(changes).map_err(|e| format!("{} {e}", changes.display()))?;
    if store_path.exists() {
        std::fs::copy(&store_path, store_path.with_extension("json.bak"))
            .map_err(|e| format!("Could not back up {}: {e}", store_path.display()))?;
    }
    let report = store.merge(&theirs);
    store
        .save(&store_path)
        .map_err(|e| format!("Could not write {}: {e}", store_path.display()))?;
    Ok(format!(
        "Took in {report} from {} into {}",
        changes.display(),
        store_path.display()
    ))
}

/// `--plan-areas [map]` and `--fill-areas [map]`: put every room with a
/// region and no area into one, from its title, location and walls (see
/// [`crate::area_fill`]). Both write a review table beside the store --
/// `<store>.fill-plan.tsv` for a plan, `<store>.fill.tsv` for a fill --
/// and only `--fill-areas` writes the store, keeping it as it was in
/// `.json.bak`.
///
/// **Close the mapper first.** An open mapper holds the store in memory
/// and would save over the fill with its next edit.
///
/// # Errors
///
/// When the map or store will not load, or a file cannot be written.
pub fn fill_areas_headless(path: Option<&Path>, write: bool) -> Result<String, String> {
    let map = load_map(path).map_err(|p| p.to_string())?;
    let store_path = overrides::store_path(path.ok_or("no map path")?);
    let mut store =
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?;
    let baseline = Baseline::of(&map);
    let fill = crate::area_fill::propose(&map, &store, &baseline);
    // A plan gets its own table, so checking afterwards does not overwrite
    // the record of what a fill did.
    let table = store_path.with_extension(if write { "fill.tsv" } else { "fill-plan.tsv" });
    std::fs::write(
        &table,
        crate::area_fill::review_tsv(&fill, &map, &store, &baseline),
    )
    .map_err(|e| format!("Could not write {}: {e}", table.display()))?;
    if !write {
        return Ok(format!(
            "{fill}. Nothing written; review {}",
            table.display()
        ));
    }
    if store_path.exists() {
        std::fs::copy(&store_path, store_path.with_extension("json.bak"))
            .map_err(|e| format!("Could not back up {}: {e}", store_path.display()))?;
    }
    crate::area_fill::apply(&fill, &map, &mut store);
    store
        .save(&store_path)
        .map_err(|e| format!("Could not write {}: {e}", store_path.display()))?;
    Ok(format!(
        "{fill}. Written to {}; each room is listed in {}",
        store_path.display(),
        table.display()
    ))
}

/// `--export-areas`: the same export, with no window.
///
/// # Errors
///
/// When the map will not load or the store will not parse -- the table
/// would say every room is unassigned, which is worse than nothing.
pub fn export_areas_headless(path: Option<&Path>) -> Result<String, String> {
    let map = load_map(path).map_err(|p| p.to_string())?;
    let store_path = overrides::store_path(path.ok_or("no map path")?);
    let store =
        MapOverrides::load(&store_path).map_err(|e| format!("{} {e}", store_path.display()))?;
    Ok(write_areas(&map, &store, &store_path))
}

pub(crate) fn load_map(path: Option<&Path>) -> Result<Map, LoadProblem> {
    let Some(path) = path else {
        return Err(LoadProblem::NoPath);
    };
    let path_str = path.display().to_string();
    // A bundled build's default path names no file unless someone put a
    // map there; the embedded one stands in. Only for that path, so a
    // mistyped one still says it could not be read.
    let bytes = match std::fs::read(path) {
        Ok(bytes) => std::borrow::Cow::Owned(bytes),
        Err(error) => match crate::bundle::MAP {
            Some(bytes) if crate::bundle::default_path().as_deref() == Some(path) => {
                std::borrow::Cow::Borrowed(bytes)
            }
            _ => {
                return Err(LoadProblem::CouldNotRead {
                    path: path_str,
                    error,
                });
            }
        },
    };
    cena_map::binary::decode(&bytes).map_err(|error| LoadProblem::NotAMap {
        path: path_str,
        error,
    })
}

/// Fit, the drawing toggles, and the layout's knobs. A knob re-solves
/// the shown area when it moves.
fn view_controls(
    ui: &mut egui::Ui,
    shown: &mut Shown,
    view: &mut draw::View,
    params: &mut LayoutParams,
) {
    if ui.button("Fit").clicked() {
        shown.needs_fit = true;
    }
    ui.toggle_value(&mut view.labels, "Labels")
        .on_hover_text("Draw room titles (hover still shows them)");
    ui.toggle_value(&mut view.interiors, "Interiors")
        .on_hover_text("Draw every interior room as a dot, not only each building's door");
    ui.separator();
    ui.add(egui::Slider::new(&mut params.town_scale, 1..=12).text("Town scale"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// y grows downward on the grid, so a negative dy is **north**. The
    /// panel says this out loud to a person reading a map, and getting the
    /// sign backwards would be invisible in the numbers.
    #[test]
    fn an_offset_reads_in_compass_terms() {
        assert_eq!(offset_phrase(3, -2), "3 east, 2 north");
        assert_eq!(offset_phrase(-1, 4), "1 west, 4 south");
        assert_eq!(offset_phrase(0, -1), "1 north");
        assert_eq!(offset_phrase(2, 0), "2 east");
        assert_eq!(offset_phrase(0, 0), "same cell");
    }

    /// `needs_fit` is what re-fits the camera on the next frame, and
    /// `rebuild_shown` sets it from its `Fit`. Only a switch may ask for
    /// one: an edit that does is the drag bug, where the view jumps away
    /// from whatever the person was aiming at.
    #[test]
    fn only_a_switch_asks_for_a_refit() {
        assert!(needs_fit_for(Fit::Reset), "a switch should re-fit");
        assert!(!needs_fit_for(Fit::Keep), "an edit must not re-fit");
    }

    /// Walking room to room remembers the way back, and walking into a
    /// room already behind you unwinds to it rather than growing the
    /// trail -- rooms link both ways, so a corridor walked up and down
    /// would otherwise never stop stacking.
    #[test]
    fn the_trail_unwinds_rather_than_looping() {
        let (a, b, c) = (RoomId(1), RoomId(2), RoomId(3));
        let mut trail = Vec::new();

        walk(&mut trail, Some(a), b);
        assert_eq!(trail, vec![a], "did not remember where it came from");
        walk(&mut trail, Some(b), c);
        assert_eq!(trail, vec![a, b]);

        // Back into b, which is behind us: unwind to it.
        walk(&mut trail, Some(c), b);
        assert_eq!(trail, vec![a], "a revisit stacked instead of unwinding");

        // And back to a, the start: nothing left to go back to.
        walk(&mut trail, Some(b), a);
        assert!(trail.is_empty());
    }

    /// The first room inspected has nothing behind it, so no back button.
    #[test]
    fn the_first_room_has_no_way_back() {
        let mut trail = Vec::new();
        walk(&mut trail, None, RoomId(7));
        assert!(trail.is_empty());
    }

    /// A fitted camera stays put when the same sheet is rebuilt, and only
    /// moves when a fit is actually asked for. This is the property the
    /// drag bug violated.
    #[test]
    fn a_kept_camera_does_not_move() {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(800.0, 600.0));
        let mut camera = Camera::default();
        camera.fit(Cell { x: 0, y: 0 }, Cell { x: 20, y: 20 }, canvas);
        let (centre, scale) = (camera.center, camera.scale);

        // Redrawing after an edit: nothing touches the camera.
        assert_eq!(camera.center, centre);
        assert!((camera.scale - scale).abs() < f32::EPSILON);

        // Switching areas: the camera does move.
        camera.fit(Cell { x: 90, y: 90 }, Cell { x: 100, y: 100 }, canvas);
        assert_ne!(camera.center, centre, "a switch should re-centre");
    }
}
