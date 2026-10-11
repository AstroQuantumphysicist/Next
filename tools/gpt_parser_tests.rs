#![allow(dead_code)]
#[path = "../crates/next-fs/src/gpt.rs"]
mod gpt;
use gpt::{BlockReader, Error, Guid};

#[test]
fn caller_owned_scratch_reuse_exposes_no_stale_or_partial_partitions_on_error() {
    let mut disk = Disk::fixture(512);
    let mut scratch = gpt::Gpt::empty();
    let mut workspace = gpt::Workspace::new();
    gpt::discover_with_workspace(&mut disk, &mut scratch, &mut workspace).unwrap();
    let expected = scratch.next_layout::<()>().unwrap();
    assert!(!scratch.partitions().is_empty());
    disk.data[512 + 16] ^= 1;
    assert_eq!(
        gpt::discover_with_workspace(&mut disk, &mut scratch, &mut workspace),
        Err(Error::HeaderCrc)
    );
    assert!(scratch.partitions().is_empty());
    assert_eq!(scratch.disk_guid, Guid::ZERO);
    assert_eq!(scratch.next_layout::<()>(), Err(Error::NotFound));
    disk.data[512 + 16] ^= 1;
    gpt::discover_with_workspace(&mut disk, &mut scratch, &mut workspace).unwrap();
    assert_eq!(scratch.next_layout::<()>().unwrap(), expected);
    disk.update_tables(|entries| entries[3 * 128 + 16..3 * 128 + 32].fill(0));
    assert_eq!(
        gpt::discover_with_workspace(&mut disk, &mut scratch, &mut workspace),
        Err(Error::Guid)
    );
    assert!(scratch.partitions().is_empty());
}
struct Disk {
    data: Vec<u8>,
    size: usize,
    reads: usize,
    unreadable: Option<u64>,
}
impl Disk {
    fn fixture(size: usize) -> Self {
        Self {
            data: std::fs::read(format!("work/gpt-fixtures/gpt-{size}.img")).unwrap(),
            size,
            reads: 0,
            unreadable: None,
        }
    }
    fn seal_header(&mut self, lba: u64) {
        let at = lba as usize * self.size;
        self.data[at + 16..at + 20].fill(0);
        let crc = crc(&self.data[at..at + 92]);
        self.data[at + 16..at + 20].copy_from_slice(&crc.to_le_bytes());
    }
    fn update_tables(&mut self, edit: impl FnOnce(&mut [u8])) {
        let start = 2 * self.size;
        let end = start + 16384;
        edit(&mut self.data[start..end]);
        let copy = self.data[start..end].to_vec();
        let backup = self.data.len() - self.size - 16384;
        self.data[backup..backup + 16384].copy_from_slice(&copy);
        let crc = crc(&copy);
        let last = self.block_count() - 1;
        for lba in [1, last] {
            let at = lba as usize * self.size;
            self.data[at + 88..at + 92].copy_from_slice(&crc.to_le_bytes());
            self.seal_header(lba);
        }
    }
}
impl BlockReader for Disk {
    type Error = ();
    fn block_size(&self) -> usize {
        self.size
    }
    fn block_count(&self) -> u64 {
        (self.data.len() / self.size) as u64
    }
    fn read_block(&mut self, lba: u64, out: &mut [u8]) -> Result<(), ()> {
        self.reads += 1;
        if self.unreadable == Some(lba) {
            return Err(());
        }
        assert_eq!(out.len(), self.size);
        let start = lba as usize * self.size;
        let bytes = self.data.get(start..start + self.size).ok_or(())?;
        out.copy_from_slice(bytes);
        Ok(())
    }
}
fn crc(data: &[u8]) -> u32 {
    let mut value = !0u32;
    for byte in data {
        value ^= u32::from(*byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
        }
    }
    !value
}
#[test]
fn actual_planner_both_logical_block_sizes_and_typed_layout_are_read_only() {
    assert_eq!(crc(b"123456789"), 0xcbf43926);
    for size in [512, 4096] {
        let mut disk = Disk::fixture(size);
        let before = disk.data.clone();
        let table = gpt::discover(&mut disk).unwrap();
        assert_eq!(
            table.disk_guid,
            Guid([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 100])
        );
        assert_eq!(table.partitions().len(), 5);
        let layout = table.next_layout::<()>().unwrap();
        assert_eq!(layout.esp.kind(), gpt::Kind::Esp);
        assert_eq!(layout.data.kind(), gpt::Kind::Data);
        assert_eq!(layout.recovery.unwrap().kind(), gpt::Kind::Recovery);
        assert_eq!(layout.esp.first_lba, if size == 512 { 40 } else { 6 });
        assert_eq!(layout.esp.block_count(), 65536 / size as u64);
        assert_eq!(layout.data.first_lba, if size == 512 { 424 } else { 54 });
        assert_eq!(
            table
                .partitions()
                .iter()
                .filter(|p| p.kind() == gpt::Kind::System)
                .count(),
            2
        );
        assert_eq!(
            layout.esp.name_utf16(),
            "Next ESP".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(disk.data, before);
        assert!(disk.reads <= 67);
    }
}
#[test]
fn names_do_not_choose_types_and_unknown_partitions_are_preserved() {
    let mut disk = Disk::fixture(512);
    disk.update_tables(|table| {
        table[56..128].fill(0);
        for (n, unit) in "Next Data".encode_utf16().enumerate() {
            table[56 + n * 2..58 + n * 2].copy_from_slice(&unit.to_le_bytes());
        }
    });
    let table = gpt::discover(&mut disk).unwrap();
    assert_eq!(table.next_layout::<()>().unwrap().esp.first_lba, 40);
    disk.update_tables(|table| table[4 * 128..4 * 128 + 16].copy_from_slice(&[0x77; 16]));
    let table = gpt::discover(&mut disk).unwrap();
    assert_eq!(table.partitions()[4].kind(), gpt::Kind::Unknown);
    assert_eq!(table.next_layout::<()>().unwrap().recovery, None);
}
#[test]
fn protective_mbr_hybrid_and_primary_backup_crc_damage_fail_closed() {
    for offset in [
        510,
        450,
        462,
        512 + 16,
        512 + 8,
        4096 * 512 - 512 + 16,
        1024,
        4096 * 512 - 512 - 16384,
    ] {
        let mut disk = Disk::fixture(512);
        disk.data[offset] ^= 1;
        assert!(gpt::discover(&mut disk).is_err());
        assert!(disk.reads <= 67);
    }
}
#[test]
fn valid_crc_header_geometry_table_pointers_dimensions_and_copy_disagreement_reject() {
    for (offset, bytes) in [
        (24, 0u64.to_le_bytes()),
        (32, 0u64.to_le_bytes()),
        (40, 0u64.to_le_bytes()),
        (48, u64::MAX.to_le_bytes()),
        (72, u64::MAX.to_le_bytes()),
    ] {
        let mut disk = Disk::fixture(512);
        disk.data[512 + offset..512 + offset + 8].copy_from_slice(&bytes);
        disk.seal_header(1);
        assert!(gpt::discover(&mut disk).is_err());
    }
    for count in [0u32, 129, u32::MAX] {
        let mut disk = Disk::fixture(512);
        disk.data[592..596].copy_from_slice(&count.to_le_bytes());
        disk.seal_header(1);
        assert_eq!(gpt::discover(&mut disk).err(), Some(Error::EntryDimensions));
    }
    let mut disk = Disk::fixture(512);
    let last = disk.block_count() - 1;
    let at = last as usize * 512;
    disk.data[at + 56] ^= 1;
    disk.seal_header(last);
    assert_eq!(gpt::discover(&mut disk).err(), Some(Error::CopiesDisagree));
}
#[test]
fn partition_overlap_bounds_guid_duplicates_attributes_and_utf16_are_checked_after_crc() {
    let mutations: [fn(&mut [u8]); 7] = [
        |t| {
            let a = t[32..48].to_vec();
            t[128 + 32..128 + 48].copy_from_slice(&a);
        },
        |t| t[32..40].copy_from_slice(&0u64.to_le_bytes()),
        |t| t[16..32].fill(0),
        |t| {
            let id = t[16..32].to_vec();
            t[128 + 16..128 + 32].copy_from_slice(&id);
        },
        |t| t[48..56].copy_from_slice(&8u64.to_le_bytes()),
        |t| t[56..58].copy_from_slice(&0xd800u16.to_le_bytes()),
        |t| {
            t[0..16].fill(0);
        },
    ];
    for mutate in mutations {
        let mut disk = Disk::fixture(512);
        disk.update_tables(mutate);
        assert!(gpt::discover(&mut disk).is_err());
    }
}
#[test]
fn utf16_surrogate_pairs_full_names_and_noncanonical_padding_have_defined_policy() {
    let mut disk = Disk::fixture(512);
    disk.update_tables(|t| {
        t[56..128].fill(0);
        t[56..58].copy_from_slice(&0xd83du16.to_le_bytes());
        t[58..60].copy_from_slice(&0xde80u16.to_le_bytes());
    });
    let table = gpt::discover(&mut disk).unwrap();
    assert_eq!(table.partitions()[0].name_utf16(), &[0xd83d, 0xde80]);
    disk.update_tables(|t| {
        for raw in t[56..128].chunks_exact_mut(2) {
            raw.copy_from_slice(&65u16.to_le_bytes());
        }
    });
    assert_eq!(
        gpt::discover(&mut disk).unwrap().partitions()[0]
            .name_utf16()
            .len(),
        36
    );
    disk.update_tables(|t| {
        t[56..58].fill(0);
    });
    assert_eq!(gpt::discover(&mut disk).err(), Some(Error::Name));
}
#[test]
fn ambiguous_esp_data_and_no_block_io_never_select_an_arbitrary_partition() {
    let mut disk = Disk::fixture(512);
    disk.update_tables(|t| t[128..144].copy_from_slice(&gpt::ESP.0));
    let table = gpt::discover(&mut disk).unwrap();
    assert_eq!(table.next_layout::<()>().err(), Some(Error::Ambiguous));
    let mut disk = Disk::fixture(512);
    disk.update_tables(|t| t[3 * 128 + 48..3 * 128 + 56].copy_from_slice(&2u64.to_le_bytes()));
    let table = gpt::discover(&mut disk).unwrap();
    assert_eq!(table.next_layout::<()>().err(), Some(Error::Attributes));
}
#[test]
fn media_truncation_device_errors_and_unknown_logical_sizes_are_explicit() {
    let mut disk = Disk::fixture(512);
    disk.data.truncate(4 * 512);
    assert_eq!(gpt::discover(&mut disk).err(), Some(Error::Geometry));
    let mut disk = Disk::fixture(512);
    disk.unreadable = Some(1);
    assert_eq!(gpt::discover(&mut disk).err(), Some(Error::Device(())));
    let mut disk = Disk::fixture(512);
    disk.size = 1024;
    assert_eq!(gpt::discover(&mut disk).err(), Some(Error::Geometry));
}
#[test]
fn hostile_seeded_metadata_bytes_never_panic_or_trigger_unbounded_reads() {
    let original = Disk::fixture(512).data;
    let mut state = 0x76543210u64;
    for _ in 0..256 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut disk = Disk {
            data: original.clone(),
            size: 512,
            reads: 0,
            unreadable: None,
        };
        let offset = state as usize % 17408;
        disk.data[offset] ^= (state >> 40) as u8 | 1;
        let _ = gpt::discover(&mut disk);
        assert!(disk.reads <= 67);
    }
}
