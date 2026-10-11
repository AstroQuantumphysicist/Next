//! Bounded Files dialogs and backend intents. No directory entry is changed by
//! this model. The backend revalidates snapshots and applies mutations atomically.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Path {
    bytes: [u8; 32],
    len: u8,
}
impl Path {
    pub fn new(text: &str) -> Result<Self, Error> {
        if text.len() < 2
            || text.len() > 31
            || !text.starts_with('/')
            || !text[1..].split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            })
        {
            return Err(Error::InvalidPath);
        }
        let mut path = Self {
            bytes: [0; 32],
            len: text.len() as u8,
        };
        path.bytes[..text.len()].copy_from_slice(text.as_bytes());
        Ok(path)
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Namespace {
    path: Path,
    root: bool,
}
impl Namespace {
    /// This namespace is supplied by the trusted Store adapter, not by a file.
    pub fn new(text: &str) -> Result<Self, Error> {
        if text == "/" {
            return Ok(Self {
                path: Path {
                    bytes: [0; 32],
                    len: 0,
                },
                root: true,
            });
        }
        Ok(Self {
            path: Path::new(text)?,
            root: false,
        })
    }
    pub fn permits(self, path: Path) -> bool {
        self.root
            || path
                .as_str()
                .strip_prefix(self.path.as_str())
                .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1)
    }
}
/// Backend-issued revision token. The backend must reject a changed source or
/// changed destination; the UI does not infer existence or permissions itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub path: Path,
    pub revision: u64,
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
    Unavailable,
}
impl BackendError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::ReadOnly => "This store is read-only. Your files were not changed.",
            Self::Exists => "A file already has that name. Choose another name.",
            Self::NotFound => "The selected file is no longer available. Refresh Files.",
            Self::Conflict => "The file changed. Refresh Files before trying again.",
            Self::Busy => "The file is busy. Close its active operation and try again.",
            Self::Full => "The store has no free file slots. Remove an unneeded file first.",
            Self::Io => {
                "Storage could not confirm the operation. Check the device before retrying."
            }
            Self::InvalidPath => "Use an absolute path within this file namespace.",
            Self::Unavailable => "The file store is unavailable. Try again when it is mounted.",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidPath,
    WrongState,
    Busy,
    StaleCompletion,
    InvalidResponse,
    RequestExhausted,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    Create {
        path: Path,
    },
    Rename {
        source: Snapshot,
        destination: Path,
        replace: Option<Snapshot>,
    },
    Copy {
        source: Snapshot,
        destination: Path,
        replace: Option<Snapshot>,
    },
    Delete {
        source: Snapshot,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    InspectDestination { request: u64, path: Path },
    Commit { request: u64, intent: Intent },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Created(Path),
    Renamed { old: Path, new: Path },
    Copied { source: Path, new: Path },
    Deleted(Path),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Idle,
    Editing,
    Inspecting,
    ConfirmDelete,
    ConfirmOverwrite,
    Committing,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Create(Namespace),
    Rename {
        source: Snapshot,
        namespace: Namespace,
    },
    Copy {
        source: Snapshot,
        namespace: Namespace,
    },
    Delete(Snapshot),
}

