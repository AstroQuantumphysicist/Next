//! Persistent bounded development COW store, separate from the read-only ESP.
//! This is not the complete NextFS, GPT discovery, or a POSIX filesystem.
use crate::storage;
use core::{
    cell::UnsafeCell,
    fmt::{self, Write},
};
use next_fs::{
    journal::{
        self,
        locking::{MutationGuard, MutationLock},
        FileInfo, Partition, Store, WritableBlockDevice,
    },
    BlockDevice,
};

const ESP_SECTORS: u64 = 131072;
const DEVELOPMENT_SECTORS: u64 = 133120;
pub(crate) struct Device {
    capacity: u64,
    write_start: u64,
    write_sectors: u64,
    writable: bool,
}
impl BlockDevice for Device {
    type Error = storage::StorageError;
    fn sector_count(&self) -> u64 {
        self.capacity
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
        if lba >= self.capacity {
            return Err(storage::StorageError::SectorOutOfRange);
        }
        // Exclusively owned boot-CPU adapter; storage serializes its DMA buffers.
        unsafe { storage::read_sector(lba, out) }
    }
}
impl WritableBlockDevice for Device {
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), Self::Error> {
        if !self.writable {
            return Err(storage::StorageError::ReadOnly);
        }
        if lba >= self.capacity
            || lba < self.write_start
            || lba - self.write_start >= self.write_sectors
        {
            return Err(storage::StorageError::SectorOutOfRange);
        }
        unsafe { storage::write_sector(lba, data) }
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        if !self.writable {
            return Err(storage::StorageError::ReadOnly);
        }
        unsafe { storage::flush() }
    }
}
struct Global<T>(UnsafeCell<T>);
// Main boot CPU owns the adapter; LOCK excludes overlapping references. No
// IRQ or AP accesses this filesystem. BSP native calls use the same guard.
unsafe impl<T> Sync for Global<T> {}
static DEVICE: Global<Device> = Global(UnsafeCell::new(Device {
    capacity: 0,
    write_start: 0,
    write_sectors: 0,
    writable: false,
}));
static STORE: Global<Option<Store<'static, Device>>> = Global(UnsafeCell::new(None));
static LOCK: MutationLock = MutationLock::new();
struct Guard {
    _held: MutationGuard<'static>,
}
impl Guard {
    fn acquire() -> Result<Self, FsError> {
        LOCK.try_acquire()
            .map(|held| Self { _held: held })
            .map_err(|_| FsError::Busy)
    }
}

#[derive(Debug)]
pub enum FsError {
    Journal(journal::Error<storage::StorageError>),
    Device(storage::StorageError),
    Geometry,
    Busy,
    AlreadyInitialized,
    Unavailable,
    InvalidCommand,
    NonUtf8,
    SelfTest,
    Stream(next_abi::Status),
    Namespace,
}
impl From<journal::Error<storage::StorageError>> for FsError {
    fn from(error: journal::Error<storage::StorageError>) -> Self {
        Self::Journal(error)
    }
}
impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => write!(f, "{error:?}"),
            Self::Device(error) => write!(f, "{error}"),
            Self::Geometry => f.write_str("unrecognized development image geometry"),
            Self::Busy => f.write_str("busy"),
            Self::AlreadyInitialized => f.write_str("already initialized"),
            Self::Unavailable => f.write_str("unavailable"),
            Self::InvalidCommand => f.write_str("invalid command arguments"),
            Self::NonUtf8 => f.write_str("file is not UTF-8 text"),
            Self::Stream(error) => write!(f, "{error:?}"),
            Self::SelfTest => f.write_str("persistence verification failed"),
            Self::Namespace => f.write_str("invalid or unavailable directory namespace"),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct InitInfo {
    pub generation: u64,
    pub read_only: bool,
    pub formatted: bool,
    pub self_test: bool,
}

fn development_geometry(capacity: u64, bpb: &[u8; 512]) -> bool {
    capacity == DEVELOPMENT_SECTORS
        && !(0..4).any(|slot| bpb[446 + slot * 16 + 4] == 0xee)
        && bpb[510..512] == [0x55, 0xaa]
        && bpb[11..13] == 512u16.to_le_bytes()
        && bpb[32..36] == (ESP_SECTORS as u32).to_le_bytes()
        && &bpb[71..82] == b"NEXTDEV    "
        && &bpb[82..90] == b"FAT32   "
}

