//! Bounded Linux x86-64 linux_dirent64 serializer; cookies are snapshot indexes.
pub const MAX_ENTRIES: usize = 10; // eight journal children plus . and ..
#[derive(Clone, Copy)]
pub struct Entry {
    pub inode: u64,
    pub kind: u8,
    pub name: [u8; 32],
    pub length: usize,
}
impl Entry {
    pub const EMPTY: Self = Self {
        inode: 0,
        kind: 0,
        name: [0; 32],
        length: 0,
    };
    fn valid(&self) -> bool {
        self.inode != 0
            && matches!(self.kind, 4 | 8)
            && (1..=31).contains(&self.length)
            && !self.name[..self.length]
                .iter()
                .any(|byte| *byte == 0 || *byte == b'/')
    }
    pub fn new(inode: u64, kind: u8, name: &str) -> Option<Self> {
        if inode == 0
            || !matches!(kind, 4 | 8)
            || name.is_empty()
            || name.len() > 31
            || name.bytes().any(|b| b == 0 || b == b'/')
        {
            return None;
        }
        let mut entry = Self {
            inode,
            kind,
            ..Self::EMPTY
        };
        entry.length = name.len();
        entry.name[..name.len()].copy_from_slice(name.as_bytes());
        Some(entry)
    }
}
#[derive(Clone, Copy)]
pub struct Snapshot {
    pub entries: [Entry; MAX_ENTRIES],
    pub count: usize,
}
impl Snapshot {
    pub const EMPTY: Self = Self {
        entries: [Entry::EMPTY; MAX_ENTRIES],
        count: 0,
    };
    pub fn push(&mut self, entry: Entry) -> Option<()> {
        if !entry.valid() {
            return None;
        }
        *self.entries.get_mut(self.count)? = entry;
        self.count += 1;
        Some(())
    }
    pub fn valid(&self) -> bool {
        self.count <= MAX_ENTRIES && self.entries[..self.count].iter().all(Entry::valid)
    }
    /// Directory snapshots must carry the two fixed, typed dot records.
    pub fn valid_directory(&self) -> bool {
        self.valid()
            && self.count >= 2
            && self.entries[0].kind == 4
            && self.entries[1].kind == 4
            && self.entries[0].length == 1
            && &self.entries[0].name[..1] == b"."
            && self.entries[1].length == 2
            && &self.entries[1].name[..2] == b".."
    }
    /// Emit only complete records. No record fits => EINVAL; EOF => zero.
    /// Caller commits returned cookie only after successful user copyout.
    pub fn encode(&self, cursor: usize, out: &mut [u8]) -> Result<(usize, usize), ()> {
        if cursor > self.count || !self.valid() {
            return Err(());
        }
        let mut written = 0;
        let mut next = cursor;
        for entry in &self.entries[cursor..self.count] {
            let name = &entry.name[..entry.length];
            let length = (19 + entry.length + 1 + 7) & !7;
            if length > out.len() - written {
                break;
            }
            let record = &mut out[written..written + length];
            record.fill(0);
            record[..8].copy_from_slice(&entry.inode.to_le_bytes());
            record[8..16].copy_from_slice(&((next + 1) as i64).to_le_bytes());
            record[16..18].copy_from_slice(&(length as u16).to_le_bytes());
            record[18] = entry.kind;
            record[19..19 + entry.length].copy_from_slice(name);
            written += length;
            next += 1;
        }
        if written == 0 && cursor < self.count {
            Err(())
        } else {
            Ok((written, next))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_layout_whole_records_cookies_and_padding() {
        let mut s = Snapshot::EMPTY;
        s.push(Entry::new(7, 4, ".").unwrap()).unwrap();
        s.push(Entry::new(9, 8, "abcdef").unwrap()).unwrap();
        let mut b = [0xa5; 64];
        assert_eq!(s.encode(0, &mut b[..23]), Err(()));
        assert_eq!(&b[..23], &[0xa5; 23]);
        assert_eq!(s.encode(0, &mut b[..31]), Ok((24, 1)));
        assert_eq!(u64::from_le_bytes(b[..8].try_into().unwrap()), 7);
        assert_eq!(i64::from_le_bytes(b[8..16].try_into().unwrap()), 1);
        assert_eq!(&b[16..24], &[24, 0, 4, b'.', 0, 0, 0, 0]);
        assert_eq!(s.encode(1, &mut b), Ok((32, 2)));
        assert_eq!(&b[19..26], b"abcdef\0");
        assert!(b[26..32].iter().all(|v| *v == 0));
        assert_eq!(s.encode(2, &mut []), Ok((0, 2)));
    }

    #[test]
    fn malformed_internal_entries_return_error_without_panicking() {
        let mut snapshot = Snapshot::EMPTY;
        snapshot.count = 1;
        snapshot.entries[0] = Entry::EMPTY;
        assert_eq!(snapshot.encode(0, &mut [0; 64]), Err(()));

        let mut oversized = Entry::new(1, 8, "safe").unwrap();
        oversized.length = usize::MAX;
        snapshot.entries[0] = oversized;
        assert_eq!(snapshot.encode(0, &mut [0; 64]), Err(()));
    }
    #[test]
    fn malformed_tail_is_rejected_before_any_output_and_directory_shape_is_checked() {
        let mut s = Snapshot::EMPTY;
        s.push(Entry::new(1, 4, ".").unwrap()).unwrap();
        s.push(Entry::new(1, 4, "..").unwrap()).unwrap();
        assert!(s.valid_directory());
        for bad in [
            Entry::EMPTY,
            Entry {
                length: 32,
                ..Entry::new(2, 8, "alpha").unwrap()
            },
            Entry {
                length: usize::MAX,
                ..Entry::EMPTY
            },
            Entry {
                kind: 255,
                ..Entry::new(2, 8, "alpha").unwrap()
            },
        ] {
            s.entries[2] = bad;
            s.count = 3;
            let mut out = [0xa5; 128];
            assert_eq!(s.encode(0, &mut out), Err(()));
            assert_eq!(out, [0xa5; 128]);
            assert!(!s.valid_directory());
        }
        s.count = usize::MAX;
        assert!(!s.valid());
        assert_eq!(s.encode(0, &mut [0; 128]), Err(()));
        s.count = 1;
        assert!(!s.valid_directory());
        s.count = 2;
        s.entries[1] = Entry::new(2, 8, "..").unwrap();
        assert!(!s.valid_directory());
        assert!(Entry::new(1, 255, "name").is_none());
        let mut empty = Snapshot::EMPTY;
        assert!(empty.push(Entry::EMPTY).is_none());
    }
}
