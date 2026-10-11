//! Bounded development resolver results. This is not DNSSEC authentication or
//! a general socket interface; resolver policy belongs to the trusted runtime.
pub const MAX_NAME: usize = 253;
pub const MAX_ADDRESSES: usize = 8;
pub const RESULT_BYTES: usize = 144;
pub const CACHE_HIT: u32 = 1;
pub const LOOKUP_RIGHTS: u64 = 0x39;

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    A = 1,
    Aaaa = 28,
}
impl Family {
    pub const fn from_wire(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::A),
            28 => Some(Self::Aaaa),
            _ => None,
        }
    }
}

pub fn canonical_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LookupResult {
    pub version: u32,
    pub family: u16,
    pub count: u16,
    pub ttl_seconds: u32,
    pub flags: u32,
    pub addresses: [[u8; 16]; MAX_ADDRESSES],
}
impl LookupResult {
    pub fn valid(&self) -> bool {
        self.version == 1
            && Family::from_wire(self.family as u64).is_some()
            && (1..=MAX_ADDRESSES as u16).contains(&self.count)
            && self.flags & !CACHE_HIT == 0
            && self.addresses[self.count as usize..]
                .iter()
                .all(|address| *address == [0; 16])
            && (self.family != Family::A as u16
                || self.addresses[..self.count as usize]
                    .iter()
                    .all(|address| address[4..] == [0; 12]))
    }
    pub fn encode(&self) -> Option<[u8; RESULT_BYTES]> {
        if !self.valid() {
            return None;
        }
        let mut bytes = [0; RESULT_BYTES];
        bytes[..4].copy_from_slice(&self.version.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.family.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.count.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.ttl_seconds.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.flags.to_le_bytes());
        for (index, address) in self.addresses.iter().enumerate() {
            bytes[16 + index * 16..32 + index * 16].copy_from_slice(address);
        }
        Some(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != RESULT_BYTES {
            return None;
        }
        let mut result = Self {
            version: u32::from_le_bytes(bytes[..4].try_into().ok()?),
            family: u16::from_le_bytes(bytes[4..6].try_into().ok()?),
            count: u16::from_le_bytes(bytes[6..8].try_into().ok()?),
            ttl_seconds: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
            flags: u32::from_le_bytes(bytes[12..16].try_into().ok()?),
            addresses: [[0; 16]; MAX_ADDRESSES],
        };
        for (index, address) in result.addresses.iter_mut().enumerate() {
            address.copy_from_slice(&bytes[16 + index * 16..32 + index * 16]);
        }
        result.valid().then_some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_labels_are_bounded_ascii_without_implicit_search_or_folding() {
        for name in ["next.test", "ipv6.next.test", "a-1.example"] {
            assert!(canonical_name(name));
        }
        for name in [
            "",
            ".",
            "Next.test",
            "next.test.",
            "a..b",
            "-a.b",
            "a-.b",
            "a/b",
            "é.test",
            "a\0b",
        ] {
            assert!(!canonical_name(name));
        }
        assert!(!canonical_name(&"a".repeat(64)));
        let maximum = [
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61),
        ]
        .join(".");
        assert_eq!(maximum.len(), MAX_NAME);
        assert!(canonical_name(&maximum));
        assert!(!canonical_name(&(maximum + "d")));
    }
    #[test]
    fn wire_layout_preserves_all_eight_answers_and_rejects_ambiguous_output() {
        assert_eq!(core::mem::size_of::<LookupResult>(), RESULT_BYTES);
        let mut result = LookupResult {
            version: 1,
            family: 28,
            count: 8,
            ttl_seconds: 37,
            flags: CACHE_HIT,
            ..Default::default()
        };
        for (index, address) in result.addresses.iter_mut().enumerate() {
            *address = [index as u8 + 1; 16];
        }
        let bytes = result.encode().unwrap();
        assert_eq!(
            &bytes[..16],
            &[1, 0, 0, 0, 28, 0, 8, 0, 37, 0, 0, 0, 1, 0, 0, 0]
        );
        assert_eq!(LookupResult::decode(&bytes), Some(result));
        for length in 0..RESULT_BYTES {
            assert!(LookupResult::decode(&bytes[..length]).is_none());
        }
        for (offset, value) in [(0, 2), (4, 2), (6, 0), (6, 9), (12, 2)] {
            let mut bad = bytes;
            bad[offset] = value;
            assert!(LookupResult::decode(&bad).is_none());
        }
        result.family = 1;
        result.count = 1;
        result.addresses = [[0; 16]; 8];
        result.addresses[0][..4].copy_from_slice(&[10, 0, 2, 15]);
        assert!(result.valid());
        result.addresses[0][4] = 1;
        assert!(!result.valid());
        result.addresses[0][4] = 0;
        result.addresses[1][0] = 1;
        assert!(!result.valid());
    }
}
