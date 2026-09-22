//! The window's state and its `eframe::App` implementation.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use cena_map::{Map, RoomId};
use cena_map_layout::Cell;
use cena_map_layout::scene::Sheet;
use cena_map_layout::{
    Dir, EdgeAction, Layout, MapScene, build_scene, generate_layout, generate_layout_with,
};

use crate::areas::{AreaKind, Areas};
use crate::camera::Camera;
use crate::draw;
use crate::export;
use crate::inspect::{Crossed, RoomFacts, bearing};
use crate::overrides::{self, MapOverrides, RoomKey};

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
    /// Read by the inspector panel, for the diagnostics the scene does not
    /// carry: group, pack method and direction violations.
    layout: Layout,
    /// The area's own rooms, kept so the inspector can read full room
    /// records -- the scene carries only what it needs to draw.
    subset: Map,
    scene: MapScene,
    /// Set when the area has just changed, so the next frame -- the first
    /// one that knows how big the canvas is -- fits the camera to it.
    needs_fit: bool,
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
    /// Place one room within its group's frame.
    PinRoom { key: RoomKey, pin: Cell },
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
    /// Whether dragging moves rooms instead of panning the view.
    edit_mode: bool,
    drag: Option<DragState>,
    /// Name being typed for a new plate.
    new_plate: String,
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
}

/// A drag in progress. Committed as one correction on release, so dragging
/// a group across the sheet is a single entry rather than one per frame.
#[derive(Debug, Clone, Copy)]
struct DragState {
    /// The group being moved.
    group: usize,
    /// Set when only this room moves (Alt held at drag start).
    room: Option<RoomId>,
    /// Pixels moved so far, converted to whole cells on release.
    accumulated: egui::Vec2,
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
        let areas = match &map {
            Ok(map) => Areas::build(map, &store),
            Err(_) => Areas {
                official: Vec::new(),
                mapdb: Vec::new(),
                plates: Vec::new(),
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
            inspected: None,
            shown: None,
            store,
            store_path,
            store_problem,
            edit_mode: false,
            drag: None,
            new_plate: String::new(),
            pending_inspect: None,
            trail: Vec::new(),
            export_note: None,
        }
    }

    /// Which rooms of each area land on its interiors shelf.
    ///
    /// Recomputed here rather than cached: exporting is rare, and a stale
    /// answer would put rooms on the wrong grid.
    fn interiors_by_area(&self, map: &Map) -> Vec<(String, Vec<RoomId>)> {
        let mut out = Vec::new();
        for kind in [AreaKind::Official, AreaKind::Mapdb, AreaKind::Plates] {
            for area in self.areas.list(kind) {
                let rooms: Vec<cena_map::Room> = area
                    .rooms
                    .iter()
                    .filter_map(|&id| map.room(id).cloned())
                    .collect();
                let Ok(subset) = Map::from_rooms(rooms) else {
                    continue;
                };
                let location = self.store.location(&area.name);
                let edges = location
                    .map(|l| l.edge_overrides(&subset))
                    .unwrap_or_default();
                let layout = if edges.is_empty() {
                    generate_layout(&subset)
                } else {
                    generate_layout_with(&subset, &edges)
                };
                let shelved: Vec<RoomId> = layout
                    .interiors
                    .iter()
                    .flat_map(|&i| layout.groups[i].room_ids.iter().copied())
                    .collect();
                if !shelved.is_empty() {
                    out.push((area.name.clone(), shelved));
                }
            }
        }
        out
    }

