//! Bounded declarative service configuration. Parsing establishes structural
//! validity only: a trusted loader must authenticate these exact bytes and
//! independently bind executable identities and attenuate launch authority.
pub const MAGIC: &[u8; 8] = b"NXSVC001";
pub const HEADER_BYTES: usize = 32;
pub const ENTRY_BYTES: usize = 256;
pub const MAX_SERVICES: usize = 8;
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_SERVICES * ENTRY_BYTES;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Header,
    Version,
    Size,
    Reserved,
    Checksum,
    Name,
    Path,
    Identity,
    Rights,
    Policy,
    Duplicate,
    Dependency,
    Cycle,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub package_id: &'a str,
    pub executable_digest: [u8; 32],
    pub capability_ceiling: u64,
    pub permissions: u64,
    pub dependencies: u8,
    pub max_starts: u8,
    pub window_ns: u64,
    pub backoff_ns: u64,
    pub max_backoff_ns: u64,
    pub health_ns: u64,
}
impl Entry<'_> {
    /// Compare against independently authenticated principal values. Success
    /// does not insert capabilities or authorize installation/user consent.
    pub fn matches_identity(
        &self,
        package_id: &str,
        executable_digest: &[u8; 32],
        permission_ceiling: u64,
        capability_ceiling: u64,
    ) -> bool {
        self.package_id == package_id
            && &self.executable_digest == executable_digest
            && self.permissions & !permission_ceiling == 0
            && self.capability_ceiling & !capability_ceiling == 0
    }
}
#[derive(Debug)]
pub struct Config<'a> {
    bytes: &'a [u8],
    count: usize,
    active_mask: u8,
}
fn u16_at(b: &[u8], n: usize) -> u16 {
    u16::from_le_bytes([b[n], b[n + 1]])
}
fn u32_at(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap_or([0; 4]))
}
fn u64_at(b: &[u8], n: usize) -> u64 {
    u64::from_le_bytes(b[n..n + 8].try_into().unwrap_or([0; 8]))
}
/// IEEE CRC32 over the exact configuration, treating the checksum field as zero.
/// This detects corruption, not hostile replacement; signature verification is
/// the trusted delivery boundary's responsibility.
pub fn checksum(bytes: &[u8]) -> u32 {
    let mut value = !0u32;
    for (index, &byte) in bytes.iter().enumerate() {
        value ^= if (20..24).contains(&index) { 0 } else { byte } as u32;
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(value & 1)));
        }
    }
    !value
}
fn text(bytes: &[u8], error: Error) -> Result<&str, Error> {
    let end = bytes.iter().position(|b| *b == 0).ok_or(error)?;
    if end == 0 || bytes[end..].iter().any(|b| *b != 0) {
        return Err(error);
    }
    core::str::from_utf8(&bytes[..end]).map_err(|_| error)
}
fn name(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_lowercase()
}
fn package_id(value: &str) -> bool {
    value.contains('.')
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_')
                })
        })
}
fn path(value: &str) -> bool {
    value.starts_with("/system/")
        && value.ends_with(".elf")
        && value[1..].split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with('.')
                && part.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
                })
        })
}
impl<'a> Config<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES || bytes.get(..8) != Some(MAGIC) {
            return Err(Error::Header);
        }
        if u16_at(bytes, 8) != 1 {
            return Err(Error::Version);
        }
        let count = u16_at(bytes, 14) as usize;
        if u16_at(bytes, 10) as usize != HEADER_BYTES
            || u16_at(bytes, 12) as usize != ENTRY_BYTES
            || !(1..=MAX_SERVICES).contains(&count)
            || bytes.len() != HEADER_BYTES + count * ENTRY_BYTES
            || u32_at(bytes, 16) as usize != bytes.len()
        {
            return Err(Error::Size);
        }
        if bytes[25..32].iter().any(|b| *b != 0) {
            return Err(Error::Reserved);
        }
        if checksum(bytes) != u32_at(bytes, 20) {
            return Err(Error::Checksum);
        }
        let registered = ((1u16 << count) - 1) as u8;
        let active_mask = bytes[24];
        if active_mask == 0 || active_mask & !registered != 0 {
            return Err(Error::Dependency);
        }
        let config = Self {
            bytes,
            count,
            active_mask,
        };
        for index in 0..count {
            let entry = config.decode(index)?;
            if entry.dependencies & !registered != 0
                || entry.dependencies & (1 << index) != 0
                || (active_mask & (1 << index) != 0 && entry.dependencies & !active_mask != 0)
            {
                return Err(Error::Dependency);
            }
            for old in 0..index {
                if config.decode(old)?.name == entry.name {
                    return Err(Error::Duplicate);
                }
            }
        }
        let mut visited = 0u8;
        for _ in 0..count {
            for index in 0..count {
                if config.decode(index)?.dependencies & !visited == 0 {
                    visited |= 1 << index;
                }
            }
        }
        if visited != registered {
            return Err(Error::Cycle);
        }
        Ok(config)
    }
    pub const fn active_mask(&self) -> u8 {
        self.active_mask
    }
    pub fn entry_enabled(&self, index: usize) -> bool {
        index < self.count && self.active_mask & (1 << index) != 0
    }
    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn entry(&self, index: usize) -> Option<Entry<'a>> {
        if index < self.count {
            self.decode(index).ok()
        } else {
            None
        }
    }
    fn decode(&self, index: usize) -> Result<Entry<'a>, Error> {
        let start = HEADER_BYTES + index * ENTRY_BYTES;
        let b = &self.bytes[start..start + ENTRY_BYTES];
        if b[210..216].iter().chain(&b[248..256]).any(|b| *b != 0) {
            return Err(Error::Reserved);
        }
        let service_name = text(&b[..32], Error::Name)?;
        let service_path = text(&b[32..96], Error::Path)?;
        let service_id = text(&b[96..160], Error::Identity)?;
        if !name(service_name) {
            return Err(Error::Name);
        }
        if !path(service_path) {
            return Err(Error::Path);
        }
        if !package_id(service_id) || b[160..192].iter().all(|b| *b == 0) {
            return Err(Error::Identity);
        }
        let rights = u64_at(b, 192);
        let permissions = u64_at(b, 200);
        if rights & !0x7f != 0 || permissions & !0x3f != 0 {
            return Err(Error::Rights);
        }
        let entry = Entry {
            name: service_name,
            path: service_path,
            package_id: service_id,
            executable_digest: b[160..192].try_into().map_err(|_| Error::Identity)?,
            capability_ceiling: rights,
            permissions,
            dependencies: b[208],
            max_starts: b[209],
            window_ns: u64_at(b, 216),
            backoff_ns: u64_at(b, 224),
            max_backoff_ns: u64_at(b, 232),
            health_ns: u64_at(b, 240),
        };
        if entry.max_starts == 0
            || entry.window_ns == 0
            || entry.backoff_ns == 0
            || entry.max_backoff_ns < entry.backoff_ns
            || entry.health_ns == 0
        {
            return Err(Error::Policy);
        }
        Ok(entry)
    }
}
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;
    fn put16(b: &mut [u8], at: usize, n: u16) {
        b[at..at + 2].copy_from_slice(&n.to_le_bytes());
    }
    fn put32(b: &mut [u8], at: usize, n: u32) {
        b[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fn put64(b: &mut [u8], at: usize, n: u64) {
        b[at..at + 8].copy_from_slice(&n.to_le_bytes());
    }
    fn seal(b: &mut [u8]) {
        let crc = checksum(b);
        put32(b, 20, crc);
    }
    fn fixture(count: usize) -> Vec<u8> {
        let mut b = vec![0; HEADER_BYTES + count * ENTRY_BYTES];
        b[..8].copy_from_slice(MAGIC);
        put16(&mut b, 8, 1);
        put16(&mut b, 10, HEADER_BYTES as u16);
        put16(&mut b, 12, ENTRY_BYTES as u16);
        put16(&mut b, 14, count as u16);
        b[24] = ((1u16 << count.min(8)) - 1) as u8;
        let length = b.len() as u32;
        put32(&mut b, 16, length);
        for i in 0..count {
            let e = &mut b[HEADER_BYTES + i * ENTRY_BYTES..HEADER_BYTES + (i + 1) * ENTRY_BYTES];
            e[..6].copy_from_slice(b"worker");
            e[6] = b'0' + i as u8;
            e[32..58].copy_from_slice(b"/system/service-worker.elf");
            e[96..111].copy_from_slice(b"org.next.worker");
            e[160..192].fill(7);
            put64(e, 192, 0x7b);
            put64(e, 200, 3);
            e[208] = if i == 0 { 0 } else { 1 << (i - 1) };
            e[209] = 4;
            for (at, n) in [
                (216, 10_000_000_000),
                (224, 20_000_000),
                (232, 200_000_000),
                (240, 300_000_000),
            ] {
                put64(e, at, n);
            }
        }
        seal(&mut b);
        b
    }
    fn error(b: &[u8]) -> Error {
        Config::parse(b).unwrap_err()
    }
    #[test]
    fn graph_identity_ceilings_and_bounded_lifetimes() {
        let bytes = fixture(8);
        let config = Config::parse(&bytes).unwrap();
        assert_eq!(config.len(), 8);
        assert!(!config.is_empty());
        assert!(config.entry(8).is_none());
        let e = config.entry(7).unwrap();
        assert_eq!(e.name, "worker7");
        assert_eq!(e.dependencies, 64);
        assert!(e.matches_identity("org.next.worker", &[7; 32], 3, 0x7b));
        assert!(e.matches_identity("org.next.worker", &[7; 32], 0x3f, 0xff));
        for (id, digest, permissions, rights) in [
            ("org.next.renamed", [7; 32], 3, 0x7b),
            ("org.next.worker", [8; 32], 3, 0x7b),
            ("org.next.worker", [7; 32], 1, 0x7b),
            ("org.next.worker", [7; 32], 3, 1),
        ] {
            assert!(!e.matches_identity(id, &digest, permissions, rights));
        }
        assert_eq!(error(&fixture(0)), Error::Size);
        assert_eq!(error(&fixture(9)), Error::Size);
    }
    #[test]
    fn all_truncations_and_unsigned_corruption_rejected_without_panics() {
        let bytes = fixture(8);
        for length in 0..bytes.len() {
            assert!(Config::parse(&bytes[..length]).is_err());
        }
        for index in 0..bytes.len() {
            let mut altered = bytes.clone();
            altered[index] ^= 1;
            assert!(Config::parse(&altered).is_err(), "byte {index}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(error(&trailing), Error::Size);
    }
    #[test]
    fn resealed_hostile_strings_rights_policy_reserved_and_dimensions() {
        let bytes = fixture(2);
        for (offset, value, want) in [
            (8, 2, Error::Version),
            (10, 0, Error::Size),
            (12, 1, Error::Size),
            (14, 255, Error::Size),
            (16, 255, Error::Size),
            (25, 1, Error::Reserved),
            (HEADER_BYTES + 210, 1, Error::Reserved),
            (HEADER_BYTES + 248, 1, Error::Reserved),
            (HEADER_BYTES, 0, Error::Name),
            (HEADER_BYTES + 7, 1, Error::Name),
            (HEADER_BYTES + 32, b'.', Error::Path),
            (HEADER_BYTES + 39, b'.', Error::Path),
            (HEADER_BYTES + 96, b'A', Error::Identity),
            (HEADER_BYTES + 192, 128, Error::Rights),
            (HEADER_BYTES + 200, 64, Error::Rights),
            (HEADER_BYTES + 209, 0, Error::Policy),
        ] {
            let mut b = bytes.clone();
            b[offset] = value;
            seal(&mut b);
            assert_eq!(error(&b), want, "offset {offset}");
        }
        for field in [216, 224, 240] {
            let mut b = bytes.clone();
            put64(&mut b, HEADER_BYTES + field, 0);
            seal(&mut b);
            assert_eq!(error(&b), Error::Policy);
        }
        let mut b = bytes.clone();
        put64(&mut b, HEADER_BYTES + 232, 1);
        seal(&mut b);
        assert_eq!(error(&b), Error::Policy);
        let mut b = bytes.clone();
        b[HEADER_BYTES + 160..HEADER_BYTES + 192].fill(0);
        seal(&mut b);
        assert_eq!(error(&b), Error::Identity);
        let mut b = bytes;
        b.copy_within(HEADER_BYTES..HEADER_BYTES + 32, HEADER_BYTES + ENTRY_BYTES);
        seal(&mut b);
        assert_eq!(error(&b), Error::Duplicate);
    }
    #[test]
    fn active_mask_limits_admission_without_hiding_declared_graph_errors() {
        let mut b = fixture(2);
        b[24] = 1;
        seal(&mut b);
        let c = Config::parse(&b).unwrap();
        assert_eq!(c.active_mask(), 1);
        assert!(c.entry_enabled(0));
        assert!(!c.entry_enabled(1));
        assert!(!c.entry_enabled(usize::MAX));
        for mask in [0, 2, 4, 128] {
            let mut invalid = b.clone();
            invalid[24] = mask;
            seal(&mut invalid);
            assert_eq!(error(&invalid), Error::Dependency);
        }
        // Disabled entries are still authenticated configuration and validated.
        b[HEADER_BYTES + ENTRY_BYTES + 208] = 2;
        seal(&mut b);
        assert_eq!(error(&b), Error::Dependency);
    }
    #[test]
    fn dependencies_cycles_and_maximum_graph_masks() {
        let bytes = fixture(2);
        for mask in [1, 4, 128] {
            let mut b = bytes.clone();
            b[HEADER_BYTES + 208] = mask;
            seal(&mut b);
            assert_eq!(error(&b), Error::Dependency);
        }
        let mut cycle = bytes;
        cycle[HEADER_BYTES + 208] = 2;
        seal(&mut cycle);
        assert_eq!(error(&cycle), Error::Cycle);
        let mut full = fixture(8);
        full[HEADER_BYTES + 208] = 128;
        seal(&mut full);
        assert_eq!(error(&full), Error::Cycle);
        // A valid graph can reference a later-index dependency; index order is
        // not boot order and must not create an accidental parsing constraint.
        let mut reverse = fixture(8);
        for i in 0..8 {
            reverse[HEADER_BYTES + i * ENTRY_BYTES + 208] = if i == 7 { 0 } else { 1 << (i + 1) };
        }
        seal(&mut reverse);
        assert!(Config::parse(&reverse).is_ok());
    }
}
