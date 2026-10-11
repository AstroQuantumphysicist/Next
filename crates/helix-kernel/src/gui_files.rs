//! Trusted Files UI transactions. The caller holds the journal guard through
//! validation, durable mutation and coherent VFS namespace-cache publication.
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::journal::{self, Store, WritableBlockDevice};
use prism::files_actions::{BackendError, Intent, Path, Snapshot};
// Matches the sole development-volume mount policy identity in filesystem.rs.
const DIRECTORY_OWNER: u64 = 0x4e45_5854_4445_5631;
fn journal_error<E>(error: journal::Error<E>) -> BackendError {
    match error {
        journal::Error::ReadOnly => BackendError::ReadOnly,
        journal::Error::NotFound => BackendError::NotFound,
        journal::Error::Exists => BackendError::Exists,
        journal::Error::Full => BackendError::Full,
        journal::Error::InvalidPath | journal::Error::TooLarge | journal::Error::BufferTooSmall => {
            BackendError::InvalidPath
        }
        _ => BackendError::Io,
    }
}
fn directory_error<E>(error: next_fs::journal::directories::Error<E>) -> BackendError {
    use next_fs::journal::directories::Error as D;
    match error {
        D::Store(error) => journal_error(error),
        D::Full => BackendError::Full,
        D::NotFound => BackendError::NotFound,
        D::Exists => BackendError::Exists,
        D::Conflict => BackendError::Conflict,
        D::Path | D::Owner | D::Corrupt | D::MissingParent | D::NotDirectory | D::NotEmpty => {
            BackendError::InvalidPath
        }
    }
}
fn stream_error(error: file::Error) -> BackendError {
    match error {
        file::Error::ReadOnly => BackendError::ReadOnly,
        file::Error::NotFound => BackendError::NotFound,
        file::Error::Exists => BackendError::Exists,
        file::Error::Full => BackendError::Full,
        file::Error::Busy => BackendError::Busy,
        file::Error::InvalidPath | file::Error::TooLarge => BackendError::InvalidPath,
        _ => BackendError::Io,
    }
}
fn permits(path: Path) -> Result<(), BackendError> {
    if path.as_str().starts_with("/native/") {
        Ok(())
    } else {
        Err(BackendError::ReadOnly)
    }
}
fn checked_native_leaf<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    path: Path,
) -> Result<Option<next_fs::journal::directories::Entry>, BackendError> {
    use next_fs::journal::directories::{self, Kind};
    permits(path)?;
    let parent = path
        .as_str()
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    let listing = directories::list(store, DIRECTORY_OWNER, parent).map_err(directory_error)?;
    let leaf = listing
        .entries
        .iter()
        .take(listing.count)
        .find(|entry| entry.path() == path.as_str())
        .copied();
    if leaf.is_some_and(|entry| entry.kind != Kind::File) {
        return Err(BackendError::InvalidPath);
    }
    Ok(leaf)
}
pub(crate) fn inspect<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    path: Path,
) -> Result<Option<Snapshot>, BackendError> {
    permits(path)?;
    if checked_native_leaf(store, path)?.is_none() {
        return Ok(None);
    }
    // A snapshot is issued only after its actual payload checksum validates.
    let mut data = [0; FILE_BYTES];
    store
        .read_file(path.as_str(), &mut data)
        .map_err(journal_error)?;
    Ok(Some(Snapshot {
        path,
        revision: store.generation(),
    }))
}
struct ConditionalBackend<'s, 'd, D: WritableBlockDevice> {
    store: &'s mut Store<'d, D>,
    intent: Intent,
    error: Option<BackendError>,
}
impl<D: WritableBlockDevice> ConditionalBackend<'_, '_, D> {
    fn validate(&mut self) -> Result<(), BackendError> {
        match self.intent {
            Intent::Copy { .. } => return Err(BackendError::InvalidPath),
            Intent::Create { path } => {
                permits(path)?;
                if checked_native_leaf(self.store, path)?.is_some() {
                    return Err(BackendError::Exists);
                }
            }
            Intent::Rename {
                source,
                destination,
                replace,
            } => {
                permits(source.path)?;
                permits(destination)?;
                if source.revision != self.store.generation() {
                    return Err(BackendError::Conflict);
                }
                if checked_native_leaf(self.store, source.path)?.is_none() {
                    return Err(BackendError::NotFound);
                }
                let present = checked_native_leaf(self.store, destination)?.is_some();
                match replace {
                    Some(target)
                        if target.path != destination
                            || target.revision != self.store.generation()
                            || !present =>
                    {
                        return Err(BackendError::Conflict)
                    }
                    None if present => return Err(BackendError::Exists),
                    _ => {}
                }
            }
            Intent::Delete { source } => {
                permits(source.path)?;
                if source.revision != self.store.generation() {
                    return Err(BackendError::Conflict);
                }
                if checked_native_leaf(self.store, source.path)?.is_none() {
                    return Err(BackendError::NotFound);
                }
            }
        }
        if self.store.is_read_only() {
            return Err(BackendError::ReadOnly);
        }
        Ok(())
    }
    fn record(&mut self, result: Result<(), BackendError>) -> Result<(), file::Error> {
        result.map_err(|error| {
            self.error = Some(error);
            file::Error::Io
        })
    }
}
impl<D: WritableBlockDevice> Backend for ConditionalBackend<'_, '_, D> {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
        self.store.read_file(path, out).map_err(|_| file::Error::Io)
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), file::Error> {
        let result = (|| {
            if !matches!(self.intent, Intent::Create { path: expected } if expected.as_str() == path)
                || !bytes.is_empty()
            {
                return Err(BackendError::InvalidPath);
            }
            self.validate()?;
            let generation = self.store.generation();
            next_fs::journal::directories::create_file(
                self.store,
                DIRECTORY_OWNER,
                generation,
                path,
                bytes,
            )
            .map(|_| ())
            .map_err(directory_error)
        })();
        self.record(result)
    }
    fn rename(&mut self, old: &str, new: &str) -> Result<(), file::Error> {
        let result = (|| {
            if !matches!(self.intent, Intent::Rename { source, destination, .. } if source.path.as_str() == old && destination.as_str() == new)
            {
                return Err(BackendError::InvalidPath);
            }
            self.validate()?;
            self.store.rename(old, new).map_err(journal_error)
        })();
        self.record(result)
    }
    fn remove(&mut self, path: &str) -> Result<(), file::Error> {
        let result = (|| {
            if !matches!(self.intent, Intent::Delete { source } if source.path.as_str() == path) {
                return Err(BackendError::InvalidPath);
            }
            self.validate()?;
            self.store.remove(path).map_err(journal_error)
        })();
        self.record(result)
    }
}
pub(crate) fn commit<D: WritableBlockDevice, const N: usize>(
    files: &mut Files<N>,
    store: &mut Store<'_, D>,
    intent: Intent,
) -> Result<(), BackendError> {
    if let Intent::Copy {
        source,
        destination,
        replace,
    } = intent
    {
        return super::gui_copy::copy(
            files,
            store,
            super::gui_copy::CopyRequest {
                source,
                destination,
                replace,
            },
        );
    }
    let mut backend = ConditionalBackend {
        store,
        intent,
        error: None,
    };
    let result = match intent {
        Intent::Copy { .. } => return Err(BackendError::InvalidPath),
        Intent::Create { path } => files.replace(&mut backend, path.as_str(), b""),
        Intent::Rename {
            source,
            destination,
            ..
        } => files.rename(&mut backend, source.path.as_str(), destination.as_str()),
        Intent::Delete { source } => files.remove(&mut backend, source.path.as_str()),
    };
    result.map_err(|error| backend.error.unwrap_or_else(|| stream_error(error)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use next_fs::BlockDevice;
    struct Ram {
        sectors: std::vec::Vec<[u8; 512]>,
        fail: bool,
    }
    impl Ram {
        fn new() -> Self {
            Self {
                sectors: std::vec![[0;512]; journal::REQUIRED_SECTORS as usize],
                fail: false,
            }
        }
    }
    impl BlockDevice for Ram {
        type Error = ();
        fn sector_count(&self) -> u64 {
            self.sectors.len() as u64
        }
        fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
            *out = self.sectors[lba as usize];
            Ok(())
        }
    }
    impl WritableBlockDevice for Ram {
        fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), ()> {
            if self.fail {
                return Err(());
            }
            self.sectors[lba as usize] = *data;
            Ok(())
        }
        fn flush(&mut self) -> Result<(), ()> {
            if self.fail {
                Err(())
            } else {
                Ok(())
            }
        }
    }
    struct Disk<'s, 'd>(&'s mut Store<'d, Ram>);
    impl Backend for Disk<'_, '_> {
        fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
            self.0
                .read_file(path, out)
                .map_err(|_| file::Error::NotFound)
        }
        fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), file::Error> {
            self.0.put(path, bytes).map_err(|_| file::Error::Io)
        }
        fn rename(&mut self, old: &str, new: &str) -> Result<(), file::Error> {
            self.0.rename(old, new).map_err(|_| file::Error::Io)
        }
        fn remove(&mut self, path: &str) -> Result<(), file::Error> {
            self.0.remove(path).map_err(|_| file::Error::Io)
        }
    }
    fn p(path: &str) -> Path {
        Path::new(path).unwrap()
    }
    #[test]
    fn appearance_path_rejects_generic_mutation_and_coherent_record_replace_updates_open_inode() {
        assert_eq!(
            crate::file::replace("/prefs/appearance", b"poison"),
            Err(next_abi::Status::AccessDenied)
        );
        assert_eq!(
            crate::file::rename("/prefs/appearance", "/native/poison"),
            Err(next_abi::Status::AccessDenied)
        );
        assert_eq!(
            crate::file::rename("/native/poison", "/prefs/appearance"),
            Err(next_abi::Status::AccessDenied)
        );
        assert_eq!(
            crate::file::remove("/prefs/appearance"),
            Err(next_abi::Status::AccessDenied)
        );
        let mut ram = Ram::new();
        let mut store = Store::format(
            journal::Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let mut files = Files::<16>::new();
        files
            .replace(&mut Disk(&mut store), "/prefs/appearance", &[1; 32])
            .unwrap();
        let handle = files
            .open(&mut Disk(&mut store), "/prefs/appearance", false, false)
            .unwrap();
        // The trusted API routes this exact bounded record through this same
        // coherent replace operation, rather than mutating Store directly.
        files
            .replace(&mut Disk(&mut store), "/prefs/appearance", &[2; 32])
            .unwrap();
        let mut data = [0; FILE_BYTES];
        assert_eq!(files.read(handle, &mut data).unwrap(), 32);
        assert_eq!(&data[..32], &[2; 32]);
        assert_eq!(store.read_file("/prefs/appearance", &mut data).unwrap(), 32);
        assert_eq!(&data[..32], &[2; 32]);
    }
    #[test]
    fn exclusive_create_and_guarded_replacement_preserve_held_inodes() {
        let mut ram = Ram::new();
        let mut store = Store::format(
            journal::Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let mut files = Files::<16>::new();
        commit(
            &mut files,
            &mut store,
            Intent::Create {
                path: p("/native/new"),
            },
        )
        .unwrap();
        let generation = store.generation();
        assert_eq!(
            commit(
                &mut files,
                &mut store,
                Intent::Create {
                    path: p("/native/new")
                }
            ),
            Err(BackendError::Exists)
        );
        assert_eq!(store.generation(), generation);
        files
            .replace(&mut Disk(&mut store), "/native/a", b"source")
            .unwrap();
        files
            .replace(&mut Disk(&mut store), "/native/b", b"target")
            .unwrap();
        let source_handle = files
            .open(&mut Disk(&mut store), "/native/a", false, false)
            .unwrap();
        let target_handle = files
            .open(&mut Disk(&mut store), "/native/b", false, false)
            .unwrap();
        let source = inspect(&mut store, p("/native/a")).unwrap().unwrap();
        let target = inspect(&mut store, p("/native/b")).unwrap().unwrap();
        assert_eq!(
            commit(
                &mut files,
                &mut store,
                Intent::Rename {
                    source,
                    destination: target.path,
                    replace: None
                }
            ),
            Err(BackendError::Exists)
        );
        commit(
            &mut files,
            &mut store,
            Intent::Rename {
                source,
                destination: target.path,
                replace: Some(target),
            },
        )
        .unwrap();
        let mut out = [0; FILE_BYTES];
        assert_eq!(files.read(target_handle, &mut out).unwrap(), 6);
        assert_eq!(&out[..6], b"target");
        files
            .write(&mut Disk(&mut store), source_handle, b"SOURCE")
            .unwrap();
        assert_eq!(store.read_file("/native/b", &mut out).unwrap(), 6);
        assert_eq!(&out[..6], b"SOURCE");
        let source = inspect(&mut store, p("/native/b")).unwrap().unwrap();
        commit(&mut files, &mut store, Intent::Delete { source }).unwrap();
        files
            .replace(&mut Disk(&mut store), "/native/b", b"new inode")
            .unwrap();
        files.seek(source_handle, 0, 0).unwrap();
        files
            .write(&mut Disk(&mut store), source_handle, b"old")
            .unwrap();
        assert_eq!(store.read_file("/native/b", &mut out).unwrap(), 9);
        assert_eq!(&out[..9], b"new inode");
    }
    #[test]
    fn concurrent_namespace_revision_and_policy_failures_change_nothing() {
        let mut ram = Ram::new();
        let mut store = Store::format(
            journal::Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let mut files = Files::<16>::new();
        files
            .replace(&mut Disk(&mut store), "/native/source", b"original")
            .unwrap();
        let h = files
            .open(&mut Disk(&mut store), "/native/source", false, false)
            .unwrap();
        let source = inspect(&mut store, p("/native/source")).unwrap().unwrap();
        store.put("/native/other", b"race").unwrap();
        let generation = store.generation();
        for intent in [
            Intent::Delete { source },
            Intent::Rename {
                source,
                destination: p("/native/new"),
                replace: None,
            },
        ] {
            assert_eq!(
                commit(&mut files, &mut store, intent),
                Err(BackendError::Conflict)
            );
            assert_eq!(store.generation(), generation);
        }
        let mut out = [0; FILE_BYTES];
        assert_eq!(files.read(h, &mut out).unwrap(), 8);
        assert_eq!(&out[..8], b"original");
        assert_eq!(store.read_file("/native/source", &mut out).unwrap(), 8);
        assert_eq!(
            commit(
                &mut files,
                &mut store,
                Intent::Create {
                    path: p("/prefs/appearance")
                }
            ),
            Err(BackendError::ReadOnly)
        );
        assert_eq!(
            inspect(&mut store, p("/prefs/appearance")),
            Err(BackendError::ReadOnly)
        );
        let source = inspect(&mut store, p("/native/source")).unwrap().unwrap();
        store.restrict_read_only();
        assert_eq!(
            commit(&mut files, &mut store, Intent::Delete { source }),
            Err(BackendError::ReadOnly)
        );
        assert_eq!(store.generation(), generation);
    }
    #[test]
    fn full_store_and_failed_durable_io_never_publish_namespace_changes() {
        let mut ram = Ram::new();
        let mut store = Store::format(
            journal::Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let mut files = Files::<16>::new();
        for index in 0..journal::MAX_FILES {
            store
                .put(&std::format!("/native/f{index}"), b"kept")
                .unwrap();
        }
        let generation = store.generation();
        assert_eq!(
            commit(
                &mut files,
                &mut store,
                Intent::Create {
                    path: p("/native/overflow")
                }
            ),
            Err(BackendError::Full)
        );
        assert_eq!(store.generation(), generation);
        let source = inspect(&mut store, p("/native/f0")).unwrap().unwrap();
        let h = files
            .open(&mut Disk(&mut store), "/native/f0", false, false)
            .unwrap();
        {
            let _partition = store.into_partition();
        }
        ram.fail = true;
        let mut store = Store::mount(
            journal::Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            commit(&mut files, &mut store, Intent::Delete { source }),
            Err(BackendError::Io)
        );
        assert_eq!(store.generation(), generation);
        assert!(store.is_read_only());
        let mut out = [0; FILE_BYTES];
        assert_eq!(files.read(h, &mut out).unwrap(), 4);
        assert_eq!(&out[..4], b"kept");
        assert_eq!(store.read_file("/native/f0", &mut out).unwrap(), 4);
    }
    #[test]
    fn copy_intent_routes_through_conditional_copy_without_empty_create_semantics() {
        let mut ram = Ram::new();
        let mut store = Store::format(
            journal::Partition::new(&mut ram, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let mut files = Files::<16>::new();
        files
            .replace(
                &mut Disk(&mut store),
                "/native/source",
                b"actual copied bytes",
            )
            .unwrap();
        let source = inspect(&mut store, p("/native/source")).unwrap().unwrap();
        commit(
            &mut files,
            &mut store,
            Intent::Copy {
                source,
                destination: p("/native/new"),
                replace: None,
            },
        )
        .unwrap();
        let mut bytes = [0; FILE_BYTES];
        assert_eq!(store.read_file("/native/new", &mut bytes).unwrap(), 19);
        assert_eq!(&bytes[..19], b"actual copied bytes");
        assert_eq!(store.read_file("/native/source", &mut bytes).unwrap(), 19);
        let source = inspect(&mut store, p("/native/source")).unwrap().unwrap();
        assert_eq!(
            commit(
                &mut files,
                &mut store,
                Intent::Copy {
                    source,
                    destination: p("/native/new"),
                    replace: None
                }
            ),
            Err(BackendError::Exists)
        );
    }
}