/// Authorize only the generated development image's trailing data extent.
/// Existing nonzero metadata is mounted, never reformatted on error.
/// # Safety
/// Must run on the boot CPU in ring 0 with sole ownership of the block device,
/// coherent identity-mapped DMA, and no filesystem calls from IRQ or AP paths.
pub unsafe fn initialize(capacity: u64) -> Result<InitInfo, FsError> {
    let _guard = Guard::acquire()?;
    if (*STORE.0.get()).is_some() {
        return Err(FsError::AlreadyInitialized);
    }
    let mut bpb = [0; 512];
    storage::read_sector(0, &mut bpb).map_err(FsError::Device)?;
    if !development_geometry(capacity, &bpb) {
        return Err(FsError::Geometry);
    }
    let mut virgin = true;
    for relative in 0..journal::REQUIRED_SECTORS {
        let mut sector = [0; 512];
        storage::read_sector(ESP_SECTORS + relative, &mut sector).map_err(FsError::Device)?;
        virgin &= sector.iter().all(|byte| *byte == 0);
    }
    (*DEVICE.0.get()).capacity = capacity;
    (*DEVICE.0.get()).write_start = ESP_SECTORS;
    (*DEVICE.0.get()).write_sectors = journal::REQUIRED_SECTORS;
    (*DEVICE.0.get()).writable = true;
    let device = &mut *DEVICE.0.get();
    let partition = Partition::new(
        device,
        ESP_SECTORS,
        journal::REQUIRED_SECTORS,
        &[(0, ESP_SECTORS)],
    )?;
    let mut store = if virgin {
        Store::format(partition)?
    } else {
        Store::mount(partition)?
    };
    if virgin {
        let initial = store.generation();
        store.put("/.__boot_probe", b"persistent Next development COW I/O")?;
        let mut read = [0; 64];
        let size = store.read_file("/.__boot_probe", &mut read)?;
        if &read[..size] != b"persistent Next development COW I/O" {
            return Err(FsError::SelfTest);
        }
        store.rename("/.__boot_probe", "/.__boot_renamed")?;
        if !matches!(
            store.read_file("/.__boot_probe", &mut read),
            Err(journal::Error::NotFound)
        ) {
            return Err(FsError::SelfTest);
        }
        let size = store.read_file("/.__boot_renamed", &mut read)?;
        if &read[..size] != b"persistent Next development COW I/O" {
            return Err(FsError::SelfTest);
        }
        store.remove("/.__boot_renamed")?;
        let expected = initial + 3;
        store = Store::mount(store.into_partition())?;
        let mut entries = [FileInfo::EMPTY; journal::MAX_FILES];
        if store.generation() != expected || store.is_read_only() || store.list(&mut entries)? != 0
        {
            return Err(FsError::SelfTest);
        }
    }
    let info = InitInfo {
        generation: store.generation(),
        read_only: store.is_read_only(),
        formatted: virgin,
        self_test: virgin,
    };
    *STORE.0.get() = Some(store);
    Ok(info)
}

/// Mount an existing journal only after owner-signature and exact GPT binding.
/// This entry point never formats zero data or repairs corrupt data. The device
/// adapter independently excludes every write outside the bounded journal.
///
/// # Safety
/// Same boot-CPU ownership and DMA requirements as initialize. Capacity must be
/// from the same probed device whose GPT was used to bind the opaque grant.
pub unsafe fn initialize_authenticated_extent(
    capacity: u64,
    extent: crate::storage_policy::AuthenticatedExtent,
) -> Result<InitInfo, FsError> {
    let _guard = Guard::acquire()?;
    if (*STORE.0.get()).is_some() {
        return Err(FsError::AlreadyInitialized);
    }
    let start = extent.start();
    let count = extent.sectors();
    let end = start.checked_add(count).ok_or(FsError::Geometry)?;
    if extent.capacity() != capacity || count != journal::REQUIRED_SECTORS || end > capacity {
        return Err(FsError::Geometry);
    }
    *DEVICE.0.get() = Device {
        capacity,
        write_start: start,
        write_sectors: count,
        writable: extent.writable(),
    };
    let device = &mut *DEVICE.0.get();
    // Protect all media outside the fixed prefix, including GPT headers, ESP,
    // other partitions and the remainder of a larger NEXT_DATA partition.
    let store = crate::storage_policy::mount_existing(device, &extent)?;
    let info = InitInfo {
        generation: store.generation(),
        read_only: store.is_read_only(),
        formatted: false,
        self_test: false,
    };
    *STORE.0.get() = Some(store);
    Ok(info)
}

