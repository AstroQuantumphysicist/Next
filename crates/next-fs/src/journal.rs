//! Bounded development COW store, not the complete NextFS format or POSIX VFS.
//!
//! Two 512-byte CRC-protected commit superblocks point to two fixed snapshot
//! banks. Each bank contains eight 4 KiB file slots and one metadata sector.
//! A transaction writes the inactive data bank, flushes, writes its metadata
//! root, flushes, writes the alternating commit superblock, and flushes again.
//! Only a valid superblock with a matching generation/root/data checksum is
//! visible after mount. An incomplete transaction cannot overwrite the active
//! bank. The device must honor successful flush barriers; CRC detects accidental
//! corruption, not malicious modification, and dishonest hardware is not covered.
//!
//! All integer fields are little endian. Metadata CRC occupies bytes 508..512
//! and covers bytes 0..508. Unknown versions, reserved bytes and geometry fail
//! closed. Corruption recovery may select the previous snapshot read-only.
use crate::BlockDevice;
#[path = "journal/transaction.rs"]
pub mod transaction;

/// Checksummed, bounded explicit directories over the COW journal namespace.
#[path = "journal/directories.rs"]
pub mod directories;

pub const MAX_FILES: usize = 8;
pub const MAX_FILE_BYTES: usize = 4096;
const DATA_SECTORS: u64 = (MAX_FILES * MAX_FILE_BYTES / 512) as u64;
const BANK_SECTORS: u64 = DATA_SECTORS + 1;
pub const REQUIRED_SECTORS: u64 = 2 + 2 * BANK_SECTORS;
const SUPER_MAGIC: &[u8; 8] = b"NXTCOW01";
const ROOT_MAGIC: &[u8; 8] = b"NXTRT001";

