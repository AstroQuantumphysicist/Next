//! Session-only, single-operation undo for coherent bounded Trash transactions.
//! Trusted callers hold the same Files/Store guard for commit and undo. Receipts
//! grant no authority, survive no reboot, and never recover overwritten bytes.
use super::trash;
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::{
    journal::{self, Store, WritableBlockDevice},
    trash_record::{Key, Record, RECORD_BYTES},
};
use prism::{
    files_actions::{BackendError, Path},
    trash_ui::{Intent, Item},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Move,
    Restore,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Available {
    pub generation: u64,
    pub kind: Kind,
    pub path: Path,
}
#[derive(Clone, Copy)]
enum Inverse {
    Move(Item),
    Restore {
        destination: Path,
        record: [u8; RECORD_BYTES],
    },
}
#[derive(Clone, Copy)]
struct Receipt {
    generation: u64,
    size: usize,
    checksum: u32,
    inverse: Inverse,
}
pub(crate) struct History {
    receipt: Option<Receipt>,
}
impl History {
    pub(crate) const fn new() -> Self {
        Self { receipt: None }
    }
    pub(crate) fn clear(&mut self) {
        self.receipt = None;
    }
    pub(crate) fn available(&self) -> Option<Available> {
        self.receipt.map(|r| Available {
            generation: r.generation,
            kind: match r.inverse {
                Inverse::Move(_) => Kind::Move,
                Inverse::Restore { .. } => Kind::Restore,
            },
            path: match r.inverse {
                Inverse::Move(item) => item.original,
                Inverse::Restore { destination, .. } => destination,
            },
        })
    }
    pub(crate) fn commit<D: WritableBlockDevice, const N: usize>(
        &mut self,
        files: &mut Files<N>,
        store: &mut Store<'_, D>,
        intent: Intent,
    ) -> Result<(), BackendError> {
        let generation = store.generation().checked_add(1).ok_or(BackendError::Io)?;
        // All fallible receipt construction precedes the durable commit. Never
        // report a failed mutation because a post-commit receipt read failed.
        let receipt = match intent {
            Intent::Move { source } => {
                if source.revision != store.generation() {
                    return Err(BackendError::Conflict);
                }
                let (size, checksum) = fingerprint(store, source.path.as_str())?;
                Some(Receipt {
                    generation,
                    size,
                    checksum,
                    inverse: Inverse::Move(Item {
                        id: generation,
                        revision: generation,
                        original: source.path,
                        size: size as u32,
                    }),
                })
            }
            Intent::Restore { item, destination } => {
                if item.revision != store.generation() {
                    return Err(BackendError::Conflict);
                }
                let listing = trash::list(store)?;
                if !listing.items.iter().flatten().any(|found| *found == item) {
                    return Err(BackendError::Conflict);
                }
                let key = Key::metadata(item.id).map_err(|_| BackendError::Io)?;
                let mut record = [0; RECORD_BYTES];
                let len = store.read_file(key.path(), &mut record).map_err(error)?;
                let decoded = Record::decode(&record[..len], 1).map_err(|_| BackendError::Io)?;
                Some(Receipt {
                    generation,
                    size: decoded.size as usize,
                    checksum: decoded.checksum,
                    inverse: Inverse::Restore {
                        destination,
                        record,
                    },
                })
            }
            Intent::Purge { .. } => None,
        };
        trash::commit(files, store, intent)?;
        self.receipt = receipt;
        Ok(())
    }
    pub(crate) fn undo<D: WritableBlockDevice, const N: usize>(
        &mut self,
        files: &mut Files<N>,
        store: &mut Store<'_, D>,
        expected_generation: u64,
    ) -> Result<(), BackendError> {
        let receipt = self.receipt.ok_or(BackendError::NotFound)?;
        if store.is_read_only() {
            return Err(BackendError::ReadOnly);
        }
        if expected_generation != receipt.generation || store.generation() != receipt.generation {
            return Err(BackendError::Conflict);
        }
        match receipt.inverse {
            Inverse::Move(item) => {
                let key = Key::data(item.id).map_err(|_| BackendError::Io)?;
                if fingerprint(store, key.path())? != (receipt.size, receipt.checksum) {
                    return Err(BackendError::Conflict);
                }
                trash::commit(
                    files,
                    store,
                    Intent::Restore {
                        item,
                        destination: item.original,
                    },
                )?;
            }
            Inverse::Restore {
                destination,
                record,
            } => {
                if fingerprint(store, destination.as_str())? != (receipt.size, receipt.checksum) {
                    return Err(BackendError::Conflict);
                }
                // Preserve exact original id/path metadata, even when Restore
                // selected a different destination. A fresh Move would lose it.
                let decoded = Record::decode(&record, 1).map_err(|_| BackendError::Io)?;
                let data = Key::data(decoded.id).map_err(|_| BackendError::Io)?;
                let metadata = Key::metadata(decoded.id).map_err(|_| BackendError::Io)?;
                let mut adapter = Adapter { store, error: None };
                files
                    .transaction(
                        &mut adapter,
                        receipt.generation,
                        &[
                            file::Mutation::Rename {
                                old: destination.as_str(),
                                new: data.path(),
                                replace: false,
                            },
                            file::Mutation::Create {
                                path: metadata.path(),
                                bytes: &record,
                            },
                        ],
                    )
                    .map_err(|e| adapter.error.unwrap_or(core_error(e)))?;
            }
        }
        self.receipt = None;
        Ok(())
    }
}
fn fingerprint<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    path: &str,
) -> Result<(usize, u32), BackendError> {
    let mut bytes = [0; FILE_BYTES];
    let n = store.read_file(path, &mut bytes).map_err(error)?;
    Ok((n, journal::crc32(&bytes[..n])))
}
fn error<E>(e: journal::Error<E>) -> BackendError {
    match e {
        journal::Error::ReadOnly => BackendError::ReadOnly,
        journal::Error::Full => BackendError::Full,
        journal::Error::Exists => BackendError::Exists,
        journal::Error::NotFound => BackendError::NotFound,
        journal::Error::InvalidPath | journal::Error::TooLarge => BackendError::InvalidPath,
        _ => BackendError::Io,
    }
}
fn core_error(e: file::Error) -> BackendError {
    match e {
        file::Error::ReadOnly => BackendError::ReadOnly,
        file::Error::Full => BackendError::Full,
        file::Error::Exists => BackendError::Exists,
        file::Error::NotFound => BackendError::NotFound,
        file::Error::Busy => BackendError::Busy,
        file::Error::InvalidPath | file::Error::TooLarge => BackendError::InvalidPath,
        _ => BackendError::Io,
    }
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
        if operations.len() != 2 {
            return Err(file::Error::InvalidPath);
        }
        let convert = |op| match op {
            file::Mutation::Create { path, bytes } => {
                journal::transaction::Mutation::Create { path, bytes }
            }
            file::Mutation::Replace { path, bytes } => {
                journal::transaction::Mutation::Replace { path, bytes }
            }
            file::Mutation::Rename { old, new, replace } => {
                journal::transaction::Mutation::Rename { old, new, replace }
            }
            file::Mutation::Remove { path } => journal::transaction::Mutation::Remove { path },
        };
        self.store
            .transaction(
                generation,
                &[convert(operations[0]), convert(operations[1])],
            )
            .map(|_| ())
            .map_err(|e| {
                self.error = Some(match e {
                    journal::transaction::TransactionError::Conflict => BackendError::Conflict,
                    journal::transaction::TransactionError::Count => BackendError::InvalidPath,
                    journal::transaction::TransactionError::Journal(e) => error(e),
                });
                file::Error::Io
            })
    }
}
