//! Bounded development window manager. No allocation or application claims.
use crate::snap::{self, Minimum, Preview, Zone};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum App {
    Terminal,
    Files,
    Monitor,
    Settings,
    Editor,
}
impl App {
    pub const ALL: [Self; 5] = [
        Self::Terminal,
        Self::Files,
        Self::Monitor,
        Self::Settings,
        Self::Editor,
    ];
    pub const fn title(self) -> &'static str {
        match self {
            Self::Terminal => "Next Terminal",
            Self::Files => "Prism Files",
            Self::Monitor => "Next Monitor",
            Self::Settings => "Next Settings",
            Self::Editor => "Next Editor",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Closed,
    Floating,
    Minimized,
    Maximized,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub bounds: Bounds,
    pub restore: Bounds,
    pub state: WindowState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open(App),
    Move,
    Resize,
    Minimize,
    Maximize,
    Restore,
    Close,
}
pub const ACTIONS: [Action; 11] = [
    Action::Open(App::Terminal),
    Action::Open(App::Files),
    Action::Open(App::Monitor),
    Action::Open(App::Settings),
    Action::Open(App::Editor),
    Action::Move,
    Action::Resize,
    Action::Minimize,
    Action::Maximize,
    Action::Restore,
    Action::Close,
];
impl Action {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open(a) => a.title(),
            Self::Move => "Move focused window",
            Self::Resize => "Resize focused window",
            Self::Minimize => "Minimize focused window",
            Self::Maximize => "Maximize focused window",
            Self::Restore => "Restore focused window",
            Self::Close => "Close focused window",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Move,
    Resize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edit {
    app: App,
    mode: EditMode,
    original: Bounds,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Down(i32, i32),
    Move(i32, i32),
    Up,
}
#[derive(Clone, Copy)]
struct Drag {
    app: App,
    mode: EditMode,
    x: i32,
    y: i32,
    original: Bounds,
}
/// At most one window for each development panel. Last entry in `order` is
/// frontmost; closed and minimized windows never receive focus or hit testing.
pub struct Desktop {
    windows: [Window; 5],
    order: [App; 5],
    focus: Option<App>,
    previous: App,
    width: i32,
    height: i32,
    edit: Option<Edit>,
    drag: Option<Drag>,
    snap: Option<(App, Preview)>,
    snapped: [Option<Zone>; 5],
    switch_order: Option<[App; 5]>,
    switch_index: usize,
}
impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}
impl Desktop {
    pub const fn new() -> Self {
        let bounds = Bounds {
            x: 24,
            y: 80,
            width: 976,
            height: 544,
        };
        let mut windows = [Window {
            bounds,
            restore: bounds,
            state: WindowState::Closed,
        }; 5];
        let mut i = 0;
        while i < windows.len() {
            windows[i].bounds = Bounds {
                x: 64 + i as i32 * 28,
                y: 96 + i as i32 * 28,
                width: 640,
                height: 400,
            };
            windows[i].restore = windows[i].bounds;
            i += 1;
        }
        windows[0] = Window {
            bounds,
            restore: bounds,
            state: WindowState::Floating,
        };
        Self {
            windows,
            order: App::ALL,
            focus: Some(App::Terminal),
            previous: App::Terminal,
            width: 1024,
            height: 768,
            edit: None,
            drag: None,
            snap: None,
            snapped: [None; 5],
            switch_order: None,
            switch_index: 0,
        }
    }
    pub fn viewport(&mut self, width: usize, height: usize) {
        self.finish_switch();
        self.finish_snap(false);
        self.width = width.min(i32::MAX as usize) as i32;
        self.height = height.min(i32::MAX as usize) as i32;
        for app in App::ALL {
            let w = self.windows[app as usize];
            let restore = self.clamp(w.restore);
            let bounds = if w.state == WindowState::Maximized {
                self.full()
            } else if let Some(zone) = self.snapped[app as usize] {
                match snap::target(self.full(), Self::minimum(), zone) {
                    Ok(bounds) => bounds,
                    Err(_) => {
                        self.snapped[app as usize] = None;
                        self.clamp(w.restore)
                    }
                }
            } else {
                self.clamp(w.bounds)
            };
            self.windows[app as usize].bounds = bounds;
            self.windows[app as usize].restore = restore;
        }
    }
    fn full(&self) -> Bounds {
        let x = 8.min((self.width - 1).max(0));
        let y = 68.min((self.height - 81).max(0));
        Bounds {
            x,
            y,
            width: (self.width - 2 * x).max(1),
            height: (self.height - 80 - y - 8).max(1),
        }
    }
    fn clamp(&self, b: Bounds) -> Bounds {
        let area = self.full();
        let width = b.width.max(240.min(area.width)).min(area.width);
        let height = b.height.max(144.min(area.height)).min(area.height);
        Bounds {
            x: b.x.max(area.x).min(area.x + area.width - width),
            y: b.y.max(area.y).min(area.y + area.height - height),
            width,
            height,
        }
    }
    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }
    pub fn window(&self, app: App) -> Window {
        self.windows[app as usize]
    }
    pub fn focused(&self) -> Option<App> {
        self.focus
    }
    pub fn order(&self) -> [App; 5] {
        self.order
    }
    pub fn editing(&self) -> Option<EditMode> {
        self.edit.map(|e| e.mode)
    }
    fn visible(&self, app: App) -> bool {
        matches!(
            self.windows[app as usize].state,
            WindowState::Floating | WindowState::Maximized
        )
    }
    fn front(&mut self, app: App) {
        let i = self.order.iter().position(|a| *a == app).unwrap_or(0);
        self.order[i..].rotate_left(1);
        self.focus = Some(app);
        self.previous = app;
    }
    fn fallback(&mut self) {
        self.focus = self.order.iter().rev().copied().find(|a| self.visible(*a));
        if let Some(a) = self.focus {
            self.previous = a;
        }
    }
    pub fn open(&mut self, app: App) {
        self.finish_switch();
        let i = app as usize;
        if !self.visible(app) {
            self.windows[i].state = WindowState::Floating;
            self.windows[i].bounds = self.snapped[i]
                .and_then(|z| snap::target(self.full(), Self::minimum(), z).ok())
                .unwrap_or_else(|| self.clamp(self.windows[i].restore));
        }
        self.front(app);
    }
    /// Keep a stable recency order throughout one held Alt gesture. Raising a
    /// preview must not reorder the next candidate in that same gesture.
    pub fn switch_next(&mut self) {
        self.switch(false);
    }
    pub fn switch_previous(&mut self) {
        self.switch(true);
    }
    pub fn finish_switch(&mut self) {
        self.switch_order = None;
    }
    fn switch(&mut self, reverse: bool) {
        let order = *self.switch_order.get_or_insert_with(|| {
            let mut order = self.order;
            order.reverse();
            self.switch_index = self
                .focus
                .and_then(|app| order.iter().position(|a| *a == app))
                .unwrap_or(if reverse { 0 } else { 4 });
            order
        });
        for _ in 0..5 {
            self.switch_index = (self.switch_index + if reverse { 4 } else { 1 }) % 5;
            let app = order[self.switch_index];
            if self.windows[app as usize].state != WindowState::Closed {
                if self.windows[app as usize].state == WindowState::Minimized {
                    self.windows[app as usize].state = WindowState::Floating;
                }
                self.front(app);
                break;
            }
        }
    }
    pub fn cycle(&mut self) {
        self.finish_switch();
        let current = self.focus;
        let start = current
            .and_then(|a| App::ALL.iter().position(|x| *x == a))
            .unwrap_or(4);
        for offset in 1..=5 {
            let a = App::ALL[(start + offset) % 5];
            if self.visible(a) {
                self.front(a);
                break;
            }
        }
    }
    pub fn cycle_reverse(&mut self) {
        self.finish_switch();
        let start = self
            .focus
            .and_then(|a| App::ALL.iter().position(|x| *x == a))
            .unwrap_or(0);
        for offset in 1..=5 {
            let app = App::ALL[(start + 5 - offset) % 5];
            if self.visible(app) {
                self.front(app);
                break;
            }
        }
    }
    pub fn action(&mut self, action: Action) {
        self.finish_switch();
        if let Action::Open(a) = action {
            self.open(a);
            return;
        }
        let app = self.focus.unwrap_or(self.previous);
        let i = app as usize;
        match action {
            Action::Move | Action::Resize if self.visible(app) => {
                if self.windows[i].state == WindowState::Maximized || self.snapped[i].is_some() {
                    self.restore(app);
                }
                self.edit = Some(Edit {
                    app,
                    mode: if action == Action::Move {
                        EditMode::Move
                    } else {
                        EditMode::Resize
                    },
                    original: self.windows[i].bounds,
                });
            }
            Action::Minimize if self.visible(app) => {
                if self.windows[i].state == WindowState::Floating && self.snapped[i].is_none() {
                    self.windows[i].restore = self.windows[i].bounds;
                }
                self.windows[i].state = WindowState::Minimized;
                self.fallback();
            }
            Action::Maximize if self.visible(app) => {
                if self.windows[i].state != WindowState::Maximized {
                    if self.snapped[i].is_none() {
                        self.windows[i].restore = self.windows[i].bounds;
                    }
                    self.snapped[i] = None;
                    self.windows[i].bounds = self.full();
                    self.windows[i].state = WindowState::Maximized;
                }
            }
            Action::Restore if self.windows[i].state != WindowState::Closed => {
                self.restore(app);
                self.front(app);
            }
            Action::Close if self.visible(app) => {
                self.windows[i].state = WindowState::Closed;
                self.snapped[i] = None;
                self.snap = None;
                self.edit = None;
                self.drag = None;
                self.fallback();
            }
            _ => {}
        }
    }
    fn restore(&mut self, app: App) {
        let i = app as usize;
        self.snap = None;
        self.snapped[i] = None;
        self.windows[i].bounds = self.clamp(self.windows[i].restore);
        self.windows[i].state = WindowState::Floating;
    }
    const fn minimum() -> Minimum {
        Minimum {
            width: 240,
            height: 144,
        }
    }
    pub fn snap_preview(&self) -> Option<(App, Bounds)> {
        self.snap.map(|(app, p)| (app, p.bounds()))
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn preview_zone(&self) -> Option<Zone> {
        self.snap.map(|(_, p)| p.zone())
    }
    pub fn snapped_zone(&self, app: App) -> Option<Zone> {
        self.snapped[app as usize]
    }
    pub fn begin_snap(&mut self, zone: Zone) -> Result<(), snap::Error> {
        let app = self.focus.ok_or(snap::Error::InvalidGeometry)?;
        if !self.visible(app) {
            return Err(snap::Error::InvalidGeometry);
        }
        if let Some((current, mut preview)) = self.snap {
            if current == app {
                preview.select(zone)?;
                self.snap = Some((app, preview));
                return Ok(());
            }
        }
        let original = self
            .drag
            .filter(|d| d.app == app)
            .map_or(self.windows[app as usize].bounds, |d| d.original);
        let preview = Preview::new(self.full(), original, Self::minimum(), zone)?;
        self.snap = Some((app, preview));
        Ok(())
    }
    pub fn finish_snap(&mut self, commit: bool) -> Option<(App, Zone, Bounds)> {
        let (app, preview) = self.snap.take()?;
        let i = app as usize;
        if commit {
            if self.snapped[i].is_none() && self.windows[i].state != WindowState::Maximized {
                self.windows[i].restore = preview.cancel();
            }
            self.windows[i].bounds = preview.commit();
            self.windows[i].state = WindowState::Floating;
            self.snapped[i] = Some(preview.zone());
        } else {
            self.windows[i].bounds = self.clamp(preview.cancel());
        }
        self.drag = None;
        self.edit = None;
        Some((app, preview.zone(), self.windows[i].bounds))
    }
    pub fn cancel_drag(&mut self) {
        self.finish_snap(false);
        if let Some(d) = self.drag.take() {
            self.windows[d.app as usize].bounds = self.clamp(d.original);
        }
    }
    pub fn edit_delta(&mut self, dx: i32, dy: i32) {
        if let Some(e) = self.edit {
            let mut b = self.windows[e.app as usize].bounds;
            match e.mode {
                EditMode::Move => {
                    b.x = b.x.saturating_add(dx);
                    b.y = b.y.saturating_add(dy)
                }
                EditMode::Resize => {
                    b.width = b.width.saturating_add(dx);
                    b.height = b.height.saturating_add(dy)
                }
            }
            self.windows[e.app as usize].bounds = self.clamp(b);
        }
    }
    pub fn finish_edit(&mut self, commit: bool) {
        if let Some(e) = self.edit.take() {
            if !commit {
                self.windows[e.app as usize].bounds = self.clamp(e.original);
            }
            self.windows[e.app as usize].restore = self.windows[e.app as usize].bounds;
        }
    }
    pub fn pointer(&mut self, event: Pointer) {
        match event {
            Pointer::Down(x, y) => {
                self.finish_switch();
                self.finish_snap(false);
                self.drag = None;
                for app in self.order.into_iter().rev() {
                    let w = self.window(app);
                    if !self.visible(app)
                        || x < w.bounds.x
                        || y < w.bounds.y
                        || x >= w.bounds.x + w.bounds.width
                        || y >= w.bounds.y + w.bounds.height
                    {
                        continue;
                    }
                    self.front(app);
                    let b = w.bounds;
                    if y < b.y + 40 {
                        let right = b.x + b.width;
                        if x >= right - 32 {
                            self.action(Action::Close);
                        } else if x >= right - 64 {
                            self.action(if w.state == WindowState::Maximized {
                                Action::Restore
                            } else {
                                Action::Maximize
                            });
                        } else if x >= right - 96 {
                            self.action(Action::Minimize);
                        } else if w.state == WindowState::Floating {
                            self.drag = Some(Drag {
                                app,
                                mode: EditMode::Move,
                                x,
                                y,
                                original: b,
                            });
                        }
                    } else if x >= b.x + b.width - 20
                        && y >= b.y + b.height - 20
                        && w.state == WindowState::Floating
                    {
                        self.drag = Some(Drag {
                            app,
                            mode: EditMode::Resize,
                            x,
                            y,
                            original: b,
                        });
                    }
                    break;
                }
            }
            Pointer::Move(x, y) => {
                if let Some(d) = self.drag {
                    let mut b = d.original;
                    let dx = x.saturating_sub(d.x);
                    let dy = y.saturating_sub(d.y);
                    match d.mode {
                        EditMode::Move => {
                            b.x = b.x.saturating_add(dx);
                            b.y = b.y.saturating_add(dy)
                        }
                        EditMode::Resize => {
                            b.width = b.width.saturating_add(dx);
                            b.height = b.height.saturating_add(dy)
                        }
                    }
                    self.windows[d.app as usize].bounds = self.clamp(b);
                    if d.mode == EditMode::Move {
                        if let Some(zone) = snap::pointer_zone(self.full(), x, y, 24) {
                            let _ = self.begin_snap(zone);
                        } else {
                            self.snap = None;
                        }
                    }
                }
            }
            Pointer::Up => {
                if self.snap.is_some() {
                    self.finish_snap(true);
                    return;
                }
                if let Some(d) = self.drag.take() {
                    self.snapped[d.app as usize] = None;
                    self.windows[d.app as usize].restore = self.windows[d.app as usize].bounds;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_window_switch_keeps_recency_snapshot_and_release_commits_selection() {
        let mut d = Desktop::new();
        d.open(App::Settings);
        d.open(App::Files);
        d.open(App::Monitor);
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Files));
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Settings));
        d.switch_previous();
        assert_eq!(d.focused(), Some(App::Files));
        d.finish_switch();
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Settings));
        d.finish_switch();
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Files));
    }
    #[test]
    fn window_switch_restores_minimized_and_skips_closed_windows() {
        let mut d = Desktop::new();
        d.open(App::Files);
        d.action(Action::Minimize);
        d.open(App::Settings);
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Files));
        assert_eq!(d.window(App::Files).state, WindowState::Floating);
        d.action(Action::Close);
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Terminal));
        d.action(Action::Close);
        d.action(Action::Close);
        d.switch_next();
        assert_eq!(d.focused(), None);
    }
    #[test]
    fn switching_from_all_minimized_starts_at_recency_edge_in_both_directions() {
        let mut d = Desktop::new();
        d.open(App::Files);
        d.open(App::Settings);
        d.action(Action::Minimize);
        d.action(Action::Minimize);
        d.action(Action::Minimize);
        assert_eq!(d.focused(), None);
        d.switch_next();
        assert_eq!(d.focused(), Some(App::Settings));
        assert_eq!(d.window(App::Settings).state, WindowState::Floating);
        d.finish_switch();
        d.action(Action::Minimize);
        assert_eq!(d.focused(), None);
        d.switch_previous();
        assert_eq!(d.focused(), Some(App::Terminal));
        assert_eq!(d.window(App::Terminal).state, WindowState::Floating);
        d.action(Action::Close);
        d.switch_previous();
        assert_eq!(d.focused(), Some(App::Files));
    }
    #[test]
    fn focus_order_minimize_restore_and_close() {
        let mut d = Desktop::new();
        d.open(App::Files);
        d.open(App::Monitor);
        assert_eq!(d.focused(), Some(App::Monitor));
        assert_eq!(d.order()[4], App::Monitor);
        d.action(Action::Minimize);
        assert_eq!(d.window(App::Monitor).state, WindowState::Minimized);
        assert_eq!(d.focused(), Some(App::Files));
        d.open(App::Monitor);
        assert_eq!(d.focused(), Some(App::Monitor));
        d.action(Action::Close);
        assert_eq!(d.window(App::Monitor).state, WindowState::Closed);
        assert_eq!(d.focused(), Some(App::Files));
        d.cycle();
        assert_eq!(d.focused(), Some(App::Terminal));
        d.cycle();
        assert_eq!(d.focused(), Some(App::Files));
    }
    #[test]
    fn editing_cancel_commit_and_resize_limits() {
        let mut d = Desktop::new();
        d.open(App::Files);
        let original = d.window(App::Files).bounds;
        d.action(Action::Move);
        d.edit_delta(32, 16);
        assert_eq!(d.window(App::Files).bounds.x, original.x + 32);
        d.finish_edit(false);
        assert_eq!(d.window(App::Files).bounds, original);
        d.action(Action::Resize);
        d.edit_delta(-10000, -10000);
        d.finish_edit(true);
        let b = d.window(App::Files).bounds;
        assert_eq!((b.width, b.height), (240, 144));
        d.action(Action::Move);
        d.edit_delta(i32::MAX, i32::MAX);
        d.finish_edit(true);
        let b = d.window(App::Files).bounds;
        assert!(b.x + b.width <= 1016);
        assert!(b.y + b.height <= 720);
    }
    #[test]
    fn maximized_restore_survives_display_resize() {
        let mut d = Desktop::new();
        d.open(App::Settings);
        let original = d.window(App::Settings).bounds;
        d.action(Action::Maximize);
        assert_eq!(d.window(App::Settings).state, WindowState::Maximized);
        d.action(Action::Restore);
        assert_eq!(d.window(App::Settings).bounds, original);
        d.action(Action::Maximize);
        d.viewport(320, 240);
        d.action(Action::Restore);
        let b = d.window(App::Settings).bounds;
        assert!(b.x >= 0 && b.y >= 0 && b.x + b.width <= 320 && b.y + b.height <= 200);
    }
    #[test]
    fn all_windows_closed_leaves_no_hidden_focus() {
        let mut d = Desktop::new();
        d.action(Action::Close);
        assert_eq!(d.focused(), None);
        d.cycle();
        assert_eq!(d.focused(), None);
        d.open(App::Files);
        assert_eq!(d.focused(), Some(App::Files));
    }
    #[test]
    fn pointer_controls_drag_resize_and_focus() {
        let mut d = Desktop::new();
        d.open(App::Files);
        let b = d.window(App::Files).bounds;
        d.pointer(Pointer::Down(b.x + 20, b.y + 10));
        d.pointer(Pointer::Move(b.x + 52, b.y + 26));
        d.pointer(Pointer::Up);
        let moved = d.window(App::Files).bounds;
        assert_eq!((moved.x, moved.y), (b.x + 32, b.y + 16));
        d.pointer(Pointer::Down(
            moved.x + moved.width - 10,
            moved.y + moved.height - 10,
        ));
        d.pointer(Pointer::Move(
            moved.x + moved.width + 22,
            moved.y + moved.height + 6,
        ));
        d.pointer(Pointer::Up);
        let resized = d.window(App::Files).bounds;
        assert_eq!(
            (resized.width, resized.height),
            (moved.width + 32, moved.height + 16)
        );
        d.pointer(Pointer::Down(
            resized.x + resized.width - 10,
            resized.y + 10,
        ));
        assert_eq!(d.window(App::Files).state, WindowState::Closed);
        assert_eq!(d.focused(), Some(App::Terminal));
    }
    #[test]
    fn snapping_preview_apply_cancel_restore_and_minimize_preserve_original() {
        let mut d = Desktop::new();
        let original = d.window(App::Terminal).bounds;
        d.begin_snap(Zone::Left).unwrap();
        let left = d.snap_preview().unwrap().1;
        assert_eq!(d.window(App::Terminal).bounds, original);
        d.finish_snap(false);
        assert_eq!(d.window(App::Terminal).bounds, original);
        d.begin_snap(Zone::Left).unwrap();
        d.finish_snap(true);
        assert_eq!(d.window(App::Terminal).bounds, left);
        assert_eq!(d.window(App::Terminal).restore, original);
        d.begin_snap(Zone::Right).unwrap();
        d.finish_snap(true);
        assert_eq!(d.window(App::Terminal).restore, original);
        let right = d.window(App::Terminal).bounds;
        d.action(Action::Minimize);
        d.open(App::Terminal);
        assert_eq!(d.window(App::Terminal).bounds, right);
        d.action(Action::Restore);
        assert_eq!(d.window(App::Terminal).bounds, original);
        assert_eq!(d.snapped_zone(App::Terminal), None);
    }
    #[test]
    fn pointer_edges_preview_release_apply_and_viewport_reflows_or_unsnaps() {
        let mut d = Desktop::new();
        let original = d.window(App::Terminal).bounds;
        d.pointer(Pointer::Down(original.x + 100, original.y + 10));
        d.pointer(Pointer::Move(8, 200));
        assert_eq!(d.preview_zone(), Some(Zone::Left));
        d.pointer(Pointer::Up);
        assert_eq!(d.snapped_zone(App::Terminal), Some(Zone::Left));
        assert_eq!(d.window(App::Terminal).restore, original);
        d.viewport(1600, 900);
        assert_eq!(d.window(App::Terminal).bounds.width, d.full().width / 2);
        d.viewport(320, 240); // half-width cannot satisfy 240-pixel minimum
        assert_eq!(d.snapped_zone(App::Terminal), None);
        let bounds = d.window(App::Terminal).bounds;
        assert!(
            bounds.x >= 0
                && bounds.y >= 0
                && bounds.x + bounds.width <= 320
                && bounds.y + bounds.height <= 240
        );
    }
}
