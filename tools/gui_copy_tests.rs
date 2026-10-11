#![allow(dead_code)]
#[path = "../crates/helix-kernel/src/gui_copy.rs"]
mod gui_copy;
use gui_copy::{copy, CopyRequest};
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::{
    journal::{self, Partition, Store, WritableBlockDevice},
    BlockDevice,
};
use prism::files_actions::{BackendError, Path, Snapshot};
struct Ram {
    bytes: Vec<[u8; 512]>,
    fail: bool,
    writes: usize,
}
impl Ram {
    fn new() -> Self {
        Self {
            bytes: vec![[0; 512]; 132],
            fail: false,
            writes: 0,
        }
    }
}
impl BlockDevice for Ram {
    type Error = ();
    fn sector_count(&self) -> u64 {
        132
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
        *out = self.bytes[lba as usize];
        Ok(())
    }
}
impl WritableBlockDevice for Ram {
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), ()> {
        if self.fail {
            return Err(());
        }
        self.writes += 1;
        self.bytes[lba as usize] = *data;
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
struct Disk<'a, 'b>(&'a mut Store<'b, Ram>);
impl Backend for Disk<'_, '_> {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
        self.0.read_file(path, out).map_err(|_| file::Error::Io)
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), file::Error> {
        self.0.put(path, bytes).map_err(|_| file::Error::Io)
    }
}
fn p(s: &str) -> Path {
    Path::new(s).unwrap()
}
fn request(store: &Store<'_, Ram>, dest: &str, replace: bool) -> CopyRequest {
    CopyRequest {
        source: Snapshot {
            path: p("/native/source"),
            revision: store.generation(),
        },
        destination: p(dest),
        replace: replace.then_some(Snapshot {
            path: p(dest),
            revision: store.generation(),
        }),
    }
}
fn format(ram: &mut Ram) -> Store<'_, Ram> {
    Store::format(Partition::new(ram, 0, 132, &[]).unwrap()).unwrap()
}
#[test]
fn copy_preserves_source_and_updates_existing_target_inode_without_changing_offsets() {
    let mut ram = Ram::new();
    let mut store = format(&mut ram);
    let mut files = Files::<16>::new();
    files
        .replace(&mut Disk(&mut store), "/native/source", b"source bytes")
        .unwrap();
    files
        .replace(&mut Disk(&mut store), "/native/target", b"old target")
        .unwrap();
    let source = files
        .open(&mut Disk(&mut store), "/native/source", false, false)
        .unwrap();
    let target = files
        .open(&mut Disk(&mut store), "/native/target", false, false)
        .unwrap();
    files.seek(target, 2, 0).unwrap();
    let before = store.generation();
    let req = request(&store, "/native/target", true);
    copy(&mut files, &mut store, req).unwrap();
    assert_eq!(store.generation(), before + 1);
    let mut out = [0; FILE_BYTES];
    assert_eq!(files.read(target, &mut out).unwrap(), 10);
    assert_eq!(&out[..10], b"urce bytes");
    assert_eq!(files.read(source, &mut out).unwrap(), 12);
    assert_eq!(&out[..12], b"source bytes");
    let req = request(&store, "/native/new", false);
    copy(&mut files, &mut store, req).unwrap();
    assert_eq!(store.read_file("/native/new", &mut out).unwrap(), 12);
    assert_eq!(&out[..12], b"source bytes");
}
#[test]
fn unconfirmed_overwrite_stale_or_wrong_confirmation_and_same_path_refuse() {
    let mut ram = Ram::new();
    let mut store = format(&mut ram);
    let mut files = Files::<16>::new();
    store.put("/native/source", b"source").unwrap();
    store.put("/native/target", b"target").unwrap();
    let generation = store.generation();
    let base = request(&store, "/native/target", true);
    let mut cases = vec![
        (
            request(&store, "/native/target", false),
            BackendError::Exists,
        ),
        (
            request(&store, "/native/source", true),
            BackendError::InvalidPath,
        ),
        (
            request(&store, "/prefs/appearance", false),
            BackendError::ReadOnly,
        ),
    ];
    let mut stale = base;
    stale.replace.as_mut().unwrap().revision -= 1;
    cases.push((stale, BackendError::Conflict));
    let mut stale = base;
    stale.source.revision -= 1;
    cases.push((stale, BackendError::Conflict));
    let mut wrong = base;
    wrong.replace.as_mut().unwrap().path = p("/native/other");
    cases.push((wrong, BackendError::Conflict));
    for (req, error) in cases {
        assert_eq!(copy(&mut files, &mut store, req), Err(error));
        assert_eq!(store.generation(), generation);
    }
    let mut out = [0; FILE_BYTES];
    assert_eq!(store.read_file("/native/target", &mut out).unwrap(), 6);
    assert_eq!(&out[..6], b"target");
}
#[test]
fn full_and_read_only_store_refuse_copy_without_changing_namespace() {
    for readonly in [false, true] {
        let mut ram = Ram::new();
        let mut store = format(&mut ram);
        let mut files = Files::<16>::new();
        store.put("/native/source", b"source").unwrap();
        if readonly {
            store.restrict_read_only();
        } else {
            for n in 0..7 {
                store.put(&format!("/native/{n}"), b"occupied").unwrap();
            }
        }
        let before = store.generation();
        let req = request(&store, "/native/new", false);
        assert_eq!(
            copy(&mut files, &mut store, req),
            Err(if readonly {
                BackendError::ReadOnly
            } else {
                BackendError::Full
            })
        );
        assert_eq!(store.generation(), before);
        let mut out = [0; FILE_BYTES];
        assert_eq!(
            store.read_file("/native/new", &mut out),
            Err(journal::Error::NotFound)
        );
    }
}
#[test]
fn actual_device_failure_does_not_publish_target_cache_or_permit_retry() {
    let mut ram = Ram::new();
    let mut store = format(&mut ram);
    store.put("/native/source", b"source").unwrap();
    store.put("/native/target", b"target").unwrap();
    drop(store);
    ram.fail = true;
    let mut store = Store::mount(Partition::new(&mut ram, 0, 132, &[]).unwrap()).unwrap();
    let mut files = Files::<16>::new();
    let target = files
        .open(&mut Disk(&mut store), "/native/target", false, false)
        .unwrap();
    let before = store.generation();
    let req = request(&store, "/native/target", true);
    assert_eq!(copy(&mut files, &mut store, req), Err(BackendError::Io));
    assert_eq!(store.generation(), before);
    assert!(store.is_read_only());
    let mut out = [0; FILE_BYTES];
    assert_eq!(files.read(target, &mut out).unwrap(), 6);
    assert_eq!(&out[..6], b"target");
    assert_eq!(
        copy(&mut files, &mut store, req),
        Err(BackendError::ReadOnly)
    );
}