pub struct Dialog {
    operation: Option<Operation>,
    phase: Phase,
    input: [u8; 32],
    input_len: usize,
    approve: bool,
    error: Option<BackendError>,
    next_request: u64,
    pending: Option<u64>,
    target: Option<Snapshot>,
    action: Option<Action>,
    outcome: Option<Outcome>,
}
impl Dialog {
    pub const fn new() -> Self {
        Self {
            operation: None,
            phase: Phase::Idle,
            input: [0; 32],
            input_len: 0,
            approve: false,
            error: None,
            next_request: 1,
            pending: None,
            target: None,
            action: None,
            outcome: None,
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn input(&self) -> &str {
        core::str::from_utf8(&self.input[..self.input_len]).unwrap_or("")
    }
    pub fn approved(&self) -> bool {
        self.approve
    }
    pub fn error(&self) -> Option<BackendError> {
        self.error
    }
    pub fn is_copy(&self) -> bool {
        matches!(self.operation, Some(Operation::Copy { .. }))
    }
    pub fn selected(&self) -> Option<Snapshot> {
        match self.operation {
            Some(
                Operation::Rename { source, .. }
                | Operation::Copy { source, .. }
                | Operation::Delete(source),
            ) => Some(source),
            _ => None,
        }
    }
    pub fn replacement(&self) -> Option<Snapshot> {
        self.target
    }
    pub fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
    fn start(&mut self, operation: Operation, phase: Phase) -> Result<(), Error> {
        if self.phase != Phase::Idle {
            return Err(Error::Busy);
        }
        self.operation = Some(operation);
        self.phase = phase;
        self.input_len = 0;
        self.approve = false;
        self.error = None;
        self.target = None;
        self.outcome = None;
        Ok(())
    }
    pub fn begin_create(&mut self, namespace: Namespace) -> Result<(), Error> {
        self.start(Operation::Create(namespace), Phase::Editing)
    }
    pub fn begin_rename(&mut self, source: Snapshot, namespace: Namespace) -> Result<(), Error> {
        if !namespace.permits(source.path) {
            return Err(Error::InvalidPath);
        }
        self.start(Operation::Rename { source, namespace }, Phase::Editing)?;
        self.set_input(source.path.as_str())
    }
    pub fn begin_copy(&mut self, source: Snapshot, namespace: Namespace) -> Result<(), Error> {
        if !namespace.permits(source.path) {
            return Err(Error::InvalidPath);
        }
        self.start(Operation::Copy { source, namespace }, Phase::Editing)?;
        self.set_input(source.path.as_str())
    }
    pub fn begin_delete(&mut self, source: Snapshot) -> Result<(), Error> {
        self.start(Operation::Delete(source), Phase::ConfirmDelete)
    }
    pub fn set_input(&mut self, text: &str) -> Result<(), Error> {
        if self.phase != Phase::Editing {
            return Err(Error::WrongState);
        }
        if text.len() > 31 || !text.bytes().all(|b| b.is_ascii() && !b.is_ascii_control()) {
            return Err(Error::InvalidPath);
        }
        self.input.fill(0);
        self.input[..text.len()].copy_from_slice(text.as_bytes());
        self.input_len = text.len();
        self.error = None;
        Ok(())
    }
    pub fn insert(&mut self, c: char) -> Result<(), Error> {
        if self.phase != Phase::Editing {
            return Err(Error::WrongState);
        }
        if !c.is_ascii() || c.is_ascii_control() || self.input_len == 31 {
            return Err(Error::InvalidPath);
        }
        self.input[self.input_len] = c as u8;
        self.input_len += 1;
        self.error = None;
        Ok(())
    }
    pub fn backspace(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Editing {
            return Err(Error::WrongState);
        }
        self.input_len = self.input_len.saturating_sub(1);
        self.error = None;
        Ok(())
    }
    pub fn choose_approve(&mut self, approve: bool) -> Result<(), Error> {
        if !matches!(self.phase, Phase::ConfirmDelete | Phase::ConfirmOverwrite) {
            return Err(Error::WrongState);
        }
        self.approve = approve;
        Ok(())
    }
    fn request(&mut self) -> Result<u64, Error> {
        let next = self
            .next_request
            .checked_add(1)
            .ok_or(Error::RequestExhausted)?;
        let request = self.next_request;
        self.next_request = next;
        Ok(request)
    }
    pub fn cancel(&mut self) -> Result<(), Error> {
        // A mutation already submitted to storage cannot be represented as cancelled.
        if self.phase == Phase::Committing {
            return Err(Error::Busy);
        }
        self.phase = Phase::Idle;
        self.operation = None;
        self.pending = None;
        self.action = None;
        self.target = None;
        self.error = None;
        self.approve = false;
        Ok(())
    }
    pub fn activate(&mut self) -> Result<(), Error> {
        match self.phase {
            Phase::Editing => {
                let path = Path::new(self.input())
                    .inspect_err(|_| self.error = Some(BackendError::InvalidPath))?;
                let namespace = match self.operation {
                    Some(
                        Operation::Create(n)
                        | Operation::Rename { namespace: n, .. }
                        | Operation::Copy { namespace: n, .. },
                    ) => n,
                    _ => return Err(Error::WrongState),
                };
                if !namespace.permits(path) {
                    self.error = Some(BackendError::InvalidPath);
                    return Err(Error::InvalidPath);
                }
                if self.selected().is_some_and(|s| s.path == path) {
                    self.error = Some(BackendError::Exists);
                    return Err(Error::InvalidPath);
                }
                let request = self.request()?;
                self.pending = Some(request);
                self.phase = Phase::Inspecting;
                self.error = None;
                self.action = Some(Action::InspectDestination { request, path });
                Ok(())
            }
            Phase::ConfirmDelete | Phase::ConfirmOverwrite => {
                if !self.approve {
                    return self.cancel();
                }
                self.commit()
            }
            _ => Err(Error::WrongState),
        }
    }
    fn commit(&mut self) -> Result<(), Error> {
        let intent = match self.operation {
            Some(Operation::Create(_)) => Intent::Create {
                path: Path::new(self.input())?,
            },
            Some(Operation::Rename { source, .. }) => Intent::Rename {
                source,
                destination: Path::new(self.input())?,
                replace: self.target,
            },
            Some(Operation::Copy { source, .. }) => Intent::Copy {
                source,
                destination: Path::new(self.input())?,
                replace: self.target,
            },
            Some(Operation::Delete(source)) => Intent::Delete { source },
            None => return Err(Error::WrongState),
        };
        let request = self.request()?;
        self.pending = Some(request);
        self.phase = Phase::Committing;
        self.action = Some(Action::Commit { request, intent });
        self.error = None;
        Ok(())
    }
    pub fn destination_result(
        &mut self,
        request: u64,
        result: Result<Option<Snapshot>, BackendError>,
    ) -> Result<(), Error> {
        if self.phase != Phase::Inspecting || self.pending != Some(request) {
            return Err(Error::StaleCompletion);
        }
        if result.as_ref().is_ok_and(|target| {
            target.is_some_and(|s| Some(s.path) != Path::new(self.input()).ok())
        }) {
            return Err(Error::InvalidResponse);
        }
        self.pending = None;
        self.action = None;
        match result {
            Err(error) => {
                self.error = Some(error);
                self.phase = Phase::Editing;
                Ok(())
            }
            Ok(Some(target)) => {
                if matches!(self.operation, Some(Operation::Create(_))) {
                    self.error = Some(BackendError::Exists);
                    self.phase = Phase::Editing;
                } else {
                    self.target = Some(target);
                    self.approve = false;
                    self.phase = Phase::ConfirmOverwrite;
                }
                Ok(())
            }
            Ok(None) => {
                self.target = None;
                self.phase = Phase::Editing;
                self.commit()
            }
        }
    }
    pub fn complete(
        &mut self,
        request: u64,
        result: Result<(), BackendError>,
    ) -> Result<(), Error> {
        if self.phase != Phase::Committing || self.pending != Some(request) {
            return Err(Error::StaleCompletion);
        }
        self.pending = None;
        self.action = None;
        match result {
            Ok(()) => {
                self.outcome = match self.operation {
                    Some(Operation::Create(_)) => Some(Outcome::Created(Path::new(self.input())?)),
                    Some(Operation::Rename { source, .. }) => Some(Outcome::Renamed {
                        old: source.path,
                        new: Path::new(self.input())?,
                    }),
                    Some(Operation::Copy { source, .. }) => Some(Outcome::Copied {
                        source: source.path,
                        new: Path::new(self.input())?,
                    }),
                    Some(Operation::Delete(s)) => Some(Outcome::Deleted(s.path)),
                    None => return Err(Error::WrongState),
                };
                self.operation = None;
                self.phase = Phase::Idle;
                self.error = None;
                self.approve = false;
                Ok(())
            }
            Err(error) => {
                self.error = Some(error);
                self.approve = false;
                self.target = None;
                self.phase = if matches!(self.operation, Some(Operation::Delete(_))) {
                    Phase::ConfirmDelete
                } else {
                    Phase::Editing
                };
                Ok(())
            }
        }
    }
}
impl Default for Dialog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn native() -> Namespace {
        Namespace::new("/native").unwrap()
    }
    fn s(path: &str, revision: u64) -> Snapshot {
        Snapshot {
            path: Path::new(path).unwrap(),
            revision,
        }
    }
    fn inspect(d: &mut Dialog) -> u64 {
        match d.take_action().unwrap() {
            Action::InspectDestination { request, .. } => request,
            _ => panic!("inspect first"),
        }
    }
    fn commit(d: &mut Dialog) -> (u64, Intent) {
        match d.take_action().unwrap() {
            Action::Commit { request, intent } => (request, intent),
            _ => panic!("commit"),
        }
    }
    #[test]
    fn canonical_namespace_validation_matches_store_and_keeps_invalid_input() {
        for path in [
            "/native/../a",
            "/native//a",
            "/native/a/",
            "/native/a b",
            "/native/é",
            "relative",
        ] {
            assert_eq!(Path::new(path), Err(Error::InvalidPath));
        }
        let mut d = Dialog::new();
        d.begin_create(native()).unwrap();
        d.set_input("/native-other/x").unwrap();
        assert_eq!(d.activate(), Err(Error::InvalidPath));
        assert_eq!(d.input(), "/native-other/x");
        assert!(d.take_action().is_none());
        let namespace = Namespace::new("/journal").unwrap();
        assert!(namespace.permits(Path::new("/journal/real").unwrap()));
        assert!(!namespace.permits(Path::new("/native/real").unwrap()));
    }
    #[test]
    fn create_does_not_publish_outcome_until_backend_confirms_and_never_overwrites() {
        let mut d = Dialog::new();
        d.begin_create(native()).unwrap();
        d.set_input("/native/new").unwrap();
        d.activate().unwrap();
        let req = inspect(&mut d);
        d.destination_result(req, Ok(Some(s("/native/new", 3))))
            .unwrap();
        assert_eq!(d.error(), Some(BackendError::Exists));
        assert_eq!(d.phase(), Phase::Editing);
        assert!(d.take_outcome().is_none());
        d.set_input("/native/empty").unwrap();
        d.activate().unwrap();
        let req = inspect(&mut d);
        d.destination_result(req, Ok(None)).unwrap();
        let (req, intent) = commit(&mut d);
        assert_eq!(
            intent,
            Intent::Create {
                path: Path::new("/native/empty").unwrap()
            }
        );
        assert!(d.take_outcome().is_none());
        assert_eq!(d.cancel(), Err(Error::Busy));
        d.complete(req, Ok(())).unwrap();
        assert_eq!(
            d.take_outcome(),
            Some(Outcome::Created(Path::new("/native/empty").unwrap()))
        );
    }
    #[test]
    fn rename_overwrite_default_cancel_and_explicit_commit_hold_both_snapshots() {
        let source = s("/native/source", 7);
        let target = s("/native/target", 8);
        let mut d = Dialog::new();
        d.begin_rename(source, native()).unwrap();
        d.set_input(target.path.as_str()).unwrap();
        d.activate().unwrap();
        let req = inspect(&mut d);
        d.destination_result(req, Ok(Some(target))).unwrap();
        assert_eq!(d.phase(), Phase::ConfirmOverwrite);
        assert!(!d.approved());
        assert_eq!(d.selected(), Some(source));
        d.activate().unwrap();
        assert!(d.take_action().is_none());
        assert_eq!(d.phase(), Phase::Idle);
        d.begin_rename(source, native()).unwrap();
        d.set_input(target.path.as_str()).unwrap();
        d.activate().unwrap();
        let req = inspect(&mut d);
        d.destination_result(req, Ok(Some(target))).unwrap();
        d.choose_approve(true).unwrap();
        d.activate().unwrap();
        let (req, intent) = commit(&mut d);
        assert_eq!(
            intent,
            Intent::Rename {
                source,
                destination: target.path,
                replace: Some(target)
            }
        );
        d.complete(req, Err(BackendError::Conflict)).unwrap();
        assert_eq!(d.selected(), Some(source));
        assert_eq!(d.input(), target.path.as_str());
        assert_eq!(d.error(), Some(BackendError::Conflict));
        assert_eq!(d.phase(), Phase::Editing);
    }
    #[test]
    fn permanent_delete_is_cancel_first_and_error_preserves_selection() {
        let source = s("/native/precious", 9);
        let mut d = Dialog::new();
        d.begin_delete(source).unwrap();
        d.activate().unwrap();
        assert!(d.take_action().is_none());
        d.begin_delete(source).unwrap();
        d.choose_approve(true).unwrap();
        d.activate().unwrap();
        let (req, intent) = commit(&mut d);
        assert_eq!(intent, Intent::Delete { source });
        d.complete(req, Err(BackendError::ReadOnly)).unwrap();
        assert_eq!(d.selected(), Some(source));
        assert!(!d.approved());
        assert_eq!(d.error(), Some(BackendError::ReadOnly));
        assert!(d.take_outcome().is_none());
        d.choose_approve(true).unwrap();
        d.activate().unwrap();
        let (req, _) = commit(&mut d);
        d.complete(req, Ok(())).unwrap();
        assert_eq!(d.take_outcome(), Some(Outcome::Deleted(source.path)));
    }
    #[test]
    fn cancelled_inspection_and_old_completion_cannot_affect_new_dialog() {
        let mut d = Dialog::new();
        d.begin_create(native()).unwrap();
        d.set_input("/native/old").unwrap();
        d.activate().unwrap();
        let old = inspect(&mut d);
        d.cancel().unwrap();
        d.begin_create(native()).unwrap();
        d.set_input("/native/new").unwrap();
        d.activate().unwrap();
        let new = inspect(&mut d);
        assert_ne!(old, new);
        assert_eq!(
            d.destination_result(old, Ok(None)),
            Err(Error::StaleCompletion)
        );
        assert_eq!(d.input(), "/native/new");
        d.destination_result(new, Ok(None)).unwrap();
        let (req, _) = commit(&mut d);
        assert_eq!(d.complete(new, Ok(())), Err(Error::StaleCompletion));
        assert_eq!(d.phase(), Phase::Committing);
        d.complete(req, Err(BackendError::Io)).unwrap();
        assert_eq!(d.input(), "/native/new");
    }
    #[test]
    fn mismatched_authoritative_destination_is_rejected_and_failures_are_retriable() {
        let mut d = Dialog::new();
        d.begin_rename(s("/native/from", 1), native()).unwrap();
        d.set_input("/native/to").unwrap();
        d.activate().unwrap();
        let req = inspect(&mut d);
        assert_eq!(
            d.destination_result(req, Ok(Some(s("/native/wrong", 2)))),
            Err(Error::InvalidResponse)
        );
        assert_eq!(d.phase(), Phase::Inspecting);
        d.destination_result(req, Err(BackendError::Unavailable))
            .unwrap();
        assert_eq!(d.phase(), Phase::Editing);
        assert_eq!(d.error(), Some(BackendError::Unavailable));
        assert_eq!(d.input(), "/native/to");
    }
    #[test]
    fn bounded_input_and_request_exhaustion_do_not_emit_partial_actions() {
        let mut d = Dialog::new();
        d.begin_create(native()).unwrap();
        d.set_input("/native/a").unwrap();
        assert_eq!(
            d.set_input("/native/aaaaaaaaaaaaaaaaaaaaaaaaa"),
            Err(Error::InvalidPath)
        );
        assert_eq!(d.input(), "/native/a");
        d.next_request = u64::MAX;
        assert_eq!(d.activate(), Err(Error::RequestExhausted));
        assert_eq!(d.phase(), Phase::Editing);
        assert!(d.take_action().is_none());
    }
    #[test]
    fn copy_preserves_source_snapshot_and_requires_explicit_target_confirmation() {
        let source = s("/native/source", 7);
        let target = s("/native/target", 7);
        let mut dialog = Dialog::new();
        dialog.begin_copy(source, native()).unwrap();
        assert!(dialog.is_copy());
        assert_eq!(dialog.activate(), Err(Error::InvalidPath));
        dialog.set_input(target.path.as_str()).unwrap();
        dialog.activate().unwrap();
        let req = inspect(&mut dialog);
        dialog.destination_result(req, Ok(Some(target))).unwrap();
        assert!(!dialog.approved());
        dialog.activate().unwrap();
        assert_eq!(dialog.phase(), Phase::Idle);
        assert!(dialog.take_action().is_none());
        dialog.begin_copy(source, native()).unwrap();
        dialog.set_input(target.path.as_str()).unwrap();
        dialog.activate().unwrap();
        let req = inspect(&mut dialog);
        dialog.destination_result(req, Ok(Some(target))).unwrap();
        dialog.choose_approve(true).unwrap();
        dialog.activate().unwrap();
        let (req, intent) = commit(&mut dialog);
        assert_eq!(
            intent,
            Intent::Copy {
                source,
                destination: target.path,
                replace: Some(target)
            }
        );
        assert!(dialog.take_outcome().is_none());
        dialog.complete(req, Err(BackendError::Conflict)).unwrap();
        assert_eq!(dialog.input(), target.path.as_str());
        assert_eq!(dialog.selected(), Some(source));
        assert_eq!(dialog.phase(), Phase::Editing);
        dialog.activate().unwrap();
        let inspect_req = inspect(&mut dialog);
        dialog.destination_result(inspect_req, Ok(None)).unwrap();
        let (req, intent) = commit(&mut dialog);
        assert_eq!(
            intent,
            Intent::Copy {
                source,
                destination: target.path,
                replace: None
            }
        );
        assert_eq!(
            dialog.complete(req.wrapping_sub(1), Ok(())),
            Err(Error::StaleCompletion)
        );
        dialog.complete(req, Ok(())).unwrap();
        assert_eq!(
            dialog.take_outcome(),
            Some(Outcome::Copied {
                source: source.path,
                new: target.path
            })
        );
    }
}
