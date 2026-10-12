#![no_std]
//! Read-only FAT32 with short (8.3) paths. This is not NextFS or a POSIX VFS.

#[cfg(feature = "host-backup")]
pub mod backup;
#[cfg(feature = "host-backup")]
pub mod backup_archive;
pub mod gpt;
pub mod journal;
#[cfg(feature = "host-backup")]
pub mod journal_backup;
pub mod storage_policy;
pub mod trash_record;

pub trait BlockDevice {
    type Error;
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error>;
    fn sector_count(&self) -> u64;
}
#[derive(Debug, PartialEq, Eq)]
pub enum FsError<E> {
    Device(E),
    InvalidBootSector,
    InvalidGeometry,
    Unsupported,
    InvalidPath,
    NotFound,
    NotFile,
    BufferTooSmall,
    InvalidCluster,
    CorruptChain,
    DirectoryBufferTooSmall,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: [u8; 11],
    pub size: u32,
    pub is_directory: bool,
    pub first_cluster: u32,
}
impl DirectoryEntry {
    pub const EMPTY: Self = Self {
        name: [0; 11],
        size: 0,
        is_directory: false,
        first_cluster: 0,
    };
}
pub struct Volume<D> {
    device: D,
    total_sectors: u32,
    cluster_count: u32,
    sectors_per_cluster: u8,
    fat_start: u32,
    fat_sectors: u32,
    data_start: u32,
    root_cluster: u32,
}
fn u16le(b: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([b[offset], b[offset + 1]])
}
fn u32le(b: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([b[offset], b[offset + 1], b[offset + 2], b[offset + 3]])
}
/// Brent cycle detection needs no allocation and only one FAT lookup per step.
struct Chain {
    current: u32,
    checkpoint: u32,
    power: u64,
    distance: u64,
    visited: u64,
}
impl Chain {
    fn new(cluster: u32) -> Self {
        Self {
            current: cluster,
            checkpoint: cluster,
            power: 1,
            distance: 0,
            visited: 1,
        }
    }
    fn advance<D: BlockDevice>(
        &mut self,
        volume: &mut Volume<D>,
    ) -> Result<bool, FsError<D::Error>> {
        let Some(next) = volume.next_cluster(self.current)? else {
            return Ok(false);
        };
        self.visited += 1;
        self.distance += 1;
        if next == self.checkpoint || self.visited > volume.cluster_count as u64 {
            return Err(FsError::CorruptChain);
        }
        if self.distance == self.power {
            self.checkpoint = next;
            self.power *= 2;
            self.distance = 0;
        }
        self.current = next;
        Ok(true)
    }
}
impl<D: BlockDevice> Volume<D> {
    /// The supplied device starts at the volume's BPB, not an MBR partition table.
    pub fn mount(mut device: D) -> Result<Self, FsError<D::Error>> {
        let mut b = [0; 512];
        device.read_sector(0, &mut b).map_err(FsError::Device)?;
        if b[510..512] != [0x55, 0xaa] {
            return Err(FsError::InvalidBootSector);
        }
        if u16le(&b, 11) != 512 || u16le(&b, 42) != 0 {
            return Err(FsError::Unsupported);
        }
        let spc = b[13];
        let reserved = u16le(&b, 14) as u32;
        let fats = b[16] as u32;
        let total = u32le(&b, 32);
        let fat_sectors = u32le(&b, 36);
        if spc == 0
            || !spc.is_power_of_two()
            || spc > 128
            || reserved == 0
            || fats == 0
            || fats > 2
            || total == 0
            || fat_sectors == 0
            || u16le(&b, 17) != 0
            || u16le(&b, 19) != 0
            || u16le(&b, 22) != 0
            || total as u64 > device.sector_count()
        {
            return Err(FsError::InvalidGeometry);
        }
        let data_start = fat_sectors
            .checked_mul(fats)
            .and_then(|n| n.checked_add(reserved))
            .ok_or(FsError::InvalidGeometry)?;
        if data_start >= total {
            return Err(FsError::InvalidGeometry);
        }
        let clusters = (total - data_start) / spc as u32;
        if !(65525..0x0fff_fff5).contains(&clusters)
            || (clusters as u64 + 2) * 4 > fat_sectors as u64 * 512
        {
            return Err(FsError::InvalidGeometry);
        }
        let root = u32le(&b, 44);
        if root < 2 || root >= clusters + 2 {
            return Err(FsError::InvalidCluster);
        }
        let flags = u16le(&b, 40);
        let active = if flags & 0x80 != 0 {
            (flags & 0x0f) as u32
        } else {
            0
        };
        if active >= fats {
            return Err(FsError::InvalidGeometry);
        }
        let fat_start = reserved + active * fat_sectors;
        Ok(Self {
            device,
            total_sectors: total,
            cluster_count: clusters,
            sectors_per_cluster: spc,
            fat_start,
            fat_sectors,
            data_start,
            root_cluster: root,
        })
    }
    pub fn into_device(self) -> D {
        self.device
    }
    pub fn cluster_count(&self) -> u32 {
        self.cluster_count
    }
    fn sector(&mut self, lba: u32, out: &mut [u8; 512]) -> Result<(), FsError<D::Error>> {
        if lba >= self.total_sectors {
            return Err(FsError::InvalidGeometry);
        }
        self.device
            .read_sector(lba as u64, out)
            .map_err(FsError::Device)
    }
    fn cluster_sector(&self, cluster: u32) -> Result<u32, FsError<D::Error>> {
        if cluster < 2 || cluster >= self.cluster_count + 2 {
            return Err(FsError::InvalidCluster);
        }
        Ok(self.data_start + (cluster - 2) * self.sectors_per_cluster as u32)
    }
    fn next_cluster(&mut self, cluster: u32) -> Result<Option<u32>, FsError<D::Error>> {
        self.cluster_sector(cluster)?;
        let offset = cluster as u64 * 4;
        if offset / 512 >= self.fat_sectors as u64 {
            return Err(FsError::InvalidCluster);
        }
        let mut b = [0; 512];
        self.sector(self.fat_start + (offset / 512) as u32, &mut b)?;
        let next = u32le(&b, (offset % 512) as usize) & 0x0fff_ffff;
        if next >= 0x0fff_fff8 {
            return Ok(None);
        }
        if !(2..0x0fff_fff0).contains(&next) || next >= self.cluster_count + 2 {
            return Err(FsError::CorruptChain);
        }
        Ok(Some(next))
    }
    fn walk_directory<F: FnMut(DirectoryEntry) -> bool>(
        &mut self,
        cluster: u32,
        mut visit: F,
    ) -> Result<(), FsError<D::Error>> {
        self.cluster_sector(cluster)?;
        let mut chain = Chain::new(cluster);
        loop {
            let sector = self.cluster_sector(chain.current)?;
            for s in 0..self.sectors_per_cluster as u32 {
                let mut b = [0; 512];
                self.sector(sector + s, &mut b)?;
                for raw in b.chunks_exact(32) {
                    if raw[0] == 0 {
                        return Ok(());
                    }
                    if raw[0] == 0xe5 || raw[11] == 0x0f || raw[11] & 8 != 0 {
                        continue;
                    }
                    let mut name = [0; 11];
                    name.copy_from_slice(&raw[..11]);
                    if name[0] == 5 {
                        name[0] = 0xe5;
                    }
                    if name[0] == b'.' {
                        continue;
                    }
                    let entry = DirectoryEntry {
                        name,
                        size: u32le(raw, 28),
                        is_directory: raw[11] & 0x10 != 0,
                        first_cluster: ((u16le(raw, 20) as u32) << 16) | u16le(raw, 26) as u32,
                    };
                    if !visit(entry) {
                        return Ok(());
                    }
                }
            }
            if !chain.advance(self)? {
                return Ok(());
            }
        }
    }
    fn lookup(&mut self, path: &str) -> Result<DirectoryEntry, FsError<D::Error>> {
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.is_empty() || path.ends_with('/') {
            return Err(FsError::InvalidPath);
        }
        let mut cluster = self.root_cluster;
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            let name = short_name(component)?;
            let mut found = None;
            self.walk_directory(cluster, |entry| {
                if entry.name == name {
                    found = Some(entry);
                    false
                } else {
                    true
                }
            })?;
            let entry = found.ok_or(FsError::NotFound)?;
            if components.peek().is_none() {
                return Ok(entry);
            }
            if !entry.is_directory {
                return Err(FsError::NotFile);
            }
            cluster = entry.first_cluster;
        }
        Err(FsError::InvalidPath)
    }
    pub fn list_root(&mut self, out: &mut [DirectoryEntry]) -> Result<usize, FsError<D::Error>> {
        let mut count = 0;
        let mut overflow = false;
        self.walk_directory(self.root_cluster, |entry| {
            if count == out.len() {
                overflow = true;
                false
            } else {
                out[count] = entry;
                count += 1;
                true
            }
        })?;
        if overflow {
            Err(FsError::DirectoryBufferTooSmall)
        } else {
            Ok(count)
        }
    }
    /// Reads the complete file or errors. No success result ever denotes a partial read.
    pub fn read_file(&mut self, path: &str, out: &mut [u8]) -> Result<usize, FsError<D::Error>> {
        let entry = self.lookup(path)?;
        if entry.is_directory {
            return Err(FsError::NotFile);
        }
        let size = usize::try_from(entry.size).map_err(|_| FsError::BufferTooSmall)?;
        if out.len() < size {
            return Err(FsError::BufferTooSmall);
        }
        if size == 0 && entry.first_cluster == 0 {
            return Ok(0);
        }
        self.cluster_sector(entry.first_cluster)?;
        // Validate the whole chain before touching output, including cycles beyond EOF.
        let mut check = Chain::new(entry.first_cluster);
        while check.advance(self)? {}
        let capacity = check.visited * self.sectors_per_cluster as u64 * 512;
        if capacity < size as u64 {
            return Err(FsError::CorruptChain);
        }
        let mut chain = Chain::new(entry.first_cluster);
        let mut done = 0;
        while done < size {
            let sector = self.cluster_sector(chain.current)?;
            for s in 0..self.sectors_per_cluster as u32 {
                if done == size {
                    break;
                }
                let mut b = [0; 512];
                self.sector(sector + s, &mut b)?;
                let n = (size - done).min(512);
                out[done..done + n].copy_from_slice(&b[..n]);
                done += n;
            }
            if done < size && !chain.advance(self)? {
                return Err(FsError::CorruptChain);
            }
        }
        Ok(done)
    }
}
fn short_name<E>(text: &str) -> Result<[u8; 11], FsError<E>> {
    if text.is_empty() || text == "." || text == ".." {
        return Err(FsError::InvalidPath);
    }
    let mut parts = text.split('.');
    let base = parts.next().unwrap_or("");
    let ext = parts.next().unwrap_or("");
    if parts.next().is_some()
        || base.is_empty()
        || base.len() > 8
        || ext.len() > 3
        || text.ends_with('.')
    {
        return Err(FsError::Unsupported);
    }
    let mut result = [b' '; 11];
    for (part, start) in [(base, 0), (ext, 8)] {
        for (i, byte) in part.bytes().enumerate() {
            if !byte.is_ascii_alphanumeric() && !b"!#$%&'()-@^_`{}~".contains(&byte) {
                return Err(FsError::Unsupported);
            }
            result[start + i] = byte.to_ascii_uppercase();
        }
    }
    Ok(result)
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    struct Disk {
        sectors: BTreeMap<u64, [u8; 512]>,
        total: u64,
        reads: usize,
    }
    impl BlockDevice for Disk {
        type Error = ();
        fn sector_count(&self) -> u64 {
            self.total
        }
        fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), ()> {
            if lba >= self.total {
                return Err(());
            }
            self.reads += 1;
            *out = self.sectors.get(&lba).copied().unwrap_or([0; 512]);
            Ok(())
        }
    }
    fn put16(b: &mut [u8], offset: usize, value: u16) {
        b[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(b: &mut [u8], offset: usize, value: u32) {
        b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn entry(name: &[u8; 11], cluster: u32, size: u32, directory: bool) -> [u8; 32] {
        let mut b = [0; 32];
        b[..11].copy_from_slice(name);
        b[11] = if directory { 16 } else { 32 };
        put16(&mut b, 20, (cluster >> 16) as u16);
        put16(&mut b, 26, cluster as u16);
        put32(&mut b, 28, size);
        b
    }
    fn disk() -> Disk {
        let mut d = Disk {
            sectors: BTreeMap::new(),
            total: 70000,
            reads: 0,
        };
        let mut boot = [0; 512];
        put16(&mut boot, 11, 512);
        boot[13] = 1;
        put16(&mut boot, 14, 32);
        boot[16] = 1;
        boot[21] = 0xf8;
        put32(&mut boot, 32, 70000);
        put32(&mut boot, 36, 600);
        put32(&mut boot, 44, 2);
        boot[510] = 0x55;
        boot[511] = 0xaa;
        d.sectors.insert(0, boot);
        let mut fat = [0; 512];
        put32(&mut fat, 0, 0x0fff_fff8);
        put32(&mut fat, 4, 0x0fff_ffff);
        put32(&mut fat, 8, 0x0fff_ffff);
        put32(&mut fat, 12, 4);
        put32(&mut fat, 16, 0x0fff_ffff);
        put32(&mut fat, 20, 0x0fff_ffff);
        put32(&mut fat, 24, 0x0fff_ffff);
        d.sectors.insert(32, fat);
        let mut root = [0; 512];
        root[..32].copy_from_slice(&entry(b"NEXT    TXT", 3, 700, false));
        root[32..64].copy_from_slice(&entry(b"EFI        ", 5, 0, true));
        d.sectors.insert(632, root);
        d.sectors.insert(633, [b'A'; 512]);
        d.sectors.insert(634, [b'B'; 512]);
        let mut subdir = [0; 512];
        subdir[..32].copy_from_slice(&entry(b"TEST    TXT", 6, 5, false));
        d.sectors.insert(635, subdir);
        let mut content = [0; 512];
        content[..5].copy_from_slice(b"hello");
        d.sectors.insert(636, content);
        d
    }
    #[test]
    fn sparse_realistic_volume_multicluster_and_nested_paths() {
        let mut v = Volume::mount(disk()).unwrap();
        assert_eq!(v.cluster_count(), 69368);
        let mut b = [0; 700];
        assert_eq!(v.read_file("/next.txt", &mut b), Ok(700));
        assert!(b[..512].iter().all(|&b| b == b'A'));
        assert!(b[512..].iter().all(|&b| b == b'B'));
        let mut b = [0; 5];
        assert_eq!(v.read_file("/EFI/TEST.TXT", &mut b), Ok(5));
        assert_eq!(&b, b"hello");
        let mut list = [DirectoryEntry::EMPTY; 2];
        assert_eq!(v.list_root(&mut list), Ok(2));
        assert_eq!(list[0].name, *b"NEXT    TXT");
        assert!(list[1].is_directory);
    }
    #[test]
    fn rejects_malformed_and_out_of_bounds_geometry() {
        let mut d = disk();
        put32(d.sectors.get_mut(&0).unwrap(), 32, 70001);
        assert!(matches!(Volume::mount(d), Err(FsError::InvalidGeometry)));
        let mut d = disk();
        put32(d.sectors.get_mut(&0).unwrap(), 36, 1);
        assert!(matches!(Volume::mount(d), Err(FsError::InvalidGeometry)));
        let mut d = disk();
        d.sectors.get_mut(&0).unwrap()[13] = 3;
        assert!(matches!(Volume::mount(d), Err(FsError::InvalidGeometry)));
        let mut d = disk();
        put32(d.sectors.get_mut(&0).unwrap(), 44, 70000);
        assert!(matches!(Volume::mount(d), Err(FsError::InvalidCluster)));
        let mut d = disk();
        d.sectors.get_mut(&0).unwrap()[510] = 0;
        assert!(matches!(Volume::mount(d), Err(FsError::InvalidBootSector)));
    }
    #[test]
    fn detects_cycles_before_output_and_short_chains() {
        let mut d = disk();
        put32(d.sectors.get_mut(&32).unwrap(), 16, 3);
        let mut v = Volume::mount(d).unwrap();
        let mut b = [0x55; 700];
        assert_eq!(v.read_file("NEXT.TXT", &mut b), Err(FsError::CorruptChain));
        assert!(b.iter().all(|&b| b == 0x55));
        assert!(v.into_device().reads < 20);
        let mut d = disk();
        put32(d.sectors.get_mut(&32).unwrap(), 12, 0x0fff_ffff);
        let mut v = Volume::mount(d).unwrap();
        assert_eq!(v.read_file("NEXT.TXT", &mut b), Err(FsError::CorruptChain));
        let mut d = disk();
        put32(d.sectors.get_mut(&32).unwrap(), 12, 70000);
        let mut v = Volume::mount(d).unwrap();
        assert_eq!(v.read_file("NEXT.TXT", &mut b), Err(FsError::CorruptChain));
    }
    #[test]
    fn short_buffer_and_paths_are_explicit_errors() {
        let mut v = Volume::mount(disk()).unwrap();
        assert_eq!(
            v.read_file("NEXT.TXT", &mut [0; 5]),
            Err(FsError::BufferTooSmall)
        );
        assert_eq!(
            v.read_file("../NEXT.TXT", &mut [0; 700]),
            Err(FsError::InvalidPath)
        );
        assert_eq!(
            v.read_file("LONGFILENAME.TXT", &mut [0; 700]),
            Err(FsError::Unsupported)
        );
        assert_eq!(v.read_file("/EFI", &mut [0; 700]), Err(FsError::NotFile));
        assert_eq!(
            v.list_root(&mut [DirectoryEntry::EMPTY; 1]),
            Err(FsError::DirectoryBufferTooSmall)
        );
    }
    #[test]
    fn deleted_lfn_and_volume_entries_are_skipped() {
        let mut d = disk();
        let root = d.sectors.get_mut(&632).unwrap();
        root.copy_within(0..64, 96);
        root[0] = 0xe5;
        root[32] = b'A';
        root[43] = 0x0f;
        root[64] = b'V';
        root[75] = 8;
        let mut v = Volume::mount(d).unwrap();
        let mut entries = [DirectoryEntry::EMPTY; 2];
        assert_eq!(v.list_root(&mut entries), Ok(2));
        assert_eq!(entries[0].name, *b"NEXT    TXT");
    }
    #[test]
    fn unterminated_directory_cycle_is_bounded() {
        let mut d = disk();
        let root = d.sectors.get_mut(&632).unwrap();
        for raw in root.chunks_exact_mut(32) {
            raw[0] = 0xe5;
        }
        put32(d.sectors.get_mut(&32).unwrap(), 8, 2);
        let mut v = Volume::mount(d).unwrap();
        assert_eq!(
            v.read_file("MISSING.TXT", &mut [0; 1]),
            Err(FsError::CorruptChain)
        );
        assert!(v.into_device().reads < 10);
    }
}
