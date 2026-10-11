#![allow(dead_code)]
#[path = "../crates/helix-kernel/src/trash.rs"]
mod trash;
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::{
    journal::{self, Partition, Store, WritableBlockDevice},
    trash_record::{Key, Record},
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
#[test]
fn open_move_write_restore_preserves_inode_offsets_and_paired_fingerprint() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let handle = files
        .open(&mut Disk(&mut store), "/native/live", false, false)
        .unwrap();
    let inode = files.inode(handle).unwrap();
    files.retain(handle).unwrap();
    files.seek(handle, 3, 0).unwrap();
    let before = store.generation();
    let request = Intent::Move {
        source: source(&store),
    };
    trash::commit(&mut files, &mut store, request).unwrap();
    assert_eq!(store.generation(), before + 1);
    assert_eq!(files.seek(handle, 0, 1).unwrap(), 3);
    files.seek(handle, 0, 0).unwrap();
    files
        .write(&mut Disk(&mut store), handle, b"MODIFIED")
        .unwrap();
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    let metadata = Key::metadata(item.id).unwrap();
    let mut record = [0; 128];
    let len = store.read_file(metadata.path(), &mut record).unwrap();
    let record = Record::decode(&record[..len], 1).unwrap();
    assert_eq!(record.checksum, journal::crc32(b"MODIFIED"));
    assert_eq!(
        store.read_file("/native/live", &mut [0; FILE_BYTES]),
        Err(journal::Error::NotFound)
    );
    let destination = Path::new("/native/restored").unwrap();
    trash::commit(
        &mut files,
        &mut store,
        Intent::Restore { item, destination },
    )
    .unwrap();
    assert!(trash::list(&mut store)
        .unwrap()
        .items
        .iter()
        .all(Option::is_none));
    let reopened = files
        .open(&mut Disk(&mut store), destination.as_str(), false, false)
        .unwrap();
    assert_eq!(files.inode(reopened).unwrap(), inode);
    assert_eq!(files.seek(handle, 0, 1).unwrap(), 8);
    let mut bytes = [0; FILE_BYTES];
    assert_eq!(files.read(reopened, &mut bytes).unwrap(), 8);
    assert_eq!(&bytes[..8], b"MODIFIED");
    files.close(reopened).unwrap();
    files.close(handle).unwrap();
    files.close(handle).unwrap();
    let _ = store.into_partition();
    ram.protected();
}
#[test]
fn collision_full_budget_read_only_and_forged_item_preserve_data() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let request = Intent::Move {
        source: source(&store),
    };
    trash::commit(&mut files, &mut store, request).unwrap();
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    let before = store.generation();
    assert_eq!(
        trash::commit(
            &mut files,
            &mut store,
            Intent::Restore {
                item,
                destination: Path::new("/native/keep").unwrap()
            }
        ),
        Err(BackendError::Exists)
    );
    assert_eq!(store.generation(), before);
    let mut forged = item;
    forged.size += 1;
    assert_eq!(
        trash::commit(&mut files, &mut store, Intent::Purge { item: forged }),
        Err(BackendError::Conflict)
    );
    let mut stale = item;
    stale.revision -= 1;
    assert_eq!(
        trash::commit(&mut files, &mut store, Intent::Purge { item: stale }),
        Err(BackendError::Conflict)
    );
    store.put("/native/second", b"second").unwrap();
    let request = Intent::Move {
        source: Snapshot {
            path: Path::new("/native/second").unwrap(),
            revision: store.generation(),
        },
    };
    trash::commit(&mut files, &mut store, request).unwrap();
    let before = store.generation();
    let request = Intent::Move {
        source: Snapshot {
            path: Path::new("/native/keep").unwrap(),
            revision: before,
        },
    };
    assert_eq!(
        trash::commit(&mut files, &mut store, request),
        Err(BackendError::Full)
    );
    assert_eq!(store.generation(), before);
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    store.restrict_read_only();
    assert_eq!(
        trash::commit(&mut files, &mut store, Intent::Purge { item }),
        Err(BackendError::ReadOnly)
    );
    assert_eq!(store.generation(), before);
}
#[test]
fn purge_leaves_open_inode_anonymous_and_cannot_recreate_pair() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let handle = files
        .open(&mut Disk(&mut store), "/native/live", false, false)
        .unwrap();
    let request = Intent::Move {
        source: source(&store),
    };
    trash::commit(&mut files, &mut store, request).unwrap();
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    trash::commit(&mut files, &mut store, Intent::Purge { item }).unwrap();
    let generation = store.generation();
    files
        .write(&mut Disk(&mut store), handle, b"anonymous")
        .unwrap();
    assert_eq!(store.generation(), generation);
    assert!(trash::list(&mut store)
        .unwrap()
        .items
        .iter()
        .all(Option::is_none));
    assert_eq!(
        store.read_file("/native/live", &mut [0; FILE_BYTES]),
        Err(journal::Error::NotFound)
    );
    let mut bytes = [0; FILE_BYTES];
    files.seek(handle, 0, 0).unwrap();
    assert_eq!(files.read(handle, &mut bytes).unwrap(), 9);
    assert_eq!(&bytes[..9], b"anonymous");
}
#[test]
fn malformed_owner_or_orphaned_pair_rejects_without_repair() {
    for mode in 0..3 {
        let mut ram = setup();
        let mut store = Store::mount(part(&mut ram)).unwrap();
        let mut files = Files::<16>::new();
        let request = Intent::Move {
            source: source(&store),
        };
        trash::commit(&mut files, &mut store, request).unwrap();
        let item = trash::list(&mut store).unwrap().items[0].unwrap();
        let metadata = Key::metadata(item.id).unwrap();
        if mode == 0 {
            store.remove(metadata.path()).unwrap();
        } else {
            let record = Record::new(if mode == 1 { 2 } else { 1 }, item.id, "/native/live", 8, 0)
                .unwrap()
                .encode();
            store.put(metadata.path(), &record).unwrap();
        }
        let generation = store.generation();
        let _ = store.into_partition();
        ram.crash();
        let before = ram.sectors.clone();
        let mut store = Store::mount(part(&mut ram)).unwrap();
        assert_eq!(trash::list(&mut store), Err(BackendError::Io));
        assert_eq!(store.generation(), generation);
        let _ = store.into_partition();
        assert_eq!(ram.operations, 0);
        assert!(ram.pending.is_empty());
        assert_eq!(ram.sectors, before);
    }
}
#[test]
fn move_restore_and_purge_failures_preserve_live_cache_and_whole_recovery_pair() {
    for stage in 0..3 {
        let mut baseline = setup();
        if stage != 0 {
            let mut store = Store::mount(part(&mut baseline)).unwrap();
            let mut files = Files::<16>::new();
            let request = Intent::Move {
                source: source(&store),
            };
            trash::commit(&mut files, &mut store, request).unwrap();
            let _ = store.into_partition();
            baseline.crash();
        }
        for after in [false, true] {
            for boundary in 1..=69 {
                let mut ram = baseline.clone();
                ram.failure = Some((boundary, after));
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let mut files = Files::<16>::new();
                let (path, intent) = if stage == 0 {
                    (
                        "/native/live".to_owned(),
                        Intent::Move {
                            source: source(&store),
                        },
                    )
                } else {
                    let item = trash::list(&mut store).unwrap().items[0].unwrap();
                    (
                        Key::data(item.id).unwrap().path().to_owned(),
                        if stage == 1 {
                            Intent::Restore {
                                item,
                                destination: Path::new("/native/restored").unwrap(),
                            }
                        } else {
                            Intent::Purge { item }
                        },
                    )
                };
                let handle = files
                    .open(&mut Disk(&mut store), &path, false, false)
                    .unwrap();
                assert_eq!(
                    trash::commit(&mut files, &mut store, intent),
                    Err(BackendError::Io)
                );
                assert!(store.is_read_only());
                let mut bytes = [0; FILE_BYTES];
                assert_eq!(files.read(handle, &mut bytes).unwrap(), 8);
                assert_eq!(&bytes[..8], b"original");
                assert_eq!(
                    files
                        .read_path(&mut Disk(&mut store), &path, &mut bytes)
                        .unwrap(),
                    8
                );
                let _ = store.into_partition();
                ram.crash();
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let listing = trash::list(&mut store).unwrap();
                let count = listing.items.iter().flatten().count();
                assert!(count <= 1);
                let original = store.read_file("/native/live", &mut bytes).is_ok();
                let restored = store.read_file("/native/restored", &mut bytes).is_ok();
                assert!(match stage {
                    0 => original != (count == 1),
                    1 => restored != (count == 1),
                    _ => !original && !restored,
                });
                let _ = store.into_partition();
                ram.protected();
            }
        }
    }
}

