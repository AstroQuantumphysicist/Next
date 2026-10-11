#![allow(dead_code)]
#[path = "../crates/helix-kernel/src/files_query.rs"]
mod files_query;
use next_fs::{
    journal::{self, FileInfo, Partition, Store, WritableBlockDevice},
    BlockDevice,
};
use prism::files_query::{Query, Search, Sort};
use std::{cell::Cell, rc::Rc};
#[derive(Default)]
struct Control {
    reads: Cell<usize>,
    writes: Cell<usize>,
    fail: Cell<Option<usize>>,
    corrupt: Cell<Option<u64>>,
}
struct Ram {
    sectors: Vec<[u8; 512]>,
    control: Rc<Control>,
}
impl BlockDevice for Ram {
    type Error = ();
    fn sector_count(&self) -> u64 {
        256
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
        let n = self.control.reads.get() + 1;
        self.control.reads.set(n);
        if self.control.fail.get() == Some(n) {
            return Err(());
        }
        *out = self.sectors[lba as usize];
        if self.control.corrupt.get() == Some(lba) {
            out[0] ^= 1;
        }
        Ok(())
    }
}
impl WritableBlockDevice for Ram {
    fn write_sector(&mut self, lba: u64, bytes: &[u8; 512]) -> Result<(), ()> {
        assert!((64..196).contains(&lba));
        self.control.writes.set(self.control.writes.get() + 1);
        self.sectors[lba as usize] = *bytes;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), ()> {
        self.control.writes.set(self.control.writes.get() + 1);
        Ok(())
    }
}
fn part(ram: &mut Ram) -> Partition<'_, Ram> {
    Partition::new(ram, 64, 132, &[(0, 64), (196, 60)]).unwrap()
}
fn setup() -> Ram {
    let mut ram = Ram {
        sectors: vec![[0xa5; 512]; 256],
        control: Rc::new(Control::default()),
    };
    let mut store = Store::format(part(&mut ram)).unwrap();
    for (path, bytes) in [
        ("/native/zebra", &b"long"[..]),
        ("/native/Alpha", &b"x"[..]),
        ("/native/alpha", &b"zz"[..]),
        ("/native/empty", &b""[..]),
        ("/trash/d.0000000000000001", &b"secret"[..]),
        ("/prefs/appearance", &b"secret"[..]),
    ] {
        store.put(path, bytes).unwrap();
    }
    let _ = store.into_partition();
    ram.control.reads.set(0);
    ram.control.writes.set(0);
    ram
}
fn active_bank(ram: &Ram) -> u64 {
    let slot = [64usize, 65]
        .into_iter()
        .max_by_key(|slot| u64::from_le_bytes(ram.sectors[*slot][16..24].try_into().unwrap()))
        .unwrap();
    assert_eq!(&ram.sectors[slot][..8], b"NXTCOW01");
    u32::from_le_bytes(ram.sectors[slot][24..28].try_into().unwrap()) as u64
}
fn list(store: &mut Store<'_, Ram>, q: Query) -> Result<files_query::Listing, journal::Error<()>> {
    files_query::collect(store, q, FileInfo::EMPTY, |info, _| Ok(info)).map(|(listing, entries)| {
        assert_eq!(listing.entries, entries);
        listing
    })
}
fn names(listing: files_query::Listing) -> Vec<String> {
    listing.entries[..listing.matched]
        .iter()
        .map(|f| f.path().to_owned())
        .collect()
}
#[test]
fn actual_sort_size_case_ties_direction_and_zero_write_full_media() {
    let mut ram = setup();
    let before = ram.sectors.clone();
    let control = ram.control.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    control.reads.set(0);
    let q = Query::new();
    let listing = list(&mut store, q).unwrap();
    assert_eq!(listing.total, 5);
    assert_eq!(listing.matched, 5);
    assert_eq!(
        names(listing),
        [
            "/native/Alpha",
            "/native/alpha",
            "/native/empty",
            "/native/zebra",
            "/prefs/appearance"
        ]
    );
    assert_eq!(control.reads.get(), 5 * 8);
    let mut q = q;
    q.sort = Sort::Size;
    assert_eq!(
        names(list(&mut store, q).unwrap()),
        [
            "/native/empty",
            "/native/Alpha",
            "/native/alpha",
            "/native/zebra",
            "/prefs/appearance"
        ]
    );
    q.descending = true;
    assert_eq!(
        names(list(&mut store, q).unwrap()),
        [
            "/prefs/appearance",
            "/native/zebra",
            "/native/alpha",
            "/native/Alpha",
            "/native/empty"
        ]
    );
    let _ = store.into_partition();
    assert_eq!(control.writes.get(), 0);
    assert_eq!(ram.sectors, before);
}
#[test]
fn explicit_content_search_scans_full_binary_payload_but_never_private_preferences() {
    let mut ram = setup();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut bytes = [0; 4096];
    bytes[4000..4006].copy_from_slice(b"SeCrEt");
    store.put("/native/full", &bytes).unwrap();
    let mut q = Query::new();
    q.set_text("secret").unwrap();
    assert_eq!(list(&mut store, q).unwrap().matched, 0);
    q.search = Search::PathAndContent;
    assert_eq!(names(list(&mut store, q).unwrap()), ["/native/full"]);
    q.set_text("TRASH").unwrap();
    assert_eq!(list(&mut store, q).unwrap().matched, 0);
    q.set_text("ALPHA").unwrap();
    assert_eq!(
        names(list(&mut store, q).unwrap()),
        ["/native/Alpha", "/native/alpha"]
    );
}
#[test]
fn read_only_empty_matches_and_exact_converter_results_are_sorted_together() {
    let mut ram = setup();
    let control = ram.control.clone();
    let before = ram.sectors.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    store.restrict_read_only();
    let mut q = Query::new();
    q.sort = Sort::Size;
    let (listing, values) = files_query::collect(&mut store, q, 0usize, |info, data| {
        assert_eq!(info.size as usize, data.len());
        Ok(data.len())
    })
    .unwrap();
    assert_eq!(&values[..listing.matched], &[0, 1, 2, 4, 6]);
    q.set_text("absent").unwrap();
    let l = list(&mut store, q).unwrap();
    assert_eq!(l.total, 5);
    assert_eq!(l.matched, 0);
    assert!(l.entries.iter().all(|info| *info == FileInfo::EMPTY));
    let _ = store.into_partition();
    assert_eq!(control.writes.get(), 0);
    assert_eq!(ram.sectors, before);
}
#[test]
fn every_payload_read_failure_fails_whole_query_without_writing() {
    for boundary in 1..=40 {
        let mut ram = setup();
        let before = ram.sectors.clone();
        let control = ram.control.clone();
        let mut store = Store::mount(part(&mut ram)).unwrap();
        control.reads.set(0);
        control.fail.set(Some(boundary));
        assert_eq!(
            list(&mut store, Query::new()),
            Err(journal::Error::Device(()))
        );
        assert!(store.is_read_only());
        let _ = store.into_partition();
        assert_eq!(control.writes.get(), 0);
        assert_eq!(ram.sectors, before);
    }
}
#[test]
fn corruption_in_unmatched_file_is_not_hidden_by_filter() {
    let mut ram = setup();
    let before = ram.sectors.clone();
    let control = ram.control.clone();
    let bank = active_bank(&ram);
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let mut metadata = [FileInfo::EMPTY; 8];
    store.list(&mut metadata).unwrap();
    let slot = metadata
        .iter()
        .position(|i| i.path() == "/native/zebra")
        .unwrap();
    let lba = 64 + 2 + bank * 65 + slot as u64 * 8;
    control.corrupt.set(Some(lba));
    let mut q = Query::new();
    q.set_text("Alpha").unwrap();
    assert!(list(&mut store, q).is_err());
    assert!(store.is_read_only());
    let _ = store.into_partition();
    assert_eq!(control.writes.get(), 0);
    assert_eq!(ram.sectors, before);
}
#[test]
fn converter_failure_never_returns_partial_listing() {
    let mut ram = setup();
    let control = ram.control.clone();
    let mut store = Store::mount(part(&mut ram)).unwrap();
    let result = files_query::collect(&mut store, Query::new(), 0u32, |info, _| {
        if info.path() == "/native/alpha" {
            Err(journal::Error::CorruptData)
        } else {
            Ok(info.size)
        }
    });
    assert_eq!(result, Err(journal::Error::CorruptData));
    assert_eq!(control.writes.get(), 0);
}
