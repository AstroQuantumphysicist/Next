use super::*;
use native_windows::WindowRef;
impl Shell {
    pub fn native_resize_preview(&self) -> Option<native_resize::NativeResizeAction> {
        self.client_input_allowed()
            .then(|| self.native_resize.preview())
            .flatten()
    }
    pub fn native_resize_error(&self) -> Option<native_resize::ResizeError> {
        self.native_resize.error()
    }
    pub fn take_native_resize_action(&mut self) -> Option<native_resize::NativeResizeAction> {
        if !self.client_input_allowed() {
            self.native_resize.cancel();
            return None;
        }
        let a = self.native_resize.preview()?;
        if self.client_configuration(a.surface) != Some(a.expected_configuration) {
            self.native_resize.cancel();
            return None;
        }
        self.native_resize.take()
    }
    pub fn native_resize_result(
        &mut self,
        request: u64,
        result: Result<u64, native_resize::ResizeError>,
    ) -> bool {
        let Some(a) = self
            .native_resize
            .preview()
            .filter(|a| a.request == request)
        else {
            return false;
        };
        match result {
            Err(error) => self.native_resize.failed(request, error),
            Ok(configuration) => {
                if !self.native_resize.pending() || configuration <= a.expected_configuration {
                    return false;
                }
                if self
                    .retire_client_content(a.surface, a.expected_configuration, configuration)
                    .is_err()
                {
                    return false;
                }
                self.native_windows.set_bounds(a.surface, a.bounds);
                self.sync_native_placement();
                self.native_resize.complete(request).is_some()
            }
        }
    }
    pub fn native_window_resize_handle_bounds(
        &self,
        id: client_interaction::SurfaceRef,
    ) -> Option<Bounds> {
        let p = self.native_window_presentation(id).filter(|p| p.mapped)?;
        if !self.client_content_available(id) {
            return None;
        }
        Some(Bounds {
            x: p.bounds.x + p.bounds.width - 14,
            y: p.bounds.y + p.bounds.height - 14,
            width: 12,
            height: 12,
        })
    }
    pub(super) fn render_native_resize(&self, canvas: &mut Canvas<'_>) {
        if !self.client_input_allowed() {
            return;
        }
        let Some(a) = self.native_resize_preview() else {
            return;
        };
        theme::outline(canvas, a.bounds, 10, theme::ACCENT);
        let mut label = Line::new();
        use core::fmt::Write;
        let _ = write!(label, "{} x {}", a.width, a.height);
        let message = match self.native_resize.error() {
            Some(native_resize::ResizeError::Retry) => "Busy. Enter retries; Escape cancels.",
            Some(_) => "Resize failed. Escape cancels.",
            None if self.native_resize.pending() => "Requesting resize...",
            None => "Resize preview. Escape cancels.",
        };
        theme::rounded_rect(
            canvas,
            Bounds {
                x: a.bounds.x + 8,
                y: a.bounds.y + a.bounds.height - 56,
                width: (a.bounds.width - 16).max(0),
                height: 44,
            },
            6,
            theme::SURFACE_RAISED,
        );
        theme::text(
            canvas,
            a.bounds.x + 16,
            a.bounds.y + a.bounds.height - 52,
            label.as_str(),
            theme::TEXT,
            theme::Font::Small,
        );
        theme::text(
            canvas,
            a.bounds.x + 16,
            a.bounds.y + a.bounds.height - 34,
            message,
            theme::TEXT,
            theme::Font::Small,
        );
    }
    pub fn native_window_presentation(
        &self,
        id: client_interaction::SurfaceRef,
    ) -> Option<native_windows::Presentation> {
        let mut p = self
            .native_windows
            .presentation(id, self.client_focused())?;
        let (w, h) = self.desktop.dimensions();
        p.mapped &= w >= 320 && h >= 240;
        Some(p)
    }
    pub fn native_window_restore_button_bounds(&self, slot: usize) -> Option<Bounds> {
        self.client_previews.identity(slot)?;
        let (w, h) = self.desktop.dimensions();
        Some(Bounds {
            x: 16 + slot as i32 * 128,
            y: h - 116,
            width: 120.min((w - 32).max(0)),
            height: 32,
        })
    }
    pub fn native_window_minimize_button_bounds(
        &self,
        id: client_interaction::SurfaceRef,
    ) -> Option<Bounds> {
        if self.client_previews.identity(id.slot) != Some(id) {
            return None;
        }
        let (w, h) = self.desktop.dimensions();
        self.client_previews.minimize_button_bounds(id.slot, w, h)
    }
    /// The unobstructed left portion of the trusted title chrome, excluding controls.
    pub fn native_window_titlebar_bounds(
        &self,
        id: client_interaction::SurfaceRef,
    ) -> Option<Bounds> {
        let p = self.native_window_presentation(id).filter(|p| p.mapped)?;
        let (w, h) = self.desktop.dimensions();
        let content = self.client_previews.content_bounds(id.slot, w, h)?;
        Some(Bounds {
            x: p.bounds.x + 8,
            y: p.bounds.y + 8,
            width: (p.bounds.width - 96).max(0),
            height: (content.y - p.bounds.y - 8).max(0),
        })
    }
    pub(super) fn top_native_content_at(
        &self,
        x: i32,
        y: i32,
    ) -> Option<(client_interaction::SurfaceRef, i32, i32)> {
        if !self.client_input_allowed()
            || (0..2).any(|slot| {
                self.native_window_restore_button_bounds(slot)
                    .is_some_and(|b| Self::hit(b, x, y))
            })
        {
            return None;
        }
        let WindowRef::Native(id) = self.top_window_at(x, y)? else {
            return None;
        };
        if self
            .native_window_resize_handle_bounds(id)
            .is_some_and(|b| Self::hit(b, x, y))
        {
            return None;
        }
        let (w, h) = self.desktop.dimensions();
        self.client_previews.hit_content_for(id, x, y, w, h)
    }
    pub(super) fn sync_native_placement(&mut self) {
        for slot in 0..2 {
            if let Some(id) = self.client_previews.identity(slot) {
                if let Some(p) = self.native_windows.presentation(id, None) {
                    self.client_previews
                        .set_placement(id, p.bounds, p.minimized);
                }
            }
        }
    }
    pub(super) fn top_window_at(&self, x: i32, y: i32) -> Option<WindowRef> {
        self.native_windows
            .order()
            .into_iter()
            .rev()
            .flatten()
            .find(|id| match *id {
                WindowRef::Builtin(app) => {
                    let w = self.desktop.window(app);
                    matches!(w.state, WindowState::Floating | WindowState::Maximized)
                        && Self::hit(w.bounds, x, y)
                }
                WindowRef::Native(id) => self
                    .native_windows
                    .presentation(id, None)
                    .is_some_and(|p| p.mapped && Self::hit(p.bounds, x, y)),
            })
    }
    pub fn restore_native_window(&mut self, id: client_interaction::SurfaceRef) -> bool {
        if !self.client_input_allowed() || !self.native_windows.restore(id) {
            return false;
        }
        self.sync_native_placement();
        self.client_input.chrome_gesture(id);
        true
    }
    pub fn minimize_native_window(&mut self, id: client_interaction::SurfaceRef) -> bool {
        if !self.client_input_allowed() || !self.native_windows.minimize(id) {
            return false;
        }
        if self.client_input.context() == Some(id) {
            self.client_input.suspend();
        }
        self.sync_native_placement();
        true
    }
    pub(super) fn native_window_pointer(&mut self, event: Pointer) -> bool {
        if !self.client_input_allowed() {
            self.native_windows.finish_drag();
            self.native_resize.cancel();
            return false;
        }
        if self.native_resize.preview().is_some() {
            match event {
                Pointer::Move(x, y) => {
                    let (w, h) = self.desktop.dimensions();
                    self.native_resize.motion(x, y, w, h);
                }
                Pointer::Up => self.native_resize.release(),
                _ => {}
            }
            return true;
        }
        if self.native_windows.dragging() {
            match event {
                Pointer::Move(x, y) => {
                    let (w, h) = self.desktop.dimensions();
                    self.native_windows.drag(x, y, w, h);
                    self.sync_native_placement();
                }
                Pointer::Up => self.native_windows.finish_drag(),
                _ => {}
            }
            return true;
        }
        let target = match event {
            Pointer::Down(x, y) | Pointer::Move(x, y) => Some((x, y)),
            Pointer::Up => self.pointer,
        };
        if let Some((x, y)) = target {
            for slot in 0..2 {
                if self
                    .native_window_restore_button_bounds(slot)
                    .is_some_and(|b| Self::hit(b, x, y))
                {
                    if matches!(event, Pointer::Down(..)) {
                        if let Some(id) = self.client_previews.identity(slot) {
                            self.restore_native_window(id);
                        }
                    }
                    if !matches!(event, Pointer::Down(..)) && self.client_input.buttons() != 0 {
                        self.client_input.revoke();
                    }
                    return true;
                }
            }
        }
        if let Some((x, y)) = target {
            if let Some(WindowRef::Native(id)) = self.top_window_at(x, y) {
                if !matches!(event, Pointer::Down(..))
                    && self
                        .native_window_resize_handle_bounds(id)
                        .is_some_and(|b| Self::hit(b, x, y))
                {
                    if self.client_input.buttons() != 0 {
                        self.client_input.revoke();
                    }
                    return true;
                }
            }
        }
        let Pointer::Down(x, y) = event else {
            return false;
        };
        let Some(WindowRef::Native(id)) = self.top_window_at(x, y) else {
            return false;
        };
        self.desktop.cancel_drag();
        if self
            .native_window_resize_handle_bounds(id)
            .is_some_and(|b| Self::hit(b, x, y))
        {
            let p = self.native_window_presentation(id).unwrap();
            let configuration = self.client_configuration(id).unwrap();
            let (width, height, _) = self.client_previews.geometry(id).unwrap();
            let content = self.client_content_bounds(id.slot).unwrap();
            self.client_input.block_context(id);
            self.native_resize.begin(
                id,
                configuration,
                p.bounds,
                (width, height),
                (x, y),
                (content.width, content.height),
            );
            return true;
        }

        if self
            .native_window_minimize_button_bounds(id)
            .is_some_and(|b| Self::hit(b, x, y))
        {
            self.minimize_native_window(id);
            return true;
        }
        if self
            .client_close_button_bounds(id.slot)
            .is_some_and(|b| Self::hit(b, x, y))
        {
            self.request_client_close(id);
            return true;
        }
        if self
            .native_window_titlebar_bounds(id)
            .is_some_and(|b| Self::hit(b, x, y))
        {
            self.native_windows.raise(WindowRef::Native(id));
            self.client_input.chrome_gesture(id);
            self.native_windows.begin_drag(id, x, y);
            return true;
        }
        false
    }
    pub(super) fn switch_ordinary_window(&mut self, reverse: bool) {
        let current = self
            .client_input
            .context()
            .map(WindowRef::Native)
            .or_else(|| self.desktop.focused().map(WindowRef::Builtin));
        let native = self.client_previews.identity(0);
        let native2 = self.client_previews.identity(1);
        let windows = App::ALL.map(|a| self.desktop.window(a).state != WindowState::Closed);
        let next = self.native_windows.switch(current, reverse, |id| match id {
            WindowRef::Builtin(app) => windows[app as usize],
            WindowRef::Native(id) => Some(id) == native || Some(id) == native2,
        });
        if let Some(id) = next {
            match id {
                WindowRef::Builtin(app) => {
                    self.client_input.suspend();
                    self.desktop.open(app);
                    self.native_windows.raise(id);
                }
                WindowRef::Native(id) => {
                    self.restore_native_window(id);
                }
            }
        }
    }
    pub(super) fn render_native_window_items(&self, canvas: &mut Canvas<'_>) {
        for slot in 0..2 {
            let Some(id) = self.client_previews.identity(slot) else {
                continue;
            };
            let Some(p) = self.native_window_presentation(id) else {
                continue;
            };
            let Some(b) = self.native_window_restore_button_bounds(slot) else {
                continue;
            };
            theme::rounded_rect(canvas, b, 6, theme::SURFACE);
            theme::outline(
                canvas,
                b,
                6,
                if p.focused {
                    theme::ACCENT
                } else {
                    theme::BORDER
                },
            );
            let mut text = Line::new();
            use core::fmt::Write;
            let pid = self
                .client_previews
                .accessible_identity(id)
                .map_or(0, |(_, pid)| pid);
            let _ = write!(
                text,
                "{} {}",
                if p.minimized { "Restore" } else { "App" },
                pid
            );
            theme::text(
                canvas,
                b.x + 8,
                b.y + 8,
                text.as_str(),
                theme::TEXT,
                theme::Font::Small,
            );
        }
    }
}