#[test]
fn full_eight_object_store_refuses_move_before_cache_or_media_changes() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    for index in 0..6 {
        store
            .put(&format!("/native/full{index}"), b"occupied")
            .unwrap();
    }
    let _ = store.into_partition();
    ram.crash();
    let before = ram.sectors.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let handle = files
        .open(&mut Disk(&mut store), "/native/live", false, false)
        .unwrap();
    let generation = store.generation();
    let request = Intent::Move {
        source: source(&store),
    };
    assert_eq!(
        trash::commit(&mut files, &mut store, request),
        Err(BackendError::Full)
    );
    assert_eq!(store.generation(), generation);
    let mut bytes = [0; FILE_BYTES];
    assert_eq!(files.read(handle, &mut bytes).unwrap(), 8);
    assert_eq!(&bytes[..8], b"original");
    assert_eq!(store.read_file("/native/live", &mut bytes).unwrap(), 8);
    assert!(trash::list(&mut store)
        .unwrap()
        .items
        .iter()
        .all(Option::is_none));
    let _ = store.into_partition();
    assert_eq!(ram.operations, 0);
    assert!(ram.pending.is_empty());
    assert_eq!(ram.sectors, before);
}

#[test]
fn review_held_trash_write_faults_keep_live_inode_and_recover_matching_fingerprint() {
    let mut baseline = setup();
    {
        let mut store = Store::mount(part(&mut baseline)).unwrap();
        let mut files = Files::<16>::new();
        let request = Intent::Move {
            source: source(&store),
        };
        trash::commit(&mut files, &mut store, request).unwrap();
        let _ = store.into_partition();
    }
    baseline.crash();
    for after in [false, true] {
        for boundary in 1..=69 {
            let mut ram = baseline.clone();
            ram.failure = Some((boundary, after));
            let mut store = Store::mount(part(&mut ram)).unwrap();
            let item = trash::list(&mut store).unwrap().items[0].unwrap();
            let path = Key::data(item.id).unwrap();
            let mut files = Files::<16>::new();
            let handle = files
                .open(&mut Disk(&mut store), path.path(), false, false)
                .unwrap();
            let inode = files.inode(handle).unwrap();
            files.seek(handle, 2, 0).unwrap();
            assert_eq!(
                files.write(&mut Disk(&mut store), handle, b"CHANGED"),
                Err(file::Error::Io)
            );
            assert!(store.is_read_only());
            assert_eq!(files.info(handle).unwrap(), (8, 2));
            assert_eq!(files.inode(handle).unwrap(), inode);
            let mut bytes = [0; FILE_BYTES];
            assert_eq!(
                files
                    .read_path(&mut Disk(&mut store), path.path(), &mut bytes)
                    .unwrap(),
                8
            );
            assert_eq!(&bytes[..8], b"original");
            assert_eq!(
                files.write(&mut Disk(&mut store), handle, b"CHANGED"),
                Err(file::Error::Io)
            );
            assert_eq!(files.info(handle).unwrap(), (8, 2));
            let _ = store.into_partition();
            assert_eq!(ram.operations, boundary);
            ram.crash();
            let mut store = Store::mount(part(&mut ram)).unwrap();
            let recovered = trash::list(&mut store).unwrap().items[0].unwrap();
            assert_eq!(recovered.id, item.id);
            let len = store.read_file(path.path(), &mut bytes).unwrap();
            assert!(bytes[..len] == *b"original" || bytes[..len] == *b"orCHANGED");
            let metadata = Key::metadata(item.id).unwrap();
            let mut record = [0; 128];
            let count = store.read_file(metadata.path(), &mut record).unwrap();
            let record = Record::decode(&record[..count], 1).unwrap();
            assert_eq!(record.size as usize, len);
            assert_eq!(record.checksum, journal::crc32(&bytes[..len]));
            let _ = store.into_partition();
            ram.protected();
        }
    }
}

