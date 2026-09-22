//! Turning a `cena_map_layout::MapScene` into `egui` shapes. Everything
//! about *what* to draw lives in `cena-map-layout`'s `Cell`/`SceneEdgeKind`
//! model; this module only decides pixels, colors and fonts.
//!
//! The canvas is a camera view, not a scroll pane: [`crate::camera`] owns
//! the cell-to-screen transform, and this module asks it rather than
//! placing anything at a fixed offset.

use std::collections::HashSet;

use cena_map::RoomId;
use cena_map_layout::Cell;
use cena_map_layout::MapScene;
use cena_map_layout::scene::{SceneEdgeKind, SheetScene};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::camera::Camera;

/// Room square side in pixels at scale 1.0, smaller than a cell so
/// adjacent rooms don't touch.
const ROOM_PX: f32 = 18.0;

/// Below this scale a room is a few pixels across, so labels stop drawing
/// rather than piling into an unreadable smear.
const LABEL_MIN_SCALE: f32 = 0.55;

/// How fast the wheel zooms, per notch.
const ZOOM_PER_NOTCH: f32 = 1.0015;

pub(crate) const ROOM_FILL: Color32 = Color32::from_rgb(60, 90, 130);
pub(crate) const ROOM_STROKE: Color32 = Color32::from_rgb(140, 180, 220);
pub(crate) const ENTRANCE_STROKE: Color32 = Color32::from_rgb(230, 170, 60);
/// A room out of focus: the doorway amber, dimmed well below the rooms in
/// focus, so the rest of the area reads as road under the squares rather
/// than as a field of bright points over them.
pub(crate) const ECHO_DOT: Color32 = Color32::from_rgb(120, 90, 35);
pub(crate) const DIRECTIONAL_LINE: Color32 = Color32::from_rgb(120, 150, 180);
pub(crate) const CONNECTOR_LINE: Color32 = Color32::from_rgb(150, 120, 90);
pub(crate) const LABEL_COLOR: Color32 = Color32::from_rgb(220, 220, 200);
pub(crate) const CANVAS_BG: Color32 = Color32::from_rgb(24, 26, 30);
const SELECTED_STROKE: Color32 = Color32::from_rgb(250, 250, 250);
const HOVER_STROKE: Color32 = Color32::from_rgb(200, 220, 250);
const GHOST_STROKE: Color32 = Color32::from_rgb(250, 220, 120);

/// What the pointer did over the canvas this frame, for the caller to act
/// on: the canvas itself owns no selection state.
///
/// Only the click is reported. Hovering is handled here, as a tooltip, so
/// the caller never needs to know about it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hit {
    /// The room just clicked. `None` on a drag, so panning never changes
    /// the selection.
    pub clicked: Option<RoomId>,
    /// In edit mode: the room a drag just started on, and whether Alt was
    /// held (move one room rather than its whole group).
    pub drag_started: Option<(RoomId, bool)>,
    /// In edit mode: pixels dragged this frame, to accumulate.
    pub dragged_by: Option<egui::Vec2>,
    /// In edit mode: the drag ended this frame; commit it.
    pub drag_stopped: bool,
}

/// How the canvas behaves and what it draws, beyond the sheet itself.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Dragging moves rooms instead of panning.
    pub edit_mode: bool,
    /// Room titles are drawn beside the rooms.
    pub labels: bool,
    /// Every interior room out of focus is drawn as a dot, not only the
    /// door you enter its building by.
    pub interiors: bool,
}

/// What is in focus: the rooms drawn as squares. Everything else on the
/// sheet is a dot on the same roads, so the area stays one continuous
/// map whichever part of it is being looked at.
pub struct Focus<'a> {
    pub rooms: &'a HashSet<RoomId>,
    /// The street rooms: always drawn, as squares or as dots, because
    /// they are the roads the rest hangs off.
    pub streets: &'a HashSet<RoomId>,
    /// Rooms a door leads into from outside their unit, and the street
    /// rooms hosting a doorway: drawn as larger dots when out of focus,
    /// so the ways in are visible.
    pub doors: &'a HashSet<RoomId>,
}

impl Focus<'_> {
    fn has(&self, id: RoomId) -> bool {
        self.rooms.contains(&id)
    }

    /// Whether a room is drawn at all. In focus, always; out of focus,
    /// only the streets and the doors -- a building not being looked at
    /// is one dot where you enter it, not its floor plan sprinkled beside
    /// the street.
    fn shows(&self, id: RoomId, interiors: bool) -> bool {
        interiors || self.has(id) || self.streets.contains(&id) || self.doors.contains(&id)
    }
}

