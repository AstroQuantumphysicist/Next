#![allow(dead_code)]
use gpt::BlockReader;
use next_fs::gpt;
#[path = "../crates/helix-kernel/src/storage_policy.rs"]
mod authenticated;
#[path = "../crates/next-fs/src/storage_policy.rs"]
mod schema;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
fn root() -> PathBuf {
    PathBuf::from(env!("NEXT_SOURCE_ROOT"))
}
struct Disk(Vec<u8>);
impl gpt::BlockReader for Disk {
    type Error = ();
    fn block_size(&self) -> usize {
        512
    }
    fn block_count(&self) -> u64 {
        self.0.len() as u64 / 512
    }
    fn read_block(&mut self, lba: u64, out: &mut [u8]) -> Result<(), ()> {
        let at = lba as usize * 512;
        out.copy_from_slice(self.0.get(at..at + 512).ok_or(())?);
        Ok(())
    }
}
fn fixture() -> (gpt::Gpt, u64, [u8; 80]) {
    let mut disk = Disk(fs::read(root().join("work/gpt-fixtures/gpt-512.img")).unwrap());
    let capacity = disk.block_count();
    let table = gpt::discover(&mut disk).unwrap();
    let data = table.next_layout::<()>().unwrap().data;
    let mut bytes = [0; 80];
    bytes[..8].copy_from_slice(b"NXTSTOR1");
    bytes[8..12].copy_from_slice(&[1, 0, 80, 0]);
    bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
    bytes[16..32].copy_from_slice(&table.disk_guid.0);
    bytes[32..48].copy_from_slice(&data.unique_guid.0);
    bytes[48..56].copy_from_slice(&data.first_lba.to_le_bytes());
    bytes[56..64].copy_from_slice(&data.block_count().to_le_bytes());
    bytes[64..72].copy_from_slice(&capacity.to_le_bytes());
    bytes[72..76].copy_from_slice(&512u32.to_le_bytes());
    (table, capacity, bytes)
}
#[test]
fn exact_gpt_binding_confines_journal_and_attenuates_read_only() {
    let (table, capacity, bytes) = fixture();
    let data = table.next_layout::<()>().unwrap().data;
    let policy = schema::Policy::parse(&bytes).unwrap();
    let extent = policy.bind(table.disk_guid, data, capacity).unwrap();
    assert_eq!(extent.sectors, next_fs::journal::REQUIRED_SECTORS);
    assert_eq!(extent.start, data.first_lba);
    assert!(extent.writable);
    assert!(extent.start + extent.sectors <= data.last_lba + 1);
    let mut ro = data;
    ro.attributes |= 1 << 60;
    assert!(!policy.bind(table.disk_guid, ro, capacity).unwrap().writable);
    ro.attributes |= 2;
    assert_eq!(
        policy.bind(table.disk_guid, ro, capacity),
        Err(schema::Error::Attributes)
    );
    let mut wrong = data;
    wrong.type_guid = gpt::ESP;
    assert_eq!(
        policy.bind(table.disk_guid, wrong, capacity),
        Err(schema::Error::Binding)
    );
    wrong = data;
    wrong.last_lba -= 1;
    assert_eq!(
        policy.bind(table.disk_guid, wrong, capacity),
        Err(schema::Error::Binding)
    );
    assert_eq!(
        policy.bind(gpt::Guid([9; 16]), data, capacity),
        Err(schema::Error::Binding)
    );
    assert_eq!(
        policy.bind(table.disk_guid, data, capacity + 1),
        Err(schema::Error::Binding)
    );
}
#[test]
fn malformed_schema_unknown_bits_zero_ids_geometry_and_trailing_data_reject() {
    let (_, _, bytes) = fixture();
    for length in 0..80 {
        assert_eq!(
            schema::Policy::parse(&bytes[..length]),
            Err(schema::Error::Size)
        );
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert_eq!(schema::Policy::parse(&trailing), Err(schema::Error::Size));
    for offset in [0, 8, 10, 76] {
        let mut bad = bytes;
        bad[offset] ^= 1;
        assert!(schema::Policy::parse(&bad).is_err());
    }
    let mut bad = bytes;
    bad[12] = 2;
    assert_eq!(schema::Policy::parse(&bad), Err(schema::Error::Flags));
    for start in [16, 32] {
        let mut bad = bytes;
        bad[start..start + 16].fill(0);
        assert_eq!(schema::Policy::parse(&bad), Err(schema::Error::Guid));
    }
    for (at, value) in [(48, u64::MAX), (56, 131), (64, 0)] {
        let mut bad = bytes;
        bad[at..at + 8].copy_from_slice(&value.to_le_bytes());
        assert_eq!(schema::Policy::parse(&bad), Err(schema::Error::Geometry));
    }
    let mut bad = bytes;
    bad[72..76].copy_from_slice(&4096u32.to_le_bytes());
    assert_eq!(schema::Policy::parse(&bad), Err(schema::Error::Geometry));
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let path = root().join(format!(
            "work/storage-policy-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn pack(&self, bytes: &[u8], id: &str, seed: &Path) -> Vec<u8> {
        let member = self.0.join("storage.bin");
        fs::write(&member, bytes).unwrap();
        let destination = self.0.join("app.nxp");
        let result = Command::new(root().join("target/debug/next-package"))
            .arg("pack")
            .arg(seed)
            .arg(id)
            .args(["0.1.0", "3f", "app", "bin/nextd.elf"])
            .arg(&destination)
            .arg(format!(
                "bin/nextd.elf={}",
                root().join("build/user/nextd.elf").display()
            ))
            .arg(format!("config/storage.bin={}", member.display()))
            .output()
            .unwrap();
        assert!(result.status.success(), "package tool failed");
        fs::read(destination).unwrap()
    }
    fn archive(&self, bundle: &[u8]) -> Vec<u8> {
        let member = self.0.join("input.nxp");
        fs::write(&member, bundle).unwrap();
        let destination = self.0.join("initrd.nxi");
        let result=Command::new("python3").arg("-c")
            .arg("import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); import initrd; Path(sys.argv[3]).write_bytes(initrd.pack([('/system/nextd.elf',Path(sys.argv[2]).read_bytes())]))")
            .arg(root().join("tools")).arg(member).arg(&destination).output().unwrap();
        assert!(result.status.success());
        fs::read(destination).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn actual_signed_member_loads_and_crc_or_tampered_signature_never_grants() {
    let scratch = Scratch::new();
    let (table, capacity, bytes) = fixture();
    let data = table.next_layout::<()>().unwrap().data;
    let mut bundle = scratch.pack(
        &bytes,
        "org.next.development.native.nextd",
        &root().join("work/package-dev.seed"),
    );
    let archive = scratch.archive(&bundle);
    let grant = authenticated::AuthenticatedPolicy::load(&archive)
        .unwrap()
        .bind(table.disk_guid, data, capacity)
        .unwrap();
    assert_eq!(grant.start(), data.first_lba);
    assert_eq!(grant.sectors(), 132);
    assert!(grant.writable());
    let offset = bundle
        .windows(bytes.len())
        .position(|window| window == bytes)
        .unwrap();
    let mut tampered_member = bundle.clone();
    tampered_member[offset + 12] ^= 1;
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&tampered_member)),
        Err(authenticated::Error::Package(next_package::Error::Digest))
    ));
    let mut ro_bytes = bytes;
    ro_bytes[12..16].fill(0);
    let ro_bundle = scratch.pack(
        &ro_bytes,
        "org.next.development.native.nextd",
        &root().join("work/package-dev.seed"),
    );
    let ro_grant = authenticated::AuthenticatedPolicy::load(&scratch.archive(&ro_bundle))
        .unwrap()
        .bind(table.disk_guid, data, capacity)
        .unwrap();
    assert!(!ro_grant.writable());
    let mut ro_data = data;
    ro_data.attributes |= 1 << 60;
    let ro_grant = authenticated::AuthenticatedPolicy::load(&archive)
        .unwrap()
        .bind(table.disk_guid, ro_data, capacity)
        .unwrap();
    assert!(!ro_grant.writable());
    let last = bundle.len() - 64;
    bundle[last] ^= 1;
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&bundle)),
        Err(authenticated::Error::Package(_))
    ));
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&bytes)),
        Err(authenticated::Error::Package(_))
    ));
    let wrong = scratch.pack(
        &bytes,
        "org.next.development.native.probe",
        &root().join("work/package-dev.seed"),
    );
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&wrong)),
        Err(authenticated::Error::Supervisor)
    ));
}
#[test]
fn unknown_signer_missing_policy_and_signed_malformed_metadata_fail_closed() {
    let scratch = Scratch::new();
    let (_, _, bytes) = fixture();
    let seed = scratch.0.join("other.seed");
    let public = scratch.0.join("other.pub");
    let result = Command::new(root().join("target/debug/next-package"))
        .arg("keygen")
        .arg(&seed)
        .arg(public)
        .output()
        .unwrap();
    assert!(result.status.success());
    let bundle = scratch.pack(&bytes, "org.next.development.native.nextd", &seed);
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&bundle)),
        Err(authenticated::Error::Package(
            next_package::Error::UnknownSigner
        ))
    ));
    let normal = fs::read(root().join("build/apps/system/nextd.elf")).unwrap();
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&normal)),
        Err(authenticated::Error::Missing)
    ));
    let mut bad = bytes;
    bad[12] = 0x80;
    let bundle = scratch.pack(
        &bad,
        "org.next.development.native.nextd",
        &root().join("work/package-dev.seed"),
    );
    assert!(matches!(
        authenticated::AuthenticatedPolicy::load(&scratch.archive(&bundle)),
        Err(authenticated::Error::Policy(_))
    ));
}