#[test]
fn review_restore_open_destination_reports_collision() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut files = Files::<16>::new();
    let destination = files
        .open(&mut Disk(&mut store), "/native/keep", false, false)
        .unwrap();
    let request = Intent::Move {
        source: source(&store),
    };
    trash::commit(&mut files, &mut store, request).unwrap();
    let item = trash::list(&mut store).unwrap().items[0].unwrap();
    assert_eq!(
        trash::commit(
            &mut files,
            &mut store,
            Intent::Restore {
                item,
                destination: Path::new("/native/keep").unwrap()
            }
        ),
        Err(BackendError::Exists)
    );
    assert_eq!(files.info(destination).unwrap(), (4, 0));
}

#[test]
fn empty_and_binary_files_roundtrip_without_text_conversion() {
    for payload in [&b""[..], &b"\0\xff\x80\n"[..]] {
        let mut ram = setup();
        let mut store = Store::mount(part(&mut ram)).unwrap();
        store.put("/native/live", payload).unwrap();
        let mut files = Files::<16>::new();
        let held = files
            .open(&mut Disk(&mut store), "/native/live", false, false)
            .unwrap();
        let snapshot = source(&store);
        trash::commit(&mut files, &mut store, Intent::Move { source: snapshot }).unwrap();
        let item = trash::list(&mut store).unwrap().items[0].unwrap();
        assert_eq!(item.size as usize, payload.len());
        trash::commit(
            &mut files,
            &mut store,
            Intent::Restore {
                item,
                destination: Path::new("/native/back").unwrap(),
            },
        )
        .unwrap();
        let mut bytes = [0; FILE_BYTES];
        assert_eq!(files.read(held, &mut bytes).unwrap(), payload.len());
        assert_eq!(&bytes[..payload.len()], payload);
        assert_eq!(
            store.read_file("/native/back", &mut bytes).unwrap(),
            payload.len()
        );
        assert_eq!(&bytes[..payload.len()], payload);
        assert!(trash::list(&mut store)
            .unwrap()
            .items
            .iter()
            .all(Option::is_none));
        let _ = store.into_partition();
        ram.protected();
    }
}
