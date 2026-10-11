#![allow(dead_code)]
extern crate self as next_fs;
#[path = "../crates/next-fs/src/lib.rs"]
#[allow(unused_attributes)]
mod fat;
pub use fat::{BlockDevice, FsError, Volume};
#[path = "../crates/next-fs/src/gpt.rs"]
pub mod gpt;
#[path = "../crates/helix-kernel/src/volumes.rs"]
mod volumes;
mod storage {
    #[derive(Debug, PartialEq, Eq)]
    pub enum StorageError {
        SectorOutOfRange,
    }
    pub unsafe fn read_sector(_: u64, _: &mut [u8; 512]) -> Result<(), StorageError> {
        panic!("host tests must not invoke device I/O")
    }
}
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    rc::Rc,
};
#[derive(Debug, PartialEq, Eq)]
enum DeviceError {
    Bounds,
    Io,
}
struct State {
    file: File,
    reads: Vec<u64>,
    patches: BTreeMap<u64, [u8; 512]>,
    capacity: Cell<u64>,
    fail: Option<u64>,
}
#[derive(Clone)]
struct Disk(Rc<RefCell<State>>);
impl Disk {
    fn fixture() -> Self {
        let file = File::open("work/gpt-fixtures/gpt-fat.img").unwrap();
        let capacity = Cell::new(file.metadata().unwrap().len() / 512);
        Self(Rc::new(RefCell::new(State {
            file,
            reads: Vec::new(),
            patches: BTreeMap::new(),
            capacity,
            fail: None,
        })))
    }
    fn patch(&self, lba: u64, edit: impl FnOnce(&mut [u8; 512])) {
        let mut data = [0; 512];
        self.clone().read_sector(lba, &mut data).unwrap();
        edit(&mut data);
        self.0.borrow_mut().patches.insert(lba, data);
    }
}
impl BlockDevice for Disk {
    type Error = DeviceError;
    fn sector_count(&self) -> u64 {
        self.0.borrow().capacity.get()
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), DeviceError> {
        let mut state = self.0.borrow_mut();
        state.reads.push(lba);
        if lba >= state.capacity.get() {
            return Err(DeviceError::Bounds);
        }
        if state.fail == Some(lba) {
            return Err(DeviceError::Io);
        }
        if let Some(patch) = state.patches.get(&lba) {
            out.copy_from_slice(patch);
            return Ok(());
        }
        state
            .file
            .seek(SeekFrom::Start(lba * 512))
            .map_err(|_| DeviceError::Io)?;
        state.file.read_exact(out).map_err(|_| DeviceError::Io)
    }
}
fn layout(disk: &Disk) -> gpt::NextLayout {
    struct Reader(Disk);
    impl gpt::BlockReader for Reader {
        type Error = DeviceError;
        fn block_size(&self) -> usize {
            512
        }
        fn block_count(&self) -> u64 {
            self.0.sector_count()
        }
        fn read_block(&mut self, lba: u64, out: &mut [u8]) -> Result<(), DeviceError> {
            self.0.read_sector(lba, out.try_into().unwrap())
        }
    }
    gpt::discover(&mut Reader(disk.clone()))
        .unwrap()
        .next_layout::<DeviceError>()
        .unwrap()
}

#[test]
fn actual_planner_gpt_reads_fat_note_at_esp_offset_only() {
    let disk = Disk::fixture();
    let expected = layout(&disk);
    disk.0.borrow_mut().reads.clear();
    let mut out = [0; 128];
    let info = volumes::discover_and_read(disk.clone(), &mut out).unwrap();
    assert_eq!(&out[..info.note_bytes], b"NEXT GPT readonly ESP fixture\n");
    assert_eq!(info.esp, expected.esp);
    assert_eq!(info.data, expected.data);
    assert!(info.esp.first_lba > 0);
    let reads = &disk.0.borrow().reads;
    // After bounded GPT metadata reads, every FAT request is ESP-relative.
    let fat_start = reads
        .iter()
        .position(|lba| *lba == expected.esp.first_lba)
        .unwrap();
    assert!(reads[fat_start..]
        .iter()
        .all(|lba| *lba >= expected.esp.first_lba && *lba <= expected.esp.last_lba));
    for relative in [0, 32, 1056, 1057] {
        assert!(reads[fat_start..].contains(&(expected.esp.first_lba + relative)));
    }
    assert!(!reads
        .iter()
        .any(|lba| *lba >= expected.data.first_lba && *lba <= expected.data.last_lba));
    assert!(reads.len() <= 80);
}

