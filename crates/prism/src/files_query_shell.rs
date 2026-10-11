//! Files-only authoritative query order; portal/Home retain all real bindings.
use super::*;
impl Shell {
    pub fn file_query(&self) -> &files_query::Model {
        &self.file_query
    }
    pub fn file_query_dialog_active(&self) -> bool {
        self.file_query_open
    }
    pub fn take_file_query_action(&mut self) -> Option<files_query::Action> {
        self.file_query.take()
    }
    fn validate_query_paths(
        &self,
        stats: files_query::Stats,
        paths: &[files_actions::Path],
    ) -> Result<(), files_query::Error> {
        if paths.len() != stats.matched || paths.len() > 8 {
            return Err(files_query::Error::InvalidResult);
        }
        for (i, path) in paths.iter().enumerate() {
            if paths[..i].contains(path)
                || !self
                    .files
                    .iter()
                    .skip(1)
                    .any(|f| f.available && f.path.as_str() == path.as_str())
            {
                return Err(files_query::Error::InvalidResult);
            }
        }
        Ok(())
    }
    fn publish_query_paths(&mut self, paths: &[files_actions::Path]) {
        self.file_query_paths = [None; 8];
        for (slot, path) in self.file_query_paths.iter_mut().zip(paths) {
            *slot = Some(*path);
        }
        self.file_query_applied = true;
        if !(0..self.file_view_len())
            .any(|row| self.file_view_index(row) == Some(self.file_selection))
        {
            self.file_selection = self.file_view_index(0).unwrap_or(0);
            self.file_detail = false;
        }
    }
    pub fn file_query_result(
        &mut self,
        request: u64,
        result: Result<files_query::Stats, files_actions::BackendError>,
        paths: &[files_actions::Path],
    ) -> Result<(), files_query::Error> {
        if let Ok(stats) = result {
            self.validate_query_paths(stats, paths)?;
        }
        self.file_query.complete(request, result)?;
        if result.is_ok() {
            self.publish_query_paths(paths);
            self.file_query_open = false;
        }
        Ok(())
    }
    pub fn refresh_file_query(
        &mut self,
        stats: files_query::Stats,
        paths: &[files_actions::Path],
    ) -> Result<(), files_query::Error> {
        self.validate_query_paths(stats, paths)?;
        self.file_query.refresh(stats)?;
        self.publish_query_paths(paths);
        Ok(())
    }
    pub fn file_query_load_error(&mut self, error: files_actions::BackendError) {
        self.file_query.load_failed(error);
    }
    pub(super) fn file_view_len(&self) -> usize {
        (0..FILE_CAPACITY)
            .filter(|i| self.file_view_index(*i).is_some())
            .count()
    }
    pub(super) fn file_view_index(&self, row: usize) -> Option<usize> {
        if !self.file_query_applied {
            return self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.available)
                .nth(row)
                .map(|(i, _)| i);
        }
        let boot = usize::from(self.files[0].available);
        if row < boot {
            return Some(0);
        }
        let path = self
            .file_query_paths
            .iter()
            .flatten()
            .filter(|p| {
                self.files
                    .iter()
                    .skip(1)
                    .any(|f| f.available && f.path.as_str() == p.as_str())
            })
            .nth(row - boot)?;
        self.files
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, f)| f.available && f.path.as_str() == path.as_str())
            .map(|(i, _)| i)
    }
    pub(super) fn show_file_query(&mut self) {
        if !self.client_input_allowed() || self.file_query.busy() {
            return;
        }
        self.client_input.suspend();
        self.file_query_open = true;
        self.file_query_control = 0;
    }
    pub fn file_query_bounds(&self) -> Bounds {
        let (w, h) = self.desktop.dimensions();
        Bounds {
            x: ((w - 480) / 2).max(4),
            y: ((h - 340) / 2).max(4),
            width: (w - 8).clamp(0, 480),
            height: (h - 8).clamp(0, 340),
        }
    }
    pub fn file_query_control_bounds(&self, index: usize) -> Option<Bounds> {
        let b = self.file_query_bounds();
        match index {
            0 => Some(Bounds {
                x: b.x + 20,
                y: b.y + 52,
                width: b.width - 40,
                height: 36,
            }),
            1..=3 => Some(Bounds {
                x: b.x + 20,
                y: b.y + 96 + (index as i32 - 1) * 38,
                width: b.width - 40,
                height: 32,
            }),
            4..=5 => Some(Bounds {
                x: b.x + 20 + (index as i32 - 4) * 140,
                y: b.y + b.height - 52,
                width: 128,
                height: 36,
            }),
            _ => None,
        }
    }
    pub(super) fn file_query_key(&mut self, key: Key) {
        if self.file_query.busy() {
            return;
        }
        match key {
            Key::Escape => self.file_query_open = false,
            Key::Tab => self.file_query_control = (self.file_query_control + 1) % 6,
            Key::BackTab => self.file_query_control = (self.file_query_control + 5) % 6,
            Key::Character(c) if self.file_query_control == 0 => {
                let mut q = self.file_query.desired();
                if q.insert(c).is_ok() {
                    let _ = self.file_query.edit(q);
                }
            }
            Key::Backspace if self.file_query_control == 0 => {
                let mut q = self.file_query.desired();
                q.backspace();
                let _ = self.file_query.edit(q);
            }
            Key::Enter | Key::Left | Key::Right => {
                let mut q = self.file_query.desired();
                match self.file_query_control {
                    0 | 5 if key == Key::Enter => {
                        let _ = self.file_query.request();
                    }
                    1 => {
                        q.sort = if q.sort == files_query::Sort::Name {
                            files_query::Sort::Size
                        } else {
                            files_query::Sort::Name
                        };
                        let _ = self.file_query.edit(q);
                    }
                    2 => {
                        q.descending = !q.descending;
                        let _ = self.file_query.edit(q);
                    }
                    3 => {
                        q.search = if q.search == files_query::Search::Path {
                            files_query::Search::PathAndContent
                        } else {
                            files_query::Search::Path
                        };
                        let _ = self.file_query.edit(q);
                    }
                    4 if key == Key::Enter => self.file_query_open = false,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    pub(super) fn file_query_pointer(&mut self, x: i32, y: i32) {
        for i in 0..6 {
            if self
                .file_query_control_bounds(i)
                .is_some_and(|b| Self::hit(b, x, y))
            {
                self.file_query_control = i;
                if i != 0 {
                    self.file_query_key(Key::Enter);
                }
                return;
            }
        }
    }
    pub(super) fn render_file_query(&self, canvas: &mut Canvas<'_>) {
        let b = self.file_query_bounds();
        theme::shadow(canvas, b, 12);
        theme::rounded_rect(canvas, b, 12, theme::SURFACE);
        theme::text(
            canvas,
            b.x + 20,
            b.y + 16,
            "Find and sort files",
            theme::TEXT,
            theme::Font::Title,
        );
        let q = self.file_query.desired();
        let labels = [
            q.text(),
            if q.sort == files_query::Sort::Name {
                "Sort: name"
            } else {
                "Sort: size"
            },
            if q.descending {
                "Order: descending"
            } else {
                "Order: ascending"
            },
            if q.search == files_query::Search::Path {
                "Search: file paths"
            } else {
                "Search: paths and full contents"
            },
            "Cancel",
            if self.file_query.busy() {
                "Searching"
            } else {
                "Apply"
            },
        ];
        for (i, label) in labels.iter().enumerate() {
            let control = self.file_query_control_bounds(i).unwrap();
            theme::rounded_rect(canvas, control, 5, theme::BACKGROUND);
            Self::control_border(
                canvas,
                control,
                if i == self.file_query_control {
                    theme::ACCENT
                } else {
                    theme::BORDER
                },
            );
            let mut label = *label;
            while theme::text_width(label, theme::Font::Small) > control.width - 16
                && !label.is_empty()
            {
                if i == 0 {
                    label = &label[1..];
                } else {
                    label = &label[..label.len() - 1];
                }
            }
            theme::text(
                canvas,
                control.x + 8,
                control.y + 8,
                label,
                theme::TEXT,
                theme::Font::Small,
            );
        }
        if let Some(error) = self.file_query.error() {
            theme::text(
                canvas,
                b.x + 20,
                b.y + b.height - 76,
                error.message(),
                theme::TEXT,
                theme::Font::Small,
            );
        }
    }
}
