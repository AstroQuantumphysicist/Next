//! Host/test-gated bridge between the bounded journal and backup archives.
//!
//! Export reads the current snapshot into fixed-size buffers before encoding.
//! Restore parses and SHA-256-preflights the complete archive into fixed-size
//! staging buffers, then replaces the namespace in one journal COW commit. The
//! archive remains unsigned and unauthenticated; this bridge does not provide
//! encryption, external-media handling, destination selection, or a UI. The
//! journal's COW protocol preserves old-or-new visibility across failed commit
//! operations when the device honors its flush contract; a failed device
//! operation still requires remount before further writes.

use super::backup::{self, FilePayload, ManifestError, RestoreError, RestoreTarget};
use super::backup_archive::{self, Archive, ArchiveError};
use super::journal::{self, FileInfo, Store, WritableBlockDevice};

#[derive(Debug, PartialEq, Eq)]
pub enum JournalBackupError<E> {
    Store(journal::Error<E>),
    Archive(ArchiveError),
    Manifest(ManifestError),
    Staging,
}

/// Export the current journal namespace into an archive in `output`.
///
/// The output is not touched unless the complete file set was read and the
/// archive fits. The caller controls buffer ownership and persistence.
pub fn export_archive<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    output: &mut [u8],
) -> Result<usize, JournalBackupError<D::Error>> {
    let mut info = [FileInfo::EMPTY; journal::MAX_FILES];
    let count = store.list(&mut info).map_err(JournalBackupError::Store)?;
    let mut contents = [[0u8; journal::MAX_FILE_BYTES]; journal::MAX_FILES];
    let mut lengths = [0usize; journal::MAX_FILES];
    for index in 0..count {
        lengths[index] = store
            .read_file(info[index].path(), &mut contents[index])
            .map_err(JournalBackupError::Store)?;
    }

    let mut files = [FilePayload {
        path: "",
        bytes: &[],
    }; backup::MAX_ENTRIES];
    for index in 0..count {
        files[index] = FilePayload {
            path: info[index].path(),
            bytes: &contents[index][..lengths[index]],
        };
    }
    backup_archive::encode(&files[..count], output).map_err(JournalBackupError::Archive)
}

/// Restore an archive as one complete namespace transaction.
///
/// Archive parsing and the entire manifest/payload SHA-256 check finish before
/// `Store::replace_all` can issue a write. Invalid, oversized, duplicate, or
/// reserved input paths are rejected before device mutation. If the underlying
/// commit returns an I/O error, remount the store to resolve old-or-new state.
pub fn restore_archive<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    bytes: &[u8],
) -> Result<(), JournalBackupError<D::Error>> {
    let archive = Archive::parse(bytes).map_err(JournalBackupError::Archive)?;
    let mut staged = Staging::new();
    match archive.restore(&mut staged) {
        Ok(()) => (),
        Err(RestoreError::Manifest(error)) => return Err(JournalBackupError::Manifest(error)),
        Err(RestoreError::Target(())) => return Err(JournalBackupError::Staging),
    }

    let mut files = [("", &[][..]); backup::MAX_ENTRIES];
    for (index, file) in files[..staged.count].iter_mut().enumerate() {
        let path = core::str::from_utf8(&staged.paths[index][..staged.path_lengths[index]])
            .map_err(|_| JournalBackupError::Staging)?;
        *file = (
            path,
            &staged.contents[index][..staged.content_lengths[index]],
        );
    }
    store
        .replace_all(&files[..staged.count])
        .map_err(JournalBackupError::Store)
}

struct Staging {
    paths: [[u8; backup::MAX_PATH_BYTES]; backup::MAX_ENTRIES],
    path_lengths: [usize; backup::MAX_ENTRIES],
    contents: [[u8; backup::MAX_ENTRY_BYTES]; backup::MAX_ENTRIES],
    content_lengths: [usize; backup::MAX_ENTRIES],
    count: usize,
}

impl Staging {
    const fn new() -> Self {
        Self {
            paths: [[0; backup::MAX_PATH_BYTES]; backup::MAX_ENTRIES],
            path_lengths: [0; backup::MAX_ENTRIES],
            contents: [[0; backup::MAX_ENTRY_BYTES]; backup::MAX_ENTRIES],
            content_lengths: [0; backup::MAX_ENTRIES],
            count: 0,
        }
    }
}

impl RestoreTarget for Staging {
    type Error = ();

