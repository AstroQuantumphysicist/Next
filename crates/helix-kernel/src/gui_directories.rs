//! Trusted Prism Files explicit-directory transactions. The caller holds the
//! Store guard from snapshot validation through commit and refreshed listing.
use next_fs::journal::{directories, Store, WritableBlockDevice};
use prism::files_actions::Path;
use prism::files_directory::{
    self, Action, BackendError, Entry, FolderSnapshot, Intent, Kind, Listing,
};

fn backend_error<E>(error: directories::Error<E>) -> BackendError {
    match error {
        directories::Error::Path => BackendError::InvalidPath,
        directories::Error::Owner => BackendError::Unavailable,
        directories::Error::MissingParent => BackendError::MissingParent,
        directories::Error::NotDirectory => BackendError::NotDirectory,
        directories::Error::NotFound => BackendError::NotFound,
        directories::Error::Exists => BackendError::Exists,
        directories::Error::NotEmpty => BackendError::NotEmpty,
        directories::Error::Full => BackendError::Full,
        directories::Error::Conflict => BackendError::Conflict,
        directories::Error::Corrupt | directories::Error::Store(_) => BackendError::Io,
    }
}
fn path(text: &str) -> Result<Path, BackendError> {
    Path::new(text).map_err(|_| BackendError::InvalidPath)
}
fn map_listing(source: directories::Listing) -> Result<Listing, BackendError> {
    if source.count > files_directory::CAPACITY {
        return Err(BackendError::Io);
    }
    let folder = FolderSnapshot {
        path: path(source.folder.path())?,
        id: source.folder.id,
        revision: source.generation,
    };
    if source.folder.kind != directories::Kind::Directory {
        return Err(BackendError::NotDirectory);
    }
    let mut entries = [None; files_directory::CAPACITY];
    for (slot, entry) in entries
        .iter_mut()
        .zip(source.entries[..source.count].iter())
    {
        *slot = Some(Entry {
            path: path(entry.path())?,
            kind: match entry.kind {
                directories::Kind::File => Kind::File,
                directories::Kind::Directory => Kind::Directory,
            },
            size: entry.size,
            id: entry.id,
        });
    }
    Ok(Listing { folder, entries })
}
fn snapshot<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    expected: FolderSnapshot,
) -> Result<directories::FolderSnapshot, BackendError> {
    let actual =
        directories::snapshot(store, owner, expected.path.as_str()).map_err(backend_error)?;
    if actual.generation != expected.revision
        || actual.folder.id != expected.id
        || actual.folder.kind != directories::Kind::Directory
    {
        return Err(BackendError::Conflict);
    }
    Ok(actual)
}
fn parent(path: Path) -> Result<Path, BackendError> {
    let parent = path
        .as_str()
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .ok_or(BackendError::InvalidPath)?;
    Path::new(parent).map_err(|_| BackendError::InvalidPath)
}
fn listing<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    path: Path,
) -> Result<Listing, BackendError> {
    directories::list(store, owner, path.as_str())
        .map_err(backend_error)
        .and_then(map_listing)
}
/// Execute one taken Prism action. Every successful mutation is paired with a
/// same-guard listing for the source parent before UI state can be published.
pub(crate) fn perform<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    action: Action,
) -> Result<Listing, BackendError> {
    match action.intent {
        Intent::Navigate { path } => listing(store, owner, path),
        Intent::CreateFolder { parent, path } => {
            let expected = snapshot(store, owner, parent)?;
            directories::mkdir_at(store, owner, expected, path.as_str()).map_err(backend_error)?;
            listing(store, owner, parent.path)
        }
        Intent::MoveFolder {
            source,
            destination,
        } => {
            let source_snapshot = snapshot(store, owner, source)?;
            // The destination parent is fetched from the explicit namespace at
            // the same Store generation; its immutable ID is transaction-bound.
            let destination_parent =
                directories::snapshot(store, owner, parent(destination)?.as_str())
                    .map_err(backend_error)?;
            if destination_parent.generation != source.revision {
                return Err(BackendError::Conflict);
            }
            directories::rename_between_snapshots(
                store,
                owner,
                source_snapshot,
                destination_parent,
                destination.as_str(),
            )
            .map_err(backend_error)?;
            listing(store, owner, parent(source.path)?)
        }
        Intent::RemoveFolder { source } => {
            let exact = snapshot(store, owner, source)?;
            directories::rmdir_snapshot(store, owner, exact).map_err(backend_error)?;
            listing(store, owner, parent(source.path)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use next_fs::journal::{self, Store as JournalStore};
    use next_fs::BlockDevice;
    struct Ram(std::vec::Vec<[u8; 512]>);
    impl Ram {
        fn new() -> Self {
            Self(std::vec![[0; 512]; journal::REQUIRED_SECTORS as usize])
        }
    }
    impl BlockDevice for Ram {
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
        fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), ()> {
            self.0[lba as usize] = *data;
            Ok(())
        }
        fn flush(&mut self) -> Result<(), ()> {
            Ok(())
        }
    }
    fn store() -> Ram {
        Ram::new()
    }
    fn listing_for<D: WritableBlockDevice>(store: &mut JournalStore<'_, D>, path: &str) -> Listing {
        listing(store, 1, Path::new(path).unwrap()).unwrap()
    }
    #[test]
    fn directory_actions_commit_and_return_authoritative_refreshed_rows() {
        let mut disk = store();
        let mut store = JournalStore::format(
            journal::Partition::new(&mut disk, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let root = listing_for(&mut store, "/native");
        let created = perform(
            &mut store,
            1,
            Action {
                request: 1,
                intent: Intent::CreateFolder {
                    parent: root.folder,
                    path: Path::new("/native/docs").unwrap(),
                },
            },
        )
        .unwrap();
        assert_eq!(created.folder.revision, root.folder.revision + 1);
        let docs = created
            .entries
            .iter()
            .flatten()
            .find(|e| e.path.as_str() == "/native/docs")
            .unwrap();
        assert_eq!(docs.kind, Kind::Directory);
        let nested = perform(
            &mut store,
            1,
            Action {
                request: 2,
                intent: Intent::Navigate { path: docs.path },
            },
        )
        .unwrap();
        assert_eq!(nested.folder.id, docs.id);
        assert_eq!(nested.folder.path.as_str(), "/native/docs");
        let moved = perform(
            &mut store,
            1,
            Action {
                request: 3,
                intent: Intent::MoveFolder {
                    source: FolderSnapshot {
                        path: docs.path,
                        id: docs.id,
                        revision: created.folder.revision,
                    },
                    destination: Path::new("/native/moved").unwrap(),
                },
            },
        )
        .unwrap();
        assert!(!moved
            .entries
            .iter()
            .flatten()
            .any(|e| e.path.as_str() == "/native/docs"));
        let moved_folder = listing_for(&mut store, "/native")
            .entries
            .into_iter()
            .flatten()
            .find(|e| e.path.as_str() == "/native/moved")
            .unwrap();
        assert_eq!(moved_folder.id, docs.id);
        let removed = perform(
            &mut store,
            1,
            Action {
                request: 4,
                intent: Intent::RemoveFolder {
                    source: FolderSnapshot {
                        path: moved_folder.path,
                        id: moved_folder.id,
                        revision: moved.folder.revision,
                    },
                },
            },
        )
        .unwrap();
        assert!(!removed
            .entries
            .iter()
            .flatten()
            .any(|e| e.path.as_str() == "/native/moved"));
    }
    #[test]
    fn stale_or_forged_folder_identity_is_rejected() {
        let mut disk = store();
        let mut store = JournalStore::format(
            journal::Partition::new(&mut disk, 0, journal::REQUIRED_SECTORS, &[]).unwrap(),
        )
        .unwrap();
        let root = listing_for(&mut store, "/native");
        assert_eq!(
            perform(
                &mut store,
                1,
                Action {
                    request: 1,
                    intent: Intent::CreateFolder {
                        parent: FolderSnapshot {
                            id: 99,
                            ..root.folder
                        },
                        path: Path::new("/native/docs").unwrap(),
                    }
                }
            ),
            Err(BackendError::Conflict)
        );
    }
}
