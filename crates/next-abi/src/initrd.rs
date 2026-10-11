//! Read-only, bounded development init image. CRC32 detects corruption only;
//! images are unsigned until the deployment trust chain is implemented.
//!
//! Header: magic[8], version LE u32, count LE u32, total bytes LE u64,
//! CRC32 LE u32 (these four bytes treated as zero), reserved LE u32.
//! Entries: zero-padded absolute ASCII path[64], offset LE u64, size LE u64.
//! Payloads immediately follow the table in entry order with no gaps or aliases.
pub const MAGIC: [u8; 8] = *b"NEXTINIT";
pub const HEADER_SIZE: usize = 32;
pub const ENTRY_SIZE: usize = 80;
pub const MAX_ENTRIES: usize = 64;
pub const MAX_IMAGE_SIZE: usize = 16 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    Header,
    Version,
    Size,
    Checksum,
    Path,
    Duplicate,
    Range,
    Reserved,
}
#[derive(Debug)]
pub struct InitImage<'a> {
    bytes: &'a [u8],
    count: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub path: &'a str,
    pub data: &'a [u8],
}
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for (i, byte) in bytes.iter().enumerate() {
        crc ^= if (24..28).contains(&i) {
            0
        } else {
            u32::from(*byte)
        };
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
pub fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && path.len() < 64
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        && path[1..]
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
impl<'a> InitImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, InitError> {
        if bytes.len() < HEADER_SIZE || bytes[..8] != MAGIC {
            return Err(InitError::Header);
        }
        if bytes[8..12] != [1, 0, 0, 0] {
            return Err(InitError::Version);
        }
        let count = u32_at(bytes, 12) as usize;
        if bytes.len() > MAX_IMAGE_SIZE
            || u64_at(bytes, 16) != bytes.len() as u64
            || count == 0
            || count > MAX_ENTRIES
            || HEADER_SIZE + count * ENTRY_SIZE > bytes.len()
        {
            return Err(InitError::Size);
        }
        if u32_at(bytes, 28) != 0 {
            return Err(InitError::Reserved);
        }
        if u32_at(bytes, 24) != crc32(bytes) {
            return Err(InitError::Checksum);
        }
        let image = Self { bytes, count };
        let mut expected = HEADER_SIZE + count * ENTRY_SIZE;
        for i in 0..count {
            let at = HEADER_SIZE + i * ENTRY_SIZE;
            let name = &bytes[at..at + 64];
            let zero = name.iter().position(|b| *b == 0).ok_or(InitError::Path)?;
            if name[zero..].iter().any(|b| *b != 0) {
                return Err(InitError::Path);
            }
            let path = core::str::from_utf8(&name[..zero]).map_err(|_| InitError::Path)?;
            if !valid_path(path) {
                return Err(InitError::Path);
            }
            let offset = usize::try_from(u64_at(bytes, at + 64)).map_err(|_| InitError::Range)?;
            let size = usize::try_from(u64_at(bytes, at + 72)).map_err(|_| InitError::Range)?;
            if size == 0 || offset != expected {
                return Err(InitError::Range);
            }
            expected = offset
                .checked_add(size)
                .filter(|end| *end <= bytes.len())
                .ok_or(InitError::Range)?;
            for j in 0..i {
                if image.entry(j).is_some_and(|e| e.path == path) {
                    return Err(InitError::Duplicate);
                }
            }
        }
        if expected != bytes.len() {
            return Err(InitError::Range);
        }
        Ok(image)
    }
    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn entry(&self, index: usize) -> Option<Entry<'a>> {
        if index >= self.count {
            return None;
        }
        let at = HEADER_SIZE + index * ENTRY_SIZE;
        let path_bytes = &self.bytes[at..at + 64];
        let zero = path_bytes.iter().position(|b| *b == 0)?;
        let offset = usize::try_from(u64_at(self.bytes, at + 64)).ok()?;
        let size = usize::try_from(u64_at(self.bytes, at + 72)).ok()?;
        Some(Entry {
            path: core::str::from_utf8(&path_bytes[..zero]).ok()?,
            data: self.bytes.get(offset..offset.checked_add(size)?)?,
        })
    }
    pub fn find(&self, path: &str) -> Option<Entry<'a>> {
        (0..self.count)
            .filter_map(|i| self.entry(i))
            .find(|e| e.path == path)
    }
}
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    fn fixture() -> [u8; 114] {
        let mut b = [0; 114];
        b[..8].copy_from_slice(&MAGIC);
        b[8] = 1;
        b[12] = 1;
        b[16..24].copy_from_slice(&114u64.to_le_bytes());
        b[32..45].copy_from_slice(b"/system/a.elf");
        b[96..104].copy_from_slice(&112u64.to_le_bytes());
        b[104..112].copy_from_slice(&2u64.to_le_bytes());
        b[112..].copy_from_slice(b"ok");
        seal(&mut b);
        b
    }
    fn seal(b: &mut [u8]) {
        let crc = crc32(b);
        b[24..28].copy_from_slice(&crc.to_le_bytes());
    }
    fn two_entries() -> [u8; 197] {
        let mut b = [0; 197];
        b[..8].copy_from_slice(&MAGIC);
        b[8] = 1;
        b[12] = 2;
        b[16..24].copy_from_slice(&197u64.to_le_bytes());
        for (at, path, offset, size) in [
            (32, &b"/system/a.elf"[..], 192u64, 2u64),
            (112, &b"/system/b.elf"[..], 194u64, 3u64),
        ] {
            b[at..at + path.len()].copy_from_slice(path);
            b[at + 64..at + 72].copy_from_slice(&offset.to_le_bytes());
            b[at + 72..at + 80].copy_from_slice(&size.to_le_bytes());
        }
        b[192..].copy_from_slice(b"aabbb");
        seal(&mut b);
        b
    }
    fn error(b: &[u8]) -> InitError {
        InitImage::parse(b).unwrap_err()
    }
    #[test]
    fn exact_entry_limit_and_complete_table_boundaries_are_checked() {
        fn archive(count: usize) -> std::vec::Vec<u8> {
            let table_end = HEADER_SIZE + count * ENTRY_SIZE;
            let mut bytes = std::vec![0; table_end + count];
            bytes[..8].copy_from_slice(&MAGIC);
            bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
            bytes[12..16].copy_from_slice(&(count as u32).to_le_bytes());
            let total = bytes.len() as u64;
            bytes[16..24].copy_from_slice(&total.to_le_bytes());
            for index in 0..count {
                let name = std::format!("/a{index:02}");
                let at = HEADER_SIZE + index * ENTRY_SIZE;
                bytes[at..at + name.len()].copy_from_slice(name.as_bytes());
                bytes[at + 64..at + 72]
                    .copy_from_slice(&((table_end + index) as u64).to_le_bytes());
                bytes[at + 72..at + 80].copy_from_slice(&1u64.to_le_bytes());
                bytes[table_end + index] = index as u8;
            }
            seal(&mut bytes);
            bytes
        }
        assert_eq!(MAX_ENTRIES, 64);
        let bytes = archive(MAX_ENTRIES);
        let image = InitImage::parse(&bytes).unwrap();
        for index in 0..MAX_ENTRIES {
            let entry = image.entry(index).unwrap();
            assert_eq!(entry.path, std::format!("/a{index:02}"));
            assert_eq!(entry.data, &[index as u8]);
        }
        assert!(image.entry(MAX_ENTRIES).is_none());
        assert_eq!(error(&archive(MAX_ENTRIES + 1)), InitError::Size);
        let mut truncated = bytes[..HEADER_SIZE + MAX_ENTRIES * ENTRY_SIZE - 1].to_vec();
        let len = truncated.len() as u64;
        truncated[16..24].copy_from_slice(&len.to_le_bytes());
        seal(&mut truncated);
        assert_eq!(error(&truncated), InitError::Size);
        let mut overlap = bytes;
        let last = HEADER_SIZE + (MAX_ENTRIES - 1) * ENTRY_SIZE;
        overlap[last + 64..last + 72].copy_from_slice(&(last as u64).to_le_bytes());
        seal(&mut overlap);
        assert_eq!(error(&overlap), InitError::Range);
    }
    #[test]
    fn crc_matches_standard_polynomial_and_ignores_only_its_field() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let b = fixture();
        let mut modified = b;
        modified[24..28].copy_from_slice(&[0xff; 4]);
        assert_eq!(crc32(&modified), crc32(&b));
        modified[28] = 1;
        assert_ne!(crc32(&modified), crc32(&b));
    }
    #[test]
    fn header_version_size_reserved_and_count_fail_closed() {
        let mut b = fixture();
        b[0] = 0;
        assert_eq!(error(&b), InitError::Header);
        let mut b = fixture();
        b[8] = 2;
        seal(&mut b);
        assert_eq!(error(&b), InitError::Version);
        let mut b = fixture();
        b[9] = 1;
        seal(&mut b);
        assert_eq!(error(&b), InitError::Version);
        for count in [0, 2, MAX_ENTRIES as u32 + 1, u32::MAX] {
            let mut b = fixture();
            b[12..16].copy_from_slice(&count.to_le_bytes());
            seal(&mut b);
            assert_eq!(error(&b), InitError::Size);
        }
        for size in [0, 113, 115, u64::MAX] {
            let mut b = fixture();
            b[16..24].copy_from_slice(&size.to_le_bytes());
            seal(&mut b);
            assert_eq!(error(&b), InitError::Size);
        }
        let mut b = fixture();
        b[28] = 1;
        seal(&mut b);
        assert_eq!(error(&b), InitError::Reserved);
        let mut oversized = std::vec![0;MAX_IMAGE_SIZE+1];
        oversized[..114].copy_from_slice(&fixture());
        assert_eq!(error(&oversized), InitError::Size);
    }
    #[test]
    fn canonical_paths_require_terminated_zero_padding() {
        let mut b = fixture();
        b[32..96].fill(b'a');
        b[32] = b'/';
        seal(&mut b);
        assert_eq!(error(&b), InitError::Path);
        let mut b = fixture();
        b[95] = 1;
        seal(&mut b);
        assert_eq!(error(&b), InitError::Path);
        let mut b = fixture();
        b[33] = 0xff;
        seal(&mut b);
        assert_eq!(error(&b), InitError::Path);
        let mut b = fixture();
        b[32..96].fill(0);
        seal(&mut b);
        assert_eq!(error(&b), InitError::Path);
        assert!(valid_path("/system/lib/foo-1.0_x.elf"));
        let mut b = fixture();
        b[32..96].fill(b'a');
        b[32] = b'/';
        b[95] = 0;
        seal(&mut b);
        assert!(InitImage::parse(&b).is_ok());
    }
    #[test]
    fn rejects_empty_alias_overflow_and_out_of_image_payloads() {
        for offset in [0, 32, 111, 113, u64::MAX] {
            let mut b = fixture();
            b[96..104].copy_from_slice(&offset.to_le_bytes());
            seal(&mut b);
            assert_eq!(error(&b), InitError::Range);
        }
        for size in [0, 1, 3, u64::MAX] {
            let mut b = fixture();
            b[104..112].copy_from_slice(&size.to_le_bytes());
            seal(&mut b);
            assert_eq!(error(&b), InitError::Range);
        }
    }
    #[test]
    fn multi_entry_table_preserves_order_and_rejects_duplicates_and_gaps() {
        let b = two_entries();
        let image = InitImage::parse(&b).unwrap();
        assert_eq!(image.len(), 2);
        assert!(!image.is_empty());
        assert_eq!(image.entry(0).unwrap().data, b"aa");
        assert_eq!(image.entry(1).unwrap().data, b"bbb");
        assert_eq!(image.find("/system/b.elf").unwrap().data, b"bbb");
        let mut b = two_entries();
        b.copy_within(32..96, 112);
        seal(&mut b);
        assert_eq!(error(&b), InitError::Duplicate);
        for offset in [193, 195, 192, u64::MAX] {
            let mut b = two_entries();
            b[176..184].copy_from_slice(&offset.to_le_bytes());
            seal(&mut b);
            assert_eq!(error(&b), InitError::Range);
        }
        let mut b = two_entries();
        b[96..104].copy_from_slice(&194u64.to_le_bytes());
        seal(&mut b);
        assert_eq!(error(&b), InitError::Range);
    }
    #[test]
    fn resealed_mutations_and_truncated_inputs_never_panic() {
        let valid = two_entries();
        for n in 0..valid.len() {
            assert!(InitImage::parse(&valid[..n]).is_err());
        }
        for at in 0..valid.len() {
            for value in [0, 1, 0x7f, 0xff] {
                let mut b = valid;
                b[at] = value;
                seal(&mut b);
                let _ = InitImage::parse(&b);
            }
        }
    }
    #[test]
    fn validated_lookup_and_every_truncated_prefix() {
        let b = fixture();
        let image = InitImage::parse(&b).unwrap();
        assert_eq!(image.find("/system/a.elf").unwrap().data, b"ok");
        assert!(image.entry(usize::MAX).is_none());
        assert!(image.find("/missing").is_none());
        for n in 0..b.len() {
            assert!(InitImage::parse(&b[..n]).is_err());
        }
    }
    #[test]
    fn rejects_corruption_aliases_and_hostile_ranges() {
        let mut b = fixture();
        b[113] ^= 1;
        assert_eq!(InitImage::parse(&b).unwrap_err(), InitError::Checksum);
        let mut b = fixture();
        b[32..96].fill(0);
        b[32..37].copy_from_slice(b"/../a");
        seal(&mut b);
        assert_eq!(InitImage::parse(&b).unwrap_err(), InitError::Path);
        let mut b = fixture();
        b[96..104].copy_from_slice(&u64::MAX.to_le_bytes());
        seal(&mut b);
        assert_eq!(InitImage::parse(&b).unwrap_err(), InitError::Range);
        for path in [
            "",
            "/",
            "/a//b",
            "/a/../b",
            "/a/./b",
            "/a\\b",
            "/A:stream",
            "/a/",
            "relative",
            "/é",
        ] {
            assert!(!valid_path(path));
        }
    }
}