#[test]
fn extent_rejects_overflow_zero_escape_and_changed_capacity_before_backend() {
    let disk = Disk::fixture();
    for (start, count) in [(0, 0), (u64::MAX, 2), (disk.sector_count(), 1)] {
        assert!(matches!(
            volumes::ReadExtent::new(disk.clone(), start, count),
            Err(volumes::ReadError::Bounds)
        ));
    }
    let mut extent = volumes::ReadExtent::new(disk.clone(), 40, 2).unwrap();
    assert_eq!(extent.sector_count(), 2);
    let mut out = [0; 512];
    extent.read_sector(1, &mut out).unwrap();
    assert_eq!(disk.0.borrow().reads, vec![41]);
    for lba in [2, u64::MAX] {
        assert_eq!(
            extent.read_sector(lba, &mut out),
            Err(volumes::ReadError::Bounds)
        );
    }
    disk.0.borrow().capacity.set(40);
    assert_eq!(
        extent.read_sector(0, &mut out),
        Err(volumes::ReadError::Bounds)
    );
    assert_eq!(disk.0.borrow().reads, vec![41]);
}

#[test]
fn corrupt_gpt_never_falls_back_to_fat_or_reads_data() {
    let disk = Disk::fixture();
    disk.patch(1, |sector| sector[16] ^= 1);
    disk.0.borrow_mut().reads.clear();
    assert!(matches!(
        volumes::discover_and_read(disk.clone(), &mut [0; 128]),
        Err(volumes::DiscoveryError::Gpt(gpt::Error::HeaderCrc))
    ));
    assert!(disk
        .0
        .borrow()
        .reads
        .iter()
        .all(|lba| *lba <= 1 || *lba == disk.sector_count() - 1));
}

#[test]
fn esp_bpb_cannot_claim_adjacent_data_and_read_errors_remain_errors() {
    let disk = Disk::fixture();
    let esp = layout(&disk).esp;
    disk.patch(esp.first_lba, |sector| {
        sector[32..36].copy_from_slice(&((esp.block_count() + 1) as u32).to_le_bytes())
    });
    assert!(matches!(
        volumes::discover_and_read(disk.clone(), &mut [0; 128]),
        Err(volumes::DiscoveryError::Fat(FsError::InvalidGeometry))
    ));
    let disk = Disk::fixture();
    disk.0.borrow_mut().fail = Some(esp.first_lba + 1057);
    assert!(matches!(
        volumes::discover_and_read(disk, &mut [0; 128]),
        Err(volumes::DiscoveryError::Fat(FsError::Device(
            volumes::ReadError::Device(DeviceError::Io)
        )))
    ));
}

#[test]
fn missing_note_and_small_output_fail_without_invented_content() {
    let disk = Disk::fixture();
    assert!(matches!(
        volumes::discover_and_read(disk.clone(), &mut [0; 2]),
        Err(volumes::DiscoveryError::Fat(FsError::BufferTooSmall))
    ));
    let esp = layout(&disk).esp;
    disk.patch(esp.first_lba + 1056, |sector| sector[0] = 0);
    assert!(matches!(
        volumes::discover_and_read(disk, &mut [0; 128]),
        Err(volumes::DiscoveryError::Fat(FsError::NotFound))
    ));
}