fn stream_error(error: FsError) -> helix_core::file::Error {
    use helix_core::file::Error as E;
    match error {
        FsError::Journal(journal::Error::NotFound) => E::NotFound,
        FsError::Journal(journal::Error::InvalidPath) => E::InvalidPath,
        FsError::Journal(journal::Error::TooLarge | journal::Error::BufferTooSmall) => E::TooLarge,
        FsError::Journal(journal::Error::Full) => E::Full,
        FsError::Journal(journal::Error::ReadOnly) => E::ReadOnly,
        FsError::Busy => E::Busy,
        FsError::Namespace => E::InvalidPath,
        _ => E::Io,
    }
}

// This build has one mounted development volume. Its mount policy supplies a
// domain-separated owner id; records cannot choose or authenticate ownership.
pub(crate) const DEVELOPMENT_VOLUME_OWNER: u64 = 0x4e45_5854_4445_5631;
fn directory_error(error: next_fs::journal::directories::Error<storage::StorageError>) -> FsError {
    use next_fs::journal::directories::Error as D;
    match error {
        D::Store(error) => FsError::Journal(error),
        D::Full => FsError::Journal(journal::Error::Full),
        D::Exists => FsError::Journal(journal::Error::Exists),
        D::NotFound => FsError::Journal(journal::Error::NotFound),
        D::Conflict => FsError::Namespace,
        D::Path | D::Owner | D::Corrupt | D::MissingParent | D::NotDirectory | D::NotEmpty => {
            FsError::Namespace
        }
    }
}

fn reserved_directory_metadata(path: &str) -> bool {
    path == "/dirs" || path.starts_with("/dirs/")
}
fn native_namespace_path(path: &str) -> bool {
    path == "/native" || path.starts_with("/native/")
}

/// Validate a native path against persisted ancestry and file-vs-directory
/// type while the Store guard is held. `None` means a vacant leaf.
fn checked_native_leaf(
    store: &mut Store<'_, Device>,
    path: &str,
) -> Result<Option<next_fs::journal::directories::Entry>, FsError> {
    use next_fs::journal::directories::{self, Kind};
    if reserved_directory_metadata(path) || !path.starts_with("/native/") {
        return Err(FsError::Namespace);
    }
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let listing =
        directories::list(store, DEVELOPMENT_VOLUME_OWNER, parent).map_err(directory_error)?;
    let leaf = listing
        .entries
        .iter()
        .take(listing.count)
        .find(|entry| entry.path() == path)
        .copied();
    if leaf.is_some_and(|entry| entry.kind != Kind::File) {
        return Err(FsError::Namespace);
    }
    Ok(leaf)
}

/// Run trusted directory actions under the same journal guard as file streams.
/// The owner comes from mount policy and is not exposed to UI or user input.
pub(crate) fn with_directory_store<T>(
    operation: impl FnOnce(
        &mut Store<'_, Device>,
        u64,
    ) -> Result<T, prism::files_directory::BackendError>,
) -> Result<T, prism::files_directory::BackendError> {
    use prism::files_directory::BackendError as E;
    let _guard = Guard::acquire().map_err(|_| E::Busy)?;
    let store = unsafe { &mut *STORE.0.get() }
        .as_mut()
        .ok_or(E::Unavailable)?;
    operation(store, DEVELOPMENT_VOLUME_OWNER)
}

