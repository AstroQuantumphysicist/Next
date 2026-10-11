#![allow(dead_code)]
#[path = "../crates/helix-kernel/src/trash.rs"]
mod trash;
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::{
    journal::{Partition, Store, WritableBlockDevice},
    trash_record::Key,
    BlockDevice,
};
use prism::{
    files_actions::{BackendError, Path, Snapshot},
    trash_ui::Intent,
};
use std::collections::BTreeMap;
#[derive(Clone)]
struct Ram {
    sectors: Vec<[u8; 512]>,
    pending: BTreeMap<u64, [u8; 512]>,
    operations: usize,
    failure: Option<(usize, bool)>,
}
impl Ram {
    fn new() -> Self {
        Self {
            sectors: vec![[0xa5; 512]; 256],
            pending: BTreeMap::new(),
            operations: 0,
            failure: None,
        }
    }
    fn begin(&mut self) -> Result<(), ()> {
        self.operations += 1;
        if self.failure == Some((self.operations, false)) {
            Err(())
        } else {
            Ok(())
        }
    }
    fn end(&self) -> Result<(), ()> {
        if self.failure == Some((self.operations, true)) {
            Err(())
        } else {
            Ok(())
        }
    }
    fn crash(&mut self) {
        self.pending.clear();
        self.failure = None;
        self.operations = 0;
    }
    fn protected(&self) {
        assert!(self.sectors[..64]
            .iter()
            .chain(&self.sectors[196..])
            .all(|b| *b == [0xa5; 512]));
    }
}
impl BlockDevice for Ram {
    type Error = ();
    fn sector_count(&self) -> u64 {
        256
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
        *out = self
            .pending
            .get(&lba)
            .copied()
            .unwrap_or(self.sectors[lba as usize]);
        Ok(())
    }
}
impl WritableBlockDevice for Ram {
    fn write_sector(&mut self, lba: u64, bytes: &[u8; 512]) -> Result<(), ()> {
        assert!((64..196).contains(&lba));
        self.begin()?;
        self.pending.insert(lba, *bytes);
        self.end()
    }
    fn flush(&mut self) -> Result<(), ()> {
        self.begin()?;
        for (lba, bytes) in std::mem::take(&mut self.pending) {
            self.sectors[lba as usize] = bytes;
        }
        self.end()
    }
}
fn part(ram: &mut Ram) -> Partition<'_, Ram> {
    Partition::new(ram, 64, 132, &[(0, 64), (196, 60)]).unwrap()
}
fn setup() -> Ram {
    let mut ram = Ram::new();
    let mut store = Store::format(part(&mut ram)).unwrap();
    store.put("/native/live", b"original").unwrap();
    store.put("/native/keep", b"keep").unwrap();
    let _ = store.into_partition();
    ram.crash();
    ram
}
struct Disk<'s, 'd>(&'s mut Store<'d, Ram>);
impl Backend for Disk<'_, '_> {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
        self.0.read_file(path, out).map_err(|_| file::Error::Io)
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), file::Error> {
        if path.starts_with("/trash/") {
            trash::write_held(self.0, path, bytes).map_err(|_| file::Error::Io)
        } else {
            self.0.put(path, bytes).map_err(|_| file::Error::Io)
        }
    }
}
fn source(store: &Store<'_, Ram>) -> Snapshot {
    Snapshot {
        path: Path::new("/native/live").unwrap(),
        revision: store.generation(),
    }
}

