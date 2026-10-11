//! Trusted bounded development Trash adapter. No metadata grants authority.
//! All functions require the existing filesystem guard; only the private kernel
//! session may invoke mutations. One core Files transaction publishes cache
//! changes only after one actual journal transaction succeeds.
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::{
    journal::{self, FileInfo, Store, WritableBlockDevice},
    trash_record::{Key, Record, MAX_ITEMS, RECORD_BYTES},
};
use prism::{
    files_actions::{BackendError, Path},
    trash_ui::{Intent, Item, Listing},
};
const OWNER: u64 = 1;
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
fn read_record<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    id: u64,
) -> Result<Record, BackendError> {
    let key = Key::metadata(id).map_err(|_| BackendError::Io)?;
    let mut bytes = [0; RECORD_BYTES];
    let len = store
        .read_file(key.path(), &mut bytes)
        .map_err(journal_error)?;
    let record = Record::decode(&bytes[..len], OWNER).map_err(|_| BackendError::Io)?;
    if record.id != id {
        return Err(BackendError::Io);
    }
    Ok(record)
}
pub(crate) fn list<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
) -> Result<Listing, BackendError> {
    let mut entries = [FileInfo::EMPTY; journal::MAX_FILES];
    let count = store.list(&mut entries).map_err(journal_error)?;
    let mut data = [0u64; MAX_ITEMS];
    let mut metadata = [0u64; MAX_ITEMS];
    let mut nd = 0;
    let mut nm = 0;
    for entry in entries[..count].iter() {
        if !entry.path().starts_with("/trash/") {
            continue;
        }
        let (id, is_metadata) = Key::parse(entry.path()).map_err(|_| BackendError::Io)?;
        let (ids, n) = if is_metadata {
            (&mut metadata, &mut nm)
        } else {
            (&mut data, &mut nd)
        };
        if *n == MAX_ITEMS {
            return Err(BackendError::Io);
        }
        ids[*n] = id;
        *n += 1;
    }
    if nd != nm || data[..nd].iter().any(|id| !metadata[..nm].contains(id)) {
        return Err(BackendError::Io);
    }
    let revision = store.generation();
    let mut items = [None; MAX_ITEMS];
    let mut bytes = [0; FILE_BYTES];
    for (slot, id) in metadata[..nm].iter().copied().enumerate() {
        let record = read_record(store, id)?;
        let key = Key::data(id).map_err(|_| BackendError::Io)?;
        let len = store
            .read_file(key.path(), &mut bytes)
            .map_err(journal_error)?;
        if len != record.size as usize || journal::crc32(&bytes[..len]) != record.checksum {
            return Err(BackendError::Io);
        }
        items[slot] = Some(Item {
            id,
            revision,
            original: Path::new(record.original()).map_err(|_| BackendError::Io)?,
            size: record.size,
        });
    }
    if items[0].zip(items[1]).is_some_and(|(a, b)| a.id > b.id) {
        items.swap(0, 1);
    }
    Ok(Listing { items, revision })
}
fn validate_item<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    item: Item,
) -> Result<(), BackendError> {
    if item.revision != store.generation() {
        return Err(BackendError::Conflict);
    }
    let listing = list(store)?;
    if !listing.items.iter().flatten().any(|found| {
        found.id == item.id
            && found.revision == item.revision
            && found.original == item.original
            && found.size == item.size
    }) {
        return Err(BackendError::Conflict);
    }
    Ok(())
}
struct Adapter<'s, 'd, D: WritableBlockDevice> {
    store: &'s mut Store<'d, D>,
    error: Option<BackendError>,
}
impl<D: WritableBlockDevice> Backend for Adapter<'_, '_, D> {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
        self.store.read_file(path, out).map_err(|_| file::Error::Io)
    }
    fn put(&mut self, _: &str, _: &[u8]) -> Result<(), file::Error> {
        Err(file::Error::ReadOnly)
    }
    fn transaction(
        &mut self,
        generation: u64,
        operations: &[file::Mutation<'_>],
    ) -> Result<(), file::Error> {
        use journal::transaction::{Mutation as J, TransactionError};
        if operations.len() != 2 {
            return Err(file::Error::Io);
        }
        fn convert(operation: file::Mutation<'_>) -> journal::transaction::Mutation<'_> {
            match operation {
                file::Mutation::Create { path, bytes } => J::Create { path, bytes },
                file::Mutation::Replace { path, bytes } => J::Replace { path, bytes },
                file::Mutation::Rename { old, new, replace } => J::Rename { old, new, replace },
                file::Mutation::Remove { path } => J::Remove { path },
            }
        }
        self.store
            .transaction(
                generation,
                &[convert(operations[0]), convert(operations[1])],
            )
            .map(|_| ())
            .map_err(|error| {
                self.error = Some(match error {
                    TransactionError::Count => BackendError::InvalidPath,
                    TransactionError::Conflict => BackendError::Conflict,
                    TransactionError::Journal(error) => journal_error(error),
                });
                file::Error::Io
            })
    }
}
fn apply<D: WritableBlockDevice, const N: usize>(
    files: &mut Files<N>,
    store: &mut Store<'_, D>,
    generation: u64,
    operations: &[file::Mutation<'_>],
) -> Result<(), BackendError> {
    let mut adapter = Adapter { store, error: None };
    files
        .transaction(&mut adapter, generation, operations)
        .map_err(|error| {
            adapter.error.unwrap_or(match error {
                file::Error::Full => BackendError::Full,
                file::Error::Exists => BackendError::Exists,
                file::Error::NotFound => BackendError::NotFound,
                file::Error::Busy => BackendError::Busy,
                file::Error::ReadOnly => BackendError::ReadOnly,
                file::Error::InvalidPath | file::Error::TooLarge => BackendError::InvalidPath,
                _ => BackendError::Io,
            })
        })
}
pub(crate) fn commit<D: WritableBlockDevice, const N: usize>(
    files: &mut Files<N>,
    store: &mut Store<'_, D>,
    intent: Intent,
) -> Result<(), BackendError> {
    match intent {
        Intent::Move { source } => {
            if !source.path.as_str().starts_with("/native/") {
                return Err(BackendError::ReadOnly);
            }
            if source.revision != store.generation() {
                return Err(BackendError::Conflict);
            }
            if list(store)?.items.iter().flatten().count() == MAX_ITEMS {
                return Err(BackendError::Full);
            }
            let id = source.revision.checked_add(1).ok_or(BackendError::Io)?;
            let data = Key::data(id).map_err(|_| BackendError::Io)?;
            let metadata = Key::metadata(id).map_err(|_| BackendError::Io)?;
            let mut bytes = [0; FILE_BYTES];
            let len = store
                .read_file(source.path.as_str(), &mut bytes)
                .map_err(journal_error)?;
            let record = Record::new(
                OWNER,
                id,
                source.path.as_str(),
                len,
                journal::crc32(&bytes[..len]),
            )
            .map_err(|_| BackendError::Io)?
            .encode();
            apply(
                files,
                store,
                source.revision,
                &[
                    file::Mutation::Rename {
                        old: source.path.as_str(),
                        new: data.path(),
                        replace: false,
                    },
                    file::Mutation::Create {
                        path: metadata.path(),
                        bytes: &record,
                    },
                ],
            )
        }
        Intent::Restore { item, destination } => {
            if !destination.as_str().starts_with("/native/") {
                return Err(BackendError::ReadOnly);
            }
            validate_item(store, item)?;
            let data = Key::data(item.id).map_err(|_| BackendError::Io)?;
            let metadata = Key::metadata(item.id).map_err(|_| BackendError::Io)?;
            apply(
                files,
                store,
                item.revision,
                &[
                    file::Mutation::Rename {
                        old: data.path(),
                        new: destination.as_str(),
                        replace: false,
                    },
                    file::Mutation::Remove {
                        path: metadata.path(),
                    },
                ],
            )
        }
        Intent::Purge { item } => {
            validate_item(store, item)?;
            let data = Key::data(item.id).map_err(|_| BackendError::Io)?;
            let metadata = Key::metadata(item.id).map_err(|_| BackendError::Io)?;
            apply(
                files,
                store,
                item.revision,
                &[
                    file::Mutation::Remove { path: data.path() },
                    file::Mutation::Remove {
                        path: metadata.path(),
                    },
                ],
            )
        }
    }
}
/// Only held previously authorized inode writes reach this private path. Update
/// both data and fingerprint in one transaction before Files publishes new bytes.
pub(crate) fn write_held<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    path: &str,
    bytes: &[u8],
) -> Result<(), BackendError> {
    let (id, is_metadata) = Key::parse(path).map_err(|_| BackendError::ReadOnly)?;
    if is_metadata {
        return Err(BackendError::ReadOnly);
    }
    let listing = list(store)?;
    if !listing.items.iter().flatten().any(|item| item.id == id) {
        return Err(BackendError::Conflict);
    }
    let record = read_record(store, id)?
        .updated(bytes)
        .map_err(|_| BackendError::InvalidPath)?
        .encode();
    let metadata = Key::metadata(id).map_err(|_| BackendError::Io)?;
    store
        .transaction(
            listing.revision,
            &[
                journal::transaction::Mutation::Replace { path, bytes },
                journal::transaction::Mutation::Replace {
                    path: metadata.path(),
                    bytes: &record,
                },
            ],
        )
        .map(|_| ())
        .map_err(|error| match error {
            journal::transaction::TransactionError::Conflict => BackendError::Conflict,
            journal::transaction::TransactionError::Count => BackendError::InvalidPath,
            journal::transaction::TransactionError::Journal(error) => journal_error(error),
        })
}