pub fn stream_read(
    path: &str,
    out: &mut [u8; journal::MAX_FILE_BYTES],
) -> Result<usize, helix_core::file::Error> {
    (|| {
        let _guard = Guard::acquire()?;
        let store = unsafe { &mut *STORE.0.get() }
            .as_mut()
            .ok_or(FsError::Unavailable)?;
        if reserved_directory_metadata(path) || path == "/native" {
            return Err(FsError::Namespace);
        }
        if path.starts_with("/native/") && checked_native_leaf(store, path)?.is_none() {
            return Err(FsError::Journal(journal::Error::NotFound));
        }
        store.read_file(path, out).map_err(FsError::from)
    })()
    .map_err(stream_error)
}
pub fn stream_put(path: &str, bytes: &[u8]) -> Result<(), helix_core::file::Error> {
    (|| {
        let _guard = Guard::acquire()?;
        let store = unsafe { &mut *STORE.0.get() }
            .as_mut()
            .ok_or(FsError::Unavailable)?;
        if reserved_directory_metadata(path) || path == "/native" {
            return Err(FsError::Namespace);
        }
        if path.starts_with("/native/") && checked_native_leaf(store, path)?.is_none() {
            let generation = store.generation();
            next_fs::journal::directories::create_file(
                store,
                DEVELOPMENT_VOLUME_OWNER,
                generation,
                path,
                bytes,
            )
            .map_err(directory_error)?;
            return Ok(());
        }
        if path.starts_with("/trash/") {
            crate::file::trash::write_held(store, path, bytes).map_err(|error| match error {
                prism::files_actions::BackendError::ReadOnly => {
                    FsError::Journal(journal::Error::ReadOnly)
                }
                prism::files_actions::BackendError::Full => FsError::Journal(journal::Error::Full),
                _ => FsError::Stream(next_abi::Status::IoError),
            })
        } else {
            store.put(path, bytes).map_err(FsError::from)
        }
    })()
    .map_err(stream_error)
}

pub fn stream_rename(old: &str, new: &str) -> Result<(), helix_core::file::Error> {
    (|| {
        let _guard = Guard::acquire()?;
        let store = unsafe { &mut *STORE.0.get() }
            .as_mut()
            .ok_or(FsError::Unavailable)?;
        if reserved_directory_metadata(old)
            || reserved_directory_metadata(new)
            || old == "/native"
            || new == "/native"
            || native_namespace_path(old) != native_namespace_path(new)
        {
            return Err(FsError::Namespace);
        }
        if old.starts_with("/native/") {
            if checked_native_leaf(store, old)?.is_none() {
                return Err(FsError::Journal(journal::Error::NotFound));
            }
            if !new.starts_with("/native/") {
                return Err(FsError::Namespace);
            }
            let _ = checked_native_leaf(store, new)?;
        }
        store.rename(old, new).map_err(FsError::from)
    })()
    .map_err(stream_error)
}
pub fn stream_remove(path: &str) -> Result<(), helix_core::file::Error> {
    (|| {
        let _guard = Guard::acquire()?;
        let store = unsafe { &mut *STORE.0.get() }
            .as_mut()
            .ok_or(FsError::Unavailable)?;
        if reserved_directory_metadata(path) || path == "/native" {
            return Err(FsError::Namespace);
        }
        if path.starts_with("/native/") && checked_native_leaf(store, path)?.is_none() {
            return Err(FsError::Journal(journal::Error::NotFound));
        }
        store.remove(path).map_err(FsError::from)
    })()
    .map_err(stream_error)
}
pub fn generation() -> Result<u64, FsError> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() }
        .as_mut()
        .ok_or(FsError::Unavailable)?;
    Ok(store.generation())
}