#[path = "../crates/helix-kernel/src/trash_undo.rs"]
mod trash_undo;
use trash_undo::{History, Kind};
#[test]
fn move_undo_and_restore_undo_preserve_inode_and_exact_original_record() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let mut history = History::new();
    let handle = files
        .open(&mut Disk(&mut store), "/native/live", false, false)
        .unwrap();
    let inode = files.inode(handle).unwrap();
    files.seek(handle, 3, 0).unwrap();
    let request = Intent::Move {
        source: source(&store),
    };
    history.commit(&mut files, &mut store, request).unwrap();
    let receipt = history.available().unwrap();
    assert_eq!(receipt.kind, Kind::Move);
    history
        .undo(&mut files, &mut store, receipt.generation)
        .unwrap();
    assert!(history.available().is_none());
    assert_eq!(files.inode(handle).unwrap(), inode);
    assert_eq!(files.info(handle).unwrap(), (8, 3));
    let request = Intent::Move {
        source: source(&store),
    };
    history.commit(&mut files, &mut store, request).unwrap();
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    let metadata = Key::metadata(item.id).unwrap();
    let mut before = [0; 128];
    assert_eq!(store.read_file(metadata.path(), &mut before).unwrap(), 128);
    history
        .commit(
            &mut files,
            &mut store,
            Intent::Restore {
                item,
                destination: Path::new("/native/alternative").unwrap(),
            },
        )
        .unwrap();
    let receipt = history.available().unwrap();
    assert_eq!(receipt.kind, Kind::Restore);
    history
        .undo(&mut files, &mut store, receipt.generation)
        .unwrap();
    let restored = trash::list(&mut store).unwrap().items[0].unwrap();
    assert_eq!(restored.id, item.id);
    assert_eq!(restored.original, item.original);
    let mut after = [0; 128];
    store.read_file(metadata.path(), &mut after).unwrap();
    assert_eq!(after, before);
    assert_eq!(files.inode(handle).unwrap(), inode);
    assert_eq!(files.info(handle).unwrap(), (8, 3));
    assert_eq!(
        history.undo(&mut files, &mut store, receipt.generation),
        Err(BackendError::NotFound)
    );
}
#[test]
fn stale_receipt_intervening_write_and_purge_never_restore_unstored_bytes() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let mut history = History::new();
    let request = Intent::Move {
        source: source(&store),
    };
    history.commit(&mut files, &mut store, request).unwrap();
    let receipt = history.available().unwrap();
    assert_eq!(
        history.undo(&mut files, &mut store, receipt.generation - 1),
        Err(BackendError::Conflict)
    );
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    let key = Key::data(item.id).unwrap();
    trash::write_held(&mut store, key.path(), b"newer").unwrap();
    assert_eq!(
        history.undo(&mut files, &mut store, receipt.generation),
        Err(BackendError::Conflict)
    );
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    history
        .commit(&mut files, &mut store, Intent::Purge { item })
        .unwrap();
    assert!(history.available().is_none());
    assert_eq!(
        history.undo(&mut files, &mut store, receipt.generation),
        Err(BackendError::NotFound)
    );
    let mut reboot_history = History::new();
    assert!(reboot_history.available().is_none());
    reboot_history.clear();
}
#[test]
fn undo_conflict_and_read_only_preserve_existing_receipt_and_media() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let mut history = History::new();
    let request = Intent::Move {
        source: source(&store),
    };
    history.commit(&mut files, &mut store, request).unwrap();
    let receipt = history.available().unwrap();
    store.restrict_read_only();
    assert_eq!(
        history.undo(&mut files, &mut store, receipt.generation),
        Err(BackendError::ReadOnly)
    );
    assert_eq!(history.available(), Some(receipt));
    let _ = store.into_partition();
    ram.crash();
    let before = ram.sectors.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    store.put("/native/live", b"different").unwrap();
    assert_eq!(
        history.undo(&mut files, &mut store, receipt.generation),
        Err(BackendError::Conflict)
    );
    assert_eq!(history.available(), Some(receipt));
    let mut bytes = [0; FILE_BYTES];
    assert_eq!(store.read_file("/native/live", &mut bytes).unwrap(), 9);
    assert_eq!(&bytes[..9], b"different");
    let _ = store.into_partition();
    assert_ne!(ram.sectors, before);
    ram.protected();
}
#[test]
fn every_undo_write_and_flush_failure_keeps_cache_old_and_recovers_whole_pair() {
    for restore in [false, true] {
        for after in [false, true] {
            for boundary in 1..=69 {
                let mut ram = setup();
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let mut files = Files::<16>::new();
                let mut history = History::new();
                let handle = files
                    .open(&mut Disk(&mut store), "/native/live", false, false)
                    .unwrap();
                let inode = files.inode(handle).unwrap();
                files.seek(handle, 2, 0).unwrap();
                let request = Intent::Move {
                    source: source(&store),
                };
                history.commit(&mut files, &mut store, request).unwrap();
                let item = trash::list(&mut store).unwrap().items[0].unwrap();
                if restore {
                    history
                        .commit(
                            &mut files,
                            &mut store,
                            Intent::Restore {
                                item,
                                destination: Path::new("/native/alternative").unwrap(),
                            },
                        )
                        .unwrap();
                }
                let receipt = history.available().unwrap();
                let _ = store.into_partition();
                ram.crash();
                ram.failure = Some((boundary, after));
                let mut store = Store::mount(part(&mut ram)).unwrap();
                assert_eq!(
                    history.undo(&mut files, &mut store, receipt.generation),
                    Err(BackendError::Io)
                );
                assert!(store.is_read_only());
                assert_eq!(history.available(), Some(receipt));
                assert_eq!(files.info(handle).unwrap(), (8, 2));
                assert_eq!(files.inode(handle).unwrap(), inode);
                let named = if restore {
                    "/native/alternative".to_owned()
                } else {
                    Key::data(item.id).unwrap().path().to_owned()
                };
                let mut bytes = [0; FILE_BYTES];
                assert_eq!(
                    files
                        .read_path(&mut Disk(&mut store), &named, &mut bytes)
                        .unwrap(),
                    8
                );
                assert_eq!(&bytes[..8], b"original");
                assert_eq!(
                    history.undo(&mut files, &mut store, receipt.generation),
                    Err(BackendError::ReadOnly)
                );
                let _ = store.into_partition();
                assert_eq!(ram.operations, boundary);
                ram.crash();
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let listing = trash::list(&mut store).unwrap();
                let recovered = listing.items[0];
                if let Some(found) = recovered {
                    assert_eq!(found.id, item.id);
                    assert_eq!(found.original, item.original);
                    let key = Key::data(item.id).unwrap();
                    assert_eq!(store.read_file(key.path(), &mut bytes).unwrap(), 8);
                } else {
                    let path = if restore {
                        "/native/alternative"
                    } else {
                        "/native/live"
                    };
                    assert_eq!(store.read_file(path, &mut bytes).unwrap(), 8);
                }
                assert_eq!(&bytes[..8], b"original");
                let _ = store.into_partition();
                ram.protected();
            }
        }
    }
}

