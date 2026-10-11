//! Canonical bounded Trash record. CRC detects corruption, not authenticity;
//! expected owner and all namespace authority come from a trusted caller.
use super::journal::{crc32, MAX_FILE_BYTES};
pub const RECORD_BYTES: usize = 128;
pub const MAX_ITEMS: usize = 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Magic,
    Version,
    Reserved,
    Checksum,
    Owner,
    Id,
    Path,
    Size,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub owner: u64,
    pub id: u64,
    pub size: u32,
    pub checksum: u32,
    path: [u8; 31],
    len: u8,
}
fn valid_path(path: &str) -> bool {
    path.starts_with("/native/")
        && path.len() <= 31
        && path[1..].split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}
impl Record {
    pub fn new(owner: u64, id: u64, path: &str, size: usize, checksum: u32) -> Result<Self, Error> {
        if owner == 0 {
            return Err(Error::Owner);
        }
        if id == 0 {
            return Err(Error::Id);
        }
        if !valid_path(path) {
            return Err(Error::Path);
        }
        if size > MAX_FILE_BYTES {
            return Err(Error::Size);
        }
        let mut result = Self {
            owner,
            id,
            size: size as u32,
            checksum,
            path: [0; 31],
            len: path.len() as u8,
        };
        result.path[..path.len()].copy_from_slice(path.as_bytes());
        Ok(result)
    }
    pub fn original(&self) -> &str {
        core::str::from_utf8(&self.path[..self.len as usize]).unwrap_or("")
    }
    pub fn updated(self, bytes: &[u8]) -> Result<Self, Error> {
        Self::new(
            self.owner,
            self.id,
            self.original(),
            bytes.len(),
            crc32(bytes),
        )
    }
    pub fn encode(self) -> [u8; RECORD_BYTES] {
        let mut bytes = [0; RECORD_BYTES];
        bytes[..8].copy_from_slice(b"NXTRASH1");
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&(RECORD_BYTES as u16).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.owner.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.id.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.size.to_le_bytes());
        bytes[36..40].copy_from_slice(&self.checksum.to_le_bytes());
        bytes[40] = self.len;
        bytes[41..72].copy_from_slice(&self.path);
        let crc = crc32(&bytes[..124]);
        bytes[124..].copy_from_slice(&crc.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8], expected_owner: u64) -> Result<Self, Error> {
        if bytes.len() != RECORD_BYTES {
            return Err(Error::Length);
        }
        if &bytes[..8] != b"NXTRASH1" {
            return Err(Error::Magic);
        }
        if bytes[8..10] != 1u16.to_le_bytes()
            || bytes[10..12] != (RECORD_BYTES as u16).to_le_bytes()
        {
            return Err(Error::Version);
        }
        if u32::from_le_bytes(bytes[124..128].try_into().map_err(|_| Error::Length)?)
            != crc32(&bytes[..124])
        {
            return Err(Error::Checksum);
        }
        if bytes[12..16]
            .iter()
            .chain(bytes[72..124].iter())
            .any(|b| *b != 0)
        {
            return Err(Error::Reserved);
        }
        let owner = u64::from_le_bytes(bytes[16..24].try_into().map_err(|_| Error::Length)?);
        if owner != expected_owner || expected_owner == 0 {
            return Err(Error::Owner);
        }
        let id = u64::from_le_bytes(bytes[24..32].try_into().map_err(|_| Error::Length)?);
        let size = u32::from_le_bytes(bytes[32..36].try_into().map_err(|_| Error::Length)?);
        let checksum = u32::from_le_bytes(bytes[36..40].try_into().map_err(|_| Error::Length)?);
        let len = bytes[40] as usize;
        if len > 31 || bytes[41 + len..72].iter().any(|b| *b != 0) {
            return Err(Error::Path);
        }
        let path = core::str::from_utf8(&bytes[41..41 + len]).map_err(|_| Error::Path)?;
        Self::new(owner, id, path, size as usize, checksum)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key([u8; 25]);
impl Key {
    pub fn data(id: u64) -> Result<Self, Error> {
        Self::make(id, b'd')
    }
    pub fn metadata(id: u64) -> Result<Self, Error> {
        Self::make(id, b'm')
    }
    fn make(id: u64, kind: u8) -> Result<Self, Error> {
        if id == 0 {
            return Err(Error::Id);
        }
        let mut bytes = *b"/trash/d.0000000000000000";
        bytes[7] = kind;
        for index in 0..16 {
            let nibble = ((id >> (4 * (15 - index))) & 15) as usize;
            bytes[9 + index] = b"0123456789abcdef"[nibble];
        }
        Ok(Self(bytes))
    }
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or("")
    }
    pub fn parse(path: &str) -> Result<(u64, bool), Error> {
        if path.len() != 25 || !(path.starts_with("/trash/d.") || path.starts_with("/trash/m.")) {
            return Err(Error::Path);
        }
        let mut id = 0u64;
        for b in path.as_bytes()[9..].iter().copied() {
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                _ => return Err(Error::Path),
            };
            id = (id << 4) | u64::from(digit);
        }
        if id == 0 {
            return Err(Error::Id);
        }
        Ok((id, path.as_bytes()[7] == b'm'))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_roundtrip_and_binding() {
        let record = Record::new(1, u64::MAX, "/native/document", 4096, 0x12345678).unwrap();
        let bytes = record.encode();
        assert_eq!(Record::decode(&bytes, 1), Ok(record));
        assert_eq!(Record::decode(&bytes, 2), Err(Error::Owner));
        assert_eq!(Record::decode(&bytes, 0), Err(Error::Owner));
        assert_eq!(record.updated(b"changed").unwrap().size, 7);
    }
    #[test]
    fn every_bit_corruption_and_noncanonical_padding_reject() {
        let bytes = Record::new(1, 17, "/native/file", 5, 123).unwrap().encode();
        for bit in 0..RECORD_BYTES * 8 {
            let mut changed = bytes;
            changed[bit / 8] ^= 1 << (bit % 8);
            assert!(Record::decode(&changed, 1).is_err());
        }
        for index in [12, 15, 71, 72, 123] {
            let mut changed = bytes;
            changed[index] = 1;
            let crc = crc32(&changed[..124]);
            changed[124..].copy_from_slice(&crc.to_le_bytes());
            assert!(Record::decode(&changed, 1).is_err());
        }
        assert_eq!(Record::decode(&bytes[..127], 1), Err(Error::Length));
    }
    #[test]
    fn keys_and_untrusted_paths_are_strictly_bounded() {
        for id in [1, 16, u64::MAX] {
            for metadata in [false, true] {
                let key = if metadata {
                    Key::metadata(id)
                } else {
                    Key::data(id)
                }
                .unwrap();
                assert_eq!(key.path().len(), 25);
                assert_eq!(Key::parse(key.path()), Ok((id, metadata)));
            }
        }
        for path in [
            "/native/file",
            "/trash/d.0000000000000000",
            "/trash/d.000000000000000A",
            "/trash/d.0000000000000001/child",
        ] {
            assert!(Key::parse(path).is_err());
        }
        for path in [
            "/native/",
            "/native/../secret",
            "/prefs/appearance",
            "/native-other/a",
        ] {
            assert_eq!(Record::new(1, 1, path, 0, 0), Err(Error::Path));
        }
        assert_eq!(Record::new(1, 1, "/native/file", 4097, 0), Err(Error::Size));
    }
}