pub struct Reply {
    bytes: [u8; 4096],
    len: usize,
}
impl Reply {
    fn new() -> Self {
        Self {
            bytes: [0; 4096],
            len: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("Invalid UTF-8 response")
    }
}
impl Write for Reply {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.len() > self.bytes.len() - self.len {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
fn arguments(text: &str) -> Result<(&str, &str), FsError> {
    let (left, right) = text.split_once(' ').ok_or(FsError::InvalidCommand)?;
    if left.is_empty() || right.is_empty() {
        return Err(FsError::InvalidCommand);
    }
    Ok((left, right))
}
fn execute(command: &str, reply: &mut Reply) -> Result<(), FsError> {
    if command == "ls" {
        let _guard = Guard::acquire()?;
        let store = unsafe { &mut *STORE.0.get() }
            .as_mut()
            .ok_or(FsError::Unavailable)?;
        let mut entries = [FileInfo::EMPTY; journal::MAX_FILES];
        let count = store.list(&mut entries)?;
        let visible = entries[..count]
            .iter()
            .filter(|entry| !reserved_directory_metadata(entry.path()));
        let visible_count = visible.clone().count();
        if visible_count == 0 {
            let _ = reply.write_str("No files in development data store");
        }
        for info in visible {
            let _ = writeln!(reply, "{} {} bytes", info.path(), info.size);
        }
    } else if let Some(path) = command.strip_prefix("cat ") {
        let mut data = [0; journal::MAX_FILE_BYTES];
        let size = crate::file::read_path(path, &mut data).map_err(FsError::Stream)?;
        let text = core::str::from_utf8(&data[..size]).map_err(|_| FsError::NonUtf8)?;
        let _ = reply.write_str(text);
    } else if let Some(text) = command.strip_prefix("write ") {
        let (path, data) = arguments(text)?;
        crate::file::replace(path, data.as_bytes()).map_err(FsError::Stream)?;
        let _ = write!(
            reply,
            "Wrote {} bytes to {}; generation={}",
            data.len(),
            path,
            generation()?
        );
    } else if let Some(text) = command.strip_prefix("mv ") {
        let (old, new) = arguments(text)?;
        if new.contains(' ') {
            return Err(FsError::InvalidCommand);
        }
        crate::file::rename(old, new).map_err(FsError::Stream)?;
        let _ = write!(
            reply,
            "Renamed {} to {}; generation={}",
            old,
            new,
            generation()?
        );
    } else if let Some(path) = command.strip_prefix("rm ") {
        crate::file::remove(path).map_err(FsError::Stream)?;
        let _ = write!(reply, "Removed {}; generation={}", path, generation()?);
    } else {
        return Err(FsError::InvalidCommand);
    }
    Ok(())
}
/// Execute a recognized development filesystem command with bounded output.
/// The store retains indeterminate/read-only failures; errors never autoformat.
pub fn command(command: &str) -> Option<Reply> {
    if command != "ls"
        && !["cat ", "write ", "mv ", "rm "]
            .iter()
            .any(|prefix| command.starts_with(prefix))
    {
        return None;
    }
    let mut reply = Reply::new();
    let result = execute(command, &mut reply);
    if let Err(error) = result {
        reply = Reply::new();
        let _ = write!(reply, "Data store error: {}", error);
    }
    Some(reply)
}

/// An occupied slot carries actual verified disk metadata and a UTF-8 excerpt.
/// `size` is the full file length; `len` bounds the excerpt, without implying
/// that a truncated preview represents the complete file.
#[derive(Clone, Copy)]
pub struct FileSnapshot {
    pub path: [u8; 32],
    pub content: [u8; 128],
    pub len: usize,
    pub size: u32,
    pub binary: bool,
    pub valid: bool,
}
impl FileSnapshot {
    const EMPTY: Self = Self {
        path: [0; 32],
        content: [0; 128],
        len: 0,
        size: 0,
        binary: false,
        valid: false,
    };
    pub fn path(&self) -> &str {
        let end = self
            .path
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.path.len());
        core::str::from_utf8(&self.path[..end]).unwrap_or("")
    }
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.content[..self.len.min(self.content.len())]).unwrap_or("")
    }
    fn from_file(info: FileInfo, data: &[u8]) -> Result<Self, FsError> {
        if data.len() != info.size as usize {
            return Err(FsError::SelfTest);
        }
        let mut snapshot = Self {
            path: info.name,
            size: info.size,
            valid: true,
            ..Self::EMPTY
        };
        let Ok(text) = core::str::from_utf8(data) else {
            snapshot.binary = true;
            return Ok(snapshot);
        };
        if data
            .iter()
            .any(|byte| byte.is_ascii_control() && !matches!(*byte, b'\n' | b'\r' | b'\t'))
        {
            snapshot.binary = true;
            return Ok(snapshot);
        }
        let mut len = text.len().min(snapshot.content.len());
        while !text.is_char_boundary(len) {
            len -= 1;
        }
        snapshot.content[..len].copy_from_slice(&data[..len]);
        snapshot.len = len;
        Ok(snapshot)
    }
}

