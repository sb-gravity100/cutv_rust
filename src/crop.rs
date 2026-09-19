// Crop tool: a resizable/movable rectangle drawn directly over the video
// texture in egui (no native overlay window needed — GStreamer's frames
// render as an egui texture, not a native child window like mpv used to,
// so egui can just draw on top of it — see PLAN.md's "Architecture
// decision"). Stored in SOURCE-pixel coordinates (not canvas/screen
// coordinates) since that's what both ffmpeg's `-vf crop=...` and the
// canvas<->source mapping need, and it stays correct across window resizes.

use egui::{pos2, vec2, Pos2, Rect, Vec2};

/// Crop region in source-pixel coordinates.
#[derive(Clone, Copy, Debug)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Smallest a crop rect is allowed to shrink to, in source pixels — purely
/// to keep it from collapsing to zero/negative size while dragging a handle.
const MIN_SIZE: f32 = 16.0;

impl CropRect {
    /// A sensible starting rect when the user first turns crop mode on:
    /// centered, 80% of the frame in each dimension.
    pub fn default_for(src_w: u32, src_h: u32) -> Self {
        let (sw, sh) = (src_w as f32, src_h as f32);
        let w = sw * 0.8;
        let h = sh * 0.8;
        CropRect { x: (sw - w) / 2.0, y: (sh - h) / 2.0, w, h }
    }

    pub fn clamp_to(&mut self, src_w: f32, src_h: f32) {
        self.w = self.w.clamp(MIN_SIZE, src_w);
        self.h = self.h.clamp(MIN_SIZE, src_h);
        self.x = self.x.clamp(0.0, src_w - self.w);
        self.y = self.y.clamp(0.0, src_h - self.h);
    }