#[test]
fn binary_maximum_and_empty_sources_copy_exactly_with_one_generation_commit() {
    for source in [vec![], (0..FILE_BYTES).map(|i| (i % 251) as u8).collect()] {
        let mut ram = Ram::new();
        let mut store = format(&mut ram);
        store.put("/native/source", &source).unwrap();
        store.put("/native/target", &[0x5a; FILE_BYTES]).unwrap();
        let mut files = Files::<16>::new();
        let target = files
            .open(&mut Disk(&mut store), "/native/target", false, false)
            .unwrap();
        let before = store.generation();
        let req = request(&store, "/native/target", true);
        copy(&mut files, &mut store, req).unwrap();
        assert_eq!(store.generation(), before + 1);
        let mut out = [0; FILE_BYTES];
        assert_eq!(files.read(target, &mut out).unwrap(), source.len());
        assert_eq!(&out[..source.len()], source);
        assert_eq!(
            store.read_file("/native/source", &mut out).unwrap(),
            source.len()
        );
        assert_eq!(&out[..source.len()], source);
        assert_eq!(
            copy(&mut files, &mut store, req),
            Err(BackendError::Conflict)
        );
    }
}

#[test]
fn copy_into_nested_folder_uses_explicit_parent_and_publishes_real_file() {
    let mut ram = Ram::new();
    let mut store = format(&mut ram);
    store.put("/native/source", b"source bytes").unwrap();
    let before_mkdir = store.generation();
    next_fs::journal::directories::mkdir(
        &mut store,
        0x4e45_5854_4445_5631,
        before_mkdir,
        "/native/docs",
    )
    .unwrap();
    let mut files = Files::<16>::new();
    let revision = store.generation();
    let req = CopyRequest {
        source: Snapshot {
            path: p("/native/source"),
            revision,
        },
        destination: p("/native/docs/copy"),
        replace: None,
    };
    copy(&mut files, &mut store, req).unwrap();
    let mut out = [0; FILE_BYTES];
    assert_eq!(store.read_file("/native/docs/copy", &mut out).unwrap(), 12);
    assert_eq!(&out[..12], b"source bytes");
    let listing =
        next_fs::journal::directories::list(&mut store, 0x4e45_5854_4445_5631, "/native/docs")
            .unwrap();
    assert_eq!(listing.count, 1);
    assert_eq!(listing.entries[0].path(), "/native/docs/copy");
}