#[path = "files_query.rs"]
pub(crate) mod files_query;
#[derive(Clone, Copy)]
pub struct QuerySnapshot {
    pub stats: prism::files_query::Stats,
    pub files: [FileSnapshot; journal::MAX_FILES],
}
/// Actual checked, sorted and filtered listing under one journal guard.
/// Full-content scanning is opt-in and limited to /native development files.
#[inline(never)]
pub fn query_snapshot(query: prism::files_query::Query) -> Result<QuerySnapshot, FsError> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() }
        .as_mut()
        .ok_or(FsError::Unavailable)?;
    next_fs::journal::directories::list(store, DEVELOPMENT_VOLUME_OWNER, "/native")
        .map_err(directory_error)?;
    query_snapshots_from_store(store, query).map_err(FsError::from)
}
fn query_snapshots_from_store<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    query: prism::files_query::Query,
) -> Result<QuerySnapshot, journal::Error<D::Error>> {
    let (listing, files) =
        files_query::collect(store, query, FileSnapshot::EMPTY, |info, bytes| {
            FileSnapshot::from_file(info, bytes).map_err(|_| journal::Error::CorruptData)
        })?;
    Ok(QuerySnapshot {
        stats: prism::files_query::Stats {
            revision: listing.revision,
            total: listing.total,
            matched: listing.matched,
        },
        files,
    })
}

/// Obtain a consistent, bounded Files-panel view of the mounted data store.
/// Each payload is read and checked by the store; an I/O/checksum error
/// fails the snapshot instead of publishing stale or invented file contents.
/// Binary files retain verified metadata with an empty text preview.
pub fn snapshot() -> Result<[FileSnapshot; journal::MAX_FILES], FsError> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() }
        .as_mut()
        .ok_or(FsError::Unavailable)?;
    next_fs::journal::directories::list(store, DEVELOPMENT_VOLUME_OWNER, "/native")
        .map_err(directory_error)?;
    snapshots_from_store(store).map_err(FsError::from)
}
fn snapshots_from_store<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
) -> Result<[FileSnapshot; journal::MAX_FILES], journal::Error<D::Error>> {
    let mut metadata = [FileInfo::EMPTY; journal::MAX_FILES];
    let count = store.list(&mut metadata)?;
    let mut files = [FileSnapshot::EMPTY; journal::MAX_FILES];
    // Private Trash pairs have their own validated, bounded listing. They must
    // neither expose implementation keys nor consume normal Files bindings.
    let visible = metadata[..count].iter().filter(|info| {
        info.path() != "/trash"
            && !info.path().starts_with("/trash/")
            && info.path() != "/dirs"
            && !info.path().starts_with("/dirs/")
    });
    let mut data = [0; journal::MAX_FILE_BYTES];
    for (target, info) in files.iter_mut().zip(visible) {
        let len = store.read_file(info.path(), &mut data)?;
        *target = FileSnapshot::from_file(*info, &data[..len])
            .map_err(|_| journal::Error::CorruptData)?;
    }
    Ok(files)
}

