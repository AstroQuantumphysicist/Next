//! Cross-component crash tests: actual Appearance codec/save and actual COW store.
#![allow(dead_code)]
#[path = "../crates/prism/src/appearance.rs"]
mod appearance;
#[path = "../crates/next-fs/src/journal.rs"]
mod journal;
use appearance::{Accent, Persistence, Preferences, TextScale, Theme};
use journal::{Partition, Store, WritableBlockDevice};
use std::collections::BTreeMap;
pub trait BlockDevice {
    type Error;
    fn sector_count(&self) -> u64;
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error>;
}
#[derive(Clone)]
struct Disk {
    durable: Vec<[u8; 512]>,
    pending: BTreeMap<u64, [u8; 512]>,
    boundary: usize,
    failure: Option<(usize, bool)>,
}
impl Disk {
    fn new() -> Self {
        Self {
            durable: vec![[0xa5; 512]; 200],
            pending: BTreeMap::new(),
            boundary: 0,
            failure: None,
        }
    }
    fn crash(&mut self) {
        self.pending.clear();
        self.failure = None;
        self.boundary = 0;
    }
    fn begin(&mut self) -> Result<(), ()> {
        self.boundary += 1;
        if self.failure == Some((self.boundary, false)) {
            Err(())
        } else {
            Ok(())
        }
    }
    fn end(&self) -> Result<(), ()> {
        if self.failure == Some((self.boundary, true)) {
            Err(())
        } else {
            Ok(())
        }
    }
    fn protected(&self) {
        assert!(self.durable[..16]
            .iter()
            .chain(&self.durable[148..])
            .all(|s| *s == [0xa5; 512]));
    }
}
impl BlockDevice for Disk {
    type Error = ();
    fn sector_count(&self) -> u64 {
        self.durable.len() as u64
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
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
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), ()> {
        assert!((16..148).contains(&lba));
        self.begin()?;
        self.pending.insert(lba, *data);
        self.end()
    }
    fn flush(&mut self) -> Result<(), ()> {
        self.begin()?;
        for (lba, data) in std::mem::take(&mut self.pending) {
            self.durable[lba as usize] = data;
        }
        self.end()
    }
}
fn partition(disk: &mut Disk) -> Partition<'_, Disk> {
    Partition::new(disk, 16, 132, &[(0, 16), (148, 52)]).unwrap()
}
struct Backend<'a>(Store<'a, Disk>);
impl Persistence for Backend<'_> {
    type Error = journal::Error<()>;
    fn read(&mut self, path: &str, out: &mut [u8; 32]) -> Result<Option<usize>, Self::Error> {
        match self.0.read_file(path, out) {
            Ok(n) => Ok(Some(n)),
            Err(journal::Error::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn replace_atomic(&mut self, path: &str, bytes: &[u8; 32]) -> Result<(), Self::Error> {
        self.0.put(path, bytes)
    }
}
fn proposed() -> Preferences {
    Preferences {
        theme: Theme::HighContrast,
        text_scale: TextScale::Percent150,
        accent: Accent::Teal,
    }
}
fn baseline() -> Disk {
    let mut disk = Disk::new();
    let mut store = Store::format(partition(&mut disk)).unwrap();
    store
        .put(appearance::PATH, &Preferences::default().encode())
        .unwrap();
    store
        .put("/native/editor", b"unsaved editor baseline")
        .unwrap();
    drop(store);
    disk.crash();
    disk
}
#[test]
fn preference_activation_and_reboot_at_every_real_write_flush_boundary() {
    let original = baseline();
    for after in [false, true] {
        for boundary in 1..=69 {
            let mut disk = original.clone();
            disk.failure = Some((boundary, after));
            let mut backend = Backend(Store::mount(partition(&mut disk)).unwrap());
            let mut active = Preferences::default();
            assert!(appearance::save(&mut backend, &mut active, proposed()).is_err());
            assert_eq!(
                active,
                Preferences::default(),
                "failed save activated settings at {boundary}"
            );
            assert!(backend.0.is_read_only());
            assert_eq!(
                backend.0.put("/native/editor", b"retry"),
                Err(journal::Error::Indeterminate)
            );
            drop(backend);
            disk.crash();
            let mut backend = Backend(Store::mount(partition(&mut disk)).unwrap());
            let loaded = appearance::load(&mut backend).unwrap().preferences;
            assert!(loaded == Preferences::default() || loaded == proposed());
            let mut editor = [0; 64];
            let len = backend.0.read_file("/native/editor", &mut editor).unwrap();
            assert_eq!(&editor[..len], b"unsaved editor baseline");
            drop(backend);
            disk.protected();
        }
    }
}
#[test]
fn successful_save_activates_only_after_durable_commit_and_survives_crash() {
    let mut disk = baseline();
    let mut backend = Backend(Store::mount(partition(&mut disk)).unwrap());
    let mut active = Preferences::default();
    appearance::save(&mut backend, &mut active, proposed()).unwrap();
    assert_eq!(active, proposed());
    drop(backend);
    assert_eq!(disk.boundary, 69);
    assert!(disk.pending.is_empty());
    disk.crash();
    let mut backend = Backend(Store::mount(partition(&mut disk)).unwrap());
    assert_eq!(
        appearance::load(&mut backend).unwrap().preferences,
        proposed()
    );
    drop(backend);
    disk.protected();
}
#[test]
fn corrupt_preferences_load_never_rewrites_or_formats_real_journal() {
    let mut disk = baseline();
    let mut store = Store::mount(partition(&mut disk)).unwrap();
    let mut corrupt = proposed().encode();
    corrupt[12] ^= 1;
    store.put(appearance::PATH, &corrupt).unwrap();
    drop(store);
    disk.crash();
    let before = disk.durable.clone();
    let mut backend = Backend(Store::mount(partition(&mut disk)).unwrap());
    assert!(matches!(
        appearance::load(&mut backend),
        Err(appearance::Error::Corrupt(
            appearance::DecodeError::Checksum
        ))
    ));
    drop(backend);
    assert_eq!(disk.boundary, 0);
    assert_eq!(disk.durable, before);
    assert!(disk.pending.is_empty());
}
#[test]
fn read_only_and_full_store_fail_without_writes_or_activating_preferences() {
    for readonly in [false, true] {
        let mut disk = Disk::new();
        let mut store = Store::format(partition(&mut disk)).unwrap();
        if !readonly {
            for slot in 0..8 {
                store.put(&format!("/native/{slot}"), b"occupied").unwrap();
            }
        }
        drop(store);
        disk.crash();
        let before = disk.durable.clone();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        if readonly {
            store.restrict_read_only();
        }
        let mut backend = Backend(store);
        let mut active = Preferences::default();
        assert!(appearance::save(&mut backend, &mut active, proposed()).is_err());
        assert_eq!(active, Preferences::default());
        drop(backend);
        assert_eq!(disk.boundary, 0);
        assert_eq!(disk.durable, before);
        assert!(disk.pending.is_empty());
        disk.protected();
    }
}
#[test]
fn missing_or_oversized_record_never_truncates_or_creates_preferences() {
    for oversized in [false, true] {
        let mut disk = Disk::new();
        let mut store = Store::format(partition(&mut disk)).unwrap();
        if oversized {
            let mut bytes = proposed().encode().to_vec();
            bytes.push(0);
            store.put(appearance::PATH, &bytes).unwrap();
        }
        drop(store);
        disk.crash();
        let before = disk.durable.clone();
        let mut backend = Backend(Store::mount(partition(&mut disk)).unwrap());
        let loaded = appearance::load(&mut backend);
        if oversized {
            assert_eq!(
                loaded,
                Err(appearance::Error::Storage(journal::Error::BufferTooSmall))
            );
        } else {
            assert_eq!(loaded.unwrap().origin, appearance::Origin::MissingDefault);
        }
        drop(backend);
        assert_eq!(disk.boundary, 0);
        assert_eq!(disk.durable, before);
        assert!(disk.pending.is_empty());
    }
}
