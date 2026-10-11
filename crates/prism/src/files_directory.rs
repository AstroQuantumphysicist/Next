//! Work-only bounded directory navigation candidate; no live Shell integration.
use crate::files_actions::Path;
pub const CAPACITY: usize = 8;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    File,
    Directory,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FolderSnapshot {
    pub path: Path,
    pub revision: u64,
    pub id: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    pub path: Path,
    pub kind: Kind,
    pub size: u32,
    pub id: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Listing {
    pub folder: FolderSnapshot,
    pub entries: [Option<Entry>; CAPACITY],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    ReadOnly,
    Exists,
    NotFound,
    Conflict,
    Busy,
    Full,
    Io,
    InvalidPath,
    MissingParent,
    NotDirectory,
    NotEmpty,
    Unavailable,
}
impl BackendError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::ReadOnly => "This store is read-only. No folder was changed.",
            Self::Exists => "A folder already has that name. Choose another path.",
            Self::NotFound => "The folder is no longer available. Refresh Files.",
            Self::Conflict => "The folder changed. Refresh Files before retrying.",
            Self::Busy => "The folder store is busy. Try again.",
            Self::Full => "The store has no free slots. Remove an unneeded item first.",
            Self::Io => "Storage could not confirm the operation.",
            Self::InvalidPath => "Use a valid absolute path under /native.",
            Self::MissingParent => "The destination parent folder does not exist.",
            Self::NotDirectory => "A path component is not a folder.",
            Self::NotEmpty => "Only an empty folder can be moved or removed.",
            Self::Unavailable => "The folder store is unavailable.",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Busy,
    InvalidPath,
    InvalidListing,
    Unavailable,
    Exhausted,
    StaleCompletion,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    Navigate {
        path: Path,
    },
    CreateFolder {
        parent: FolderSnapshot,
        path: Path,
    },
    /// Move an empty directory to a new name and/or parent. The backend checks
    /// the exact source identity and destination ancestry under one transaction.
    MoveFolder {
        source: FolderSnapshot,
        destination: Path,
    },
    /// Removes only the selected empty directory after explicit confirmation.
    RemoveFolder {
        source: FolderSnapshot,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action {
    pub request: u64,
    pub intent: Intent,
}
#[derive(Clone, Copy)]
struct Text {
    bytes: [u8; 31],
    len: usize,
}
impl Text {
    const fn new() -> Self {
        Self {
            bytes: [0; 31],
            len: 0,
        }
    }
    fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap()
    }
    fn push(&mut self, byte: u8) -> Result<(), Error> {
        if self.len == self.bytes.len()
            || !byte.is_ascii()
            || !byte.is_ascii_alphanumeric() && !matches!(byte, b'/' | b'.' | b'_' | b'-')
        {
            return Err(Error::InvalidPath);
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        Ok(())
    }
    fn backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.bytes[self.len] = 0;
        }
    }
    fn set(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > 31 || !text.bytes().all(|b| b.is_ascii() && !b.is_ascii_control()) {
            return Err(Error::InvalidPath);
        }
        self.bytes = [0; 31];
        self.bytes[..text.len()].copy_from_slice(text.as_bytes());
        self.len = text.len();
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogKind {
    CreateFolder,
    MoveFolder,
    RemoveFolder,
}
#[derive(Clone, Copy)]
enum Dialog {
    Create(FolderSnapshot),
    Move(FolderSnapshot),
    Remove(FolderSnapshot),
}
pub struct Model {
    listing: Option<Listing>,
    selected: Option<usize>,
    address: Text,
    input: Text,
    dialog: Option<Dialog>,
    approve: bool,
    pending: Option<Action>,
    taken: bool,
    next: u64,
    error: Option<BackendError>,
}
fn root() -> Path {
    Path::new("/native").unwrap()
}
fn permitted(path: Path) -> bool {
    path == root() || path.as_str().starts_with("/native/")
}
fn is_descendant(path: Path, ancestor: Path) -> bool {
    path.as_str()
        .strip_prefix(ancestor.as_str())
        .is_some_and(|suffix| suffix.starts_with('/'))
}
fn parent(path: Path) -> Option<Path> {
    let (parent, _) = path.as_str().rsplit_once('/')?;
    Path::new(parent).ok().filter(|p| permitted(*p))
}
fn valid_listing(listing: Listing) -> bool {
    let folder = listing.folder;
    if !permitted(folder.path)
        || folder.revision == 0
        || (folder.path == root()) != (folder.id == 0)
    {
        return false;
    }
    let mut saw_empty = false;
    for (i, entry) in listing.entries.iter().enumerate() {
        let Some(entry) = entry else {
            saw_empty = true;
            continue;
        };
        if saw_empty
            || parent(entry.path) != Some(folder.path)
            || !permitted(entry.path)
            || (entry.kind == Kind::Directory && (entry.id == 0 || entry.size != 0))
            || (entry.kind == Kind::File && entry.id != 0)
        {
            return false;
        }
        if listing.entries[..i].iter().flatten().any(|old| {
            old.path == entry.path
                || entry.kind == Kind::Directory
                    && old.kind == Kind::Directory
                    && old.id == entry.id
        }) {
            return false;
        }
    }
    true
}
impl Model {
    pub const fn new() -> Self {
        Self {
            listing: None,
            selected: None,
            address: Text::new(),
            input: Text::new(),
            dialog: None,
            approve: false,
            pending: None,
            taken: false,
            next: 1,
            error: None,
        }
    }
    pub fn listing(&self) -> Option<Listing> {
        self.listing
    }
    pub fn current_path(&self) -> Option<&str> {
        self.listing
            .as_ref()
            .map(|listing| listing.folder.path.as_str())
    }
    pub fn selected(&self) -> Option<Entry> {
        self.listing?.entries[self.selected?]
    }
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }
    pub fn move_selection(&mut self, forward: bool) -> bool {
        if self.busy() || self.dialog_active() {
            return false;
        }
        let Some(listing) = self.listing else {
            return false;
        };
        let count = listing.entries.iter().flatten().count();
        if count == 0 {
            return false;
        }
        let current = self.selected.unwrap_or(0).min(count - 1);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.selected = Some(next);
        true
    }
    pub fn address(&self) -> &str {
        self.address.text()
    }
    pub fn input(&self) -> &str {
        self.input.text()
    }
    pub fn push_input(&mut self, byte: u8) -> Result<(), Error> {
        if self.busy() || self.dialog.is_none() || matches!(self.dialog, Some(Dialog::Remove(_))) {
            return Err(Error::Unavailable);
        }
        self.input.push(byte)
    }
    pub fn backspace_input(&mut self) -> Result<(), Error> {
        if self.busy() || self.dialog.is_none() || matches!(self.dialog, Some(Dialog::Remove(_))) {
            return Err(Error::Unavailable);
        }
        self.input.backspace();
        Ok(())
    }
    pub fn push_address(&mut self, byte: u8) -> Result<(), Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        self.address.push(byte)
    }
    pub fn backspace_address(&mut self) -> Result<(), Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        self.address.backspace();
        Ok(())
    }
    pub fn error(&self) -> Option<BackendError> {
        self.error
    }
    pub fn dialog_active(&self) -> bool {
        self.dialog.is_some()
    }
    pub fn dialog_kind(&self) -> Option<DialogKind> {
        match self.dialog {
            Some(Dialog::Create(_)) => Some(DialogKind::CreateFolder),
            Some(Dialog::Move(_)) => Some(DialogKind::MoveFolder),
            Some(Dialog::Remove(_)) => Some(DialogKind::RemoveFolder),
            None => None,
        }
    }
    pub fn can_move_selected(&self) -> bool {
        !self.busy()
            && !self.dialog_active()
            && self
                .selected()
                .is_some_and(|entry| entry.kind == Kind::Directory)
    }
    pub fn can_remove_selected(&self) -> bool {
        self.can_move_selected() && self.selected().is_some_and(|entry| entry.path != root())
    }
    pub fn approves(&self) -> bool {
        self.approve
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn set_address(&mut self, text: &str) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        self.address.set(text)
    }
    pub fn set_input(&mut self, text: &str) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        if self.dialog.is_none() {
            return Err(Error::Unavailable);
        }
        self.input.set(text)
    }
    fn enqueue(&mut self, intent: Intent) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        if self.next == u64::MAX {
            return Err(Error::Exhausted);
        }
        self.pending = Some(Action {
            request: self.next,
            intent,
        });
        self.next += 1;
        self.taken = false;
        self.error = None;
        Ok(())
    }
    pub fn navigate(&mut self, path: Path) -> Result<(), Error> {
        if !permitted(path) {
            return Err(Error::InvalidPath);
        }
        if self.dialog_active() {
            return Err(Error::Busy);
        }
        self.enqueue(Intent::Navigate { path })?;
        self.address.set(path.as_str())
    }
    pub fn navigate_address(&mut self) -> Result<(), Error> {
        let path = Path::new(self.address.text()).map_err(|_| Error::InvalidPath)?;
        self.navigate(path)
    }
    pub fn up(&mut self) -> Result<bool, Error> {
        let path = self.listing.ok_or(Error::Unavailable)?.folder.path;
        let Some(parent) = parent(path) else {
            return Ok(false);
        };
        self.navigate(parent)?;
        Ok(true)
    }
    pub fn select(&mut self, index: usize) -> bool {
        if self.busy() || self.dialog_active() {
            return false;
        }
        if self
            .listing
            .is_some_and(|l| l.entries.get(index).is_some_and(Option::is_some))
        {
            self.selected = Some(index);
            true
        } else {
            false
        }
    }
    pub fn open_selected(&mut self) -> Result<Option<Path>, Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        let entry = self.selected().ok_or(Error::Unavailable)?;
        match entry.kind {
            Kind::Directory => {
                self.navigate(entry.path)?;
                Ok(None)
            }
            Kind::File => Ok(Some(entry.path)),
        }
    }
    pub fn begin_create(&mut self) -> Result<(), Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        let folder = self.listing.ok_or(Error::Unavailable)?.folder;
        let mut text = Text::new();
        let n = folder.path.as_str().len();
        if n >= 31 {
            return Err(Error::InvalidPath);
        }
        text.bytes[..n].copy_from_slice(folder.path.as_str().as_bytes());
        text.bytes[n] = b'/';
        text.len = n + 1;
        self.input = text;
        self.dialog = Some(Dialog::Create(folder));
        self.approve = false;
        self.error = None;
        Ok(())
    }
    pub fn begin_move(&mut self) -> Result<(), Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        let entry = self
            .selected()
            .filter(|e| e.kind == Kind::Directory)
            .ok_or(Error::Unavailable)?;
        let listing = self.listing.unwrap();
        self.input.set("/native/")?;
        self.dialog = Some(Dialog::Move(FolderSnapshot {
            path: entry.path,
            id: entry.id,
            revision: listing.folder.revision,
        }));
        self.approve = false;
        self.error = None;
        Ok(())
    }
    pub fn begin_remove(&mut self) -> Result<(), Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        let entry = self
            .selected()
            .filter(|e| e.kind == Kind::Directory)
            .ok_or(Error::Unavailable)?;
        let listing = self.listing.ok_or(Error::Unavailable)?;
        if entry.path == root() {
            return Err(Error::InvalidPath);
        }
        self.input.set(entry.path.as_str())?;
        self.dialog = Some(Dialog::Remove(FolderSnapshot {
            path: entry.path,
            id: entry.id,
            revision: listing.folder.revision,
        }));
        self.approve = false;
        self.error = None;
        Ok(())
    }
    pub fn choose_approve(&mut self, approve: bool) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        if !self.dialog_active() {
            return Err(Error::Unavailable);
        }
        self.approve = approve;
        Ok(())
    }
    pub fn confirm(&mut self) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        let dialog = self.dialog.ok_or(Error::Unavailable)?;
        if !self.approve {
            self.cancel();
            return Ok(());
        }
        let path = Path::new(self.input.text()).map_err(|_| Error::InvalidPath)?;
        if !permitted(path) {
            return Err(Error::InvalidPath);
        }
        let intent = match dialog {
            Dialog::Create(folder) => {
                if parent(path) != Some(folder.path) {
                    return Err(Error::InvalidPath);
                }
                Intent::CreateFolder {
                    parent: folder,
                    path,
                }
            }
            Dialog::Move(source) => {
                let source_parent = parent(source.path).ok_or(Error::InvalidPath)?;
                let destination_parent = parent(path).ok_or(Error::InvalidPath)?;
                if path == source.path
                    || destination_parent == source.path
                    || is_descendant(destination_parent, source.path)
                {
                    return Err(Error::InvalidPath);
                }
                // A same-path edit is never a successful move. Destination may
                // cross parents; the trusted adapter checks its directory ID.
                let _ = source_parent;
                Intent::MoveFolder {
                    source,
                    destination: path,
                }
            }
            Dialog::Remove(source) => Intent::RemoveFolder { source },
        };
        self.enqueue(intent)
    }
    pub fn cancel(&mut self) {
        if !self.busy() {
            self.dialog = None;
            self.approve = false;
            self.error = None;
        }
    }
    pub fn take(&mut self) -> Option<Action> {
        if self.taken {
            return None;
        }
        let action = self.pending?;
        self.taken = true;
        Some(action)
    }
    pub fn complete(
        &mut self,
        request: u64,
        result: Result<Listing, BackendError>,
    ) -> Result<(), Error> {
        let action = self
            .pending
            .filter(|a| a.request == request && self.taken)
            .ok_or(Error::StaleCompletion)?;
        let expected = match action.intent {
            Intent::Navigate { path } => path,
            Intent::CreateFolder { parent, .. } => parent.path,
            Intent::MoveFolder { source, .. } | Intent::RemoveFolder { source } => {
                parent(source.path).ok_or(Error::InvalidPath)?
            }
        };
        if let Ok(listing) = result {
            if !valid_listing(listing)
                || listing.folder.path != expected
                || self
                    .listing
                    .is_some_and(|old| listing.folder.revision < old.folder.revision)
            {
                return Err(Error::InvalidListing);
            }
            let mutation_matches = match action.intent {
                Intent::Navigate { .. } => true,
                Intent::CreateFolder { parent, path } => {
                    listing.folder.revision > parent.revision
                        && listing
                            .entries
                            .iter()
                            .flatten()
                            .any(|e| e.path == path && e.kind == Kind::Directory)
                }
                Intent::MoveFolder {
                    source,
                    destination,
                } => {
                    listing.folder.revision > source.revision
                        && !listing
                            .entries
                            .iter()
                            .flatten()
                            .any(|e| e.path == source.path)
                        && (parent(destination) != Some(listing.folder.path)
                            || listing.entries.iter().flatten().any(|e| {
                                e.path == destination
                                    && e.kind == Kind::Directory
                                    && e.id == source.id
                            }))
                }
                Intent::RemoveFolder { source } => {
                    listing.folder.revision > source.revision
                        && !listing
                            .entries
                            .iter()
                            .flatten()
                            .any(|e| e.path == source.path)
                }
            };
            if !mutation_matches {
                return Err(Error::InvalidListing);
            }
        }
        self.pending = None;
        self.taken = false;
        match result {
            Err(error) => self.error = Some(error),
            Ok(listing) => {
                let select = match action.intent {
                    Intent::CreateFolder { path, .. } => Some(path),
                    Intent::MoveFolder { destination, .. }
                        if parent(destination) == Some(listing.folder.path) =>
                    {
                        Some(destination)
                    }
                    Intent::MoveFolder { .. }
                    | Intent::RemoveFolder { .. }
                    | Intent::Navigate { .. } => None,
                };
                self.selected = select
                    .and_then(|path| {
                        listing
                            .entries
                            .iter()
                            .position(|e| e.is_some_and(|e| e.path == path))
                    })
                    .or_else(|| listing.entries.iter().position(Option::is_some));
                self.listing = Some(listing);
                self.address.set(listing.folder.path.as_str())?;
                self.dialog = None;
                self.approve = false;
                self.error = None;
            }
        }
        Ok(())
    }
    pub fn refresh(&mut self, listing: Listing) -> Result<(), Error> {
        if self.busy() || self.dialog_active() {
            return Err(Error::Busy);
        }
        if !valid_listing(listing)
            || self.listing.is_some_and(|old| {
                old.folder.path != listing.folder.path
                    || listing.folder.revision < old.folder.revision
            })
        {
            return Err(Error::InvalidListing);
        }
        let selected = self.selected().map(|e| e.path);
        self.selected = selected
            .and_then(|p| {
                listing
                    .entries
                    .iter()
                    .position(|e| e.is_some_and(|e| e.path == p))
            })
            .or_else(|| listing.entries.iter().position(Option::is_some));
        self.listing = Some(listing);
        self.address.set(listing.folder.path.as_str())?;
        Ok(())
    }
}
impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn path(p: &str) -> Path {
        Path::new(p).unwrap()
    }
    fn listing(current: &str, id: u64, revision: u64, entries: &[(&str, Kind, u64)]) -> Listing {
        let mut rows = [None; 8];
        for (i, (p, kind, id)) in entries.iter().enumerate() {
            rows[i] = Some(Entry {
                path: path(p),
                kind: *kind,
                id: *id,
                size: 0,
            });
        }
        Listing {
            folder: FolderSnapshot {
                path: path(current),
                id,
                revision,
            },
            entries: rows,
        }
    }
    fn loaded() -> Model {
        let mut m = Model::new();
        m.navigate(path("/native")).unwrap();
        let action = m.take().unwrap();
        m.complete(
            action.request,
            Ok(listing(
                "/native",
                0,
                3,
                &[
                    ("/native/docs", Kind::Directory, 2),
                    ("/native/readme", Kind::File, 0),
                ],
            )),
        )
        .unwrap();
        m
    }
    #[test]
    fn navigation_parent_and_file_open_use_only_authoritative_rows() {
        let mut m = loaded();
        assert!(m.select(0));
        assert_eq!(m.open_selected(), Ok(None));
        let a = m.take().unwrap();
        assert!(matches!(a.intent,Intent::Navigate{path:p} if p==path("/native/docs")));
        assert_eq!(m.listing().unwrap().folder.path, path("/native"));
        m.complete(
            a.request,
            Ok(listing(
                "/native/docs",
                2,
                3,
                &[("/native/docs/a", Kind::File, 0)],
            )),
        )
        .unwrap();
        assert_eq!(m.open_selected(), Ok(Some(path("/native/docs/a"))));
        assert!(m.up().unwrap());
        let up = m.take().unwrap();
        m.complete(up.request, Ok(listing("/native", 0, 3, &[])))
            .unwrap();
        assert!(!m.up().unwrap());
    }
    #[test]
    fn prefix_files_do_not_fabricate_directory_rows_or_breadcrumbs() {
        let mut m = loaded();
        let old = m.listing();
        m.navigate(path("/native/missing/deep")).unwrap();
        let a = m.take().unwrap();
        m.complete(a.request, Err(BackendError::MissingParent))
            .unwrap();
        assert_eq!(m.listing(), old);
        assert_eq!(m.address(), "/native/missing/deep");
        assert_eq!(m.error(), Some(BackendError::MissingParent));
        assert!(m.navigate(path("/prefs/appearance")).is_err());
        let fabricated = listing("/native", 0, 4, &[("/native/inferred/file", Kind::File, 0)]);
        assert_eq!(m.refresh(fabricated), Err(Error::InvalidListing));
        assert_eq!(m.listing(), old);
    }
    #[test]
    fn create_defaults_cancel_then_conflict_preserves_input_and_rows() {
        let mut m = loaded();
        let old = m.listing();
        m.begin_create().unwrap();
        assert!(!m.approves());
        m.set_input("/native/new").unwrap();
        m.confirm().unwrap();
        assert!(m.take().is_none());
        assert_eq!(m.listing(), old);
        m.begin_create().unwrap();
        m.set_input("/native/new").unwrap();
        m.choose_approve(true).unwrap();
        m.confirm().unwrap();
        let a = m.take().unwrap();
        assert!(m.take().is_none());
        assert!(
            matches!(a.intent,Intent::CreateFolder{parent,path:p} if parent.id==0&&parent.revision==3&&p==path("/native/new"))
        );
        assert_eq!(m.set_input("/native/other"), Err(Error::Busy));
        assert_eq!(
            m.complete(a.request + 1, Err(BackendError::Conflict)),
            Err(Error::StaleCompletion)
        );
        m.complete(a.request, Err(BackendError::Conflict)).unwrap();
        assert_eq!(m.listing(), old);
        assert_eq!(m.input(), "/native/new");
        assert!(m.dialog_active());
        assert_eq!(m.error(), Some(BackendError::Conflict));
        m.confirm().unwrap();
        let retry = m.take().unwrap();
        m.complete(
            retry.request,
            Ok(listing(
                "/native",
                0,
                4,
                &[
                    ("/native/docs", Kind::Directory, 2),
                    ("/native/new", Kind::Directory, 4),
                ],
            )),
        )
        .unwrap();
        assert_eq!(m.selected().unwrap().path, path("/native/new"));
    }
    #[test]
    fn nonempty_rename_reports_real_failure_and_does_not_rewrite_children() {
        let mut m = loaded();
        let old = m.listing();
        m.begin_move().unwrap();
        assert!(!m.approves());
        m.set_input("/native/renamed").unwrap();
        m.choose_approve(true).unwrap();
        m.confirm().unwrap();
        let a = m.take().unwrap();
        assert!(
            matches!(a.intent,Intent::MoveFolder{source,destination} if source.id==2&&source.revision==3&&destination==path("/native/renamed"))
        );
        m.complete(a.request, Err(BackendError::NotEmpty)).unwrap();
        assert_eq!(m.listing(), old);
        assert_eq!(m.input(), "/native/renamed");
        assert_eq!(m.selected().unwrap().path, path("/native/docs"));
        assert_eq!(m.error(), Some(BackendError::NotEmpty));
        m.cancel();
        assert!(!m.dialog_active());
    }
    #[test]
    fn invalid_listing_never_commits_wrong_identity_duplicate_or_private_path() {
        let mut m = loaded();
        let old = m.listing();
        m.navigate(path("/native/docs")).unwrap();
        let a = m.take().unwrap();
        for wrong in [
            listing("/native/docs", 0, 3, &[]),
            listing(
                "/native/docs",
                2,
                3,
                &[
                    ("/native/docs/a", Kind::File, 0),
                    ("/native/docs/a", Kind::File, 0),
                ],
            ),
            listing(
                "/native/docs",
                2,
                3,
                &[("/dirs/m.0000000000000002", Kind::File, 0)],
            ),
        ] {
            assert_eq!(m.complete(a.request, Ok(wrong)), Err(Error::InvalidListing));
            assert_eq!(m.listing(), old);
        }
        m.complete(a.request, Ok(listing("/native/docs", 2, 3, &[])))
            .unwrap();
        assert!(m.selected().is_none());
    }
    #[test]
    fn success_requires_actual_new_directory_and_preserved_rename_identity() {
        let mut m = loaded();
        m.begin_create().unwrap();
        m.set_input("/native/new").unwrap();
        m.choose_approve(true).unwrap();
        m.confirm().unwrap();
        let a = m.take().unwrap();
        assert_eq!(
            m.complete(a.request, Ok(listing("/native", 0, 4, &[]))),
            Err(Error::InvalidListing)
        );
        assert_eq!(
            m.complete(
                a.request,
                Ok(listing(
                    "/native",
                    0,
                    3,
                    &[("/native/new", Kind::Directory, 4)]
                ))
            ),
            Err(Error::InvalidListing)
        );
        m.complete(
            a.request,
            Ok(listing(
                "/native",
                0,
                4,
                &[("/native/new", Kind::Directory, 4)],
            )),
        )
        .unwrap();
        m.begin_move().unwrap();
        m.set_input("/native/renamed").unwrap();
        m.choose_approve(true).unwrap();
        m.confirm().unwrap();
        let rename = m.take().unwrap();
        assert_eq!(
            m.complete(
                rename.request,
                Ok(listing(
                    "/native",
                    0,
                    5,
                    &[("/native/renamed", Kind::Directory, 5)]
                ))
            ),
            Err(Error::InvalidListing)
        );
        m.complete(
            rename.request,
            Ok(listing(
                "/native",
                0,
                5,
                &[("/native/renamed", Kind::Directory, 4)],
            )),
        )
        .unwrap();
    }
    #[test]
    fn move_folder_across_nested_parents_waits_for_backend_and_keeps_identity() {
        let mut m = Model::new();
        m.navigate(path("/native")).unwrap();
        let first = m.take().unwrap();
        m.complete(
            first.request,
            Ok(listing(
                "/native",
                0,
                3,
                &[
                    ("/native/docs", Kind::Directory, 2),
                    ("/native/archive", Kind::Directory, 3),
                ],
            )),
        )
        .unwrap();
        assert!(m.select(0));
        let old = m.listing();
        m.begin_move().unwrap();
        m.set_input("/native/archive/docs").unwrap();
        assert!(!m.approves());
        m.choose_approve(true).unwrap();
        m.confirm().unwrap();
        let action = m.take().unwrap();
        assert!(
            matches!(action.intent, Intent::MoveFolder { source, destination }
            if source.path == path("/native/docs") && source.id == 2
                && destination == path("/native/archive/docs"))
        );
        assert_eq!(m.listing(), old);
        m.complete(
            action.request,
            Ok(listing(
                "/native",
                0,
                4,
                &[("/native/archive", Kind::Directory, 3)],
            )),
        )
        .unwrap();
        assert!(m.selected().is_some());
        m.navigate(path("/native/archive")).unwrap();
        let refresh = m.take().unwrap();
        m.complete(
            refresh.request,
            Ok(listing(
                "/native/archive",
                3,
                4,
                &[("/native/archive/docs", Kind::Directory, 2)],
            )),
        )
        .unwrap();
        assert_eq!(m.selected().unwrap().id, 2);
    }
    #[test]
    fn remove_requires_explicit_approval_and_only_commits_refreshed_absence() {
        let mut m = loaded();
        let old = m.listing();
        assert!(m.select(0));
        m.begin_remove().unwrap();
        assert!(!m.approves());
        m.confirm().unwrap();
        assert!(m.take().is_none());
        assert_eq!(m.listing(), old);
        assert!(!m.dialog_active());
        m.begin_remove().unwrap();
        m.choose_approve(true).unwrap();
        m.confirm().unwrap();
        let action = m.take().unwrap();
        assert!(matches!(action.intent, Intent::RemoveFolder { source }
            if source.path == path("/native/docs") && source.id == 2 && source.revision == 3));
        m.complete(action.request, Err(BackendError::NotEmpty))
            .unwrap();
        assert_eq!(m.listing(), old);
        assert_eq!(m.selected().unwrap().path, path("/native/docs"));
        assert_eq!(m.error(), Some(BackendError::NotEmpty));
        m.confirm().unwrap();
        let retry = m.take().unwrap();
        assert_eq!(
            m.complete(
                retry.request,
                Ok(listing(
                    "/native",
                    0,
                    4,
                    &[("/native/readme", Kind::File, 0)]
                )),
            ),
            Ok(())
        );
        assert!(m.selected().is_some());
    }
    #[test]
    fn move_rejects_cycles_and_remove_never_targets_namespace_root() {
        let mut m = Model::new();
        m.navigate(path("/native")).unwrap();
        let action = m.take().unwrap();
        m.complete(
            action.request,
            Ok(listing(
                "/native",
                0,
                3,
                &[("/native/docs", Kind::Directory, 2)],
            )),
        )
        .unwrap();
        assert!(m.select(0));
        m.begin_move().unwrap();
        m.set_input("/native/docs/child/moved").unwrap();
        m.choose_approve(true).unwrap();
        assert_eq!(m.confirm(), Err(Error::InvalidPath));
        assert!(m.take().is_none());
        m.cancel();
        assert!(m.select(0));
        m.begin_remove().unwrap();
        assert_eq!(m.input(), "/native/docs");
        assert!(!m.approves());
    }
    #[test]
    fn stale_directory_refresh_and_completion_keep_current_selection() {
        let mut m = loaded();
        assert!(m.select(1));
        let selected = m.selected();
        assert_eq!(
            m.refresh(listing("/native", 0, 2, &[])),
            Err(Error::InvalidListing)
        );
        assert_eq!(m.selected(), selected);
        m.begin_create().unwrap();
        m.set_input("/native/sub/illegal").unwrap();
        m.choose_approve(true).unwrap();
        assert_eq!(m.confirm(), Err(Error::InvalidPath));
        assert!(m.take().is_none());
        assert_eq!(m.selected(), selected);
    }
}

// Cross-parent actions deliberately publish only backend-refreshed source rows.
// The destination remains an ordinary authoritative directory row that can be
// opened next; the model never fabricates the moved child locally.
