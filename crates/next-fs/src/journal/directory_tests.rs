#![allow(dead_code)]
use super::directories as directory;
use super::directories::{Error as DirError, Kind};
use super::{Partition, Store, WritableBlockDevice};
use crate::BlockDevice;
use std::collections::BTreeMap;
use std::{borrow::ToOwned, format, string::String, vec, vec::Vec};
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
    let g = store.generation();
    directory::mkdir(&mut store, 1, g, "/native/a").unwrap();
    let _ = store.into_partition();
    ram.crash();
    ram
}
fn paths(store: &mut Store<'_, Ram>, path: &str) -> Vec<(String, Kind, u32, u64)> {
    let list = directory::list(store, 1, path).unwrap();
    list.entries[..list.count]
        .iter()
        .map(|e| (e.path().to_owned(), e.kind, e.size, e.id))
        .collect()
}
#[test]
fn explicit_nested_hierarchy_survives_remount_and_parent_enforcement() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let g = store.generation();
    directory::mkdir(&mut store, 1, g, "/native/a/b").unwrap();
    let g = store.generation();
    directory::create_file(&mut store, 1, g, "/native/a/b/data", b"\0\xff").unwrap();
    assert_eq!(paths(&mut store, "/native/a")[0].0, "/native/a/b");
    assert_eq!(paths(&mut store, "/native/a/b")[0].2, 2);
    let g = store.generation();
    assert_eq!(
        directory::rmdir(&mut store, 1, g, "/native/a"),
        Err(DirError::<()>::NotEmpty)
    );
    assert_eq!(
        directory::rename_empty(&mut store, 1, g, "/native/a", "/native/new"),
        Err(DirError::<()>::NotEmpty)
    );
    let _ = store.into_partition();
    ram.crash();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    assert_eq!(paths(&mut store, "/native").len(), 1);
    let g = store.generation();
    directory::remove_file(&mut store, 1, g, "/native/a/b/data").unwrap();
    let g = store.generation();
    directory::rmdir(&mut store, 1, g, "/native/a/b").unwrap();
    let id = paths(&mut store, "/native")[0].3;
    let g = store.generation();
    directory::rename_empty(&mut store, 1, g, "/native/a", "/native/renamed").unwrap();
    assert_eq!(
        paths(&mut store, "/native")[0],
        ("/native/renamed".to_owned(), Kind::Directory, 0, id)
    );
    ram.protected();
}
#[test]
fn files_do_not_manufacture_prefix_folders_and_magic_bytes_are_only_data() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    store.put("/native/ghost/file", b"x").unwrap();
    let writes = 69;
    let g = store.generation();
    assert_eq!(
        directory::list(&mut store, 1, "/native"),
        Err(DirError::<()>::MissingParent)
    );
    assert_eq!(store.generation(), g);
    let _ = store.into_partition();
    assert_eq!(ram.operations, writes);
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let g = store.generation();
    directory::create_file(&mut store, 1, g, "/native/magic", b"NXTDIR01").unwrap();
    assert_eq!(
        directory::list(&mut store, 1, "/native/magic"),
        Err(DirError::<()>::NotDirectory)
    );
    assert!(paths(&mut store, "/native")
        .iter()
        .any(|e| e.0 == "/native/magic" && e.1 == Kind::File));
}
#[test]
fn invalid_canonical_paths_and_self_descendant_rename_issue_zero_writes() {
    let mut ram = setup();
    let before = ram.sectors.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let g = store.generation();
    for path in [
        "/native/../x",
        "/native/a//b",
        "/native/a/",
        "/native/a b",
        "/dirs/fake",
        "/trash/x",
        "/nativeevil/x",
        "/native/012345678901234567890123456789",
    ] {
        assert_eq!(
            directory::mkdir(&mut store, 1, g, path),
            Err(DirError::<()>::Path)
        );
    }
    assert_eq!(
        directory::rename_empty(&mut store, 1, g, "/native/a", "/native/a/b"),
        Err(DirError::<()>::Path)
    );
    assert_eq!(
        directory::create_file(&mut store, 1, g, "/native/absent/file", b"x"),
        Err(DirError::<()>::MissingParent)
    );
    assert_eq!(
        store.put("/dirs/arbitrary", b"x"),
        Err(super::Error::InvalidPath)
    );
    assert_eq!(
        store.remove("/dirs/arbitrary"),
        Err(super::Error::InvalidPath)
    );
    assert_eq!(
        store.rename("/native/a", "/dirs/m.0000000000000002"),
        Err(super::Error::InvalidPath)
    );
    assert_eq!(
        store.transaction(
            g,
            &[super::transaction::Mutation::Create {
                path: "/dirs/arbitrary",
                bytes: b"x"
            }]
        ),
        Err(super::transaction::TransactionError::Journal(
            super::Error::InvalidPath
        ))
    );
    let _ = store.into_partition();
    assert_eq!(ram.operations, 0);
    assert_eq!(ram.sectors, before);
}
#[test]
fn exact_generation_owner_collision_full_and_readonly_preserve_whole_media() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let g = store.generation();
    assert_eq!(
        directory::mkdir(&mut store, 1, g - 1, "/native/b"),
        Err(DirError::<()>::Conflict)
    );
    assert_eq!(
        directory::list(&mut store, 2, "/native"),
        Err(DirError::<()>::Corrupt)
    );
    assert_eq!(
        directory::mkdir(&mut store, 1, g, "/native/a"),
        Err(DirError::<()>::Exists)
    );
    assert_eq!(
        directory::create_file(&mut store, 1, g, "/native/a", b"x"),
        Err(DirError::<()>::Exists)
    );
    for i in 0..7 {
        store.put(&format!("/native/f{i}"), b"x").unwrap();
    }
    let g = store.generation();
    let _ = store.into_partition();
    ram.crash();
    let before = ram.sectors.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    assert_eq!(
        directory::mkdir(&mut store, 1, g, "/native/b"),
        Err(DirError::<()>::Full)
    );
    store.restrict_read_only();
    assert_eq!(
        directory::rmdir(&mut store, 1, g, "/native/a"),
        Err(DirError::<()>::Store(super::Error::<()>::ReadOnly))
    );
    let _ = store.into_partition();
    assert_eq!(ram.operations, 0);
    assert_eq!(ram.sectors, before);
}
#[test]
fn malformed_private_metadata_and_root_shadow_are_not_repaired() {
    for mode in 0..4 {
        let mut ram = setup();
        let mut store = Store::mount(part(&mut ram)).unwrap();
        let mut info = [super::FileInfo::EMPTY; 8];
        store.list(&mut info).unwrap();
        let key = info[0].path().to_owned();
        let mut raw = [0; 96];
        store.read_file(&key, &mut raw).unwrap();
        let g = store.generation();
        match mode {
            0 => {
                raw[64] = 1;
                let c = super::crc32(&raw[..92]);
                raw[92..].copy_from_slice(&c.to_le_bytes());
                store
                    .directory_transaction(
                        g,
                        &[super::transaction::Mutation::Replace {
                            path: &key,
                            bytes: &raw,
                        }],
                    )
                    .unwrap();
            }
            1 => {
                store
                    .directory_transaction(
                        g,
                        &[super::transaction::Mutation::Create {
                            path: "/dirs/unknown",
                            bytes: b"x",
                        }],
                    )
                    .unwrap();
            }
            2 => {
                store.put("/native", b"x").unwrap();
            }
            _ => {
                raw[16..24].copy_from_slice(&2u64.to_le_bytes());
                let c = super::crc32(&raw[..92]);
                raw[92..].copy_from_slice(&c.to_le_bytes());
                store
                    .directory_transaction(
                        g,
                        &[super::transaction::Mutation::Replace {
                            path: &key,
                            bytes: &raw,
                        }],
                    )
                    .unwrap();
            }
        }
        let _ = store.into_partition();
        ram.crash();
        let before = ram.sectors.clone();
        let mut store = Store::mount(part(&mut ram)).unwrap();
        assert_eq!(
            directory::list(&mut store, 1, "/native"),
            Err(DirError::<()>::Corrupt)
        );
        let _ = store.into_partition();
        assert_eq!(ram.operations, 0);
        assert_eq!(ram.sectors, before);
    }
}
#[test]
fn every_write_flush_fault_recovers_complete_old_or_new_directory_namespace() {
    for op in 0..4 {
        for after in [false, true] {
            for boundary in 1..=69 {
                let mut ram = setup();
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let g = store.generation();
                let old = paths(&mut store, "/native");
                let _ = store.into_partition();
                ram.failure = Some((boundary, after));
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let result = match op {
                    0 => directory::mkdir(&mut store, 1, g, "/native/a/b"),
                    1 => directory::rename_empty(&mut store, 1, g, "/native/a", "/native/new"),
                    2 => directory::rmdir(&mut store, 1, g, "/native/a"),
                    _ => directory::create_file(&mut store, 1, g, "/native/a/file", b"new"),
                };
                assert!(result.is_err(), "op{op} boundary{boundary} after{after}");
                assert!(store.is_read_only());
                assert_eq!(store.generation(), g);
                assert_eq!(paths(&mut store, "/native"), old);
                assert_eq!(
                    directory::mkdir(&mut store, 1, g, "/native/retry"),
                    Err(DirError::<()>::Store(super::Error::<()>::Indeterminate))
                );
                let _ = store.into_partition();
                assert_eq!(ram.operations, boundary);

                let writes = ram.operations;
                let mut store = Store::mount(part(&mut ram)).unwrap();
                store.restrict_read_only();
                let gg = store.generation();
                assert!(directory::mkdir(&mut store, 1, gg, "/native/retry").is_err());
                let _ = store.into_partition();
                assert_eq!(ram.operations, writes);
                ram.crash();
                let mut store = Store::mount(part(&mut ram)).unwrap();
                let generation = store.generation();
                assert!(generation == g || generation == g + 1);
                let root = paths(&mut store, "/native");
                if generation == g {
                    assert_eq!(root, old);
                } else {
                    match op {
                        0 => assert_eq!(paths(&mut store, "/native/a")[0].0, "/native/a/b"),
                        1 => assert_eq!(root[0].0, "/native/new"),
                        2 => assert!(root.is_empty()),
                        _ => assert_eq!(paths(&mut store, "/native/a")[0].0, "/native/a/file"),
                    }
                }
                let _ = store.into_partition();
                ram.protected();
            }
        }
    }
}

