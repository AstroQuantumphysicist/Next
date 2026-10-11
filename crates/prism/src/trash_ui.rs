//! Bounded trusted Trash UI intents; only backend snapshots change visible items.
use crate::files_actions::{BackendError, Path, Snapshot};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UndoKind {
    Move,
    Restore,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UndoReceipt {
    pub generation: u64,
    pub kind: UndoKind,
    pub path: Path,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UndoAction {
    pub request: u64,
    pub generation: u64,
}
pub const CAPACITY: usize = 2;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Item {
    pub id: u64,
    pub revision: u64,
    pub original: Path,
    pub size: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Listing {
    pub items: [Option<Item>; CAPACITY],
    pub revision: u64,
}
impl Listing {
    pub const fn empty() -> Self {
        Self {
            items: [None; CAPACITY],
            revision: 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    Move { source: Snapshot },
    Restore { item: Item, destination: Path },
    Purge { item: Item },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action {
    pub request: u64,
    pub intent: Intent,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Idle,
    ConfirmMove,
    RestorePath,
    ConfirmPurge,
    Saving,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Busy,
    InvalidSnapshot,
    InvalidPath,
    Unavailable,
    StaleCompletion,
    RequestExhausted,
}
pub struct Model {
    listing: Listing,
    loaded: bool,
    selected: Option<u64>,
    phase: Phase,
    intent: Option<Intent>,
    input: [u8; 32],
    input_len: usize,
    approve: bool,
    error: Option<BackendError>,
    next_request: u64,
    pending: Option<u64>,
    taken: bool,
    action: Option<Action>,
    resume: Phase,
}
impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}
impl Model {
    pub const fn new() -> Self {
        Self {
            listing: Listing::empty(),
            loaded: false,
            selected: None,
            phase: Phase::Idle,
            intent: None,
            input: [0; 32],
            input_len: 0,
            approve: false,
            error: None,
            next_request: 1,
            pending: None,
            taken: false,
            action: None,
            resume: Phase::Idle,
        }
    }
    pub fn listing(&self) -> &Listing {
        &self.listing
    }
    pub fn loaded(&self) -> bool {
        self.loaded
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn approved(&self) -> bool {
        self.approve
    }
    pub fn error(&self) -> Option<BackendError> {
        self.error
    }
    pub fn input(&self) -> &str {
        core::str::from_utf8(&self.input[..self.input_len]).unwrap_or("")
    }
    pub fn intent(&self) -> Option<Intent> {
        self.intent
    }
    pub fn selected(&self) -> Option<Item> {
        self.listing
            .items
            .iter()
            .flatten()
            .copied()
            .find(|i| Some(i.id) == self.selected)
    }
    pub fn set_listing(&mut self, listing: Listing) -> Result<(), Error> {
        if self.loaded && listing.revision < self.listing.revision {
            return Err(Error::InvalidSnapshot);
        }
        let mut ids = [0; CAPACITY];
        for (index, item) in listing.items.iter().enumerate() {
            if let Some(item) = item {
                if item.id == 0
                    || item.revision != listing.revision
                    || item.size > 4096
                    || !native(item.original)
                    || ids.contains(&item.id)
                {
                    return Err(Error::InvalidSnapshot);
                }
                ids[index] = item.id;
            }
        }
        self.listing = listing;
        self.loaded = true;
        if self.phase == Phase::Idle {
            self.error = None;
        }
        if self.selected().is_none() {
            self.selected = listing.items.iter().flatten().next().map(|i| i.id)
        }
        Ok(())
    }
    pub fn load_failed(&mut self, error: BackendError) {
        self.error = Some(error)
    }
    pub fn select(&mut self, id: u64) -> Result<(), Error> {
        if self.phase != Phase::Idle {
            return Err(Error::Busy);
        }
        if !self.listing.items.iter().flatten().any(|i| i.id == id) {
            return Err(Error::InvalidSnapshot);
        }
        self.selected = Some(id);
        Ok(())
    }
    pub fn cycle(&mut self, reverse: bool) {
        if self.phase != Phase::Idle {
            return;
        }
        let ids: [Option<u64>; 2] = self.listing.items.map(|i| i.map(|i| i.id));
        let start = ids.iter().position(|id| *id == self.selected).unwrap_or(0);
        for offset in 1..=CAPACITY {
            let index = if reverse {
                (start + CAPACITY - offset) % CAPACITY
            } else {
                (start + offset) % CAPACITY
            };
            if let Some(id) = ids[index] {
                self.selected = Some(id);
                return;
            }
        }
    }
    fn begin(&mut self, intent: Intent, phase: Phase) -> Result<(), Error> {
        if self.phase != Phase::Idle {
            return Err(Error::Busy);
        }
        self.phase = phase;
        self.intent = Some(intent);
        self.approve = false;
        self.error = None;
        self.input_len = 0;
        Ok(())
    }
    pub fn begin_move(&mut self, source: Snapshot) -> Result<(), Error> {
        if !native(source.path) {
            return Err(Error::InvalidPath);
        }
        self.begin(Intent::Move { source }, Phase::ConfirmMove)
    }
    pub fn begin_restore(&mut self) -> Result<(), Error> {
        let item = self.selected().ok_or(Error::Unavailable)?;
        self.begin(
            Intent::Restore {
                item,
                destination: item.original,
            },
            Phase::RestorePath,
        )?;
        self.set_input(item.original.as_str())
    }
    pub fn begin_purge(&mut self) -> Result<(), Error> {
        let item = self.selected().ok_or(Error::Unavailable)?;
        self.begin(Intent::Purge { item }, Phase::ConfirmPurge)
    }
    pub fn set_input(&mut self, text: &str) -> Result<(), Error> {
        if self.phase != Phase::RestorePath {
            return Err(Error::Busy);
        }
        if text.len() > 31 || !text.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::InvalidPath);
        }
        self.input[..text.len()].copy_from_slice(text.as_bytes());
        self.input_len = text.len();
        self.error = None;
        Ok(())
    }
    pub fn insert(&mut self, c: char) -> Result<(), Error> {
        if self.phase != Phase::RestorePath {
            return Err(Error::Busy);
        }
        if !c.is_ascii() || c.is_ascii_control() || c == ' ' || self.input_len == 31 {
            return Err(Error::InvalidPath);
        }
        self.input[self.input_len] = c as u8;
        self.input_len += 1;
        self.error = None;
        Ok(())
    }
    pub fn backspace(&mut self) {
        if self.phase == Phase::RestorePath {
            self.input_len = self.input_len.saturating_sub(1);
            self.error = None
        }
    }
    pub fn choose_approve(&mut self, approve: bool) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::ConfirmMove | Phase::ConfirmPurge | Phase::RestorePath
        ) {
            return Err(Error::Busy);
        }
        self.approve = approve;
        Ok(())
    }
    pub fn cancel(&mut self) -> Result<(), Error> {
        if self.phase == Phase::Saving {
            return Err(Error::Busy);
        }
        self.phase = Phase::Idle;
        self.intent = None;
        self.error = None;
        self.approve = false;
        Ok(())
    }
    pub fn activate(&mut self) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::ConfirmMove | Phase::ConfirmPurge | Phase::RestorePath
        ) {
            return Err(Error::Busy);
        }
        if !self.approve {
            return self.cancel();
        }
        let mut intent = self.intent.ok_or(Error::Unavailable)?;
        if let Intent::Restore { item, .. } = intent {
            let destination = Path::new(self.input()).map_err(|_| {
                self.error = Some(BackendError::InvalidPath);
                Error::InvalidPath
            })?;
            if !native(destination) {
                self.error = Some(BackendError::InvalidPath);
                return Err(Error::InvalidPath);
            }
            intent = Intent::Restore { item, destination };
        }
        let next = self
            .next_request
            .checked_add(1)
            .ok_or(Error::RequestExhausted)?;
        let request = self.next_request;
        self.next_request = next;
        self.pending = Some(request);
        self.taken = false;
        self.action = Some(Action { request, intent });
        self.intent = Some(intent);
        self.resume = self.phase;
        self.phase = Phase::Saving;
        self.error = None;
        Ok(())
    }
    pub fn take_action(&mut self) -> Option<Action> {
        let action = self.action.take()?;
        self.taken = true;
        Some(action)
    }
    pub fn complete(
        &mut self,
        request: u64,
        result: Result<(), BackendError>,
    ) -> Result<(), Error> {
        if self.phase != Phase::Saving || self.pending != Some(request) || !self.taken {
            return Err(Error::StaleCompletion);
        }
        self.pending = None;
        self.taken = false;
        match result {
            Ok(()) => {
                self.phase = Phase::Idle;
                self.intent = None;
                self.error = None
            }
            Err(error) => {
                self.phase = self.resume;
                self.error = Some(error);
                self.approve = false
            }
        }
        Ok(())
    }
}
fn native(path: Path) -> bool {
    path.as_str().starts_with("/native/")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item(id: u64) -> Item {
        Item {
            id,
            revision: 7,
            original: Path::new("/native/doc").unwrap(),
            size: 4,
        }
    }
    #[test]
    fn authoritative_list_and_default_cancel_do_not_mutate_items() {
        let mut m = Model::new();
        m.set_listing(Listing {
            items: [Some(item(1)), None],
            revision: 7,
        })
        .unwrap();
        m.begin_purge().unwrap();
        m.activate().unwrap();
        assert!(m.take_action().is_none());
        assert_eq!(m.selected(), Some(item(1)));
        m.begin_restore().unwrap();
        m.choose_approve(true).unwrap();
        m.activate().unwrap();
        let a = m.take_action().unwrap();
        assert_eq!(
            m.complete(a.request + 1, Ok(())),
            Err(Error::StaleCompletion)
        );
        m.complete(a.request, Ok(())).unwrap();
        assert_eq!(m.selected(), Some(item(1))); // retained until actual backend listing
        m.set_listing(Listing {
            items: [None, None],
            revision: 8,
        })
        .unwrap();
        assert_eq!(m.selected(), None);
    }
    #[test]
    fn collision_failure_retains_snapshot_input_and_rejects_untaken_completion() {
        let mut m = Model::new();
        m.set_listing(Listing {
            items: [Some(item(1)), None],
            revision: 7,
        })
        .unwrap();
        m.begin_restore().unwrap();
        m.set_input("/native/other").unwrap();
        m.choose_approve(true).unwrap();
        m.activate().unwrap();
        assert_eq!(m.complete(1, Ok(())), Err(Error::StaleCompletion));
        let a = m.take_action().unwrap();
        m.complete(a.request, Err(BackendError::Exists)).unwrap();
        assert_eq!(m.input(), "/native/other");
        assert_eq!(m.selected(), Some(item(1)));
        assert!(!m.approved());
        m.choose_approve(true).unwrap();
        m.activate().unwrap();
        assert_eq!(m.take_action().unwrap().request, 2);
    }
    #[test]
    fn malformed_or_stale_snapshots_and_outside_paths_fail_before_actions() {
        let mut m = Model::new();
        assert_eq!(
            m.set_listing(Listing {
                items: [Some(item(1)), Some(item(1))],
                revision: 7
            }),
            Err(Error::InvalidSnapshot)
        );
        assert!(!m.loaded());
        m.set_listing(Listing {
            items: [Some(item(1)), None],
            revision: 7,
        })
        .unwrap();
        assert_eq!(m.set_listing(Listing::empty()), Err(Error::InvalidSnapshot));
        assert_eq!(
            m.begin_move(Snapshot {
                path: Path::new("/prefs/appearance").unwrap(),
                revision: 7
            }),
            Err(Error::InvalidPath)
        );
        m.begin_restore().unwrap();
        m.set_input("/trash/forged").unwrap();
        m.choose_approve(true).unwrap();
        assert_eq!(m.activate(), Err(Error::InvalidPath));
        assert_eq!(m.error(), Some(BackendError::InvalidPath));
        assert_eq!(m.input(), "/trash/forged");
        assert!(m.take_action().is_none());
    }
    #[test]
    fn move_purge_ids_are_checked_and_saving_cannot_cancel() {
        let mut m = Model::new();
        m.begin_move(Snapshot {
            path: item(1).original,
            revision: 7,
        })
        .unwrap();
        m.choose_approve(true).unwrap();
        m.activate().unwrap();
        assert_eq!(m.cancel(), Err(Error::Busy));
        let a = m.take_action().unwrap();
        m.complete(a.request, Err(BackendError::Full)).unwrap();
        assert_eq!(m.error(), Some(BackendError::Full));
        assert_eq!(m.phase(), Phase::ConfirmMove);
        m.next_request = u64::MAX;
        m.choose_approve(true).unwrap();
        assert_eq!(m.activate(), Err(Error::RequestExhausted));
        assert!(m.take_action().is_none());
    }
}
