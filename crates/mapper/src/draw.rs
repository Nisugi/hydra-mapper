//! Turning a `cena_map_layout::MapScene` into `egui` shapes. Everything
//! about *what* to draw lives in `cena-map-layout`'s `Cell`/`SceneEdgeKind`
//! model; this module only decides pixels, colors and fonts.
//!
//! The canvas is a camera view, not a scroll pane: [`crate::camera`] owns
//! the cell-to-screen transform, and this module asks it rather than
//! placing anything at a fixed offset.

use cena_map::RoomId;
use cena_map_layout::MapScene;
use cena_map_layout::scene::{SceneEdgeKind, Sheet, SheetScene};
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

const ROOM_FILL: Color32 = Color32::from_rgb(60, 90, 130);
const ROOM_STROKE: Color32 = Color32::from_rgb(140, 180, 220);
const ENTRANCE_STROKE: Color32 = Color32::from_rgb(230, 170, 60);
const DIRECTIONAL_LINE: Color32 = Color32::from_rgb(120, 150, 180);
const CONNECTOR_LINE: Color32 = Color32::from_rgb(150, 120, 90);
const LABEL_COLOR: Color32 = Color32::from_rgb(220, 220, 200);
const CANVAS_BG: Color32 = Color32::from_rgb(24, 26, 30);
const SELECTED_STROKE: Color32 = Color32::from_rgb(250, 250, 250);
const HOVER_STROKE: Color32 = Color32::from_rgb(200, 220, 250);

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
    sheet: Sheet,
    camera: &mut Camera,
    selected: Option<RoomId>,
) -> Hit {
    let sheet = scene.sheet(sheet);
    if sheet.rooms.is_empty() {
        ui.label("This sheet has no rooms to show.");
        return Hit::default();
    }

    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let canvas = response.rect;
    painter.rect_filled(canvas, 0.0, CANVAS_BG);

    apply_input(ui, &response, camera);

    let hovered = response
        .hover_pos()
        .and_then(|pos| room_at(pos, sheet, *camera, canvas));
    // `clicked` is false after a drag, so panning across rooms does not
    // select whichever one the release happened over.
    let hit = Hit {
        clicked: response.clicked().then_some(hovered).flatten(),
    };

    // Clipped so a panned sheet does not paint over the panels beside it.
    let painter = painter.with_clip_rect(canvas);
    draw_edges(&painter, sheet, *camera, canvas);
    draw_rooms(&painter, sheet, *camera, canvas, selected, hovered);
    if camera.scale >= LABEL_MIN_SCALE {
        draw_labels(&painter, sheet, *camera, canvas);
    }
    if let Some(id) = hovered {
        hover_tooltip(&response, sheet, id);
    }
    hit
}

/// The room whose square contains `pos`, searched back to front so the
/// topmost drawn room wins where two sit in one cell.
fn room_at(pos: Pos2, sheet: &SheetScene, camera: Camera, canvas: Rect) -> Option<RoomId> {
    // Always at least a few pixels, so rooms stay clickable when zoomed
    // far out and the drawn square is tiny.
    let side = (ROOM_PX * camera.scale).max(6.0);
    sheet
        .rooms
        .iter()
        .rev()
        .find(|room| {
            Rect::from_center_size(camera.to_screen(room.cell, canvas), Vec2::splat(side))
                .contains(pos)
        })
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
fn apply_input(ui: &egui::Ui, response: &egui::Response, camera: &mut Camera) {
    if response.dragged() {
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

fn draw_edges(painter: &egui::Painter, sheet: &SheetScene, camera: Camera, canvas: Rect) {
    for edge in &sheet.edges {
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

fn draw_rooms(
    painter: &egui::Painter,
    sheet: &SheetScene,
    camera: Camera,
    canvas: Rect,
    selected: Option<RoomId>,
    hovered: Option<RoomId>,
) {
    let side = (ROOM_PX * camera.scale).max(2.0);
    for room in &sheet.rooms {
        let rect = Rect::from_center_size(camera.to_screen(room.cell, canvas), Vec2::splat(side));
        if !canvas.intersects(rect) {
            continue;
        }
        let is_selected = selected == Some(room.id);
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

fn draw_labels(painter: &egui::Painter, sheet: &SheetScene, camera: Camera, canvas: Rect) {
    let side = ROOM_PX * camera.scale;
    for label in &sheet.labels {
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