#[test]
fn current_folder_and_receipts_bind_exact_id_and_revision() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let root = directory::snapshot(&mut store, 1, "/native").unwrap();
    assert_eq!(root.folder.id, 0);
    let source = directory::snapshot(&mut store, 1, "/native/a").unwrap();
    assert!(source.folder.id > 0);
    assert_eq!(
        directory::mkdir_at(&mut store, 1, root, "/native/a/nested"),
        Err(DirError::<()>::Path)
    );
    directory::mkdir_at(&mut store, 1, source, "/native/a/child").unwrap();
    assert_eq!(
        directory::mkdir_at(&mut store, 1, source, "/native/a/other"),
        Err(DirError::<()>::Conflict)
    );
    let g = store.generation();
    directory::rmdir(&mut store, 1, g, "/native/a/child").unwrap();
    let source = directory::snapshot(&mut store, 1, "/native/a").unwrap();
    let mut forged = source;
    forged.folder.id += 1;
    assert_eq!(
        directory::rename_snapshot(&mut store, 1, forged, "/native/fake"),
        Err(DirError::<()>::Conflict)
    );
    directory::rename_snapshot(&mut store, 1, source, "/native/moved").unwrap();
    assert_eq!(
        directory::snapshot(&mut store, 1, "/native/moved")
            .unwrap()
            .folder
            .id,
        source.folder.id
    );
}