/// One journal guard covers UI revision checks, persistent commit and coherent
/// file-cache publication. Filesystem operations never run from IRQ or an AP.
pub(crate) fn with_gui_store<T>(
    operation: impl FnOnce(&mut Store<'_, Device>) -> Result<T, prism::files_actions::BackendError>,
) -> Result<T, prism::files_actions::BackendError> {
    use prism::files_actions::BackendError;
    let _guard = Guard::acquire().map_err(|_| BackendError::Busy)?;
    let store = unsafe { &mut *STORE.0.get() }
        .as_mut()
        .ok_or(BackendError::Unavailable)?;
    operation(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_write_authority_refuses_outside_prefix_and_read_only_before_port_io() {
        let mut device = Device {
            capacity: 1000,
            write_start: 100,
            write_sectors: journal::REQUIRED_SECTORS,
            writable: true,
        };
        for lba in [0, 99, 232, 999, 1000, u64::MAX] {
            assert_eq!(
                device.write_sector(lba, &[0; 512]),
                Err(storage::StorageError::SectorOutOfRange)
            );
        }
        device.writable = false;
        for lba in [100, 231] {
            assert_eq!(
                device.write_sector(lba, &[0; 512]),
                Err(storage::StorageError::ReadOnly)
            );
        }
        assert_eq!(device.flush(), Err(storage::StorageError::ReadOnly));
        assert_eq!(
            device.read_sector(1000, &mut [0; 512]),
            Err(storage::StorageError::SectorOutOfRange)
        );
    }
    #[test]
    fn only_explicit_development_geometry_is_authorized() {
        let mut bpb = [0; 512];
        bpb[510..].copy_from_slice(&[0x55, 0xaa]);
        bpb[11..13].copy_from_slice(&512u16.to_le_bytes());
        bpb[32..36].copy_from_slice(&(ESP_SECTORS as u32).to_le_bytes());
        bpb[71..82].copy_from_slice(b"NEXTDEV    ");
        bpb[82..90].copy_from_slice(b"FAT32   ");
        assert!(development_geometry(DEVELOPMENT_SECTORS, &bpb));
        assert!(!development_geometry(DEVELOPMENT_SECTORS + 1, &bpb));
        assert!(!development_geometry(ESP_SECTORS, &bpb));
        for offset in [11, 32, 71, 82, 510] {
            let mut bad = bpb;
            bad[offset] ^= 1;
            assert!(!development_geometry(DEVELOPMENT_SECTORS, &bad));
        }
        for slot in 0..4 {
            let mut forged = bpb;
            forged[446 + slot * 16 + 4] = 0xee;
            assert!(!development_geometry(DEVELOPMENT_SECTORS, &forged));
        }
    }
    #[test]
    fn reply_bounds_preserve_complete_utf8_and_command_arguments() {
        let mut reply = Reply::new();
        reply.write_str("ok").unwrap();
        assert!(reply.write_str(&"x".repeat(4095)).is_err());
        assert_eq!(reply.as_str(), "ok");
        assert_eq!(
            arguments("/file many words here").unwrap(),
            ("/file", "many words here")
        );
        assert!(arguments("/file").is_err());
        assert!(arguments("/file ").is_err());
    }
    #[test]
    fn snapshots_preserve_verified_lengths_and_utf8_boundaries() {
        let mut info = FileInfo::EMPTY;
        info.name[..5].copy_from_slice(b"/note");
        let text = "a".repeat(127) + "€tail";
        info.size = text.len() as u32;
        let preview = FileSnapshot::from_file(info, text.as_bytes()).unwrap();
        assert!(preview.valid);
        assert_eq!(preview.path(), "/note");
        assert_eq!(preview.size, text.len() as u32);
        assert_eq!(preview.len, 127);
        assert_eq!(preview.text(), "a".repeat(127));
        assert!(preview.content[preview.len..].iter().all(|byte| *byte == 0));
        let text = "€".repeat(42) + "aa";
        info.size = 128;
        let preview = FileSnapshot::from_file(info, text.as_bytes()).unwrap();
        assert_eq!(preview.len, 128);
        assert_eq!(preview.text(), text);
        info.size = 0;
        let empty_file = FileSnapshot::from_file(info, b"").unwrap();
        assert!(empty_file.valid);
        assert_eq!(empty_file.text(), "");
    }
    #[test]
    fn snapshots_keep_binary_metadata_and_refuse_inconsistent_lengths() {
        let mut info = FileInfo::EMPTY;
        info.size = 1;
        let binary = FileSnapshot::from_file(info, &[0xff]).unwrap();
        assert!(binary.valid && binary.binary);
        assert_eq!(binary.size, 1);
        assert_eq!(binary.text(), "");
        assert!(binary.content.iter().all(|byte| *byte == 0));
        assert!(matches!(
            FileSnapshot::from_file(info, b"long"),
            Err(FsError::SelfTest)
        ));
    }
    #[test]
    fn preference_records_are_listed_without_poisoning_other_previews() {
        let record = prism::appearance::Preferences::defaults().encode();
        let mut info = FileInfo::EMPTY;
        info.name[..17].copy_from_slice(b"/prefs/appearance");
        info.size = record.len() as u32;
        let binary = FileSnapshot::from_file(info, &record).unwrap();
        assert_eq!(binary.path(), "/prefs/appearance");
        assert_eq!(binary.size, 32);
        assert!(binary.valid && binary.binary);
        assert_eq!(binary.text(), "");
        info.size = 3;
        let text = FileSnapshot::from_file(info, b"a\nb").unwrap();
        assert!(text.valid && !text.binary);
        assert_eq!(text.text(), "a\nb");
        let controls = FileSnapshot::from_file(info, b"a\0b").unwrap();
        assert!(controls.valid && controls.binary);
    }
    #[test]
    fn normal_files_snapshot_excludes_private_trash_pairs_without_hiding_native_neighbors() {
        struct Ram(std::vec::Vec<[u8; 512]>);
        impl next_fs::BlockDevice for Ram {
            type Error = ();
            fn sector_count(&self) -> u64 {
                self.0.len() as u64
            }
            fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
                *out = self.0[lba as usize];
                Ok(())
            }
        }
        impl WritableBlockDevice for Ram {
            fn write_sector(&mut self, lba: u64, bytes: &[u8; 512]) -> Result<(), ()> {
                self.0[lba as usize] = *bytes;
                Ok(())
            }
            fn flush(&mut self) -> Result<(), ()> {
                Ok(())
            }
        }
        let mut ram = Ram(std::vec![[0;512];journal::REQUIRED_SECTORS as usize]);
        let mut store =
            Store::format(Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap())
                .unwrap();
        store.put("/trash/d.0000000000000001", b"private").unwrap();
        store
            .put("/trash/m.0000000000000001", &[0xff; 128])
            .unwrap();
        store.put("/native/visible", b"real file").unwrap();
        store.put("/native/trash-neighbor", b"neighbor").unwrap();
        let files = snapshots_from_store(&mut store).unwrap();
        let visible: std::vec::Vec<_> = files.iter().filter(|file| file.valid).collect();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].path(), "/native/visible");
        assert_eq!(visible[0].text(), "real file");
        assert_eq!(visible[1].path(), "/native/trash-neighbor");
        assert!(files.iter().all(|file| !file.path().starts_with("/trash/")));

        let text = "a".repeat(127) + "€" + &"b".repeat(3000) + "SeCrEt";
        store.put("/native/long", text.as_bytes()).unwrap();
        let mut query = prism::files_query::Query::new();
        query.set_text("secret").unwrap();
        query.search = prism::files_query::Search::PathAndContent;
        let result = query_snapshots_from_store(&mut store, query).unwrap();
        assert_eq!(result.stats.revision, store.generation());
        assert_eq!(result.stats.total, 3);
        assert_eq!(result.stats.matched, 1);
        assert_eq!(result.files[0].path(), "/native/long");
        assert_eq!(result.files[0].size as usize, text.len());
        assert_eq!(result.files[0].len, 127);
        assert_eq!(result.files[0].text(), "a".repeat(127));
        assert!(!result.files[0].binary);
        assert!(result.files[1..].iter().all(|file| !file.valid));
        query.set_text("trash").unwrap();
        let result = query_snapshots_from_store(&mut store, query).unwrap();
        assert_eq!(result.stats.matched, 1);
        assert_eq!(result.files[0].path(), "/native/trash-neighbor");
    }
}