struct Memory {
    sectors: Vec<[u8; 512]>,
    writes: Vec<u64>,
    flushes: usize,
}
impl next_fs::BlockDevice for Memory {
    type Error = ();
    fn sector_count(&self) -> u64 {
        self.sectors.len() as u64
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
        out.copy_from_slice(self.sectors.get(lba as usize).ok_or(())?);
        Ok(())
    }
}
impl next_fs::journal::WritableBlockDevice for Memory {
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), ()> {
        self.sectors
            .get_mut(lba as usize)
            .ok_or(())?
            .copy_from_slice(data);
        self.writes.push(lba);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), ()> {
        self.flushes += 1;
        Ok(())
    }
}
#[test]
fn actual_authenticated_mount_never_formats_unknown_data_and_confines_commits() {
    use next_fs::journal::{Error as JournalError, Partition, Store};
    let scratch = Scratch::new();
    let (table, capacity, bytes) = fixture();
    let data = table.next_layout::<()>().unwrap().data;
    let bundle = scratch.pack(
        &bytes,
        "org.next.development.native.nextd",
        &root().join("work/package-dev.seed"),
    );
    let archive = scratch.archive(&bundle);
    let grant = || {
        authenticated::AuthenticatedPolicy::load(&archive)
            .unwrap()
            .bind(table.disk_guid, data, capacity)
            .unwrap()
    };
    let source = fs::read(root().join("work/gpt-fixtures/gpt-512.img")).unwrap();
    let mut memory = Memory {
        sectors: source
            .chunks_exact(512)
            .map(|b| b.try_into().unwrap())
            .collect(),
        writes: Vec::new(),
        flushes: 0,
    };
    let before = memory.sectors.clone();
    assert!(matches!(
        authenticated::mount_existing(&mut memory, &grant()),
        Err(JournalError::InvalidFormat)
    ));
    assert_eq!(memory.sectors, before);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.flushes, 0);
    // Fixture creation is explicit host-side formatting of owned RAM, separate
    // from the exact authenticated kernel mount function being exercised.
    let partition =
        Partition::new(&mut memory, data.first_lba, 132, &[(0, data.first_lba)]).unwrap();
    let formatted = Store::format(partition).unwrap();
    drop(formatted);
    memory.writes.clear();
    memory.flushes = 0;
    let mut mounted = authenticated::mount_existing(&mut memory, &grant()).unwrap();
    assert!(!mounted.is_read_only());
    assert_eq!(mounted.generation(), 1);
    mounted
        .put("/policy.txt", b"owner-authorized commit")
        .unwrap();
    drop(mounted);
    assert!(memory
        .writes
        .iter()
        .all(|lba| *lba >= data.first_lba && *lba < data.first_lba + 132));
    assert!(memory.flushes > 0);
    let start = data.first_lba as usize;
    let end = start + 132;
    assert_eq!(memory.sectors[..start], before[..start]);
    assert_eq!(memory.sectors[end..], before[end..]);
    let mut ro_bytes = bytes;
    ro_bytes[12..16].fill(0);
    let ro_bundle = scratch.pack(
        &ro_bytes,
        "org.next.development.native.nextd",
        &root().join("work/package-dev.seed"),
    );
    let ro_archive = scratch.archive(&ro_bundle);
    let ro_grant = authenticated::AuthenticatedPolicy::load(&ro_archive)
        .unwrap()
        .bind(table.disk_guid, data, capacity)
        .unwrap();
    memory.writes.clear();
    memory.flushes = 0;
    let committed = memory.sectors.clone();
    let mut ro = authenticated::mount_existing(&mut memory, &ro_grant).unwrap();
    assert!(ro.is_read_only());
    let mut out = [0; 64];
    let len = ro.read_file("/policy.txt", &mut out).unwrap();
    assert_eq!(&out[..len], b"owner-authorized commit");
    assert_eq!(
        ro.put("/policy.txt", b"denied"),
        Err(JournalError::ReadOnly)
    );
    drop(ro);
    assert_eq!(memory.sectors, committed);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.flushes, 0);
    memory.sectors[start].fill(0);
    memory.sectors[start + 1].fill(0);
    let corrupt = memory.sectors.clone();
    assert!(matches!(
        authenticated::mount_existing(&mut memory, &grant()),
        Err(JournalError::InvalidFormat)
    ));
    assert_eq!(memory.sectors, corrupt);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.flushes, 0);
}
