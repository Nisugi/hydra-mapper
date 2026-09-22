//! The view onto a sheet: where it sits and how far in it is zoomed.
//!
//! The first cut drew into an `egui::ScrollArea` sized to the sheet's cell
//! bounds, which pinned every layout to the top-left of the canvas and
//! offered no way to move it but the scrollbars. A camera replaces that:
//! the sheet is drawn at an offset the person drags, at a scale they
//! wheel, and a new sheet starts centred and fitted rather than in a
//! corner.
//!
//! Kept apart from [`crate::draw`] so the transform has one home: `draw`
//! asks the camera to turn a [`Cell`] into a screen position and never
//! computes one itself.

use cena_map_layout::Cell;
use egui::{Pos2, Rect, Vec2};

/// Pixels per grid cell at scale 1.0. `cena-map-layout` works in an
/// abstract integer grid; this is the cell-to-pixel constant the camera
/// then scales.
pub const CELL_PX: f32 = 28.0;

/// How far in and out the wheel may go. The lower bound keeps a large
/// location readable rather than a smear of dots; the upper stops a single
/// room filling the window.
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 4.0;

/// Fitting a sheet leaves this much of the canvas as margin, so rooms at
/// the edge are not flush against it.
const FIT_MARGIN: f32 = 0.9;

/// A sheet never fits into less than this many cells' worth of canvas --
/// without it, a one-room location zooms to `MAX_SCALE` and looks broken.
const MIN_FIT_CELLS: f32 = 12.0;

/// Where the sheet sits under the canvas, and how far in.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// The grid cell drawn at the centre of the canvas.
    pub center: Vec2,
    /// Pixels per cell = `CELL_PX * scale`.
    pub scale: f32,
}

impl Default for Camera {
    fn default() -> Camera {
        Camera {
            center: Vec2::ZERO,
            scale: 1.0,
        }
    }
}

impl Camera {
    /// Pixels one cell spans at the current scale.
    pub fn cell_px(self) -> f32 {
        CELL_PX * self.scale
    }

    /// Where `cell` lands on screen, given the canvas it is drawn into.
    ///
    /// A map grid cell never approaches `f32`'s 24-bit mantissa (that would
    /// be a room count in the tens of millions), so the conversion loses
    /// nothing in practice.
    #[allow(clippy::cast_precision_loss)]
    pub fn to_screen(self, cell: Cell, canvas: Rect) -> Pos2 {
        let px = self.cell_px();
        canvas.center()
            + Vec2::new(
                (cell.x as f32 - self.center.x) * px,
                (cell.y as f32 - self.center.y) * px,
            )
    }

    /// The inverse: which cell coordinate sits under a screen position.
    /// Fractional on purpose -- zooming about the pointer needs the exact
    /// point under it, not the nearest room.
    pub fn to_cell(self, pos: Pos2, canvas: Rect) -> Vec2 {
        let px = self.cell_px();
        self.center + (pos - canvas.center()) / px
    }

    /// Centre on a sheet's cell bounds and zoom so the whole thing fits
    /// the canvas. What a freshly picked area gets, instead of the
    /// top-left corner.
    #[allow(clippy::cast_precision_loss)]
    pub fn fit(&mut self, min: Cell, max: Cell, canvas: Rect) {
        self.center = Vec2::new(
            f32::midpoint(min.x as f32, max.x as f32),
            f32::midpoint(min.y as f32, max.y as f32),
        );

        // `+ 1` because bounds are inclusive: a sheet from x=3 to x=5 is
        // three cells wide, not two.
        let cells_wide = ((max.x - min.x + 1) as f32).max(MIN_FIT_CELLS);
        let cells_tall = ((max.y - min.y + 1) as f32).max(MIN_FIT_CELLS);
        if canvas.width() <= 0.0 || canvas.height() <= 0.0 {
            return;
        }
        let fit_x = canvas.width() * FIT_MARGIN / (cells_wide * CELL_PX);
        let fit_y = canvas.height() * FIT_MARGIN / (cells_tall * CELL_PX);
        self.scale = fit_x.min(fit_y).clamp(MIN_SCALE, MAX_SCALE);
    }