    fn restore_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Self::Error> {
        if self.count == backup::MAX_ENTRIES
            || path.len() > backup::MAX_PATH_BYTES
            || bytes.len() > backup::MAX_ENTRY_BYTES
        {
            return Err(());
        }
        self.paths[self.count][..path.len()].copy_from_slice(path.as_bytes());
        self.path_lengths[self.count] = path.len();
        self.contents[self.count][..bytes.len()].copy_from_slice(bytes);
        self.content_lengths[self.count] = bytes.len();
        self.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{journal::Partition, BlockDevice};
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::vec::Vec;

    const DISK_SECTORS: usize = 132;
    const COMMIT_OPERATIONS: usize = journal::MAX_FILES * 8 + 5;

    #[derive(Clone, Copy)]
    enum Fault {
        Before(usize),
        After(usize),
    }

    struct Disk {
        durable: Vec<[u8; 512]>,
        pending: BTreeMap<u64, [u8; 512]>,
        operations: Rc<Cell<usize>>,
        fault: Option<Fault>,
    }

    impl Clone for Disk {
        fn clone(&self) -> Self {
            Self {
                durable: self.durable.clone(),
                pending: self.pending.clone(),
                operations: Rc::new(Cell::new(self.operations.get())),
                fault: self.fault,
            }
        }
    }

    impl Disk {
        fn new() -> Self {
            Self {
                durable: std::vec![[0xa5; 512]; DISK_SECTORS],
                pending: BTreeMap::new(),
                operations: Rc::new(Cell::new(0)),
                fault: None,
            }
        }

        fn crash(&mut self) {
            self.pending.clear();
            self.operations.set(0);
            self.fault = None;
        }

        fn begin(&mut self) -> Result<usize, ()> {
            let operation = self.operations.get() + 1;
            self.operations.set(operation);
            if matches!(self.fault, Some(Fault::Before(at)) if at == operation) {
                return Err(());
            }
            Ok(operation)
        }

        fn end(&self, operation: usize) -> Result<(), ()> {
            if matches!(self.fault, Some(Fault::After(at)) if at == operation) {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    impl BlockDevice for Disk {
        type Error = ();

        fn sector_count(&self) -> u64 {
            self.durable.len() as u64
        }

        fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
            *out = self
                .pending
                .get(&lba)
                .copied()
                .or_else(|| self.durable.get(lba as usize).copied())
                .ok_or(())?;
            Ok(())
        }
    }

    impl WritableBlockDevice for Disk {
        fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), Self::Error> {
            let operation = self.begin()?;
            self.pending.insert(lba, *data);
            self.end(operation)
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            let operation = self.begin()?;
            for (lba, data) in core::mem::take(&mut self.pending) {
                self.durable[lba as usize] = data;
            }
            self.end(operation)
        }
    }

    fn partition(disk: &mut Disk) -> Partition<'_, Disk> {
        Partition::new(disk, 0, DISK_SECTORS as u64, &[]).unwrap()
    }

    fn baseline() -> Disk {
        let mut disk = Disk::new();
        let mut store = Store::format(partition(&mut disk)).unwrap();
        store.put("/old", b"old bytes").unwrap();
        store.put("/keep", b"kept bytes").unwrap();
        let _ = store.into_partition();
        disk.crash();
        disk
    }

    fn archive(files: &[FilePayload<'_>]) -> Vec<u8> {
        let mut bytes = std::vec![0; backup_archive::MAX_ARCHIVE_BYTES];
        let length = backup_archive::encode(files, &mut bytes).unwrap();
        bytes.truncate(length);
        bytes
    }

    fn read(store: &mut Store<'_, Disk>, path: &str) -> Result<Vec<u8>, journal::Error<()>> {
        let mut bytes = [0; journal::MAX_FILE_BYTES];
        let length = store.read_file(path, &mut bytes)?;
        Ok(bytes[..length].to_vec())
    }

    fn assert_old(store: &mut Store<'_, Disk>) {
        assert_eq!(read(store, "/old").unwrap(), b"old bytes");
        assert_eq!(read(store, "/keep").unwrap(), b"kept bytes");
        assert_eq!(read(store, "/new-a"), Err(journal::Error::NotFound));
        assert_eq!(read(store, "/new-b"), Err(journal::Error::NotFound));
    }

    fn assert_new(store: &mut Store<'_, Disk>) {
        assert_eq!(read(store, "/old"), Err(journal::Error::NotFound));
        assert_eq!(read(store, "/keep"), Err(journal::Error::NotFound));
        assert_eq!(read(store, "/new-a").unwrap(), b"new payload a");
        assert_eq!(read(store, "/new-b").unwrap(), b"new payload b");
    }

    fn replacement_archive() -> Vec<u8> {
        archive(&[
            FilePayload {
                path: "/new-a",
                bytes: b"new payload a",
            },
            FilePayload {
                path: "/new-b",
                bytes: b"new payload b",
            },
        ])
    }

    #[test]
    fn export_and_restore_replace_the_complete_namespace_across_remount() {
        let mut disk = baseline();
        let operations = disk.operations.clone();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        let mut archive_bytes = std::vec![0; backup_archive::MAX_ARCHIVE_BYTES];
        let length = export_archive(&mut store, &mut archive_bytes).unwrap();
        archive_bytes.truncate(length);
        store.put("/temporary", b"remove me on restore").unwrap();
        let generation_before_restore = store.generation();
        operations.set(0);

        restore_archive(&mut store, &archive_bytes).unwrap();
        assert_eq!(store.generation(), generation_before_restore + 1);
        assert_eq!(operations.get(), COMMIT_OPERATIONS);
        assert_old(&mut store);
        assert_eq!(
            read(&mut store, "/temporary"),
            Err(journal::Error::NotFound)
        );
        let _ = store.into_partition();
        disk.crash();

        let mut remounted = Store::mount(partition(&mut disk)).unwrap();
        assert_eq!(remounted.generation(), generation_before_restore + 1);
        assert_old(&mut remounted);
        assert_eq!(
            read(&mut remounted, "/temporary"),
            Err(journal::Error::NotFound)
        );
        assert_eq!(
            remounted.list(&mut [FileInfo::EMPTY; journal::MAX_FILES]),
            Ok(2)
        );
    }

    #[test]
    fn malformed_digest_invalid_and_preflight_rejections_issue_zero_writes() {
        let mut disk = baseline();
        let valid = replacement_archive();
        let operations = disk.operations.clone();
        let mut store = Store::mount(partition(&mut disk)).unwrap();

        assert_eq!(
            restore_archive(&mut store, &valid[..valid.len() - 1]),
            Err(JournalBackupError::Archive(ArchiveError::Truncated))
        );
        assert_eq!(operations.get(), 0);

        let mut bad_digest = valid.clone();
        *bad_digest.last_mut().unwrap() ^= 1;
        assert_eq!(
            restore_archive(&mut store, &bad_digest),
            Err(JournalBackupError::Manifest(ManifestError::DigestMismatch))
        );
        assert_eq!(operations.get(), 0);

        let reserved = archive(&[FilePayload {
            path: "/dirs/private",
            bytes: b"reserved namespace",
        }]);
        assert_eq!(
            restore_archive(&mut store, &reserved),
            Err(JournalBackupError::Store(journal::Error::InvalidPath))
        );
        assert_eq!(operations.get(), 0);

        assert_eq!(
            store.replace_all(&[("/same", b"one"), ("/same", b"two")]),
            Err(journal::Error::Exists)
        );
        assert_eq!(
            store.replace_all(&[("/large", &[0u8; journal::MAX_FILE_BYTES + 1])]),
            Err(journal::Error::TooLarge)
        );
        let too_many = [("/a", &[][..]); journal::MAX_FILES + 1];
        assert_eq!(store.replace_all(&too_many), Err(journal::Error::Full));
        assert_eq!(operations.get(), 0);

        store.restrict_read_only();
        assert_eq!(
            store.replace_all(&[("/valid", b"payload")]),
            Err(journal::Error::ReadOnly)
        );
        assert_eq!(operations.get(), 0);
    }

    #[test]
    fn every_commit_fault_recovers_the_entire_old_or_new_archive_snapshot() {
        let baseline = baseline();
        let archive = replacement_archive();

        for operation in 1..=COMMIT_OPERATIONS {
            for fault in [Fault::Before(operation), Fault::After(operation)] {
                let mut disk = baseline.clone();
                disk.fault = Some(fault);
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                assert!(matches!(
                    restore_archive(&mut store, &archive),
                    Err(JournalBackupError::Store(journal::Error::Device(())))
                ));
                let _ = store.into_partition();
                disk.crash();

                let mut recovered = Store::mount(partition(&mut disk)).unwrap();
                let is_old = read(&mut recovered, "/old").is_ok();
                let is_new = read(&mut recovered, "/new-a").is_ok();
                assert_ne!(is_old, is_new, "operation {operation}");
                if is_old {
                    assert_old(&mut recovered);
                } else {
                    assert_new(&mut recovered);
                }
            }
        }
    }
}
