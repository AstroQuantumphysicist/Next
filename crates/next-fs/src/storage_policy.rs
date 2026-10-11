//! Canonical bounded GPT journal policy schema. Parsing does not authenticate a
//! policy or grant authority; a trusted caller must verify its signed container.
use super::gpt::{Guid, Partition, NEXT_DATA};
pub const BYTES: usize = 80;
pub const JOURNAL_SECTORS: u64 = 132;
const WRITE: u32 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Size,
    Header,
    Flags,
    Guid,
    Geometry,
    Binding,
    Attributes,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    disk: Guid,
    data: Guid,
    start: u64,
    blocks: u64,
    capacity: u64,
    writable: bool,
}
/// A checked plan, not an authenticated capability. Mutation is confined to the
/// fixed journal prefix; the remainder of the selected data partition is excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub start: u64,
    pub sectors: u64,
    pub writable: bool,
}
impl Policy {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != BYTES {
            return Err(Error::Size);
        }
        if &bytes[..8] != b"NXTSTOR1" || bytes[8..12] != [1, 0, 80, 0] || bytes[76..80] != [0; 4] {
            return Err(Error::Header);
        }
        let flags = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        if flags & !WRITE != 0 {
            return Err(Error::Flags);
        }
        let disk = Guid(bytes[16..32].try_into().unwrap());
        let data = Guid(bytes[32..48].try_into().unwrap());
        if disk == Guid::ZERO || data == Guid::ZERO {
            return Err(Error::Guid);
        }
        let start = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
        let blocks = u64::from_le_bytes(bytes[56..64].try_into().unwrap());
        let capacity = u64::from_le_bytes(bytes[64..72].try_into().unwrap());
        let logical = u32::from_le_bytes(bytes[72..76].try_into().unwrap());
        if logical != 512
            || start < 2
            || blocks < JOURNAL_SECTORS
            || start.checked_add(blocks).is_none_or(|end| end >= capacity)
            || capacity.checked_mul(512).is_none()
        {
            return Err(Error::Geometry);
        }
        Ok(Self {
            disk,
            data,
            start,
            blocks,
            capacity,
            writable: flags & WRITE != 0,
        })
    }
    /// Inputs must come from successful GPT validation, not caller/user names.
    /// Whole-partition geometry binds exactly; GPT read-only bit60 attenuates.
    pub fn bind(self, disk: Guid, data: Partition, capacity: u64) -> Result<Extent, Error> {
        if disk != self.disk
            || data.unique_guid != self.data
            || data.type_guid != NEXT_DATA
            || data.first_lba != self.start
            || data.block_count() != self.blocks
            || capacity != self.capacity
        {
            return Err(Error::Binding);
        }
        if data.no_block_io() {
            return Err(Error::Attributes);
        }
        Ok(Extent {
            start: self.start,
            sectors: JOURNAL_SECTORS,
            writable: self.writable && data.attributes & (1 << 60) == 0,
        })
    }
}