#[test]
fn failed_forward_restore_preserves_prior_receipt_without_any_media_write() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let mut history = History::new();
    let request = Intent::Move {
        source: source(&store),
    };
    history.commit(&mut files, &mut store, request).unwrap();
    let prior = history.available();
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    let _ = store.into_partition();
    ram.crash();
    let before = ram.sectors.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    assert_eq!(
        history.commit(
            &mut files,
            &mut store,
            Intent::Restore {
                item,
                destination: Path::new("/native/keep").unwrap()
            }
        ),
        Err(BackendError::Exists)
    );
    assert_eq!(history.available(), prior);
    assert_eq!(store.generation(), item.revision);
    let _ = store.into_partition();
    assert_eq!(ram.operations, 0);
    assert_eq!(ram.sectors, before);
    assert!(ram.pending.is_empty());
}

#[test]
fn actual_shell_callback_precedes_stale_receipt_retirement_after_held_write() {
    use prism::{
        trash_ui::{UndoKind, UndoReceipt},
        Key as UiKey, Shell,
    };
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let mut history = History::new();
    let request = Intent::Move {
        source: source(&store),
    };
    history.commit(&mut files, &mut store, request).unwrap();
    let receipt = history.available().unwrap();
    let mut shell = Shell::new();
    shell
        .set_trash_listing(trash::list(&mut store).unwrap())
        .unwrap();
    shell
        .set_trash_undo_receipt(Some(UndoReceipt {
            generation: receipt.generation,
            kind: UndoKind::Move,
            path: receipt.path,
        }))
        .unwrap();
    shell.show_trash();
    shell.on_key(UiKey::Character('u'));
    let action = shell.take_trash_undo_action().unwrap();
    assert_eq!(action.generation, receipt.generation);
    assert!(shell.take_trash_undo_action().is_none());
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    let key = Key::data(item.id).unwrap();
    trash::write_held(&mut store, key.path(), b"newer").unwrap();
    let result = history.undo(&mut files, &mut store, action.generation);
    assert_eq!(result, Err(BackendError::Conflict));
    // Root's actual ordering: acknowledge this request first, then refresh the
    // authoritative current generation and retire its stale receipt.
    shell.trash_undo_result(action.request, result).unwrap();
    history.clear();
    shell.set_trash_undo_receipt(None).unwrap();
    shell
        .set_trash_listing(trash::list(&mut store).unwrap())
        .unwrap();
    assert!(shell.trash_undo_receipt().is_none());
    shell.on_key(UiKey::Character('u'));
    assert!(shell.take_trash_undo_action().is_none());
    assert_eq!(
        shell.trash_undo_result(action.request, Ok(())),
        Err(prism::trash_ui::Error::StaleCompletion)
    );
    let mut bytes = [0; FILE_BYTES];
    assert_eq!(store.read_file(key.path(), &mut bytes).unwrap(), 5);
    assert_eq!(&bytes[..5], b"newer");
}
