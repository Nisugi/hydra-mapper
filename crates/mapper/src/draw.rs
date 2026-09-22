//! Turning a `cena_map_layout::MapScene` into `egui` shapes. Everything
//! about *what* to draw lives in `cena-map-layout`'s `Cell`/`SceneEdgeKind`
//! model; this module only decides pixels, colors and fonts.
//!
//! The canvas is a camera view, not a scroll pane: [`crate::camera`] owns
//! the cell-to-screen transform, and this module asks it rather than
//! placing anything at a fixed offset.

use cena_map_layout::MapScene;
use cena_map_layout::scene::{SceneEdgeKind, Sheet, SheetScene};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};

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

/// Draw one sheet into a pannable, zoomable canvas filling the panel, and
/// apply whatever drag and wheel input lands on it.
///
/// `camera` is borrowed mutably because the same gesture that draws the
/// frame also moves the view: egui reports the drag on the response the
/// painter is allocated from, so there is no earlier point to handle it.
pub fn scene(ui: &mut egui::Ui, scene: &MapScene, sheet: Sheet, camera: &mut Camera) {
    let sheet = scene.sheet(sheet);
    if sheet.rooms.is_empty() {
        ui.label("This sheet has no rooms to show.");
        return;
    }

    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let canvas = response.rect;
    painter.rect_filled(canvas, 0.0, CANVAS_BG);

    apply_input(ui, &response, camera);

    // Clipped so a panned sheet does not paint over the panels beside it.
    let painter = painter.with_clip_rect(canvas);
    draw_edges(&painter, sheet, *camera, canvas);
    draw_rooms(&painter, sheet, *camera, canvas);
    if camera.scale >= LABEL_MIN_SCALE {
        draw_labels(&painter, sheet, *camera, canvas);
    }
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

fn draw_rooms(painter: &egui::Painter, sheet: &SheetScene, camera: Camera, canvas: Rect) {
    let side = (ROOM_PX * camera.scale).max(2.0);
    for room in &sheet.rooms {
        let rect = Rect::from_center_size(camera.to_screen(room.cell, canvas), Vec2::splat(side));
        if !canvas.intersects(rect) {
            continue;
        }
        let stroke_color = if room.entrance {
            ENTRANCE_STROKE
        } else {
            ROOM_STROKE
        };
        painter.rect(
            rect,
            2.0 * camera.scale,
            ROOM_FILL,
            Stroke::new((camera.scale * 1.5).max(0.5), stroke_color),
            StrokeKind::Outside,
        );
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
