//! Turning a `cena_map_layout::MapScene` into `egui` shapes. Everything
//! about *what* to draw lives in `cena-map-layout`'s `Cell`/`SceneEdgeKind`
//! model; this module only decides pixels, colors and fonts.

use cena_map_layout::scene::{SceneEdgeKind, Sheet};
use cena_map_layout::{Cell, MapScene};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

/// Pixels per grid cell. `cena-map-layout` works in an abstract integer
/// grid (`Cell`); this is the one place that turns a cell into a screen
/// position, so pan/zoom later has one function to change.
const CELL_PX: f32 = 28.0;
/// Room square side, smaller than [`CELL_PX`] so adjacent rooms don't
/// touch.
const ROOM_PX: f32 = 18.0;

const ROOM_FILL: Color32 = Color32::from_rgb(60, 90, 130);
const ROOM_STROKE: Color32 = Color32::from_rgb(140, 180, 220);
const ENTRANCE_STROKE: Color32 = Color32::from_rgb(230, 170, 60);
const DIRECTIONAL_LINE: Color32 = Color32::from_rgb(120, 150, 180);
const CONNECTOR_LINE: Color32 = Color32::from_rgb(150, 120, 90);
const LABEL_COLOR: Color32 = Color32::from_rgb(220, 220, 200);

/// Which sheet to draw. v1 always shows the outdoor sheet: a location's
/// interiors shelf matters once the window has a way to switch between
/// them, which is not built yet (`plan/26` §5 names it as follow-on work).
const SHOWN: Sheet = Sheet::Outdoor;

/// A map grid cell never approaches `f32`'s 24-bit mantissa (a room count
/// in the tens of millions), so the conversion loses nothing in practice.
#[allow(clippy::cast_precision_loss)]
fn to_pos(cell: Cell, origin: Pos2) -> Pos2 {
    Pos2 {
        x: origin.x + cell.x as f32 * CELL_PX,
        y: origin.y + cell.y as f32 * CELL_PX,
    }
}

/// Draw one sheet of `scene` into a scrollable, pannable canvas sized to
/// fill the rest of the panel.
pub fn scene(ui: &mut egui::Ui, scene: &MapScene) {
    let sheet = scene.sheet(SHOWN);
    if sheet.rooms.is_empty() {
        ui.label("This location has no outdoor rooms to show.");
        return;
    }

    let cells_wide = f64::from(sheet.max.x - sheet.min.x + 1);
    let cells_tall = f64::from(sheet.max.y - sheet.min.y + 1);
    #[allow(clippy::cast_possible_truncation)]
    let size = Vec2::new(
        (cells_wide * f64::from(CELL_PX)) as f32 + ROOM_PX,
        (cells_tall * f64::from(CELL_PX)) as f32 + ROOM_PX,
    );

    egui::ScrollArea::both().show(ui, |ui| {
        let (response, painter) = ui.allocate_painter(size, Sense::hover());
        // Half a room-square in from the top-left, and shifted so the
        // sheet's own minimum cell (which may be negative) lands inside
        // the allocated rect rather than off it.
        let min_offset = to_pos(sheet.min, Pos2::ZERO);
        let origin = Pos2 {
            x: response.rect.min.x + ROOM_PX / 2.0 - min_offset.x,
            y: response.rect.min.y + ROOM_PX / 2.0 - min_offset.y,
        };

        for edge in &sheet.edges {
            let a = to_pos(edge.a, origin);
            let b = to_pos(edge.b, origin);
            let stroke = match edge.kind {
                SceneEdgeKind::Directional | SceneEdgeKind::Stub => {
                    Stroke::new(1.5, DIRECTIONAL_LINE)
                }
                SceneEdgeKind::Connector => Stroke::new(1.0, CONNECTOR_LINE),
            };
            painter.line_segment([a, b], stroke);
        }

        for room in &sheet.rooms {
            let center = to_pos(room.cell, origin);
            let rect = Rect::from_center_size(center, Vec2::splat(ROOM_PX));
            let stroke_color = if room.entrance {
                ENTRANCE_STROKE
            } else {
                ROOM_STROKE
            };
            painter.rect(
                rect,
                2.0,
                ROOM_FILL,
                Stroke::new(1.5, stroke_color),
                StrokeKind::Outside,
            );
        }

        for label in &sheet.labels {
            let pos = to_pos(label.cell, origin) - Vec2::new(ROOM_PX / 2.0, ROOM_PX / 2.0 + 2.0);
            painter.text(
                pos,
                Align2::LEFT_BOTTOM,
                &label.text,
                FontId::proportional(12.0),
                LABEL_COLOR,
            );
        }
    });
}