/// Draw one sheet into a pannable, zoomable canvas filling the panel,
/// apply whatever drag and wheel input lands on it, and report what the
/// pointer touched.
///
/// `camera` is borrowed mutably because the same gesture that draws the
/// frame also moves the view: egui reports the drag on the response the
/// painter is allocated from, so there is no earlier point to handle it.
pub fn scene(
    ui: &mut egui::Ui,
    scene: &MapScene,
    focus: &Focus<'_>,
    camera: &mut Camera,
    selected: Option<RoomId>,
    view: View,
    ghost: Option<(usize, Option<RoomId>, Cell)>,
) -> Hit {
    let View {
        edit_mode,
        labels,
        interiors,
    } = view;
    let sheet = &scene.sheet;
    if sheet.rooms.is_empty() {
        ui.label("This sheet has no rooms to show.");
        return Hit::default();
    }

    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let canvas = response.rect;
    painter.rect_filled(canvas, 0.0, CANVAS_BG);

    // In edit mode a drag moves rooms, so the view must not pan with it.
    apply_input(ui, &response, camera, edit_mode);

    let hovered = response
        .hover_pos()
        .and_then(|pos| room_at(pos, sheet, focus, interiors, *camera, canvas));
    // `clicked` is false after a drag, so panning across rooms does not
    // select whichever one the release happened over.
    let mut hit = Hit {
        clicked: response.clicked().then_some(hovered).flatten(),
        ..Hit::default()
    };
    if edit_mode {
        if response.drag_started() {
            hit.drag_started = hovered.map(|id| (id, ui.input(|i| i.modifiers.alt)));
        }
        if response.dragged() {
            hit.dragged_by = Some(response.drag_delta());
        }
        hit.drag_stopped = response.drag_stopped();
    }

    // Clipped so a panned sheet does not paint over the panels beside it.
    let painter = painter.with_clip_rect(canvas);
    draw_edges(&painter, sheet, focus, *camera, canvas);
    draw_rooms(
        &painter, sheet, focus, interiors, *camera, canvas, selected, hovered,
    );
    if labels && camera.scale >= LABEL_MIN_SCALE {
        draw_labels(&painter, scene, focus, *camera, canvas);
    }
    if let Some((group, room, delta)) = ghost {
        draw_ghost(&painter, sheet, *camera, canvas, group, room, delta);
    }
    if let Some(id) = hovered {
        hover_tooltip(&response, sheet, id);
    }
    hit
}

/// Where a drag would land, previewed as outlines while the mouse is down,
/// so a move is aimed rather than guessed and undone.
#[allow(clippy::cast_precision_loss)] // a drag delta is a handful of cells
fn draw_ghost(
    painter: &egui::Painter,
    sheet: &SheetScene,
    camera: Camera,
    canvas: Rect,
    group: usize,
    room: Option<RoomId>,
    delta: Cell,
) {
    let side = (ROOM_PX * camera.scale).max(2.0);
    let shift = Vec2::new(
        delta.x as f32 * camera.cell_px(),
        delta.y as f32 * camera.cell_px(),
    );
    for scene_room in &sheet.rooms {
        let moving = match room {
            Some(id) => scene_room.id == id,
            None => scene_room.group == group,
        };
        if !moving {
            continue;
        }
        let at = camera.to_screen(scene_room.cell, canvas) + shift;
        painter.rect_stroke(
            Rect::from_center_size(at, Vec2::splat(side)),
            2.0,
            Stroke::new(1.5, GHOST_STROKE),
            StrokeKind::Outside,
        );
    }
}

/// The room whose square contains `pos`, searched back to front so the
/// topmost drawn room wins where two sit in one cell.
fn room_at(
    pos: Pos2,
    sheet: &SheetScene,
    focus: &Focus<'_>,
    interiors: bool,
    camera: Camera,
    canvas: Rect,
) -> Option<RoomId> {
    // Always at least a few pixels, so rooms stay clickable when zoomed
    // far out and the drawn square is tiny.
    let side = (ROOM_PX * camera.scale).max(6.0);
    let hit = |cell: Cell| {
        Rect::from_center_size(camera.to_screen(cell, canvas), Vec2::splat(side)).contains(pos)
    };
    sheet
        .rooms
        .iter()
        .rev()
        .find(|room| focus.shows(room.id, interiors) && hit(room.cell))
        .map(|room| room.id)
}

/// Title and id under the pointer, so a room identifies itself without a
/// click.
fn hover_tooltip(response: &egui::Response, sheet: &SheetScene, id: RoomId) {
    let Some(room) = sheet.rooms.iter().find(|r| r.id == id) else {
        return;
    };
    response.clone().on_hover_ui(|ui| {
        if room.title.is_empty() {
            ui.label(format!("Room {}", id.0));
        } else {
            ui.strong(&room.title);
            ui.label(format!("Room {}", id.0));
        }
        if !room.service_tags.is_empty() {
            ui.label(room.service_tags.join(", "));
        }
    });
}

