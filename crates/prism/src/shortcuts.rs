//! User-facing keyboard reference for the currently implemented Prism shell.

use crate::Key;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    pub name: &'static str,
    pub keys: &'static str,
}

pub const ENTRIES: [Entry; 10] = [
    Entry {
        name: "Open or close Home",
        keys: "Super or F10",
    },
    Entry {
        name: "Search apps and files",
        keys: "Super + Space",
    },
    Entry {
        name: "Open Files, Settings, or Terminal",
        keys: "Super + E, I, or Enter",
    },
    Entry {
        name: "Switch windows",
        keys: "Alt+Tab / Shift+Alt+Tab",
    },
    Entry {
        name: "Close the focused window",
        keys: "Alt + F4",
    },
    Entry {
        name: "Snap or restore a window",
        keys: "Super + arrow keys",
    },
    Entry {
        name: "Move focus in controls",
        keys: "Tab / Shift+Tab",
    },
    Entry {
        name: "Move through items and activate",
        keys: "Arrow keys, then Enter",
    },
    Entry {
        name: "Cancel or close a dialog",
        keys: "Escape",
    },
    Entry {
        name: "Files: find/create/rename/copy",
        keys: "/ N R C Del T U",
    },
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Model {
    open: bool,
    selected: usize,
}

impl Model {
    pub const fn new() -> Self {
        Self {
            open: false,
            selected: 0,
        }
    }

    pub const fn open(&self) -> bool {
        self.open
    }

    pub const fn selected(&self) -> usize {
        self.selected
    }

    const fn control_count() -> usize {
        ENTRIES.len() + 1
    }

    pub fn show(&mut self) {
        self.open = true;
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn select(&mut self, index: usize) -> bool {
        if !self.open || index >= Self::control_count() {
            return false;
        }
        self.selected = index;
        true
    }

    pub fn on_key(&mut self, key: Key) {
        if !self.open {
            return;
        }
        match key {
            Key::Escape | Key::Menu => self.close(),
            Key::Tab | Key::Down => self.selected = (self.selected + 1) % Self::control_count(),
            Key::BackTab | Key::Up => {
                self.selected = (self.selected + Self::control_count() - 1) % Self::control_count()
            }
            Key::Enter if self.selected == ENTRIES.len() => self.close(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_reference_ignores_navigation_and_open_starts_at_first_entry() {
        let mut model = Model::new();
        model.on_key(Key::Down);
        assert!(!model.open());
        model.show();
        assert_eq!(model.selected(), 0);
        model.on_key(Key::BackTab);
        assert_eq!(model.selected(), ENTRIES.len());
    }

    #[test]
    fn navigation_wraps_and_escape_closes_reference() {
        let mut model = Model::new();
        model.show();
        model.on_key(Key::Up);
        assert_eq!(model.selected(), ENTRIES.len());
        model.on_key(Key::Tab);
        assert_eq!(model.selected(), 0);
        for _ in 0..ENTRIES.len() {
            model.on_key(Key::Tab);
        }
        assert_eq!(model.selected(), ENTRIES.len());
        model.on_key(Key::Enter);
        assert!(!model.open());
    }

    #[test]
    fn pointer_selection_is_bounded_and_requires_open_reference() {
        let mut model = Model::new();
        assert!(!model.select(2));
        model.show();
        assert!(model.select(2));
        assert_eq!(model.selected(), 2);
        assert!(model.select(ENTRIES.len()));
        assert_eq!(model.selected(), ENTRIES.len());
        assert!(!model.select(ENTRIES.len() + 1));
    }
}
