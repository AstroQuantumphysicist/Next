//! Trusted Shell interaction with authoritative bounded Trash snapshots.
use super::*;
impl Shell {
    pub fn set_trash_undo_receipt(
        &mut self,
        receipt: Option<trash_ui::UndoReceipt>,
    ) -> Result<(), trash_ui::Error> {
        self.trash_undo.set_receipt(receipt)
    }
    pub fn trash_undo_receipt(&self) -> Option<trash_ui::UndoReceipt> {
        self.trash_undo.receipt()
    }
    pub fn trash_undo_load_error(&mut self, error: files_actions::BackendError) {
        self.trash_undo.load_failed(error);
    }
    pub fn take_trash_undo_action(&mut self) -> Option<trash_ui::UndoAction> {
        self.trash_undo.take()
    }
    pub fn trash_undo_result(
        &mut self,
        request: u64,
        result: Result<(), files_actions::BackendError>,
    ) -> Result<(), trash_ui::Error> {
        let receipt = self.trash_undo.receipt();
        self.trash_undo.complete(request, result)?;
        self.file_status.set(match result {
            Ok(()) => "Trash operation undone.",
            Err(error) => error.message(),
        });
        if result.is_ok() && self.files_directory_mode {
            self.files_directory.refresh_all();
            self.sync_selected_directory_file();
        }
        if result.is_ok() && receipt.is_some_and(|r| r.kind == trash_ui::UndoKind::Move) {
            self.file_pending_selection = receipt.map(|r| r.path);
        }
        Ok(())
    }
    pub(super) fn request_trash_undo(&mut self) {
        if self.trash_dialog_active() || self.file_dialog_active() {
            return;
        }
        let _ = self.trash_undo.request();
    }
    pub fn trash_state(&self) -> &trash_ui::Model {
        &self.trash
    }
    pub fn set_trash_listing(&mut self, listing: trash_ui::Listing) -> Result<(), trash_ui::Error> {
        self.trash.set_listing(listing)
    }
    pub fn trash_load_error(&mut self, error: files_actions::BackendError) {
        self.trash.load_failed(error)
    }
    pub fn trash_page(&self) -> bool {
        self.trash_page
    }
    pub fn trash_dialog_active(&self) -> bool {
        self.trash.phase() != trash_ui::Phase::Idle
    }
    pub fn take_trash_action(&mut self) -> Option<trash_ui::Action> {
        self.trash.take_action()
    }
    pub fn trash_action_result(
        &mut self,
        request: u64,
        result: Result<(), files_actions::BackendError>,
    ) -> Result<(), trash_ui::Error> {
        let intent = self.trash.intent();
        self.trash.complete(request, result)?;
        if result.is_ok() {
            if self.files_directory_mode {
                self.files_directory.refresh_all();
                self.sync_selected_directory_file();
            }
            self.file_status.set(match intent {
                Some(trash_ui::Intent::Move { .. }) => "File moved to Trash.",
                Some(trash_ui::Intent::Restore { .. }) => "File restored.",
                _ => "Trash item permanently deleted.",
            });
            if let Some(trash_ui::Intent::Restore { destination, .. }) = intent {
                self.file_pending_selection = Some(destination);
            }
        }
        if self.trash_dialog_active() {
            self.sync_trash_dialog_focus();
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 7, local: 2 },
                keyboard_focus::Input::Keyboard,
            );
        }
        if !self.trash_dialog_active() {
            let _ = self.file_focus.pop_modal(7);
        }
        Ok(())
    }
    pub fn show_trash(&mut self) {
        if self.file_portal.is_some()
            || self.editor_deferred.is_some()
            || self.file_dialog_active()
            || self.trash_dialog_active()
        {
            return;
        }
        self.desktop.open(App::Files);
        self.launcher = false;
        self.trash_page = true;
        self.trash_control = 0;
        self.sync_trash_page_focus();
    }
    pub fn trash_page_button_bounds(&self, index: usize) -> Option<Bounds> {
        if index >= 4 {
            return None;
        }
        let b = self.desktop.window(App::Files).bounds;
        let width = ((b.width - 48) / 4).clamp(0, 144);
        Some(Bounds {
            x: b.x + 12 + index as i32 * (width + 8),
            y: b.y + 52,
            width,
            height: 32,
        })
    }
    pub(super) fn begin_trash_move(&mut self) {
        if self.files_directory_mode {
            let Some(index) = self.selected_file_binding_index() else {
                self.file_selection = 0;
                self.file_status
                    .set("Select a stored file from the active folder first.");
                return;
            };
            self.file_selection = index;
        }
        if self.file_selection == 0 || !self.files[self.file_selection].available {
            self.file_status
                .set("Select a native file to move to Trash.");
            return;
        }
        let path = match files_actions::Path::new(self.files[self.file_selection].path.as_str()) {
            Ok(path) => path,
            Err(_) => return,
        };
        if self
            .trash
            .begin_move(files_actions::Snapshot {
                path,
                revision: self.files_revision,
            })
            .is_ok()
        {
            self.trash_control = 0;
            self.sync_trash_dialog_focus();
        } else {
            self.file_status
                .set("Only native files can be moved to Trash.");
        }
    }
    pub(super) fn sync_trash_page_focus(&mut self) {
        let _ = self.file_focus.replace_scope(2, &[]);
        let _ = self.file_focus.replace_scope(3, &[]);
        let _ = self.file_focus.replace_scope(5, &[]);
        let _ = self.file_focus.replace_scope(7, &[]);
        if self.file_focus.active_scope() != 6 {
            let _ = self.file_focus.set_base_scope(6);
        }
        let selected = self.trash.selected().is_some();
        let controls = [
            Self::file_control(
                6,
                1,
                "Trash items",
                keyboard_focus::Role::ListItem,
                selected,
            ),
            Self::file_control(
                6,
                2,
                "Restore selected file",
                keyboard_focus::Role::Button,
                selected,
            ),
            Self::file_control(
                6,
                3,
                "Permanently delete selected Trash item",
                keyboard_focus::Role::Button,
                selected,
            ),
            Self::file_control(6, 4, "Back to Files", keyboard_focus::Role::Button, true),
            Self::file_control(
                6,
                5,
                "Undo last Trash operation",
                keyboard_focus::Role::Button,
                self.trash_undo.available(),
            ),
        ];
        let _ = self.file_focus.replace_scope(6, &controls);
    }
    pub(super) fn sync_trash_dialog_focus(&mut self) {
        if !self.trash_dialog_active() {
            let _ = self.file_focus.pop_modal(7);
            return;
        }
        let restoring = self.trash.phase() == trash_ui::Phase::RestorePath;
        let busy = self.trash.phase() == trash_ui::Phase::Saving;
        let controls = [
            Self::file_control(
                7,
                1,
                "Restore destination path",
                keyboard_focus::Role::TextBox,
                restoring && !busy,
            ),
            Self::file_control(7, 2, "Cancel", keyboard_focus::Role::Button, !busy),
            Self::file_control(
                7,
                3,
                if restoring {
                    "Restore"
                } else if self.trash.phase() == trash_ui::Phase::ConfirmPurge {
                    "Delete forever"
                } else {
                    "Move to Trash"
                },
                keyboard_focus::Role::Button,
                !busy,
            ),
        ];
        let _ = self.file_focus.replace_scope(7, &controls);
        if self.file_focus.active_scope() != 7 {
            let _ = self.file_focus.push_modal(7);
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 7, local: 2 },
                keyboard_focus::Input::Keyboard,
            );
        }
    }
    pub(super) fn trash_page_key(&mut self, key: Key) {
        self.sync_trash_page_focus();
        if self.trash_undo.busy() && matches!(key, Key::Character('r') | Key::Delete | Key::Enter) {
            return;
        }
        match key {
            Key::Tab | Key::BackTab => {
                self.file_focus.traverse(if key == Key::Tab {
                    keyboard_focus::Direction::Forward
                } else {
                    keyboard_focus::Direction::Reverse
                });
            }
            Key::Up | Key::Down => self.trash.cycle(key == Key::Up),
            Key::Escape => {
                self.trash_page = false;
                let _ = self.file_focus.replace_scope(6, &[]);
                self.sync_files_focus();
            }
            Key::Character('u') => self.request_trash_undo(),
            Key::Character('r') => {
                let _ = self.trash.begin_restore();
                self.sync_trash_dialog_focus();
            }
            Key::Delete => {
                let _ = self.trash.begin_purge();
                self.sync_trash_dialog_focus();
            }
            Key::Enter => match self.file_focus.focused().map(|id| id.local) {
                Some(1 | 2) => {
                    let _ = self.trash.begin_restore();
                    self.sync_trash_dialog_focus();
                }
                Some(3) => {
                    let _ = self.trash.begin_purge();
                    self.sync_trash_dialog_focus();
                }
                Some(5) => self.request_trash_undo(),
                Some(4) => {
                    self.trash_page = false;
                    let _ = self.file_focus.replace_scope(6, &[]);
                    self.sync_files_focus();
                }
                _ => {}
            },
            _ => {}
        }
    }
    pub(super) fn trash_page_pointer(&mut self, x: i32, y: i32) {
        self.sync_trash_page_focus();
        for index in 0..4 {
            if self
                .trash_page_button_bounds(index)
                .is_some_and(|b| Self::hit(b, x, y))
            {
                if self
                    .file_focus
                    .select(
                        keyboard_focus::ControlId {
                            scope: 6,
                            local: index as u32 + 2,
                        },
                        keyboard_focus::Input::Pointer,
                    )
                    .is_ok()
                {
                    self.trash_page_key(Key::Enter);
                }
                return;
            }
        }
        let b = self.desktop.window(App::Files).bounds;
        for index in 0..2 {
            let row = Bounds {
                x: b.x + 12,
                y: b.y + 108 + index * 64,
                width: b.width - 24,
                height: 56,
            };
            if Self::hit(row, x, y) {
                if let Some(item) = self.trash.listing().items[index as usize] {
                    let _ = self.trash.select(item.id);
                    let _ = self.file_focus.select(
                        keyboard_focus::ControlId { scope: 6, local: 1 },
                        keyboard_focus::Input::Pointer,
                    );
                }
                return;
            }
        }
    }
    pub(super) fn trash_dialog_key(&mut self, key: Key) {
        self.sync_trash_dialog_focus();
        match key {
            Key::Escape => {
                let _ = self.trash.cancel();
            }
            Key::Tab | Key::BackTab => {
                self.file_focus.traverse(if key == Key::Tab {
                    keyboard_focus::Direction::Forward
                } else {
                    keyboard_focus::Direction::Reverse
                });
            }
            Key::Character(c) if self.file_focus.focused().is_some_and(|id| id.local == 1) => {
                let _ = self.trash.insert(c);
            }
            Key::Backspace if self.file_focus.focused().is_some_and(|id| id.local == 1) => {
                self.trash.backspace()
            }
            Key::Enter => {
                if self.file_focus.focused().is_some_and(|id| id.local == 3) {
                    let _ = self.trash.choose_approve(true);
                    let _ = self.trash.activate();
                } else if self.file_focus.focused().is_some_and(|id| id.local == 2) {
                    let _ = self.trash.cancel();
                }
            }
            _ => {}
        }
        self.sync_trash_dialog_focus();
    }
    pub(super) fn trash_dialog_pointer(&mut self, x: i32, y: i32) {
        self.sync_trash_dialog_focus();
        let b = self.file_dialog_bounds();
        let local = if Self::hit(
            Bounds {
                x: b.x + 20,
                y: b.y + 88,
                width: b.width - 40,
                height: 40,
            },
            x,
            y,
        ) && self.trash.phase() == trash_ui::Phase::RestorePath
        {
            Some(1)
        } else if Self::hit(
            Bounds {
                x: b.x + 20,
                y: b.y + b.height - 60,
                width: 112,
                height: 36,
            },
            x,
            y,
        ) {
            Some(2)
        } else if Self::hit(
            Bounds {
                x: b.x + 144,
                y: b.y + b.height - 60,
                width: 144,
                height: 36,
            },
            x,
            y,
        ) {
            Some(3)
        } else {
            None
        };
        if let Some(local) = local {
            if self
                .file_focus
                .select(
                    keyboard_focus::ControlId { scope: 7, local },
                    keyboard_focus::Input::Pointer,
                )
                .is_ok()
                && local != 1
            {
                self.trash_dialog_key(Key::Enter);
            }
        }
    }
    pub(super) fn render_trash_page(&self, canvas: &mut Canvas<'_>) {
        let b = self.desktop.window(App::Files).bounds;
        for (index, label) in ["Restore", "Delete forever", "Back to Files", "Undo"]
            .iter()
            .enumerate()
        {
            let button = self.trash_page_button_bounds(index).unwrap();
            theme::rounded_rect(canvas, button, 6, theme::BACKGROUND);
            Self::control_border(canvas, button, theme::BORDER);
            theme::text(
                canvas,
                button.x + 8,
                button.y + 6,
                if index == 3 && self.trash_undo.busy() {
                    "Undoing"
                } else {
                    label
                },
                if index == 3 && !self.trash_undo.available() {
                    theme::MUTED
                } else {
                    theme::TEXT
                },
                theme::Font::Body,
            );
            if self.file_focus.focus_visible()
                && self
                    .file_focus
                    .focused()
                    .is_some_and(|id| id.scope == 6 && id.local == index as u32 + 2)
            {
                canvas.rect(button.x, button.y + 30, button.width, 2, theme::ACCENT);
            }
        }
        theme::text(
            canvas,
            b.x + 12,
            b.y + 90,
            "Trash",
            theme::TEXT,
            theme::Font::Small,
        );
        if !self.trash.loaded() {
            theme::text(
                canvas,
                b.x + 12,
                b.y + 118,
                "Trash is unavailable.",
                theme::TEXT,
                theme::Font::Body,
            );
        } else if self.trash.listing().items.iter().all(Option::is_none) {
            theme::text(
                canvas,
                b.x + 12,
                b.y + 118,
                "Trash is empty.",
                theme::TEXT,
                theme::Font::Body,
            );
        }
        for (index, item) in self.trash.listing().items.iter().enumerate() {
            let Some(item) = item else { continue };
            let row = Bounds {
                x: b.x + 12,
                y: b.y + 108 + index as i32 * 64,
                width: b.width - 24,
                height: 56,
            };
            if self
                .trash
                .selected()
                .is_some_and(|selected| selected.id == item.id)
            {
                theme::rounded_rect(canvas, row, 6, theme::ACCENT_SOFT);
                Self::control_border(canvas, row, theme::ACCENT);
            }
            let mut label_y = row.y + 4;
            fit_text(item.original.as_str(), row.width - 16, |part| {
                theme::text(
                    canvas,
                    row.x + 8,
                    label_y,
                    part,
                    theme::TEXT,
                    theme::Font::Small,
                );
                label_y += 18;
            });
            let mut size = Line::new();
            use core::fmt::Write;
            let _ = write!(size, "{} bytes", item.size);
            theme::text(
                canvas,
                row.x + 8,
                row.y + 38,
                size.as_str(),
                theme::MUTED,
                theme::Font::Small,
            );
        }
        if let Some(error) = self.trash_undo.error().or(self.trash.error()) {
            theme::text(
                canvas,
                b.x + 12,
                b.y + b.height - 54,
                error.message(),
                theme::TEXT,
                theme::Font::Small,
            );
        }
        let mut footer_y = b.y + b.height - 44;
        fit_text(
            if self.trash_undo.receipt().is_some() {
                "U: Undo last Trash operation. Delete forever cannot be undone."
            } else {
                "Restore keeps existing files. Delete forever cannot be undone."
            },
            b.width - 24,
            |line| {
                theme::text(
                    canvas,
                    b.x + 12,
                    footer_y,
                    line,
                    theme::MUTED,
                    theme::Font::Small,
                );
                footer_y += 16;
            },
        );
    }
    pub(super) fn render_trash_dialog(&self, canvas: &mut Canvas<'_>) {
        use trash_ui::Phase;
        let b = self.file_dialog_bounds();
        theme::shadow(canvas, b, 12);
        theme::rounded_rect(canvas, b, 12, theme::SURFACE);
        theme::outline(canvas, b, 12, theme::BORDER);
        let title = match self.trash.phase() {
            Phase::ConfirmMove => "Move file to Trash?",
            Phase::ConfirmPurge => "Delete forever?",
            Phase::RestorePath => "Restore file",
            _ => "Saving Trash operation",
        };
        theme::text(
            canvas,
            b.x + 20,
            b.y + 18,
            title,
            theme::TEXT,
            theme::Font::Title,
        );
        let path = match self.trash.intent() {
            Some(trash_ui::Intent::Move { source }) => Some(source.path),
            Some(trash_ui::Intent::Restore { item, .. } | trash_ui::Intent::Purge { item }) => {
                Some(item.original)
            }
            _ => None,
        };
        if let Some(path) = path {
            theme::text(
                canvas,
                b.x + 20,
                b.y + 52,
                path.as_str(),
                theme::MUTED,
                theme::Font::Small,
            );
        }
        if self.trash.phase() == Phase::RestorePath {
            let field = Bounds {
                x: b.x + 20,
                y: b.y + 88,
                width: b.width - 40,
                height: 40,
            };
            theme::rounded_rect(canvas, field, 6, theme::SURFACE_RAISED);
            Self::control_border(canvas, field, theme::BORDER);
            theme::text(
                canvas,
                field.x + 10,
                field.y + 10,
                self.trash.input(),
                theme::TEXT,
                theme::Font::Body,
            );
        } else {
            theme::text(
                canvas,
                b.x + 20,
                b.y + 90,
                if self.trash.phase() == Phase::ConfirmPurge {
                    "This permanently removes the file. It cannot be restored."
                } else {
                    "You can restore this file from Trash."
                },
                theme::TEXT,
                theme::Font::Body,
            );
        }
        if let Some(error) = self.trash.error() {
            let mut row = 0;
            wrap_text(error.message(), 46, |line| {
                theme::text(
                    canvas,
                    b.x + 20,
                    b.y + 152 + row * 20,
                    line,
                    theme::TEXT,
                    theme::Font::Body,
                );
                row += 1;
            });
        }
        for (index, label) in [
            "Cancel",
            if self.trash.phase() == Phase::RestorePath {
                "Restore"
            } else if self.trash.phase() == Phase::ConfirmPurge {
                "Delete forever"
            } else {
                "Move to Trash"
            },
        ]
        .iter()
        .enumerate()
        {
            let button = Bounds {
                x: b.x + 20 + index as i32 * 124,
                y: b.y + b.height - 60,
                width: if index == 0 { 112 } else { 144 },
                height: 36,
            };
            let focused = self
                .file_focus
                .focused()
                .is_some_and(|id| id.scope == 7 && id.local == index as u32 + 2);
            theme::rounded_rect(
                canvas,
                button,
                6,
                if focused {
                    theme::ACCENT
                } else {
                    theme::BACKGROUND
                },
            );
            Self::control_border(canvas, button, theme::BORDER);
            theme::text(
                canvas,
                button.x + 10,
                button.y + 9,
                label,
                if focused {
                    self.accent_foreground()
                } else {
                    theme::TEXT
                },
                theme::Font::Body,
            );
        }
    }
}

fn fit_text(mut text: &str, width: i32, mut emit: impl FnMut(&str)) {
    while !text.is_empty() {
        let mut end = 0;
        for n in 1..=text.len() {
            if theme::text_width(&text[..n], theme::Font::Small) > width {
                break;
            }
            end = n;
        }
        if end == 0 {
            return;
        }
        emit(&text[..end]);
        text = &text[end..];
    }
}