/// Read-only bounded Linux directory snapshot from validated native ancestry.
/// No user path can escape the owner-authenticated development volume.
#[cfg(target_os = "none")]
pub(crate) fn linux_directory(path: &str) -> Result<crate::linux_fs::dirent::Snapshot, u64> {
    use crate::linux_fs::dirent::{Entry, Snapshot};
    use next_fs::journal::directories::{self, Error, Kind};
    let errno = |code: u64| 0u64.wrapping_sub(code);
    let _guard = Guard::acquire().map_err(|_| errno(16))?;
    let store = unsafe { &mut *STORE.0.get() }.as_mut().ok_or(errno(5))?;
    let listing = directories::list(store, DEVELOPMENT_VOLUME_OWNER, path).map_err(|error| {
        errno(match error {
            Error::NotFound | Error::MissingParent => 2,
            Error::NotDirectory => 20,
            Error::Path => 22,
            _ => 5,
        })
    })?;
    let inode = |name: &str| {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in name.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        hash | (1 << 63)
    };
    let mut snapshot = Snapshot::EMPTY;
    snapshot
        .push(Entry::new(inode(path), 4, ".").ok_or(errno(5))?)
        .ok_or(errno(5))?;
    let parent = if path == "/native" {
        "/native"
    } else {
        path.rsplit_once('/')
            .map_or("/native", |(parent, _)| parent)
    };
    snapshot
        .push(Entry::new(inode(parent), 4, "..").ok_or(errno(5))?)
        .ok_or(errno(5))?;
    for entry in &listing.entries[..listing.count] {
        let name = entry.path().rsplit('/').next().ok_or(errno(5))?;
        snapshot
            .push(
                Entry::new(
                    inode(entry.path()),
                    if entry.kind == Kind::Directory { 4 } else { 8 },
                    name,
                )
                .ok_or(errno(5))?,
            )
            .ok_or(errno(5))?;
    }
    Ok(snapshot)
}
