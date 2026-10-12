#![no_std]
//! Prism's early framebuffer development shell, not the completed Prism desktop.
//! The bitmap glyphs below are original project code, licensed under Apache-2.0.
pub mod accessibility;
mod accessibility_shell;
pub mod appearance;
pub mod appearance_ui;
pub mod calculator;
pub mod calculator_app;
pub mod client_interaction;
pub mod client_surface;
pub mod cursor;
pub mod desktop;
pub mod editor;
pub mod files_actions;
pub mod files_directory;
pub mod files_query;
mod files_query_shell;
pub mod keyboard_focus;
pub mod native_resize;
mod native_window_shell;
pub mod native_windows;
pub mod notifications;
pub mod palette;
pub mod shortcuts;
pub mod snap;
pub mod surface_state;
pub mod theme;
mod trash_shell;
pub mod trash_ui;
mod trash_undo_ui;
pub use desktop::{Action, App, Bounds, Desktop, EditMode, Pointer, WindowState};
pub use editor::{Action as EditorAction, Error as EditorError, Path as EditorPath};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Bgr,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasError {
    InvalidGeometry,
    BufferTooSmall,
}

/// A checked framebuffer view. Colors passed to drawing methods are 0xRRGGBB.
/// UEFI RGB means byte order R,G,B,reserved; BGR means B,G,R,reserved.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
    stride: usize,
    format: PixelFormat,
    palette: palette::Palette,
}
impl<'a> Canvas<'a> {
    pub fn new(
        pixels: &'a mut [u32],
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
    ) -> Result<Self, CanvasError> {
        if width == 0
            || height == 0
            || stride < width
            || width > i32::MAX as usize
            || height > i32::MAX as usize
        {
            return Err(CanvasError::InvalidGeometry);
        }
        let required = (height - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(width))
            .ok_or(CanvasError::InvalidGeometry)?;
        if pixels.len() < required {
            return Err(CanvasError::BufferTooSmall);
        }
        Ok(Self {
            pixels,
            width,
            height,
            stride,
            format,
            palette: palette::Palette::DEFAULT,
        })
    }
    /// Copy a completed software frame into the same-format front buffer.
    /// Geometry is checked before any write; stride padding remains untouched.
    /// This is bounded software presentation, not a vblank/tear-free promise.
    pub fn present_to(&self, front: &mut [u32]) -> Result<(), CanvasError> {
        let required = (self.height - 1)
            .checked_mul(self.stride)
            .and_then(|n| n.checked_add(self.width))
            .ok_or(CanvasError::InvalidGeometry)?;
        if front.len() < required {
            return Err(CanvasError::BufferTooSmall);
        }
        for row in 0..self.height {
            let start = row * self.stride;
            front[start..start + self.width]
                .copy_from_slice(&self.pixels[start..start + self.width]);
        }
        Ok(())
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn set_palette(&mut self, palette: palette::Palette) {
        self.palette = palette;
    }
    pub(crate) fn font_color(&self, color: u32) -> u32 {
        // White glyphs/icons express an explicit foreground role. White panel
        // fills use rect(), which maps the raised-surface token separately.
        if color == 0xffffff {
            color
        } else {
            self.palette.resolve(color)
        }
    }
    fn encode(&self, color: u32) -> u32 {
        let color = color & 0x00ff_ffff;
        match self.format {
            PixelFormat::Bgr => color,
            PixelFormat::Rgb => {
                (color & 0x00ff00) | ((color & 0xff0000) >> 16) | ((color & 0x0000ff) << 16)
            }
        }
    }
    pub fn pixel(&mut self, x: i32, y: i32, color: u32) {
        self.pixel_literal(x, y, self.palette.resolve(color));
    }
    pub(crate) fn pixel_literal(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let index = y as usize * self.stride + x as usize;
            self.pixels[index] = self.encode(color);
        }
    }
    /// Rectangles are clipped, including negative origins and oversized extents.
    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        self.rect_literal(x, y, width, height, self.palette.resolve(color));
    }
    pub(crate) fn rect_literal(&mut self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let x0 = x.max(0).min(self.width as i32) as usize;
        let y0 = y.max(0).min(self.height as i32) as usize;
        let x1 = x.saturating_add(width).max(0).min(self.width as i32) as usize;
        let y1 = y.saturating_add(height).max(0).min(self.height as i32) as usize;
        let encoded = self.encode(color);
        for row in y0..y1 {
            self.pixels[row * self.stride + x0..row * self.stride + x1].fill(encoded);
        }
    }
    pub fn clear(&mut self, color: u32) {
        self.rect(0, 0, self.width as i32, self.height as i32, color);
    }
    pub fn text(&mut self, x: i32, y: i32, text: &str, color: u32, scale: i32) {
        if scale <= 0 || scale > 16 {
            return;
        }
        let mut cursor = x;
        let color = self.font_color(color);
        for character in text.chars() {
            let rows = glyph(character);
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect_literal(
                            cursor.saturating_add(col * scale),
                            y.saturating_add(row as i32 * scale),
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
            cursor = cursor.saturating_add(6 * scale);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Backspace,
    Enter,
    Escape,
    Tab,
    BackTab,
    WindowNext,
    WindowPrevious,
    WindowSwitchEnd,
    OpenSearch,
    OpenSettings,
    OpenFiles,
    OpenTerminal,
    CloseWindow,
    Up,
    Down,
    Left,
    Right,
    Menu,
    Delete,
    Home,
    End,
    SnapLeft,
    SnapRight,
    SnapMaximize,
    SnapRestore,
    Notifications,
}
const LINE_CAPACITY: usize = 96;
const HISTORY_CAPACITY: usize = 20;
const FILE_CAPACITY: usize = 9;
const HOME_ACTION_COUNT: usize = desktop::ACTIONS.len() + 1;
const TASKBAR_ITEM_COUNT: usize = App::ALL.len() + 2;
const TASKBAR_NOTIFICATIONS_INDEX: usize = App::ALL.len() + 1;
#[derive(Clone, Copy)]
struct Line {
    bytes: [u8; LINE_CAPACITY],
    len: usize,
}
impl Line {
    const fn new() -> Self {
        Self {
            bytes: [0; LINE_CAPACITY],
            len: 0,
        }
    }
    fn set(&mut self, text: &str) {
        self.len = 0;
        for byte in text.bytes() {
            if byte.is_ascii() && !byte.is_ascii_control() && self.len < LINE_CAPACITY {
                self.bytes[self.len] = byte;
                self.len += 1;
            }
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl core::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        if !text.is_ascii() || text.len() > LINE_CAPACITY - self.len {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct FileEntry {
    path: Line,
    content: Line,
    available: bool,
}
impl FileEntry {
    const fn empty() -> Self {
        Self {
            path: Line::new(),
            content: Line::new(),
            available: false,
        }
    }
}
/// Bounded framebuffer development desktop, not a user-mode compositor.
/// Panels display only explicitly supplied data. Escape opens Home; Tab chooses
/// an app/action; Enter activates it. Outside Home, Tab cycles visible windows.
#[derive(Clone, Copy)]
enum EditorDeferred {
    Open(EditorPath),
    Close,
}
#[derive(Clone, Copy)]
enum SearchResult {
    App(App),
    File(usize),
    Shortcuts,
}
fn search_matches(candidate: &str, query: &str) -> bool {
    query.is_empty()
        || candidate
            .as_bytes()
            .windows(query.len())
            .any(|w| w.eq_ignore_ascii_case(query.as_bytes()))
}
/// Trusted selection result; the kernel must revalidate the requester and path
/// before opening and attenuating a file capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilePortalAction {
    Approve { request: u64, path: EditorPath },
    Deny { request: u64 },
}
impl FilePortalAction {
    fn request(self) -> u64 {
        match self {
            Self::Approve { request, .. } | Self::Deny { request } => request,
        }
    }
}
#[derive(Clone, Copy)]
struct FilePortal {
    request: u64,
    pid: u64,
    package: Line,
    writable: bool,
    selection: usize,
    approve: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapEventKind {
    Preview,
    Applied,
    Cancelled,
    Restored,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapEvent {
    pub kind: SnapEventKind,
    pub app: App,
    pub zone: Option<snap::Zone>,
    pub bounds: desktop::Bounds,
}
pub struct Shell {
    native_windows: native_windows::Windows,
    native_resize: native_resize::Resize,
    accessibility: accessibility::Observer,
    client_previews: client_surface::Previews,
    client_input: client_interaction::Interaction,
    input: Line,
    history: [Line; HISTORY_CAPACITY],
    count: usize,
    status: Line,
    launcher: bool,
    search_mode: bool,
    search_input: Line,
    boot_file: Line,
    file_available: bool,
    uptime_seconds: Option<u64>,
    desktop: Desktop,
    calculator: calculator_app::CalculatorApp,
    selection: usize,
    files: [FileEntry; FILE_CAPACITY],
    file_selection: usize,
    file_detail: bool,
    file_query: files_query::Model,
    file_query_paths: [Option<files_actions::Path>; 8],
    file_query_applied: bool,
    files_directory: files_directory::Model,
    files_directory_address_editing: bool,
    file_query_open: bool,
    file_query_control: usize,
    files_revision: u64,
    file_dialog: files_actions::Dialog,
    trash: trash_ui::Model,
    trash_undo: trash_undo_ui::Undo,
    trash_page: bool,
    trash_control: usize,
    file_focus: keyboard_focus::Focus<'static, 10, 2>,
    file_status: Line,
    file_outcome: Option<files_actions::Outcome>,
    file_pending_selection: Option<files_actions::Path>,
    appearance: appearance_ui::AppearanceUi,
    settings_diagnostics: bool,
    monitor: [Line; 8],
    system: [Line; 8],
    response: Option<[Line; HISTORY_CAPACITY]>,
    response_count: usize,
    pointer: Option<(i32, i32)>,
    pointer_buttons: u8,
    document: editor::Document,
    editor_loaded: bool,
    editor_action: Option<EditorAction>,
    editor_status: Line,
    editor_deferred: Option<EditorDeferred>,
    editor_choice: usize,
    file_portal: Option<FilePortal>,
    file_portal_action: Option<FilePortalAction>,
    snap_event: Option<SnapEvent>,
    notifications: notifications::Center,
    notification_open: bool,
    notification_selected: Option<u64>,
    notification_control: usize,
    notification_toast: Option<(u64, u64)>,
    shortcuts: shortcuts::Model,
}
impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}
impl Shell {
    /// Constant construction permits static storage without materializing a
    /// large aggregate on the boot stack. Call initialize_defaults afterward.
    pub const fn empty() -> Self {
        Self {
            native_windows: native_windows::Windows::new(),
            native_resize: native_resize::Resize::new(),
            accessibility: accessibility::Observer::new(),
            client_previews: client_surface::Previews::new(),
            client_input: client_interaction::Interaction::new(),
            input: Line::new(),
            history: [Line::new(); HISTORY_CAPACITY],
            count: 0,
            status: Line::new(),
            launcher: false,
            search_mode: false,
            search_input: Line::new(),
            boot_file: Line::new(),
            file_available: false,
            uptime_seconds: None,
            desktop: Desktop::new(),
            calculator: calculator_app::CalculatorApp::new(),
            selection: 0,
            files: [FileEntry::empty(); FILE_CAPACITY],
            file_selection: 0,
            file_detail: false,
            file_query: files_query::Model::new(),
            file_query_paths: [None; 8],
            file_query_applied: false,
            files_directory: files_directory::Model::new(),
            files_directory_address_editing: false,
            file_query_open: false,
            file_query_control: 0,
            files_revision: 0,
            file_dialog: files_actions::Dialog::new(),
            trash: trash_ui::Model::new(),
            trash_undo: trash_undo_ui::Undo::new(),
            trash_page: false,
            trash_control: 0,
            file_focus: keyboard_focus::Focus::new(2),
            file_status: Line::new(),
            file_outcome: None,
            file_pending_selection: None,
            appearance: appearance_ui::AppearanceUi::new(),
            settings_diagnostics: false,
            monitor: [Line::new(); 8],
            system: [Line::new(); 8],
            response: None,
            response_count: 0,
            pointer: None,
            pointer_buttons: 0,
            document: editor::Document::new(),
            editor_loaded: false,
            editor_action: None,
            editor_status: Line::new(),
            editor_deferred: None,
            editor_choice: 0,
            file_portal: None,
            file_portal_action: None,
            snap_event: None,
            notifications: notifications::Center::new(),
            notification_open: false,
            notification_selected: None,
            notification_control: 0,
            notification_toast: None,
            shortcuts: shortcuts::Model::new(),
        }
    }
    /// Populate startup messages in place. This does not construct another Shell.
    pub fn initialize_defaults(&mut self) {
        self.status.set("Hardware status has not been supplied.");
        self.push("Welcome to Next Terminal. Type help to get started.");
        self.push("Tip: Alt+Tab switches windows; F10 opens Home.");
    }
    pub fn new() -> Self {
        let mut result = Self::empty();
        result.initialize_defaults();
        result
    }
    pub fn desktop(&self) -> &Desktop {
        &self.desktop
    }
    pub fn calculator(&self) -> &calculator_app::CalculatorApp {
        &self.calculator
    }
    pub fn set_viewport(&mut self, width: usize, height: usize) {
        self.desktop.viewport(width, height);
        let (w, h) = self.desktop.dimensions();
        self.native_windows.viewport(w, h);
        self.sync_native_placement();
    }
    pub fn open_app(&mut self, app: App) {
        self.client_input.suspend();
        self.desktop.open(app);
        self.native_windows
            .raise(native_windows::WindowRef::Builtin(app));
        self.ensure_files_directory_loaded(app);
    }
    fn ensure_files_directory_loaded(&mut self, app: App) {
        if app == App::Files
            && self.files_directory.listing().is_none()
            && !self.files_directory.busy()
        {
            let _ = self
                .files_directory
                .navigate(files_actions::Path::new("/native").unwrap());
        }
    }
    pub fn window_action(&mut self, action: Action) {
        self.apply_window_action(action);
    }
    /// Add real diagnostic output without synthesizing a user command.
    pub fn push_output(&mut self, text: &str) {
        self.append_output(text);
    }
    /// Append actual process output, splitting CRLF and retaining long lines
    /// in bounded chunks; rendering wraps again for the current window size.
    pub fn append_output(&mut self, text: &str) {
        for line in text.lines() {
            wrap_text(line, LINE_CAPACITY, |part| self.push(part));
        }
    }
    /// Supply the result of the kernel's external command dispatcher before
    /// sending Enter. This consumes exactly one pending response.
    pub fn set_command_response(&mut self, text: &str) {
        let mut lines = [Line::new(); HISTORY_CAPACITY];
        let mut count = 0;
        for text in text.lines() {
            wrap_text(text, LINE_CAPACITY, |part| {
                if count == HISTORY_CAPACITY {
                    lines.copy_within(1..HISTORY_CAPACITY, 0);
                    count -= 1;
                }
                lines[count].set(part);
                count += 1;
            });
        }
        self.response = Some(lines);
        self.response_count = count;
    }
    pub fn set_monitor_line(&mut self, index: usize, text: &str) -> bool {
        if let Some(line) = self.monitor.get_mut(index) {
            line.set(text);
            true
        } else {
            false
        }
    }
    pub fn set_monitor_process(
        &mut self,
        index: usize,
        pid: u64,
        state: &str,
        preemptions: u64,
    ) -> bool {
        if let Some(line) = self.monitor.get_mut(index) {
            use core::fmt::Write;
            line.len = 0;
            write!(line, "PID {} {} / preemptions {}", pid, state, preemptions).is_ok()
        } else {
            false
        }
    }
    pub fn set_system_line(&mut self, index: usize, text: &str) -> bool {
        if let Some(line) = self.system.get_mut(index) {
            line.set(text);
            true
        } else {
            false
        }
    }
    /// Development file panel: supplied file content only, never a synthetic
    /// directory or writeable file-system claim.
    pub fn set_file(&mut self, index: usize, path: &str, content: &str) -> bool {
        if let Some(file) = self.files.get_mut(index) {
            file.path.set(path);
            file.content.set(content);
            file.available = !path.is_empty();
            if self
                .file_pending_selection
                .is_some_and(|p| p.as_str() == path)
            {
                self.file_selection = index;
                self.file_pending_selection = None;
            }
            true
        } else {
            false
        }
    }
    pub fn load_appearance(
        &mut self,
        loaded: appearance::Loaded,
    ) -> Result<(), appearance_ui::Error> {
        self.appearance.set_loaded(loaded)
    }
    pub fn appearance_load_error(&mut self) {
        self.appearance.corruption_error();
    }
    pub fn active_appearance(&self) -> appearance::Preferences {
        self.appearance.active()
    }
    pub fn appearance_state(&self) -> &appearance_ui::AppearanceUi {
        &self.appearance
    }
    pub fn take_appearance_request(&mut self) -> Option<appearance_ui::SaveRequest> {
        self.appearance.take_request()
    }
    pub fn appearance_save_result(&mut self, id: u64, success: bool) -> bool {
        self.appearance.saved(id, success)
    }
    fn accent_foreground(&self) -> u32 {
        palette::Palette::new(self.active_appearance()).accent_foreground()
    }
    fn sync_settings_focus(&mut self) {
        let _ = self.file_focus.replace_scope(2, &[]);
        let _ = self.file_focus.replace_scope(3, &[]);
        let _ = self.file_focus.replace_scope(6, &[]);
        let _ = self.file_focus.replace_scope(7, &[]);
        if self.file_focus.active_scope() != 5 {
            let _ = self.file_focus.set_base_scope(5);
        }
        let enabled = self.appearance.loaded()
            && !matches!(
                self.appearance.status(),
                appearance_ui::Status::Saving
                    | appearance_ui::Status::Corrupt
                    | appearance_ui::Status::UnsupportedScale
            );
        let controls = [
            Self::file_control(5, 1, "Theme", keyboard_focus::Role::ListItem, enabled),
            Self::file_control(
                5,
                2,
                "Accent color",
                keyboard_focus::Role::ListItem,
                enabled,
            ),
            Self::file_control(
                5,
                3,
                "Save appearance",
                keyboard_focus::Role::Button,
                enabled,
            ),
            Self::file_control(
                5,
                4,
                "Revert unsaved appearance",
                keyboard_focus::Role::Button,
                enabled,
            ),
            Self::file_control(
                5,
                5,
                "System diagnostics",
                keyboard_focus::Role::Button,
                true,
            ),
        ];
        let _ = self.file_focus.replace_scope(5, &controls);
    }
    fn settings_key(&mut self, key: Key) {
        self.sync_settings_focus();
        if let Some(id) = self.file_focus.focused() {
            let _ = self.file_focus.select(id, keyboard_focus::Input::Keyboard);
        }
        if self.settings_diagnostics {
            if key == Key::Escape {
                self.settings_diagnostics = false;
            }
            return;
        }
        if matches!(key, Key::Tab | Key::BackTab) {
            self.file_focus.traverse(if key == Key::Tab {
                keyboard_focus::Direction::Forward
            } else {
                keyboard_focus::Direction::Reverse
            });
            return;
        }
        let local = self.file_focus.focused().map_or(5, |id| id.local);
        let offset = if matches!(key, Key::Left | Key::Up) {
            -1
        } else {
            1
        };
        if matches!(
            key,
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Enter
        ) {
            match local {
                1 => {
                    let themes = [
                        appearance::Theme::Light,
                        appearance::Theme::Dark,
                        appearance::Theme::HighContrast,
                    ];
                    let current = self.appearance.draft().theme as i32;
                    let _ = self
                        .appearance
                        .choose_theme(themes[(current + offset + 3) as usize % 3]);
                }
                2 => {
                    let accents = [
                        appearance::Accent::Blue,
                        appearance::Accent::Violet,
                        appearance::Accent::Teal,
                        appearance::Accent::Orange,
                    ];
                    let current = self.appearance.draft().accent as i32;
                    let _ = self
                        .appearance
                        .choose_accent(accents[(current + offset + 4) as usize % 4]);
                }
                3 if key == Key::Enter => {
                    let _ = self.appearance.apply();
                }
                4 if key == Key::Enter => {
                    let _ = self.appearance.cancel();
                }
                5 if key == Key::Enter => self.settings_diagnostics = true,
                _ => {}
            }
        }
    }
    pub fn appearance_control_bounds(&self, group: usize, index: usize) -> Option<Bounds> {
        let b = self.desktop.window(App::Settings).bounds;
        let (count, y) = match group {
            0 => (3, 72),
            1 => (4, 136),
            2 => (3, 196),
            _ => return None,
        };
        if index >= count {
            return None;
        }
        let width = ((b.width - 32) / count as i32).clamp(0, 132);
        Some(Bounds {
            x: b.x + 12 + index as i32 * (width + 4),
            y: b.y + y,
            width,
            height: 32,
        })
    }
    fn settings_pointer(&mut self, x: i32, y: i32) -> bool {
        if self.settings_diagnostics {
            return false;
        }
        self.sync_settings_focus();
        for group in 0..3 {
            for index in 0..if group == 1 { 4 } else { 3 } {
                if self
                    .appearance_control_bounds(group, index)
                    .is_some_and(|b| Self::hit(b, x, y))
                {
                    let local = if group == 0 {
                        1
                    } else if group == 1 {
                        2
                    } else {
                        index as u32 + 3
                    };
                    if self
                        .file_focus
                        .select(
                            keyboard_focus::ControlId { scope: 5, local },
                            keyboard_focus::Input::Pointer,
                        )
                        .is_ok()
                    {
                        match group {
                            0 => {
                                let _ = self.appearance.choose_theme(
                                    [
                                        appearance::Theme::Light,
                                        appearance::Theme::Dark,
                                        appearance::Theme::HighContrast,
                                    ][index],
                                );
                            }
                            1 => {
                                let _ = self.appearance.choose_accent(
                                    [
                                        appearance::Accent::Blue,
                                        appearance::Accent::Violet,
                                        appearance::Accent::Teal,
                                        appearance::Accent::Orange,
                                    ][index],
                                );
                            }
                            _ => self.settings_key(Key::Enter),
                        }
                    }
                    return true;
                }
            }
        }
        false
    }
    pub fn set_files_revision(&mut self, revision: u64) {
        self.files_revision = revision;
    }
    pub fn files_revision(&self) -> u64 {
        self.files_revision
    }
    pub fn file_dialog_active(&self) -> bool {
        self.file_dialog.phase() != files_actions::Phase::Idle
    }
    pub fn file_dialog(&self) -> &files_actions::Dialog {
        &self.file_dialog
    }
    pub fn file_status(&self) -> &str {
        self.file_status.as_str()
    }
    pub fn files_directory(&self) -> &files_directory::Model {
        &self.files_directory
    }
    pub fn take_files_directory_action(&mut self) -> Option<files_directory::Action> {
        self.files_directory.take()
    }
    pub fn files_directory_action_result(
        &mut self,
        request: u64,
        result: Result<files_directory::Listing, files_directory::BackendError>,
    ) -> Result<(), files_directory::Error> {
        if let Err(error) = self.files_directory.complete(request, result) {
            self.file_status.set(match error {
                files_directory::Error::Busy => "Finish the current folder action first.",
                files_directory::Error::InvalidPath => "Use a valid folder path under /native.",
                files_directory::Error::InvalidListing => {
                    "The store returned an invalid folder listing."
                }
                files_directory::Error::Unavailable => "The folder view is unavailable.",
                files_directory::Error::Exhausted => "Folder request limit reached.",
                files_directory::Error::StaleCompletion => {
                    "This folder action is no longer current."
                }
            });
            self.sync_files_focus();
            return Err(error);
        }
        self.file_status.set("");
        self.sync_selected_directory_file();
        self.sync_files_focus();
        Ok(())
    }
    pub fn take_file_action(&mut self) -> Option<files_actions::Action> {
        self.file_dialog.take_action()
    }
    pub fn take_file_outcome(&mut self) -> Option<files_actions::Outcome> {
        self.file_outcome.take()
    }
    pub fn file_destination_result(
        &mut self,
        request: u64,
        result: Result<Option<files_actions::Snapshot>, files_actions::BackendError>,
    ) -> Result<(), files_actions::Error> {
        self.file_dialog.destination_result(request, result)?;
        self.sync_file_dialog_focus();
        Ok(())
    }
    pub fn file_action_result(
        &mut self,
        request: u64,
        result: Result<(), files_actions::BackendError>,
    ) -> Result<(), files_actions::Error> {
        let refresh_directory = result.is_ok() && self.files_directory.current_path().is_some();
        self.file_dialog.complete(request, result)?;
        if let Some(outcome) = self.file_dialog.take_outcome() {
            self.file_pending_selection = match outcome {
                files_actions::Outcome::Created(path)
                | files_actions::Outcome::Renamed { new: path, .. }
                | files_actions::Outcome::Copied { new: path, .. } => Some(path),
                _ => None,
            };
            self.file_status.set(match outcome {
                files_actions::Outcome::Created(_) => "File created.",
                files_actions::Outcome::Renamed { .. } => "File renamed.",
                files_actions::Outcome::Copied { .. } => "File copied.",
                files_actions::Outcome::Deleted(_) => "File permanently deleted.",
            });
            self.file_outcome = Some(outcome);
        }
        if refresh_directory {
            if let Some(path) = self
                .files_directory
                .current_path()
                .and_then(|path| files_actions::Path::new(path).ok())
            {
                let _ = self.files_directory.navigate(path);
            }
        }
        self.sync_file_dialog_focus();
        self.sync_files_focus();
        Ok(())
    }
    pub fn keyboard_focus_state(&self) -> Option<keyboard_focus::State<'static>> {
        self.file_focus
            .focused()
            .and_then(|id| self.file_focus.state(id))
    }
    fn file_list_rows(&self) -> usize {
        ((self.desktop.window(App::Files).bounds.height - 180).max(24) / 24)
            .min(FILE_CAPACITY as i32) as usize
    }
    fn file_list_start(&self) -> usize {
        (0..self.file_view_len())
            .position(|row| self.file_view_index(row) == Some(self.file_selection))
            .unwrap_or(0)
            .saturating_sub(self.file_list_rows().saturating_sub(1))
    }
    pub fn file_action_button_bounds(&self, index: usize) -> Option<Bounds> {
        if index >= 6 {
            return None;
        }
        let b = self.desktop.window(App::Files).bounds;
        let width = ((b.width - 56) / 6).clamp(0, 96);
        Some(Bounds {
            x: b.x + 12 + index as i32 * (width + 8),
            y: b.y + 52,
            width,
            height: 32,
        })
    }
    fn files_directory_dialog_bounds(&self) -> Bounds {
        let (w, h) = self.desktop.dimensions();
        let width = (w - 32).clamp(0, 520);
        let height = (h - 120).clamp(0, 220);
        Bounds {
            x: (w - width) / 2,
            y: (h - height) / 2,
            width,
            height,
        }
    }
    fn files_directory_dialog_button_bounds(&self, index: usize) -> Bounds {
        let b = self.files_directory_dialog_bounds();
        Bounds {
            x: b.x + 16 + index as i32 * 124,
            y: b.y + b.height - 44,
            width: 112,
            height: 32,
        }
    }
    pub fn file_dialog_bounds(&self) -> Bounds {
        let (w, h) = self.desktop.dimensions();
        let width = (w - 24).clamp(0, 600);
        let height = (h - 100).clamp(0, 300);
        Bounds {
            x: (w - width) / 2,
            y: (h - height) / 2,
            width,
            height,
        }
    }
    fn file_control(
        scope: u32,
        local: u32,
        label: &'static str,
        role: keyboard_focus::Role,
        enabled: bool,
    ) -> keyboard_focus::Control<'static> {
        keyboard_focus::Control {
            id: keyboard_focus::ControlId { scope, local },
            label,
            role,
            enabled,
            visible: true,
            tab_stop: true,
            checked: None,
        }
    }
    fn sync_files_focus(&mut self) {
        if self.file_dialog_active() || self.files_directory.dialog_active() {
            return;
        }
        if self.files_directory.listing().is_some() {
            let _ = self.file_focus.replace_scope(3, &[]);
            let _ = self.file_focus.replace_scope(5, &[]);
            let _ = self.file_focus.replace_scope(6, &[]);
            let _ = self.file_focus.replace_scope(7, &[]);
            let selected_file = self
                .files_directory
                .selected()
                .is_some_and(|entry| entry.kind == files_directory::Kind::File);
            let controls = [
                Self::file_control(2, 1, "Directory list", keyboard_focus::Role::ListItem, true),
                Self::file_control(2, 2, "Go up one folder", keyboard_focus::Role::Button, true),
                Self::file_control(2, 3, "Create folder", keyboard_focus::Role::Button, true),
                Self::file_control(
                    2,
                    4,
                    "Move selected item",
                    keyboard_focus::Role::Button,
                    selected_file || self.files_directory.can_move_selected(),
                ),
                Self::file_control(
                    2,
                    5,
                    "Remove selected folder or trash file",
                    keyboard_focus::Role::Button,
                    selected_file || self.files_directory.can_remove_selected(),
                ),
                Self::file_control(2, 6, "Open Trash", keyboard_focus::Role::Button, true),
                Self::file_control(2, 7, "Find files", keyboard_focus::Role::Button, true),
                Self::file_control(2, 8, "Address path", keyboard_focus::Role::TextBox, true),
            ];
            if self.file_focus.active_scope() != 2 {
                let _ = self.file_focus.set_base_scope(2);
            }
            let _ = self.file_focus.replace_scope(2, &controls);
            return;
        }
        let _ = self.file_focus.replace_scope(5, &[]);
        let _ = self.file_focus.replace_scope(3, &[]);
        let _ = self.file_focus.replace_scope(6, &[]);
        let _ = self.file_focus.replace_scope(7, &[]);
        let mutable = self.file_selection != 0 && self.files[self.file_selection].available;
        let controls = [
            Self::file_control(2, 1, "File list", keyboard_focus::Role::ListItem, true),
            Self::file_control(2, 2, "Create file", keyboard_focus::Role::Button, true),
            Self::file_control(
                2,
                3,
                "Rename selected file",
                keyboard_focus::Role::Button,
                mutable,
            ),
            Self::file_control(
                2,
                4,
                "Move selected file to Trash",
                keyboard_focus::Role::Button,
                mutable,
            ),
            Self::file_control(
                2,
                5,
                "Copy selected file",
                keyboard_focus::Role::Button,
                mutable,
            ),
            Self::file_control(2, 6, "Open Trash", keyboard_focus::Role::Button, true),
            Self::file_control(
                2,
                7,
                "Find and sort files",
                keyboard_focus::Role::Button,
                true,
            ),
        ];
        if self.file_focus.active_scope() != 2 {
            let _ = self.file_focus.set_base_scope(2);
        }
        let _ = self.file_focus.replace_scope(2, &controls);
    }
    fn sync_files_directory_dialog_focus(&mut self) {
        if !self.files_directory.dialog_active() {
            let _ = self.file_focus.pop_modal(8);
            self.sync_files_focus();
            return;
        }
        let editable = matches!(
            self.files_directory.dialog_kind(),
            Some(
                files_directory::DialogKind::CreateFolder | files_directory::DialogKind::MoveFolder
            )
        );
        let label = match self.files_directory.dialog_kind() {
            Some(files_directory::DialogKind::CreateFolder) => "Create folder",
            Some(files_directory::DialogKind::MoveFolder) => "Move folder",
            Some(files_directory::DialogKind::RemoveFolder) => "Remove empty folder",
            None => "Folder operation",
        };
        let controls = [
            Self::file_control(8, 1, "Folder path", keyboard_focus::Role::TextBox, editable),
            Self::file_control(
                8,
                2,
                "Cancel",
                keyboard_focus::Role::Button,
                !self.files_directory.busy(),
            ),
            Self::file_control(
                8,
                3,
                label,
                keyboard_focus::Role::Button,
                !self.files_directory.busy(),
            ),
        ];
        let _ = self.file_focus.replace_scope(8, &controls);
        if self.file_focus.active_scope() != 8 {
            let _ = self.file_focus.push_modal(8);
        }
        if !self.files_directory.approves() {
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 8, local: 2 },
                keyboard_focus::Input::Keyboard,
            );
        }
    }
    fn sync_selected_directory_file(&mut self) {
        if let Some(entry) = self
            .files_directory
            .selected()
            .filter(|entry| entry.kind == files_directory::Kind::File)
        {
            if let Some(index) = self
                .files
                .iter()
                .position(|file| file.available && file.path.as_str() == entry.path.as_str())
            {
                self.file_selection = index;
            }
        }
    }
    fn begin_files_directory_action(&mut self, index: usize) {
        self.file_status.set("");
        let result = match index {
            0 => self.files_directory.up().map(|_| ()),
            1 => self.files_directory.begin_create(),
            2 if self
                .files_directory
                .selected()
                .is_some_and(|entry| entry.kind == files_directory::Kind::File) =>
            {
                self.sync_selected_directory_file();
                self.begin_file_operation(1);
                return;
            }
            2 => self.files_directory.begin_move(),
            3 if self
                .files_directory
                .selected()
                .is_some_and(|entry| entry.kind == files_directory::Kind::File) =>
            {
                self.sync_selected_directory_file();
                self.begin_file_operation(2);
                return;
            }
            3 => self.files_directory.begin_remove(),
            4 => {
                self.show_trash();
                return;
            }
            5 => {
                self.show_file_query();
                return;
            }
            _ => return,
        };
        if let Err(error) = result {
            self.file_status.set(match error {
                files_directory::Error::Busy => "Finish the current folder action first.",
                files_directory::Error::InvalidPath => "Enter a valid path in /native.",
                files_directory::Error::InvalidListing => "Refresh this folder before continuing.",
                files_directory::Error::Unavailable => "Select an available folder first.",
                files_directory::Error::Exhausted => "Folder request limit reached.",
                files_directory::Error::StaleCompletion => {
                    "This folder action is no longer current."
                }
            });
        }
        self.sync_files_directory_dialog_focus();
    }
    fn files_directory_dialog_key(&mut self, key: Key) {
        match key {
            Key::Escape => self.files_directory.cancel(),
            Key::Tab | Key::BackTab | Key::Left | Key::Right => {
                let _ = self
                    .files_directory
                    .choose_approve(!self.files_directory.approves());
            }
            Key::Backspace => {
                let _ = self.files_directory.backspace_input();
            }
            Key::Character(c) if c.is_ascii() && !c.is_control() => {
                let _ = self.files_directory.push_input(c as u8);
            }
            Key::Enter => {
                let _ = self.files_directory.confirm();
            }
            _ => {}
        }
        self.sync_files_directory_dialog_focus();
    }
    fn files_directory_key(&mut self, key: Key) {
        if self.files_directory.dialog_active() {
            self.files_directory_dialog_key(key);
            return;
        }
        if self.files_directory_address_editing {
            match key {
                Key::Escape => {
                    if let Some(listing) = self.files_directory.listing() {
                        let _ = self
                            .files_directory
                            .set_address(listing.folder.path.as_str());
                    }
                    self.files_directory_address_editing = false;
                }
                Key::Backspace => {
                    let _ = self.files_directory.backspace_address();
                }
                Key::Enter => {
                    if self.files_directory.navigate_address().is_err() {
                        self.file_status
                            .set("Use a valid absolute folder path under /native.");
                    }
                    self.files_directory_address_editing = false;
                }
                Key::Character(c) if c.is_ascii() && !c.is_control() => {
                    let _ = self.files_directory.push_address(c as u8);
                }
                _ => {}
            }
            return;
        }
        match key {
            Key::Down | Key::Character('j') => {
                self.files_directory.move_selection(true);
                self.sync_selected_directory_file();
                self.sync_files_focus();
            }
            Key::Up | Key::Character('k') => {
                self.files_directory.move_selection(false);
                self.sync_selected_directory_file();
                self.sync_files_focus();
            }
            Key::Backspace => {
                let _ = self.files_directory.up();
            }
            Key::Character('g') => {
                let _ = self.files_directory.set_address("");
                self.files_directory_address_editing = true;
            }
            Key::Character('n') => self.begin_files_directory_action(1),
            Key::Character('f') => self.begin_file_operation(0),
            Key::Character('m') => self.begin_files_directory_action(2),
            Key::Character('r') => {
                self.sync_selected_directory_file();
                if self
                    .files_directory
                    .selected()
                    .is_some_and(|entry| entry.kind == files_directory::Kind::File)
                {
                    self.begin_file_operation(1);
                } else {
                    self.begin_files_directory_action(2);
                }
            }
            Key::Character('c')
                if self
                    .files_directory
                    .selected()
                    .is_some_and(|entry| entry.kind == files_directory::Kind::File) =>
            {
                self.sync_selected_directory_file();
                self.begin_file_operation(3);
            }
            Key::Character('t')
                if self
                    .files_directory
                    .selected()
                    .is_some_and(|entry| entry.kind == files_directory::Kind::File) =>
            {
                self.sync_selected_directory_file();
                self.begin_file_operation(2);
            }
            Key::Delete => self.begin_files_directory_action(3),
            Key::Enter => {
                let focused = self.file_focus.focused().map(|id| id.local);
                match focused {
                    Some(local @ 2..=7) => self.begin_files_directory_action(local as usize - 2),
                    Some(8) => self.files_directory_address_editing = true,
                    _ => {
                        if let Ok(Some(path)) = self.files_directory.open_selected() {
                            if let Err(error) = self.request_editor_open(path.as_str()) {
                                self.editor_status.set(error.message());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        self.sync_files_focus();
    }
    fn sync_file_dialog_focus(&mut self) {
        use files_actions::Phase;
        if !self.file_dialog_active() {
            let _ = self.file_focus.pop_modal(3);
            return;
        }
        let phase = self.file_dialog.phase();
        let editing = phase == Phase::Editing;
        let controls = [
            Self::file_control(3, 1, "File path", keyboard_focus::Role::TextBox, editing),
            Self::file_control(
                3,
                2,
                "Cancel",
                keyboard_focus::Role::Button,
                phase != Phase::Committing,
            ),
            Self::file_control(
                3,
                3,
                if phase == Phase::ConfirmDelete {
                    "Permanently delete"
                } else if phase == Phase::ConfirmOverwrite {
                    "Replace destination file"
                } else if self.file_dialog.is_copy() {
                    "Copy file"
                } else if self.file_dialog.selected().is_some() {
                    "Rename file"
                } else {
                    "Create file"
                },
                keyboard_focus::Role::Button,
                !matches!(phase, Phase::Inspecting | Phase::Committing),
            ),
        ];
        let changed = self
            .file_focus
            .state(keyboard_focus::ControlId { scope: 3, local: 1 })
            .is_some_and(|state| state.control.enabled != editing);
        let _ = self.file_focus.replace_scope(3, &controls);
        if self.file_focus.active_scope() != 3 {
            let _ = self.file_focus.push_modal(3);
        }
        if !editing && changed
            || matches!(phase, Phase::ConfirmDelete | Phase::ConfirmOverwrite)
                && !self.file_dialog.approved()
        {
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 3, local: 2 },
                keyboard_focus::Input::Keyboard,
            );
        }
    }
    fn begin_file_operation(&mut self, index: usize) {
        if index == 5 {
            self.show_file_query();
            return;
        }
        if index == 4 {
            self.show_trash();
            return;
        }
        if index == 2 {
            self.begin_trash_move();
            return;
        }
        self.sync_files_focus();
        let source = files_actions::Path::new(self.files[self.file_selection].path.as_str())
            .ok()
            .map(|path| files_actions::Snapshot {
                path,
                revision: self.files_revision,
            });
        let result = match index {
            0 => self.file_dialog.begin_create(
                files_actions::Namespace::new(
                    self.files_directory.current_path().unwrap_or("/native"),
                )
                .unwrap(),
            ),
            1 | 3 => source
                .filter(|_| self.file_selection != 0)
                .ok_or(files_actions::Error::InvalidPath)
                .and_then(|source| {
                    let text = source.path.as_str();
                    let parent = text.rsplit_once('/').map_or("/native", |(parent, _)| {
                        if parent.is_empty() {
                            "/"
                        } else {
                            parent
                        }
                    });
                    let namespace = if self.files_directory.listing().is_some() {
                        files_actions::Namespace::new("/native")
                    } else {
                        files_actions::Namespace::new(parent)
                    };
                    namespace.and_then(|namespace| {
                        if index == 3 {
                            self.file_dialog.begin_copy(source, namespace)
                        } else {
                            self.file_dialog.begin_rename(source, namespace)
                        }
                    })
                }),
            _ => source
                .filter(|_| self.file_selection != 0)
                .ok_or(files_actions::Error::InvalidPath)
                .and_then(|source| self.file_dialog.begin_delete(source)),
        };
        if result.is_ok() {
            if index == 0 {
                let mut address = Line::new();
                use core::fmt::Write;
                let _ = write!(
                    address,
                    "{}/",
                    self.files_directory.current_path().unwrap_or("/native")
                );
                let _ = self.file_dialog.set_input(address.as_str());
            }
            self.sync_file_dialog_focus();
        } else {
            self.file_status
                .set("Select a stored file; the boot note is read-only.");
        }
    }
    fn file_dialog_key(&mut self, key: Key) {
        use files_actions::Phase;
        self.sync_file_dialog_focus();
        if matches!(key, Key::Character(_) | Key::Backspace) {
            if let Some(id) = self.file_focus.focused() {
                let _ = self.file_focus.select(id, keyboard_focus::Input::Keyboard);
            }
        }
        match key {
            Key::Escape => {
                let _ = self.file_dialog.cancel();
            }
            Key::Tab | Key::BackTab => {
                self.file_focus.traverse(if key == Key::Tab {
                    keyboard_focus::Direction::Forward
                } else {
                    keyboard_focus::Direction::Reverse
                });
                if matches!(
                    self.file_dialog.phase(),
                    Phase::ConfirmDelete | Phase::ConfirmOverwrite
                ) {
                    let _ = self
                        .file_dialog
                        .choose_approve(self.file_focus.focused().is_some_and(|id| id.local == 3));
                }
            }
            Key::Character(c) => {
                if self.file_focus.focused().is_some_and(|id| id.local == 1) {
                    let _ = self.file_dialog.insert(c);
                }
            }
            Key::Backspace => {
                if self.file_focus.focused().is_some_and(|id| id.local == 1) {
                    let _ = self.file_dialog.backspace();
                }
            }
            Key::Enter => {
                if self.file_focus.focused().is_some_and(|id| id.local == 2) {
                    let _ = self.file_dialog.cancel();
                } else {
                    let _ = self.file_dialog.activate();
                }
            }
            _ => {}
        }
        self.sync_file_dialog_focus();
    }
    fn file_dialog_pointer(&mut self, x: i32, y: i32) {
        let b = self.file_dialog_bounds();
        if y >= b.y + 88
            && y < b.y + 128
            && x >= b.x + 20
            && x < b.x + b.width - 20
            && self.file_dialog.phase() == files_actions::Phase::Editing
        {
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 3, local: 1 },
                keyboard_focus::Input::Pointer,
            );
            return;
        }
        if y >= b.y + b.height - 60 && y < b.y + b.height - 24 {
            for (local, start) in [(2, b.x + 20), (3, b.x + 144)] {
                if x >= start && x < start + 112 {
                    if self
                        .file_focus
                        .select(
                            keyboard_focus::ControlId { scope: 3, local },
                            keyboard_focus::Input::Pointer,
                        )
                        .is_ok()
                    {
                        let _ = self.file_dialog.choose_approve(local == 3);
                        self.file_dialog_key(Key::Enter);
                    }
                    return;
                }
            }
        }
    }
    /// Remove a deleted or no-longer-bound file. Index zero is the boot note;
    /// callers can refresh storage bindings at indices one through eight.
    pub fn clear_file(&mut self, index: usize) -> bool {
        let Some(file) = self.files.get_mut(index) else {
            return false;
        };
        *file = FileEntry::empty();
        if self.file_selection == index {
            self.file_selection = self
                .files
                .iter()
                .position(|entry| entry.available)
                .unwrap_or(0);
            self.file_detail = false;
        }
        true
    }
    #[allow(clippy::too_many_arguments)]
    pub fn submit_client_surface(
        &mut self,
        slot: usize,
        generation: u64,
        frame_generation: u64,
        owner_pid: u64,
        verified_package: &str,
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
        pixels: &[u8],
    ) -> Result<(), client_surface::Error> {
        let id = client_interaction::SurfaceRef { slot, generation };
        if self.client_configuration(id).is_some() && !self.client_content_available(id) {
            return Err(client_surface::Error::Stale);
        }
        self.submit_client_surface_unchecked_configuration(
            slot,
            generation,
            frame_generation,
            owner_pid,
            verified_package,
            width,
            height,
            stride,
            format,
            pixels,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn submit_client_surface_unchecked_configuration(
        &mut self,
        slot: usize,
        generation: u64,
        frame_generation: u64,
        owner_pid: u64,
        verified_package: &str,
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
        pixels: &[u8],
    ) -> Result<(), client_surface::Error> {
        self.client_previews.submit(
            slot,
            generation,
            frame_generation,
            owner_pid,
            verified_package,
            width,
            height,
            stride,
            format,
            pixels,
        )?;
        if self
            .native_resize
            .preview()
            .is_some_and(|a| a.surface.slot == slot && a.surface.generation != generation)
        {
            self.native_resize.cancel();
        }
        let (w, h) = self.desktop.dimensions();
        let id = client_interaction::SurfaceRef { slot, generation };
        let mut bounds = self.client_previews.bounds(slot, w, h).unwrap_or(Bounds {
            x: 4,
            y: 4,
            width: 292,
            height: 240,
        });
        bounds.height = bounds.height.max(240).min(h.saturating_sub(88).max(240));
        let new = self.native_window_presentation(id).is_none();
        self.native_windows.install(id, bounds);
        if new {
            self.native_windows.viewport(w, h);
        }
        self.sync_native_placement();
        if self
            .client_input
            .context()
            .is_some_and(|id| id.slot == slot && id.generation != generation)
        {
            self.client_input.suspend();
        }
        Ok(())
    }
    pub fn set_client_configuration(
        &mut self,
        id: client_interaction::SurfaceRef,
        configuration: u64,
    ) -> Result<(), client_surface::Error> {
        self.client_previews.set_configuration(id, configuration)
    }
    pub fn client_configuration(&self, id: client_interaction::SurfaceRef) -> Option<u64> {
        self.client_previews.configuration(id)
    }
    pub fn client_content_available(&self, id: client_interaction::SurfaceRef) -> bool {
        self.client_previews.content_available(id)
    }
    pub fn retire_client_content(
        &mut self,
        id: client_interaction::SurfaceRef,
        expected: u64,
        new: u64,
    ) -> Result<(), client_surface::Error> {
        self.client_previews.retire_content(id, expected, new)?;
        if self.client_input.context() == Some(id) {
            self.client_input.revoke();
        }
        if self
            .native_resize
            .preview()
            .is_some_and(|a| a.surface == id)
            && !self.native_resize.pending()
        {
            self.native_resize.cancel();
        }
        self.native_windows.finish_drag();
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn submit_client_surface_configured(
        &mut self,
        slot: usize,
        generation: u64,
        frame_generation: u64,
        configuration: u64,
        owner_pid: u64,
        verified_package: &str,
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
        pixels: &[u8],
    ) -> Result<(), client_surface::Error> {
        let id = client_interaction::SurfaceRef { slot, generation };
        if configuration == 0 {
            return Err(client_surface::Error::Stale);
        }
        if let Some(current) = self.client_configuration(id) {
            if configuration != current {
                return Err(client_surface::Error::Stale);
            }
        }
        self.submit_client_surface_unchecked_configuration(
            slot,
            generation,
            frame_generation,
            owner_pid,
            verified_package,
            width,
            height,
            stride,
            format,
            pixels,
        )?;
        self.set_client_configuration(id, configuration)
    }
    pub fn set_client_close_status(
        &mut self,
        surface: client_interaction::SurfaceRef,
        status: client_surface::CloseStatus,
    ) -> bool {
        self.client_previews.set_close_status(surface, status)
    }
    pub fn client_close_status(
        &self,
        surface: client_interaction::SurfaceRef,
    ) -> Option<client_surface::CloseStatus> {
        self.client_previews.close_status(surface)
    }
    pub fn client_close_button_bounds(&self, slot: usize) -> Option<Bounds> {
        let (w, h) = self.desktop.dimensions();
        self.client_previews.close_button_bounds(slot, w, h)
    }
    fn request_client_close(&mut self, surface: client_interaction::SurfaceRef) {
        if self
            .client_previews
            .close_status(surface)
            .is_some_and(|status| status.requestable())
        {
            self.client_input.request_close(surface);
        }
    }
    pub fn client_input_allowed(&self) -> bool {
        self.file_portal.is_none()
            && self.editor_deferred.is_none()
            && !self.file_dialog_active()
            && !self.file_query_dialog_active()
            && !self.trash_dialog_active()
            && !self.notification_open
            && !self.launcher
            && self.snap_preview().is_none()
    }
    pub fn client_context_active(&self) -> bool {
        self.client_input.context_active()
    }
    pub fn client_focused(&self) -> Option<client_interaction::SurfaceRef> {
        self.client_input.focused()
    }
    pub fn client_leave_context(&mut self) {
        self.client_input.suspend();
    }
    pub fn client_input_failed(&mut self) {
        self.client_input.revoke();
    }
    pub fn take_client_action(&mut self) -> Option<client_interaction::Action> {
        if !self.client_input_allowed() {
            self.client_input.suspend();
        }
        self.client_input.take()
    }
    pub fn client_focus_result(
        &mut self,
        surface: client_interaction::SurfaceRef,
        accepted: bool,
    ) -> bool {
        if !self.client_input_allowed()
            || self.client_previews.identity(surface.slot) != Some(surface)
        {
            self.client_input.suspend();
            return false;
        }
        self.client_input.focus_result(surface, accepted)
    }
    pub fn client_content_bounds(&self, slot: usize) -> Option<Bounds> {
        let (w, h) = self.desktop.dimensions();
        self.client_previews.pixel_bounds(slot, w, h)
    }
    pub fn clear_client_surface(
        &mut self,
        slot: usize,
        generation: u64,
    ) -> Result<(), client_surface::Error> {
        self.client_previews.clear(slot, generation)?;
        if self
            .native_resize
            .preview()
            .is_some_and(|a| a.surface == client_interaction::SurfaceRef { slot, generation })
        {
            self.native_resize.cancel();
        }
        self.native_windows
            .remove(client_interaction::SurfaceRef { slot, generation });
        if self
            .client_input
            .context()
            .is_some_and(|id| id.slot == slot && id.generation == generation)
        {
            self.client_input.suspend();
        }
        Ok(())
    }
    pub fn editor(&self) -> &editor::Document {
        &self.document
    }
    pub fn take_editor_action(&mut self) -> Option<EditorAction> {
        self.editor_action.take()
    }
    pub fn request_editor_open(&mut self, path: &str) -> Result<(), EditorError> {
        let path = EditorPath::new(path)?;
        self.client_input.suspend();
        self.desktop.open(App::Editor);
        self.native_windows
            .raise(native_windows::WindowRef::Builtin(App::Editor));
        if self.document.dirty() {
            self.editor_deferred = Some(EditorDeferred::Open(path));
            self.editor_choice = 0;
        } else {
            self.editor_action = Some(EditorAction::Open { path });
        }
        Ok(())
    }
    pub fn request_editor_save(&mut self) {
        if self.notification_open
            || self.file_portal.is_some()
            || self.file_dialog_active()
            || self.trash_dialog_active()
        {
            return;
        }
        if !self.editor_loaded {
            self.editor_status
                .set("Open the file first; no storage baseline is available.");
            return;
        }
        self.editor_action = Some(self.document.save_action());
        self.editor_status.set("Saving document...");
    }
    pub fn load_document(&mut self, path: &str, text: &str) -> Result<(), EditorError> {
        if self.document.dirty() {
            return Err(EditorError::UnsavedChanges);
        }
        self.document.load(path, text)?;
        self.editor_loaded = true;
        self.editor_status.set("Loaded document.");
        self.open_app(App::Editor);
        Ok(())
    }
    pub fn editor_load_failed(&mut self, message: &str) {
        self.editor_status.set(message);
    }
    pub fn editor_save_result(&mut self, revision: u64, result: Result<(), &str>) {
        match result {
            Ok(()) => {
                self.document.saved(revision);
                if self.document.dirty() {
                    self.editor_status
                        .set("Saved older revision; newer edits remain unsaved.");
                } else {
                    self.editor_status.set("Saved document.");
                    if let Some(action) = self.editor_deferred.take() {
                        self.perform_editor_deferred(action);
                    }
                }
            }
            Err(message) => self.editor_status.set(message),
        }
    }
    fn apply_window_action(&mut self, action: Action) {
        if action == Action::Open(App::Editor) && !self.document.dirty() {
            let path = EditorPath::new(self.document.path()).expect("document paths validated");
            if let Err(error) = self.request_editor_open(path.as_str()) {
                self.editor_status.set(error.message());
            }
            return;
        }
        if action == Action::Close
            && self.desktop.focused() == Some(App::Editor)
            && self.document.dirty()
        {
            self.editor_deferred = Some(EditorDeferred::Close);
            self.editor_choice = 0;
        } else {
            self.desktop.action(action);
            if let Some(app) = self.desktop.focused() {
                self.native_windows
                    .raise(native_windows::WindowRef::Builtin(app));
                self.ensure_files_directory_loaded(app);
            }
        }
    }
    fn perform_editor_deferred(&mut self, action: EditorDeferred) {
        match action {
            EditorDeferred::Open(path) => self.editor_action = Some(EditorAction::Open { path }),
            EditorDeferred::Close => {
                self.desktop.open(App::Editor);
                self.desktop.action(Action::Close);
            }
        }
    }
    fn editor_key(&mut self, key: Key) {
        let result = match key {
            Key::Character(c) if !c.is_control() => self.document.insert(c),
            Key::Enter => self.document.insert('\n'),
            Key::Backspace => self.document.backspace(),
            Key::Delete => self.document.delete(),
            Key::Left => {
                self.document.left();
                Ok(())
            }
            Key::Right => {
                self.document.right();
                Ok(())
            }
            Key::Up => {
                self.document.up();
                Ok(())
            }
            Key::Down => {
                self.document.down();
                Ok(())
            }
            Key::Home => {
                self.document.home();
                Ok(())
            }
            Key::End => {
                self.document.end();
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.editor_status.set(error.message());
        }
    }
    /// Show kernel-authenticated identity and requested access over real files.
    /// Repeated refreshes of the same request preserve the user's selection.
    pub fn show_file_portal(&mut self, request: u64, pid: u64, package: &str, writable: bool) {
        if request == 0 || self.file_portal.is_some_and(|p| p.request == request) {
            return;
        }
        let mut name = Line::new();
        name.set(package);
        name.len = name.len.min(64);
        self.file_portal = Some(FilePortal {
            request,
            pid,
            package: name,
            writable,
            selection: 0,
            approve: false,
        });
    }
    pub fn file_portal_active(&self) -> bool {
        self.file_portal.is_some()
    }
    /// The displayed selection is resolved against current available bindings.
    pub fn file_portal_selected_path(&self) -> Option<&str> {
        let portal = self.file_portal?;
        self.portal_file(portal.selection)
            .map(|file| file.path.as_str())
    }
    pub fn take_file_portal_action(&mut self) -> Option<FilePortalAction> {
        self.file_portal_action.take()
    }
    /// Expiry/death cannot dismiss a newer request or emit a stale selection.
    pub fn dismiss_file_portal(&mut self, request: u64) {
        if self.file_portal.is_some_and(|p| p.request == request) {
            self.file_portal = None;
        }
        if self
            .file_portal_action
            .is_some_and(|a| a.request() == request)
        {
            self.file_portal_action = None;
        }
    }
    pub fn file_portal_bounds(&self) -> Option<desktop::Bounds> {
        self.file_portal?;
        let (width, height) = self.desktop.dimensions();
        let w = width.clamp(0, 640);
        let h = height.clamp(0, 460);
        Some(desktop::Bounds {
            x: (width - w) / 2,
            y: (height - h) / 2,
            width: w,
            height: h,
        })
    }
    fn portal_file(&self, index: usize) -> Option<&FileEntry> {
        self.files
            .iter()
            .filter(|f| {
                f.available
                    && f.path.as_str().starts_with("/native/")
                    && EditorPath::new(f.path.as_str()).is_ok()
            })
            .nth(index)
    }
    fn portal_count(&self) -> usize {
        (0..FILE_CAPACITY)
            .take_while(|i| self.portal_file(*i).is_some())
            .count()
    }
    fn portal_identity_rows(&self, bounds: desktop::Bounds) -> usize {
        let scale = if self.desktop.dimensions().0 >= 800 {
            2
        } else {
            1
        };
        let columns = ((bounds.width - 32).max(0) / (6 * scale)).max(1) as usize;
        self.file_portal
            .map_or(1, |p| p.package.len.max(1).div_ceil(columns))
    }
    fn portal_list_top(&self, bounds: desktop::Bounds) -> i32 {
        116 + (self.portal_identity_rows(bounds) as i32 - 1) * 24
    }
    fn portal_rows(&self, bounds: desktop::Bounds) -> usize {
        ((bounds.height - self.portal_list_top(bounds) - 68).max(0) / 24).min(FILE_CAPACITY as i32)
            as usize
    }
    fn portal_key(&mut self, key: Key) {
        let Some(mut portal) = self.file_portal else {
            return;
        };
        match key {
            Key::Escape => {
                self.file_portal_action = Some(FilePortalAction::Deny {
                    request: portal.request,
                });
                self.file_portal = None;
                return;
            }
            Key::Tab | Key::BackTab | Key::Left | Key::Right => portal.approve = !portal.approve,
            Key::Down => {
                let n = self.portal_count();
                if n != 0 {
                    portal.selection = (portal.selection + 1) % n;
                }
            }
            Key::Up => {
                let n = self.portal_count();
                if n != 0 {
                    portal.selection = (portal.selection + n - 1) % n;
                }
            }
            Key::Enter => {
                let action = if portal.approve {
                    self.portal_file(portal.selection)
                        .and_then(|f| EditorPath::new(f.path.as_str()).ok())
                        .map(|path| FilePortalAction::Approve {
                            request: portal.request,
                            path,
                        })
                } else {
                    Some(FilePortalAction::Deny {
                        request: portal.request,
                    })
                };
                if let Some(action) = action {
                    self.file_portal_action = Some(action);
                    self.file_portal = None;
                }
                return;
            }
            _ => {}
        }
        self.file_portal = Some(portal);
    }
    fn portal_pointer(&mut self, x: i32, y: i32) {
        let (Some(mut portal), Some(b)) = (self.file_portal, self.file_portal_bounds()) else {
            return;
        };
        let rows = self.portal_rows(b);
        if x >= b.x + 16
            && x < b.x + b.width - 16
            && y >= b.y + self.portal_list_top(b)
            && y < b.y + self.portal_list_top(b) + rows as i32 * 24
        {
            let start = portal.selection.saturating_sub(rows.saturating_sub(1));
            let index = start + ((y - b.y - self.portal_list_top(b)) / 24) as usize;
            if self.portal_file(index).is_some() {
                portal.selection = index;
                self.file_portal = Some(portal);
            }
        } else if y >= b.y + b.height - 68 && y < b.y + b.height - 36 {
            if x >= b.x + 16 && x < b.x + 116 {
                portal.approve = false;
            } else if x >= b.x + 132 && x < b.x + 232 {
                portal.approve = true;
            } else {
                return;
            }
            self.file_portal = Some(portal);
            self.portal_key(Key::Enter);
        }
    }
    pub fn search_query(&self) -> Option<&str> {
        (self.launcher && self.search_mode).then(|| self.search_input.as_str())
    }
    pub fn shortcuts_open(&self) -> bool {
        self.shortcuts.open()
    }
    pub fn shortcuts_selected(&self) -> usize {
        self.shortcuts.selected()
    }
    fn search_result(&self, mut index: usize) -> Option<SearchResult> {
        for app in App::ALL {
            if search_matches(app.title(), self.search_input.as_str()) {
                if index == 0 {
                    return Some(SearchResult::App(app));
                }
                index -= 1;
            }
        }
        for (i, file) in self.files.iter().enumerate() {
            if file.available && search_matches(file.path.as_str(), self.search_input.as_str()) {
                if index == 0 {
                    return Some(SearchResult::File(i));
                }
                index -= 1;
            }
        }
        if !self.search_input.as_str().is_empty()
            && search_matches("Keyboard shortcuts", self.search_input.as_str())
            && index == 0
        {
            return Some(SearchResult::Shortcuts);
        }
        None
    }
    fn search_count(&self) -> usize {
        (0..App::ALL.len() + FILE_CAPACITY + 1)
            .take_while(|i| self.search_result(*i).is_some())
            .count()
    }
    fn activate_search(&mut self) {
        if let Some(result) = self.search_result(self.selection) {
            match result {
                SearchResult::App(app) => self.apply_window_action(Action::Open(app)),
                SearchResult::File(i) => {
                    let path = self.files[i].path;
                    if let Err(error) = self.request_editor_open(path.as_str()) {
                        self.editor_status.set(error.message());
                    }
                }
                SearchResult::Shortcuts => {
                    self.shortcuts.show();
                    self.launcher = false;
                    self.search_mode = false;
                    return;
                }
            }
            self.launcher = false;
            self.search_mode = false;
        }
    }
    fn search_key(&mut self, key: Key) {
        match key {
            Key::Escape => {
                self.search_mode = false;
                self.search_input = Line::new();
                self.selection = 0;
            }
            Key::Character(c) if c.is_ascii() && !c.is_control() => {
                if self.search_input.len < 31 {
                    self.search_input.bytes[self.search_input.len] = c as u8;
                    self.search_input.len += 1;
                    self.selection = 0;
                }
            }
            Key::Backspace => {
                self.search_input.len = self.search_input.len.saturating_sub(1);
                self.selection = 0;
            }
            Key::Down | Key::Tab => {
                let count = self.search_count();
                if count != 0 {
                    self.selection = (self.selection + 1) % count;
                }
            }
            Key::Up | Key::BackTab => {
                let count = self.search_count();
                if count != 0 {
                    self.selection = (self.selection + count - 1) % count;
                }
            }
            Key::Enter => self.activate_search(),
            _ => {}
        }
    }
    pub fn notifications(&self) -> &notifications::Center {
        &self.notifications
    }
    pub fn notifications_open(&self) -> bool {
        self.notification_open
    }
    pub fn notification_selected(&self) -> Option<&notifications::Notification> {
        self.notification_selected
            .and_then(|id| self.notifications.get(id))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn publish_notification(
        &mut self,
        source: notifications::Source,
        label: &str,
        title: &str,
        body: &str,
        priority: notifications::Priority,
        monotonic_ns: u64,
    ) -> Result<notifications::Published, notifications::Error> {
        let published =
            self.notifications
                .publish(source, label, title, body, priority, monotonic_ns)?;
        if self.notification_selected().is_none() {
            self.notification_selected = Some(published.id);
        }
        if published.show_alert {
            self.notification_toast =
                Some((published.id, monotonic_ns.saturating_add(4_000_000_000)));
        }
        Ok(published)
    }
    pub fn toggle_notifications(&mut self) {
        if self.file_portal.is_some()
            || self.editor_deferred.is_some()
            || self.snap_preview().is_some()
        {
            return;
        }
        self.notification_open = !self.notification_open;
        self.notification_control = 0;
        self.notification_toast = None;
        if self.notification_selected().is_none() {
            self.notification_selected = self.notifications.at(0).map(|n| n.id);
        }
    }
    fn notification_index(&self) -> usize {
        (0..self.notifications.len())
            .find(|i| {
                self.notifications
                    .at(*i)
                    .is_some_and(|n| Some(n.id) == self.notification_selected)
            })
            .unwrap_or(0)
    }
    pub fn notification_bounds(&self) -> Bounds {
        let (w, h) = self.desktop.dimensions();
        let width = w.clamp(0, 480);
        let height = (h - 84).clamp(0, 520);
        Bounds {
            x: (w - width - 12).max(0),
            y: (h - height - 84).max(0),
            width,
            height,
        }
    }
    fn notification_rows(&self) -> usize {
        ((self.notification_bounds().height - 220).max(0) / 64).min(notifications::CAPACITY as i32)
            as usize
    }
    fn notification_key(&mut self, key: Key) {
        match key {
            Key::Escape | Key::Notifications => self.notification_open = false,
            Key::Tab => self.notification_control = (self.notification_control + 1) % 4,
            Key::BackTab => self.notification_control = (self.notification_control + 3) % 4,
            Key::Up | Key::Down => {
                let n = self.notifications.len();
                if n != 0 {
                    let i = self.notification_index();
                    let next = if key == Key::Down {
                        (i + 1) % n
                    } else {
                        (i + n - 1) % n
                    };
                    self.notification_selected = self.notifications.at(next).map(|n| n.id);
                    self.notification_control = 0;
                }
            }
            Key::Delete => self.dismiss_selected_notification(),
            Key::Enter => match self.notification_control {
                0 => {
                    if let Some(id) = self.notification_selected {
                        self.notifications.mark_read(id);
                    }
                }
                1 => self.dismiss_selected_notification(),
                2 => {
                    self.notifications.clear_read();
                    self.select_remaining_notification();
                }
                _ => {
                    self.notifications
                        .set_do_not_disturb(!self.notifications.do_not_disturb());
                    if self.notifications.do_not_disturb() {
                        self.notification_toast = None;
                    }
                }
            },
            _ => {}
        }
    }
    fn select_remaining_notification(&mut self) {
        if self.notification_selected().is_none() {
            self.notification_selected = self.notifications.at(0).map(|n| n.id);
        }
    }
    fn dismiss_selected_notification(&mut self) {
        if let Some(id) = self.notification_selected {
            self.notifications.dismiss(id);
        }
        self.select_remaining_notification();
    }
    fn notification_pointer(&mut self, x: i32, y: i32) {
        let b = self.notification_bounds();
        let rows = self.notification_rows();
        if x >= b.x + 12
            && x < b.x + b.width - 12
            && y >= b.y + 72
            && y < b.y + 72 + rows as i32 * 64
        {
            let start = self
                .notification_index()
                .saturating_sub(rows.saturating_sub(1));
            let index = start + ((y - b.y - 72) / 64) as usize;
            if let Some(n) = self.notifications.at(index) {
                self.notification_selected = Some(n.id);
                self.notification_control = 0;
                self.notification_key(Key::Enter);
            }
        } else if y >= b.y + b.height - 48
            && y < b.y + b.height - 16
            && x >= b.x + 12
            && x < b.x + b.width - 12
        {
            let slot = (x - b.x - 12) / ((b.width - 24) / 3).max(1);
            if slot < 3 {
                self.notification_control = slot as usize + 1;
                self.notification_key(Key::Enter);
            }
        }
    }
    pub fn take_snap_event(&mut self) -> Option<SnapEvent> {
        self.snap_event.take()
    }
    pub fn snap_preview(&self) -> Option<(App, desktop::Bounds)> {
        self.desktop.snap_preview()
    }
    pub fn begin_snap(&mut self, zone: snap::Zone) -> bool {
        if self.notification_open
            || self.file_portal.is_some()
            || self.editor_deferred.is_some()
            || self.file_dialog_active()
            || self.files_directory.dialog_active()
        {
            return false;
        }
        if self.desktop.begin_snap(zone).is_err() {
            return false;
        }
        if let Some((app, bounds)) = self.desktop.snap_preview() {
            self.snap_event = Some(SnapEvent {
                kind: SnapEventKind::Preview,
                app,
                zone: Some(zone),
                bounds,
            });
        }
        true
    }
    pub fn finish_snap(&mut self, commit: bool) {
        if self.file_portal.is_some() || self.editor_deferred.is_some() {
            return;
        }
        if let Some((app, zone, bounds)) = self.desktop.finish_snap(commit) {
            self.snap_event = Some(SnapEvent {
                kind: if commit {
                    SnapEventKind::Applied
                } else {
                    SnapEventKind::Cancelled
                },
                app,
                zone: Some(zone),
                bounds,
            });
        }
    }
    pub fn taskbar_bounds(&self) -> Bounds {
        let (w, h) = self.desktop.dimensions();
        let width = (w - 24).clamp(0, 416);
        let height = h.clamp(0, 60);
        Bounds {
            x: (w - width) / 2,
            y: (h - height - 12).max(0),
            width,
            height,
        }
    }
    pub fn taskbar_item_bounds(&self, index: usize) -> Option<Bounds> {
        if index >= TASKBAR_ITEM_COUNT {
            return None;
        }
        let b = self.taskbar_bounds();
        let slot = ((b.width - 24).max(0) / TASKBAR_ITEM_COUNT as i32).max(1);
        Some(Bounds {
            x: b.x + 12 + index as i32 * slot,
            y: b.y + 6,
            width: (slot - 4).max(0),
            height: (b.height - 12).max(0),
        })
    }
    pub fn home_bounds(&self) -> Bounds {
        let (w, h) = self.desktop.dimensions();
        let width = (w - 24).clamp(0, 620);
        let height = (h - 96).clamp(0, 450);
        Bounds {
            x: (w - width) / 2,
            y: (self.taskbar_bounds().y - height - 12).max(0),
            width,
            height,
        }
    }
    pub fn home_action_bounds(&self, index: usize) -> Option<Bounds> {
        if index >= HOME_ACTION_COUNT {
            return None;
        }
        let b = self.home_bounds();
        let inner = (b.width - 56).max(0);
        if index < App::ALL.len() {
            let column = (inner / App::ALL.len() as i32).max(1);
            Some(Bounds {
                x: b.x + 28 + index as i32 * column,
                y: b.y + 126,
                width: (column - 8).max(0),
                height: 88,
            })
        } else {
            let item = index - App::ALL.len();
            let column = (inner / 2).max(1);
            let row_step = if b.height < 420 { 34 } else { 40 };
            Some(Bounds {
                x: b.x + 28 + (item % 2) as i32 * column,
                y: b.y + 244 + (item / 2) as i32 * row_step,
                width: (column - 12).max(0),
                height: 32,
            })
        }
    }
    pub fn calculator_bounds(&self) -> Bounds {
        let window = self.desktop.window(App::Calculator).bounds;
        Bounds {
            x: window.x.saturating_add(8),
            y: window.y.saturating_add(48),
            width: window.width.saturating_sub(16).max(0),
            height: window.height.saturating_sub(56).max(0),
        }
    }
    pub fn shortcuts_bounds(&self) -> Bounds {
        let (screen_width, screen_height) = self.desktop.dimensions();
        let width = screen_width.saturating_sub(32).clamp(1, 720);
        let height = screen_height.saturating_sub(32).clamp(1, 560);
        Bounds {
            x: (screen_width - width) / 2,
            y: (screen_height - height) / 2,
            width,
            height,
        }
    }
    pub fn shortcut_row_bounds(&self, index: usize) -> Option<Bounds> {
        if index >= shortcuts::ENTRIES.len() {
            return None;
        }
        let panel = self.shortcuts_bounds();
        Some(Bounds {
            x: panel.x + 16,
            y: panel.y + 72 + index as i32 * 32,
            width: (panel.width - 32).max(0),
            height: 30,
        })
    }
    fn shortcut_close_bounds(&self) -> Bounds {
        let panel = self.shortcuts_bounds();
        Bounds {
            x: panel.x + panel.width - 106,
            y: panel.y + panel.height - 46,
            width: 90,
            height: 32,
        }
    }
    fn shortcuts_pointer(&mut self, x: i32, y: i32) {
        if Self::hit(self.shortcut_close_bounds(), x, y) {
            self.shortcuts.close();
            return;
        }
        if let Some(index) = (0..shortcuts::ENTRIES.len()).find(|index| {
            self.shortcut_row_bounds(*index)
                .is_some_and(|b| Self::hit(b, x, y))
        }) {
            self.shortcuts.select(index);
        } else if !Self::hit(self.shortcuts_bounds(), x, y) {
            self.shortcuts.close();
        }
    }
    fn hit(bounds: Bounds, x: i32, y: i32) -> bool {
        bounds.width > 0
            && bounds.height > 0
            && x >= bounds.x
            && y >= bounds.y
            && x < bounds.x + bounds.width
            && y < bounds.y + bounds.height
    }
    /// Route one complete physical PS/2 packet. Client records carry the new
    /// three-button mask directly, never a stale Move mask before a transition.
    /// Trusted desktop controls continue to receive left-button gestures only.
    pub fn on_pointer_packet(&mut self, x: i32, y: i32, buttons: u8) {
        if buttons & !7 != 0 {
            self.client_input.revoke();
            return;
        }
        let previous = self.pointer_buttons;
        self.pointer_buttons = buttons;
        self.pointer = Some((x, y));
        if self.files_directory.dialog_active() {
            if buttons & 1 != 0 && previous & 1 == 0 {
                self.on_pointer(Pointer::Down(x, y));
            } else if buttons & 1 == 0 && previous & 1 != 0 {
                self.on_pointer(Pointer::Up);
            } else {
                self.on_pointer(Pointer::Move(x, y));
            }
            return;
        }
        if self.shortcuts.open() {
            if buttons & 1 != 0 && previous & 1 == 0 {
                self.on_pointer(Pointer::Down(x, y));
            } else if buttons & 1 == 0 && previous & 1 != 0 {
                self.on_pointer(Pointer::Up);
            } else {
                self.on_pointer(Pointer::Move(x, y));
            }
            return;
        }
        if self.client_input_allowed()
            && !self.native_windows.dragging()
            && self.native_resize.preview().is_none()
            && !(0..2).any(|slot| {
                self.native_window_restore_button_bounds(slot)
                    .is_some_and(|b| Self::hit(b, x, y))
            })
        {
            if let Some((surface, px, py)) = self.top_native_content_at(x, y) {
                self.desktop.cancel_drag();
                if buttons & 1 != 0 && previous & 1 == 0 {
                    self.native_windows
                        .raise(native_windows::WindowRef::Native(surface));
                }
                if buttons & 1 != 0
                    && previous & 1 == 0
                    && !self.client_input.accepts_pointer(surface)
                {
                    self.client_input
                        .gesture_packet(surface, px, py, u32::from(buttons));
                } else if self.client_input.context().is_some_and(|id| id != surface) {
                    self.client_input.leave();
                } else {
                    self.client_input
                        .pointer(surface, px, py, u32::from(buttons));
                }
                return;
            }
        }
        self.on_pointer(Pointer::Move(x, y));
        if buttons & 1 != 0 && previous & 1 == 0 {
            self.on_pointer(Pointer::Down(x, y));
        } else if buttons & 1 == 0 && previous & 1 != 0 {
            self.on_pointer(Pointer::Up);
        }
    }
    pub fn on_pointer(&mut self, event: Pointer) {
        if self.files_directory.dialog_active() {
            if let Pointer::Down(x, y) = event {
                self.files_directory_pointer(x, y);
            }
            return;
        }
        if self.file_query_dialog_active()
            && self.file_portal.is_none()
            && self.editor_deferred.is_none()
        {
            if let Pointer::Down(x, y) = event {
                self.file_query_pointer(x, y);
            }
            return;
        }

        match event {
            Pointer::Down(..) => self.pointer_buttons |= 1,
            Pointer::Up => self.pointer_buttons &= !1,
            Pointer::Move(..) => {}
        }
        if !self.client_input_allowed() {
            self.client_input.suspend();
        }
        if let Pointer::Down(x, y) | Pointer::Move(x, y) = event {
            self.pointer = Some((x, y));
        }
        if self.file_portal.is_some() {
            if let Pointer::Down(x, y) = event {
                self.portal_pointer(x, y);
            }
            return;
        }
        if self.shortcuts.open() {
            if let Pointer::Down(x, y) = event {
                self.shortcuts_pointer(x, y);
            }
            return;
        }
        if let Pointer::Down(x, y) = event {
            if self.editor_deferred.is_some() {
                let b = self.desktop.window(App::Editor).bounds;
                if y >= b.y + 96 && y < b.y + 124 && x >= b.x + 12 && x < b.x + 300 {
                    self.editor_choice = ((x - b.x - 12) / 96).min(2) as usize;
                    self.on_key(Key::Enter);
                }
                return;
            }
            if self.trash_dialog_active() {
                self.trash_dialog_pointer(x, y);
                return;
            }
            if self.file_dialog_active() {
                self.file_dialog_pointer(x, y);
                return;
            }
            if self.notification_open {
                if self
                    .taskbar_item_bounds(TASKBAR_NOTIFICATIONS_INDEX)
                    .is_some_and(|b| Self::hit(b, x, y))
                {
                    self.toggle_notifications();
                } else {
                    self.notification_pointer(x, y);
                }
                return;
            }
            for index in 0..TASKBAR_ITEM_COUNT {
                if self
                    .taskbar_item_bounds(index)
                    .is_some_and(|b| Self::hit(b, x, y))
                {
                    self.client_input.suspend();
                    match index {
                        0 => {
                            self.launcher = !self.launcher;
                            self.search_mode = false;
                            self.search_input = Line::new();
                            self.selection = 0;
                        }
                        TASKBAR_NOTIFICATIONS_INDEX => self.toggle_notifications(),
                        _ => self.apply_window_action(Action::Open(App::ALL[index - 1])),
                    }
                    return;
                }
            }
            if self.launcher {
                let b = self.home_bounds();
                if x >= b.x + 28 && x < b.x + b.width - 28 && y >= b.y + 54 && y < b.y + 96 {
                    self.search_mode = true;
                    self.search_input = Line::new();
                    self.selection = 0;
                    return;
                }
                if self.search_mode {
                    if x >= b.x + 28 && x < b.x + b.width - 28 && y >= b.y + 116 && y < b.y + 404 {
                        let row = ((y - b.y - 116) / 32) as usize;
                        let index = self.selection.saturating_sub(8) + row;
                        if self.search_result(index).is_some() {
                            self.selection = index;
                            self.activate_search();
                        }
                    } else {
                        self.launcher = false;
                        self.search_mode = false;
                    }
                } else if let Some(index) = (0..HOME_ACTION_COUNT).find(|i| {
                    self.home_action_bounds(*i)
                        .is_some_and(|item| Self::hit(item, x, y))
                }) {
                    self.selection = index;
                    if index == desktop::ACTIONS.len() {
                        self.shortcuts.show();
                    } else {
                        self.apply_window_action(desktop::ACTIONS[index]);
                        self.launcher = false;
                    }
                } else {
                    self.launcher = false;
                }
                return;
            }
        }
        if self.notification_open || self.file_dialog_active() || self.trash_dialog_active() {
            return;
        }
        if self.native_window_pointer(event) {
            return;
        }
        let target = match event {
            Pointer::Down(x, y) | Pointer::Move(x, y) => Some((x, y)),
            Pointer::Up => self.pointer,
        };
        if let Some((x, y)) = target {
            if let Some(native_windows::WindowRef::Builtin(app)) = self.top_window_at(x, y) {
                if matches!(event, Pointer::Down(..)) {
                    self.open_app(app);
                    self.native_windows
                        .raise(native_windows::WindowRef::Builtin(app));
                }
            }
            if let Some((surface, px, py)) = self.top_native_content_at(x, y) {
                if !matches!(event, Pointer::Down(..))
                    && self.client_input.context().is_some_and(|id| id != surface)
                {
                    self.client_input.leave();
                    return;
                }
                match event {
                    Pointer::Down(..) => {
                        self.desktop.cancel_drag();
                        self.native_windows
                            .raise(native_windows::WindowRef::Native(surface));
                        self.client_input.gesture(surface, px, py)
                    }
                    Pointer::Move(..) => {
                        self.client_input
                            .pointer(surface, px, py, self.client_input.buttons())
                    }
                    Pointer::Up => self.client_input.pointer(surface, px, py, 0),
                }
                return;
            }
            if matches!(
                self.top_window_at(x, y),
                Some(native_windows::WindowRef::Native(_))
            ) {
                self.desktop.cancel_drag();
                self.client_input.leave();
                return;
            }
            if matches!(event, Pointer::Down(..)) {
                self.client_input.suspend();
            } else {
                self.client_input.leave();
            }
        }
        if let Pointer::Down(x, y) = event {
            let e = self.desktop.window(App::Editor);
            if self.desktop.focused() == Some(App::Editor)
                && self.document.dirty()
                && x >= e.bounds.x + e.bounds.width - 32
                && x < e.bounds.x + e.bounds.width
                && y >= e.bounds.y
                && y < e.bounds.y + 40
            {
                self.apply_window_action(Action::Close);
                return;
            }
        }
        let before = self.desktop.snap_preview();
        self.desktop.pointer(event);
        if let Some((app, bounds)) = self.desktop.snap_preview() {
            if before != Some((app, bounds)) {
                let zone = self.desktop.preview_zone();
                self.snap_event = Some(SnapEvent {
                    kind: SnapEventKind::Preview,
                    app,
                    zone,
                    bounds,
                });
            }
        } else if let Some((app, _)) = before {
            if event == Pointer::Up {
                self.snap_event = Some(SnapEvent {
                    kind: SnapEventKind::Applied,
                    app,
                    zone: self.desktop.snapped_zone(app),
                    bounds: self.desktop.window(app).bounds,
                });
            }
        }
        if let Pointer::Down(x, y) = event {
            if self.desktop.focused() == Some(App::Calculator)
                && self
                    .calculator
                    .on_pointer_embedded(x, y, self.calculator_bounds())
                    != calculator_app::Change::Ignored
            {
                return;
            }
            if self.desktop.focused() == Some(App::Settings) && self.settings_pointer(x, y) {
                return;
            }
            if self.desktop.focused() == Some(App::Files) {
                let b = self.desktop.window(App::Files).bounds;
                if self.files_directory.listing().is_some() {
                    self.files_directory_pointer(x, y);
                    return;
                }
                if self.trash_page {
                    self.trash_page_pointer(x, y);
                    return;
                }
                self.sync_files_focus();
                for index in 0..6 {
                    if self
                        .file_action_button_bounds(index)
                        .is_some_and(|button| Self::hit(button, x, y))
                    {
                        let _ = self.file_focus.select(
                            keyboard_focus::ControlId {
                                scope: 2,
                                local: index as u32 + 2,
                            },
                            keyboard_focus::Input::Pointer,
                        );
                        self.begin_file_operation(index);
                        return;
                    }
                }
                if x >= b.x + 8
                    && x < b.x + b.width - 8
                    && y >= b.y + 106
                    && y < b.y + 106 + FILE_CAPACITY as i32 * 24
                    && y < b.y + b.height - 8
                {
                    let row = ((y - b.y - 106) / 24) as usize;
                    let index = self.file_view_index(self.file_list_start() + row);
                    if let Some(index) = index.filter(|_| row < self.file_list_rows()) {
                        let _ = self.file_focus.select(
                            keyboard_focus::ControlId { scope: 2, local: 1 },
                            keyboard_focus::Input::Pointer,
                        );
                        let open = self.file_selection == index;
                        self.file_selection = index;
                        if open {
                            let file = self.files[index];
                            if let Err(error) = self.request_editor_open(file.path.as_str()) {
                                self.editor_status.set(error.message());
                            }
                        }
                    }
                }
            }
        }
    }
    fn files_directory_pointer(&mut self, x: i32, y: i32) {
        if self.files_directory.dialog_active() {
            if Self::hit(self.files_directory_dialog_button_bounds(0), x, y) {
                self.files_directory.cancel();
                self.sync_files_directory_dialog_focus();
            } else if Self::hit(self.files_directory_dialog_button_bounds(1), x, y) {
                let _ = self.files_directory.choose_approve(true);
                let _ = self.files_directory.confirm();
                self.sync_files_directory_dialog_focus();
            }
            return;
        }
        if Self::hit(self.file_action_button_bounds(0).unwrap(), x, y)
            || Self::hit(self.file_action_button_bounds(1).unwrap(), x, y)
            || Self::hit(self.file_action_button_bounds(2).unwrap(), x, y)
            || Self::hit(self.file_action_button_bounds(3).unwrap(), x, y)
            || Self::hit(self.file_action_button_bounds(4).unwrap(), x, y)
            || Self::hit(self.file_action_button_bounds(5).unwrap(), x, y)
        {
            let index = (0..6)
                .find(|i| {
                    self.file_action_button_bounds(*i)
                        .is_some_and(|b| Self::hit(b, x, y))
                })
                .unwrap_or(0);
            let _ = self.file_focus.select(
                keyboard_focus::ControlId {
                    scope: 2,
                    local: index as u32 + 2,
                },
                keyboard_focus::Input::Pointer,
            );
            self.begin_files_directory_action(index);
            return;
        }
        let b = self.desktop.window(App::Files).bounds;
        if x >= b.x + 8 && x < b.x + b.width - 8 && y >= b.y + 98 && y < b.y + 128 {
            let _ = self.files_directory.set_address("");
            self.files_directory_address_editing = true;
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 2, local: 8 },
                keyboard_focus::Input::Pointer,
            );
            return;
        }
        if x >= b.x + 8 && x < b.x + b.width - 8 && y >= b.y + 136 && y < b.y + b.height - 8 {
            let row = ((y - b.y - 136) / 27) as usize;
            let max_rows =
                ((b.height - 166).max(0) / 27).min(files_directory::CAPACITY as i32) as usize;
            if row >= max_rows {
                return;
            }
            let open = self.files_directory.selected_index() == Some(row);
            if !self.files_directory.select(row) {
                return;
            }
            self.sync_selected_directory_file();
            self.sync_files_focus();
            // A second click on the selected row activates it, matching the
            // existing Files row behavior while keeping selection authoritative.
            let _ = self.file_focus.select(
                keyboard_focus::ControlId { scope: 2, local: 1 },
                keyboard_focus::Input::Pointer,
            );
            if open {
                if let Ok(Some(path)) = self.files_directory.open_selected() {
                    if let Err(error) = self.request_editor_open(path.as_str()) {
                        self.editor_status.set(error.message());
                    }
                }
            }
        }
    }
    pub fn on_terminal_key(&mut self, key: Key) {
        if self.file_portal.is_some()
            || self.editor_deferred.is_some()
            || self.notification_open
            || self.file_dialog_active()
            || self.files_directory.dialog_active()
            || self.file_query_dialog_active()
            || self.trash_dialog_active()
            || self.snap_preview().is_some()
        {
            self.on_key(key);
            return;
        }
        self.client_leave_context();
        self.desktop.open(App::Terminal);
        self.native_windows
            .raise(native_windows::WindowRef::Builtin(App::Terminal));
        self.launcher = false;
        self.terminal_key(key);
    }
    pub fn set_status(&mut self, status: &str) {
        self.status.set(status);
    }
    pub fn set_uptime_ns(&mut self, nanoseconds: u64) {
        self.uptime_seconds = Some(nanoseconds / 1_000_000_000);
        if self
            .notification_toast
            .is_some_and(|(_, deadline)| nanoseconds >= deadline)
        {
            self.notification_toast = None;
        }
    }
    pub fn set_boot_file(&mut self, text: &str) {
        self.boot_file.set(text.trim());
        self.file_available = true;
        self.set_file(0, "/NEXT.TXT", text.trim());
    }
    /// Clear terminal state without injecting keystrokes or changing the
    /// currently focused panel. Used by the terminal's Ctrl+L binding.
    pub fn clear_terminal(&mut self) {
        self.input = Line::new();
        self.history = [Line::new(); HISTORY_CAPACITY];
        self.count = 0;
        self.response = None;
        self.response_count = 0;
    }
    pub fn input(&self) -> &str {
        self.input.as_str()
    }
    pub fn launcher_open(&self) -> bool {
        self.launcher
    }
    pub fn history_len(&self) -> usize {
        self.count
    }
    pub fn history_line(&self, index: usize) -> Option<&str> {
        if index < self.count {
            Some(self.history[index].as_str())
        } else {
            None
        }
    }
    fn push(&mut self, text: &str) {
        if self.count == HISTORY_CAPACITY {
            self.history.copy_within(1..HISTORY_CAPACITY, 0);
            self.count -= 1;
        }
        self.history[self.count].set(text);
        self.count += 1;
    }
    pub fn on_key(&mut self, key: Key) {
        if self.native_resize.preview().is_some() {
            if !self.client_input_allowed() {
                self.native_resize.cancel();
            } else {
                match key {
                    Key::Escape => self.native_resize.cancel(),
                    Key::Enter => {
                        self.native_resize.retry();
                    }
                    _ => {}
                }
                return;
            }
        }

        if !self.client_input_allowed() {
            self.client_input.suspend();
            if self.file_portal.is_some()
                || self.editor_deferred.is_some()
                || self.file_dialog_active()
                || self.trash_dialog_active()
                || self.notification_open
            {
                self.desktop.finish_switch();
            }
        }
        if !matches!(key, Key::WindowNext | Key::WindowPrevious) {
            self.native_windows.finish_switch();
            self.desktop.finish_switch();
        }
        if key == Key::WindowSwitchEnd {
            self.native_windows.finish_switch();
            return;
        }

        if self.file_portal.is_some() {
            self.portal_key(key);
            return;
        }
        if let Some(action) = self.editor_deferred {
            match key {
                Key::Escape => self.editor_deferred = None,
                Key::Tab | Key::Right => self.editor_choice = (self.editor_choice + 1) % 3,
                Key::Left | Key::BackTab => self.editor_choice = (self.editor_choice + 2) % 3,
                Key::Enter => match self.editor_choice {
                    0 => self.editor_deferred = None,
                    1 => {
                        self.document.discard_changes();
                        self.editor_loaded = false;
                        self.editor_deferred = None;
                        self.perform_editor_deferred(action);
                    }
                    _ => self.request_editor_save(),
                },
                _ => {}
            }
            return;
        }

        if self.files_directory.dialog_active() {
            self.files_directory_dialog_key(key);
            return;
        }
        if self.file_query_dialog_active() {
            self.file_query_key(key);
            return;
        }
        if self.trash_dialog_active() {
            self.trash_dialog_key(key);
            return;
        }
        if self.file_dialog_active() {
            self.file_dialog_key(key);
            return;
        }
        if self.shortcuts.open() {
            self.shortcuts.on_key(key);
            return;
        }
        if self.client_input_allowed() && matches!(key, Key::WindowNext | Key::WindowPrevious) {
            self.switch_ordinary_window(key == Key::WindowPrevious);
            return;
        }
        if self.client_input_allowed() {
            if matches!(
                key,
                Key::Menu
                    | Key::WindowNext
                    | Key::WindowPrevious
                    | Key::WindowSwitchEnd
                    | Key::OpenSearch
                    | Key::OpenFiles
                    | Key::OpenSettings
                    | Key::OpenTerminal
                    | Key::CloseWindow
                    | Key::SnapLeft
                    | Key::SnapRight
                    | Key::SnapMaximize
                    | Key::SnapRestore
                    | Key::Notifications
            ) {
                if key == Key::CloseWindow {
                    if let Some(surface) = self.client_input.context() {
                        self.request_client_close(surface);
                        return;
                    }
                }
                self.client_input.suspend();
            } else if self.client_input.key(key) {
                return;
            }
        }
        if key == Key::Notifications {
            self.toggle_notifications();
            return;
        }
        if self.notification_open {
            self.notification_key(key);
            return;
        }
        if matches!(key, Key::WindowNext | Key::WindowPrevious) {
            self.switch_ordinary_window(key == Key::WindowPrevious);
            return;
        }
        match key {
            Key::OpenSearch => {
                self.launcher = true;
                self.search_mode = true;
                self.search_input = Line::new();
                self.selection = 0;
                return;
            }
            Key::OpenSettings | Key::OpenFiles | Key::OpenTerminal => {
                self.launcher = false;
                self.search_mode = false;
                self.apply_window_action(Action::Open(match key {
                    Key::OpenSettings => App::Settings,
                    Key::OpenFiles => App::Files,
                    _ => App::Terminal,
                }));
                return;
            }
            Key::CloseWindow => {
                self.launcher = false;
                self.apply_window_action(Action::Close);
                return;
            }
            _ => {}
        }
        match key {
            Key::SnapLeft => {
                self.begin_snap(snap::Zone::Left);
                return;
            }
            Key::SnapRight => {
                self.begin_snap(snap::Zone::Right);
                return;
            }
            Key::SnapMaximize => {
                self.begin_snap(snap::Zone::Maximize);
                return;
            }
            Key::SnapRestore => {
                self.finish_snap(false);
                self.desktop.action(Action::Restore);
                if let Some(app) = self.desktop.focused() {
                    self.snap_event = Some(SnapEvent {
                        kind: SnapEventKind::Restored,
                        app,
                        zone: None,
                        bounds: self.desktop.window(app).bounds,
                    });
                }
                return;
            }
            _ => {}
        }
        if self.desktop.snap_preview().is_some() {
            match key {
                Key::Enter => self.finish_snap(true),
                Key::Escape => self.finish_snap(false),
                Key::Left => {
                    self.begin_snap(snap::Zone::Left);
                }
                Key::Right => {
                    self.begin_snap(snap::Zone::Right);
                }
                Key::Up => {
                    self.begin_snap(snap::Zone::Maximize);
                }
                _ => {}
            }
            return;
        }
        if key == Key::Escape && self.desktop.dragging() {
            self.desktop.cancel_drag();
            return;
        }
        if self.desktop.editing().is_some() {
            match key {
                Key::Escape => self.desktop.finish_edit(false),
                Key::Enter => self.desktop.finish_edit(true),
                Key::Left | Key::Character('h') => self.desktop.edit_delta(-16, 0),
                Key::Right | Key::Character('l') => self.desktop.edit_delta(16, 0),
                Key::Up | Key::Character('k') => self.desktop.edit_delta(0, -16),
                Key::Down | Key::Character('j') => self.desktop.edit_delta(0, 16),
                _ => {}
            }
            return;
        }
        if key == Key::Escape
            && !self.launcher
            && self.desktop.focused() == Some(App::Settings)
            && self.settings_diagnostics
        {
            self.settings_diagnostics = false;
            return;
        }
        if key == Key::Escape
            && !self.launcher
            && self.file_detail
            && self.desktop.focused() == Some(App::Files)
        {
            self.file_detail = false;
            return;
        }
        if self.desktop.focused() == Some(App::Calculator)
            && matches!(
                key,
                Key::Tab
                    | Key::BackTab
                    | Key::Up
                    | Key::Down
                    | Key::Enter
                    | Key::Escape
                    | Key::Delete
                    | Key::Backspace
                    | Key::Character(_)
            )
        {
            self.calculator.on_key(key);
            return;
        }
        if self.launcher && self.search_mode && key != Key::Menu {
            self.search_key(key);
            return;
        }
        if key == Key::Escape || key == Key::Menu {
            self.launcher = !self.launcher;
            self.search_mode = false;
            self.search_input = Line::new();
            self.selection = 0;
            return;
        }
        if self.launcher {
            if matches!(key, Key::Character(c) if c.is_ascii() && !c.is_control()) {
                self.search_mode = true;
                self.search_input = Line::new();
                self.selection = 0;
                self.search_key(key);
                return;
            }
            match key {
                Key::Tab | Key::Down => self.selection = (self.selection + 1) % HOME_ACTION_COUNT,
                Key::Up | Key::BackTab => {
                    self.selection = (self.selection + HOME_ACTION_COUNT - 1) % HOME_ACTION_COUNT
                }
                Key::Enter => {
                    if self.selection == desktop::ACTIONS.len() {
                        self.shortcuts.show();
                    } else {
                        self.apply_window_action(desktop::ACTIONS[self.selection]);
                        self.launcher = false;
                    }
                }
                _ => {}
            }
            return;
        }
        if self.desktop.focused() == Some(App::Files) && matches!(key, Key::Tab | Key::BackTab) {
            if self.trash_page {
                self.trash_page_key(key);
                return;
            }
            self.sync_files_focus();
            self.file_focus.traverse(if key == Key::Tab {
                keyboard_focus::Direction::Forward
            } else {
                keyboard_focus::Direction::Reverse
            });
            return;
        }
        if self.desktop.focused() == Some(App::Settings) && matches!(key, Key::Tab | Key::BackTab) {
            self.settings_key(key);
            return;
        }
        if key == Key::BackTab {
            self.desktop.cycle_reverse();
            if let Some(app) = self.desktop.focused() {
                self.native_windows
                    .raise(native_windows::WindowRef::Builtin(app));
            }
            return;
        }
        if key == Key::Tab {
            self.desktop.cycle();
            if let Some(app) = self.desktop.focused() {
                self.native_windows
                    .raise(native_windows::WindowRef::Builtin(app));
            }
            return;
        }
        match self.desktop.focused() {
            Some(App::Terminal) => self.terminal_key(key),
            Some(App::Editor) => self.editor_key(key),
            Some(App::Settings) => self.settings_key(key),
            Some(App::Calculator) => {
                self.calculator.on_key(key);
            }
            Some(App::Files) => {
                if self.files_directory.listing().is_some() {
                    self.files_directory_key(key);
                    return;
                }
                if self.trash_page {
                    self.trash_page_key(key);
                    return;
                }
                self.sync_files_focus();
                match key {
                    Key::Down | Key::Character('j') => self.select_file(1),
                    Key::Up | Key::Character('k') => self.select_file(FILE_CAPACITY - 1),
                    Key::Character('/') => self.show_file_query(),
                    Key::Character('u') => self.request_trash_undo(),
                    Key::Character('n') => self.begin_file_operation(0),
                    Key::Character('r') => self.begin_file_operation(1),
                    Key::Character('c') => self.begin_file_operation(3),
                    Key::Character('t') => self.show_trash(),
                    Key::Delete => self.begin_file_operation(2),
                    Key::Enter if self.file_focus.focused().is_some_and(|id| id.local > 1) => {
                        let local = self.file_focus.focused().unwrap().local;
                        self.begin_file_operation(local as usize - 2);
                    }
                    Key::Enter => {
                        let file = self.files[self.file_selection];
                        if file.available {
                            if let Err(error) = self.request_editor_open(file.path.as_str()) {
                                self.editor_status.set(error.message());
                            }
                        }
                    }
                    Key::Backspace => self.file_detail = false,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    fn select_file(&mut self, offset: usize) {
        let count = self.file_view_len();
        if count == 0 {
            return;
        }
        let current = (0..count)
            .position(|row| self.file_view_index(row) == Some(self.file_selection))
            .unwrap_or(0);
        if let Some(index) =
            self.file_view_index((current + if offset == 1 { 1 } else { count - 1 }) % count)
        {
            self.file_selection = index;
            self.file_detail = false;
        }
    }
    fn terminal_key(&mut self, key: Key) {
        match key {
            Key::Character(c) if c.is_ascii() && !c.is_ascii_control() => {
                if self.input.len < LINE_CAPACITY {
                    self.input.bytes[self.input.len] = c as u8;
                    self.input.len += 1;
                }
            }
            Key::Backspace => self.input.len = self.input.len.saturating_sub(1),
            Key::Enter => self.execute(),
            _ => {}
        }
    }
    fn execute(&mut self) {
        let command = self.input;
        self.input.len = 0;
        let command = command.as_str().trim();
        if command.is_empty() {
            return;
        }
        self.push(command);
        if let Some(response) = self.response.take() {
            for line in response.iter().take(self.response_count) {
                self.push(line.as_str());
            }
            self.response_count = 0;
            return;
        }
        match command {
            "help" => {
                self.push("help   - list console commands");
                self.push("about  - current implementation scope");
                self.push("status - hardware startup status");
                self.push("uptime - monotonic system uptime");
                self.push("calc EXPR - checked integer calculator");
                self.push("edit PATH - open an actual file in Editor");
                self.push("clear  - clear console output");
                self.push("cat /NEXT.TXT - read boot volume note");
                self.push("Escape: Home; Tab: choose; Enter: open/action");
                self.push("Tab: switch windows; move/resize: hjkl + Enter");
            }
            "about" => {
                self.push("Next / Helix / Prism - early development build");
                self.push("Standalone Rust boot and framebuffer console.");
                self.push("Framebuffer desktop diagnostics; no user compositor.");
                self.push("This is not the finished Next 1.0 system.");
            }
            "status" => {
                let status = self.status;
                self.push(status.as_str());
            }
            "uptime" => {
                if let Some(seconds) = self.uptime_seconds {
                    use core::fmt::Write;
                    let mut line = Line::new();
                    let _ = write!(line, "Uptime: {} seconds", seconds);
                    self.push(line.as_str());
                } else {
                    self.push("Monotonic clock unavailable.");
                }
            }
            "cat /NEXT.TXT" => {
                if self.file_available {
                    let note = self.boot_file;
                    self.push(note.as_str());
                } else {
                    self.push("Boot volume file unavailable.");
                }
            }
            "edit" => self.push("Usage: edit /native/filename"),
            command if command.starts_with("edit ") => {
                match self.request_editor_open(command[5..].trim()) {
                    Ok(()) => self.push("Editor open requested."),
                    Err(error) => self.push(error.message()),
                }
            }
            "calc" => self.push(calculator::Error::Empty.message()),
            command if command.starts_with("calc ") => match calculator::evaluate(&command[5..]) {
                Ok(value) => {
                    use core::fmt::Write;
                    let mut output = Line::new();
                    let _ = write!(output, "= {}", value);
                    self.push(output.as_str());
                }
                Err(error) => self.push(error.message()),
            },
            "clear" => self.clear_terminal(),
            _ => self.push("Unknown command. Type help for available commands."),
        }
    }
    pub fn render(&self, canvas: &mut Canvas<'_>) {
        canvas.set_palette(palette::Palette::new(self.active_appearance()));
        let w = canvas.width() as i32;
        let h = canvas.height() as i32;
        theme::wallpaper(canvas);
        theme::icon(canvas, theme::Icon::Next, 24, 22, 26, 0xffffff);
        theme::text(canvas, 62, 20, "Next", 0xffffff, theme::Font::Title);
        theme::text(
            canvas,
            62,
            45,
            "Prism development desktop",
            0xe0ebff,
            theme::Font::Small,
        );
        for entry in self.native_windows.order().into_iter().flatten() {
            let app = match entry {
                native_windows::WindowRef::Builtin(app) => app,
                native_windows::WindowRef::Native(id) => {
                    self.client_previews.render_window(
                        canvas,
                        id,
                        self.client_focused(),
                        self.client_input.context(),
                    );
                    continue;
                }
            };
            let window = self.desktop.window(app);
            if !matches!(window.state, WindowState::Floating | WindowState::Maximized) {
                continue;
            }
            // Clip windows to the current output even if a caller has not yet
            // supplied a viewport update. Root compositor owns mode changes.
            let b = window.bounds;
            let width = b.width.min((w - b.x).max(0));
            let height = b.height.min((h - 80 - b.y).max(0));
            if width <= 0 || height <= 0 {
                continue;
            }
            let focused = self.desktop.focused() == Some(app);
            let surface = Bounds { width, height, ..b };
            theme::shadow(canvas, surface, 10);
            theme::rounded_rect(canvas, surface, 10, theme::SURFACE);
            theme::outline(
                canvas,
                surface,
                10,
                if focused { theme::BORDER } else { 0xdbe1eb },
            );
            let columns = ((width - 24).max(0) / 12) as usize;
            theme::icon(canvas, app_icon(app), b.x + 14, b.y + 11, 18, theme::ACCENT);
            theme::text(
                canvas,
                b.x + 42,
                b.y + 10,
                app.title(),
                theme::TEXT,
                theme::Font::Body,
            );
            canvas.rect(b.x + 1, b.y + 40, width - 2, 1, theme::BORDER);
            if width >= 128 {
                for (i, icon) in [
                    theme::Icon::Minimize,
                    theme::Icon::Maximize,
                    theme::Icon::Close,
                ]
                .into_iter()
                .enumerate()
                {
                    let cx = b.x + width - 96 + i as i32 * 32;
                    theme::icon(canvas, icon, cx + 8, b.y + 13, 14, theme::MUTED);
                }
            }
            if height < 64 {
                continue;
            }
            match app {
                App::Terminal => self.render_terminal(canvas, b.x, b.y, width, height, columns),
                App::Files => self.render_files(canvas, b.x, b.y, width, height, columns),
                App::Monitor => {
                    self.panel_line(
                        canvas,
                        b.x,
                        b.y,
                        0,
                        "Actual kernel telemetry",
                        columns,
                        height,
                        theme::ACCENT,
                    );
                    let mut uptime = Line::new();
                    use core::fmt::Write;
                    if let Some(seconds) = self.uptime_seconds {
                        let _ = write!(uptime, "Monotonic uptime: {} seconds", seconds);
                    } else {
                        uptime.set("Monotonic clock unavailable.");
                    }
                    self.panel_line(
                        canvas,
                        b.x,
                        b.y,
                        1,
                        uptime.as_str(),
                        columns,
                        height,
                        theme::TEXT,
                    );
                    self.render_lines(canvas, b.x, b.y, &self.monitor, 2, columns, height);
                    if self.monitor.iter().all(|line| line.len == 0) {
                        self.panel_line(
                            canvas,
                            b.x,
                            b.y,
                            3,
                            "Process/memory counters not supplied.",
                            columns,
                            height,
                            theme::MUTED,
                        );
                    }
                }
                App::Editor => self.render_editor(canvas, b.x, b.y, width, height, columns),
                App::Calculator => self
                    .calculator
                    .render_embedded(canvas, self.calculator_bounds()),
                App::Settings => {
                    if !self.settings_diagnostics {
                        self.render_appearance(canvas, b.x, b.y, width, height, columns);
                        continue;
                    }
                    self.panel_line(
                        canvas,
                        b.x,
                        b.y,
                        0,
                        "System diagnostics / read-only",
                        columns,
                        height,
                        theme::ACCENT,
                    );
                    self.panel_line(
                        canvas,
                        b.x,
                        b.y,
                        1,
                        self.status.as_str(),
                        columns,
                        height,
                        theme::TEXT,
                    );
                    self.render_lines(canvas, b.x, b.y, &self.system, 2, columns, height);
                    self.panel_line(
                        canvas,
                        b.x,
                        b.y,
                        11,
                        "Configuration persistence not available.",
                        columns,
                        height,
                        theme::MUTED,
                    );
                }
            }
            if self.desktop.editing().is_some() && focused {
                self.panel_line(
                    canvas,
                    b.x,
                    b.y,
                    0,
                    "Arrows/hjkl: adjust; Enter: apply; Esc: cancel",
                    columns,
                    height,
                    theme::ACCENT,
                );
            }
        }

        if let Some((_, b)) = self.desktop.snap_preview() {
            canvas.rect(b.x, b.y, b.width, 4, 0x7c9cff);
            canvas.rect(b.x, b.y + b.height - 4, b.width, 4, 0x7c9cff);
            canvas.rect(b.x, b.y, 4, b.height, 0x7c9cff);
            canvas.rect(b.x + b.width - 4, b.y, 4, b.height, 0x7c9cff);
            draw_limited(
                canvas,
                b.x + 12,
                b.y + 12,
                "SNAP: Enter apply / Esc cancel",
                ((b.width - 24).max(0) / (6 * ui_scale(canvas))) as usize,
                theme::TEXT,
            );
        }
        self.render_taskbar(canvas);
        self.render_native_resize(canvas);
        self.render_native_window_items(canvas);
        if self.launcher {
            self.render_home(canvas);
        }
        if self.file_query_dialog_active() {
            self.render_file_query(canvas);
        }
        self.render_editor_confirmation(canvas);
        if self.file_portal.is_none()
            && self.editor_deferred.is_none()
            && !self.file_dialog_active()
            && !self.trash_dialog_active()
            && self.snap_preview().is_none()
        {
            if self.notification_open {
                self.render_notifications(canvas);
            } else if let Some((id, _)) = self.notification_toast {
                self.render_notification_toast(canvas, id);
            }
        }
        if self.file_dialog_active() {
            self.render_file_dialog(canvas);
        }
        if self.trash_dialog_active() {
            self.render_trash_dialog(canvas);
        }
        if self.file_portal.is_some() {
            self.render_file_portal(canvas);
        }
        if self.shortcuts.open() {
            self.render_shortcuts(canvas);
        }
        if let Some((x, y)) = self.pointer {
            // A software cursor is drawn last, from actual pointer events.
            cursor::draw(canvas, x, y);
        }
    }
    fn render_taskbar(&self, canvas: &mut Canvas<'_>) {
        let b = self.taskbar_bounds();
        if b.width <= 0 || b.height <= 0 {
            return;
        }

        // A layered, cool-white shell gives the dock a mica-like lift while
        // staying legible over both light and dark parts of the wallpaper.
        theme::shadow(canvas, b, 12);
        theme::rounded_rect(canvas, b, 15, theme::BORDER);
        let inset = Bounds {
            x: b.x.saturating_add(1),
            y: b.y.saturating_add(1),
            width: b.width.saturating_sub(2),
            height: b.height.saturating_sub(2),
        };
        theme::rounded_rect(canvas, inset, 14, theme::SURFACE);
        if b.height >= 16 && b.width >= 4 {
            // A narrow highlight catches the top edge without adding bulk.
            theme::rounded_rect(
                canvas,
                Bounds {
                    x: b.x.saturating_add(12),
                    y: b.y.saturating_add(2),
                    width: b.width.saturating_sub(24).max(0),
                    height: 1,
                },
                1,
                theme::SURFACE_RAISED,
            );
        }

        for index in 0..TASKBAR_ITEM_COUNT {
            let Some(item) = self.taskbar_item_bounds(index) else {
                continue;
            };
            if item.width <= 0 || item.height <= 0 {
                continue;
            }
            let active = match index {
                0 => self.launcher,
                TASKBAR_NOTIFICATIONS_INDEX => self.notification_open,
                _ => self.desktop.focused() == Some(App::ALL[index - 1]),
            };

            // The selected tile sits comfortably inside its hit target; the
            // hit geometry itself remains unchanged.
            if active {
                let tile = Bounds {
                    x: item.x.saturating_add(4),
                    y: item.y.saturating_add(3),
                    width: item.width.saturating_sub(8).max(0),
                    height: item.height.saturating_sub(8).max(0),
                };
                theme::rounded_rect(canvas, tile, 10, theme::ACCENT_SOFT);
            }
            let icon = match index {
                0 => theme::Icon::Next,
                TASKBAR_NOTIFICATIONS_INDEX => theme::Icon::Notices,
                _ => app_icon(App::ALL[index - 1]),
            };
            let size = item.width.min(item.height.saturating_sub(8)).clamp(0, 24);
            theme::icon(
                canvas,
                icon,
                item.x + (item.width - size) / 2,
                item.y + (item.height - size) / 2 - 1,
                size,
                if active { theme::ACCENT } else { theme::TEXT },
            );

            if index > 0
                && index < TASKBAR_NOTIFICATIONS_INDEX
                && self.desktop.window(App::ALL[index - 1]).state != WindowState::Closed
            {
                let indicator_width =
                    if active { 16 } else { 8 }.min(item.width.saturating_sub(12));
                let indicator_height = if active { 3 } else { 2 }.min(item.height);
                theme::rounded_rect(
                    canvas,
                    Bounds {
                        x: item.x + (item.width - indicator_width) / 2,
                        y: item.y + item.height - indicator_height - 1,
                        width: indicator_width,
                        height: indicator_height,
                    },
                    2,
                    if active { theme::ACCENT } else { theme::MUTED },
                );
            }
            if index == TASKBAR_NOTIFICATIONS_INDEX && self.notifications.unread() > 0 {
                theme::rounded_rect(
                    canvas,
                    Bounds {
                        x: item.x + item.width - 12,
                        y: item.y + 5,
                        width: 7,
                        height: 7,
                    },
                    3,
                    theme::ACCENT,
                );
            }
        }
    }
    fn render_home(&self, canvas: &mut Canvas<'_>) {
        let b = self.home_bounds();
        theme::shadow(canvas, b, 14);
        theme::rounded_rect(canvas, b, 14, theme::SURFACE);
        theme::outline(canvas, b, 14, theme::BORDER);
        // A slim accent at the top gives this transient launcher a clear
        // identity without adding a second title bar.
        canvas.rect(b.x + 18, b.y + 1, (b.width - 36).max(0), 2, theme::ACCENT);
        theme::icon(
            canvas,
            theme::Icon::Next,
            b.x + 26,
            b.y + 17,
            20,
            theme::ACCENT,
        );
        theme::text(
            canvas,
            b.x + 56,
            b.y + 18,
            "Your workspace",
            theme::TEXT,
            theme::Font::Title,
        );
        let field = Bounds {
            x: b.x + 28,
            y: b.y + 54,
            width: (b.width - 56).max(0),
            height: 40,
        };
        theme::rounded_rect(canvas, field, 8, theme::SURFACE_RAISED);
        theme::outline(
            canvas,
            field,
            8,
            if self.search_mode {
                theme::ACCENT
            } else {
                theme::BORDER
            },
        );
        theme::icon(
            canvas,
            theme::Icon::Search,
            field.x + 12,
            field.y + 12,
            18,
            theme::MUTED,
        );
        theme::text(
            canvas,
            field.x + 40,
            field.y + 10,
            if self.search_mode && !self.search_input.as_str().is_empty() {
                self.search_input.as_str()
            } else {
                "Search apps and files"
            },
            if self.search_mode && !self.search_input.as_str().is_empty() {
                theme::TEXT
            } else {
                theme::MUTED
            },
            theme::Font::Body,
        );
        if self.search_mode {
            theme::text(
                canvas,
                b.x + 28,
                b.y + 106,
                "Search results",
                theme::MUTED,
                theme::Font::Small,
            );
            let start = self.selection.saturating_sub(8);
            let row_limit = ((b.height - 148).max(0) / 32).min(9) as usize;
            for row in 0..row_limit {
                let index = start + row;
                let Some(result) = self.search_result(index) else {
                    break;
                };
                let y = b.y + 116 + row as i32 * 32;
                let selected = index == self.selection;
                if selected {
                    theme::rounded_rect(
                        canvas,
                        Bounds {
                            x: b.x + 24,
                            y,
                            width: (b.width - 48).max(0),
                            height: 30,
                        },
                        8,
                        theme::ACCENT,
                    );
                    theme::rounded_rect(
                        canvas,
                        Bounds {
                            x: b.x + 25,
                            y: y + 1,
                            width: (b.width - 50).max(0),
                            height: 28,
                        },
                        7,
                        theme::ACCENT_SOFT,
                    );
                }
                let (label, icon) = match result {
                    SearchResult::App(app) => (app.title(), app_icon(app)),
                    SearchResult::File(i) => (self.files[i].path.as_str(), theme::Icon::Files),
                    SearchResult::Shortcuts => ("Keyboard shortcuts", theme::Icon::Notices),
                };
                theme::icon(canvas, icon, b.x + 34, y + 7, 16, theme::ACCENT);
                theme::text(
                    canvas,
                    b.x + 62,
                    y + 4,
                    label,
                    theme::TEXT,
                    theme::Font::Body,
                );
            }
            if self.search_count() == 0 {
                theme::text(
                    canvas,
                    b.x + 28,
                    b.y + 126,
                    "No matching apps or files",
                    theme::MUTED,
                    theme::Font::Body,
                );
            }
        } else {
            theme::text(
                canvas,
                b.x + 28,
                b.y + 105,
                "Pinned",
                theme::TEXT,
                theme::Font::Small,
            );
            let app_count = App::ALL.len();
            let mut count = Line::new();
            use core::fmt::Write;
            let _ = write!(count, "{} apps", app_count);
            let count_width = theme::text_width(count.as_str(), theme::Font::Small);
            theme::text(
                canvas,
                b.x + b.width - 28 - count_width,
                b.y + 105,
                count.as_str(),
                theme::MUTED,
                theme::Font::Small,
            );
            for (index, app) in App::ALL.into_iter().enumerate() {
                let Some(item) = self.home_action_bounds(index) else {
                    continue;
                };
                let fill = if index == self.selection {
                    theme::ACCENT_SOFT
                } else {
                    theme::SURFACE_RAISED
                };
                let border = if index == self.selection {
                    theme::ACCENT
                } else {
                    theme::BORDER
                };
                theme::rounded_rect(canvas, item, 8, border);
                theme::rounded_rect(
                    canvas,
                    Bounds {
                        x: item.x + 1,
                        y: item.y + 1,
                        width: (item.width - 2).max(0),
                        height: (item.height - 2).max(0),
                    },
                    7,
                    fill,
                );
                let icon_tile = Bounds {
                    x: item.x + (item.width - 42).max(0) / 2,
                    y: item.y + 8,
                    width: item.width.min(42),
                    height: 42,
                };
                theme::rounded_rect(canvas, icon_tile, 12, theme::ACCENT_SOFT);
                theme::icon(
                    canvas,
                    app_icon(app),
                    icon_tile.x + (icon_tile.width - 26) / 2,
                    icon_tile.y + 8,
                    26,
                    theme::ACCENT,
                );
                let label = app.title();
                theme::text(
                    canvas,
                    item.x + (item.width - theme::text_width(label, theme::Font::Small)) / 2,
                    item.y + 58,
                    label,
                    theme::TEXT,
                    theme::Font::Small,
                );
            }
            theme::text(
                canvas,
                b.x + 28,
                b.y + 224,
                "Window tools",
                theme::TEXT,
                theme::Font::Small,
            );
            for index in App::ALL.len()..HOME_ACTION_COUNT {
                let Some(item) = self.home_action_bounds(index) else {
                    continue;
                };
                let fill = if self.selection == index {
                    theme::ACCENT_SOFT
                } else {
                    theme::SURFACE_RAISED
                };
                let border = if self.selection == index {
                    theme::ACCENT
                } else {
                    theme::BORDER
                };
                theme::rounded_rect(canvas, item, 8, border);
                theme::rounded_rect(
                    canvas,
                    Bounds {
                        x: item.x + 1,
                        y: item.y + 1,
                        width: (item.width - 2).max(0),
                        height: (item.height - 2).max(0),
                    },
                    7,
                    fill,
                );
                canvas.rect(item.x + 12, item.y + 12, 8, 8, theme::ACCENT);
                let label = if index == desktop::ACTIONS.len() {
                    "Keyboard shortcuts"
                } else {
                    desktop::ACTIONS[index].label()
                };
                theme::text(
                    canvas,
                    item.x + 30,
                    item.y + 7,
                    label,
                    theme::TEXT,
                    theme::Font::Small,
                );
            }
        }
        let footer_y = b.y + b.height - 22;
        canvas.rect(
            b.x + 24,
            footer_y - 8,
            (b.width - 48).max(0),
            1,
            theme::BORDER,
        );
        theme::text(
            canvas,
            b.x + 28,
            footer_y,
            "Arrows choose    Enter opens    Esc closes",
            theme::MUTED,
            theme::Font::Small,
        );
    }
    fn render_shortcuts(&self, canvas: &mut Canvas<'_>) {
        let b = self.shortcuts_bounds();
        theme::shadow(canvas, b, 16);
        theme::rounded_rect(canvas, b, 14, theme::SURFACE);
        theme::outline(canvas, b, 14, theme::BORDER);
        canvas.rect(b.x + 18, b.y + 1, (b.width - 36).max(0), 2, theme::ACCENT);
        theme::icon(
            canvas,
            theme::Icon::Notices,
            b.x + 22,
            b.y + 19,
            22,
            theme::ACCENT,
        );
        theme::text(
            canvas,
            b.x + 54,
            b.y + 16,
            "Keyboard shortcuts",
            theme::TEXT,
            theme::Font::Title,
        );
        theme::text(
            canvas,
            b.x + 22,
            b.y + 48,
            "Use Tab or the arrow keys to browse. Escape closes this guide.",
            theme::MUTED,
            theme::Font::Small,
        );
        let key_x = b.x + b.width * 58 / 100;
        for (index, entry) in shortcuts::ENTRIES.iter().enumerate() {
            let Some(row) = self.shortcut_row_bounds(index) else {
                continue;
            };
            let selected = index == self.shortcuts.selected();
            if selected {
                theme::rounded_rect(canvas, row, 7, theme::ACCENT);
                theme::rounded_rect(
                    canvas,
                    Bounds {
                        x: row.x + 1,
                        y: row.y + 1,
                        width: row.width - 2,
                        height: row.height - 2,
                    },
                    6,
                    theme::ACCENT_SOFT,
                );
            } else if index % 2 == 1 {
                theme::rounded_rect(canvas, row, 7, theme::SURFACE_RAISED);
            }
            let label_columns = text_fit_columns(entry.name, key_x - row.x - 18);
            draw_limited(
                canvas,
                row.x + 10,
                row.y + 1,
                entry.name,
                label_columns,
                theme::TEXT,
            );
            let key_columns = text_fit_columns(entry.keys, row.x + row.width - key_x - 12);
            draw_limited(
                canvas,
                key_x,
                row.y + 1,
                entry.keys,
                key_columns,
                if selected {
                    theme::ACCENT
                } else {
                    theme::MUTED
                },
            );
        }
        let close = self.shortcut_close_bounds();
        if self.shortcuts.selected() == shortcuts::ENTRIES.len() {
            theme::rounded_rect(canvas, close, 8, theme::TEXT);
            theme::rounded_rect(
                canvas,
                Bounds {
                    x: close.x + 2,
                    y: close.y + 2,
                    width: close.width - 4,
                    height: close.height - 4,
                },
                6,
                theme::ACCENT,
            );
        } else {
            theme::rounded_rect(canvas, close, 8, theme::ACCENT);
        }
        theme::text(
            canvas,
            close.x + 18,
            close.y,
            "Close",
            0xffffff,
            theme::Font::Small,
        );
    }
    // Coordinates and available text extent stay explicit in drawing helpers.
    #[allow(clippy::too_many_arguments)]
    fn panel_line(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        row: usize,
        text: &str,
        columns: usize,
        height: i32,
        color: u32,
    ) {
        if 64 + row as i32 * 24 + 7 * ui_scale(canvas) < height - 8 {
            draw_limited(
                canvas,
                x + 12,
                y + 64 + row as i32 * 24,
                text,
                columns,
                color,
            );
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn render_lines(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        lines: &[Line],
        offset: usize,
        columns: usize,
        height: i32,
    ) {
        for (i, line) in lines.iter().enumerate() {
            self.panel_line(
                canvas,
                x,
                y,
                i + offset,
                line.as_str(),
                columns,
                height,
                theme::TEXT,
            );
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn render_notification_toast(&self, canvas: &mut Canvas<'_>, id: u64) {
        let Some(n) = self.notifications.get(id) else {
            return;
        };
        let b = self.notification_bounds();
        canvas.rect(b.x, b.y + b.height - 100, b.width, 100, theme::BACKGROUND);
        let cols = ((b.width - 24).max(0) / (6 * ui_scale(canvas))) as usize;
        draw_limited(
            canvas,
            b.x + 12,
            b.y + b.height - 88,
            n.title.as_str(),
            cols,
            theme::TEXT,
        );
        for (i, part) in n.label.as_str().as_bytes().chunks(cols.max(1)).enumerate() {
            draw_limited(
                canvas,
                b.x + 12,
                b.y + b.height - 62 + i as i32 * 20,
                core::str::from_utf8(part).unwrap_or(""),
                cols,
                theme::MUTED,
            );
        }
    }
    fn render_notifications(&self, canvas: &mut Canvas<'_>) {
        let b = self.notification_bounds();
        let scale = ui_scale(canvas);
        let cols = ((b.width - 24).max(0) / (6 * scale)) as usize;
        theme::shadow(canvas, b, 12);
        theme::rounded_rect(canvas, b, 12, theme::SURFACE);
        theme::outline(canvas, b, 12, theme::BORDER);
        let mut heading = Line::new();
        use core::fmt::Write;
        let _ = write!(
            heading,
            "NOTICES {} unread / {} stored",
            self.notifications.unread(),
            self.notifications.len()
        );
        draw_limited(
            canvas,
            b.x + 12,
            b.y + 16,
            heading.as_str(),
            cols,
            theme::TEXT,
        );
        draw_limited(
            canvas,
            b.x + 12,
            b.y + 42,
            if self.notifications.do_not_disturb() {
                "Do Not Disturb: ON (session)"
            } else {
                "Do Not Disturb: OFF (session)"
            },
            cols,
            theme::MUTED,
        );
        let rows = self.notification_rows();
        let start = self
            .notification_index()
            .saturating_sub(rows.saturating_sub(1));
        for row in 0..rows {
            let Some(n) = self.notifications.at(start + row) else {
                break;
            };
            let y = b.y + 72 + row as i32 * 64;
            if Some(n.id) == self.notification_selected {
                canvas.rect(
                    b.x + 8,
                    y,
                    b.width - 16,
                    62,
                    if self.notification_control == 0 {
                        theme::ACCENT_SOFT
                    } else {
                        theme::BACKGROUND
                    },
                );
            }
            let mut title = Line::new();
            let _ = write!(
                title,
                "{} {}",
                if n.read { " " } else { "*" },
                n.title.as_str()
            );
            let color = match n.priority {
                notifications::Priority::Info => theme::TEXT,
                notifications::Priority::Warning => 0x965d00,
                notifications::Priority::Critical => 0xb42338,
            };
            draw_limited(canvas, b.x + 12, y + 4, title.as_str(), cols, color);
            for (i, part) in n
                .label
                .as_str()
                .as_bytes()
                .chunks(cols.max(1))
                .take(2)
                .enumerate()
            {
                draw_limited(
                    canvas,
                    b.x + 12,
                    y + 24 + i as i32 * 18,
                    core::str::from_utf8(part).unwrap_or(""),
                    cols,
                    theme::MUTED,
                );
            }
        }
        let detail_y = b.y + 80 + rows as i32 * 64;
        if let Some(n) = self.notification_selected() {
            let mut origin = Line::new();
            let _ = match n.source {
                notifications::Source::Process { pid } => {
                    write!(origin, "Process PID {} / {:?}", pid, n.priority)
                }
                notifications::Source::Service { id } => {
                    write!(origin, "Service {} / {:?}", id, n.priority)
                }
                notifications::Source::System => write!(origin, "Helix system / {:?}", n.priority),
            };
            draw_limited(
                canvas,
                b.x + 12,
                detail_y,
                origin.as_str(),
                cols,
                theme::TEXT,
            );
            let mut row = 0;
            wrap_text(n.body.as_str(), cols.max(1), |part| {
                let y = detail_y + 24 + row * 20;
                if y + 14 < b.y + b.height - 56 {
                    draw_limited(canvas, b.x + 12, y, part, cols, theme::MUTED);
                }
                row += 1;
            });
        } else {
            draw_limited(
                canvas,
                b.x + 12,
                b.y + 80,
                "No runtime events in this session",
                cols,
                theme::MUTED,
            );
        }
        let width = ((b.width - 24) / 3).max(1);
        for (i, label) in ["Dismiss", "Clear read", "DND"].into_iter().enumerate() {
            let x = b.x + 12 + i as i32 * width;
            canvas.rect(
                x,
                b.y + b.height - 48,
                width - 4,
                32,
                if self.notification_control == i + 1 {
                    theme::ACCENT
                } else {
                    theme::BACKGROUND
                },
            );
            theme::text(
                canvas,
                x + 8,
                b.y + b.height - 42,
                label,
                if self.notification_control == i + 1 {
                    self.accent_foreground()
                } else {
                    theme::TEXT
                },
                theme::Font::Body,
            );
        }
    }
    fn control_border(canvas: &mut Canvas<'_>, b: Bounds, color: u32) {
        canvas.rect(b.x + 5, b.y, (b.width - 10).max(0), 1, color);
        canvas.rect(b.x + 5, b.y + b.height - 1, (b.width - 10).max(0), 1, color);
        canvas.rect(b.x, b.y + 5, 1, (b.height - 10).max(0), color);
        canvas.rect(b.x + b.width - 1, b.y + 5, 1, (b.height - 10).max(0), color);
    }
    fn render_appearance(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        columns: usize,
    ) {
        let draft = self.appearance.draft();

        // These soft group surfaces sit behind the existing interactive
        // controls, so the visual grouping follows the real keyboard and
        // pointer targets without creating decorative hit areas.
        for row in [
            Bounds {
                x: x + 8,
                y: y + 68,
                width: width - 16,
                height: 40,
            },
            Bounds {
                x: x + 8,
                y: y + 132,
                width: width - 16,
                height: 40,
            },
            Bounds {
                x: x + 8,
                y: y + 192,
                width: width - 16,
                height: 40,
            },
        ] {
            theme::rounded_rect(canvas, row, 8, theme::SURFACE_RAISED);
            theme::outline(canvas, row, 8, theme::BORDER);
        }

        theme::text(
            canvas,
            x + 12,
            y + 47,
            "Theme",
            theme::TEXT,
            theme::Font::Small,
        );
        theme::text(
            canvas,
            x + 12,
            y + 111,
            "Accent color",
            theme::TEXT,
            theme::Font::Small,
        );
        theme::text(
            canvas,
            x + 12,
            y + 171,
            "Changes",
            theme::TEXT,
            theme::Font::Small,
        );
        let accent_colors = [0x2368df, 0x7256d9, 0x008c87, 0xe17928];
        for group in 0..3 {
            for index in 0..if group == 1 { 4 } else { 3 } {
                let Some(button) = self.appearance_control_bounds(group, index) else {
                    continue;
                };
                if button.y + button.height > y + height - 8 {
                    continue;
                }
                let selected = match group {
                    0 => draft.theme as usize == index,
                    1 => draft.accent as usize == index,
                    _ => false,
                };
                let local = if group == 0 {
                    1
                } else if group == 1 {
                    2
                } else {
                    index as u32 + 3
                };
                let focused = self.file_focus.focus_visible()
                    && self
                        .file_focus
                        .focused()
                        .is_some_and(|id| id.scope == 5 && id.local == local)
                    && (group == 2 || selected);
                let action_primary = group == 2
                    && index == 0
                    && self.active_appearance().theme != appearance::Theme::HighContrast
                    && matches!(
                        self.appearance.status(),
                        appearance_ui::Status::Unsaved | appearance_ui::Status::SaveFailed
                    );
                theme::rounded_rect(
                    canvas,
                    button,
                    7,
                    if selected || action_primary {
                        theme::ACCENT_SOFT
                    } else {
                        theme::BACKGROUND
                    },
                );
                if selected
                    || focused
                    || action_primary
                    || self.active_appearance().theme == appearance::Theme::HighContrast
                {
                    Self::control_border(
                        canvas,
                        button,
                        if selected || focused || action_primary {
                            theme::ACCENT
                        } else {
                            theme::BORDER
                        },
                    );
                }
                if focused {
                    canvas.rect(
                        button.x,
                        button.y + button.height - 2,
                        button.width,
                        2,
                        theme::ACCENT,
                    );
                }
                let label = match group {
                    0 => ["Light", "Dark", "High contrast"][index],
                    1 => ["Blue", "Violet", "Teal", "Orange"][index],
                    _ => ["Save", "Revert", "Diagnostics"][index],
                };
                if group == 1 {
                    canvas.rect(button.x + 8, button.y + 11, 10, 10, accent_colors[index]);
                }
                let label_x = button.x + if group == 1 { 24 } else { 8 };
                theme::text(
                    canvas,
                    label_x,
                    button.y + 5,
                    label,
                    if group == 2
                        && index == 0
                        && (!self.appearance.loaded()
                            || matches!(
                                self.appearance.status(),
                                appearance_ui::Status::Saving
                                    | appearance_ui::Status::Corrupt
                                    | appearance_ui::Status::UnsupportedScale
                            ))
                    {
                        theme::MUTED
                    } else {
                        theme::TEXT
                    },
                    theme::Font::Body,
                );
            }
        }
        let status = match self.appearance.status() {
            appearance_ui::Status::Ready => {
                if self.appearance.loaded() {
                    "Saved appearance is active."
                } else {
                    "Appearance has not been loaded; editing is disabled."
                }
            }
            appearance_ui::Status::Unsaved => "Unsaved choices. Save to activate and persist.",
            appearance_ui::Status::Saving => "Saving appearance to the real store...",
            appearance_ui::Status::Saved => "Appearance saved and active.",
            appearance_ui::Status::SaveFailed => {
                "Save failed. Active appearance unchanged; retry or Revert."
            }
            appearance_ui::Status::Corrupt => {
                "Preference record is corrupt. No settings were overwritten."
            }
            appearance_ui::Status::UnsupportedScale => {
                "Stored text scale is unsupported. No settings were overwritten."
            }
        };

        let status_top = y + 238;
        let status_bottom = (y + height - 72).max(status_top + 40);
        let status_box = Bounds {
            x: x + 8,
            y: status_top,
            width: width - 16,
            height: status_bottom - status_top,
        };
        theme::rounded_rect(canvas, status_box, 8, theme::BACKGROUND);
        theme::outline(canvas, status_box, 8, theme::BORDER);
        theme::text(
            canvas,
            x + 16,
            status_top + 7,
            "Status",
            theme::TEXT,
            theme::Font::Small,
        );
        let mut row = 0;
        wrap_text(status, columns, |part| {
            if status_top + 27 + row * 20 + 16 < status_bottom {
                draw_limited(
                    canvas,
                    x + 16,
                    status_top + 25 + row * 20,
                    part,
                    columns,
                    theme::MUTED,
                );
            }
            row += 1;
        });
        if height > 340 {
            let help_y = y + height - 60;
            canvas.rect(x + 12, help_y - 7, width - 24, 1, theme::BORDER);
            draw_limited(
                canvas,
                x + 12,
                help_y,
                "Text size: 100%",
                columns,
                theme::MUTED,
            );
            draw_limited(
                canvas,
                x + 12,
                y + height - 36,
                "Tab focus  /  Arrows choose  /  Enter applies",
                columns,
                theme::MUTED,
            );
        }
    }
    fn render_file_dialog(&self, canvas: &mut Canvas<'_>) {
        use files_actions::Phase;
        let b = self.file_dialog_bounds();
        let phase = self.file_dialog.phase();
        let columns = ((b.width - 40).max(0) / 12) as usize;
        theme::shadow(canvas, b, 12);
        theme::rounded_rect(canvas, b, 12, theme::SURFACE);
        theme::outline(canvas, b, 12, theme::BORDER);
        theme::text(
            canvas,
            b.x + 20,
            b.y + 18,
            match phase {
                Phase::ConfirmDelete => "Permanently delete file?",
                Phase::ConfirmOverwrite => "Replace destination file?",
                Phase::Inspecting => "Checking destination",
                Phase::Committing => "Saving file operation",
                _ => {
                    if self.file_dialog.is_copy() {
                        "Copy file"
                    } else if self.file_dialog.selected().is_some() {
                        "Rename file"
                    } else {
                        "Create file"
                    }
                }
            },
            theme::TEXT,
            theme::Font::Title,
        );
        if let Some(source) = self.file_dialog.selected() {
            draw_limited(
                canvas,
                b.x + 20,
                b.y + 52,
                source.path.as_str(),
                columns,
                theme::MUTED,
            );
        }
        if matches!(
            phase,
            Phase::Editing | Phase::Inspecting | Phase::Committing
        ) && self
            .file_dialog
            .selected()
            .is_none_or(|_| phase != Phase::Committing || !self.file_dialog.input().is_empty())
        {
            let field = Bounds {
                x: b.x + 20,
                y: b.y + 88,
                width: (b.width - 40).max(0),
                height: 40,
            };
            theme::rounded_rect(canvas, field, 6, theme::SURFACE_RAISED);
            canvas.rect(
                field.x,
                field.y + field.height - 2,
                field.width,
                2,
                if self
                    .file_focus
                    .focused()
                    .is_some_and(|id| id.scope == 3 && id.local == 1)
                    && self.file_focus.focus_visible()
                {
                    theme::ACCENT
                } else {
                    theme::BORDER
                },
            );
            draw_limited(
                canvas,
                field.x + 10,
                field.y + 10,
                self.file_dialog.input(),
                columns.saturating_sub(1),
                theme::TEXT,
            );
        } else {
            let warning = if phase == Phase::ConfirmDelete {
                "This permanently removes the file. There is no Trash."
            } else {
                "The existing destination will be replaced."
            };
            let mut row = 0;
            wrap_text(warning, columns, |part| {
                draw_limited(
                    canvas,
                    b.x + 20,
                    b.y + 92 + row * 20,
                    part,
                    columns,
                    0xb42338,
                );
                row += 1;
            });
            if let Some(target) = self.file_dialog.replacement() {
                draw_limited(
                    canvas,
                    b.x + 20,
                    b.y + 120,
                    target.path.as_str(),
                    columns,
                    theme::TEXT,
                );
            }
        }
        if let Some(error) = self.file_dialog.error() {
            let mut row = 0;
            wrap_text(error.message(), columns, |part| {
                if row < 3 {
                    draw_limited(
                        canvas,
                        b.x + 20,
                        b.y + 152 + row * 20,
                        part,
                        columns,
                        0xb42338,
                    );
                }
                row += 1;
            });
        }
        for (local, label) in [
            (2, "Cancel"),
            (
                3,
                if phase == Phase::ConfirmDelete {
                    "Delete"
                } else if phase == Phase::ConfirmOverwrite {
                    "Replace"
                } else {
                    "Apply"
                },
            ),
        ] {
            let button = Bounds {
                x: b.x + 20 + (local - 2) * 124,
                y: b.y + b.height - 60,
                width: 112,
                height: 36,
            };
            let enabled = self
                .file_focus
                .state(keyboard_focus::ControlId {
                    scope: 3,
                    local: local as u32,
                })
                .is_some_and(|s| s.control.enabled);
            let selected = self
                .file_focus
                .focused()
                .is_some_and(|id| id.scope == 3 && id.local == local as u32);
            theme::rounded_rect(
                canvas,
                button,
                6,
                if selected && enabled {
                    theme::ACCENT
                } else {
                    theme::BACKGROUND
                },
            );
            if self.active_appearance().theme == appearance::Theme::HighContrast {
                Self::control_border(canvas, button, theme::BORDER);
            }
            theme::text(
                canvas,
                button.x + 12,
                button.y + 8,
                label,
                if selected && enabled {
                    self.accent_foreground()
                } else {
                    theme::MUTED
                },
                theme::Font::Body,
            );
        }
    }
    fn render_file_portal(&self, canvas: &mut Canvas<'_>) {
        let (Some(portal), Some(b)) = (self.file_portal, self.file_portal_bounds()) else {
            return;
        };
        let scale = ui_scale(canvas);
        let cols = ((b.width - 32).max(0) / (6 * scale)) as usize;
        theme::shadow(canvas, b, 12);
        theme::rounded_rect(canvas, b, 12, theme::SURFACE);
        theme::outline(canvas, b, 12, theme::BORDER);
        draw_limited(
            canvas,
            b.x + 16,
            b.y + 16,
            "NEXT / TRUSTED FILE ACCESS",
            cols,
            theme::TEXT,
        );
        let identity_columns = cols.max(1);
        for (i, part) in portal
            .package
            .as_str()
            .as_bytes()
            .chunks(identity_columns)
            .enumerate()
        {
            draw_limited(
                canvas,
                b.x + 16,
                b.y + 42 + i as i32 * 24,
                core::str::from_utf8(part).unwrap_or(""),
                cols,
                theme::TEXT,
            );
        }
        let identity_extra = (self.portal_identity_rows(b) as i32 - 1) * 24;
        use core::fmt::Write;
        let mut grant = Line::new();
        let _ = write!(
            grant,
            "PID {} / Grant: {}",
            portal.pid,
            if portal.writable {
                "READ + WRITE"
            } else {
                "READ ONLY"
            }
        );
        draw_limited(
            canvas,
            b.x + 16,
            b.y + 68 + identity_extra,
            grant.as_str(),
            cols,
            theme::TEXT,
        );
        draw_limited(
            canvas,
            b.x + 16,
            b.y + 94 + identity_extra,
            "Choose an existing /native/ file:",
            cols,
            theme::MUTED,
        );
        let rows = self.portal_rows(b);
        let start = portal.selection.saturating_sub(rows.saturating_sub(1));
        for row in 0..rows {
            let index = start + row;
            let Some(file) = self.portal_file(index) else {
                break;
            };
            let y = b.y + self.portal_list_top(b) + 6 + row as i32 * 24;
            if index == portal.selection {
                canvas.rect(b.x + 12, y - 6, b.width - 24, 22, theme::ACCENT_SOFT);
                canvas.rect(b.x + 12, y - 6, 2, 22, theme::ACCENT);
            }
            draw_limited(canvas, b.x + 16, y, file.path.as_str(), cols, theme::TEXT);
        }
        if self.portal_count() == 0 {
            draw_limited(
                canvas,
                b.x + 16,
                b.y + self.portal_list_top(b) + 6,
                "No eligible files; Cancel to return",
                cols,
                theme::MUTED,
            );
        }
        for (i, label) in ["Cancel", "Approve"].into_iter().enumerate() {
            let x = b.x + 16 + i as i32 * 116;
            let selected = portal.approve == (i == 1);
            canvas.rect(
                x,
                b.y + b.height - 68,
                100,
                32,
                if selected {
                    theme::ACCENT
                } else {
                    theme::BACKGROUND
                },
            );
            theme::text(
                canvas,
                x + 8,
                b.y + b.height - 62,
                label,
                if selected {
                    self.accent_foreground()
                } else {
                    theme::TEXT
                },
                theme::Font::Body,
            );
        }
        draw_limited(
            canvas,
            b.x + 16,
            b.y + b.height - 24,
            "Arrows: file Tab: button Enter: act Esc: deny",
            cols,
            theme::MUTED,
        );
    }
    fn render_editor(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        _width: i32,
        height: i32,
        columns: usize,
    ) {
        let mut heading = Line::new();
        use core::fmt::Write;
        let _ = write!(
            heading,
            "{}{}",
            self.document.path(),
            if self.document.dirty() {
                " * unsaved"
            } else {
                ""
            }
        );
        self.panel_line(
            canvas,
            x,
            y,
            0,
            heading.as_str(),
            columns,
            height,
            theme::ACCENT,
        );
        let (cursor_line, cursor_column) = self.document.line_column();
        let rows = ((height - 156).max(0) / 20) as usize;
        let first = cursor_line.saturating_sub(rows.saturating_sub(1));
        for (i, line) in self
            .document
            .text()
            .split('\n')
            .enumerate()
            .skip(first)
            .take(rows)
        {
            let yy = y + 96 + (i - first) as i32 * 20;
            let horizontal = if i == cursor_line {
                cursor_column.saturating_sub(columns.saturating_sub(1))
            } else {
                0
            };
            let start = line
                .char_indices()
                .nth(horizontal)
                .map_or(line.len(), |(index, _)| index);
            draw_limited(canvas, x + 12, yy, &line[start..], columns, theme::TEXT);
            if i == cursor_line {
                canvas.rect(
                    x + 12
                        + theme::text_width(
                            &line[start
                                ..line
                                    .char_indices()
                                    .nth(cursor_column)
                                    .map_or(line.len(), |(i, _)| i)],
                            theme::Font::Body,
                        ),
                    yy + 7 * ui_scale(canvas) + 2,
                    5 * ui_scale(canvas),
                    2,
                    theme::ACCENT,
                );
            }
        }
        if height > 100 {
            draw_limited(
                canvas,
                x + 12,
                y + height - 40,
                self.editor_status.as_str(),
                columns,
                0xb42338,
            );
            draw_limited(
                canvas,
                x + 12,
                y + height - 20,
                "Ctrl+S saves / UTF-8 edits / Latin display font",
                columns,
                theme::MUTED,
            );
        }
    }
    fn render_editor_confirmation(&self, canvas: &mut Canvas<'_>) {
        let b = self.desktop.window(App::Editor).bounds;
        let (x, y, _width) = (b.x, b.y, b.width);
        let columns = ((b.width - 24).max(0) / 12) as usize;
        if self.editor_deferred.is_some() {
            canvas.rect(x + 8, y + 56, _width - 16, 100, theme::SURFACE);
            draw_limited(
                canvas,
                x + 16,
                y + 68,
                "Unsaved changes: choose an action",
                columns.saturating_sub(1),
                0xb42338,
            );
            for (i, label) in ["Cancel", "Discard", "Save"].into_iter().enumerate() {
                let xx = x + 16 + i as i32 * 96;
                if i == self.editor_choice {
                    canvas.rect(xx - 4, y + 96, 92, 28, theme::ACCENT);
                }
                theme::text(
                    canvas,
                    xx,
                    y + 101,
                    label,
                    if i == self.editor_choice {
                        self.accent_foreground()
                    } else {
                        theme::TEXT
                    },
                    theme::Font::Body,
                );
            }
            draw_limited(
                canvas,
                x + 16,
                y + 136,
                "Tab selects / Enter applies / Esc cancels",
                columns.saturating_sub(1),
                theme::MUTED,
            );
        }
    }

    fn render_terminal(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        columns: usize,
    ) {
        // Give the console a focused, Windows Terminal-inspired work surface
        // inside the light Prism window chrome. High contrast keeps its native
        // semantic colors and avoids a fixed dark palette.
        let high_contrast = self.active_appearance().theme == appearance::Theme::HighContrast;
        let surface = if high_contrast {
            theme::BACKGROUND
        } else {
            0x101722
        };
        let text = if high_contrast { theme::TEXT } else { 0xdce5f2 };
        let muted = if high_contrast {
            theme::MUTED
        } else {
            0x8392a8
        };
        let inset_x = x + 8;
        let inset_y = y + 48;
        let inset_width = (width - 16).max(0);
        let inset_height = (height - 56).max(0);
        if inset_width >= 32 && inset_height >= 32 {
            theme::rounded_rect(
                canvas,
                Bounds {
                    x: inset_x,
                    y: inset_y,
                    width: inset_width,
                    height: inset_height,
                },
                8,
                surface,
            );
            canvas.rect(inset_x + 9, inset_y + 28, inset_width - 18, 1, muted);
            theme::icon(
                canvas,
                theme::Icon::Terminal,
                inset_x + 13,
                inset_y + 5,
                18,
                if high_contrast {
                    theme::ACCENT
                } else {
                    0x9bbcff
                },
            );
            canvas.rect(inset_x + 12, inset_y + 26, 22, 2, theme::ACCENT);
        }

        // Keep a small-window fallback for resize previews and low-resolution
        // modes where the inset terminal surface would leave no readable area.
        let compact = width < 112 || height < 112;
        let text_x = if compact { x + 12 } else { inset_x + 16 };
        let content_top = if compact { y + 56 } else { inset_y + 42 };
        let prompt_y = if compact {
            y + height - 28
        } else {
            y + height - 34
        };
        let available_columns = if compact {
            columns
        } else {
            columns.min(((width - 48).max(0) / 12) as usize)
        };
        let line_height = 22;
        let mut visual = [Line::new(); HISTORY_CAPACITY];
        let mut count = 0;
        for line in self.history.iter().take(self.count) {
            wrap_text(line.as_str(), available_columns.max(1), |part| {
                if count == HISTORY_CAPACITY {
                    visual.copy_within(1..HISTORY_CAPACITY, 0);
                    count -= 1;
                }
                visual[count].set(part);
                count += 1;
            });
        }
        let history_bottom = prompt_y - 12;
        let visible = count.min(((history_bottom - content_top).max(0) / line_height) as usize);
        let first = count - visible;
        for index in 0..visible {
            draw_limited(
                canvas,
                text_x,
                content_top + index as i32 * line_height,
                visual[first + index].as_str(),
                available_columns,
                if high_contrast { theme::TEXT } else { text },
            );
        }

        if !compact && inset_width >= 96 {
            canvas.rect(inset_x + 10, prompt_y - 7, inset_width - 20, 1, muted);
            canvas.rect(inset_x + 10, prompt_y - 2, 2, 20, theme::ACCENT);
        }
        let prompt_x = text_x;
        theme::text(
            canvas,
            prompt_x,
            prompt_y,
            ">",
            theme::ACCENT,
            theme::Font::Body,
        );
        let input = self.input.as_str();
        let cols = available_columns.saturating_sub(2);
        let first = input.len().saturating_sub(cols);
        draw_limited(
            canvas,
            prompt_x + 6 * ui_scale(canvas),
            prompt_y,
            &input[first..],
            cols,
            if high_contrast { theme::TEXT } else { text },
        );
        if width > 36 {
            canvas.rect(
                prompt_x
                    + 6 * ui_scale(canvas)
                    + theme::text_width(&input[first..], theme::Font::Body),
                prompt_y + 7 * ui_scale(canvas) + 2,
                2 * ui_scale(canvas),
                12 * ui_scale(canvas),
                theme::ACCENT,
            );
        }
    }
    fn render_files_directory(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        columns: usize,
    ) {
        let selected_kind = self.files_directory.selected().map(|entry| entry.kind);
        let selected_file = selected_kind == Some(files_directory::Kind::File);
        let toolbar = Bounds {
            x: x + 8,
            y: y + 48,
            width: (width - 16).max(0),
            height: 40,
        };
        theme::rounded_rect(canvas, toolbar, 9, theme::SURFACE);
        Self::control_border(canvas, toolbar, theme::BORDER);
        let labels = [
            "Up",
            "New folder",
            if selected_file {
                "Move file"
            } else {
                "Move folder"
            },
            if selected_file {
                "Trash file"
            } else {
                "Remove folder"
            },
            "Trash",
            "Find",
        ];
        for (index, label) in labels.into_iter().enumerate() {
            let Some(button) = self.file_action_button_bounds(index) else {
                continue;
            };
            let enabled = match index {
                0 | 1 | 4 | 5 => true,
                2 => selected_file || self.files_directory.can_move_selected(),
                3 => selected_file || self.files_directory.can_remove_selected(),
                _ => false,
            };
            theme::rounded_rect(
                canvas,
                button,
                8,
                if enabled {
                    theme::ACCENT_SOFT
                } else {
                    theme::SURFACE
                },
            );
            Self::control_border(
                canvas,
                button,
                if enabled {
                    theme::BORDER
                } else {
                    theme::BACKGROUND
                },
            );
            theme::text(
                canvas,
                button.x + 7,
                button.y + 6,
                if button.width < 82 {
                    [
                        "Up",
                        "New",
                        "Move",
                        if selected_file { "Trash" } else { "Remove" },
                        "Trash",
                        "Find",
                    ][index]
                } else {
                    label
                },
                if enabled { theme::TEXT } else { theme::MUTED },
                theme::Font::Small,
            );
            if self.file_focus.focus_visible()
                && self
                    .file_focus
                    .focused()
                    .is_some_and(|id| id.scope == 2 && id.local == index as u32 + 2)
            {
                theme::outline(canvas, button, 8, theme::ACCENT);
            }
        }
        let address = Bounds {
            x: x + 8,
            y: y + 98,
            width: (width - 16).max(0),
            height: 30,
        };
        theme::rounded_rect(canvas, address, 8, theme::SURFACE_RAISED);
        Self::control_border(
            canvas,
            address,
            if self.files_directory_address_editing {
                theme::ACCENT
            } else {
                theme::BORDER
            },
        );
        theme::text(
            canvas,
            address.x + 10,
            address.y + 4,
            "LOCATION",
            theme::MUTED,
            theme::Font::Small,
        );
        let text = self.files_directory.address();
        draw_limited(
            canvas,
            address.x + 86,
            address.y + 4,
            text,
            ((width - 220).max(0) / 8) as usize,
            theme::TEXT,
        );
        let count = self
            .files_directory
            .listing()
            .map_or(0, |listing| listing.entries.iter().flatten().count());
        let count_label = match count {
            0 => "0 ITEMS",
            1 => "1 ITEM",
            2 => "2 ITEMS",
            3 => "3 ITEMS",
            4 => "4 ITEMS",
            5 => "5 ITEMS",
            6 => "6 ITEMS",
            7 => "7 ITEMS",
            _ => "8 ITEMS",
        };
        theme::text(
            canvas,
            x + width - 108,
            address.y + 4,
            count_label,
            theme::MUTED,
            theme::Font::Small,
        );
        let Some(listing) = self.files_directory.listing() else {
            self.panel_line(
                canvas,
                x,
                y,
                3,
                "Loading /native directory…",
                columns,
                height,
                theme::MUTED,
            );
            return;
        };
        let visible_rows =
            ((height - 166).max(0) / 27).min(files_directory::CAPACITY as i32) as usize;
        if listing.entries.iter().all(Option::is_none) {
            let empty = Bounds {
                x: x + 8,
                y: y + 138,
                width: (width - 16).max(0),
                height: 76,
            };
            theme::rounded_rect(canvas, empty, 10, theme::SURFACE);
            theme::text(
                canvas,
                empty.x + 16,
                empty.y + 12,
                "This folder is empty",
                theme::TEXT,
                theme::Font::Body,
            );
            theme::text(
                canvas,
                empty.x + 16,
                empty.y + 42,
                "Create a folder or file to get started.",
                theme::MUTED,
                theme::Font::Small,
            );
        }
        for (row, entry) in listing
            .entries
            .iter()
            .flatten()
            .take(visible_rows)
            .enumerate()
        {
            let top = y + 136 + row as i32 * 27;
            let row_bounds = Bounds {
                x: x + 8,
                y: top,
                width: (width - 16).max(0),
                height: 25,
            };
            if self.files_directory.selected_index() == Some(row) {
                theme::rounded_rect(canvas, row_bounds, 7, theme::ACCENT_SOFT);
                canvas.rect(x + 8, top + 5, 3, 15, theme::ACCENT);
                Self::control_border(canvas, row_bounds, theme::BORDER);
            } else if row % 2 == 1 {
                theme::rounded_rect(canvas, row_bounds, 7, theme::SURFACE);
            }
            theme::rounded_rect(
                canvas,
                Bounds {
                    x: x + 15,
                    y: top + 3,
                    width: 20,
                    height: 19,
                },
                5,
                if entry.kind == files_directory::Kind::Directory {
                    theme::ACCENT_SOFT
                } else {
                    theme::BACKGROUND
                },
            );
            theme::icon(
                canvas,
                if entry.kind == files_directory::Kind::Directory {
                    theme::Icon::Files
                } else {
                    theme::Icon::Editor
                },
                x + 17,
                top + 4,
                16,
                if entry.kind == files_directory::Kind::Directory {
                    theme::ACCENT
                } else {
                    theme::MUTED
                },
            );
            let name = entry
                .path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or(entry.path.as_str());
            theme::text(
                canvas,
                x + 40,
                top + 4,
                name,
                theme::TEXT,
                theme::Font::Body,
            );
            if entry.kind == files_directory::Kind::Directory {
                theme::text(
                    canvas,
                    x + width - 78,
                    top + 7,
                    "FOLDER",
                    theme::MUTED,
                    theme::Font::Small,
                );
            } else {
                theme::text(
                    canvas,
                    x + width - 78,
                    top + 7,
                    "FILE",
                    theme::MUTED,
                    theme::Font::Small,
                );
            }
        }
        if let Some(error) = self.files_directory.error() {
            draw_limited(
                canvas,
                x + 12,
                y + height - 26,
                error.message(),
                columns,
                0xa53b34,
            );
        } else if !self.file_status.as_str().is_empty() {
            draw_limited(
                canvas,
                x + 12,
                y + height - 26,
                self.file_status.as_str(),
                columns,
                theme::MUTED,
            );
        } else {
            draw_limited(
                canvas,
                x + 12,
                y + height - 26,
                "Enter open / Backspace up / g address / n folder / f file / m move / r rename / c copy / t trash",
                columns,
                theme::MUTED,
            );
        }
        if self.files_directory.dialog_active() {
            self.render_files_directory_dialog(canvas);
        }
    }
    fn render_files_directory_dialog(&self, canvas: &mut Canvas<'_>) {
        let b = self.files_directory_dialog_bounds();
        theme::shadow(canvas, b, 8);
        theme::rounded_rect(canvas, b, 8, theme::SURFACE_RAISED);
        theme::outline(canvas, b, 8, theme::BORDER);
        theme::rounded_rect(
            canvas,
            Bounds {
                x: b.x + 1,
                y: b.y + 1,
                width: (b.width - 2).max(0),
                height: 4,
            },
            3,
            theme::ACCENT,
        );
        let title = match self.files_directory.dialog_kind() {
            Some(files_directory::DialogKind::CreateFolder) => "Create a folder",
            Some(files_directory::DialogKind::MoveFolder) => "Move an empty folder",
            Some(files_directory::DialogKind::RemoveFolder) => "Remove an empty folder?",
            None => "Folder operation",
        };
        theme::text(
            canvas,
            b.x + 16,
            b.y + 16,
            title,
            theme::TEXT,
            theme::Font::Title,
        );
        let message = match self.files_directory.dialog_kind() {
            Some(files_directory::DialogKind::CreateFolder) => "Choose a new path under /native.",
            Some(files_directory::DialogKind::MoveFolder) => {
                "Choose a destination path; folders must be empty."
            }
            Some(files_directory::DialogKind::RemoveFolder) => {
                "Removal succeeds only when the folder is empty."
            }
            None => "The folder store will confirm this operation.",
        };
        theme::text(
            canvas,
            b.x + 16,
            b.y + 52,
            message,
            theme::MUTED,
            theme::Font::Small,
        );
        let input = Bounds {
            x: b.x + 16,
            y: b.y + 78,
            width: (b.width - 32).max(0),
            height: 28,
        };
        theme::rounded_rect(canvas, input, 4, theme::BACKGROUND);
        Self::control_border(canvas, input, theme::BORDER);
        theme::text(
            canvas,
            input.x + 8,
            input.y + 8,
            self.files_directory.input(),
            theme::TEXT,
            theme::Font::Small,
        );
        if let Some(error) = self.files_directory.error() {
            theme::text(
                canvas,
                b.x + 16,
                b.y + 116,
                error.message(),
                0xa53b34,
                theme::Font::Small,
            );
        }
        for index in 0..2 {
            let button = self.files_directory_dialog_button_bounds(index);
            theme::rounded_rect(
                canvas,
                button,
                6,
                if (index == 1) == self.files_directory.approves() {
                    theme::ACCENT_SOFT
                } else {
                    theme::BACKGROUND
                },
            );
            Self::control_border(canvas, button, theme::BORDER);
            theme::text(
                canvas,
                button.x + 18,
                button.y + 9,
                if index == 0 { "Cancel" } else { "Approve" },
                theme::TEXT,
                theme::Font::Small,
            );
        }
        theme::text(
            canvas,
            b.x + 16,
            b.y + b.height - 16,
            "Tab switches choice; Enter confirms; Escape cancels.",
            theme::MUTED,
            theme::Font::Small,
        );
    }
    fn render_files(
        &self,
        canvas: &mut Canvas<'_>,
        x: i32,
        y: i32,
        _width: i32,
        height: i32,
        columns: usize,
    ) {
        if self.trash_page {
            self.render_trash_page(canvas);
            return;
        }
        if self.files_directory.listing().is_some() {
            self.render_files_directory(canvas, x, y, _width, height, columns);
            return;
        }
        for (index, label) in [
            "New file",
            "Rename",
            "Trash file",
            "Copy",
            "Trash",
            "Filter",
        ]
        .into_iter()
        .enumerate()
        {
            let Some(button) = self.file_action_button_bounds(index) else {
                continue;
            };
            let enabled = matches!(index, 0 | 4 | 5)
                || self.file_selection != 0 && self.files[self.file_selection].available;
            theme::rounded_rect(
                canvas,
                button,
                6,
                if enabled {
                    theme::ACCENT_SOFT
                } else {
                    theme::BACKGROUND
                },
            );
            if self.active_appearance().theme == appearance::Theme::HighContrast {
                Self::control_border(canvas, button, theme::BORDER);
            }
            if self.file_focus.focus_visible()
                && self
                    .file_focus
                    .focused()
                    .is_some_and(|id| id.scope == 2 && id.local == index as u32 + 2)
            {
                canvas.rect(
                    button.x,
                    button.y + button.height - 2,
                    button.width,
                    2,
                    theme::ACCENT,
                );
            }
            theme::text(
                canvas,
                button.x + 10,
                button.y + 6,
                if button.width < 86 {
                    ["New", "Rename", "Trash file", "Copy", "Trash", "Filter"][index]
                } else {
                    label
                },
                if enabled { theme::TEXT } else { theme::MUTED },
                if button.width < 86 {
                    theme::Font::Small
                } else {
                    theme::Font::Body
                },
            );
        }
        if !self.files.iter().any(|f| f.available) {
            self.panel_line(
                canvas,
                x,
                y,
                2,
                "No file data supplied by the kernel.",
                columns,
                height,
                theme::MUTED,
            );
            return;
        }
        if self.file_detail {
            let f = self.files[self.file_selection];
            self.panel_line(
                canvas,
                x,
                y,
                2,
                f.path.as_str(),
                columns,
                height,
                theme::TEXT,
            );
            self.panel_line(
                canvas,
                x,
                y,
                4,
                f.content.as_str(),
                columns,
                height,
                theme::TEXT,
            );
            self.panel_line(
                canvas,
                x,
                y,
                6,
                "Backspace returns to files.",
                columns,
                height,
                theme::MUTED,
            );
            return;
        }
        let start = self.file_list_start();
        for position in start..(start + self.file_list_rows()).min(self.file_view_len()) {
            let Some(i) = self.file_view_index(position) else {
                continue;
            };
            let file = &self.files[i];
            let row = position - start + 2;
            if i == self.file_selection && 44 + row as i32 * 16 + 8 < height - 8 {
                canvas.rect(
                    x + 8,
                    y + 58 + row as i32 * 24,
                    columns as i32 * 6 * ui_scale(canvas) + 8,
                    24,
                    theme::ACCENT_SOFT,
                );
            }
            if i == self.file_selection {
                canvas.rect(x + 8, y + 58 + row as i32 * 24, 3, 24, theme::ACCENT);
            }
            if i == self.file_selection
                && self.file_focus.focus_visible()
                && self
                    .file_focus
                    .focused()
                    .is_some_and(|id| id.scope == 2 && id.local == 1)
            {
                canvas.rect(x + 8, y + 58 + row as i32 * 24, 3, 24, theme::ACCENT);
            }
            self.panel_line(
                canvas,
                x,
                y,
                row,
                file.path.as_str(),
                columns,
                height,
                theme::TEXT,
            );
        }
        if height > 100 {
            if let Some(stats) = self.file_query.stats() {
                let mut summary = Line::new();
                use core::fmt::Write;
                let _ = write!(
                    summary,
                    "{} of {} stored files / boot document separate",
                    stats.matched, stats.total
                );
                draw_limited(
                    canvas,
                    x + 12,
                    y + height - 68,
                    summary.as_str(),
                    columns,
                    theme::TEXT,
                );
            }
            draw_limited(
                canvas,
                x + 12,
                y + height - 48,
                "Tab controls / N new / R rename / C copy / / filter",
                columns,
                theme::MUTED,
            );
        }
        if !self.file_status.as_str().is_empty() {
            draw_limited(
                canvas,
                x + 12,
                y + height - 24,
                self.file_status.as_str(),
                columns,
                theme::MUTED,
            );
        }
    }
}
fn wrap_text(mut text: &str, columns: usize, mut emit: impl FnMut(&str)) {
    let columns = columns.max(1);
    if text.is_empty() {
        emit("");
        return;
    }
    while !text.is_empty() {
        let mut end = text.len().min(columns);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            end = text.char_indices().nth(1).map_or(text.len(), |(i, _)| i);
        }
        if end < text.len() {
            if let Some(space) = text[..end].rfind(' ') {
                if space >= columns / 2 && space > 0 {
                    end = space;
                }
            }
        }
        emit(&text[..end]);
        text = text[end..].trim_start_matches(' ');
    }
}
fn ui_scale(canvas: &Canvas<'_>) -> i32 {
    if canvas.width() >= 800 {
        2
    } else {
        1
    }
}
fn app_icon(app: App) -> theme::Icon {
    match app {
        App::Terminal => theme::Icon::Terminal,
        App::Files => theme::Icon::Files,
        App::Monitor => theme::Icon::Monitor,
        App::Settings => theme::Icon::Settings,
        App::Editor => theme::Icon::Editor,
        App::Calculator => theme::Icon::Calculator,
    }
}

fn draw_limited(canvas: &mut Canvas<'_>, x: i32, y: i32, text: &str, columns: usize, color: u32) {
    // Shell lines are ASCII; take a valid UTF-8 prefix defensively for other callers.
    let end = text
        .char_indices()
        .nth(columns)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    theme::text(canvas, x, y, &text[..end], color, theme::Font::Body);
}
fn text_fit_columns(text: &str, width: i32) -> usize {
    if width <= 0 {
        return 0;
    }
    text.char_indices()
        .take_while(|(start, c)| {
            let end = start + c.len_utf8();
            theme::text_width(&text[..end], theme::Font::Body) <= width
        })
        .count()
}

fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ' ' => [0; 7],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ',' => [0, 0, 0, 0, 4, 4, 8],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        ';' => [0, 12, 12, 0, 4, 4, 8],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '\\' => [16, 8, 8, 4, 2, 2, 1],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '<' => [1, 2, 4, 8, 4, 2, 1],
        '|' => [4, 4, 4, 4, 4, 4, 4],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '!' => [4, 4, 4, 4, 4, 0, 4],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '[' => [14, 8, 8, 8, 8, 8, 14],
        ']' => [14, 2, 2, 2, 2, 2, 14],
        '\'' => [4, 4, 8, 0, 0, 0, 0],
        '"' => [10, 10, 10, 0, 0, 0, 0],
        '#' => [10, 10, 31, 10, 31, 10, 10],
        '*' => [0, 21, 14, 31, 14, 21, 0],
        '%' => [25, 25, 2, 4, 8, 19, 19],
        _ => [31, 17, 21, 21, 21, 17, 31],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_rejects_overflow_short_buffers_and_bad_stride() {
        let mut pixels = [0; 4];
        assert!(matches!(
            Canvas::new(&mut pixels, 0, 2, 2, PixelFormat::Bgr),
            Err(CanvasError::InvalidGeometry)
        ));
        assert!(matches!(
            Canvas::new(&mut pixels, 3, 1, 2, PixelFormat::Bgr),
            Err(CanvasError::InvalidGeometry)
        ));
        assert!(matches!(
            Canvas::new(&mut pixels, 2, 3, 2, PixelFormat::Bgr),
            Err(CanvasError::BufferTooSmall)
        ));
        assert!(matches!(
            Canvas::new(&mut pixels, 2, 3, usize::MAX, PixelFormat::Bgr),
            Err(CanvasError::InvalidGeometry)
        ));
    }
    #[test]
    fn clipping_preserves_padding_and_guards() {
        let mut pixels = [0xdeadbeef; 14];
        let mut c = Canvas::new(&mut pixels[1..13], 3, 3, 4, PixelFormat::Bgr).unwrap();
        c.rect(-2, -2, 6, 6, 0x123456);
        c.rect(i32::MAX, i32::MAX, i32::MAX, i32::MAX, 0);
        c.pixel(-1, 0, 0);
        c.text(i32::MAX, i32::MAX, "NEXT", 0, 16);
        for i in [0, 4, 8, 12, 13] {
            assert_eq!(pixels[i], 0xdeadbeef);
        }
        for i in [1, 2, 3, 5, 6, 7, 9, 10, 11] {
            assert_eq!(pixels[i], 0x123456);
        }
    }
    #[test]
    fn pixel_formats_match_uefi_byte_order() {
        let mut pixels = [0; 1];
        Canvas::new(&mut pixels, 1, 1, 1, PixelFormat::Rgb)
            .unwrap()
            .pixel(0, 0, 0x123456);
        assert_eq!(pixels[0], 0x563412);
        Canvas::new(&mut pixels, 1, 1, 1, PixelFormat::Bgr)
            .unwrap()
            .pixel(0, 0, 0x123456);
        assert_eq!(pixels[0], 0x123456);
    }
    fn command(shell: &mut Shell, text: &str) {
        for c in text.chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Enter);
    }
    #[test]
    fn commands_and_keyboard_state_work() {
        let mut s = Shell::new();
        s.set_status("Boot services exited");
        command(&mut s, "status");
        assert_eq!(
            s.history_line(s.history_len() - 1),
            Some("Boot services exited")
        );
        command(&mut s, "clear");
        assert_eq!(s.history_len(), 0);
        s.on_key(Key::Character('x'));
        s.on_key(Key::Backspace);
        s.on_key(Key::Backspace);
        assert_eq!(s.input(), "");
        s.on_key(Key::Escape);
        assert!(s.launcher_open());
        s.on_key(Key::Character('a'));
        assert_eq!(s.input(), "");
        s.on_key(Key::Enter);
        assert!(!s.launcher_open());
        command(&mut s, "help");
        assert!(s.history_len() > 3);
        command(&mut s, "unknown");
        assert!(s
            .history_line(s.history_len() - 1)
            .unwrap()
            .starts_with("Unknown"));
    }
    #[test]
    fn buffers_are_bounded_and_history_scrolls() {
        let mut s = Shell::new();
        for _ in 0..200 {
            s.on_key(Key::Character('x'));
        }
        assert_eq!(s.input().len(), LINE_CAPACITY);
        s.on_key(Key::Character('é'));
        assert_eq!(s.input().len(), LINE_CAPACITY);
        s.on_key(Key::Enter);
        for _ in 0..100 {
            command(&mut s, "help");
        }
        assert_eq!(s.history_len(), HISTORY_CAPACITY);
    }
    #[test]
    fn tiny_displays_do_not_panic() {
        let mut pixels = [0; 4];
        let mut c = Canvas::new(&mut pixels, 2, 2, 2, PixelFormat::Bgr).unwrap();
        let mut s = Shell::new();
        s.render(&mut c);
        s.on_key(Key::Escape);
        s.render(&mut c);
    }
    #[test]
    fn boot_file_content_requires_a_successful_binding() {
        let mut shell = Shell::new();
        command(&mut shell, "cat /NEXT.TXT");
        assert_eq!(
            shell.history_line(shell.history_len() - 1),
            Some("Boot volume file unavailable.")
        );
        shell.set_boot_file("  Actual disk note\n");
        command(&mut shell, "cat /NEXT.TXT");
        assert_eq!(
            shell.history_line(shell.history_len() - 1),
            Some("Actual disk note")
        );
    }
    #[test]
    fn launcher_opens_panels_and_cycles_without_leaking_text() {
        let mut shell = Shell::new();
        shell.on_key(Key::Escape);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Character('x'));
        assert_eq!(shell.input(), "");
        assert_eq!(shell.search_query(), Some("x"));
        shell.on_key(Key::Escape); // cancel search, return to Home actions
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        assert_eq!(shell.desktop().focused(), Some(App::Files));
        shell.on_key(Key::WindowNext);
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));
        shell.on_key(Key::Escape);
        for _ in 0..7 {
            shell.on_key(Key::Tab);
        }
        shell.on_key(Key::Enter);
        assert_eq!(shell.desktop().editing(), Some(EditMode::Resize));
        let original = shell.desktop().window(App::Terminal).bounds;
        shell.on_key(Key::Left);
        shell.on_key(Key::Escape);
        assert_eq!(shell.desktop().window(App::Terminal).bounds, original);
    }
    #[test]
    fn calculator_launches_from_home_search_and_taskbar_and_closes_cleanly() {
        let mut shell = Shell::new();
        shell.set_viewport(1280, 800);

        shell.on_key(Key::Menu);
        let pinned = shell.home_action_bounds(App::Calculator as usize).unwrap();
        shell.on_pointer(Pointer::Down(
            pinned.x + pinned.width / 2,
            pinned.y + pinned.height / 2,
        ));
        assert_eq!(shell.desktop().focused(), Some(App::Calculator));
        assert_eq!(
            shell.desktop().window(App::Calculator).state,
            WindowState::Floating
        );
        for character in "7 * 6".chars() {
            shell.on_key(Key::Character(character));
        }
        let bounds = shell.calculator_bounds();
        let calculate = calculator_app::calculate_bounds(bounds);
        shell.on_pointer(Pointer::Down(
            calculate.x + calculate.width / 2,
            calculate.y + calculate.height / 2,
        ));
        assert_eq!(shell.calculator.result(), Some(Ok(42)));

        let mut snapshot = accessibility::Snapshot::empty();
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert!(snapshot.complete);
        assert_eq!(snapshot.nodes()[0].role, accessibility::Role::Window);
        assert_eq!(snapshot.nodes()[0].name(), "Next Calculator");
        assert!(snapshot
            .nodes()
            .iter()
            .any(|node| node.name() == "Calculation result" && node.value() == "42"));
        assert!(shell.take_accessibility_event().is_some());

        shell.on_key(Key::CloseWindow);
        assert_eq!(
            shell.desktop().window(App::Calculator).state,
            WindowState::Closed
        );
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));

        shell.on_key(Key::OpenSearch);
        for character in "calculator".chars() {
            shell.on_key(Key::Character(character));
        }
        assert_eq!(shell.search_count(), 1);
        shell.on_key(Key::Enter);
        assert_eq!(shell.desktop().focused(), Some(App::Calculator));
        shell.on_key(Key::CloseWindow);
        assert_eq!(
            shell.desktop().window(App::Calculator).state,
            WindowState::Closed
        );

        let calculator = shell
            .taskbar_item_bounds(App::Calculator as usize + 1)
            .unwrap();
        shell.on_pointer(Pointer::Down(
            calculator.x + calculator.width / 2,
            calculator.y + calculator.height / 2,
        ));
        assert_eq!(shell.desktop().focused(), Some(App::Calculator));
    }
    #[test]
    fn supplied_command_and_panel_data_are_bounded_and_actual() {
        let mut shell = Shell::new();
        for c in "ps".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.set_command_response("PID 1 nextd sleeping");
        shell.on_key(Key::Enter);
        assert_eq!(
            shell.history_line(shell.history_len() - 1),
            Some("PID 1 nextd sleeping")
        );
        command(&mut shell, "nonsense");
        assert!(shell
            .history_line(shell.history_len() - 1)
            .unwrap()
            .starts_with("Unknown"));
        assert!(!shell.set_file(FILE_CAPACITY, "invalid", "invalid"));
        assert!(!shell.set_monitor_line(8, "invalid"));
        shell.set_file(1, "/actual.txt", "real content");
        shell.open_app(App::Files);
        shell.on_key(Key::Down);
        shell.on_key(Key::Enter);
        assert_eq!(shell.file_selection, 1);
        assert!(matches!(
            shell.take_editor_action(),
            Some(EditorAction::Open { .. })
        ));
        shell.open_app(App::Files);
        shell.file_detail = true;
        shell.on_key(Key::Escape);
        assert!(!shell.file_detail);
        assert!(!shell.launcher_open());
        shell.on_terminal_key(Key::Character('h'));
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));
        assert_eq!(shell.input(), "h");
    }
    #[test]
    fn pointer_home_and_taskbar_restore_minimized_window() {
        let mut shell = Shell::new();
        shell.window_action(Action::Minimize);
        assert_eq!(shell.desktop().focused(), None);
        let b = shell.taskbar_item_bounds(1).unwrap();
        shell.on_pointer(Pointer::Down(b.x + b.width / 2, b.y + b.height / 2));
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));
        let b = shell.taskbar_item_bounds(0).unwrap();
        shell.on_pointer(Pointer::Down(b.x + b.width / 2, b.y + b.height / 2));
        assert!(shell.launcher_open());
        let b = shell.home_action_bounds(1).unwrap();
        shell.on_pointer(Pointer::Down(b.x + b.width / 2, b.y + b.height / 2));
        assert_eq!(shell.desktop().focused(), Some(App::Files));
    }
    #[test]
    fn all_panels_render_and_resize_with_pixel_guards() {
        let mut storage = [0xdeadbeef; 4098];
        let mut shell = Shell::new();
        shell.set_boot_file("actual disk note");
        shell.set_monitor_line(0, "PID 1 sleeping");
        shell.set_system_line(0, "Network driver unavailable");
        for app in App::ALL {
            shell.open_app(app);
            shell.set_viewport(64, 64);
            let mut canvas =
                Canvas::new(&mut storage[1..4097], 64, 64, 64, PixelFormat::Bgr).unwrap();
            shell.render(&mut canvas);
            shell.window_action(Action::Maximize);
            shell.render(&mut canvas);
            shell.window_action(Action::Restore);
            shell.window_action(Action::Minimize);
        }
        assert_eq!(storage[0], 0xdeadbeef);
        assert_eq!(storage[4097], 0xdeadbeef);
        assert!(storage[1..4097].iter().any(|p| *p != 0x101114));
    }
    #[test]
    fn clearing_terminal_preserves_focus_without_typing_commands() {
        let mut shell = Shell::new();
        shell.on_key(Key::Character('x'));
        shell.open_app(App::Monitor);
        shell.set_command_response("pending response");
        shell.clear_terminal();
        assert_eq!(shell.input(), "");
        assert_eq!(shell.history_len(), 0);
        assert_eq!(shell.desktop().focused(), Some(App::Monitor));
        assert!(shell.response.is_none());
    }
    #[test]
    fn all_nine_file_bindings_can_be_selected_and_removed() {
        let mut shell = Shell::new();
        shell.set_boot_file("boot note");
        for index in 1..FILE_CAPACITY {
            assert!(shell.set_file(index, "/stored", "actual content"));
        }
        shell.open_app(App::Files);
        shell.on_key(Key::Up);
        assert_eq!(shell.file_selection, 8);
        shell.on_key(Key::Enter);
        assert!(matches!(
            shell.take_editor_action(),
            Some(EditorAction::Open { .. })
        ));
        assert!(shell.clear_file(8));
        assert_eq!(shell.file_selection, 0);
        assert!(!shell.file_detail);
        assert!(shell.files[0].available);
        assert!(!shell.files[8].available);
        assert!(!shell.clear_file(FILE_CAPACITY));
        for index in 1..FILE_CAPACITY {
            shell.clear_file(index);
        }
        shell.on_key(Key::Down);
        assert_eq!(shell.file_selection, 0);
        assert!(shell.files[0].available);
    }
    #[test]
    fn presentation_preserves_front_guards_padding_and_rejects_before_write() {
        let mut back = [0; 11];
        let mut canvas = Canvas::new(&mut back, 3, 3, 4, PixelFormat::Rgb).unwrap();
        canvas.clear(0x123456);
        let mut short = [0xdeadbeef; 10];
        assert_eq!(
            canvas.present_to(&mut short),
            Err(CanvasError::BufferTooSmall)
        );
        assert!(short.iter().all(|p| *p == 0xdeadbeef));
        let mut front = [0xdeadbeef; 13];
        canvas.present_to(&mut front[1..12]).unwrap();
        for index in [0, 4, 8, 12] {
            assert_eq!(front[index], 0xdeadbeef);
        }
        for index in [1, 2, 3, 5, 6, 7, 9, 10, 11] {
            assert_eq!(front[index], 0x563412);
        }
    }
    #[test]
    fn complete_frame_keeps_header_title_controls_and_body_in_bounds() {
        extern crate std;
        let mut pixels = std::vec![0u32;1280*800];
        let mut shell = Shell::new();
        shell.set_viewport(1280, 800);
        let b = shell.desktop().window(App::Terminal).bounds;
        let mut canvas = Canvas::new(&mut pixels, 1280, 800, 1280, PixelFormat::Bgr).unwrap();
        shell.render(&mut canvas);
        let at = |x: i32, y: i32| pixels[y as usize * 1280 + x as usize];
        // Rounded surface, title separator, client and bottom border must coexist.
        assert_eq!(at(b.x + b.width / 2, b.y), theme::BORDER);
        assert_eq!(at(b.x + 1, b.y + 48), theme::SURFACE);
        assert_eq!(at(b.x + 120, b.y + 40), theme::BORDER);
        assert_eq!(at(b.x + b.width / 2, b.y + b.height - 1), theme::BORDER);
        let coverage = |x: i32, y: i32, width: i32, height: i32, color: u32| {
            (y..y + height)
                .flat_map(|yy| (x..x + width).map(move |xx| (xx, yy)))
                .filter(|&(xx, yy)| at(xx, yy) == color)
                .count()
        };
        assert!(
            coverage(b.x + 42, b.y + 10, 100, 24, theme::TEXT) > 20,
            "AA title is present"
        );
        assert!(
            coverage(b.x + b.width - 24, b.y + 13, 16, 16, theme::MUTED) > 8,
            "close icon is present"
        );
        assert!(
            coverage(24, 22, 26, 26, 0xffffff) > 20,
            "desktop brand survives"
        );
        let bar = shell.taskbar_bounds();
        assert_eq!(at(bar.x + bar.width / 2, bar.y), theme::BORDER);
        assert!(bar.y > b.y + b.height, "window client stops before taskbar");
    }
    #[test]
    fn pointer_cursor_is_drawn_above_frontmost_window() {
        extern crate std;
        let mut pixels = std::vec![0u32;1280*800];
        let mut shell = Shell::new();
        shell.set_viewport(1280, 800);
        shell.on_pointer(Pointer::Move(100, 200));
        let mut canvas = Canvas::new(&mut pixels, 1280, 800, 1280, PixelFormat::Bgr).unwrap();
        shell.render(&mut canvas);
        assert_eq!(pixels[200 * 1280 + 100], 0x08090c);
        assert_eq!(pixels[204 * 1280 + 101], 0xffffff);
    }
    #[test]
    fn calculator_terminal_command_runs_math_and_reports_errors() {
        let mut shell = Shell::new();
        command(&mut shell, "calc (2 + 3) * 4");
        assert_eq!(shell.history_line(shell.history_len() - 1), Some("= 20"));
        command(&mut shell, "calc 1 / 0");
        assert_eq!(
            shell.history_line(shell.history_len() - 1),
            Some("Cannot divide by zero.")
        );
    }
    #[test]
    fn external_multiline_responses_strip_crlf_without_box_glyphs() {
        let mut shell = Shell::new();
        for c in "ps".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.set_command_response("PID=15\r\nPID=16\n");
        shell.on_key(Key::Enter);
        assert_eq!(shell.history_line(shell.history_len() - 2), Some("PID=15"));
        assert_eq!(shell.history_line(shell.history_len() - 1), Some("PID=16"));
        for line in shell.history.iter().take(shell.count) {
            assert!(!line.as_str().bytes().any(|b| b.is_ascii_control()));
        }
    }
    #[test]
    fn terminal_wrap_preserves_long_words_and_separates_at_spaces() {
        let mut parts = [Line::new(); 4];
        let mut count = 0;
        wrap_text("status has a verylongword", 10, |text| {
            parts[count].set(text);
            count += 1;
        });
        assert_eq!(parts[0].as_str(), "status");
        assert_eq!(parts[1].as_str(), "has a");
        assert_eq!(parts[2].as_str(), "verylongwo");
        assert_eq!(parts[3].as_str(), "rd");
    }
    #[test]
    fn editor_open_save_callbacks_and_failed_save_keep_actual_data() {
        let mut shell = Shell::new();
        shell.set_file(1, "/native/doc", "preview");
        shell.open_app(App::Files);
        shell.on_key(Key::Down);
        shell.on_key(Key::Enter);
        let Some(EditorAction::Open { path }) = shell.take_editor_action() else {
            panic!("expected actual file open");
        };
        assert_eq!(path.as_str(), "/native/doc");
        shell.load_document(path.as_str(), "aé\ntext").unwrap();
        shell.on_key(Key::End);
        shell.on_key(Key::Character('!'));
        assert_eq!(shell.editor().text(), "aé!\ntext");
        assert!(shell.editor().dirty());
        shell.request_editor_save();
        let Some(EditorAction::Save {
            path,
            data,
            len,
            revision,
        }) = shell.take_editor_action()
        else {
            panic!("expected actual file save");
        };
        assert_eq!(path.as_str(), "/native/doc");
        assert_eq!(core::str::from_utf8(&data[..len]).unwrap(), "aé!\ntext");
        shell.editor_save_result(revision, Err("Disk write failed; edits kept."));
        assert!(shell.editor().dirty());
        assert_eq!(shell.editor().text(), "aé!\ntext");
        shell.editor_save_result(revision, Ok(()));
        assert!(!shell.editor().dirty());
    }
    #[test]
    fn editor_unsaved_dialog_cancels_discards_or_saves_before_replace() {
        let mut shell = Shell::new();
        shell.load_document("/native/untitled", "").unwrap();
        shell.on_key(Key::Character('x'));
        shell.request_editor_open("/native/other").unwrap();
        assert!(shell.take_editor_action().is_none());
        shell.on_key(Key::Enter);
        assert_eq!(shell.editor().text(), "x");
        assert!(shell.editor().dirty()); // Default cancel.
        shell.window_action(Action::Close);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        let Some(EditorAction::Save { revision, .. }) = shell.take_editor_action() else {
            panic!("expected save before close");
        };
        shell.editor_save_result(revision, Err("No space"));
        assert!(shell.editor().dirty());
        assert_ne!(
            shell.desktop().window(App::Editor).state,
            WindowState::Closed
        );
        shell.on_key(Key::Escape);
        shell.request_editor_open("/native/other").unwrap();
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        assert!(matches!(
            shell.take_editor_action(),
            Some(EditorAction::Open { .. })
        ));
        assert!(!shell.editor().dirty());
        assert_eq!(shell.editor().text(), "");
    }
    #[test]
    fn home_editor_and_console_edit_request_authoritative_files() {
        let mut shell = Shell::new();
        shell.on_key(Key::Menu);
        for _ in 0..4 {
            shell.on_key(Key::Down);
        }
        shell.on_key(Key::Enter);
        let Some(EditorAction::Open { path }) = shell.take_editor_action() else {
            panic!("Home must request saved content before editing");
        };
        assert_eq!(path.as_str(), "/native/untitled");
        shell
            .load_document(path.as_str(), "existing content")
            .unwrap();
        for c in "edit /native/second".chars() {
            shell.on_terminal_key(Key::Character(c));
        }
        shell.on_terminal_key(Key::Enter);
        let Some(EditorAction::Open { path }) = shell.take_editor_action() else {
            panic!("console open");
        };
        assert_eq!(path.as_str(), "/native/second");
        assert_eq!(shell.editor().text(), "existing content"); // callback owns replacement
    }
    #[test]
    fn failed_open_and_conflicting_save_preserve_edits_until_explicit_discard() {
        let mut shell = Shell::new();
        shell.load_document("/native/doc", "saved").unwrap();
        shell.on_key(Key::End);
        shell.on_key(Key::Character('!'));
        shell.request_editor_open("/native/other").unwrap();
        assert_eq!(
            shell.load_document("/native/other", "replacement"),
            Err(EditorError::UnsavedChanges)
        );
        shell.on_key(Key::Right);
        shell.on_key(Key::Right);
        shell.on_key(Key::Enter);
        let Some(EditorAction::Save { revision, .. }) = shell.take_editor_action() else {
            panic!("save before replace");
        };
        shell.editor_save_result(
            revision,
            Err("File changed externally; reopen before overwriting."),
        );
        assert_eq!(shell.editor().text(), "saved!");
        assert!(shell.editor().dirty());
        assert!(shell.take_editor_action().is_none());
        shell.on_key(Key::Escape);
        shell.editor_load_failed("File read failed.");
        assert_eq!(shell.editor().text(), "saved!");
        shell.request_editor_open("/native/other").unwrap();
        shell.on_key(Key::Right); // explicit discard
        shell.on_key(Key::Enter);
        assert!(matches!(
            shell.take_editor_action(),
            Some(EditorAction::Open { .. })
        ));
        shell
            .load_document("/native/other", "actual replacement")
            .unwrap();
        assert_eq!(shell.editor().text(), "actual replacement");
        assert!(!shell.editor().dirty());
    }
    #[test]
    fn stale_save_completion_keeps_dirty_document_and_pending_replacement() {
        let mut shell = Shell::new();
        shell.load_document("/native/doc", "saved").unwrap();
        shell.on_key(Key::Character('x'));
        shell.request_editor_save();
        let Some(EditorAction::Save { revision, .. }) = shell.take_editor_action() else {
            panic!("save");
        };
        shell.on_key(Key::Character('y'));
        shell.request_editor_open("/native/next").unwrap();
        shell.editor_save_result(revision, Ok(()));
        assert!(shell.editor().dirty());
        assert!(shell.take_editor_action().is_none());
        assert_eq!(shell.editor().text(), "xysaved");
    }
    #[test]
    fn editor_cannot_save_without_an_authoritative_load_baseline() {
        let mut shell = Shell::new();
        shell.request_editor_save();
        assert!(shell.take_editor_action().is_none());
        assert!(shell.editor_status.as_str().contains("Open the file first"));
        shell.load_document("/native/untitled", "").unwrap();
        shell.request_editor_save();
        assert!(matches!(
            shell.take_editor_action(),
            Some(EditorAction::Save { .. })
        ));
    }
    #[test]
    fn constant_shell_initializes_defaults_in_place() {
        const INITIAL: Shell = Shell::empty();
        let mut shell = INITIAL;
        assert_eq!(shell.count, 0);
        assert_eq!(shell.editor().path(), "/native/untitled");
        assert_eq!(shell.editor().text(), "");
        shell.initialize_defaults();
        assert_eq!(shell.count, 2);
        assert_eq!(
            shell.history[0].as_str(),
            "Welcome to Next Terminal. Type help to get started."
        );
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));
        assert!(shell.take_editor_action().is_none());
    }
    #[test]
    fn search_opens_real_apps_and_current_bound_files_without_terminal_leak() {
        let mut shell = Shell::new();
        shell.on_key(Key::Menu);
        for c in "mOnItOr".chars() {
            shell.on_key(Key::Character(c));
        }
        assert_eq!(shell.search_count(), 1);
        assert_eq!(shell.input(), "");
        shell.on_key(Key::Enter);
        assert_eq!(shell.desktop().focused(), Some(App::Monitor));
        shell.set_file(8, "/native/real-note", "preview only");
        shell.on_key(Key::Menu);
        for c in "real-note".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Enter);
        let Some(EditorAction::Open { path }) = shell.take_editor_action() else {
            panic!("real file callback");
        };
        assert_eq!(path.as_str(), "/native/real-note");
        assert_eq!(shell.editor().text(), "");
    }
    #[test]
    fn search_bounds_no_results_cancel_and_removed_files_stay_current() {
        let mut shell = Shell::new();
        shell.set_file(1, "/native/removed", "");
        shell.on_key(Key::Menu);
        for c in "removed".chars() {
            shell.on_key(Key::Character(c));
        }
        assert_eq!(shell.search_count(), 1);
        shell.clear_file(1);
        assert_eq!(shell.search_count(), 0);
        shell.on_key(Key::Enter);
        assert!(shell.take_editor_action().is_none());
        assert!(shell.launcher_open());
        for _ in 0..64 {
            shell.on_key(Key::Character('z'));
        }
        assert_eq!(shell.search_query().unwrap().len(), 31);
        shell.on_key(Key::Backspace);
        assert_eq!(shell.search_query().unwrap().len(), 30);
        shell.on_key(Key::Escape);
        assert!(shell.search_query().is_none());
        assert!(shell.launcher_open());
        shell.on_key(Key::Escape);
        assert!(!shell.launcher_open());
    }
    #[test]
    fn search_mouse_scroll_and_dirty_file_open_follow_same_backend_contract() {
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        for i in 0..9 {
            shell.set_file(i, "/native/current", "");
        }
        let bar = shell.taskbar_item_bounds(0).unwrap();
        shell.on_pointer(Pointer::Down(bar.x + 20, bar.y + 20));
        let home = shell.home_bounds();
        shell.on_pointer(Pointer::Down(home.x + 50, home.y + 70));
        assert_eq!(shell.search_query(), Some(""));
        assert_eq!(shell.search_count(), 15);
        for _ in 0..14 {
            shell.on_key(Key::Down);
        }
        assert_eq!(shell.selection, 14);
        shell.on_pointer(Pointer::Down(home.x + 50, home.y + 116 + 8 * 32 + 8)); // ninth visible result at end
        let Some(EditorAction::Open { path }) = shell.take_editor_action() else {
            panic!("last file accessible");
        };
        assert_eq!(path.as_str(), "/native/current");
        shell.load_document(path.as_str(), "saved").unwrap();
        shell.on_key(Key::Character('!'));
        shell.on_key(Key::Menu);
        for c in "current".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Enter);
        assert!(shell.take_editor_action().is_none());
        assert!(shell.editor().dirty());
        shell.on_key(Key::Enter); // safe default cancel
        assert_eq!(shell.editor().text(), "!saved");
    }
    #[test]
    fn file_portal_defaults_to_deny_and_filters_actual_native_bindings() {
        let mut shell = Shell::empty();
        shell.set_file(0, "/NEXT.TXT", "boot");
        shell.set_file(1, "/native/a", "real");
        shell.set_file(2, "/system/fake", "excluded");
        shell.set_file(3, "/native/../bad", "excluded");
        shell.show_file_portal(10, 44, "original-demo", false);
        assert_eq!(shell.portal_count(), 1);
        assert_eq!(shell.portal_file(0).unwrap().path.as_str(), "/native/a");
        shell.on_key(Key::Enter);
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Deny { request: 10 })
        );
        assert!(!shell.file_portal_active());
        shell.show_file_portal(11, 44, "original-demo", true);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Approve {
                request: 11,
                path: EditorPath::new("/native/a").unwrap()
            })
        );
    }
    #[test]
    fn file_portal_modal_preserves_search_editor_dirty_and_confirmation() {
        let mut shell = Shell::new();
        shell.load_document("/native/doc", "saved").unwrap();
        shell.on_key(Key::Character('!'));
        shell.on_key(Key::Menu);
        shell.on_key(Key::Character('m'));
        let query = shell.search_input;
        shell.show_file_portal(20, 45, "requester", false);
        shell.on_key(Key::Character('x'));
        shell.on_terminal_key(Key::Character('y'));
        shell.request_editor_save();
        shell.on_pointer(Pointer::Down(0, 0)); // outside cannot act on desktop
        assert_eq!(shell.search_input.as_str(), query.as_str());
        assert_eq!(shell.editor().text(), "!saved");
        assert!(shell.editor().dirty());
        assert!(shell.take_editor_action().is_none());
        assert!(shell.file_portal_active());
        shell.on_key(Key::Escape);
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Deny { request: 20 })
        );
        assert_eq!(shell.search_query(), Some("m"));
        shell.on_key(Key::Escape);
        shell.on_key(Key::Escape);
        shell.request_editor_open("/native/other").unwrap();
        assert!(shell.editor_deferred.is_some());
        shell.show_file_portal(21, 45, "requester", false);
        shell.on_key(Key::Enter);
        assert!(shell.editor_deferred.is_some());
        assert_eq!(shell.editor_choice, 0);
        assert_eq!(shell.editor().text(), "!saved");
    }
    #[test]
    fn file_portal_dismiss_is_request_scoped_and_clears_stale_actions() {
        let mut shell = Shell::empty();
        shell.show_file_portal(30, 2, "app", false);
        shell.dismiss_file_portal(29);
        assert!(shell.file_portal_active());
        shell.on_key(Key::Enter);
        shell.dismiss_file_portal(30);
        assert!(shell.take_file_portal_action().is_none());
        shell.show_file_portal(31, 3, "other", true);
        shell.dismiss_file_portal(30);
        assert!(shell.file_portal_active());
        shell.dismiss_file_portal(31);
        assert!(!shell.file_portal_active());
        assert!(shell.take_file_portal_action().is_none());
    }
    #[test]
    fn file_portal_mouse_selection_and_buttons_share_keyboard_policy() {
        let mut shell = Shell::empty();
        shell.set_viewport(1024, 768);
        shell.set_file(1, "/native/one", "");
        shell.set_file(2, "/native/two", "");
        shell.show_file_portal(40, 5, "app", true);
        let b = shell.file_portal_bounds().unwrap();
        shell.on_pointer(Pointer::Down(b.x + 20, b.y + 116 + 24 + 2));
        shell.show_file_portal(40, 5, "app", true); // refresh must preserve selection
        assert_eq!(shell.file_portal.unwrap().selection, 1);
        shell.on_pointer(Pointer::Down(b.x + 140, b.y + b.height - 50));
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Approve {
                request: 40,
                path: EditorPath::new("/native/two").unwrap()
            })
        );
        shell.show_file_portal(41, 5, "app", false);
        shell.on_pointer(Pointer::Down(b.x + 25, b.y + b.height - 50));
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Deny { request: 41 })
        );
    }
    #[test]
    fn file_portal_empty_removed_files_cannot_grant_and_render_keeps_guards() {
        let mut shell = Shell::empty();
        shell.set_viewport(1024, 768);
        shell.set_file(1, "/native/remove", "");
        shell.show_file_portal(50, 6, "app", false);
        shell.clear_file(1);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        assert!(shell.file_portal_active());
        assert!(shell.take_file_portal_action().is_none());
        extern crate std;
        let mut pixels = std::vec![0x12345678;1024*768+2];
        {
            let mut canvas = Canvas::new(
                &mut pixels[1..1024 * 768 + 1],
                1024,
                768,
                1024,
                PixelFormat::Rgb,
            )
            .unwrap();
            shell.render(&mut canvas);
        }
        assert_eq!(pixels[0], 0x12345678);
        assert_eq!(pixels[1024 * 768 + 1], 0x12345678);
        let b = shell.file_portal_bounds().unwrap();
        assert_eq!(
            pixels[1 + b.y as usize * 1024 + (b.x + b.width / 2) as usize],
            0xe3d5cb
        ); // trusted border on top
        shell.on_key(Key::Escape);
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Deny { request: 50 })
        );
    }
    #[test]
    fn keyboard_snap_modal_does_not_type_or_execute_and_reports_real_geometry() {
        let mut shell = Shell::new();
        shell.on_key(Key::Character('q'));
        let original = shell.desktop().window(App::Terminal).bounds;
        shell.on_key(Key::SnapLeft);
        assert_eq!(
            shell.take_snap_event().unwrap().kind,
            SnapEventKind::Preview
        );
        shell.on_key(Key::Character('x'));
        assert_eq!(shell.input(), "q");
        let history = shell.history_len();
        shell.on_key(Key::Escape);
        assert_eq!(shell.desktop().window(App::Terminal).bounds, original);
        assert_eq!(
            shell.take_snap_event().unwrap().kind,
            SnapEventKind::Cancelled
        );
        shell.on_key(Key::SnapRight);
        let proposed = shell.snap_preview().unwrap().1;
        shell.on_key(Key::Enter);
        assert_eq!(shell.history_len(), history);
        assert_eq!(shell.input(), "q");
        assert_eq!(shell.desktop().window(App::Terminal).bounds, proposed);
        assert_eq!(
            shell.take_snap_event().unwrap().kind,
            SnapEventKind::Applied
        );
        shell.on_key(Key::SnapRestore);
        assert_eq!(shell.desktop().window(App::Terminal).bounds, original);
        assert_eq!(
            shell.take_snap_event().unwrap().kind,
            SnapEventKind::Restored
        );
        shell.show_file_portal(70, 2, "app", false);
        shell.on_key(Key::SnapLeft);
        assert!(shell.snap_preview().is_none());
    }
    #[test]
    fn pointer_snap_preview_pixels_cancel_and_release_share_apply_path() {
        extern crate std;
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        let original = shell.desktop().window(App::Terminal).bounds;
        shell.on_pointer(Pointer::Down(original.x + 100, original.y + 10));
        shell.on_pointer(Pointer::Move(8, 200));
        let preview = shell.snap_preview().unwrap().1;
        let mut pixels = std::vec![0;1024*768];
        let mut canvas = Canvas::new(&mut pixels, 1024, 768, 1024, PixelFormat::Bgr).unwrap();
        shell.render(&mut canvas);
        assert_eq!(
            pixels[preview.y as usize * 1024 + preview.x as usize],
            0x7c9cff
        );
        shell.on_key(Key::Escape);
        assert_eq!(shell.desktop().window(App::Terminal).bounds, original);
        shell.on_pointer(Pointer::Down(original.x + 100, original.y + 10));
        shell.on_pointer(Pointer::Move(1015, 200));
        shell.on_pointer(Pointer::Up);
        assert_eq!(
            shell.take_snap_event().unwrap().kind,
            SnapEventKind::Applied
        );
        assert_eq!(
            shell.desktop().snapped_zone(App::Terminal),
            Some(snap::Zone::Right)
        );
    }
    #[test]
    fn authenticated_full_package_id_wraps_without_losing_unique_suffix() {
        let mut shell = Shell::empty();
        let package = "org.next.development.native.portal-probe.unique-authenticated-id";
        assert_eq!(package.len(), 64);
        shell.show_file_portal(80, 123, package, false);
        assert_eq!(shell.file_portal.unwrap().package.as_str(), package);
        let b = shell.file_portal_bounds().unwrap();
        assert_eq!(shell.portal_identity_rows(b), 2);
        assert_eq!(shell.portal_list_top(b), 140);
    }
    fn actual_notification(shell: &mut Shell, pid: u64) -> u64 {
        shell
            .publish_notification(
                notifications::Source::Process { pid },
                "org.next.original",
                "Process exited",
                "Actual native exit status: 47",
                notifications::Priority::Info,
                1,
            )
            .unwrap()
            .id
    }
    #[test]
    fn notification_center_read_dismiss_and_dnd_are_real_bounded_state() {
        let mut shell = Shell::new();
        shell.open_app(App::Files);
        let id = actual_notification(&mut shell, 10);
        assert_eq!(shell.desktop().focused(), Some(App::Files));
        assert_eq!(shell.notifications().unread(), 1);
        shell.on_key(Key::Notifications);
        shell.on_key(Key::Enter);
        assert!(shell.notifications().get(id).unwrap().read);
        assert_eq!(shell.notifications().unread(), 0);
        for _ in 0..3 {
            shell.on_key(Key::Tab);
        }
        shell.on_key(Key::Enter);
        assert!(shell.notifications().do_not_disturb());
        let result = shell
            .publish_notification(
                notifications::Source::Process { pid: 11 },
                "org.next.original",
                "Process exited",
                "Exit status: 0",
                notifications::Priority::Info,
                2,
            )
            .unwrap();
        assert!(!result.show_alert);
        assert_eq!(shell.notifications().unread(), 1);
        shell.on_key(Key::Delete); // removes selected read record, no external effect
        assert!(shell.notifications().get(id).is_none());
        assert!(shell.notifications().get(result.id).is_some());
        shell.on_key(Key::Escape);
        assert!(!shell.notifications_open());
        assert_eq!(shell.desktop().focused(), Some(App::Files));
    }
    #[test]
    fn notification_overlay_preserves_terminal_and_trusted_modal_priority() {
        let mut shell = Shell::new();
        shell.on_key(Key::Character('q'));
        let id = actual_notification(&mut shell, 20);
        shell.on_key(Key::Notifications);
        shell.on_key(Key::Character('x'));
        shell.on_terminal_key(Key::Enter);
        assert_eq!(shell.input(), "q");
        assert_eq!(shell.notifications().unread(), 0);
        shell.show_file_portal(90, 20, "org.next.original", false);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Escape);
        assert_eq!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Deny { request: 90 })
        );
        assert!(shell.notifications_open());
        assert!(shell.notifications().get(id).is_some());
        shell.load_document("/native/doc", "saved").unwrap();
        shell.document.insert('!').unwrap();
        shell.request_editor_open("/native/other").unwrap();
        shell.on_terminal_key(Key::Enter); // editor defaultCancel, not notice action
        assert!(shell.editor().dirty());
        assert_eq!(shell.editor().text(), "!saved");
        assert!(shell.editor_deferred.is_none());
    }
    #[test]
    fn notification_mouse_actions_and_toast_expiry_preserve_guard_pixels() {
        extern crate std;
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        let id = actual_notification(&mut shell, 30);
        assert!(shell.notification_toast.is_some());
        shell.set_uptime_ns(4_000_000_001);
        assert!(shell.notification_toast.is_none());
        shell.toggle_notifications();
        let b = shell.notification_bounds();
        shell.on_pointer(Pointer::Down(b.x + 20, b.y + 80));
        assert!(shell.notifications().get(id).unwrap().read);
        let mut pixels = std::vec![0x12345678;1024*768+2];
        {
            let mut canvas = Canvas::new(
                &mut pixels[1..1024 * 768 + 1],
                1024,
                768,
                1024,
                PixelFormat::Bgr,
            )
            .unwrap();
            shell.render(&mut canvas);
        }
        assert_eq!(pixels[0], 0x12345678);
        assert_eq!(pixels[1024 * 768 + 1], 0x12345678);
        assert_eq!(
            pixels[1 + b.y as usize * 1024 + (b.x + b.width / 2) as usize],
            theme::BORDER
        );
        shell.on_pointer(Pointer::Down(b.x + 20, b.y + b.height - 32));
        assert!(shell.notifications().is_empty());
    }
    #[test]
    fn centered_desktop_hit_geometry_matches_render_at_supported_resolutions() {
        extern crate std;
        for (width, height) in [(640, 480), (1024, 768), (1280, 800), (1920, 1080)] {
            let mut shell = Shell::new();
            shell.set_viewport(width, height);
            let b = shell.taskbar_bounds();
            assert_eq!(b.x, (width as i32 - b.width) / 2);
            assert!(
                b.x >= 0
                    && b.y >= 0
                    && b.x + b.width <= width as i32
                    && b.y + b.height <= height as i32
            );
            for index in 0..TASKBAR_ITEM_COUNT {
                let item = shell.taskbar_item_bounds(index).unwrap();
                assert!(item.x >= b.x && item.x + item.width <= b.x + b.width);
                assert!(item.y >= b.y && item.y + item.height <= b.y + b.height);
            }
            let home = shell.taskbar_item_bounds(0).unwrap();
            shell.on_pointer(Pointer::Down(
                home.x + home.width / 2,
                home.y + home.height / 2,
            ));
            assert!(shell.launcher_open());
            let panel = shell.home_bounds();
            assert!(
                panel.x >= 0
                    && panel.y >= 0
                    && panel.x + panel.width <= width as i32
                    && panel.y + panel.height < b.y
            );
            for index in 0..App::ALL.len() {
                let item = shell.home_action_bounds(index).unwrap();
                assert!(item.x >= panel.x && item.x + item.width <= panel.x + panel.width);
                assert!(item.y >= panel.y && item.y + item.height <= panel.y + panel.height);
            }
            let mut pixels = std::vec![0xdeadbeef;width*height+2];
            let end = pixels.len() - 1;
            let mut canvas =
                Canvas::new(&mut pixels[1..end], width, height, width, PixelFormat::Bgr).unwrap();
            shell.render(&mut canvas);
            assert_eq!(pixels[0], 0xdeadbeef);
            assert_eq!(pixels[end], 0xdeadbeef);
        }
    }
    fn directory_listing(
        current: &str,
        folder_id: u64,
        revision: u64,
        rows: &[(&str, files_directory::Kind, u64)],
    ) -> files_directory::Listing {
        let mut entries = [None; files_directory::CAPACITY];
        for (slot, (path, kind, id)) in entries.iter_mut().zip(rows.iter()) {
            *slot = Some(files_directory::Entry {
                path: files_actions::Path::new(path).unwrap(),
                kind: *kind,
                size: 0,
                id: *id,
            });
        }
        files_directory::Listing {
            folder: files_directory::FolderSnapshot {
                path: files_actions::Path::new(current).unwrap(),
                id: folder_id,
                revision,
            },
            entries,
        }
    }
    fn loaded_directory_shell(rows: &[(&str, files_directory::Kind, u64)]) -> Shell {
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        shell.open_app(App::Files);
        let action = shell.take_files_directory_action().unwrap();
        assert!(
            matches!(action.intent, files_directory::Intent::Navigate { path } if path.as_str() == "/native")
        );
        shell
            .files_directory_action_result(
                action.request,
                Ok(directory_listing("/native", 0, 1, rows)),
            )
            .unwrap();
        shell
    }
    #[test]
    fn files_opens_real_directory_rows_and_routes_folder_and_file_activation() {
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        shell.open_app(App::Files);
        let action = shell.take_files_directory_action().unwrap();
        shell
            .files_directory_action_result(
                action.request,
                Ok(directory_listing(
                    "/native",
                    0,
                    1,
                    &[
                        ("/native/docs", files_directory::Kind::Directory, 7),
                        ("/native/readme", files_directory::Kind::File, 0),
                    ],
                )),
            )
            .unwrap();
        shell.on_key(Key::Enter);
        let nested = shell.take_files_directory_action().unwrap();
        assert!(
            matches!(nested.intent, files_directory::Intent::Navigate { path } if path.as_str() == "/native/docs")
        );
        shell
            .files_directory_action_result(
                nested.request,
                Ok(directory_listing(
                    "/native/docs",
                    7,
                    1,
                    &[("/native/docs/guide", files_directory::Kind::File, 0)],
                )),
            )
            .unwrap();
        shell.on_key(Key::Enter);
        assert!(
            matches!(shell.take_editor_action(), Some(EditorAction::Open { path }) if path.as_str() == "/native/docs/guide")
        );
        extern crate std;
        let mut pixels = std::vec![0xdeadbeef; 1024 * 768 + 2];
        let end = pixels.len() - 1;
        {
            let mut canvas =
                Canvas::new(&mut pixels[1..end], 1024, 768, 1024, PixelFormat::Bgr).unwrap();
            shell.render(&mut canvas);
        }
        assert_eq!(pixels[0], 0xdeadbeef);
        assert_eq!(pixels[end], 0xdeadbeef);
    }
    #[test]
    fn files_folder_create_is_cancel_first_and_publishes_only_refreshed_listing() {
        let mut shell = loaded_directory_shell(&[]);
        shell.on_key(Key::Character('n'));
        assert_eq!(
            shell.files_directory().dialog_kind(),
            Some(files_directory::DialogKind::CreateFolder)
        );
        assert!(!shell.files_directory().approves());
        shell.on_key(Key::Enter);
        assert!(!shell.files_directory().dialog_active());
        assert!(shell.take_files_directory_action().is_none());
        shell.on_key(Key::Character('n'));
        for c in "docs".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Tab);
        assert!(shell.files_directory().approves());
        shell.on_key(Key::Enter);
        let action = shell.take_files_directory_action().unwrap();
        assert!(
            matches!(action.intent, files_directory::Intent::CreateFolder { parent, path }
            if parent.id == 0 && path.as_str() == "/native/docs")
        );
        assert_eq!(shell.files_directory().listing().unwrap().entries[0], None);
        shell
            .files_directory_action_result(
                action.request,
                Ok(directory_listing(
                    "/native",
                    0,
                    2,
                    &[("/native/docs", files_directory::Kind::Directory, 9)],
                )),
            )
            .unwrap();
        assert_eq!(shell.files_directory().selected().unwrap().id, 9);
    }
    #[test]
    fn files_move_and_remove_controls_emit_snapshot_bound_directory_actions() {
        let mut shell = loaded_directory_shell(&[
            ("/native/docs", files_directory::Kind::Directory, 7),
            ("/native/archive", files_directory::Kind::Directory, 8),
        ]);
        shell.on_key(Key::Character('m'));
        for c in "archive/docs".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        let moved = shell.take_files_directory_action().unwrap();
        assert!(
            matches!(moved.intent, files_directory::Intent::MoveFolder { source, destination }
            if source.id == 7 && destination.as_str() == "/native/archive/docs")
        );
        shell
            .files_directory_action_result(
                moved.request,
                Ok(directory_listing(
                    "/native",
                    0,
                    2,
                    &[("/native/archive", files_directory::Kind::Directory, 8)],
                )),
            )
            .unwrap();
        shell.on_key(Key::Enter);
        let into_archive = shell.take_files_directory_action().unwrap();
        shell
            .files_directory_action_result(
                into_archive.request,
                Ok(directory_listing(
                    "/native/archive",
                    8,
                    2,
                    &[("/native/archive/docs", files_directory::Kind::Directory, 7)],
                )),
            )
            .unwrap();
        shell.on_key(Key::Delete);
        assert!(!shell.files_directory().approves());
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        let removed = shell.take_files_directory_action().unwrap();
        assert!(
            matches!(removed.intent, files_directory::Intent::RemoveFolder { source }
            if source.id == 7 && source.path.as_str() == "/native/archive/docs")
        );
        shell
            .files_directory_action_result(
                removed.request,
                Ok(directory_listing("/native/archive", 8, 3, &[])),
            )
            .unwrap();
        assert!(shell.files_directory().selected().is_none());
    }
    #[test]
    fn files_directory_toolbar_and_dialog_controls_stay_inside_prism_panel() {
        let mut shell =
            loaded_directory_shell(&[("/native/docs", files_directory::Kind::Directory, 7)]);
        let bounds = shell.file_action_button_bounds(3).unwrap();
        assert!(bounds.width > 0 && bounds.height > 0);
        shell.on_key(Key::Character('m'));
        assert!(shell.files_directory().dialog_active());
        let home = shell.taskbar_item_bounds(0).unwrap();
        shell.on_pointer_packet(home.x + home.width / 2, home.y + home.height / 2, 1);
        assert!(shell.files_directory().dialog_active());
        assert!(!shell.launcher);
        extern crate std;
        let mut pixels = std::vec![0xdeadbeef; 640 * 480 + 2];
        let end = pixels.len() - 1;
        {
            let mut canvas =
                Canvas::new(&mut pixels[1..end], 640, 480, 640, PixelFormat::Bgr).unwrap();
            shell.set_viewport(640, 480);
            shell.render(&mut canvas);
        }
        assert_eq!(pixels[0], 0xdeadbeef);
        assert_eq!(pixels[end], 0xdeadbeef);
        shell.on_key(Key::Escape);
        assert!(!shell.files_directory().dialog_active());
        assert!(shell.take_files_directory_action().is_none());
    }
    #[test]
    fn files_directory_pointer_uses_polished_location_and_row_geometry() {
        let mut shell = loaded_directory_shell(&[
            ("/native/docs", files_directory::Kind::Directory, 7),
            ("/native/readme", files_directory::Kind::File, 0),
        ]);
        let b = shell.desktop().window(App::Files).bounds;
        shell.on_pointer(Pointer::Down(b.x + 50, b.y + 102));
        shell.on_key(Key::Character('/'));
        assert!(shell.files_directory_address_editing);
        shell.on_key(Key::Escape);
        shell.on_pointer(Pointer::Down(b.x + 52, b.y + 136 + 8));
        assert_eq!(
            shell.files_directory().selected().unwrap().path.as_str(),
            "/native/docs"
        );
        shell.on_pointer(Pointer::Down(b.x + 52, b.y + 136 + 8));
        assert!(matches!(
            shell.take_files_directory_action(),
            Some(files_directory::Action {
                intent: files_directory::Intent::Navigate { path },
                ..
            }) if path.as_str() == "/native/docs"
        ));
    }
    fn authoritative_files() -> Shell {
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        shell.set_boot_file("immutable boot note");
        shell.set_file(1, "/native/source", "actual source bytes");
        shell.set_files_revision(7);
        shell.open_app(App::Files);
        shell.on_key(Key::Down);
        shell
    }
    #[test]
    fn files_keyboard_create_waits_for_backend_then_selects_only_refreshed_binding() {
        let mut shell = authoritative_files();
        shell.on_key(Key::Tab);
        assert_eq!(
            shell.keyboard_focus_state().unwrap().control.label,
            "Create file"
        );
        shell.on_key(Key::Enter);
        assert!(shell.file_dialog_active());
        for c in "new".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Enter);
        let Some(files_actions::Action::InspectDestination { request, path }) =
            shell.take_file_action()
        else {
            panic!("inspect")
        };
        assert_eq!(path.as_str(), "/native/new");
        assert_eq!(shell.file_selection, 1);
        assert!(shell
            .files
            .iter()
            .all(|file| file.path.as_str() != path.as_str()));
        shell.file_destination_result(request, Ok(None)).unwrap();
        let Some(files_actions::Action::Commit { request, intent }) = shell.take_file_action()
        else {
            panic!("commit")
        };
        assert!(matches!(intent, files_actions::Intent::Create { .. }));
        shell.file_action_result(request, Ok(())).unwrap();
        assert!(!shell.file_dialog_active());
        assert_eq!(shell.file_selection, 1);
        assert!(matches!(
            shell.take_file_outcome(),
            Some(files_actions::Outcome::Created(_))
        ));
        shell.set_file(2, path.as_str(), "");
        assert_eq!(shell.file_selection, 2);
    }
    #[test]
    fn files_rename_conflict_preserves_authoritative_rows_input_and_visible_focus() {
        let mut shell = authoritative_files();
        shell.on_key(Key::Character('r'));
        assert_eq!(shell.file_dialog().selected().unwrap().revision, 7);
        for _ in 0..6 {
            shell.on_key(Key::Backspace);
        }
        for c in "target".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::Enter);
        let Some(files_actions::Action::InspectDestination { request, path }) =
            shell.take_file_action()
        else {
            panic!("inspect")
        };
        shell
            .file_destination_result(
                request,
                Ok(Some(files_actions::Snapshot { path, revision: 7 })),
            )
            .unwrap();
        assert_eq!(
            shell.keyboard_focus_state().unwrap().control.label,
            "Cancel"
        );
        shell.on_key(Key::Tab);
        assert!(shell.file_dialog().approved());
        shell.on_key(Key::Enter);
        let Some(files_actions::Action::Commit { request, .. }) = shell.take_file_action() else {
            panic!("commit")
        };
        shell
            .file_action_result(request, Err(files_actions::BackendError::Conflict))
            .unwrap();
        assert_eq!(shell.file_dialog().input(), "/native/target");
        assert_eq!(shell.files[1].path.as_str(), "/native/source");
        assert_eq!(shell.files[1].content.as_str(), "actual source bytes");
        assert_eq!(shell.file_selection, 1);
        assert_eq!(
            shell.file_dialog().error(),
            Some(files_actions::BackendError::Conflict)
        );
        assert!(shell.take_file_outcome().is_none());
        shell.on_key(Key::BackTab);
        assert!(shell.keyboard_focus_state().unwrap().focus_visible);
        shell.on_key(Key::Escape);
        assert!(!shell.file_dialog_active());
    }
    #[test]
    fn files_mouse_trash_move_is_cancel_first_and_portal_has_priority() {
        let mut shell = authoritative_files();
        let button = shell.file_action_button_bounds(2).unwrap();
        shell.on_pointer(Pointer::Down(button.x + 20, button.y + 12));
        assert_eq!(shell.trash_state().phase(), trash_ui::Phase::ConfirmMove);
        shell.on_key(Key::Enter);
        assert!(shell.take_trash_action().is_none());
        shell.on_pointer(Pointer::Down(button.x + 20, button.y + 12));
        shell.show_file_portal(82, 5, "org.next.real.requester", false);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Escape);
        assert!(shell.trash_dialog_active());
        assert!(!shell.trash_state().approved());
        let bounds = shell.file_dialog_bounds();
        shell.on_pointer(Pointer::Down(bounds.x + 160, bounds.y + bounds.height - 40));
        let Some(trash_ui::Action { request, intent }) = shell.take_trash_action() else {
            panic!("delete")
        };
        assert!(matches!(intent, trash_ui::Intent::Move { .. }));
        shell
            .trash_action_result(request, Err(files_actions::BackendError::ReadOnly))
            .unwrap();
        assert_eq!(shell.file_selection, 1);
        assert!(!shell.trash_state().approved());
        assert_eq!(
            shell.keyboard_focus_state().unwrap().control.label,
            "Cancel"
        );
    }
    #[test]
    fn appearance_keyboard_save_failure_retains_active_palette_and_retry_confirms() {
        let mut shell = Shell::new();
        shell
            .load_appearance(appearance::Loaded {
                preferences: appearance::Preferences::defaults(),
                origin: appearance::Origin::MissingDefault,
            })
            .unwrap();
        shell.open_app(App::Settings);
        shell.on_key(Key::Right);
        assert_eq!(
            shell.appearance_state().draft().theme,
            appearance::Theme::Dark
        );
        assert_eq!(shell.active_appearance().theme, appearance::Theme::Light);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Right);
        assert_eq!(
            shell.appearance_state().draft().accent,
            appearance::Accent::Violet
        );
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        let request = shell.take_appearance_request().unwrap();
        assert!(shell.appearance_save_result(request.id, false));
        assert_eq!(shell.active_appearance().theme, appearance::Theme::Light);
        assert_eq!(
            shell.appearance_state().status(),
            appearance_ui::Status::SaveFailed
        );
        assert!(!shell.appearance_save_result(request.id, true));
        shell.on_key(Key::Enter);
        let retry = shell.take_appearance_request().unwrap();
        assert_ne!(retry.id, request.id);
        assert!(shell.appearance_save_result(retry.id, true));
        assert_eq!(shell.active_appearance().theme, appearance::Theme::Dark);
        assert_eq!(shell.active_appearance().accent, appearance::Accent::Violet);
    }
    #[test]
    fn appearance_pointer_uses_real_controls_and_corrupt_load_cannot_save() {
        let mut shell = Shell::new();
        shell
            .load_appearance(appearance::Loaded {
                preferences: appearance::Preferences::defaults(),
                origin: appearance::Origin::Persisted,
            })
            .unwrap();
        shell.open_app(App::Settings);
        let b = shell.appearance_control_bounds(0, 2).unwrap();
        shell.on_pointer(Pointer::Down(b.x + 20, b.y + 12));
        assert_eq!(
            shell.appearance_state().draft().theme,
            appearance::Theme::HighContrast
        );
        let b = shell.appearance_control_bounds(2, 0).unwrap();
        shell.on_pointer(Pointer::Down(b.x + 20, b.y + 12));
        let request = shell.take_appearance_request().unwrap();
        shell.appearance_save_result(request.id, true);
        assert_eq!(
            shell.active_appearance().theme,
            appearance::Theme::HighContrast
        );
        shell.appearance_load_error();
        shell.on_key(Key::Enter);
        assert!(shell.take_appearance_request().is_none());
        assert_eq!(
            shell.appearance_state().status(),
            appearance_ui::Status::Corrupt
        );
    }
    #[test]
    fn actual_dark_and_high_contrast_buttons_use_semantic_background_and_readable_text() {
        extern crate std;
        for theme in [appearance::Theme::Dark, appearance::Theme::HighContrast] {
            let mut shell = Shell::new();
            shell.set_viewport(1280, 800);
            let prefs = appearance::Preferences {
                theme,
                ..appearance::Preferences::defaults()
            };
            shell
                .load_appearance(appearance::Loaded {
                    preferences: prefs,
                    origin: appearance::Origin::Persisted,
                })
                .unwrap();
            shell.open_app(App::Settings);
            let button = shell.appearance_control_bounds(2, 0).unwrap();
            let palette = palette::Palette::new(prefs);
            let mut pixels = std::vec![0;1280*800];
            let mut canvas = Canvas::new(&mut pixels, 1280, 800, 1280, PixelFormat::Bgr).unwrap();
            shell.render(&mut canvas);
            assert_eq!(
                pixels[(button.y + 4) as usize * 1280 + (button.x + button.width / 2) as usize],
                palette.background
            );
            let text_pixels = (button.y + 6..button.y + 30)
                .flat_map(|y| (button.x + 8..button.x + 70).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let p = pixels[y as usize * 1280 + x as usize];
                    (p & 255) > 200 && ((p >> 8) & 255) > 200 && ((p >> 16) & 255) > 200
                })
                .count();
            assert!(
                text_pixels > 35 && text_pixels < 400,
                "actual font foreground remains visible on themed button"
            );
        }
    }
    #[test]
    fn small_files_viewport_scrolls_to_actual_last_file_and_pointer_selects_same_row() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        for index in 0..FILE_CAPACITY {
            shell.set_file(index, "/native/actual", "");
        }
        shell.open_app(App::Files);
        for _ in 0..8 {
            shell.on_key(Key::Down);
        }
        assert_eq!(shell.file_selection, 8);
        assert!(shell.file_list_start() > 0);
        let bounds = shell.desktop().window(App::Files).bounds;
        let row = shell.file_selection - shell.file_list_start();
        assert!(row < shell.file_list_rows());
        shell.on_pointer(Pointer::Down(
            bounds.x + 40,
            bounds.y + 106 + row as i32 * 24 + 8,
        ));
        assert!(matches!(
            shell.take_editor_action(),
            Some(EditorAction::Open { .. })
        ));
    }
    #[test]
    fn high_contrast_selected_options_and_button_bounds_remain_visible() {
        extern crate std;
        let mut shell = Shell::new();
        shell.set_viewport(1280, 800);
        let prefs = appearance::Preferences {
            theme: appearance::Theme::HighContrast,
            ..appearance::Preferences::defaults()
        };
        shell
            .load_appearance(appearance::Loaded {
                preferences: prefs,
                origin: appearance::Origin::Persisted,
            })
            .unwrap();
        shell.open_app(App::Settings);
        shell.on_key(Key::Right);
        shell.on_key(Key::Left);
        let mut pixels = std::vec![0;1280*800];
        let mut canvas = Canvas::new(&mut pixels, 1280, 800, 1280, PixelFormat::Bgr).unwrap();
        shell.render(&mut canvas);
        let selected = shell.appearance_control_bounds(0, 2).unwrap();
        let other = shell.appearance_control_bounds(0, 0).unwrap();
        let accent = shell.appearance_control_bounds(1, 0).unwrap();
        let neutral = shell.appearance_control_bounds(2, 0).unwrap();
        let p = palette::Palette::new(prefs);
        let at = |b: Bounds| pixels[b.y as usize * 1280 + (b.x + b.width / 2) as usize];
        assert_eq!(at(selected), p.accent);
        assert_eq!(at(accent), p.accent);
        assert_eq!(at(other), p.border);
        assert_eq!(at(neutral), p.border);
        assert_eq!(
            pixels[(selected.y + selected.height - 2) as usize * 1280
                + (selected.x + selected.width / 2) as usize],
            p.accent
        );
        assert_eq!(
            pixels[(other.y + other.height - 2) as usize * 1280
                + (other.x + other.width / 2) as usize],
            p.background
        );
    }
    #[test]
    fn semantic_shortcuts_open_real_windows_and_leave_terminal_input_untouched() {
        let mut shell = Shell::new();
        shell.on_key(Key::Character('x'));
        for (key, app) in [
            (Key::OpenFiles, App::Files),
            (Key::OpenSettings, App::Settings),
            (Key::OpenTerminal, App::Terminal),
        ] {
            shell.on_key(key);
            assert_eq!(shell.desktop.focused(), Some(app));
            if app == App::Files {
                let action = shell.take_files_directory_action().unwrap();
                assert!(matches!(
                    action.intent,
                    files_directory::Intent::Navigate { path }
                        if path.as_str() == "/native"
                ));
            }
            assert_eq!(shell.input(), "x");
            assert!(!shell.launcher_open());
        }
        shell.on_key(Key::OpenSearch);
        assert_eq!(shell.search_query(), Some(""));
        shell.on_key(Key::Character('f'));
        assert_eq!(shell.search_query(), Some("f"));
        shell.on_key(Key::Menu);
        assert!(!shell.launcher_open());
        assert_eq!(shell.input(), "x");
    }
    #[test]
    fn home_shortcut_guide_is_keyboard_navigable_and_publishes_focus_events() {
        use accessibility::{EventKind, Role, Snapshot};
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell.on_key(Key::Menu);
        for _ in 0..desktop::ACTIONS.len() {
            shell.on_key(Key::Down);
        }
        assert_eq!(shell.selection, desktop::ACTIONS.len());
        let home = shell.home_bounds();
        let guide = shell.home_action_bounds(desktop::ACTIONS.len()).unwrap();
        assert!(guide.y >= home.y && guide.y + guide.height <= home.y + home.height);

        let mut snapshot = Snapshot::empty();
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert!(snapshot.complete);
        let home_entry = snapshot
            .nodes()
            .iter()
            .find(|node| node.name() == "Keyboard shortcuts")
            .unwrap();
        assert_eq!(home_entry.role, Role::Button);
        assert!(home_entry.state.focused);

        shell.on_key(Key::Enter);
        assert!(shell.shortcuts_open());
        assert!(shell.launcher_open());
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.context, 28);
        assert_eq!(snapshot.nodes()[0].role, Role::Dialog);
        assert_eq!(snapshot.nodes()[0].name(), "Keyboard shortcuts");
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.state.focused)
                .unwrap()
                .name(),
            shortcuts::ENTRIES[0].name
        );
        while shell.take_accessibility_event().is_some() {}
        shell.on_key(Key::Tab);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.state.focused)
                .unwrap()
                .name(),
            shortcuts::ENTRIES[1].name
        );
        let mut focus_changed = false;
        while let Some(event) = shell.take_accessibility_event() {
            focus_changed |= event.kind == EventKind::FocusChanged;
        }
        assert!(focus_changed);
        for _ in 1..shortcuts::ENTRIES.len() {
            shell.on_key(Key::Tab);
        }
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        let close = snapshot
            .nodes()
            .iter()
            .find(|node| node.name() == "Close keyboard shortcuts")
            .unwrap();
        assert_eq!(close.role, Role::Button);
        assert!(close.state.focused);
        shell.on_key(Key::Enter);
        assert!(!shell.shortcuts_open());
        assert!(shell.launcher_open());
    }
    #[test]
    fn shortcut_guide_is_searchable_and_pointer_modal_does_not_activate_background() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell.on_key(Key::Menu);
        for character in "keyboard shortcuts".chars() {
            shell.on_key(Key::Character(character));
        }
        assert_eq!(shell.search_count(), 1);
        shell.on_key(Key::Enter);
        assert!(shell.shortcuts_open());
        assert!(!shell.launcher_open());
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));

        let row = shell.shortcut_row_bounds(4).unwrap();
        shell.on_pointer_packet(row.x + 8, row.y + 8, 1);
        assert_eq!(shell.shortcuts_selected(), 4);
        shell.on_pointer_packet(row.x + 8, row.y + 8, 0);
        shell.on_pointer_packet(4, 4, 1);
        assert!(!shell.shortcuts_open());
        assert_eq!(shell.desktop().focused(), Some(App::Terminal));
    }
    #[test]
    fn alt_close_uses_cancel_and_failed_save_preserves_dirty_editor() {
        let mut shell = Shell::new();
        shell.load_document("/native/doc", "a").unwrap();
        shell.on_key(Key::Character('b'));
        shell.on_key(Key::CloseWindow);
        assert!(matches!(shell.editor_deferred, Some(EditorDeferred::Close)));
        shell.on_key(Key::Enter);
        assert!(shell.editor_deferred.is_none());
        assert!(shell.editor().dirty());
        assert_eq!(shell.desktop.focused(), Some(App::Editor));
        shell.on_key(Key::CloseWindow);
        shell.on_key(Key::Left); // Save, after default Cancel.
        shell.on_key(Key::Enter);
        let Some(EditorAction::Save { revision, .. }) = shell.take_editor_action() else {
            panic!("save action missing")
        };
        shell.editor_save_result(revision, Err("Store read-only"));
        assert!(matches!(shell.editor_deferred, Some(EditorDeferred::Close)));
        assert!(shell.editor().dirty());
        shell.editor_save_result(revision, Ok(()));
        assert_eq!(shell.desktop.window(App::Editor).state, WindowState::Closed);
    }
    #[test]
    fn file_confirmation_blocks_global_shortcuts() {
        let mut shell = Shell::new();
        shell
            .file_dialog
            .begin_delete(files_actions::Snapshot {
                path: files_actions::Path::new("/native/doc").unwrap(),
                revision: 7,
            })
            .unwrap();
        let focus = shell.desktop.focused();
        for key in [
            Key::OpenSearch,
            Key::OpenSettings,
            Key::OpenFiles,
            Key::OpenTerminal,
            Key::CloseWindow,
        ] {
            shell.on_key(key);
            assert_eq!(shell.desktop.focused(), focus);
            assert!(!shell.launcher_open());
            assert_eq!(
                shell.file_dialog.phase(),
                files_actions::Phase::ConfirmDelete
            );
            assert!(shell.take_file_action().is_none());
        }
    }
    #[test]
    fn copy_ui_emits_authoritative_intent_and_selects_only_confirmed_binding() {
        let mut shell = Shell::new();
        shell.set_file(1, "/native/source", "real preview");
        shell.set_files_revision(19);
        shell.open_app(App::Files);
        shell.on_key(Key::Down);
        shell.on_key(Key::Character('c'));
        assert!(shell.file_dialog.is_copy());
        shell.file_dialog.set_input("/native/copy").unwrap();
        shell.file_dialog.activate().unwrap();
        let Some(files_actions::Action::InspectDestination { request, .. }) =
            shell.take_file_action()
        else {
            panic!("missing inspect")
        };
        shell.file_destination_result(request, Ok(None)).unwrap();
        let Some(files_actions::Action::Commit {
            request,
            intent:
                files_actions::Intent::Copy {
                    source,
                    destination,
                    replace,
                },
        }) = shell.take_file_action()
        else {
            panic!("missing Copy commit")
        };
        assert_eq!(source.revision, 19);
        assert_eq!(source.path.as_str(), "/native/source");
        assert_eq!(destination.as_str(), "/native/copy");
        assert_eq!(replace, None);
        assert_eq!(shell.file_selection, 1);
        assert!(!shell
            .files
            .iter()
            .any(|f| f.path.as_str() == "/native/copy"));
        shell.file_action_result(request, Ok(())).unwrap();
        assert_eq!(shell.file_status(), "File copied.");
        assert_eq!(shell.file_selection, 1);
        shell.set_file(2, "/native/copy", "actual stored bytes");
        assert_eq!(shell.file_selection, 2);
        assert_eq!(shell.files[1].path.as_str(), "/native/source");
        assert!(shell.file_action_button_bounds(3).is_some());
        assert!(shell.file_action_button_bounds(4).is_some());
        assert!(shell.file_action_button_bounds(6).is_none());
    }
    #[test]
    fn native_windows_use_exact_top_identity_for_pixels_and_input() {
        extern crate std;
        use client_interaction::{Action as C, SurfaceRef};
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        for (slot, color) in [(0, [255, 0, 0, 0]), (1, [0, 255, 0, 0])] {
            shell
                .submit_client_surface(
                    slot,
                    1,
                    1,
                    8 + slot as u64,
                    "client",
                    1,
                    1,
                    1,
                    PixelFormat::Rgb,
                    &color,
                )
                .unwrap();
        }
        raise_client_fixture(&mut shell, 1);
        let zero = SurfaceRef {
            slot: 0,
            generation: 1,
        };
        let one = SurfaceRef {
            slot: 1,
            generation: 1,
        };
        let original = shell.native_window_presentation(one).unwrap().bounds;
        let target = shell.native_window_presentation(zero).unwrap().bounds;
        let title = shell.native_window_titlebar_bounds(one).unwrap();
        shell.on_pointer(Pointer::Down(title.x + 2, title.y + 2));
        shell.on_pointer(Pointer::Move(
            title.x + 2 + target.x - original.x,
            title.y + 2 + target.y - original.y,
        ));
        shell.on_pointer(Pointer::Up);
        shell.client_leave_context();
        while shell.take_client_action().is_some() {}
        raise_client_fixture(&mut shell, 0);
        let b = shell.client_content_bounds(0).unwrap();
        let (x, y) = (b.x + 10, b.y + 10);
        assert!(Shell::hit(shell.client_content_bounds(1).unwrap(), x, y));
        let mut pixels = std::vec![0;640*480];
        let mut canvas = Canvas::new(&mut pixels, 640, 480, 640, PixelFormat::Bgr).unwrap();
        shell.render(&mut canvas);
        assert_eq!(pixels[y as usize * 640 + x as usize], 0xff0000);
        shell.on_pointer_packet(x, y, 1);
        assert_eq!(shell.take_client_action(), Some(C::Focus { surface: zero }));
        assert!(shell.client_focus_result(zero, true));
        assert!(matches!(shell.take_client_action(),Some(C::Pointer{surface,..}) if surface==zero));
        shell.on_pointer_packet(x, y, 0);
        while shell.take_client_action().is_some() {}
        shell.open_app(App::Terminal);
        assert_eq!(
            shell.top_window_at(x, y),
            Some(native_windows::WindowRef::Builtin(App::Terminal))
        );
        while shell.take_client_action().is_some() {}
        shell.on_pointer_packet(x, y, 1);
        assert!(!shell.client_context_active());
        assert!(shell.take_client_action().is_none());
    }
    #[test]
    fn native_move_minimize_restore_preserves_owner_and_publication_without_autofocus() {
        use client_interaction::{Action as C, SurfaceRef};
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        let id = SurfaceRef {
            slot: 0,
            generation: 1,
        };
        let original = shell.native_window_presentation(id).unwrap();
        assert!(!original.focused);
        assert_eq!(original.z, 0);
        raise_client_fixture(&mut shell, 0);
        let title = shell.native_window_titlebar_bounds(id).unwrap();
        for mask in [2, 4, 6, 0] {
            shell.on_pointer_packet(title.x + 2, title.y + 2, mask);
        }
        assert!(shell.take_client_action().is_none());
        shell.on_pointer(Pointer::Down(title.x + 2, title.y + 2));
        assert_eq!(
            shell.take_client_action(),
            Some(C::FocusChrome { surface: id })
        );
        shell.on_pointer(Pointer::Move(title.x - 70, title.y + 60));
        shell.on_pointer(Pointer::Up);
        assert!(shell.client_focus_result(id, true));
        assert!(shell.take_client_action().is_none());
        let moved = shell.native_window_presentation(id).unwrap();
        assert_ne!(moved.bounds, original.bounds);
        let minimize = shell.native_window_minimize_button_bounds(id).unwrap();
        shell.on_pointer(Pointer::Down(minimize.x + 2, minimize.y + 2));
        shell.on_pointer(Pointer::Up);
        assert_eq!(shell.take_client_action(), Some(C::Suspend));
        assert!(!shell.client_context_active());
        let hidden = shell.native_window_presentation(id).unwrap();
        assert!(hidden.minimized && !hidden.mapped);
        assert_eq!(shell.client_previews.identity(0), Some(id));
        assert!(shell.client_content_bounds(0).is_none());
        shell
            .submit_client_surface(
                0,
                1,
                2,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[1, 2, 3, 0],
            )
            .unwrap();
        assert_eq!(shell.native_window_presentation(id).unwrap(), hidden);
        let button = shell.native_window_restore_button_bounds(0).unwrap();
        shell.on_pointer(Pointer::Down(button.x + 2, button.y + 2));
        shell.on_pointer(Pointer::Up);
        assert_eq!(
            shell.take_client_action(),
            Some(C::FocusChrome { surface: id })
        );
        assert!(shell.client_focus_result(id, true));
        assert!(shell.take_client_action().is_none());
        assert_eq!(
            shell.native_window_presentation(id).unwrap().bounds,
            moved.bounds
        );
        assert!(shell.native_window_presentation(id).unwrap().mapped);
        shell.clear_client_surface(0, 1).unwrap();
        assert!(!shell.restore_native_window(id));
        assert!(!shell.minimize_native_window(id));
    }
    #[test]
    fn native_resize_preview_retry_cancel_and_exact_configured_retirement() {
        extern crate std;
        use client_interaction::SurfaceRef;
        use native_resize::ResizeError;
        let mut shell = Shell::new();
        shell.set_viewport(800, 600);
        shell
            .submit_client_surface_configured(
                0,
                1,
                1,
                3,
                8,
                "client",
                64,
                64,
                64,
                PixelFormat::Rgb,
                &[255; 64 * 64 * 4],
            )
            .unwrap();
        let id = SurfaceRef {
            slot: 0,
            generation: 1,
        };
        raise_client_fixture(&mut shell, 0);
        let original = shell.native_window_presentation(id).unwrap();
        let handle = shell.native_window_resize_handle_bounds(id).unwrap();
        shell.on_pointer_packet(handle.x + 5, handle.y + 5, 1);
        shell.on_pointer_packet(handle.x + 45, handle.y + 25, 1);
        let mut semantics = accessibility::Snapshot::empty();
        shell.write_accessibility_snapshot(&mut semantics).unwrap();
        assert_eq!(semantics.context, 40);
        assert!(semantics
            .nodes()
            .iter()
            .any(|n| n.name() == "Cancel resize" && n.state.focused));
        assert!(shell.take_native_resize_action().is_none());
        assert_eq!(shell.native_window_presentation(id).unwrap(), original);
        shell.on_pointer_packet(handle.x + 45, handle.y + 25, 0);
        let action = shell.take_native_resize_action().unwrap();
        assert_eq!(action.expected_configuration, 3);
        assert_eq!(action.stride, action.width * 4);
        assert!(action.width > 64 && action.width <= 128);
        assert!(!shell.native_resize_result(action.request + 1, Ok(4)));
        assert!(shell.native_resize_result(action.request, Err(ResizeError::Retry)));
        assert!(shell.client_content_available(id));
        assert_eq!(shell.native_window_presentation(id).unwrap(), original);
        shell.on_key(Key::Enter);
        let retry = shell.take_native_resize_action().unwrap();
        assert_ne!(retry.request, action.request);
        assert_eq!(retry.expected_configuration, 3);
        assert!(shell.retire_client_content(id, 3, 4).is_ok());
        assert!(shell.native_resize_result(retry.request, Ok(4)));
        while shell.take_client_action().is_some() {}
        let terminal = shell.desktop.window(App::Terminal).state;
        shell.on_key(Key::Character('a'));
        shell.on_key(Key::Enter);
        shell.on_key(Key::CloseWindow);
        assert_eq!(shell.input(), "");
        assert_eq!(shell.desktop.window(App::Terminal).state, terminal);
        assert!(shell.take_client_action().is_none());
        assert_eq!(shell.client_configuration(id), Some(4));
        assert!(!shell.client_content_available(id));
        assert_eq!(shell.client_previews.identity(0), Some(id));
        assert!(shell.client_content_bounds(0).is_none());
        assert_eq!(
            shell.client_close_status(id),
            Some(client_surface::CloseStatus::Unsupported)
        );
        let moved = shell.native_window_presentation(id).unwrap();
        assert_eq!(moved.bounds, retry.bounds);
        assert_eq!(moved.z, original.z);
        assert!(shell
            .submit_client_surface_configured(
                0,
                1,
                2,
                3,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4]
            )
            .is_err());
        assert!(shell
            .submit_client_surface(0, 1, 2, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .is_err());
        let b = shell.client_previews.content_bounds(0, 800, 600).unwrap();
        let mut pixels = std::vec![0;800*600];
        let mut canvas = Canvas::new(&mut pixels, 800, 600, 800, PixelFormat::Bgr).unwrap();
        shell.render(&mut canvas);
        assert_ne!(
            pixels[(b.y + 40) as usize * 800 + (b.x + 10) as usize],
            0xffffff,
            "retired white app pixels must disappear"
        );
        shell
            .submit_client_surface_configured(
                0,
                1,
                2,
                4,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[1, 2, 3, 0],
            )
            .unwrap();
        assert!(shell.client_content_available(id));
        assert!(shell.client_context_active());
        let handle = shell.native_window_resize_handle_bounds(id).unwrap();
        shell.on_pointer(Pointer::Down(handle.x + 5, handle.y + 5));
        shell.on_pointer(Pointer::Move(handle.x + 25, handle.y + 25));
        shell.on_key(Key::Escape);
        assert!(shell.native_resize_preview().is_none());
        assert!(shell.take_native_resize_action().is_none());
        assert_eq!(shell.native_window_presentation(id).unwrap(), moved);
    }
    #[test]
    fn held_client_button_entering_resize_chrome_revokes_without_fake_pointer_release() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(
                0,
                1,
                1,
                8,
                "client",
                64,
                64,
                64,
                PixelFormat::Rgb,
                &[0; 64 * 64 * 4],
            )
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 1,
        };
        let body = shell.client_content_bounds(0).unwrap();
        shell.on_pointer_packet(body.x + 2, body.y + 2, 1);
        shell.take_client_action();
        shell.client_focus_result(id, true);
        shell.take_client_action();
        let handle = shell.native_window_resize_handle_bounds(id).unwrap();
        shell.on_pointer_packet(handle.x + 5, handle.y + 5, 0);
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::Suspend)
        );
        assert!(shell.take_client_action().is_none());
        assert_eq!(shell.client_focused(), None);
        assert!(shell.client_context_active());
        shell.on_key(Key::Character('x'));
        assert_eq!(shell.input(), "");
    }
    #[test]
    fn native_resize_handle_overlapping_pixels_is_trusted_for_all_packet_masks() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(
                0,
                1,
                1,
                8,
                "client",
                134,
                70,
                134,
                PixelFormat::Rgb,
                &[0; 134 * 70 * 4],
            )
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 1,
        };
        let handle = shell.native_window_resize_handle_bounds(id).unwrap();
        assert!(Shell::hit(
            shell.client_content_bounds(0).unwrap(),
            handle.x,
            handle.y
        ));
        for mask in [2, 4, 6, 0] {
            shell.on_pointer_packet(handle.x, handle.y, mask);
            assert!(shell.take_client_action().is_none());
            assert!(shell.native_resize_preview().is_none());
        }
        shell.on_pointer_packet(handle.x, handle.y, 1);
        assert!(shell.native_resize_preview().is_some());
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::Suspend)
        );
        shell.show_file_portal(1, 8, "client", false);
        assert!(shell.native_resize_preview().is_none());
        assert!(shell.take_native_resize_action().is_none());
        shell.on_key(Key::Escape);
        assert!(shell.native_resize_preview().is_none());
    }
    #[test]
    fn native_retirement_preserves_minimize_watermark_and_teardown_is_distinct() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface_configured(
                0,
                2,
                7,
                1,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4],
            )
            .unwrap();
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 2,
        };
        shell.minimize_native_window(id);
        let original = shell.native_window_presentation(id).unwrap();
        assert!(shell.retire_client_content(id, 1, 2).is_ok());
        assert!(shell.retire_client_content(id, 1, 2).is_ok());
        assert!(shell.retire_client_content(id, 1, 3).is_err());
        assert!(shell
            .submit_client_surface_configured(
                0,
                2,
                7,
                2,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4]
            )
            .is_err());
        shell
            .submit_client_surface_configured(
                0,
                2,
                8,
                2,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4],
            )
            .unwrap();
        assert_eq!(shell.native_window_presentation(id).unwrap(), original);
        shell.retire_client_content(id, 2, 3).unwrap();
        assert!(shell.retire_client_content(id, 1, 3).is_err());
        shell
            .submit_client_surface_configured(
                0,
                2,
                9,
                3,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4],
            )
            .unwrap();
        shell.clear_client_surface(0, 2).unwrap();
        assert!(shell
            .submit_client_surface_configured(
                0,
                2,
                10,
                3,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4]
            )
            .is_err());
        shell
            .submit_client_surface_configured(
                0,
                3,
                1,
                1,
                9,
                "replacement",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0; 4],
            )
            .unwrap();
        assert!(shell.retire_client_content(id, 3, 4).is_err());
    }
    #[test]
    fn trusted_load_and_serial_input_raise_their_actual_window_above_native() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        shell.load_document("/native/doc", "saved").unwrap();
        let point = shell.client_content_bounds(0).unwrap();
        assert_eq!(
            shell.top_window_at(point.x + 2, point.y + 2),
            Some(native_windows::WindowRef::Builtin(App::Editor))
        );
        raise_client_fixture(&mut shell, 0);
        shell.on_terminal_key(Key::Character('x'));
        assert_eq!(
            shell.top_window_at(point.x + 2, point.y + 2),
            Some(native_windows::WindowRef::Builtin(App::Terminal))
        );
        assert_eq!(shell.input(), "x");
        assert!(!shell.client_context_active());
    }
    #[test]
    fn mixed_window_switch_uses_stable_mru_and_restores_minimized_full_identity() {
        use client_interaction::{Action as C, SurfaceRef};
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        let id = SurfaceRef {
            slot: 0,
            generation: 1,
        };
        shell.open_app(App::Files);
        raise_client_fixture(&mut shell, 0);
        // Explicit chrome focus establishes native as the current mixed-MRU target.
        assert!(shell.restore_native_window(id));
        assert_eq!(
            shell.take_client_action(),
            Some(C::FocusChrome { surface: id })
        );
        shell.client_focus_result(id, true);
        shell.on_key(Key::WindowNext);
        assert_eq!(shell.desktop.focused(), Some(App::Files));
        assert!(!shell.client_context_active());
        shell.on_key(Key::WindowPrevious);
        assert_eq!(shell.take_client_action(), Some(C::Suspend));
        assert_eq!(
            shell.take_client_action(),
            Some(C::FocusChrome { surface: id })
        );
        shell.client_focus_result(id, true);
        shell.on_key(Key::WindowSwitchEnd);
        assert!(shell.minimize_native_window(id));
        while shell.take_client_action().is_some() {}
        shell.open_app(App::Files);
        shell.on_key(Key::WindowNext);
        assert!(shell.native_window_presentation(id).unwrap().mapped);
        assert_eq!(
            shell.take_client_action(),
            Some(C::FocusChrome { surface: id })
        );
    }
    #[test]
    fn native_restore_strip_consumes_motion_masks_and_tiny_viewport_is_safe() {
        extern crate std;
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 1, "x", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 1,
        };
        raise_client_fixture(&mut shell, 0);
        let title = shell.native_window_titlebar_bounds(id).unwrap();
        shell.on_pointer(Pointer::Down(title.x + 2, title.y + 2));
        shell.on_pointer(Pointer::Move(6, 164));
        shell.on_pointer(Pointer::Up);
        while shell.take_client_action().is_some() {}
        shell.client_focus_result(id, true);
        let strip = shell.native_window_restore_button_bounds(0).unwrap();
        for mask in [2, 4, 6, 0] {
            shell.on_pointer_packet(strip.x + 24, strip.y + 6, mask);
            assert!(shell.take_client_action().is_none());
        }
        let normal = shell.native_window_presentation(id).unwrap().bounds;
        shell.set_viewport(16, 480);
        assert!(!shell.native_window_presentation(id).unwrap().mapped);
        assert!(shell.client_content_bounds(0).is_none());
        shell.set_viewport(640, 480);
        assert_eq!(shell.native_window_presentation(id).unwrap().bounds, normal);
        let mut pixels = std::vec![0;640*480];
        let mut c = Canvas::new(&mut pixels, 640, 480, 640, PixelFormat::Rgb).unwrap();
        shell.render(&mut c);
        assert!(shell.native_window_titlebar_bounds(id).unwrap().width > 0);
        shell.native_windows.viewport(i32::MIN, 0);
    }
    fn raise_client_fixture(shell: &mut Shell, slot: usize) {
        let b = shell.native_window_restore_button_bounds(slot).unwrap();
        shell.on_pointer(Pointer::Down(b.x + 2, b.y + 2));
        shell.on_pointer(Pointer::Up);
        assert!(
            matches!(shell.take_client_action(),Some(client_interaction::Action::FocusChrome{surface}) if surface.slot==slot)
        );
        // End the fixture's chrome context so each input test begins with a fresh content gesture.
        shell.client_leave_context();
        while shell.take_client_action().is_some() {}
    }
    #[test]
    fn client_publication_never_steals_focus_or_routes_clicks_into_trusted_windows() {
        let mut shell = Shell::new();
        shell.set_viewport(1280, 800);
        let focus = shell.desktop.focused();
        shell
            .submit_client_surface(
                0,
                1,
                1,
                8,
                "org.next.client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[255, 0, 0, 0],
            )
            .unwrap();
        assert_eq!(shell.desktop.focused(), focus);
        let b = shell.client_previews.bounds(0, 1280, 800).unwrap();
        assert_eq!(
            shell.top_window_at(b.x + 20, b.y + 120),
            Some(native_windows::WindowRef::Builtin(App::Terminal))
        );
        shell.on_pointer(Pointer::Down(b.x + 20, b.y + 120));
        shell.on_pointer(Pointer::Up);
        assert_eq!(shell.desktop.focused(), focus);
        assert!(!shell.client_context_active());
        assert!(shell.take_client_action().is_none());
        shell.request_editor_open("/native/doc").unwrap();
        shell.load_document("/native/doc", "a").unwrap();
        shell.on_key(Key::Character('b'));
        shell.on_key(Key::CloseWindow);
        shell
            .submit_client_surface(
                0,
                1,
                2,
                8,
                "org.next.client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0, 255, 0, 0],
            )
            .unwrap();
        assert!(matches!(shell.editor_deferred, Some(EditorDeferred::Close)));
        assert_eq!(shell.desktop.focused(), Some(App::Editor));
        shell.on_key(Key::Enter); // trusted Cancel remains the default
        assert!(shell.editor().dirty());
    }
    #[test]
    fn settings_files_modal_focus_switches_reclaim_inactive_controls() {
        let mut shell = Shell::new();
        shell
            .load_appearance(appearance::Loaded {
                preferences: appearance::Preferences::defaults(),
                origin: appearance::Origin::MissingDefault,
            })
            .unwrap();
        shell.set_file(1, "/native/test", "actual");
        for _ in 0..3 {
            shell.open_app(App::Settings);
            shell.on_key(Key::Tab);
            assert_eq!(shell.keyboard_focus_state().unwrap().control.id.scope, 5);
            shell.open_app(App::Files);
            shell.on_key(Key::Down);
            shell.on_key(Key::Tab);
            assert_eq!(
                shell.keyboard_focus_state().unwrap().control.id,
                keyboard_focus::ControlId { scope: 2, local: 2 }
            );
            shell.on_key(Key::Tab); // Rename
            shell.on_key(Key::Enter);
            assert_eq!(shell.keyboard_focus_state().unwrap().control.id.scope, 3);
            shell.on_key(Key::Escape);
            assert_eq!(
                shell.keyboard_focus_state().unwrap().control.id,
                keyboard_focus::ControlId { scope: 2, local: 3 }
            );
            shell.on_key(Key::BackTab);
            let state = shell.keyboard_focus_state().unwrap();
            assert_eq!(
                state.control.id,
                keyboard_focus::ControlId { scope: 2, local: 2 }
            );
            assert!(state.focus_visible);
            assert_eq!(shell.file_dialog.phase(), files_actions::Phase::Idle);
        }
    }
    #[test]
    fn visible_client_preview_blocks_underlying_dirty_editor_close_hitbox() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell.load_document("/native/doc", "a").unwrap();
        shell.window_action(Action::Maximize);
        shell.on_key(Key::Character('b'));
        shell
            .submit_client_surface(
                0,
                1,
                1,
                8,
                "org.next.client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[255, 0, 0, 0],
            )
            .unwrap();
        let preview = shell.client_previews.bounds(0, 640, 480).unwrap();
        // Place a floating Editor close control behind the visible preview
        // using the existing keyboard move operation.
        shell.window_action(Action::Restore);
        shell.window_action(Action::Move);
        for _ in 0..12 {
            shell.on_key(Key::Right);
        }
        for _ in 0..2 {
            shell.on_key(Key::Down);
        }
        shell.on_key(Key::Enter);
        raise_client_fixture(&mut shell, 0);
        let e = shell.desktop.window(App::Editor).bounds;
        let (x, y) = (e.x + e.width - 16, e.y + 20);
        assert!(
            Shell::hit(preview, x, y),
            "overlap required: editor={e:?} preview={preview:?}; original {x},{y}"
        );
        shell.on_pointer(Pointer::Down(x, y));
        shell.on_pointer(Pointer::Move(x, y));
        shell.on_pointer(Pointer::Up);
        assert!(shell.editor_deferred.is_none());
        assert!(shell.editor().dirty());
    }
    #[test]
    fn dirty_editor_confirmation_pixels_render_above_actual_client_content() {
        extern crate std;
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell.load_document("/native/doc", "a").unwrap();
        shell.window_action(Action::Maximize);
        shell.on_key(Key::Character('b'));
        shell
            .submit_client_surface(
                0,
                1,
                1,
                8,
                "client",
                1,
                1,
                1,
                PixelFormat::Rgb,
                &[0, 255, 0, 0],
            )
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let e = shell.desktop.window(App::Editor).bounds;
        let p = shell.client_previews.bounds(0, 640, 480).unwrap();
        let (x, y) = (p.x + 30, e.y + 155);
        let mut pixels = std::vec![0;640*480];
        {
            let mut canvas = Canvas::new(&mut pixels, 640, 480, 640, PixelFormat::Bgr).unwrap();
            shell.render(&mut canvas);
        }
        assert_eq!(
            pixels[y as usize * 640 + x as usize],
            0x00ff00,
            "point must be actual client body before prompt"
        );
        shell.on_key(Key::CloseWindow);
        {
            let mut canvas = Canvas::new(&mut pixels, 640, 480, 640, PixelFormat::Bgr).unwrap();
            shell.render(&mut canvas);
        }
        assert_eq!(
            pixels[y as usize * 640 + x as usize],
            theme::SURFACE,
            "trusted prompt must cover client pixels"
        );
    }
    #[test]
    fn trash_restore_collision_keeps_actual_row_and_input_then_refreshes_after_success() {
        let mut shell = authoritative_files();
        let original = files_actions::Path::new("/native/doc").unwrap();
        let item = trash_ui::Item {
            id: 41,
            revision: 7,
            original,
            size: 4,
        };
        shell
            .set_trash_listing(trash_ui::Listing {
                items: [Some(item), None],
                revision: 7,
            })
            .unwrap();
        shell.show_trash();
        shell.on_key(Key::Character('r'));
        assert!(shell.trash_dialog_active());
        assert_eq!(
            shell.keyboard_focus_state().unwrap().control.label,
            "Cancel"
        );
        shell.on_key(Key::BackTab); // path
        for _ in 0..3 {
            shell.on_key(Key::Backspace);
        }
        for c in "other".chars() {
            shell.on_key(Key::Character(c));
        }
        shell.on_key(Key::BackTab); // approve
        shell.on_key(Key::Enter);
        let action = shell.take_trash_action().unwrap();
        assert!(
            matches!(action.intent,trash_ui::Intent::Restore{destination,..} if destination.as_str()=="/native/other")
        );
        shell
            .trash_action_result(action.request, Err(files_actions::BackendError::Exists))
            .unwrap();
        assert_eq!(shell.trash_state().selected(), Some(item));
        assert_eq!(shell.trash_state().input(), "/native/other");
        assert_eq!(
            shell.keyboard_focus_state().unwrap().control.label,
            "Cancel"
        );
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        let retry = shell.take_trash_action().unwrap();
        assert!(shell.trash_action_result(action.request, Ok(())).is_err());
        shell.trash_action_result(retry.request, Ok(())).unwrap();
        assert_eq!(shell.trash_state().selected(), Some(item));
        shell
            .set_trash_listing(trash_ui::Listing {
                items: [None, None],
                revision: 8,
            })
            .unwrap();
        assert!(shell.trash_state().selected().is_none());
    }
    #[test]
    fn trash_permanent_delete_defaults_cancel_and_modal_blocks_shortcuts() {
        let mut shell = authoritative_files();
        let item = trash_ui::Item {
            id: 1,
            revision: 7,
            original: files_actions::Path::new("/native/doc").unwrap(),
            size: 4,
        };
        shell
            .set_trash_listing(trash_ui::Listing {
                items: [Some(item), None],
                revision: 7,
            })
            .unwrap();
        shell.show_trash();
        shell.on_key(Key::Delete);
        for key in [Key::OpenSearch, Key::OpenTerminal, Key::CloseWindow] {
            shell.on_key(key);
            assert_eq!(shell.desktop.focused(), Some(App::Files));
            assert!(!shell.launcher_open());
        }
        shell.on_key(Key::Enter);
        assert!(shell.take_trash_action().is_none());
        assert_eq!(shell.trash_state().selected(), Some(item));
        shell.on_key(Key::Delete);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        let action = shell.take_trash_action().unwrap();
        assert_eq!(action.intent, trash_ui::Intent::Purge { item });
        shell
            .trash_action_result(action.request, Err(files_actions::BackendError::Io))
            .unwrap();
        assert_eq!(shell.trash_state().selected(), Some(item));
        assert_eq!(
            shell.trash_state().error(),
            Some(files_actions::BackendError::Io)
        );
    }
    #[test]
    fn client_content_gesture_ack_maps_scaled_pixels_and_routes_only_press_semantics() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        let pixels = [0; 64 * 64 * 4];
        shell
            .submit_client_surface(0, 4, 1, 8, "client", 64, 64, 64, PixelFormat::Rgb, &pixels)
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let b = shell.client_content_bounds(0).unwrap();
        let surface = client_interaction::SurfaceRef {
            slot: 0,
            generation: 4,
        };
        shell.on_pointer(Pointer::Down(b.x + b.width / 2, b.y + b.height / 2));
        assert!(shell.client_context_active());
        assert!(shell.client_focused().is_none());
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::Focus { surface })
        );
        shell.on_key(Key::Character('x'));
        assert_eq!(shell.input(), "");
        assert!(shell.take_client_action().is_none());
        assert!(shell.client_focus_result(surface, true));
        assert!(
            matches!(shell.take_client_action(),Some(client_interaction::Action::Pointer{x,y,buttons:1,..}) if x==b.width/2*64/b.width&&y==b.height/2*64/b.height)
        );
        shell.on_pointer(Pointer::Up);
        assert!(matches!(
            shell.take_client_action(),
            Some(client_interaction::Action::Pointer { buttons: 0, .. })
        ));
        shell.on_pointer(Pointer::Move(b.x + 2, b.y + 2));
        assert!(matches!(
            shell.take_client_action(),
            Some(client_interaction::Action::Pointer { buttons: 0, .. })
        ));
        shell.on_key(Key::Enter);
        assert!(matches!(
            shell.take_client_action(),
            Some(client_interaction::Action::Key {
                key: Key::Enter,
                ..
            })
        ));
        assert_eq!(shell.input(), "");
        shell.clear_client_surface(0, 4).unwrap();
        assert!(!shell.client_context_active());
    }
    #[test]
    fn physical_packets_route_all_masks_once_without_refocus_or_stale_moves() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(
                0,
                4,
                1,
                8,
                "client",
                64,
                64,
                64,
                PixelFormat::Rgb,
                &[0; 64 * 64 * 4],
            )
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let b = shell.client_content_bounds(0).unwrap();
        let surface = client_interaction::SurfaceRef {
            slot: 0,
            generation: 4,
        };
        let (x, y) = (b.x + 10, b.y + 10);
        for mask in [2, 4, 6, 0] {
            shell.on_pointer_packet(x, y, mask);
        }
        assert!(!shell.client_context_active());
        assert!(shell.take_client_action().is_none());
        shell.on_pointer_packet(x, y, 3);
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::Focus { surface })
        );
        shell.on_pointer_packet(x + 1, y, 2); // left released while right stays held
        assert!(shell.client_focus_result(surface, true));
        for mask in [3, 2] {
            assert!(
                matches!(shell.take_client_action(), Some(client_interaction::Action::Pointer {buttons,..}) if buttons==mask)
            );
        }
        assert!(shell.take_client_action().is_none());
        for mask in [0, 1, 3, 7, 6, 4, 5, 0] {
            shell.on_pointer_packet(x + 2, y, mask);
            assert!(
                matches!(shell.take_client_action(), Some(client_interaction::Action::Pointer {buttons,..}) if buttons==u32::from(mask))
            );
            assert!(
                shell.take_client_action().is_none(),
                "one packet must not synthesize an old-mask motion or refocus"
            );
            assert_eq!(shell.client_focused(), Some(surface));
        }
    }
    #[test]
    fn right_middle_packets_never_activate_client_close_or_trusted_modal() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 4, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        let b = shell.client_content_bounds(0).unwrap();
        let chrome = shell.client_previews.bounds(0, 640, 480).unwrap();
        // Legacy left-only close hitbox must not see a synthetic right Down.
        for mask in [2, 6, 4, 0] {
            shell.on_pointer_packet(chrome.x + chrome.width - 10, chrome.y + 10, mask);
        }
        assert!(shell.take_client_action().is_none());
        assert!(!shell.client_context_active());
        shell.show_file_portal(5, 8, "org.next.actual", false);
        for mask in [2, 6, 4, 0, 1, 0] {
            shell.on_pointer_packet(b.x + 2, b.y + 2, mask);
        }
        assert!(shell.client_focused().is_none());
        assert!(!shell.client_context_active());
        while let Some(action) = shell.take_client_action() {
            assert_eq!(action, client_interaction::Action::Suspend);
        }
        assert!(shell.take_file_portal_action().is_none());
        assert!(shell.file_portal_active());
    }
    #[test]
    fn client_overflow_and_denied_ack_cannot_type_or_close_underlying_terminal() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let b = shell.client_content_bounds(0).unwrap();
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 1,
        };
        shell.on_pointer(Pointer::Down(b.x + 1, b.y + 1));
        shell.take_client_action();
        shell.client_focus_result(id, false);
        shell.on_key(Key::Character('x'));
        shell.on_key(Key::Enter);
        assert_eq!(shell.input(), "");
        assert!(shell.client_context_active());
        shell.on_key(Key::CloseWindow);
        assert_ne!(
            shell.desktop.window(App::Terminal).state,
            WindowState::Closed
        );
        shell.on_pointer(Pointer::Down(b.x + 1, b.y + 1));
        while shell.take_client_action().is_some() {}
        shell.client_focus_result(id, true);
        shell.take_client_action();
        for _ in 0..9 {
            shell.on_key(Key::Character('x'));
        }
        assert!(shell.client_focused().is_none());
        assert!(shell.client_context_active());
        shell.on_key(Key::Enter);
        assert_eq!(shell.input(), "");
        shell.on_key(Key::CloseWindow);
        assert_ne!(
            shell.desktop.window(App::Terminal).state,
            WindowState::Closed
        );
    }
    #[test]
    fn trusted_modal_emerging_after_gesture_revokes_client_and_preserves_prompt() {
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let b = shell.client_content_bounds(0).unwrap();
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 1,
        };
        shell.on_pointer(Pointer::Down(b.x + 1, b.y + 1));
        shell.take_client_action();
        shell.client_focus_result(id, true);
        shell.take_client_action();
        shell.show_file_portal(5, 8, "org.next.actual", false);
        assert!(!shell.client_input_allowed());
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::Suspend)
        );
        assert!(!shell.client_context_active());
        shell.on_key(Key::Enter);
        assert!(matches!(
            shell.take_file_portal_action(),
            Some(FilePortalAction::Deny { request: 5 })
        ));
    }
    #[test]
    fn client_focus_ring_is_visible_and_chrome_focus_routes_no_pointer() {
        extern crate std;
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let body = shell.client_content_bounds(0).unwrap();
        let card = shell.client_previews.bounds(0, 640, 480).unwrap();
        let id = client_interaction::SurfaceRef {
            slot: 0,
            generation: 1,
        };
        shell.on_pointer(Pointer::Down(body.x + 1, body.y + 1));
        shell.take_client_action();
        shell.client_focus_result(id, true);
        shell.take_client_action();
        let mut pixels = std::vec![0;640*480];
        {
            let mut c = Canvas::new(&mut pixels, 640, 480, 640, PixelFormat::Bgr).unwrap();
            shell.render(&mut c);
        }
        assert_eq!(
            pixels[card.y as usize * 640 + (card.x + card.width / 2) as usize],
            theme::ACCENT
        );
        shell.on_pointer(Pointer::Down(card.x + 20, card.y + 20));
        assert!(shell.client_context_active());
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::Suspend)
        );
        assert_eq!(
            shell.take_client_action(),
            Some(client_interaction::Action::FocusChrome { surface: id })
        );
        assert!(shell.client_focus_result(id, true));
        assert!(shell.take_client_action().is_none());
    }
    #[test]
    fn notification_modal_consumes_window_switch_without_background_focus_change() {
        let mut shell = Shell::new();
        shell.open_app(App::Files);
        let focus = shell.desktop.focused();
        shell.toggle_notifications();
        shell.on_key(Key::WindowNext);
        shell.on_key(Key::WindowPrevious);
        assert!(shell.notifications_open());
        assert_eq!(shell.desktop.focused(), focus);
    }
    #[test]
    fn native_close_gestures_are_negotiated_advisory_and_do_not_close_terminal() {
        use client_interaction::{Action as ClientAction, SurfaceRef};
        use client_surface::CloseStatus;
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        raise_client_fixture(&mut shell, 0);
        let id = SurfaceRef {
            slot: 0,
            generation: 1,
        };
        let body = shell.client_content_bounds(0).unwrap();
        shell.on_pointer(Pointer::Down(body.x + 1, body.y + 1));
        shell.take_client_action();
        shell.client_focus_result(id, true);
        shell.take_client_action();
        shell.on_key(Key::CloseWindow);
        assert_eq!(shell.take_client_action(), None);
        assert_ne!(
            shell.desktop.window(App::Terminal).state,
            desktop::WindowState::Closed
        );
        assert!(shell.set_client_close_status(id, CloseStatus::Ready));
        shell.on_key(Key::CloseWindow);
        shell.on_key(Key::CloseWindow);
        assert_eq!(
            shell.take_client_action(),
            Some(ClientAction::RequestClose { surface: id })
        );
        assert_eq!(shell.take_client_action(), None);
        assert!(shell.set_client_close_status(id, CloseStatus::Requested));
        shell.on_key(Key::CloseWindow);
        assert_eq!(shell.take_client_action(), None);
        assert!(shell.set_client_close_status(id, CloseStatus::Unresponsive));
        shell.on_key(Key::CloseWindow);
        assert_eq!(shell.take_client_action(), None);
        assert!(shell.set_client_close_status(id, CloseStatus::Refused));
        let close = shell.client_close_button_bounds(0).unwrap();
        shell.on_pointer(Pointer::Down(close.x + 2, close.y + 2));
        assert_eq!(
            shell.take_client_action(),
            Some(ClientAction::RequestClose { surface: id })
        );
        assert!(shell.set_client_close_status(id, CloseStatus::Closing));
        assert!(shell.client_content_bounds(0).is_some());
        assert_ne!(
            shell.desktop.window(App::Terminal).state,
            desktop::WindowState::Closed
        );
        shell.clear_client_surface(0, 1).unwrap();
        assert!(!shell.set_client_close_status(id, CloseStatus::Ready));
        shell
            .submit_client_surface(0, 2, 1, 9, "new-client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        assert!(!shell.set_client_close_status(id, CloseStatus::Refused));
        assert_eq!(
            shell.client_previews.close_status(SurfaceRef {
                slot: 0,
                generation: 2
            }),
            Some(CloseStatus::Unsupported)
        );
    }
    #[test]
    fn native_close_chrome_cannot_bypass_trusted_portal() {
        use client_interaction::SurfaceRef;
        let mut shell = Shell::new();
        shell.set_viewport(640, 480);
        shell
            .submit_client_surface(0, 1, 1, 8, "client", 1, 1, 1, PixelFormat::Rgb, &[0; 4])
            .unwrap();
        let id = SurfaceRef {
            slot: 0,
            generation: 1,
        };
        shell.set_client_close_status(id, client_surface::CloseStatus::Ready);
        let close = shell.client_close_button_bounds(0).unwrap();
        shell.show_file_portal(77, 8, "client", false);
        shell.on_pointer(Pointer::Down(close.x + 2, close.y + 2));
        shell.on_key(Key::CloseWindow);
        assert!(shell.file_portal_active());
        assert_eq!(shell.take_client_action(), None);
    }
    #[test]
    fn undo_uses_authoritative_receipt_preserves_rows_on_failure_and_disabled_click_does_nothing() {
        use trash_ui::{UndoKind, UndoReceipt};
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        let path = files_actions::Path::new("/native/a").unwrap();
        let item = trash_ui::Item {
            id: 1,
            revision: 3,
            original: path,
            size: 4,
        };
        shell
            .set_trash_listing(trash_ui::Listing {
                items: [Some(item), None],
                revision: 3,
            })
            .unwrap();
        shell.show_trash();
        let undo = shell.trash_page_button_bounds(3).unwrap();
        shell.on_pointer(Pointer::Down(undo.x + 2, undo.y + 2));
        assert!(!shell.trash_dialog_active());
        assert_eq!(shell.take_trash_undo_action(), None);
        shell
            .set_trash_undo_receipt(Some(UndoReceipt {
                generation: 3,
                kind: UndoKind::Move,
                path,
            }))
            .unwrap();
        shell.on_pointer(Pointer::Down(undo.x + 2, undo.y + 2));
        let action = shell.take_trash_undo_action().unwrap();
        assert_eq!(action.generation, 3);
        assert_eq!(shell.trash_state().listing().items[0], Some(item));
        shell.on_key(Key::Delete);
        assert!(!shell.trash_dialog_active());
        shell
            .trash_undo_result(action.request, Err(files_actions::BackendError::Conflict))
            .unwrap();
        assert!(shell.trash_undo_receipt().is_some());
        assert_eq!(shell.trash_state().listing().items[0], Some(item));
        shell.on_key(Key::Character('u'));
        let retry = shell.take_trash_undo_action().unwrap();
        shell.trash_undo_result(retry.request, Ok(())).unwrap();
        assert!(shell.trash_undo_receipt().is_none());
        assert_eq!(shell.trash_state().listing().items[0], Some(item));
        assert_eq!(shell.file_pending_selection, Some(path));
        shell
            .set_trash_listing(trash_ui::Listing {
                items: [None, None],
                revision: 4,
            })
            .unwrap();
        assert!(shell.trash_state().listing().items[0].is_none());
    }
    #[test]
    fn accessibility_snapshot_tracks_real_tab_focus_and_deduplicates_redraws() {
        use accessibility::{EventKind, Snapshot};
        let mut shell = Shell::new();
        let mut snapshot = Snapshot::empty();
        shell.set_viewport(1024, 768);
        shell
            .load_appearance(appearance::Loaded {
                preferences: appearance::Preferences::default(),
                origin: appearance::Origin::MissingDefault,
            })
            .unwrap();
        shell.open_app(App::Settings);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert!(snapshot.complete);
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Theme"
        );
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.name() == "Theme")
                .unwrap()
                .value(),
            "Light"
        );
        while shell.take_accessibility_event().is_some() {}
        let generation = snapshot.generation;
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.generation, generation);
        assert_eq!(shell.take_accessibility_event(), None);
        shell.on_key(Key::Tab);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Accent color"
        );
        assert!(snapshot
            .nodes()
            .iter()
            .find(|n| n.state.focused)
            .unwrap()
            .parent
            .is_some());
        let mut focus_event = false;
        while let Some(e) = shell.take_accessibility_event() {
            focus_event |= e.kind == EventKind::FocusChanged;
        }
        assert!(focus_event);
        shell.on_key(Key::BackTab);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Theme"
        );
    }
    #[test]
    fn accessibility_modal_contains_only_trusted_controls_and_cancel_restores_actual_panel() {
        use accessibility::Snapshot;
        let mut shell = Shell::new();
        let mut snapshot = Snapshot::empty();
        shell.set_viewport(1024, 768);
        shell.open_app(App::Files);
        assert!(shell.set_file(1, "/native/a", "body"));
        shell.show_file_portal(77, 8, "org.next.long.authenticated.package", true);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.context, 20);
        assert_eq!(snapshot.token, 77);
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Cancel"
        );
        assert_eq!(
            snapshot.nodes()[0].value(),
            "org.next.long.authenticated.package"
        );
        assert!(!snapshot.nodes().iter().any(|n| n.name() == "Create file"));
        shell.on_key(Key::BackTab);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Allow read and write"
        );
        shell.on_key(Key::Escape);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_ne!(snapshot.context, 20);
        assert!(snapshot.nodes().iter().any(|n| n.name() == "/native/a"));
        shell.on_key(Key::Character('n'));
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.context, 22);
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "File path"
        );
        shell.on_key(Key::Tab);
        shell.on_key(Key::Tab);
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Create file"
        );
        assert!(snapshot.nodes().iter().all(|n| n.id.context == 22));
    }
    #[test]
    fn files_query_applies_only_confirmed_paths_and_never_filters_trusted_portal() {
        use files_query::{Sort, Stats};
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        shell.open_app(App::Files);
        assert!(shell.set_file(0, "NEXT.TXT", "boot"));
        assert!(shell.set_file(1, "/native/a", "alpha"));
        assert!(shell.set_file(2, "/native/b", "beta"));
        let a = files_actions::Path::new("/native/a").unwrap();
        let b = files_actions::Path::new("/native/b").unwrap();
        shell.on_key(Key::Character('/'));
        assert!(shell.file_query_dialog_active());
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        assert_eq!(shell.file_query().desired().sort, Sort::Size);
        shell.on_key(Key::BackTab);
        shell.on_key(Key::Enter);
        let action = shell.take_file_query_action().unwrap();
        assert_eq!(shell.take_file_query_action(), None);
        assert_eq!(shell.file_view_index(1), Some(1));
        shell
            .file_query_result(
                action.request,
                Ok(Stats {
                    revision: 3,
                    total: 2,
                    matched: 2,
                }),
                &[b, a],
            )
            .unwrap();
        assert!(!shell.file_query_dialog_active());
        assert_eq!(shell.file_view_index(1), Some(2));
        assert_eq!(shell.file_view_index(2), Some(1));
        shell.on_key(Key::Down);
        assert_eq!(shell.file_selection, 2);
        shell
            .refresh_file_query(
                Stats {
                    revision: 4,
                    total: 2,
                    matched: 1,
                },
                &[a],
            )
            .unwrap();
        assert_eq!(shell.file_view_index(1), Some(1));
        assert_eq!(shell.file_view_index(2), None);
        assert!(shell.files[2].available);
        shell.show_file_portal(9, 8, "client", false);
        assert_eq!(shell.portal_count(), 2);
        shell.on_key(Key::Escape);
        shell.on_key(Key::Character('/'));
        shell.on_key(Key::Character('z'));
        shell.on_key(Key::Enter);
        let action = shell.take_file_query_action().unwrap();
        shell
            .file_query_result(
                action.request,
                Err(files_actions::BackendError::Conflict),
                &[],
            )
            .unwrap();
        assert!(shell.file_query_dialog_active());
        assert_eq!(shell.file_query().desired().text(), "z");
        assert_eq!(shell.file_view_index(1), Some(1));
        assert_eq!(
            shell.file_query_result(
                action.request,
                Ok(Stats {
                    revision: 5,
                    total: 2,
                    matched: 2
                }),
                &[a, b]
            ),
            Err(files_query::Error::StaleCompletion)
        );
    }
    #[test]
    fn files_query_keyboard_and_pointer_share_controls_and_snapshot_has_exact_values() {
        let mut shell = Shell::new();
        shell.set_viewport(1024, 768);
        shell.open_app(App::Files);
        let filter = shell.file_action_button_bounds(5).unwrap();
        shell.on_pointer(Pointer::Down(filter.x + 2, filter.y + 2));
        assert!(shell.file_query_dialog_active());
        shell.on_key(Key::Character('a'));
        shell.on_key(Key::Backspace);
        shell.on_key(Key::Character('b'));
        shell.on_key(Key::Tab);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Tab);
        shell.on_key(Key::Enter);
        assert_eq!(
            shell.file_query().desired().search,
            files_query::Search::PathAndContent
        );
        let mut snapshot = accessibility::Snapshot::empty();
        shell.write_accessibility_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.context, 27);
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.name() == "File query")
                .unwrap()
                .value(),
            "b"
        );
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|n| n.state.focused)
                .unwrap()
                .name(),
            "Search scope"
        );
        assert!(!shell.client_input_allowed());
        shell.on_key(Key::CloseWindow);
        assert!(shell.file_query_dialog_active());
        let cancel = shell.file_query_control_bounds(4).unwrap();
        shell.on_pointer(Pointer::Down(cancel.x + 2, cancel.y + 2));
        assert!(!shell.file_query_dialog_active());
        assert_eq!(shell.file_query().applied().text(), "");
    }
}