#[test]
fn cross_parent_rename_and_remove_bind_both_exact_snapshots() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let generation = store.generation();
    directory::mkdir(&mut store, 1, generation, "/native/destination").unwrap();
    let source = directory::snapshot(&mut store, 1, "/native/a").unwrap();
    let destination = directory::snapshot(&mut store, 1, "/native/destination").unwrap();
    assert_eq!(source.generation, destination.generation);
    directory::rename_between_snapshots(
        &mut store,
        1,
        source,
        destination,
        "/native/destination/a",
    )
    .unwrap();
    assert_eq!(
        directory::rename_between_snapshots(
            &mut store,
            1,
            source,
            destination,
            "/native/destination/stale",
        ),
        Err(DirError::<()>::Conflict)
    );
    let moved = directory::snapshot(&mut store, 1, "/native/destination/a").unwrap();
    assert_eq!(moved.folder.id, source.folder.id);
    let mut forged = moved;
    forged.folder.id += 1;
    assert_eq!(
        directory::rmdir_snapshot(&mut store, 1, forged),
        Err(DirError::<()>::Conflict)
    );
    directory::rmdir_snapshot(&mut store, 1, moved).unwrap();
    assert_eq!(
        directory::snapshot(&mut store, 1, "/native/destination/a"),
        Err(DirError::<()>::NotFound)
    );
}

#[test]
fn interleaved_stale_unlink_cannot_remove_a_same_path_replacement() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let generation = store.generation();
    directory::mkdir(&mut store, 1, generation, "/native/destination").unwrap();

    // Model two actors that both captured source and destination snapshots
    // before either actor mutates the namespace. The first actor moves the
    // source; the second still holds the old unlink/rename intent.
    let stale_source = directory::snapshot(&mut store, 1, "/native/a").unwrap();
    let destination = directory::snapshot(&mut store, 1, "/native/destination").unwrap();
    let original_id = stale_source.folder.id;
    directory::rename_between_snapshots(
        &mut store,
        1,
        stale_source,
        destination,
        "/native/destination/a",
    )
    .unwrap();

    assert_eq!(
        directory::rmdir_snapshot(&mut store, 1, stale_source),
        Err(DirError::<()>::Conflict)
    );
    assert_eq!(
        directory::rename_between_snapshots(
            &mut store,
            1,
            stale_source,
            destination,
            "/native/destination/second-name",
        ),
        Err(DirError::<()>::Conflict)
    );

    // Recreate the old spelling. Repeated stale unlink attempts must not
    // resolve by path and accidentally delete this new directory identity.
    let generation = store.generation();
    directory::mkdir(&mut store, 1, generation, "/native/a").unwrap();
    let replacement = directory::snapshot(&mut store, 1, "/native/a").unwrap();
    assert_ne!(replacement.folder.id, original_id);
    for _ in 0..32 {
        assert_eq!(
            directory::rmdir_snapshot(&mut store, 1, stale_source),
            Err(DirError::<()>::Conflict)
        );
    }
    assert_eq!(store.generation(), replacement.generation);

    assert_eq!(
        directory::snapshot(&mut store, 1, "/native/a")
            .unwrap()
            .folder
            .id,
        replacement.folder.id
    );
    assert_eq!(
        directory::snapshot(&mut store, 1, "/native/destination/a")
            .unwrap()
            .folder
            .id,
        original_id
    );
}