    /// Put `cell` in the middle of the view, at the current zoom.
    #[allow(clippy::cast_precision_loss)]
    pub fn center_on(&mut self, cell: Cell) {
        self.center = Vec2::new(cell.x as f32, cell.y as f32);
    }

    /// Drag the sheet by a pointer movement in pixels.
    pub fn pan_by(&mut self, delta: Vec2) {
        self.center -= delta / self.cell_px();
    }

    /// Zoom by `factor` while holding whatever is under `anchor` in place,
    /// so the wheel zooms toward the pointer rather than the centre.
    pub fn zoom_at(&mut self, factor: f32, anchor: Pos2, canvas: Rect) {
        let before = self.to_cell(anchor, canvas);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let after = self.to_cell(anchor, canvas);
        self.center += before - after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(800.0, 600.0))
    }

    /// The cell the camera is centred on draws at the canvas centre -- the
    /// property the old top-left `ScrollArea` did not have.
    #[test]
    fn the_centre_cell_lands_at_the_canvas_centre() {
        let camera = Camera {
            center: Vec2::new(5.0, 7.0),
            ..Default::default()
        };
        let at = camera.to_screen(Cell { x: 5, y: 7 }, canvas());
        assert!((at - canvas().center()).length() < 0.001);
    }

    /// Fitting a sheet centres it, so an area off in negative coordinates
    /// is as visible as one at the origin.
    #[test]
    fn fit_centres_on_the_sheet_not_the_origin() {
        let mut camera = Camera::default();
        camera.fit(Cell { x: -30, y: -10 }, Cell { x: -10, y: 10 }, canvas());
        assert!((camera.center.x - -20.0).abs() < 0.001);
        assert!((camera.center.y - 0.0).abs() < 0.001);
    }

    /// A tiny sheet fits at a sane zoom rather than slamming into the
    /// maximum.
    #[test]
    fn a_single_room_does_not_zoom_to_the_limit() {
        let mut camera = Camera::default();
        camera.fit(Cell { x: 0, y: 0 }, Cell { x: 0, y: 0 }, canvas());
        assert!(camera.scale < MAX_SCALE);
    }

    /// Screen position and cell coordinate are inverses of each other.
    #[test]
    fn to_cell_inverts_to_screen() {
        let camera = Camera {
            center: Vec2::new(3.0, -2.0),
            scale: 1.7,
        };
        let cell = Cell { x: 11, y: 4 };
        let back = camera.to_cell(camera.to_screen(cell, canvas()), canvas());
        assert!((back.x - 11.0).abs() < 0.001 && (back.y - 4.0).abs() < 0.001);
    }

    /// Zooming about a point leaves what was under it under it still.
    #[test]
    fn zoom_holds_the_anchor_still() {
        let mut camera = Camera::default();
        let anchor = Pos2::new(600.0, 150.0);
        let before = camera.to_cell(anchor, canvas());
        camera.zoom_at(1.5, anchor, canvas());
        let after = camera.to_cell(anchor, canvas());
        assert!((before.x - after.x).abs() < 0.001);
        assert!((before.y - after.y).abs() < 0.001);
    }

    /// Zoom stays inside its bounds no matter how hard the wheel is spun.
    #[test]
    fn zoom_is_clamped_both_ways() {
        let mut camera = Camera::default();
        let anchor = canvas().center();
        for _ in 0..100 {
            camera.zoom_at(2.0, anchor, canvas());
        }
        assert!((camera.scale - MAX_SCALE).abs() < 0.001);
        for _ in 0..200 {
            camera.zoom_at(0.5, anchor, canvas());
        }
        assert!((camera.scale - MIN_SCALE).abs() < 0.001);
    }
}
