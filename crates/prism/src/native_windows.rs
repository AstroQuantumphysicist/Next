//! Trusted ordinary window stacking and native placement, independent of pixels.
use crate::{client_interaction::SurfaceRef, App, Bounds};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowRef {
    Builtin(App),
    Native(SurfaceRef),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Presentation {
    pub mapped: bool,
    pub minimized: bool,
    pub bounds: Bounds,
    pub z: usize,
    pub focused: bool,
}
#[derive(Clone, Copy)]
struct Native {
    id: SurfaceRef,
    bounds: Bounds,
    minimized: bool,
}
#[derive(Clone, Copy)]
struct Drag {
    id: SurfaceRef,
    x: i32,
    y: i32,
    bounds: Bounds,
}
pub struct Windows {
    order: [Option<WindowRef>; 7],
    native: [Option<Native>; 2],
    drag: Option<Drag>,
    switch: [Option<WindowRef>; 7],
    switching: bool,
    index: usize,
}
impl Default for Windows {
    fn default() -> Self {
        Self::new()
    }
}
impl Windows {
    pub const fn new() -> Self {
        Self {
            order: [
                Some(WindowRef::Builtin(App::Terminal)),
                Some(WindowRef::Builtin(App::Files)),
                Some(WindowRef::Builtin(App::Monitor)),
                Some(WindowRef::Builtin(App::Settings)),
                Some(WindowRef::Builtin(App::Editor)),
                None,
                None,
            ],
            native: [None; 2],
            drag: None,
            switch: [None; 7],
            switching: false,
            index: 0,
        }
    }
    pub fn order(&self) -> [Option<WindowRef>; 7] {
        self.order
    }
    pub fn install(&mut self, id: SurfaceRef, bounds: Bounds) {
        if self.native.get(id.slot).is_none() {
            return;
        }
        if self.native[id.slot].is_some_and(|n| n.id == id) {
            return;
        }
        if let Some(old) = self.native[id.slot] {
            self.remove(old.id);
        }
        self.native[id.slot] = Some(Native {
            id,
            bounds,
            minimized: false,
        });
        let mut next = [None; 7];
        next[0] = Some(WindowRef::Native(id));
        let mut len = 1;
        for entry in self.order.into_iter().flatten() {
            if len < 7 {
                next[len] = Some(entry);
                len += 1;
            }
        }
        self.order = next;
    }
    pub fn remove(&mut self, id: SurfaceRef) {
        if self
            .native
            .get(id.slot)
            .is_some_and(|n| n.is_some_and(|n| n.id == id))
        {
            self.native[id.slot] = None;
            if self.drag.is_some_and(|d| d.id == id) {
                self.drag = None;
            }
            let mut next = [None; 7];
            let mut len = 0;
            for e in self.order.into_iter().flatten() {
                if e != WindowRef::Native(id) {
                    next[len] = Some(e);
                    len += 1;
                }
            }
            self.order = next;
        }
    }
    pub fn presentation(
        &self,
        id: SurfaceRef,
        focused: Option<SurfaceRef>,
    ) -> Option<Presentation> {
        let n = self.native.get(id.slot)?.filter(|n| n.id == id)?;
        Some(Presentation {
            mapped: !n.minimized,
            minimized: n.minimized,
            bounds: n.bounds,
            z: self
                .order
                .iter()
                .position(|e| *e == Some(WindowRef::Native(id)))?,
            focused: focused == Some(id),
        })
    }
    pub fn raise(&mut self, id: WindowRef) {
        if !self.order.contains(&Some(id)) {
            return;
        }
        let mut next = [None; 7];
        let mut len = 0;
        for e in self.order.into_iter().flatten() {
            if e != id {
                next[len] = Some(e);
                len += 1;
            }
        }
        next[len] = Some(id);
        self.order = next;
    }
    pub fn minimize(&mut self, id: SurfaceRef) -> bool {
        let Some(n) = self
            .native
            .get_mut(id.slot)
            .and_then(Option::as_mut)
            .filter(|n| n.id == id)
        else {
            return false;
        };
        n.minimized = true;
        if self.drag.is_some_and(|d| d.id == id) {
            self.drag = None;
        }
        true
    }
    pub fn restore(&mut self, id: SurfaceRef) -> bool {
        let Some(n) = self
            .native
            .get_mut(id.slot)
            .and_then(Option::as_mut)
            .filter(|n| n.id == id)
        else {
            return false;
        };
        n.minimized = false;
        self.raise(WindowRef::Native(id));
        true
    }
    pub fn set_bounds(&mut self, id: SurfaceRef, bounds: Bounds) -> bool {
        let Some(n) = self
            .native
            .get_mut(id.slot)
            .and_then(Option::as_mut)
            .filter(|n| n.id == id)
        else {
            return false;
        };
        n.bounds = bounds;
        true
    }
    pub fn begin_drag(&mut self, id: SurfaceRef, x: i32, y: i32) {
        if let Some(p) = self.presentation(id, None).filter(|p| p.mapped) {
            self.drag = Some(Drag {
                id,
                x,
                y,
                bounds: p.bounds,
            });
        }
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn drag(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let Some(d) = self.drag else {
            return;
        };
        if let Some(n) = self
            .native
            .get_mut(d.id.slot)
            .and_then(Option::as_mut)
            .filter(|n| n.id == d.id && !n.minimized)
        {
            n.bounds.x = d.bounds.x.saturating_add(x.saturating_sub(d.x));
            n.bounds.y = d.bounds.y.saturating_add(y.saturating_sub(d.y));
            n.bounds = clamp(n.bounds, w, h);
        }
    }
    pub fn finish_drag(&mut self) {
        self.drag = None;
    }
    pub fn viewport(&mut self, w: i32, h: i32) {
        self.drag = None;
        for n in self.native.iter_mut().flatten() {
            n.bounds = clamp(n.bounds, w, h);
        }
    }
    pub fn finish_switch(&mut self) {
        self.switching = false;
    }
    pub fn switch(
        &mut self,
        current: Option<WindowRef>,
        reverse: bool,
        eligible: impl Fn(WindowRef) -> bool,
    ) -> Option<WindowRef> {
        if !self.switching {
            self.switch = self.order;
            self.switch.reverse();
            self.index = current
                .and_then(|id| self.switch.iter().position(|e| *e == Some(id)))
                .unwrap_or(if reverse { 0 } else { 6 });
            self.switching = true;
        }
        for _ in 0..7 {
            self.index = (self.index + if reverse { 6 } else { 1 }) % 7;
            if let Some(id) = self.switch[self.index]
                .filter(|id| self.order.contains(&Some(*id)) && eligible(*id))
            {
                return Some(id);
            }
        }
        None
    }
}
fn clamp(mut b: Bounds, w: i32, h: i32) -> Bounds {
    if w < 320 || h < 240 {
        return b;
    }
    b.width = b.width.max(292).min(w.saturating_sub(8).max(0));
    b.height = b.height.max(240).min(h.saturating_sub(88).max(0));
    b.x =
        b.x.clamp(4, w.saturating_sub(b.width).saturating_sub(4).max(4));
    b.y =
        b.y.clamp(4, h.saturating_sub(b.height).saturating_sub(84).max(4));
    b
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
    #[test]
    fn publication_preserves_background_order_and_user_lifetime() {
        let mut w = Windows::new();
        let b = Bounds {
            x: 300,
            y: 76,
            width: 292,
            height: 240,
        };
        w.install(id(), b);
        assert_eq!(w.order()[0], Some(WindowRef::Native(id())));
        w.restore(id());
        let order = w.order();
        w.begin_drag(id(), 310, 80);
        w.drag(400, 200, 640, 480);
        let moved = w.presentation(id(), None).unwrap().bounds;
        assert_ne!(moved, b);
        w.install(id(), b);
        assert_eq!(w.presentation(id(), None).unwrap().bounds, moved);
        assert_eq!(w.order(), order);
        w.minimize(id());
        w.install(id(), b);
        assert!(w.presentation(id(), None).unwrap().minimized);
        w.restore(id());
        assert!(w.presentation(id(), None).unwrap().mapped);
    }
    #[test]
    fn exact_teardown_and_stable_switch_skip_slot_reuse() {
        let mut w = Windows::new();
        let b = Bounds {
            x: 10,
            y: 10,
            width: 292,
            height: 240,
        };
        w.install(id(), b);
        w.restore(id());
        let eligible = |_: WindowRef| true;
        assert_eq!(
            w.switch(Some(WindowRef::Native(id())), false, eligible),
            Some(WindowRef::Builtin(App::Editor))
        );
        w.remove(id());
        let new = SurfaceRef {
            generation: 2,
            ..id()
        };
        w.install(new, b);
        assert!(w.presentation(id(), None).is_none());
        assert!(!w.minimize(id()));
        for _ in 0..7 {
            assert_ne!(
                w.switch(None, false, eligible),
                Some(WindowRef::Native(new))
            );
        }
        w.finish_switch();
        w.restore(new);
        assert_eq!(
            w.switch(None, false, eligible),
            Some(WindowRef::Native(new))
        );
    }
}
