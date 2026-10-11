//! Read-only bounded GPT discovery. No partition writes, mount, filesystem
//! inference, repair, autoformat or authority grant. Next GUIDs are experimental.
pub const MAX_PARTITIONS: usize = 128;
const ENTRY_BYTES: usize = 128;
const TABLE_BYTES: usize = MAX_PARTITIONS * ENTRY_BYTES;
/// Caller-owned bounded read buffers. A kernel may place these in guarded static
/// storage rather than consuming its boot stack. No pointers or heap allocation.
pub struct Workspace {
    block: [u8; 4096],
    table: [u8; TABLE_BYTES],
}
impl Workspace {
    pub const fn new() -> Self {
        Self {
            block: [0; 4096],
            table: [0; TABLE_BYTES],
        }
    }
}
impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}
/// GPT on-disk little-endian GUID representation, not UUID textual byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid(pub [u8; 16]);
impl Guid {
    pub const ZERO: Self = Self([0; 16]);
}
pub const ESP: Guid = Guid([
    40, 115, 42, 193, 31, 248, 210, 17, 186, 75, 0, 160, 201, 62, 201, 59,
]);
pub const NEXT_SYSTEM: Guid = Guid([
    154, 176, 160, 107, 5, 30, 39, 95, 158, 15, 254, 31, 168, 225, 192, 191,
]);
pub const NEXT_DATA: Guid = Guid([
    233, 249, 70, 113, 228, 189, 20, 83, 186, 181, 100, 80, 185, 232, 139, 132,
]);
pub const NEXT_RECOVERY: Guid = Guid([
    133, 46, 222, 227, 126, 253, 221, 83, 146, 41, 233, 67, 49, 83, 170, 191,
]);
pub trait BlockReader {
    type Error;
    fn block_size(&self) -> usize;
    fn block_count(&self) -> u64;
    /// Output is exactly the validated logical block size (512 or 4096).
    fn read_block(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Self::Error>;
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Device(E),
    Geometry,
    ProtectiveMbr,
    Header,
    HeaderCrc,
    EntryDimensions,
    TableBounds,
    TableCrc,
    CopiesDisagree,
    PartitionBounds,
    Overlap,
    Guid,
    Name,
    Attributes,
    NotFound,
    Ambiguous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Esp,
    System,
    Data,
    Recovery,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub slot: u16,
    pub type_guid: Guid,
    pub unique_guid: Guid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    name: [u16; 36],
}
impl Partition {
    const EMPTY: Self = Self {
        slot: 0,
        type_guid: Guid::ZERO,
        unique_guid: Guid::ZERO,
        first_lba: 0,
        last_lba: 0,
        attributes: 0,
        name: [0; 36],
    };
    pub fn block_count(&self) -> u64 {
        self.last_lba
            .checked_sub(self.first_lba)
            .and_then(|count| count.checked_add(1))
            .unwrap_or(0)
    }
    pub fn name_utf16(&self) -> &[u16] {
        let len = self.name.iter().position(|v| *v == 0).unwrap_or(36);
        &self.name[..len]
    }
    pub fn kind(&self) -> Kind {
        match self.type_guid {
            ESP => Kind::Esp,
            NEXT_SYSTEM => Kind::System,
            NEXT_DATA => Kind::Data,
            NEXT_RECOVERY => Kind::Recovery,
            _ => Kind::Unknown,
        }
    }
    pub fn no_block_io(&self) -> bool {
        self.attributes & 2 != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NextLayout {
    pub esp: Partition,
    pub data: Partition,
    pub recovery: Option<Partition>,
}
pub struct Gpt {
    pub disk_guid: Guid,
    pub first_usable: u64,
    pub last_usable: u64,
    partitions: [Partition; MAX_PARTITIONS],
    len: usize,
}
impl Gpt {
    /// Caller-owned parser output/scratch. Not a validated partition table.
    pub const fn empty() -> Self {
        Self {
            disk_guid: Guid::ZERO,
            first_usable: 0,
            last_usable: 0,
            partitions: [Partition::EMPTY; MAX_PARTITIONS],
            len: 0,
        }
    }
    pub fn partitions(&self) -> &[Partition] {
        &self.partitions[..self.len]
    }
    pub fn unique_type<E>(&self, guid: Guid) -> Result<Partition, Error<E>> {
        let mut found = self.partitions().iter().filter(|p| p.type_guid == guid);
        let first = *found.next().ok_or(Error::NotFound)?;
        if found.next().is_some() {
            return Err(Error::Ambiguous);
        }
        Ok(first)
    }
    /// Select by type GUID only, never names or assumed LBAs. Ambiguous/missing
    /// required types fail; unknown partitions remain enumerated and untouched.
    pub fn next_layout<E>(&self) -> Result<NextLayout, Error<E>> {
        let esp = self.unique_type(ESP)?;
        let data = self.unique_type(NEXT_DATA)?;
        let recovery = match self.unique_type(NEXT_RECOVERY) {
            Ok(p) => Some(p),
            Err(Error::NotFound) => None,
            Err(e) => return Err(e),
        };
        if esp.no_block_io() || data.no_block_io() || recovery.is_some_and(|p| p.no_block_io()) {
            return Err(Error::Attributes);
        }
        Ok(NextLayout {
            esp,
            data,
            recovery,
        })
    }
}
#[derive(Clone, Copy)]
struct Header {
    disk: Guid,
    first: u64,
    last: u64,
    table: u64,
    count: usize,
    crc: u32,
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn guid(b: &[u8], at: usize) -> Guid {
    Guid(b[at..at + 16].try_into().unwrap())
}
fn crc32(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for byte in b {
        c ^= u32::from(*byte);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1));
        }
    }
    !c
}
fn read<D: BlockReader>(disk: &mut D, lba: u64, out: &mut [u8]) -> Result<(), Error<D::Error>> {
    if lba >= disk.block_count() || out.len() != disk.block_size() {
        return Err(Error::Geometry);
    }
    disk.read_block(lba, out).map_err(Error::Device)
}
fn header<E>(b: &[u8], blocks: u64, current: u64) -> Result<Header, Error<E>> {
    if &b[..8] != b"EFI PART"
        || u32_at(b, 8) != 0x10000
        || u32_at(b, 12) != 92
        || u32_at(b, 20) != 0
        || b[92..].iter().any(|v| *v != 0)
    {
        return Err(Error::Header);
    }
    let mut canonical = [0; 92];
    canonical.copy_from_slice(&b[..92]);
    canonical[16..20].fill(0);
    if crc32(&canonical) != u32_at(b, 16) {
        return Err(Error::HeaderCrc);
    }
    let backup = if current == 1 { blocks - 1 } else { 1 };
    let first = u64_at(b, 40);
    let last = u64_at(b, 48);
    let disk = guid(b, 56);
    if u64_at(b, 24) != current
        || u64_at(b, 32) != backup
        || first < 2
        || first > last
        || last >= blocks - 1
    {
        return Err(Error::Header);
    }
    if disk == Guid::ZERO {
        return Err(Error::Guid);
    }
    let count = u32_at(b, 80) as usize;
    if count == 0 || count > MAX_PARTITIONS || u32_at(b, 84) != ENTRY_BYTES as u32 {
        return Err(Error::EntryDimensions);
    }
    let table = u64_at(b, 72);
    let table_blocks = (count * ENTRY_BYTES).div_ceil(b.len()) as u64;
    let end = table.checked_add(table_blocks).ok_or(Error::TableBounds)?;
    if (current == 1 && (table < 2 || end > first))
        || (current != 1 && (table <= last || end > current))
    {
        return Err(Error::TableBounds);
    }
    Ok(Header {
        disk,
        first,
        last,
        table,
        count,
        crc: u32_at(b, 88),
    })
}
fn name_valid(name: &[u16; 36]) -> bool {
    let end = name.iter().position(|v| *v == 0).unwrap_or(36);
    if name[end..].iter().any(|v| *v != 0) {
        return false;
    }
    let mut at = 0;
    while at < end {
        match name[at] {
            0xd800..=0xdbff => {
                if at + 1 >= end || !(0xdc00..=0xdfff).contains(&name[at + 1]) {
                    return false;
                }
                at += 2;
            }
            0xdc00..=0xdfff => return false,
            _ => at += 1,
        }
    }
    true
}
/// Validate both copies. Any corruption/disagreement fails discovery; this API
/// never picks a writable recovery copy or grants formatting authority.
pub fn discover<D: BlockReader>(disk: &mut D) -> Result<Gpt, Error<D::Error>> {
    let mut result = Gpt::empty();
    discover_into(disk, &mut result)?;
    Ok(result)
}
/// Same validation with caller-owned output: avoids a large returned stack object.
/// On failure output exposes no partitions; metadata must only be used after Ok.
pub fn discover_into<D: BlockReader>(
    disk: &mut D,
    result: &mut Gpt,
) -> Result<(), Error<D::Error>> {
    discover_with_workspace(disk, result, &mut Workspace::new())
}
/// Full caller-owned output and scratch variant for bounded kernel stacks.
/// Scratch contents have no authority and are overwritten before each use.
pub fn discover_with_workspace<D: BlockReader>(
    disk: &mut D,
    result: &mut Gpt,
    workspace: &mut Workspace,
) -> Result<(), Error<D::Error>> {
    let outcome = discover_inner(disk, result, workspace);
    if outcome.is_err() {
        result.len = 0;
        result.disk_guid = Guid::ZERO;
        result.first_usable = 0;
        result.last_usable = 0;
    }
    outcome
}
fn discover_inner<D: BlockReader>(
    disk: &mut D,
    result: &mut Gpt,
    workspace: &mut Workspace,
) -> Result<(), Error<D::Error>> {
    let size = disk.block_size();
    let blocks = disk.block_count();
    if !matches!(size, 512 | 4096) || blocks < 6 || blocks.checked_mul(size as u64).is_none() {
        return Err(Error::Geometry);
    }
    let Workspace { block, table } = workspace;
    let b = &mut block[..size];
    read(disk, 0, b)?;
    let protected_size = (blocks - 1).min(u64::from(u32::MAX)) as u32;
    if b[510..512] != [0x55, 0xaa]
        || b[446] != 0
        || b[450] != 0xee
        || u32_at(b, 454) != 1
        || u32_at(b, 458) != protected_size
        || b[462..510].iter().any(|v| *v != 0)
    {
        return Err(Error::ProtectiveMbr);
    }
    read(disk, 1, b)?;
    let primary = header(b, blocks, 1)?;
    read(disk, blocks - 1, b)?;
    let backup = header(b, blocks, blocks - 1)?;
    if primary.disk != backup.disk
        || primary.first != backup.first
        || primary.last != backup.last
        || primary.count != backup.count
        || primary.crc != backup.crc
    {
        return Err(Error::CopiesDisagree);
    }
    let bytes = primary.count * ENTRY_BYTES;
    let table_blocks = bytes.div_ceil(size);
    for index in 0..table_blocks {
        read(disk, primary.table + index as u64, b)?;
        let offset = index * size;
        let count = (bytes - offset).min(size);
        table[offset..offset + count].copy_from_slice(&b[..count]);
        if b[count..].iter().any(|v| *v != 0) {
            return Err(Error::TableCrc);
        }
    }
    if crc32(&table[..bytes]) != primary.crc {
        return Err(Error::TableCrc);
    }
    for index in 0..table_blocks {
        read(disk, backup.table + index as u64, b)?;
        let offset = index * size;
        let count = (bytes - offset).min(size);
        if b[..count] != table[offset..offset + count] || b[count..].iter().any(|v| *v != 0) {
            return Err(Error::CopiesDisagree);
        }
    }
    result.disk_guid = primary.disk;
    result.first_usable = primary.first;
    result.last_usable = primary.last;
    result.len = 0;
    for index in 0..primary.count {
        let at = index * ENTRY_BYTES;
        let entry = &table[at..at + ENTRY_BYTES];
        let kind = guid(entry, 0);
        if kind == Guid::ZERO {
            if entry.iter().any(|v| *v != 0) {
                return Err(Error::Guid);
            }
            continue;
        }
        let unique = guid(entry, 16);
        if unique == Guid::ZERO || result.partitions().iter().any(|p| p.unique_guid == unique) {
            return Err(Error::Guid);
        }
        let first = u64_at(entry, 32);
        let last = u64_at(entry, 40);
        if first < primary.first || first > last || last > primary.last {
            return Err(Error::PartitionBounds);
        }
        if result
            .partitions()
            .iter()
            .any(|p| first <= p.last_lba && p.first_lba <= last)
        {
            return Err(Error::Overlap);
        }
        let attributes = u64_at(entry, 48);
        if attributes & (((1u64 << 48) - 1) & !7) != 0 {
            return Err(Error::Attributes);
        }
        let mut name = [0; 36];
        for (unit, raw) in name.iter_mut().zip(entry[56..128].chunks_exact(2)) {
            *unit = u16::from_le_bytes([raw[0], raw[1]]);
        }
        if !name_valid(&name) {
            return Err(Error::Name);
        }
        result.partitions[result.len] = Partition {
            slot: index as u16,
            type_guid: kind,
            unique_guid: unique,
            first_lba: first,
            last_lba: last,
            attributes,
            name,
        };
        result.len += 1;
    }
    Ok(())
}
