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
        let (export, skipped) = export::build(&self.store, map, source, &interiors);
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
        self.show_selected();
    }

    /// Compute (or recompute) the layout and scene for the selected area.
    ///
    /// Cheap enough to redo on every selection change: measured against
    /// the real map, the worst area (Wehnimer's Landing, 3,229 rooms)
    /// takes ~102ms in release, and every other one far less.
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
        // A new area's selection does not carry over: the room is not in
        // it, and a stale inspector panel would describe nothing visible.
        self.inspected = None;
        self.shown = Some(Shown {
            name,
            layout,
            subset,
            scene,
            needs_fit: true,
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
                let plate = self.store.create_map(&name);
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
            self.show_selected();
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

        egui::Panel::right("inspector").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("x")
                    .on_hover_text("Close the inspector")
                    .clicked()
                {
                    open = false;
                }
                ui.label("Inspector");
            });
            ui.separator();
            inspector(ui, shown, id);
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
        }
        edit
    }

    /// The left panel: the two list tabs, a filter box, and the list.
    fn picker(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        ui.heading("Areas");
        ui.horizontal(|ui| {
            for kind in [AreaKind::Official, AreaKind::Mapdb, AreaKind::Plates] {
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

/// The inspector panel: everything known about one room, from all three
/// sources -- the room record, its exits, and the layout that placed it.
///
/// A free function rather than a method so it borrows only the `Shown` it
/// reads, leaving the rest of the app free for the canvas beside it.
fn inspector(ui: &mut egui::Ui, shown: &Shown, id: RoomId) {
    let Some(facts) = RoomFacts::gather(id, &shown.subset, &shown.layout) else {
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
        ui.strong(format!("Exits ({})", facts.exits.len()));
        for exit in &facts.exits {
            ui.horizontal_wrapped(|ui| {
                match &exit.command {
                    Some(command) => ui.label(command),
                    None => ui.label("-"),
                };
                match &exit.to_title {
                    Some(title) => ui.weak(format!("-> {title}")),
                    // Outside this area: the subset cannot name it, and
                    // saying so beats printing a bare id with no hint why.
                    None => ui.weak(format!("-> room {} (outside area)", exit.to.0)),
                };
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
            let name = store.custom_maps.get(plate).unwrap_or(plate);
            ui.label(format!("On: {name}"));
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
                for (plate, name) in &store.custom_maps {
                    if store.membership_moves.get(&key) == Some(plate) {
                        continue;
                    }
                    if ui.selectable_label(false, name).clicked() {
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
        let can_edit = self.store_path.is_some() && self.store_problem.is_none();

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
        if export_now {
            self.export_corrections();
        }

        let mut changed = false;
        egui::Panel::left("areas").show(ui, |ui| {
            changed = self.picker(ui);
        });
        if changed {
            self.show_selected();
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
                && ui
                    .button("Delete plate")
                    .on_hover_text("Release every room back to its own area")
                    .clicked()
            {
                *edit_out = Some(EditAction::DeletePlate { plate });
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
        .find(|(key, name)| name.as_str() == shown || key.as_str() == shown)
        .map(|(key, _)| key.clone())
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