    /// The bar above everything: the export button, and whatever the
    /// corrections as a whole have to say. Returns whether to export.
    fn corrections_bar(&mut self, ui: &mut egui::Ui, can_edit: bool) -> bool {
        // A store that will not load or save is said once, at the top,
        // because it means corrections are not being kept. The export
        // note shares the bar: both are about the corrections as a whole,
        // not about whatever area is on screen.
        let mut export_now = false;
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
                });
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
        export_now
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
            for kind in [AreaKind::Official, AreaKind::Mapdb] {
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
                for kind in [AreaKind::Official, AreaKind::Mapdb] {
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
        for kind in [AreaKind::Plates, AreaKind::Official, AreaKind::Mapdb] {
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
        let interiors = self.interiors_by_area(map);
        // Which area each room belongs to, so a plated room's place
        // travels alongside the grid it is drawn on.
        let area_of = |id: RoomId| -> Option<String> {
            for kind in [AreaKind::Official, AreaKind::Mapdb] {
                if let Some(area) = self.areas.list(kind).iter().find(|a| a.rooms.contains(&id)) {
                    return Some(area.name.clone());
                }
            }
            None
        };
        let (export, skipped) = export::build(&self.store, map, source, &interiors, &area_of);
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
                if !skipped.is_empty() {
                    // Said out loud: a correction that cannot be named
                    // across a map rebuild did not travel, and a person
                    // should know rather than find out downstream.
                    let _ = write!(note, "; {} skipped ({})", skipped.len(), skipped[0].why);
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
        let rooms: Vec<cena_map::Room> = area
            .rooms
            .iter()
            .filter(|&&id| {
                area.kind == AreaKind::Plates || !self.store.is_plated(RoomKey::of(id, map))
            })
            .filter_map(|&id| map.room(id).cloned())
            .collect();
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
        let location = self.store.location(&name);
        // Edge corrections go IN to the solve: they change what the solver
        // does, so the rooms are placed by the corrected geometry. Moves
        // and pins come after, as a diff on its result.
        let edges = location
            .map(|l| l.edge_overrides(&subset))
            .unwrap_or_default();
        let mut layout = if edges.is_empty() {
            generate_layout(&subset)
        } else {
            generate_layout_with(&subset, &edges)
        };
        if let Some(location) = location {
            overrides::apply(&mut layout, &subset, location);
        }
        let scene = build_scene(&name, &layout, &subset);
        if fit == Fit::Reset {
            // A new area's selection does not carry over: the room is not
            // in it, and a stale inspector panel would describe nothing
            // visible. The trail goes with it -- walking back into the
            // area just left would be worse than no button.
            self.inspected = None;
            self.trail.clear();
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
        self.shown = Some(Shown {
            name,
            layout,
            subset,
            scene,
            needs_fit: needs_fit_for(fit),
        });
    }

    /// Track a drag across frames and turn a finished one into an edit.
    ///
    /// The whole drag is one correction: pixels accumulate while the mouse
    /// is down and convert to cells once, on release, so a slow drag does
    /// not record a trail of one-cell nudges.
    fn handle_drag(&mut self, hit: &draw::Hit) -> Option<EditAction> {
        if let Some((id, alt)) = hit.drag_started {
            let group = self
                .shown
                .as_ref()
                .and_then(|shown| shown.scene.room(id).map(|(_, room)| room.group));
            self.drag = group.map(|group| DragState {
                group,
                room: alt.then_some(id),
                accumulated: egui::Vec2::ZERO,
            });
        }
        if let (Some(delta), Some(drag)) = (hit.dragged_by, self.drag.as_mut()) {
            drag.accumulated += delta;
        }
        if !hit.drag_stopped {
            return None;
        }

        let drag = self.drag.take()?;
        let delta = cells_dragged(drag, self.camera);
        if delta.x == 0 && delta.y == 0 {
            return None;
        }
        let shown = self.shown.as_ref()?;
        #[allow(clippy::single_match_else)] // both arms build a different edit
        match drag.room {
            // One room: pinned at its position within the group's own
            // frame, which is its drawn cell less the group's offset.
            Some(id) => {
                let room = shown.scene.room(id)?.1;
                let offset = shown
                    .scene
                    .group_offsets
                    .get(&drag.group)
                    .copied()
                    .unwrap_or_default();
                Some(EditAction::PinRoom {
                    key: RoomKey::of(id, &shown.subset),
                    pin: Cell {
                        x: room.cell.x - offset.x + delta.x,
                        y: room.cell.y - offset.y + delta.y,
                    },
                })
            }
            None => {
                let group = shown.layout.groups.get(drag.group)?;
                Some(EditAction::NudgeGroup {
                    anchor: RoomKey::anchor(group, &shown.subset)?,
                    delta,
                })
            }
        }
    }

    /// Apply an edit, save the store, and redraw whatever it changed.
    fn commit(&mut self, edit: EditAction) {
        let area = self.shown.as_ref().map(|shown| shown.name.clone());
        // A plate made while looking at a plate belongs to that plate's
        // own area, not to the plate: plates are sheets of an area, never
        // of each other, and a plate owning itself is a dead end with no
        // way back to the town it hangs off.
        let owning_area = area
            .as_deref()
            .map(|name| self.store.owner_of(name).unwrap_or(name).to_owned());
        let mut membership_changed = false;
        match edit {
            EditAction::NudgeGroup { anchor, delta } => {
                let Some(area) = area else { return };
                self.store.nudge_group(&area, anchor, delta);
            }
            EditAction::PinRoom { key, pin } => {
                let Some(area) = area else { return };
                self.store.pin_room(&area, key, Some(pin));
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

    /// The inspector panel, drawn before the central panel so egui gives
    /// the canvas whatever space is left.
    fn inspector_panel(&mut self, ui: &mut egui::Ui, can_edit: bool) -> Option<EditAction> {
        let (Some(shown), Some(id)) = (&self.shown, self.inspected) else {
            return None;
        };
        let mut edit = None;
        let edit_out = &mut edit;
        let mut open = true;
        let store = &self.store;
        let new_plate = &mut self.new_plate;
        let edit_mode = self.edit_mode && can_edit;

        let whole = self.map.as_ref().ok();
        let visiting = shown.subset.room(id).is_none();
        let mut follow = None;
        let mut back = false;
        // What the back button returns to, named rather than numbered:
        // "back to [Town Square Central]" says where it goes.
        let previous = self.trail.last().and_then(|&id| {
            self.map
                .as_ref()
                .ok()
                .and_then(|m| m.room(id))
                .and_then(|r| r.title.first().cloned())
                .or_else(|| Some(format!("room {}", id.0)))
        });

        egui::Panel::right("inspector").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("x")
                    .on_hover_text("Close the inspector")
                    .clicked()
                {
                    open = false;
                }
                if let Some(previous) = &previous
                    && ui
                        .button("\u{2190}")
                        .on_hover_text(format!("Back to {previous}"))
                        .clicked()
                {
                    back = true;
                }
                ui.label("Inspector");
            });
            ui.separator();
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
                        visiting_membership(ui, shown, store, whole, id, new_plate)
                {
                    *edit_out = Some(action);
                }
                return;
            }
            inspector(ui, shown, id, whole, &mut follow);
            if !edit_mode {
                return;
            }
            // Editing controls live below the facts, so the panel
            // reads the same whether or not Edit is on.
            ui.separator();
            let key = RoomKey::of(id, &shown.subset);
            if store
                .location(&shown.name)
                .is_some_and(|l| l.room_pins.contains_key(&key))
                && ui
                    .button("Unpin room")
                    .on_hover_text("Put this room back where the solver placed it")
                    .clicked()
            {
                *edit_out = Some(EditAction::UnpinRoom { key });
            }
            if let Some(facts) = RoomFacts::gather(id, &shown.subset, &shown.layout)
                && let Some(action) = edges_editor(ui, shown, store, &facts)
            {
                *edit_out = Some(action);
            }
            if let Some(action) = membership(ui, shown, store, id, new_plate) {
                *edit_out = Some(action);
            }
        });
        if !open {
            self.inspected = None;
            self.trail.clear();
        } else if let Some(next) = follow {
            self.walk_to(next);
        } else if back {
            self.inspected = self.trail.pop();
        }
        edit
    }

    /// The left panel: the two list tabs, a filter box, and the list.
    fn picker(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        // Set when a room-number search is followed: the area to show,
        // and the room to inspect once it is on screen.
        let mut goto: Option<(AreaKind, usize, RoomId)> = None;

        ui.heading("Areas");
        ui.horizontal(|ui| {
            for kind in [AreaKind::Official, AreaKind::Mapdb, AreaKind::Plates] {
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
            let held_by = |kind: AreaKind| {
                self.areas
                    .list(kind)
                    .iter()
                    .position(|a| a.rooms.contains(&found))
                    .map(|index| (kind, index))
            };
            match held_by(AreaKind::Plates)
                .or_else(|| held_by(AreaKind::Official))
                .or_else(|| held_by(AreaKind::Mapdb))
            {
                Some((kind, index)) => {
                    let area = &self.areas.list(kind)[index];
                    if ui
                        .link(format!("room {} is in {}", found.0, area.name))
                        .on_hover_text("Show that area and inspect the room")
                        .clicked()
                    {
                        goto = Some((kind, index, found));
                    }
                }
                None => {
                    ui.weak(format!("room {} is not in this map", found.0));
                }
            }
        }
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
                // An area's count includes rooms drawn on a plate, which
                // is the point -- they are still rooms of this place --
                // so the ones that lay out elsewhere are called out
                // rather than leaving the count looking wrong.
                let plated = if self.tab == AreaKind::Plates {
                    0
                } else {
                    area.rooms
                        .iter()
                        .filter(|&&id| {
                            self.map
                                .as_ref()
                                .is_ok_and(|m| self.store.is_plated(RoomKey::of(id, m)))
                        })
                        .count()
                };
                let label = if plated > 0 {
                    format!("{}  ({}, {plated} on plates)", area.name, area.rooms.len())
                } else {
                    format!("{}  ({})", area.name, area.rooms.len())
                };
                if ui.selectable_label(selected, label).clicked() {
                    self.selected = Some(selection);
                    changed = true;
                }
            }
        });
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
            ui.label(format!("Tags: {}", facts.tags.join(", ")));
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

/// The exit list, with rooms outside this area as links to follow.
fn exit_list(ui: &mut egui::Ui, facts: &RoomFacts, follow: &mut Option<RoomId>) {
    ui.strong(format!("Exits ({})", facts.exits.len()));
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
    let saved = store.location(&shown.name);

    ui.add_space(8.0);
    ui.strong("Edges");
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
                            edit = Some(EditAction::SetEdge {
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
    edit
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
    let group = shown.scene.room(id).map(|(_, room)| room.group);

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

        if self.corrections_bar(ui, can_edit) {
            self.export_corrections();
        }

        let mut changed = false;
        egui::Panel::left("areas").show(ui, |ui| {
            changed = self.picker(ui);
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
                self.tab,
                &mut self.sheet,
                &mut self.edit_mode,
                can_edit,
                edit_out,
                go_to_plate,
                &areas_here,
            );

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

            // The in-flight drag, in whole cells, for the ghost preview.
            let ghost = self
                .drag
                .map(|drag| (drag.group, drag.room, cells_dragged(drag, self.camera)));
            let hit = draw::scene(
                ui,
                &shown.scene,
                self.sheet,
                &mut self.camera,
                self.inspected,
                self.edit_mode,
                ghost,
            );
            if let Some(id) = hit.clicked {
                // Clicking the inspected room again closes the panel, so
                // the canvas can be cleared without reaching for the x.
                self.inspected = (self.inspected != Some(id)).then_some(id);
            }
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
    tab: AreaKind,
    sheet: &mut Sheet,
    edit_mode: &mut bool,
    can_edit: bool,
    edit_out: &mut Option<EditAction>,
    go_to_plate: &mut Option<String>,
    areas_here: &[String],
) {
    ui.horizontal(|ui| {
        ui.heading(&shown.name);
        ui.separator();
        for (which, label) in [(Sheet::Outdoor, "Outdoor"), (Sheet::Interiors, "Interiors")] {
            let count = shown.scene.sheet(which).rooms.len();
            let chosen = *sheet == which;
            // An empty sheet stays visible but unclickable, so it
            // is clear the location simply has no interiors rather
            // than the toggle having gone missing.
            ui.add_enabled_ui(count > 0, |ui| {
                if ui
                    .selectable_label(chosen, format!("{label} ({count})"))
                    .clicked()
                {
                    *sheet = which;
                    shown.needs_fit = true;
                }
            });
        }
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
        ui.separator();
        if ui.button("Fit").clicked() {
            shown.needs_fit = true;
        }
        ui.separator();
        // Editing is refused outright while the store would not
        // load: the file holds hand curation, and saving over it
        // with an empty one would destroy that work silently.
        ui.add_enabled_ui(can_edit, |ui| {
            ui.toggle_value(&mut *edit_mode, "Edit")
                .on_hover_text("Drag a group to move it; hold Alt for one room");
        });
        if *edit_mode {
            ui.label("drag a group (Alt: one room)");
            // Deleting is offered only where the plate itself is
            // on screen, so it cannot be hit while looking at a
            // town that merely lost rooms to one.
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

/// A drag's pixel travel as whole grid cells, rounded, so a move snaps to
/// the grid the layout is drawn on.
#[allow(clippy::cast_possible_truncation)]
fn cells_dragged(drag: DragState, camera: Camera) -> Cell {
    let px = camera.cell_px();
    if px <= 0.0 {
        return Cell::default();
    }
    Cell {
        x: (drag.accumulated.x / px).round() as i32,
        y: (drag.accumulated.y / px).round() as i32,
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

#[cfg(test)]
mod tests {
    use super::*;

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
