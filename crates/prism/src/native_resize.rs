//! Bounded trusted resize preview. No app configuration changes before commit.
use crate::{client_interaction::SurfaceRef, Bounds};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResizeError {
    Retry,
    Stale,
    Denied,
    Invalid,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeResizeAction {
    pub request: u64,
    pub surface: SurfaceRef,
    pub expected_configuration: u64,
    pub bounds: Bounds,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}
#[derive(Clone, Copy)]
struct Gesture {
    action: NativeResizeAction,
    original: Bounds,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    content: (i32, i32),
}
pub struct Resize {
    gesture: Option<Gesture>,
    released: bool,
    taken: bool,
    error: Option<ResizeError>,
    next: u64,
}
impl Resize {
    pub const fn new() -> Self {
        Self {
            gesture: None,
            released: false,
            taken: false,
            error: None,
            next: 1,
        }
    }
    pub fn preview(&self) -> Option<NativeResizeAction> {
        self.gesture.map(|g| g.action)
    }
    pub fn dragging(&self) -> bool {
        self.gesture.is_some() && !self.released
    }
    pub fn error(&self) -> Option<ResizeError> {
        self.error
    }
    pub fn begin(
        &mut self,
        surface: SurfaceRef,
        configuration: u64,
        bounds: Bounds,
        geometry: (u32, u32),
        pointer: (i32, i32),
        content: (i32, i32),
    ) -> bool {
        let (width, height) = geometry;
        let (x, y) = pointer;
        if surface.slot >= 2
            || surface.generation == 0
            || width == 0
            || height == 0
            || width
                .checked_mul(height)
                .and_then(|n| n.checked_mul(4))
                .is_none_or(|n| n > 65536)
            || bounds.width <= 0
            || bounds.height <= 0
            || content.0 <= 0
            || content.1 <= 0
            || configuration == 0
            || self.gesture.is_some()
            || self.next == u64::MAX
        {
            return false;
        }
        self.gesture = Some(Gesture {
            action: NativeResizeAction {
                request: self.next,
                surface,
                expected_configuration: configuration,
                bounds,
                width,
                height,
                stride: width * 4,
            },
            original: bounds,
            x,
            y,
            width,
            height,
            content,
        });
        self.next += 1;
        self.released = false;
        self.taken = false;
        self.error = None;
        true
    }
    pub fn motion(&mut self, x: i32, y: i32, viewport_width: i32, viewport_height: i32) {
        if !self.dragging() {
            return;
        }
        let g = self.gesture.as_mut().unwrap();
        let dx = x.saturating_sub(g.x);
        let dy = y.saturating_sub(g.y);
        if dx == 0 && dy == 0 {
            return;
        }
        g.action.bounds.width = g.original.width.saturating_add(dx).clamp(
            292,
            viewport_width
                .saturating_sub(g.original.x)
                .saturating_sub(4)
                .max(292),
        );
        g.action.bounds.height = g.original.height.saturating_add(dy).clamp(
            240,
            viewport_height
                .saturating_sub(g.original.y)
                .saturating_sub(84)
                .max(240),
        );
        // This first profile exposes a bounded 16..128-pixel client viewport.
        g.action.width = (i64::from(g.width)
            + i64::from(dx) * i64::from(g.width) / i64::from(g.content.0))
        .clamp(16, 128) as u32;
        g.action.height = (i64::from(g.height)
            + i64::from(dy) * i64::from(g.height) / i64::from(g.content.1))
        .clamp(16, 128) as u32;
        g.action.stride = g.action.width * 4;
    }
    pub fn release(&mut self) {
        if let Some(g) = self.gesture {
            if g.action.bounds == g.original
                && g.action.width == g.width
                && g.action.height == g.height
            {
                self.cancel();
            } else {
                self.released = true;
            }
        }
    }
    pub fn take(&mut self) -> Option<NativeResizeAction> {
        if !self.released || self.taken || self.error.is_some() {
            return None;
        }
        self.taken = true;
        self.preview()
    }
    pub fn pending(&self) -> bool {
        self.taken && self.error.is_none()
    }
    pub fn failed(&mut self, request: u64, error: ResizeError) -> bool {
        if !self.taken || self.preview().is_none_or(|a| a.request != request) {
            return false;
        }
        self.error = Some(error);
        true
    }
    pub fn retry(&mut self) -> bool {
        if self.error != Some(ResizeError::Retry) || self.next == u64::MAX {
            return false;
        }
        let Some(g) = self.gesture.as_mut() else {
            return false;
        };
        g.action.request = self.next;
        self.next += 1;
        self.error = None;
        self.taken = false;
        true
    }
    pub fn cancel(&mut self) {
        self.gesture = None;
        self.released = false;
        self.taken = false;
        self.error = None;
    }
    pub fn complete(&mut self, request: u64) -> Option<NativeResizeAction> {
        let a = self.preview()?;
        if !self.taken || a.request != request {
            return None;
        }
        self.cancel();
        Some(a)
    }
}
impl Default for Resize {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> SurfaceRef {
        SurfaceRef {
            slot: 0,
            generation: 1,
        }
    }
    fn bounds() -> Bounds {
        Bounds {
            x: 4,
            y: 4,
            width: 292,
            height: 240,
        }
    }
    #[test]
    fn simple_grip_click_does_not_reconfigure_unchanged_content() {
        let mut r = Resize::new();
        assert!(r.begin(id(), 1, bounds(), (134, 70), (5, 5), (268, 140)));
        r.motion(5, 5, 640, 480);
        r.release();
        assert!(r.preview().is_none());
        assert!(r.take().is_none());
    }
    #[test]
    fn invalid_begin_and_extreme_motion_are_bounded_before_any_commit() {
        let mut r = Resize::new();
        assert!(!r.begin(id(), 1, bounds(), (u32::MAX, 64), (0, 0), (140, 140)));
        assert!(!r.begin(id(), 0, bounds(), (64, 64), (0, 0), (140, 140)));
        assert!(r.preview().is_none());
        assert!(r.begin(id(), 7, bounds(), (64, 64), (0, 0), (140, 140)));
        r.motion(i32::MAX, i32::MIN, i32::MIN, i32::MIN);
        let a = r.preview().unwrap();
        assert_eq!((a.width, a.height, a.stride), (128, 16, 512));
        assert!(a.width * a.height * 4 <= 65536);
        assert!(r.take().is_none());
        r.release();
        let a = r.take().unwrap();
        assert!(r.take().is_none());
        assert!(!r.failed(a.request + 1, ResizeError::Retry));
        assert!(r.failed(a.request, ResizeError::Retry));
        assert!(r.retry());
        let retry = r.take().unwrap();
        assert_ne!(a.request, retry.request);
        assert!(r.complete(a.request).is_none());
        assert_eq!(r.complete(retry.request), Some(retry));
    }
}
