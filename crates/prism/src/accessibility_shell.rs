//! One active trusted context projected from actual Shell state.
use super::*;
use accessibility::{Error, Role, Snapshot, State};
impl Shell {
    pub fn take_accessibility_event(&mut self) -> Option<accessibility::Event> {
        self.accessibility.take()
    }
    /// Caller owns storage (prefer static storage in the kernel). An error leaves
    /// complete=false; never consume a partial snapshot as an accessibility tree.
    pub fn write_accessibility_snapshot(&mut self, out: &mut Snapshot) -> Result<(), Error> {
        let mut state = State::ENABLED;
        if self.shortcuts.open() {
            out.begin(28, 0);
            out.add(
                1,
                None,
                Role::Dialog,
                "Keyboard shortcuts",
                "Use Tab or the arrow keys to browse. Escape closes this guide.",
                Some(self.shortcuts_bounds()),
                state,
            )?;
            for (index, entry) in shortcuts::ENTRIES.iter().enumerate() {
                out.add(
                    10 + index as u32,
                    None,
                    Role::ListItem,
                    entry.name,
                    entry.keys,
                    self.shortcut_row_bounds(index),
                    State {
                        focused: index == self.shortcuts.selected(),
                        selected: index == self.shortcuts.selected(),
                        ..state
                    },
                )?;
            }
            out.add(
                2,
                None,
                Role::Button,
                "Close keyboard shortcuts",
                "Escape",
                Some(self.shortcut_close_bounds()),
                State {
                    focused: self.shortcuts.selected() == shortcuts::ENTRIES.len(),
                    selected: self.shortcuts.selected() == shortcuts::ENTRIES.len(),
                    ..state
                },
            )?;
        } else if let Some(portal) = self.file_portal {
            out.begin(20, portal.request);
            out.add(
                1,
                None,
                Role::Dialog,
                "File access request",
                portal.package.as_str(),
                self.file_portal_bounds(),
                state,
            )?;
            for i in 0..self.portal_count() {
                if let Some(file) = self.portal_file(i) {
                    out.add(
                        10 + i as u32,
                        None,
                        Role::ListItem,
                        file.path.as_str(),
                        "",
                        None,
                        State {
                            selected: i == portal.selection,
                            ..state
                        },
                    )?;
                }
            }
            state.focused = !portal.approve;
            out.add(2, None, Role::Button, "Cancel", "", None, state)?;
            state.focused = portal.approve && self.portal_count() != 0;
            state.enabled = self.portal_count() != 0;
            out.add(
                3,
                None,
                Role::Button,
                if portal.writable {
                    "Allow read and write"
                } else {
                    "Allow read only"
                },
                "",
                None,
                state,
            )?;
        } else if self.editor_deferred.is_some() {
            out.begin(21, 0);
            out.add(
                1,
                None,
                Role::Dialog,
                "Unsaved changes",
                self.document.path(),
                Some(self.file_dialog_bounds()),
                state,
            )?;
            for (i, name) in ["Cancel", "Discard changes", "Save changes"]
                .iter()
                .enumerate()
            {
                out.add(
                    2 + i as u32,
                    None,
                    Role::Button,
                    name,
                    "",
                    None,
                    State {
                        focused: i == self.editor_choice,
                        ..state
                    },
                )?;
            }
        } else if self.file_query_dialog_active() {
            out.begin(27, 0);
            out.add(
                1,
                None,
                Role::Dialog,
                "Find and sort files",
                "",
                Some(self.file_query_bounds()),
                state,
            )?;
            let q = self.file_query.desired();
            let names = [
                "File query",
                "Sort by",
                "Sort order",
                "Search scope",
                "Cancel",
                "Apply file query",
            ];
            let values = [
                q.text(),
                if q.sort == files_query::Sort::Name {
                    "Name"
                } else {
                    "Size"
                },
                if q.descending {
                    "Descending"
                } else {
                    "Ascending"
                },
                if q.search == files_query::Search::Path {
                    "Paths"
                } else {
                    "Paths and full contents"
                },
                "",
                "",
            ];
            for i in 0..6 {
                out.add(
                    10 + i as u32,
                    None,
                    if i == 0 { Role::TextBox } else { Role::Button },
                    names[i],
                    values[i],
                    self.file_query_control_bounds(i),
                    State {
                        enabled: !self.file_query.busy(),
                        focused: !self.file_query.busy() && i == self.file_query_control,
                        busy: self.file_query.busy(),
                        ..state
                    },
                )?;
            }
            if let Some(error) = self.file_query.error() {
                out.add(
                    30,
                    None,
                    Role::Status,
                    "Query error",
                    error.message(),
                    None,
                    state,
                )?;
            }
        } else if self.trash_dialog_active() || self.file_dialog_active() {
            let trash = self.trash_dialog_active();
            out.begin(if trash { 23 } else { 22 }, 0);
            if trash {
                self.sync_trash_dialog_focus();
            } else {
                self.sync_file_dialog_focus();
            }
            out.add(
                1,
                None,
                Role::Dialog,
                if trash {
                    "Trash confirmation"
                } else {
                    "File operation"
                },
                if trash {
                    self.trash.input()
                } else {
                    self.file_dialog.input()
                },
                Some(self.file_dialog_bounds()),
                state,
            )?;
            self.accessible_focus_controls(out, if trash { 7 } else { 3 })?;
            let error = if trash {
                self.trash.error()
            } else {
                self.file_dialog.error()
            };
            if let Some(error) = error {
                out.add(
                    30,
                    None,
                    Role::Status,
                    "Operation error",
                    error.message(),
                    None,
                    state,
                )?;
            }
        } else if self.notification_open {
            out.begin(24, 0);
            out.add(
                1,
                None,
                Role::Dialog,
                "Notifications",
                "",
                Some(self.notification_bounds()),
                state,
            )?;
            for i in 0..self.notifications.len() {
                if let Some(n) = self.notifications.at(i) {
                    out.add(
                        10 + i as u32,
                        None,
                        Role::ListItem,
                        n.title.as_str(),
                        n.body.as_str(),
                        None,
                        State {
                            focused: self.notification_control == 0
                                && Some(n.id) == self.notification_selected,
                            selected: Some(n.id) == self.notification_selected,
                            checked: Some(n.read),
                            ..state
                        },
                    )?;
                    out.add(
                        20 + i as u32,
                        None,
                        Role::Status,
                        "Authenticated source",
                        n.label.as_str(),
                        None,
                        state,
                    )?;
                }
            }
            for (i, name) in [
                "Dismiss selected notification",
                "Clear read notifications",
                "Do not disturb",
            ]
            .iter()
            .enumerate()
            {
                out.add(
                    2 + i as u32,
                    None,
                    if i == 2 { Role::CheckBox } else { Role::Button },
                    name,
                    "",
                    None,
                    State {
                        focused: self.notification_control == i + 1,
                        enabled: i != 0 || self.notification_selected.is_some(),
                        checked: if i == 2 {
                            Some(self.notifications.do_not_disturb())
                        } else {
                            None
                        },
                        ..state
                    },
                )?;
            }
        } else if self.launcher {
            out.begin(if self.search_mode { 26 } else { 25 }, 0);
            out.add(
                1,
                None,
                Role::Dialog,
                if self.search_mode { "Search" } else { "Home" },
                "",
                Some(self.home_bounds()),
                state,
            )?;
            if self.search_mode {
                out.add(
                    2,
                    None,
                    Role::TextBox,
                    HOME_SEARCH_LABEL,
                    self.search_input.as_str(),
                    None,
                    State {
                        focused: true,
                        ..state
                    },
                )?;
                for i in 0..self.search_count() {
                    if let Some(result) = self.search_result(i) {
                        let mut calculation_label = Line::new();
                        let (name, _, source) =
                            Self::search_result_presentation(self, result, &mut calculation_label);
                        let value = if matches!(result, SearchResult::Calculation(_)) {
                            source
                        } else {
                            ""
                        };
                        out.add(
                            10 + i as u32,
                            None,
                            Role::ListItem,
                            name,
                            value,
                            None,
                            State {
                                selected: i == self.selection,
                                ..state
                            },
                        )?;
                    }
                }
            } else {
                for i in 0..HOME_ACTION_COUNT {
                    let name = if i == desktop::ACTIONS.len() {
                        "Keyboard shortcuts"
                    } else {
                        desktop::ACTIONS[i].label()
                    };
                    out.add(
                        10 + i as u32,
                        None,
                        Role::Button,
                        name,
                        if i == desktop::ACTIONS.len() {
                            "Browse keyboard navigation and window shortcuts"
                        } else {
                            ""
                        },
                        self.home_action_bounds(i),
                        State {
                            focused: i == self.selection,
                            ..state
                        },
                    )?;
                }
            }
        } else if let Some(resize) = self.native_resize_preview() {
            out.begin(40 + resize.surface.slot as u32, resize.surface.generation);
            let (label, pid) = self
                .client_previews
                .accessible_identity(resize.surface)
                .ok_or(Error::InvalidNode)?;
            let mut value = Line::new();
            use core::fmt::Write;
            let _ = write!(value, "PID {}; {} x {}", pid, resize.width, resize.height);
            out.add(
                1,
                None,
                Role::Window,
                label,
                value.as_str(),
                Some(resize.bounds),
                state,
            )?;
            out.add(
                2,
                None,
                Role::Button,
                "Cancel resize",
                "Escape",
                None,
                State {
                    focused: true,
                    ..state
                },
            )?;
            let error = self.native_resize_error();
            out.add(
                3,
                None,
                Role::Button,
                "Retry resize",
                "Enter",
                None,
                State {
                    enabled: error == Some(native_resize::ResizeError::Retry),
                    ..state
                },
            )?;
            out.add(
                4,
                None,
                Role::Status,
                "Resize status",
                match error {
                    Some(native_resize::ResizeError::Retry) => "Busy; retry is available.",
                    Some(_) => "Resize failed; original content is unchanged.",
                    None if self.native_resize.pending() => "Requesting configuration.",
                    None => "Preview; release the pointer to request configuration.",
                },
                None,
                state,
            )?;
        } else if let Some(id) = self.client_input.context() {
            out.begin(30 + id.slot as u32, id.generation);
            let (label, pid) = self
                .client_previews
                .accessible_identity(id)
                .ok_or(Error::InvalidNode)?;
            let mut owner = Line::new();
            use core::fmt::Write;
            let _ = write!(owner, "PID {}", pid);
            out.add(
                1,
                None,
                Role::Window,
                label,
                owner.as_str(),
                self.native_window_presentation(id).map(|p| p.bounds),
                State {
                    focused: self.client_focused() == Some(id),
                    ..state
                },
            )?;
            out.add(
                4,
                None,
                Role::Button,
                "Minimize native window",
                "",
                self.native_window_minimize_button_bounds(id),
                state,
            )?;
            let status = self.client_close_status(id).ok_or(Error::InvalidNode)?;
            out.add(
                2,
                None,
                Role::Button,
                "Close native window",
                status.label(),
                self.client_close_button_bounds(id.slot),
                State {
                    enabled: status.requestable(),
                    ..state
                },
            )?;
            out.add(
                3,
                None,
                Role::Status,
                "Application accessibility",
                "Pixel content has no accessibility tree.",
                None,
                state,
            )?;
        } else {
            let app = self.desktop.focused();
            out.begin(1 + app.map_or(9, |a| a as u32), 0);
            out.add(
                1,
                None,
                Role::Window,
                app.map_or("Desktop", App::title),
                "",
                app.map(|a| self.desktop.window(a).bounds),
                state,
            )?;
            match app {
                Some(App::Files) => {
                    if self.trash_page {
                        self.sync_trash_page_focus();
                        self.accessible_focus_controls(out, 6)?;
                        for i in 0..2 {
                            if let Some(item) = self.trash.listing().items[i] {
                                out.add(
                                    20 + i as u32,
                                    None,
                                    Role::ListItem,
                                    item.original.as_str(),
                                    "",
                                    None,
                                    State {
                                        selected: self
                                            .trash
                                            .selected()
                                            .is_some_and(|selected| selected.id == item.id),
                                        ..state
                                    },
                                )?;
                            }
                        }
                    } else {
                        self.sync_files_focus();
                        self.accessible_focus_controls(out, 2)?;
                        for row in 0..self.file_view_len() {
                            if let Some(i) = self.file_view_index(row) {
                                let file = &self.files[i];
                                let start = self.file_list_start();
                                let visible = !self.file_detail
                                    && row >= start
                                    && row < start + self.file_list_rows();
                                let b = self.desktop.window(App::Files).bounds;
                                out.add(
                                    20 + i as u32,
                                    None,
                                    Role::ListItem,
                                    file.path.as_str(),
                                    "",
                                    visible.then_some(Bounds {
                                        x: b.x + 8,
                                        y: b.y + 106 + (row.saturating_sub(start) as i32) * 24,
                                        width: (b.width - 16).max(0),
                                        height: 24,
                                    }),
                                    State {
                                        visible,
                                        selected: i == self.file_selection,
                                        ..state
                                    },
                                )?;
                            }
                        }
                    }
                    out.add(
                        40,
                        None,
                        Role::Status,
                        "File operation status",
                        self.file_status.as_str(),
                        None,
                        state,
                    )?;
                }
                Some(App::Settings) => {
                    self.sync_settings_focus();
                    self.accessible_focus_controls(out, 5)?;
                    if settings_catalog::matching_count(self.settings_search.as_str()) == 0 {
                        out.add(
                            150,
                            None,
                            Role::Status,
                            "Settings search results",
                            "No matching settings. Clear the search to view the selected category.",
                            None,
                            state,
                        )?;
                    }
                    let section = settings_catalog::section(self.settings_category)
                        .ok_or(Error::InvalidNode)?;
                    for index in 0..3 {
                        let (name, bounds) =
                            if section.category == settings_catalog::Category::Appearance {
                                if index != 0 {
                                    continue;
                                }
                                (
                                    "Additional appearance controls",
                                    Some(Bounds {
                                        x: self.settings_content_x() + 8,
                                        y: self.desktop.window(App::Settings).bounds.y + 274,
                                        width: (self.settings_content_width() - 16).max(0),
                                        height: 28,
                                    }),
                                )
                            } else {
                                (
                                    section.rows[index],
                                    self.settings_disabled_control_bounds(index),
                                )
                            };
                        out.add(
                            200 + index as u32,
                            None,
                            Role::Status,
                            name,
                            if section.category == settings_catalog::Category::Appearance {
                                "Wallpaper, fonts, density, taskbar and layout are unavailable."
                            } else if section.category
                                == settings_catalog::Category::Accessibility
                            {
                                "Unavailable in this build; only Settings text size and focus outline preferences are active."
                            } else {
                                "Unavailable in this build; no action is enabled."
                            },
                            bounds,
                            State {
                                enabled: false,
                                ..state
                            },
                        )?;
                    }
                    out.add(
                        40,
                        None,
                        Role::Status,
                        "Settings section availability",
                        match (section.category, section.state) {
                            (settings_catalog::Category::Appearance, settings_catalog::State::Available) => "Theme and accent preferences are available.",
                            (settings_catalog::Category::Accessibility, settings_catalog::State::Available) => "Settings text size and keyboard focus outline preferences are available.",
                            (_, settings_catalog::State::Available) => "Settings are available.",
                            (_, settings_catalog::State::ReadOnly) => "Read-only system diagnostics are available; configuration is unavailable.",
                            (_, settings_catalog::State::Unavailable) => "This settings provider is not connected.",
                        },
                        None,
                        state,
                    )?;
                    out.add(
                        41,
                        None,
                        Role::Status,
                        "Settings text size",
                        match self.active_appearance().text_scale {
                            appearance::TextScale::Percent100 => {
                                "100 percent; applied to Settings text."
                            }
                            appearance::TextScale::Percent125 => {
                                "125 percent; applied to Settings text."
                            }
                            _ => "Unsupported stored value; no text scaling is applied.",
                        },
                        None,
                        state,
                    )?;
                }
                Some(App::Terminal) => {
                    out.add(
                        2,
                        None,
                        Role::TextBox,
                        "Terminal command",
                        self.input.as_str(),
                        None,
                        State {
                            focused: true,
                            ..state
                        },
                    )?;
                }
                Some(App::Editor) => {
                    out.add(
                        2,
                        None,
                        Role::TextBox,
                        "Editor document",
                        self.document.path(),
                        None,
                        State {
                            focused: true,
                            busy: self.editor_action.is_some(),
                            selected: self.document.dirty(),
                            ..state
                        },
                    )?;
                    out.add(
                        3,
                        None,
                        Role::Status,
                        "Editor status",
                        self.editor_status.as_str(),
                        None,
                        state,
                    )?;
                }
                Some(App::Calculator) => {
                    let bounds = self.calculator_bounds();
                    self.calculator
                        .write_accessibility_snapshot_embedded(out, bounds)?;
                }
                _ => {}
            }
        }
        self.accessibility.observe(out)
    }
    fn accessible_control_bounds(&self, scope: u32, local: u32) -> Option<Bounds> {
        match (scope, local) {
            (2, 2..=7) => self.file_action_button_bounds((local - 2) as usize),
            (6, 2..=5) => self.trash_page_button_bounds((local - 2) as usize),
            (5, 1..=2) => {
                let group = (local - 1) as usize;
                let first = self.appearance_control_bounds(group, 0)?;
                let last = self.appearance_control_bounds(group, if group == 0 { 2 } else { 3 })?;
                Some(Bounds {
                    width: last.x + last.width - first.x,
                    ..first
                })
            }
            (5, 3..=4) => self.appearance_control_bounds(2, (local - 3) as usize),
            (5, 5) => Some(self.settings_diagnostics_bounds()),
            (5, 10..=21) => self.settings_category_bounds((local - 10) as usize),
            (5, 22..=27) => self.accessibility_control_bounds((local - 22) as usize),
            (5, 28) => Some(self.settings_search_bounds()),
            (5, 29) => Some(self.settings_clear_search_bounds()),
            (3 | 7, 1) => {
                let b = self.file_dialog_bounds();
                Some(Bounds {
                    x: b.x + 20,
                    y: b.y + 88,
                    width: (b.width - 40).max(0),
                    height: 40,
                })
            }
            (3, 2..=3) => {
                let b = self.file_dialog_bounds();
                Some(Bounds {
                    x: b.x + 20 + (local as i32 - 2) * 124,
                    y: b.y + b.height - 60,
                    width: 112,
                    height: 36,
                })
            }
            (7, 2..=3) => {
                let b = self.file_dialog_bounds();
                Some(Bounds {
                    x: b.x + 20 + (local as i32 - 2) * 124,
                    y: b.y + b.height - 60,
                    width: if local == 2 { 112 } else { 144 },
                    height: 36,
                })
            }
            _ => None,
        }
    }
    fn accessible_focus_controls(&self, out: &mut Snapshot, scope: u32) -> Result<(), Error> {
        for local in 1..=29 {
            if let Some(focus) = self
                .file_focus
                .state(keyboard_focus::ControlId { scope, local })
            {
                let c = focus.control;
                if !c.visible {
                    continue;
                }
                out.add(
                    100 + local,
                    None,
                    match c.role {
                        keyboard_focus::Role::Button => Role::Button,
                        keyboard_focus::Role::TextBox => Role::TextBox,
                        keyboard_focus::Role::ListItem => Role::List,
                        keyboard_focus::Role::CheckBox => Role::CheckBox,
                        keyboard_focus::Role::Window => Role::Window,
                        keyboard_focus::Role::Dialog => Role::Dialog,
                    },
                    c.label,
                    match (scope, local) {
                        (5, 1) => match self.appearance.draft().theme {
                            appearance::Theme::Light => "Light",
                            appearance::Theme::Dark => "Dark",
                            appearance::Theme::HighContrast => "High contrast",
                        },
                        (5, 2) => match self.appearance.draft().accent {
                            appearance::Accent::Blue => "Blue",
                            appearance::Accent::Violet => "Violet",
                            appearance::Accent::Teal => "Teal",
                            appearance::Accent::Orange => "Orange",
                        },
                        (5, 22) => "Standard",
                        (5, 23) => "Strong",
                        (5, 24) => "100 percent",
                        (5, 25) => "125 percent",
                        (5, 28) => self.settings_search.as_str(),
                        (3, 1) => self.file_dialog.input(),
                        (7, 1) => self.trash.input(),
                        _ => "",
                    },
                    self.accessible_control_bounds(scope, local),
                    State {
                        enabled: c.enabled,
                        visible: c.visible,
                        focused: focus.focused,
                        selected: (scope, local) == (5, 10 + self.settings_category as u32)
                            || ((scope, local) == (5, 22)
                                && self.appearance.draft().focus_indicator
                                    == appearance::FocusIndicator::Standard)
                            || ((scope, local) == (5, 23)
                                && self.appearance.draft().focus_indicator
                                    == appearance::FocusIndicator::Strong)
                            || ((scope, local) == (5, 24)
                                && self.appearance.draft().text_scale
                                    == appearance::TextScale::Percent100)
                            || ((scope, local) == (5, 25)
                                && self.appearance.draft().text_scale
                                    == appearance::TextScale::Percent125),
                        checked: c.checked,
                        busy: false,
                    },
                )?;
            }
        }
        Ok(())
    }
}