/// Successful flush must propagate all preceding writes to durable media as
/// far as the compliant device/controller permits. Unsupported flush errors.
pub trait WritableBlockDevice: BlockDevice {
    fn write_sector(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Device(E),
    Geometry,
    ProtectedRange,
    InvalidFormat,
    CorruptMetadata,
    CorruptData,
    InvalidPath,
    NotFound,
    Exists,
    Full,
    TooLarge,
    BufferTooSmall,
    ReadOnly,
    Indeterminate,
    GenerationExhausted,
}

/// Exclusive writable extent. The caller supplies trusted partition/protection
/// metadata; this type verifies the extent but does not discover GPT or grant
/// authority. Keeping the device mutably borrowed prevents simultaneous writers.
pub struct Partition<'a, D: WritableBlockDevice> {
    device: &'a mut D,
    start: u64,
    sectors: u64,
}
impl<'a, D: WritableBlockDevice> Partition<'a, D> {
    pub fn new(
        device: &'a mut D,
        start: u64,
        sectors: u64,
        protected_ranges: &[(u64, u64)],
    ) -> Result<Self, Error<D::Error>> {
        let end = start.checked_add(sectors).ok_or(Error::Geometry)?;
        if sectors < REQUIRED_SECTORS || end > device.sector_count() {
            return Err(Error::Geometry);
        }
        for &(protected_start, protected_sectors) in protected_ranges {
            let protected_end = protected_start
                .checked_add(protected_sectors)
                .ok_or(Error::Geometry)?;
            if protected_sectors != 0 && start < protected_end && protected_start < end {
                return Err(Error::ProtectedRange);
            }
        }
        Ok(Self {
            device,
            start,
            sectors,
        })
    }
    pub fn start_lba(&self) -> u64 {
        self.start
    }
    pub fn sector_count(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, relative: u64, data: &mut [u8; 512]) -> Result<(), Error<D::Error>> {
        if relative >= self.sectors {
            return Err(Error::Geometry);
        }
        self.device
            .read_sector(self.start + relative, data)
            .map_err(Error::Device)
    }
    fn write(&mut self, relative: u64, data: &[u8; 512]) -> Result<(), Error<D::Error>> {
        if relative >= self.sectors {
            return Err(Error::Geometry);
        }
        self.device
            .write_sector(self.start + relative, data)
            .map_err(Error::Device)
    }
    fn flush(&mut self) -> Result<(), Error<D::Error>> {
        self.device.flush().map_err(Error::Device)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub name: [u8; 32],
    pub size: u32,
}
impl FileInfo {
    pub const EMPTY: Self = Self {
        name: [0; 32],
        size: 0,
    };
    pub fn path(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    info: FileInfo,
    crc: u32,
}
impl Entry {
    const EMPTY: Self = Self {
        info: FileInfo::EMPTY,
        crc: 0,
    };
    fn used(self) -> bool {
        self.info.name[0] != 0
    }
}
#[derive(Clone, Copy)]
struct Commit {
    generation: u64,
    bank: u32,
    root_crc: u32,
}

pub struct Store<'a, D: WritableBlockDevice> {
    partition: Partition<'a, D>,
    entries: [Entry; MAX_FILES],
    generation: u64,
    bank: u32,
    read_only: bool,
    indeterminate: bool,
}
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]))
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]))
}
fn w32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn w64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
fn seal(b: &mut [u8; 512]) {
    let crc = crc32(&b[..508]);
    w32(b, 508, crc);
}
fn valid_crc(b: &[u8; 512]) -> bool {
    u32_at(b, 508) == crc32(&b[..508])
}
fn bank_start(bank: u32) -> u64 {
    2 + u64::from(bank) * BANK_SECTORS
}
fn path_name(path: &str) -> Option<[u8; 32]> {
    if !path.starts_with('/')
        || path.len() < 2
        || path.len() > 31
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        || !path[1..]
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
    {
        return None;
    }
    let mut name = [0; 32];
    name[..path.len()].copy_from_slice(path.as_bytes());
    Some(name)
}
fn encode_root(generation: u64, entries: &[Entry; MAX_FILES]) -> [u8; 512] {
    let mut b = [0; 512];
    b[..8].copy_from_slice(ROOT_MAGIC);
    w32(&mut b, 8, 1);
    w64(&mut b, 16, generation);
    w32(
        &mut b,
        24,
        entries.iter().filter(|e| e.used()).count() as u32,
    );
    for (i, e) in entries.iter().enumerate() {
        let at = 32 + i * 56;
        b[at..at + 32].copy_from_slice(&e.info.name);
        w32(&mut b, at + 32, e.info.size);
        w32(&mut b, at + 36, e.crc);
    }
    seal(&mut b);
    b
}
fn decode_root(b: &[u8; 512], commit: Commit) -> Option<[Entry; MAX_FILES]> {
    if &b[..8] != ROOT_MAGIC
        || u32_at(b, 8) != 1
        || b[12..16] != [0; 4]
        || u64_at(b, 16) != commit.generation
        || b[28..32] != [0; 4]
        || b[480..508].iter().any(|b| *b != 0)
        || !valid_crc(b)
        || u32_at(b, 508) != commit.root_crc
    {
        return None;
    }
    let mut entries = [Entry::EMPTY; MAX_FILES];
    for i in 0..MAX_FILES {
        let at = 32 + i * 56;
        if b[at + 40..at + 56].iter().any(|b| *b != 0) {
            return None;
        }
        let mut name = [0; 32];
        name.copy_from_slice(&b[at..at + 32]);
        let size = u32_at(b, at + 32);
        let crc = u32_at(b, at + 36);
        if name[0] == 0 {
            if name != [0; 32] || size != 0 || crc != 0 {
                return None;
            }
        } else {
            let zero = name.iter().position(|b| *b == 0)?;
            if name[zero..].iter().any(|b| *b != 0) {
                return None;
            }
            let path = core::str::from_utf8(&name[..zero]).ok()?;
            if path_name(path)? != name
                || size as usize > MAX_FILE_BYTES
                || entries[..i].iter().any(|e| e.info.name == name)
            {
                return None;
            }
        }
        entries[i] = Entry {
            info: FileInfo { name, size },
            crc,
        };
    }
    if u32_at(b, 24) != entries.iter().filter(|e| e.used()).count() as u32 {
        return None;
    }
    Some(entries)
}
fn encode_super(commit: Commit, start: u64, sectors: u64) -> [u8; 512] {
    let mut b = [0; 512];
    b[..8].copy_from_slice(SUPER_MAGIC);
    w32(&mut b, 8, 1);
    w32(&mut b, 12, 512);
    w64(&mut b, 16, commit.generation);
    w32(&mut b, 24, commit.bank);
    w32(&mut b, 28, MAX_FILES as u32);
    w64(&mut b, 32, start);
    w64(&mut b, 40, sectors);
    w32(&mut b, 48, commit.root_crc);
    seal(&mut b);
    b
}
fn decode_super(b: &[u8; 512], start: u64, sectors: u64) -> Option<Commit> {
    if &b[..8] != SUPER_MAGIC
        || u32_at(b, 8) != 1
        || u32_at(b, 12) != 512
        || u64_at(b, 16) == 0
        || u32_at(b, 24) > 1
        || u32_at(b, 28) != MAX_FILES as u32
        || u64_at(b, 32) != start
        || u64_at(b, 40) != sectors
        || b[52..508].iter().any(|b| *b != 0)
        || !valid_crc(b)
    {
        return None;
    }
    Some(Commit {
        generation: u64_at(b, 16),
        bank: u32_at(b, 24),
        root_crc: u32_at(b, 48),
    })
}
fn read_payload<D: WritableBlockDevice>(
    partition: &mut Partition<'_, D>,
    bank: u32,
    index: usize,
    entry: Entry,
    out: &mut [u8; MAX_FILE_BYTES],
) -> Result<(), Error<D::Error>> {
    out.fill(0);
    if !entry.used() {
        return Ok(());
    }
    for sector in 0..8 {
        let mut b = [0; 512];
        partition.read(bank_start(bank) + index as u64 * 8 + sector, &mut b)?;
        let offset = sector as usize * 512;
        out[offset..offset + 512].copy_from_slice(&b);
    }
    let size = entry.info.size as usize;
    if crc32(&out[..size]) != entry.crc || out[size..].iter().any(|b| *b != 0) {
        return Err(Error::CorruptData);
    }
    Ok(())
}
impl<'a, D: WritableBlockDevice> Store<'a, D> {
    /// Destructive format is explicit. The caller must already authorize the
    /// writable partition; mount never formats or repairs an unknown format.
    pub fn format(mut partition: Partition<'a, D>) -> Result<Self, Error<D::Error>> {
        let zero = [0; 512];
        partition.write(0, &zero)?;
        partition.write(1, &zero)?;
        partition.flush()?;
        for sector in 0..2 * BANK_SECTORS {
            partition.write(2 + sector, &zero)?;
        }
        let entries = [Entry::EMPTY; MAX_FILES];
        let root = encode_root(1, &entries);
        partition.write(bank_start(0) + DATA_SECTORS, &root)?;
        partition.flush()?;
        let commit = Commit {
            generation: 1,
            bank: 0,
            root_crc: u32_at(&root, 508),
        };
        let superblock = encode_super(commit, partition.start, partition.sectors);
        partition.write(0, &superblock)?;
        partition.flush()?;
        partition.write(1, &superblock)?;
        partition.flush()?;
        Ok(Self {
            partition,
            entries,
            generation: 1,
            bank: 0,
            read_only: false,
            indeterminate: false,
        })
    }
    pub fn mount(mut partition: Partition<'a, D>) -> Result<Self, Error<D::Error>> {
        let mut raw = [[0; 512]; 2];
        let mut last_device_error = None;
        for (i, block) in raw.iter_mut().enumerate() {
            match partition.read(i as u64, block) {
                Ok(()) => (),
                Err(Error::Device(error)) => {
                    block.fill(0);
                    last_device_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        let mut commits = [
            decode_super(&raw[0], partition.start, partition.sectors),
            decode_super(&raw[1], partition.start, partition.sectors),
        ];
        if commits.iter().all(Option::is_none) {
            return match last_device_error {
                Some(error) => Err(Error::Device(error)),
                None => Err(Error::InvalidFormat),
            };
        }
        if let [Some(a), Some(b)] = commits {
            if a.generation == b.generation && (a.bank != b.bank || a.root_crc != b.root_crc) {
                return Err(Error::CorruptMetadata);
            }
        }
        let mut read_only = last_device_error.is_some()
            || commits
                .iter()
                .enumerate()
                .any(|(i, c)| c.is_none() && raw[i].iter().any(|b| *b != 0));
        if commits[1].is_some_and(|b| commits[0].is_none_or(|a| b.generation > a.generation)) {
            commits.swap(0, 1);
        }
        for commit in commits.into_iter().flatten() {
            let mut root = [0; 512];
            match partition.read(bank_start(commit.bank) + DATA_SECTORS, &mut root) {
                Ok(()) => (),
                Err(Error::Device(error)) => {
                    last_device_error = Some(error);
                    read_only = true;
                    continue;
                }
                Err(error) => return Err(error),
            }
            let Some(entries) = decode_root(&root, commit) else {
                read_only = true;
                continue;
            };
            let mut good = true;
            let mut payload = [0; MAX_FILE_BYTES];
            for (i, entry) in entries.iter().copied().enumerate() {
                match read_payload(&mut partition, commit.bank, i, entry, &mut payload) {
                    Ok(()) => (),
                    Err(Error::CorruptData) => {
                        good = false;
                        break;
                    }
                    Err(Error::Device(error)) => {
                        last_device_error = Some(error);
                        good = false;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if !good {
                read_only = true;
                continue;
            }
            return Ok(Self {
                partition,
                entries,
                generation: commit.generation,
                bank: commit.bank,
                read_only,
                indeterminate: false,
            });
        }
        match last_device_error {
            Some(error) => Err(Error::Device(error)),
            None => Err(Error::CorruptMetadata),
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_read_only(&self) -> bool {
        self.read_only || self.indeterminate
    }
    /// Monotonically attenuate this mounted object's authority. No on-disk
    /// metadata is changed and no method can make this object writable again.
    /// Trusted mount policy must be reapplied when creating a new Store.
    pub fn restrict_read_only(&mut self) {
        self.read_only = true;
    }
    pub fn into_partition(self) -> Partition<'a, D> {
        self.partition
    }
    pub fn list(&self, out: &mut [FileInfo]) -> Result<usize, Error<D::Error>> {
        let count = self.entries.iter().filter(|e| e.used()).count();
        if out.len() < count {
            return Err(Error::BufferTooSmall);
        }
        for (target, e) in out.iter_mut().zip(self.entries.iter().filter(|e| e.used())) {
            *target = e.info;
        }
        Ok(count)
    }
    pub fn read_file(&mut self, path: &str, out: &mut [u8]) -> Result<usize, Error<D::Error>> {
        let name = path_name(path).ok_or(Error::InvalidPath)?;
        let index = self
            .entries
            .iter()
            .position(|e| e.used() && e.info.name == name)
            .ok_or(Error::NotFound)?;
        let size = self.entries[index].info.size as usize;
        if out.len() < size {
            return Err(Error::BufferTooSmall);
        }
        let mut payload = [0; MAX_FILE_BYTES];
        if let Err(e) = read_payload(
            &mut self.partition,
            self.bank,
            index,
            self.entries[index],
            &mut payload,
        ) {
            self.read_only = true;
            return Err(e);
        }
        out[..size].copy_from_slice(&payload[..size]);
        Ok(size)
    }
    pub fn put(&mut self, path: &str, data: &[u8]) -> Result<(), Error<D::Error>> {
        self.writable()?;
        if path == "/dirs" || path.starts_with("/dirs/") {
            return Err(Error::InvalidPath);
        }
        let name = path_name(path).ok_or(Error::InvalidPath)?;
        if data.len() > MAX_FILE_BYTES {
            return Err(Error::TooLarge);
        }
        let index = self
            .entries
            .iter()
            .position(|e| e.info.name == name)
            .or_else(|| self.entries.iter().position(|e| !e.used()))
            .ok_or(Error::Full)?;
        let mut entries = self.entries;
        entries[index] = Entry {
            info: FileInfo {
                name,
                size: data.len() as u32,
            },
            crc: crc32(data),
        };
        self.commit(entries, Some((index, data)))
    }
    pub fn remove(&mut self, path: &str) -> Result<(), Error<D::Error>> {
        self.writable()?;
        if path == "/dirs" || path.starts_with("/dirs/") {
            return Err(Error::InvalidPath);
        }
        let name = path_name(path).ok_or(Error::InvalidPath)?;
        let index = self
            .entries
            .iter()
            .position(|e| e.used() && e.info.name == name)
            .ok_or(Error::NotFound)?;
        let mut entries = self.entries;
        entries[index] = Entry::EMPTY;
        self.commit(entries, None)
    }
    /// Replaces an existing destination atomically, preserving source contents.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), Error<D::Error>> {
        self.writable()?;
        if old == "/dirs"
            || old.starts_with("/dirs/")
            || new == "/dirs"
            || new.starts_with("/dirs/")
        {
            return Err(Error::InvalidPath);
        }
        let old = path_name(old).ok_or(Error::InvalidPath)?;
        let new = path_name(new).ok_or(Error::InvalidPath)?;
        let index = self
            .entries
            .iter()
            .position(|e| e.used() && e.info.name == old)
            .ok_or(Error::NotFound)?;
        if old == new {
            return Ok(());
        }
        let mut entries = self.entries;
        if let Some(destination) = entries.iter().position(|e| e.used() && e.info.name == new) {
            entries[destination] = Entry::EMPTY;
        }
        entries[index].info.name = new;
        self.commit(entries, None)
    }
    fn writable(&self) -> Result<(), Error<D::Error>> {
        if self.indeterminate {
            return Err(Error::Indeterminate);
        }
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        Ok(())
    }
    fn commit(
        &mut self,
        entries: [Entry; MAX_FILES],
        replacement: Option<(usize, &[u8])>,
    ) -> Result<(), Error<D::Error>> {
        let mut replacements = [None; MAX_FILES];
        if let Some((index, bytes)) = replacement {
            replacements[index] = Some(bytes);
        }
        self.commit_payloads(entries, replacements)
    }
    fn commit_payloads(
        &mut self,
        entries: [Entry; MAX_FILES],
        replacements: [Option<&[u8]>; MAX_FILES],
    ) -> Result<(), Error<D::Error>> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::GenerationExhausted)?;
        let bank = 1 - self.bank;
        // Any I/O error leaves transaction visibility unknown until remount.
        // Do not allow a retry to clobber a possibly committed snapshot.
        self.indeterminate = true;
        let mut payload = [0; MAX_FILE_BYTES];
        for (i, entry) in entries.iter().copied().enumerate() {
            payload.fill(0);
            if entry.used() {
                if let Some(data) = replacements[i] {
                    payload[..data.len()].copy_from_slice(data);
                } else {
                    read_payload(
                        &mut self.partition,
                        self.bank,
                        i,
                        self.entries[i],
                        &mut payload,
                    )?;
                }
            }
            for sector in 0..8 {
                let start = sector * 512;
                let mut b = [0; 512];
                b.copy_from_slice(&payload[start..start + 512]);
                self.partition
                    .write(bank_start(bank) + i as u64 * 8 + sector as u64, &b)?;
            }
        }
        self.partition.flush()?;
        let root = encode_root(generation, &entries);
        self.partition
            .write(bank_start(bank) + DATA_SECTORS, &root)?;
        self.partition.flush()?;
        let superblock = encode_super(
            Commit {
                generation,
                bank,
                root_crc: u32_at(&root, 508),
            },
            self.partition.start,
            self.partition.sectors,
        );
        self.partition.write(u64::from(bank), &superblock)?;
        self.partition.flush()?;
        self.entries = entries;
        self.generation = generation;
        self.bank = bank;
        self.indeterminate = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    mod transaction_tests {
        include!("journal/tests/transaction_tests.rs");
    }
    use super::*;
    use std::collections::BTreeMap;
    use std::vec::Vec;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Op {
        Write(u64),
        Flush,
    }
    #[derive(Clone, Copy)]
    enum Fault {
        Before(usize),
        After(usize),
        Torn(usize, usize),
    }
    #[derive(Clone)]
    struct Disk {
        durable: Vec<[u8; 512]>,
        pending: BTreeMap<u64, [u8; 512]>,
        operations: Vec<Op>,
        fault: Option<Fault>,
        write_through: bool,
        unreadable: Option<u64>,
    }
    impl Disk {
        fn new() -> Self {
            Self {
                durable: std::vec![[0xa5;512];300],
                pending: BTreeMap::new(),
                operations: Vec::new(),
                fault: None,
                write_through: false,
                unreadable: None,
            }
        }
        fn crash(&mut self) {
            self.pending.clear();
            self.fault = None;
            self.operations.clear();
        }
        fn begin(&mut self, op: Op) -> Result<usize, ()> {
            self.operations.push(op);
            let n = self.operations.len();
            if matches!(self.fault,Some(Fault::Before(at)) if at==n) {
                return Err(());
            }
            Ok(n)
        }
        fn end(&self, n: usize) -> Result<(), ()> {
            if matches!(self.fault,Some(Fault::After(at)) if at==n) {
                Err(())
            } else {
                Ok(())
            }
        }
        fn protected_unchanged(&self) {
            assert!(self.durable[..100].iter().all(|b| *b == [0xa5; 512]));
            assert!(self.durable[232..].iter().all(|b| *b == [0xa5; 512]));
        }
    }
    impl BlockDevice for Disk {
        type Error = ();
        fn sector_count(&self) -> u64 {
            self.durable.len() as u64
        }
        fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
            if self.unreadable == Some(lba) {
                return Err(());
            }
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
            if lba >= self.sector_count() {
                return Err(());
            }
            let n = self.begin(Op::Write(lba))?;
            if let Some(Fault::Torn(at, prefix)) = self.fault {
                if at == n {
                    self.durable[lba as usize][..prefix].copy_from_slice(&data[..prefix]);
                    return Err(());
                }
            }
            if self.write_through {
                self.durable[lba as usize] = *data;
            } else {
                self.pending.insert(lba, *data);
            }
            self.end(n)
        }
        fn flush(&mut self) -> Result<(), ()> {
            let n = self.begin(Op::Flush)?;
            for (lba, data) in core::mem::take(&mut self.pending) {
                self.durable[lba as usize] = data;
            }
            self.end(n)
        }
    }
    fn partition(disk: &mut Disk) -> Partition<'_, Disk> {
        Partition::new(disk, 100, REQUIRED_SECTORS, &[(0, 100), (232, 68)]).unwrap()
    }
    fn baseline() -> Disk {
        let mut disk = Disk::new();
        let mut store = Store::format(partition(&mut disk)).unwrap();
        store.put("/a", b"old").unwrap();
        store.put("/keep", b"keep").unwrap();
        let _ = store.into_partition();
        disk.crash();
        disk
    }
    fn contents(store: &mut Store<'_, Disk>, path: &str) -> Vec<u8> {
        let mut bytes = [0; MAX_FILE_BYTES];
        let n = store.read_file(path, &mut bytes).unwrap();
        bytes[..n].to_vec()
    }
    #[test]
    fn extent_capability_excludes_all_protected_ranges_and_checks_overflow() {
        let mut disk = Disk::new();
        assert!(matches!(
            Partition::new(&mut disk, 0, 132, &[(0, 100)]),
            Err(Error::ProtectedRange)
        ));
        assert!(matches!(
            Partition::new(&mut disk, 100, 132, &[(231, 1)]),
            Err(Error::ProtectedRange)
        ));
        assert!(matches!(
            Partition::new(&mut disk, u64::MAX, 132, &[]),
            Err(Error::Geometry)
        ));
        assert!(matches!(
            Partition::new(&mut disk, 100, 131, &[]),
            Err(Error::Geometry)
        ));
        assert!(matches!(
            Partition::new(&mut disk, 200, 132, &[]),
            Err(Error::Geometry)
        ));
        assert!(matches!(
            Partition::new(&mut disk, 100, 132, &[(u64::MAX, 1)]),
            Err(Error::Geometry)
        ));
        assert!(Partition::new(&mut disk, 100, 132, &[(0, 100), (232, 68)]).is_ok());
        assert!(disk.operations.is_empty());
    }
    #[test]
    fn trusted_read_only_attenuation_preserves_data_and_rejects_all_mutations() {
        let mut disk = baseline();
        let durable = disk.durable.clone();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        assert!(!store.is_read_only());
        let generation = store.generation();
        store.restrict_read_only();
        assert!(store.is_read_only());
        assert_eq!(contents(&mut store, "/a"), b"old");
        assert_eq!(store.put("/a", b"changed"), Err(Error::ReadOnly));
        assert_eq!(store.put("/new", b"created"), Err(Error::ReadOnly));
        assert_eq!(store.rename("/a", "/renamed"), Err(Error::ReadOnly));
        assert_eq!(store.remove("/a"), Err(Error::ReadOnly));
        store.restrict_read_only();
        assert_eq!(store.put("/a", b"regrant"), Err(Error::ReadOnly));
        assert_eq!(store.generation(), generation);
        assert_eq!(contents(&mut store, "/a"), b"old");
        let _ = store.into_partition();
        assert!(disk.operations.is_empty());
        assert!(disk.pending.is_empty());
        assert_eq!(disk.durable, durable);
        disk.protected_unchanged();
    }
    #[test]
    fn golden_superblock_layout_crc_and_empty_root_are_stable() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let mut disk = Disk::new();
        let store = Store::format(partition(&mut disk)).unwrap();
        assert_eq!(store.generation(), 1);
        assert!(!store.is_read_only());
        let _ = store.into_partition();
        let sb = &disk.durable[100];
        assert_eq!(&sb[..16], b"NXTCOW01\x01\0\0\0\0\x02\0\0");
        assert_eq!(u64_at(sb, 16), 1);
        assert_eq!(u32_at(sb, 24), 0);
        assert_eq!(u64_at(sb, 32), 100);
        assert_eq!(u64_at(sb, 40), 132);
        assert!(valid_crc(sb));
        assert_eq!(disk.durable[100], disk.durable[101]);
        let root = &disk.durable[166];
        assert_eq!(&root[..8], ROOT_MAGIC);
        assert!(valid_crc(root));
        disk.protected_unchanged();
    }
    #[test]
    fn write_update_rename_replace_delete_and_reopen_are_atomic() {
        let mut disk = baseline();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        assert_eq!(contents(&mut store, "/a"), b"old");
        let data = [0x39; MAX_FILE_BYTES];
        store.put("/a", &data).unwrap();
        store.put("/destination", b"obsolete").unwrap();
        store.rename("/a", "/destination").unwrap();
        assert_eq!(
            store.read_file("/a", &mut [0; MAX_FILE_BYTES]),
            Err(Error::NotFound)
        );
        assert_eq!(contents(&mut store, "/destination"), data);
        store.rename("/destination", "/destination").unwrap();
        store.put("/empty", b"").unwrap();
        assert_eq!(contents(&mut store, "/empty"), b"");
        store.remove("/destination").unwrap();
        let generation = store.generation();
        let _ = store.into_partition();
        disk.crash();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        assert_eq!(store.generation(), generation);
        assert_eq!(contents(&mut store, "/keep"), b"keep");
        assert_eq!(contents(&mut store, "/empty"), b"");
        assert_eq!(
            store.read_file("/destination", &mut [0; MAX_FILE_BYTES]),
            Err(Error::NotFound)
        );
        let mut list = [FileInfo::EMPTY; MAX_FILES];
        assert_eq!(store.list(&mut list), Ok(2));
        assert!(list[..2].iter().any(|e| e.path() == "/keep"));
        assert_eq!(store.list(&mut []), Err(Error::BufferTooSmall));
        let _ = store.into_partition();
        disk.protected_unchanged();
    }
    #[test]
    fn validation_and_full_volume_do_not_issue_writes() {
        let mut disk = baseline();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        store.partition.device.operations.clear();
        for path in [
            "",
            "/",
            "relative",
            "/../a",
            "/a/./b",
            "/a//b",
            "/a/",
            "/é",
            "/a:stream",
            "/a\\b",
        ] {
            assert_eq!(store.put(path, b"x"), Err(Error::InvalidPath));
        }
        assert_eq!(
            store.put("/big", &[0; MAX_FILE_BYTES + 1]),
            Err(Error::TooLarge)
        );
        assert_eq!(store.remove("/missing"), Err(Error::NotFound));
        assert_eq!(store.rename("/missing", "/new"), Err(Error::NotFound));
        assert!(store.partition.device.operations.is_empty());
        for path in ["/b", "/c", "/d", "/e", "/f", "/g"] {
            store.put(path, b"x").unwrap();
        }
        store.partition.device.operations.clear();
        assert_eq!(store.put("/extra", b"x"), Err(Error::Full));
        assert!(store.partition.device.operations.is_empty());
        store.put("/a", b"replacement").unwrap();
        assert_eq!(contents(&mut store, "/a"), b"replacement");
        let mut output = [0xa5; 1];
        assert_eq!(
            store.read_file("/a", &mut output),
            Err(Error::BufferTooSmall)
        );
        assert_eq!(output, [0xa5]);
    }
    #[test]
    fn commit_barriers_precede_root_and_superblock_publication() {
        let mut disk = baseline();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        let bank = 1 - store.bank;
        store.partition.device.operations.clear();
        store.put("/a", b"new").unwrap();
        let ops = &store.partition.device.operations;
        assert_eq!(ops.len(), 69);
        assert!(ops[..64].iter().all(|op| matches!(op, Op::Write(_))));
        assert_eq!(ops[64], Op::Flush);
        assert_eq!(ops[65], Op::Write(100 + bank_start(bank) + DATA_SECTORS));
        assert_eq!(ops[66], Op::Flush);
        assert_eq!(ops[67], Op::Write(100 + u64::from(bank)));
        assert_eq!(ops[68], Op::Flush);
    }
    #[test]
    fn crash_at_every_write_and_flush_boundary_keeps_only_old_or_new_snapshot() {
        let baseline = baseline();
        let new = [0x77; MAX_FILE_BYTES];
        for write_through in [false, true] {
            for after in [false, true] {
                for boundary in 1..=69 {
                    let mut disk = baseline.clone();
                    disk.write_through = write_through;
                    let mut store = Store::mount(partition(&mut disk)).unwrap();
                    store.partition.device.operations.clear();
                    store.partition.device.fault = Some(if after {
                        Fault::After(boundary)
                    } else {
                        Fault::Before(boundary)
                    });
                    assert_eq!(store.put("/a", &new), Err(Error::Device(())));
                    assert_eq!(store.put("/retry", b"forbidden"), Err(Error::Indeterminate));
                    let _ = store.into_partition();
                    disk.crash();
                    let mut store = Store::mount(partition(&mut disk)).unwrap();
                    let visible = contents(&mut store, "/a");
                    assert!(
                        visible == b"old" || visible == new,
                        "boundary {boundary}, after {after}, write through {write_through}"
                    );
                    assert_eq!(contents(&mut store, "/keep"), b"keep");
                    let _ = store.into_partition();
                    disk.protected_unchanged();
                }
            }
        }
    }
    #[test]
    fn torn_data_root_and_superblock_writes_cannot_publish_partial_data() {
        let baseline = baseline();
        let new = [0x6b; MAX_FILE_BYTES];
        for boundary in 1..=69 {
            if [65, 67, 69].contains(&boundary) {
                continue;
            } // Flush operations.
            for prefix in [0, 1, 24, 255, 508, 511] {
                let mut disk = baseline.clone();
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                store.partition.device.operations.clear();
                store.partition.device.fault = Some(Fault::Torn(boundary, prefix));
                assert_eq!(store.put("/a", &new), Err(Error::Device(())));
                let _ = store.into_partition();
                disk.crash();
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                assert_eq!(contents(&mut store, "/a"), b"old");
                assert_eq!(contents(&mut store, "/keep"), b"keep");
                let _ = store.into_partition();
                disk.protected_unchanged();
            }
        }
    }
    #[test]
    fn corrupt_latest_data_or_metadata_falls_back_read_only_without_repair_writes() {
        for corrupt_root in [false, true] {
            let mut disk = baseline();
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            store.put("/a", b"new").unwrap();
            let active = store.bank;
            let _ = store.into_partition();
            let sector = 100 + bank_start(active) + if corrupt_root { DATA_SECTORS } else { 0 };
            disk.durable[sector as usize][0] ^= 1;
            disk.operations.clear();
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            assert!(store.is_read_only());
            assert_eq!(contents(&mut store, "/a"), b"old");
            assert_eq!(store.put("/x", b"x"), Err(Error::ReadOnly));
            assert!(store.partition.device.operations.is_empty());
        }
    }
    #[test]
    fn latest_snapshot_media_errors_recover_previous_snapshot_read_only() {
        for failed_root in [false, true] {
            let mut disk = baseline();
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            store.put("/a", b"new").unwrap();
            let active = store.bank;
            let _ = store.into_partition();
            disk.unreadable =
                Some(100 + bank_start(active) + if failed_root { DATA_SECTORS } else { 0 });
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            assert!(store.is_read_only());
            assert_eq!(contents(&mut store, "/a"), b"old");
            assert_eq!(store.put("/a", b"forbidden"), Err(Error::ReadOnly));
        }
        for failed_superblock in [100, 101] {
            let mut disk = baseline();
            disk.unreadable = Some(failed_superblock);
            let mut store = Store::mount(partition(&mut disk)).unwrap();
            assert!(store.is_read_only());
            assert_eq!(contents(&mut store, "/a"), b"old");
            assert_eq!(store.put("/a", b"forbidden"), Err(Error::ReadOnly));
        }
    }
    #[test]
    fn generation_wrap_and_corrupt_read_never_mutate_output_or_issue_writes() {
        let mut disk = baseline();
        let mut store = Store::mount(partition(&mut disk)).unwrap();
        store.generation = u64::MAX;
        store.partition.device.operations.clear();
        assert_eq!(store.put("/a", b"new"), Err(Error::GenerationExhausted));
        assert!(!store.is_read_only());
        assert!(store.partition.device.operations.is_empty());
        let sector = 100 + bank_start(store.bank);
        store.partition.device.durable[sector as usize][0] ^= 1;
        let mut output = [0xa5; 8];
        assert_eq!(store.read_file("/a", &mut output), Err(Error::CorruptData));
        assert_eq!(output, [0xa5; 8]);
        assert!(store.is_read_only());
        assert_eq!(store.put("/x", b"x"), Err(Error::ReadOnly));
    }
    #[test]
    fn metadata_byte_mutations_never_panic_with_or_without_recomputed_crc() {
        let mut entries = [Entry::EMPTY; MAX_FILES];
        entries[0] = Entry {
            info: FileInfo {
                name: path_name("/a").unwrap(),
                size: 3,
            },
            crc: crc32(b"old"),
        };
        let root = encode_root(1, &entries);
        let commit = Commit {
            generation: 1,
            bank: 0,
            root_crc: u32_at(&root, 508),
        };
        let superblock = encode_super(commit, 100, 132);
        for at in 0..512 {
            for value in [0, 1, 0x7f, 0xff] {
                for reseal in [false, true] {
                    let mut b = root;
                    b[at] = value;
                    if reseal {
                        seal(&mut b);
                    }
                    let _ = decode_root(
                        &b,
                        Commit {
                            root_crc: u32_at(&b, 508),
                            ..commit
                        },
                    );
                    let mut b = superblock;
                    b[at] = value;
                    if reseal {
                        seal(&mut b);
                    }
                    let _ = decode_super(&b, 100, 132);
                }
            }
        }
    }
    #[test]
    fn metadata_reserved_fields_duplicates_and_versions_reject_even_with_fresh_crc() {
        let entries = [Entry::EMPTY; MAX_FILES];
        let root = encode_root(1, &entries);
        let commit = Commit {
            generation: 1,
            bank: 0,
            root_crc: u32_at(&root, 508),
        };
        for at in [8, 12, 24, 28, 32, 72, 480] {
            let mut b = root;
            b[at] = 0xff;
            seal(&mut b);
            let updated = Commit {
                root_crc: u32_at(&b, 508),
                ..commit
            };
            assert!(decode_root(&b, updated).is_none());
        }
        let superblock = encode_super(commit, 100, 132);
        for at in [8, 12, 24, 28, 32, 40, 52, 507] {
            let mut b = superblock;
            b[at] = 0xff;
            seal(&mut b);
            assert!(decode_super(&b, 100, 132).is_none());
        }
        let mut entries = entries;
        entries[0] = Entry {
            info: FileInfo {
                name: path_name("/a").unwrap(),
                size: 0,
            },
            crc: crc32(b""),
        };
        entries[1] = entries[0];
        let b = encode_root(1, &entries);
        assert!(decode_root(
            &b,
            Commit {
                root_crc: u32_at(&b, 508),
                ..commit
            }
        )
        .is_none());
    }
    #[test]
    fn corrupt_both_roots_and_relocated_volume_fail_closed() {
        let mut disk = baseline();
        disk.durable[166][0] ^= 1;
        disk.durable[231][0] ^= 1;
        assert!(matches!(
            Store::mount(partition(&mut disk)),
            Err(Error::CorruptMetadata)
        ));
        let mut disk = baseline();
        assert!(matches!(
            Store::mount(Partition::new(&mut disk, 101, 132, &[]).unwrap()),
            Err(Error::InvalidFormat)
        ));
    }
    #[test]
    fn rename_and_delete_crashes_have_complete_pre_or_post_namespace() {
        let baseline = baseline();
        for rename in [false, true] {
            for boundary in 1..=69 {
                let mut disk = baseline.clone();
                disk.write_through = true;
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                store.partition.device.operations.clear();
                store.partition.device.fault = Some(Fault::After(boundary));
                let result = if rename {
                    store.rename("/a", "/renamed")
                } else {
                    store.remove("/a")
                };
                assert_eq!(result, Err(Error::Device(())));
                let _ = store.into_partition();
                disk.crash();
                let mut store = Store::mount(partition(&mut disk)).unwrap();
                let mut out = [0; 8];
                let old = store.read_file("/a", &mut out);
                if rename {
                    let new = store.read_file("/renamed", &mut out);
                    assert!(matches!(
                        (old, new),
                        (Ok(3), Err(Error::NotFound)) | (Err(Error::NotFound), Ok(3))
                    ));
                } else {
                    assert!(matches!(old, Ok(3) | Err(Error::NotFound)));
                }
                assert_eq!(contents(&mut store, "/keep"), b"keep");
            }
        }
    }
}

#[cfg(test)]
#[path = "journal/directory_tests.rs"]
mod directory_tests;
