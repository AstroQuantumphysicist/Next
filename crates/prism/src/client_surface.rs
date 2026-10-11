//! Copied, bounded client previews. This is not an input-delivery protocol.
use crate::{theme, Bounds, Canvas, PixelFormat};
pub const CAPACITY: usize = 2;
pub const MAX_BYTES: usize = 65536;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Slot,
    Stale,
    Identity,
    Geometry,
    Buffer,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseStatus {
    Unsupported,
    Ready,
    Requested,
    Closing,
    Refused,
    Unresponsive,
    Retry,
}
impl CloseStatus {
    pub fn requestable(self) -> bool {
        matches!(self, Self::Ready | Self::Refused | Self::Retry)
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unsupported => "Close unavailable",
            Self::Ready => "Native app",
            Self::Requested => "Close requested",
            Self::Closing => "App is closing",
            Self::Refused => "App kept window open",
            Self::Unresponsive => "App has not responded",
            Self::Retry => "Close request failed",
        }
    }
}
struct Surface {
    close_status: CloseStatus,
    generation: u64,
    frame_generation: u64,
    active: bool,
    content_valid: bool,
    configuration: u64,
    configuration_known: bool,
    retired_from: u64,
    placement: Option<Bounds>,
    minimized: bool,
    pid: u64,
    label: [u8; 64],
    label_len: usize,
    width: usize,
    height: usize,
    stride: usize,
    format: PixelFormat,
    pixels: [u8; MAX_BYTES],
}
impl Surface {
    const fn empty() -> Self {
        Self {
            close_status: CloseStatus::Unsupported,
            generation: 0,
            frame_generation: 0,
            active: false,
            content_valid: false,
            configuration: 1,
            configuration_known: false,
            retired_from: 0,
            placement: None,
            minimized: false,
            pid: 0,
            label: [0; 64],
            label_len: 0,
            width: 0,
            height: 0,
            stride: 0,
            format: PixelFormat::Rgb,
            pixels: [0; MAX_BYTES],
        }
    }
}
pub struct Previews {
    surfaces: [Surface; CAPACITY],
}
impl Default for Previews {
    fn default() -> Self {
        Self::new()
    }
}
impl Previews {
    pub const fn new() -> Self {
        Self {
            surfaces: [Surface::empty(), Surface::empty()],
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &mut self,
        slot: usize,
        generation: u64,
        frame_generation: u64,
        pid: u64,
        label: &str,
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
        pixels: &[u8],
    ) -> Result<(), Error> {
        let surface = self.surfaces.get_mut(slot).ok_or(Error::Slot)?;
        if generation == 0
            || frame_generation == 0
            || generation < surface.generation
            || (generation == surface.generation
                && (!surface.active || frame_generation <= surface.frame_generation))
        {
            return Err(Error::Stale);
        }
        if pid == 0
            || label.is_empty()
            || label.len() > 64
            || !label.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(Error::Identity);
        }
        if generation == surface.generation
            && (pid != surface.pid || label.as_bytes() != &surface.label[..surface.label_len])
        {
            return Err(Error::Identity);
        }
        if width == 0 || height == 0 || stride < width {
            return Err(Error::Geometry);
        }
        let bytes = stride
            .checked_mul(height)
            .and_then(|v| v.checked_mul(4))
            .ok_or(Error::Geometry)?;
        if bytes > MAX_BYTES || pixels.len() != bytes {
            return Err(Error::Buffer);
        }
        // Every validation precedes mutation. Copy directly into static-owned storage.
        if generation != surface.generation {
            surface.configuration = 1;
            surface.configuration_known = false;
            surface.retired_from = 0;
            surface.close_status = CloseStatus::Unsupported;
            surface.placement = None;
            surface.minimized = false;
        }
        surface.pixels[..bytes].copy_from_slice(pixels);
        surface.label[..label.len()].copy_from_slice(label.as_bytes());
        surface.label_len = label.len();
        surface.generation = generation;
        surface.frame_generation = frame_generation;
        surface.pid = pid;
        surface.width = width;
        surface.height = height;
        surface.stride = stride;
        surface.format = format;
        surface.active = true;
        surface.content_valid = true;
        Ok(())
    }
    pub fn configuration(&self, id: crate::client_interaction::SurfaceRef) -> Option<u64> {
        (self.identity(id.slot) == Some(id)).then(|| self.surfaces[id.slot].configuration)
    }
    pub fn geometry(&self, id: crate::client_interaction::SurfaceRef) -> Option<(u32, u32, u32)> {
        if self.identity(id.slot) != Some(id) {
            return None;
        }
        let s = &self.surfaces[id.slot];
        Some((s.width as u32, s.height as u32, s.stride as u32))
    }
    pub fn content_available(&self, id: crate::client_interaction::SurfaceRef) -> bool {
        self.identity(id.slot) == Some(id) && self.surfaces[id.slot].content_valid
    }
    pub fn set_configuration(
        &mut self,
        id: crate::client_interaction::SurfaceRef,
        configuration: u64,
    ) -> Result<(), Error> {
        if self.identity(id.slot) != Some(id) || configuration == 0 {
            return Err(Error::Stale);
        }
        let s = &mut self.surfaces[id.slot];
        if s.configuration_known && s.configuration != configuration {
            return Err(Error::Stale);
        }
        s.configuration = configuration;
        s.configuration_known = true;
        Ok(())
    }
    pub fn retire_content(
        &mut self,
        id: crate::client_interaction::SurfaceRef,
        expected: u64,
        new: u64,
    ) -> Result<(), Error> {
        if self.identity(id.slot) != Some(id) || new <= expected {
            return Err(Error::Stale);
        }
        let s = &mut self.surfaces[id.slot];
        if s.configuration == new && s.retired_from == expected && !s.content_valid {
            return Ok(());
        }
        if s.configuration != expected {
            return Err(Error::Stale);
        }
        s.retired_from = expected;
        s.configuration = new;
        s.configuration_known = true;
        s.content_valid = false;
        s.close_status = CloseStatus::Unsupported;
        Ok(())
    }
    pub(crate) fn accessible_identity(
        &self,
        id: crate::client_interaction::SurfaceRef,
    ) -> Option<(&str, u64)> {
        if self.identity(id.slot) != Some(id) {
            return None;
        }
        let s = &self.surfaces[id.slot];
        Some((core::str::from_utf8(&s.label[..s.label_len]).ok()?, s.pid))
    }
    pub fn set_close_status(
        &mut self,
        id: crate::client_interaction::SurfaceRef,
        status: CloseStatus,
    ) -> bool {
        if self.identity(id.slot) != Some(id) {
            return false;
        }
        self.surfaces[id.slot].close_status = status;
        true
    }
    pub fn close_status(&self, id: crate::client_interaction::SurfaceRef) -> Option<CloseStatus> {
        (self.identity(id.slot) == Some(id)).then(|| self.surfaces[id.slot].close_status)
    }
    pub(crate) fn set_placement(
        &mut self,
        id: crate::client_interaction::SurfaceRef,
        bounds: Bounds,
        minimized: bool,
    ) {
        if self.identity(id.slot) == Some(id) {
            self.surfaces[id.slot].placement = Some(bounds);
            self.surfaces[id.slot].minimized = minimized;
        }
    }
    pub fn minimize_button_bounds(&self, slot: usize, w: i32, h: i32) -> Option<Bounds> {
        let b = self.bounds(slot, w, h)?;
        Some(Bounds {
            x: b.x + b.width - 76,
            y: b.y + 8,
            width: 28,
            height: 28,
        })
    }
    pub fn close_button_bounds(&self, slot: usize, width: i32, height: i32) -> Option<Bounds> {
        let b = self.bounds(slot, width, height)?;
        Some(Bounds {
            x: b.x + b.width - 40,
            y: b.y + 8,
            width: 28,
            height: 28,
        })
    }
    pub fn clear(&mut self, slot: usize, generation: u64) -> Result<(), Error> {
        let surface = self.surfaces.get_mut(slot).ok_or(Error::Slot)?;
        if !surface.active || surface.generation != generation {
            return Err(Error::Stale);
        }
        surface.active = false;
        Ok(())
    }
    pub fn bounds(&self, slot: usize, width: i32, height: i32) -> Option<Bounds> {
        let surface = self.surfaces.get(slot)?;
        if !surface.active || surface.minimized || width < 320 || height < 240 {
            return None;
        }
        if let Some(b) = surface.placement {
            return Some(b);
        }
        let w = (width - 40).min(292);
        let h = ((height - 110) / 2).min(286);
        if h < 100 {
            return None;
        }
        Some(Bounds {
            x: width - w - 20,
            y: 76 + slot as i32 * (h + 8),
            width: w,
            height: h,
        })
    }
    pub fn identity(&self, slot: usize) -> Option<crate::client_interaction::SurfaceRef> {
        let s = self.surfaces.get(slot)?;
        s.active.then_some(crate::client_interaction::SurfaceRef {
            slot,
            generation: s.generation,
        })
    }
    pub fn content_bounds(&self, slot: usize, width: i32, height: i32) -> Option<Bounds> {
        let b = self.bounds(slot, width, height)?;
        let s = &self.surfaces[slot];
        let mut lines = 1;
        pixel_wrapped(
            core::str::from_utf8(&s.label[..s.label_len]).ok()?,
            b.width - 100,
            |_| lines += 1,
        );
        let mut bytes = [0; 64];
        use core::fmt::Write;
        let mut pid = Label {
            bytes: &mut bytes,
            len: 0,
        };
        write!(pid, "PID {}", s.pid).ok()?;
        pixel_wrapped(pid.text(), b.width - 100, |_| lines += 1);
        let y = (b.y + 88).max(b.y + 10 + lines * 18 + 8);
        let height = b.y + b.height - 12 - y;
        (height > 0 && b.width > 24).then_some(Bounds {
            x: b.x + 12,
            y,
            width: b.width - 24,
            height,
        })
    }
    pub fn pixel_bounds(&self, slot: usize, width: i32, height: i32) -> Option<Bounds> {
        if !self.surfaces.get(slot)?.content_valid {
            return None;
        }
        let mut b = self.content_bounds(slot, width, height)?;
        let s = &self.surfaces[slot];
        if b.width as usize * s.height <= b.height as usize * s.width {
            b.height = (b.width as usize * s.height / s.width).max(1) as i32;
        } else {
            b.width = (b.height as usize * s.width / s.height).max(1) as i32;
        }
        Some(b)
    }
    pub fn hit_content_for(
        &self,
        id: crate::client_interaction::SurfaceRef,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<(crate::client_interaction::SurfaceRef, i32, i32)> {
        if self.identity(id.slot) != Some(id) {
            return None;
        }
        let b = self.pixel_bounds(id.slot, width, height)?;
        if x < b.x || y < b.y || x >= b.x + b.width || y >= b.y + b.height {
            return None;
        }
        let s = &self.surfaces[id.slot];
        Some((
            id,
            ((x - b.x) as usize * s.width / b.width as usize) as i32,
            ((y - b.y) as usize * s.height / b.height as usize) as i32,
        ))
    }
    pub fn hit_content(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<(crate::client_interaction::SurfaceRef, i32, i32)> {
        (0..CAPACITY)
            .rev()
            .find_map(|slot| self.hit_content_for(self.identity(slot)?, x, y, width, height))
    }
    pub fn render(&self, canvas: &mut Canvas<'_>) {
        self.render_focused(canvas, None)
    }
    pub fn render_focused(
        &self,
        canvas: &mut Canvas<'_>,
        focused: Option<crate::client_interaction::SurfaceRef>,
    ) {
        self.render_interactive(canvas, focused, focused);
    }
    pub fn render_interactive(
        &self,
        canvas: &mut Canvas<'_>,
        focused: Option<crate::client_interaction::SurfaceRef>,
        context: Option<crate::client_interaction::SurfaceRef>,
    ) {
        self.render_selected(canvas, focused, context, None);
    }
    pub fn render_window(
        &self,
        canvas: &mut Canvas<'_>,
        id: crate::client_interaction::SurfaceRef,
        focused: Option<crate::client_interaction::SurfaceRef>,
        context: Option<crate::client_interaction::SurfaceRef>,
    ) {
        if self.identity(id.slot) == Some(id) {
            self.render_selected(canvas, focused, context, Some(id.slot));
        }
    }
    fn render_selected(
        &self,
        canvas: &mut Canvas<'_>,
        focused: Option<crate::client_interaction::SurfaceRef>,
        context: Option<crate::client_interaction::SurfaceRef>,
        wanted: Option<usize>,
    ) {
        for (slot, s) in self.surfaces.iter().enumerate() {
            if wanted.is_some_and(|wanted| wanted != slot) {
                continue;
            }
            let Some(b) = self.bounds(slot, canvas.width() as i32, canvas.height() as i32) else {
                continue;
            };
            theme::shadow(canvas, b, 10);
            theme::rounded_rect(canvas, b, 10, theme::SURFACE);
            theme::outline(
                canvas,
                b,
                10,
                if focused == self.identity(slot) {
                    theme::ACCENT
                } else {
                    theme::BORDER
                },
            );
            if let Some(min) =
                self.minimize_button_bounds(slot, canvas.width() as i32, canvas.height() as i32)
            {
                theme::rounded_rect(canvas, min, 5, theme::SURFACE_RAISED);
                theme::icon(
                    canvas,
                    theme::Icon::Minimize,
                    min.x + 6,
                    min.y + 6,
                    16,
                    theme::TEXT,
                );
            }
            if let Some(close) =
                self.close_button_bounds(slot, canvas.width() as i32, canvas.height() as i32)
            {
                theme::rounded_rect(canvas, close, 5, theme::SURFACE_RAISED);
                theme::icon(
                    canvas,
                    theme::Icon::Close,
                    close.x + 6,
                    close.y + 6,
                    16,
                    theme::TEXT,
                );
            }
            if s.content_valid {
                let grip = Bounds {
                    x: b.x + b.width - 14,
                    y: b.y + b.height - 14,
                    width: 12,
                    height: 12,
                };
                for d in [3, 6, 9] {
                    for i in 0..d {
                        canvas.pixel(grip.x + 11 - i, grip.y + 11 - (d - 1 - i), theme::MUTED);
                    }
                }
            }
            let label = core::str::from_utf8(&s.label[..s.label_len]).unwrap_or("");
            let mut next_y = b.y + 10;
            pixel_wrapped(label, b.width - 100, |part| {
                theme::text(
                    canvas,
                    b.x + 12,
                    next_y,
                    part,
                    theme::TEXT,
                    theme::Font::Small,
                );
                next_y += 18;
            });
            let mut pid = [0u8; 64];
            use core::fmt::Write;
            let mut writer = Label {
                bytes: &mut pid,
                len: 0,
            };
            write!(writer, "PID {}", s.pid).expect("u64 PID fits bounded label");
            pixel_wrapped(writer.text(), b.width - 100, |part| {
                theme::text(
                    canvas,
                    b.x + 12,
                    next_y,
                    part,
                    theme::MUTED,
                    theme::Font::Small,
                );
                next_y += 18;
            });
            theme::text(
                canvas,
                b.x + 12,
                next_y,
                if !s.content_valid {
                    "Waiting for app configuration"
                } else if context == self.identity(slot) && focused != self.identity(slot) {
                    "Click to resume input"
                } else {
                    s.close_status.label()
                },
                theme::MUTED,
                theme::Font::Small,
            );
            next_y += 18;
            let top = (b.y + 88).max(next_y + 8);
            let area = Bounds {
                x: b.x + 12,
                y: top,
                width: b.width - 24,
                height: (b.y + b.height - 12 - top).max(0),
            };
            if area.height == 0 {
                continue;
            }
            if !s.content_valid {
                theme::text(
                    canvas,
                    area.x,
                    area.y,
                    "Waiting for app to resize...",
                    theme::TEXT,
                    theme::Font::Small,
                );
                continue;
            }
            // Aspect-preserving nearest-neighbor preview; source format remains literal.
            let scale_w = area.width as usize;
            let scale_h = area.height as usize;
            let (dw, dh) = if scale_w * s.height <= scale_h * s.width {
                (scale_w, (scale_w * s.height / s.width).max(1))
            } else {
                ((scale_h * s.width / s.height).max(1), scale_h)
            };
            for y in 0..dh {
                for x in 0..dw {
                    let i = ((y * s.height / dh) * s.stride + x * s.width / dw) * 4;
                    let (r, g, blue) = match s.format {
                        PixelFormat::Rgb => (s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]),
                        PixelFormat::Bgr => (s.pixels[i + 2], s.pixels[i + 1], s.pixels[i]),
                    };
                    canvas.pixel_literal(
                        area.x + x as i32,
                        area.y + y as i32,
                        ((r as u32) << 16) | ((g as u32) << 8) | blue as u32,
                    );
                }
            }
        }
    }
}
// Authenticated package IDs are ASCII. Wrap by actual font advance, including
// wide letters, rather than assuming a byte count implies a pixel width.
fn pixel_wrapped(mut text: &str, width: i32, mut emit: impl FnMut(&str)) {
    while !text.is_empty() {
        let mut end = 0;
        for next in 1..=text.len() {
            if theme::text_width(&text[..next], theme::Font::Small) > width {
                break;
            }
            end = next;
        }
        if end == 0 {
            return;
        }
        emit(&text[..end]);
        text = &text[end..];
    }
}
struct Label<'a> {
    bytes: &'a mut [u8],
    len: usize,
}
impl Label<'_> {
    fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl core::fmt::Write for Label<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if s.len() > self.bytes.len() - self.len {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    #[test]
    fn generations_validate_before_mutation_and_clear_keeps_tombstone() {
        let mut p = Previews::new();
        let rgb = [255, 0, 0, 0];
        p.submit(0, 4, 1, 9, "org.next.app", 1, 1, 1, PixelFormat::Rgb, &rgb)
            .unwrap();
        assert_eq!(
            p.submit(0, 4, 1, 9, "org.next.app", 1, 1, 1, PixelFormat::Rgb, &rgb),
            Err(Error::Stale)
        );
        assert_eq!(
            p.submit(0, 4, 2, 9, "org.next.app", 1, 1, 0, PixelFormat::Rgb, &rgb),
            Err(Error::Geometry)
        );
        assert_eq!(p.surfaces[0].frame_generation, 1);
        p.submit(0, 4, 2, 9, "org.next.app", 1, 1, 1, PixelFormat::Rgb, &rgb)
            .unwrap();
        assert_eq!(p.clear(0, 3), Err(Error::Stale));
        p.clear(0, 4).unwrap();
        assert_eq!(
            p.submit(0, 4, 3, 9, "org.next.app", 1, 1, 1, PixelFormat::Rgb, &rgb),
            Err(Error::Stale)
        );
        p.submit(0, 5, 1, 10, "org.next.new", 1, 1, 1, PixelFormat::Rgb, &rgb)
            .unwrap();
        assert_eq!(p.clear(0, 4), Err(Error::Stale));
    }
    #[test]
    fn limits_reject_oversize_identity_and_invalid_slots() {
        let mut p = Previews::new();
        assert_eq!(
            p.submit(2, 1, 1, 1, "a", 1, 1, 1, PixelFormat::Rgb, &[0; 4]),
            Err(Error::Slot)
        );
        assert_eq!(
            p.submit(0, 1, 1, 0, "a", 1, 1, 1, PixelFormat::Rgb, &[0; 4]),
            Err(Error::Identity)
        );
        assert_eq!(
            p.submit(0, 1, 1, 1, "bad\nlabel", 1, 1, 1, PixelFormat::Rgb, &[0; 4]),
            Err(Error::Identity)
        );
        assert_eq!(
            p.submit(0, 1, 1, 1, "a", 128, 129, 128, PixelFormat::Rgb, &[]),
            Err(Error::Buffer)
        );
        assert_eq!(
            p.submit(
                0,
                1,
                1,
                1,
                "a",
                usize::MAX,
                2,
                usize::MAX,
                PixelFormat::Rgb,
                &[]
            ),
            Err(Error::Geometry)
        );
        let bytes = std::vec![0;MAX_BYTES];
        p.submit(0, 1, 1, 1, "a", 128, 128, 128, PixelFormat::Rgb, &bytes)
            .unwrap();
        assert!(p.bounds(0, 1280, 800).is_some());
        assert!(p.bounds(0, 240, 144).is_none());
    }
    #[test]
    fn actual_rgb_and_bgr_pixels_survive_palette_scaling_and_front_guards() {
        let mut p = Previews::new();
        p.submit(
            0,
            1,
            1,
            1,
            "org.next.rgb",
            2,
            2,
            3,
            PixelFormat::Rgb,
            &[
                255, 0, 0, 0, 0, 255, 0, 0, 99, 99, 99, 0, 0, 0, 255, 0, 255, 255, 255, 0, 99, 99,
                99, 0,
            ],
        )
        .unwrap();
        p.submit(
            1,
            1,
            1,
            2,
            "org.next.bgr",
            1,
            1,
            1,
            PixelFormat::Bgr,
            &[0, 0, 255, 0],
        )
        .unwrap();
        let mut pixels = std::vec![0xdeadbeef;1280*800+2];
        let mut canvas = Canvas::new(
            &mut pixels[1..1280 * 800 + 1],
            1280,
            800,
            1280,
            PixelFormat::Bgr,
        )
        .unwrap();
        canvas.set_palette(crate::palette::Palette::new(
            crate::appearance::Preferences {
                theme: crate::appearance::Theme::HighContrast,
                ..crate::appearance::Preferences::default()
            },
        ));
        p.render(&mut canvas);
        let b = p.bounds(0, 1280, 800).unwrap();
        let x = b.x as usize + 12;
        let y = b.y as usize + 88;
        assert_eq!(pixels[1 + y * 1280 + x], 0xff0000);
        assert_eq!(pixels[1 + y * 1280 + x + 150], 0x00ff00);
        assert_eq!(pixels[1 + (y + 180) * 1280 + x], 0x0000ff);
        let b = p.bounds(1, 1280, 800).unwrap();
        assert_eq!(
            pixels[1 + (b.y as usize + 88) * 1280 + b.x as usize + 12],
            0xff0000
        );
        assert_eq!(pixels[0], 0xdeadbeef);
        assert_eq!(*pixels.last().unwrap(), 0xdeadbeef);
    }
}