/// Drag to pan, wheel to zoom about the pointer. Zoom anchors on the
/// pointer rather than the centre so wheeling toward a corner of a town
/// walks into it instead of away.
fn apply_input(ui: &egui::Ui, response: &egui::Response, camera: &mut Camera, edit_mode: bool) {
    if response.dragged() && !edit_mode {
        camera.pan_by(response.drag_delta());
    }

    if !response.hovered() {
        return;
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll == 0.0 {
        return;
    }
    let anchor = ui
        .input(|i| i.pointer.latest_pos())
        .unwrap_or_else(|| response.rect.center());
    camera.zoom_at(ZOOM_PER_NOTCH.powf(scroll), anchor, response.rect);
}

/// Roads and doors always; the inside of a building only when both ends
/// are in focus, so a town's worth of interiors is not drawn over the
/// streets.
fn draw_edges(
    painter: &egui::Painter,
    sheet: &SheetScene,
    focus: &Focus<'_>,
    camera: Camera,
    canvas: Rect,
) {
    for edge in &sheet.edges {
        if edge.unit.is_some() && !(focus.has(edge.a_room) && focus.has(edge.b_room)) {
            continue;
        }
        let a = camera.to_screen(edge.a, canvas);
        let b = camera.to_screen(edge.b, canvas);
        // Scaled so lines thin out as the view pulls back, but never to
        // nothing.
        let width = (camera.scale * 1.5).max(0.5);
        let stroke = match edge.kind {
            SceneEdgeKind::Directional | SceneEdgeKind::Stub => {
                Stroke::new(width, DIRECTIONAL_LINE)
            }
            SceneEdgeKind::Connector => Stroke::new(width * 0.7, CONNECTOR_LINE),
        };
        painter.line_segment([a, b], stroke);
    }
}

/// A room in focus is a square; one out of focus is a dot on the road --
/// larger where a door leads in -- so the whole area is always there to
/// see, and the part being looked at stands out from it.
#[allow(clippy::too_many_arguments)] // one call site, each a distinct fact
fn draw_rooms(
    painter: &egui::Painter,
    sheet: &SheetScene,
    focus: &Focus<'_>,
    interiors: bool,
    camera: Camera,
    canvas: Rect,
    selected: Option<RoomId>,
    hovered: Option<RoomId>,
) {
    let side = (ROOM_PX * camera.scale).max(2.0);
    for room in &sheet.rooms {
        if !focus.shows(room.id, interiors) {
            continue;
        }
        let centre = camera.to_screen(room.cell, canvas);
        let rect = Rect::from_center_size(centre, Vec2::splat(side));
        if !canvas.intersects(rect) {
            continue;
        }
        let is_selected = selected == Some(room.id);
        if !focus.has(room.id) {
            let r = if focus.doors.contains(&room.id) {
                (side * 0.3).max(2.5)
            } else {
                (side * 0.18).max(1.5)
            };
            painter.circle_filled(centre, r, ECHO_DOT);
            if is_selected || hovered == Some(room.id) {
                let color = if is_selected {
                    SELECTED_STROKE
                } else {
                    HOVER_STROKE
                };
                painter.circle_stroke(centre, r + 2.0, Stroke::new(1.0, color));
            }
            continue;
        }
        let (stroke_color, width) = if is_selected {
            (SELECTED_STROKE, (camera.scale * 2.5).max(2.0))
        } else if hovered == Some(room.id) {
            (HOVER_STROKE, (camera.scale * 2.0).max(1.5))
        } else if room.entrance {
            (ENTRANCE_STROKE, (camera.scale * 1.5).max(0.5))
        } else {
            (ROOM_STROKE, (camera.scale * 1.5).max(0.5))
        };
        painter.rect(
            rect,
            2.0 * camera.scale,
            ROOM_FILL,
            Stroke::new(width, stroke_color),
            StrokeKind::Outside,
        );
        // A halo, so the selected room is findable even zoomed out far
        // enough that its square is a couple of pixels.
        if is_selected {
            painter.rect_stroke(
                rect.expand(side.max(6.0) * 0.6),
                2.0,
                Stroke::new(1.0, SELECTED_STROKE),
                StrokeKind::Outside,
            );
        }
    }
}

/// The names of the buildings in focus.
fn draw_labels(
    painter: &egui::Painter,
    scene: &MapScene,
    focus: &Focus<'_>,
    camera: Camera,
    canvas: Rect,
) {
    let side = ROOM_PX * camera.scale;
    for label in &scene.sheet.labels {
        let in_focus = label
            .unit
            .and_then(|u| scene.units.get(u))
            .and_then(|u| u.rooms.first())
            .is_some_and(|&id| focus.has(id));
        if !in_focus {
            continue;
        }
        let pos = camera.to_screen(label.cell, canvas) - Vec2::new(side / 2.0, side / 2.0 + 2.0);
        if !canvas.contains(pos) {
            continue;
        }
        painter.text(
            pos,
            Align2::LEFT_BOTTOM,
            &label.text,
            FontId::proportional(12.0_f32.max(12.0 * camera.scale)),
            LABEL_COLOR,
        );
    }
}