    /// ffmpeg `-vf crop=w:h:x:y` value.
    pub fn to_vf(self) -> String {
        format!(
            "crop={}:{}:{}:{}",
            self.w.round() as i32, self.h.round() as i32,
            self.x.round() as i32, self.y.round() as i32,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Handle {
    Move,
    N, S, E, W,
    Ne, Nw, Se, Sw,
}

/// Maps a source-pixel crop rect onto the on-screen rect the video texture
/// is currently drawn into (`canvas`, e.g. from `ui_video`'s letterboxed
/// display rect).
pub fn to_canvas(canvas: Rect, src_w: u32, src_h: u32, r: CropRect) -> Rect {
    let sx = canvas.width() / src_w as f32;
    let sy = canvas.height() / src_h as f32;
    Rect::from_min_size(
        canvas.min + vec2(r.x * sx, r.y * sy),
        vec2(r.w * sx, r.h * sy),
    )
}

/// Inverse of `to_canvas` for a single point (used to hit-test the pointer).
pub fn canvas_to_source_delta(canvas: Rect, src_w: u32, src_h: u32, canvas_delta: Vec2) -> Vec2 {
    vec2(
        canvas_delta.x / canvas.width() * src_w as f32,
        canvas_delta.y / canvas.height() * src_h as f32,
    )
}

/// Which handle (if any) `p` (in canvas/screen space) is close enough to
/// interact with, given the crop rect's current canvas-space bounds.
/// Corners win over edges win over the body (so a corner is grabbable even
/// though it's technically also "near" two edges).
pub fn hit_test(canvas_crop: Rect, p: Pos2, handle_r: f32) -> Option<Handle> {
    let (l, r, t, b) = (canvas_crop.left(), canvas_crop.right(), canvas_crop.top(), canvas_crop.bottom());
    let near = |a: f32, v: f32| (a - v).abs() <= handle_r;
    let in_x = p.x >= l - handle_r && p.x <= r + handle_r;
    let in_y = p.y >= t - handle_r && p.y <= b + handle_r;

    let (near_l, near_r, near_t, near_b) = (near(p.x, l), near(p.x, r), near(p.y, t), near(p.y, b));
    if near_l && near_t { return Some(Handle::Nw); }
    if near_r && near_t { return Some(Handle::Ne); }
    if near_l && near_b { return Some(Handle::Sw); }
    if near_r && near_b { return Some(Handle::Se); }
    if near_t && in_x   { return Some(Handle::N); }
    if near_b && in_x   { return Some(Handle::S); }
    if near_l && in_y   { return Some(Handle::W); }
    if near_r && in_y   { return Some(Handle::E); }
    if canvas_crop.contains(p) { return Some(Handle::Move); }
    None
}

/// Applies a drag (already converted to source-pixel space) for the given
/// handle, clamping the result to stay within the source frame.
pub fn apply_drag(r: &mut CropRect, handle: Handle, delta: Vec2, src_w: f32, src_h: f32) {
    let west  = |r: &mut CropRect, dx: f32| { let nx = (r.x + dx).min(r.x + r.w - MIN_SIZE).max(0.0); r.w += r.x - nx; r.x = nx; };
    let east  = |r: &mut CropRect, dx: f32| { r.w = (r.w + dx).clamp(MIN_SIZE, src_w - r.x); };
    let north = |r: &mut CropRect, dy: f32| { let ny = (r.y + dy).min(r.y + r.h - MIN_SIZE).max(0.0); r.h += r.y - ny; r.y = ny; };
    let south = |r: &mut CropRect, dy: f32| { r.h = (r.h + dy).clamp(MIN_SIZE, src_h - r.y); };

    match handle {
        Handle::Move => {
            r.x = (r.x + delta.x).clamp(0.0, src_w - r.w);
            r.y = (r.y + delta.y).clamp(0.0, src_h - r.h);
        }
        Handle::N => north(r, delta.y),
        Handle::S => south(r, delta.y),
        Handle::E => east(r, delta.x),
        Handle::W => west(r, delta.x),
        Handle::Ne => { north(r, delta.y); east(r, delta.x); }
        Handle::Nw => { north(r, delta.y); west(r, delta.x); }
        Handle::Se => { south(r, delta.y); east(r, delta.x); }
        Handle::Sw => { south(r, delta.y); west(r, delta.x); }
    }
}

pub fn cursor_for(handle: Handle) -> egui::CursorIcon {
    use egui::CursorIcon as C;
    match handle {
        Handle::Move => C::Move,
        Handle::N | Handle::S => C::ResizeVertical,
        Handle::E | Handle::W => C::ResizeHorizontal,
        Handle::Ne | Handle::Sw => C::ResizeNeSw,
        Handle::Nw | Handle::Se => C::ResizeNwSe,
    }
}

/// Draws the crop overlay: dims everything outside the crop rect, then an
/// accent border and a small square at each handle position.
pub fn draw(painter: &egui::Painter, canvas: Rect, canvas_crop: Rect, accent: egui::Color32) {
    let dim = egui::Color32::from_black_alpha(140);
    // Four bands covering the area outside canvas_crop, clipped to canvas.
    painter.rect_filled(Rect::from_min_max(canvas.min, pos2(canvas.max.x, canvas_crop.min.y)), 0.0, dim); // top
    painter.rect_filled(Rect::from_min_max(pos2(canvas.min.x, canvas_crop.max.y), canvas.max), 0.0, dim); // bottom
    painter.rect_filled(Rect::from_min_max(pos2(canvas.min.x, canvas_crop.min.y), pos2(canvas_crop.min.x, canvas_crop.max.y)), 0.0, dim); // left
    painter.rect_filled(Rect::from_min_max(pos2(canvas_crop.max.x, canvas_crop.min.y), pos2(canvas.max.x, canvas_crop.max.y)), 0.0, dim); // right

    painter.rect_stroke(canvas_crop, 0.0, egui::Stroke::new(1.5, accent));

    let hs = 5.0; // handle half-size
    let pts = [
        canvas_crop.left_top(), canvas_crop.right_top(),
        canvas_crop.left_bottom(), canvas_crop.right_bottom(),
        pos2(canvas_crop.center().x, canvas_crop.top()), pos2(canvas_crop.center().x, canvas_crop.bottom()),
        pos2(canvas_crop.left(), canvas_crop.center().y), pos2(canvas_crop.right(), canvas_crop.center().y),
    ];
    for p in pts {
        painter.rect_filled(Rect::from_center_size(p, vec2(hs * 2.0, hs * 2.0)), 1.0, accent);
    }
}
