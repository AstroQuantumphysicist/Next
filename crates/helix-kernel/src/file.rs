//! Boot-CPU native stream adapter. No references survive context switches;
//! syscall callers validate capability rights and user ranges before entry.
use core::cell::UnsafeCell;
pub use helix_core::file::FileHandle;
use helix_core::file::{Backend, Error, Files, FILE_BYTES};
use next_abi::Status;
#[path = "gui_copy.rs"]
pub(crate) mod gui_copy;
#[path = "gui_directories.rs"]
pub(crate) mod gui_directories;
#[path = "gui_files.rs"]
pub(crate) mod gui_files;
#[path = "trash.rs"]
pub(crate) mod trash;
#[path = "trash_undo.rs"]
pub(crate) mod trash_undo;
const PREFERENCES_PATH: &str = "/prefs/appearance";
fn reserved_mutation(path: &str) -> bool {
    path == PREFERENCES_PATH || path == "/trash" || path.starts_with("/trash/")
}
struct Global(UnsafeCell<Files<16>>);
// Single boot CPU, no file operations in interrupt handlers.
unsafe impl Sync for Global {}
static FILES: Global = Global(UnsafeCell::new(Files::new()));
struct UndoGlobal(UnsafeCell<trash_undo::History>);
// Session history and Files are boot-CPU-only. Both mutation wrappers hold the
// same filesystem guard; interrupt handlers never access either object.
unsafe impl Sync for UndoGlobal {}
static TRASH_UNDO: UndoGlobal = UndoGlobal(UnsafeCell::new(trash_undo::History::new()));
struct Disk;
impl Backend for Disk {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, Error> {
        crate::filesystem::stream_read(path, out)
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        crate::filesystem::stream_put(path, bytes)
    }
    fn rename(&mut self, old: &str, new: &str) -> Result<(), Error> {
        crate::filesystem::stream_rename(old, new)
    }
    fn remove(&mut self, path: &str) -> Result<(), Error> {
        crate::filesystem::stream_remove(path)
    }
}
fn status(error: Error) -> Status {
    match error {
        Error::InvalidHandle => Status::InvalidHandle,
        Error::InvalidPath | Error::TooLarge => Status::InvalidArgument,
        Error::NotFound => Status::NotFound,
        Error::Full => Status::NoMemory,
        Error::Busy => Status::WouldBlock,
        Error::ReadOnly => Status::AccessDenied,
        Error::Io => Status::IoError,
        Error::Exists => Status::InvalidArgument,
    }
}
fn trace_namespace(event_id: u32, result: Result<(), Status>, size: usize) {
    let (severity, outcome) = match result {
        Ok(()) => (crate::trace::Severity::Info, 0),
        Err(Status::IoError) => (crate::trace::Severity::Error, Status::IoError as i64 as u64),
        Err(error) => (crate::trace::Severity::Warning, error as i64 as u64),
    };
    crate::trace::record(
        crate::trace::Category::Filesystem,
        severity,
        event_id,
        0,
        [outcome, size as u64, 0, 0],
    );
}
/// Trusted kernel callers supply canonical paths; user dispatch checks authority.
pub fn replace(path: &str, bytes: &[u8]) -> Result<(), Status> {
    if reserved_mutation(path) {
        return Err(Status::AccessDenied);
    }
    let result = unsafe {
        (&mut *FILES.0.get())
            .replace(&mut Disk, path, bytes)
            .map_err(status)
    };
    trace_namespace(crate::trace::id::FILE_REPLACED, result, bytes.len());
    result
}
pub fn read_path(path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, Status> {
    unsafe {
        (&*FILES.0.get())
            .read_path(&mut Disk, path, out)
            .map_err(status)
    }
}
pub fn rename(old: &str, new: &str) -> Result<(), Status> {
    if reserved_mutation(old) || reserved_mutation(new) {
        return Err(Status::AccessDenied);
    }
    let result = unsafe {
        (&mut *FILES.0.get())
            .rename(&mut Disk, old, new)
            .map_err(status)
    };
    trace_namespace(crate::trace::id::FILE_RENAMED, result, 0);
    result
}
pub fn remove(path: &str) -> Result<(), Status> {
    if reserved_mutation(path) {
        return Err(Status::AccessDenied);
    }
    let result = unsafe {
        (&mut *FILES.0.get())
            .remove(&mut Disk, path)
            .map_err(status)
    };
    trace_namespace(crate::trace::id::FILE_UNLINKED, result, 0);
    result
}
/// Requires launcher authority in dispatch; no ambient path access for children.
pub fn open(path: &str, flags: u64) -> Result<FileHandle, Status> {
    open_for_linux(path, flags).map_err(status)
}
pub fn open_for_linux(path: &str, flags: u64) -> Result<FileHandle, Error> {
    if !path.starts_with("/native/") || flags & !7 != 0 || flags & 4 != 0 && flags & 1 == 0 {
        return Err(Error::ReadOnly);
    }
    unsafe {
        (&mut *FILES.0.get()).open_exclusive(
            &mut Disk,
            path,
            flags & 1 != 0,
            flags & 2 != 0,
            flags & 4 != 0,
        )
    }
}
pub fn retain(h: FileHandle) -> Result<(), Status> {
    unsafe { (&mut *FILES.0.get()).retain(h).map_err(status) }
}
pub fn close(h: FileHandle) -> Result<(), Status> {
    unsafe { (&mut *FILES.0.get()).close(h).map_err(status) }
}
pub fn read(h: FileHandle, out: &mut [u8]) -> Result<usize, Status> {
    unsafe { (&mut *FILES.0.get()).read(h, out).map_err(status) }
}
pub fn write(h: FileHandle, bytes: &[u8]) -> Result<usize, Status> {
    unsafe {
        (&mut *FILES.0.get())
            .write(&mut Disk, h, bytes)
            .map_err(status)
    }
}
/// Resize an owned open inode without changing any description offset.
/// Linux callers independently enforce descriptor write rights and bounds.
pub fn resize(h: FileHandle, length: usize) -> Result<(), Error> {
    // Keep storage errors distinct for Linux errno mapping at its boundary.
    unsafe { (&mut *FILES.0.get()).resize(&mut Disk, h, length) }
}
pub fn seek(h: FileHandle, offset: i64, origin: u64) -> Result<u64, Status> {
    unsafe {
        (&mut *FILES.0.get())
            .seek(h, offset, origin)
            .map_err(status)
    }
}
pub fn info(h: FileHandle) -> Result<(usize, usize), Status> {
    unsafe { (&mut *FILES.0.get()).info(h).map_err(status) }
}
pub fn inode(h: FileHandle) -> Result<u64, Status> {
    unsafe { (&mut *FILES.0.get()).inode(h).map_err(status) }
}
pub fn link_count(h: FileHandle) -> Result<u64, Status> {
    unsafe { (&mut *FILES.0.get()).link_count(h).map_err(status) }
}

/// Only the trusted Appearance adapter can commit the exact bounded record.
pub fn replace_preferences(bytes: &[u8; 32]) -> Result<(), Status> {
    let result = unsafe {
        (&mut *FILES.0.get())
            .replace(&mut Disk, PREFERENCES_PATH, bytes)
            .map_err(status)
    };
    trace_namespace(crate::trace::id::FILE_REPLACED, result, bytes.len());
    result
}
pub fn directory_action(
    action: prism::files_directory::Action,
) -> Result<prism::files_directory::Listing, prism::files_directory::BackendError> {
    crate::filesystem::with_directory_store(|store, owner| {
        gui_directories::perform(store, owner, action)
    })
}
pub fn inspect_gui(
    path: prism::files_actions::Path,
) -> Result<Option<prism::files_actions::Snapshot>, prism::files_actions::BackendError> {
    crate::filesystem::with_gui_store(|store| gui_files::inspect(store, path))
}
pub fn commit_gui(
    intent: prism::files_actions::Intent,
) -> Result<(), prism::files_actions::BackendError> {
    if let prism::files_actions::Intent::Copy {
        source,
        destination,
        replace,
    } = intent
    {
        return copy_gui(source, destination, replace);
    }
    crate::filesystem::with_gui_store(|store| unsafe {
        gui_files::commit(&mut *FILES.0.get(), store, intent)
    })
}

/// Trusted UI only: same journal guard spans source read, conditional commit
/// and open-inode cache publication. No ambient application grant is created.
pub fn copy_gui(
    source: prism::files_actions::Snapshot,
    destination: prism::files_actions::Path,
    replace: Option<prism::files_actions::Snapshot>,
) -> Result<(), prism::files_actions::BackendError> {
    crate::filesystem::with_gui_store(|store| unsafe {
        gui_copy::copy(
            &mut *FILES.0.get(),
            store,
            gui_copy::CopyRequest {
                source,
                destination,
                replace,
            },
        )
    })
}

pub fn list_trash() -> Result<prism::trash_ui::Listing, prism::files_actions::BackendError> {
    crate::filesystem::with_gui_store(trash::list)
}
pub fn commit_trash(
    intent: prism::trash_ui::Intent,
) -> Result<(), prism::files_actions::BackendError> {
    crate::filesystem::with_gui_store(|store| unsafe {
        (&mut *TRASH_UNDO.0.get()).commit(&mut *FILES.0.get(), store, intent)
    })
}

/// Trusted UI receipt; no application authority comes from this metadata.
/// Any intervening mutation or a read-only fence invalidates the session undo.
pub fn trash_undo_available(
) -> Result<Option<prism::trash_ui::UndoReceipt>, prism::files_actions::BackendError> {
    crate::filesystem::with_gui_store(|store| unsafe {
        let history = &mut *TRASH_UNDO.0.get();
        let Some(receipt) = history.available() else {
            return Ok(None);
        };
        if store.is_read_only() || receipt.generation != store.generation() {
            history.clear();
            return Ok(None);
        }
        Ok(Some(prism::trash_ui::UndoReceipt {
            generation: receipt.generation,
            path: receipt.path,
            kind: match receipt.kind {
                trash_undo::Kind::Move => prism::trash_ui::UndoKind::Move,
                trash_undo::Kind::Restore => prism::trash_ui::UndoKind::Restore,
            },
        }))
    })
}
/// One receipt, one coherent transaction, and publication only after success.
/// An error never synthesizes an undo or replays a previously consumed receipt.
pub fn undo_trash(expected_generation: u64) -> Result<(), prism::files_actions::BackendError> {
    crate::filesystem::with_gui_store(|store| unsafe {
        (&mut *TRASH_UNDO.0.get()).undo(&mut *FILES.0.get(), store, expected_generation)
    })
}
