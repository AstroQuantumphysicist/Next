//! Crisp, high-contrast Prism pointer inspired by the Windows 11 arrow.
//!
//! This is original raster geometry; it does not embed Microsoft's cursor art.
//! A dark outline and white face remain legible over both light and dark UI.
use crate::Canvas;

const POINTS: [(i32, i32); 7] = [
    (0, 0),
    (0, 25),
    (7, 18),
    (12, 29),
    (17, 27),
    (12, 16),
    (23, 16),
];
const WIDTH: i32 = 24;
const HEIGHT: i32 = 30;

/// Paint the pointer last in the frame. Coordinates mark its top-left hot spot.
/// Canvas clipping handles pointers at the screen edges safely.
pub fn draw(canvas: &mut Canvas<'_>, x: i32, y: i32) {
    // A one-pixel cool shadow gives the black contour separation from the
    // wallpaper without soft or blurry edges at Prism's current 1x scale.
    for py in 0..HEIGHT {
        for px in 0..WIDTH {
            if inside(px - 1, py - 1) && !inside(px, py) {
                canvas.pixel_literal(x.saturating_add(px), y.saturating_add(py), 0x66758a);
            }
        }
    }

    for py in 0..HEIGHT {
        for px in 0..WIDTH {
            if inside(px, py) {
                let edge = (-1..=1).any(|dy| (-1..=1).any(|dx| !inside(px + dx, py + dy)));
                let color = if edge { 0x08090c } else { 0xffffff };
                canvas.pixel_literal(x.saturating_add(px), y.saturating_add(py), color);
            }
        }
    }
}

fn inside(px: i32, py: i32) -> bool {
    // Evaluate pixel centers against a small integer polygon. The doubled
    // coordinates keep the raster deterministic without floating point.
    let px = px * 2 + 1;
    let py = py * 2 + 1;
    let mut hit = false;
    for index in 0..POINTS.len() {
        let (ax, ay) = POINTS[index];
        let (bx, by) = POINTS[(index + 1) % POINTS.len()];
        let (ax, ay, bx, by) = (ax * 2, ay * 2, bx * 2, by * 2);
        if (ay > py) != (by > py) {
            let lhs = (px - ax) * (by - ay);
            let rhs = (py - ay) * (bx - ax);
            let crosses_left = if by > ay { lhs < rhs } else { lhs > rhs };
            if crosses_left {
                hit = !hit;
            }
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_has_white_face_dark_outline_and_focused_hotspot() {
        extern crate std;
        let mut pixels = std::vec![0u32; 48 * 48];
        let mut canvas = Canvas::new(&mut pixels, 48, 48, 48, crate::PixelFormat::Bgr).unwrap();
        draw(&mut canvas, 8, 7);
        let at = |x: usize, y: usize| pixels[y * 48 + x];
        assert_eq!(at(8, 7), 0x08090c, "hotspot has a dark outline");
        assert_eq!(at(11, 17), 0xffffff, "arrow face is white");
        assert_eq!(at(8, 19), 0x08090c, "left edge remains outlined");
        assert_eq!(at(33, 28), 0, "drawing stays within pointer bounds");
    }

    #[test]
    fn pointer_clips_at_every_canvas_edge() {
        extern crate std;
        let mut pixels = std::vec![0u32; 12 * 12];
        let mut canvas = Canvas::new(&mut pixels, 12, 12, 12, crate::PixelFormat::Bgr).unwrap();
        for (x, y) in [(-100, -100), (11, 0), (0, 11), (i32::MAX, i32::MAX)] {
            draw(&mut canvas, x, y);
        }
        assert_eq!(pixels.len(), 144);
    }
}
