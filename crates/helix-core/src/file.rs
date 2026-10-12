//! Bounded shared-inode streams with independent open descriptions. Duplicates
//! share an offset; separate opens share contents but keep independent offsets.
//! Namespace commits precede cache changes. Unlinked data remains anonymous
//! until the last description closes. Inode IDs are lifetime-local, not on disk.
pub const FILE_BYTES: usize = 4096;
pub const PATH_BYTES: usize = 31;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidHandle,
    InvalidPath,
    NotFound,
    Full,
    Busy,
    TooLarge,
    Io,
    ReadOnly,
    Exists,
}
/// At most two ordered namespace/data operations committed as one backend
/// transaction. Expected generation and destination policy come from a trusted
/// caller; names and bytes alone never grant authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation<'a> {
    Create {
        path: &'a str,
        bytes: &'a [u8],
    },
    Replace {
        path: &'a str,
        bytes: &'a [u8],
    },
    Rename {
        old: &'a str,
        new: &'a str,
        replace: bool,
    },
    Remove {
        path: &'a str,
    },
}
#[derive(Clone, Copy)]
struct CacheName {
    path: [u8; PATH_BYTES],
    len: usize,
}
impl CacheName {
    const EMPTY: Self = Self {
        path: [0; PATH_BYTES],
        len: 0,
    };
    fn matches(self, path: &str) -> bool {
        self.len != 0 && self.len == path.len() && &self.path[..self.len] == path.as_bytes()
    }
    fn set(&mut self, path: &str) {
        self.path.fill(0);
        self.path[..path.len()].copy_from_slice(path.as_bytes());
        self.len = path.len();
    }
}
pub trait Backend {
    fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, Error>;
    /// One durable transaction under the caller's existing namespace/cache
    /// guard. Validate the exact generation, all source/destination states and
    /// checksums before writes. Failure must preserve visible names/data; an
    /// indeterminate commit must forbid further mutation until recovery.
    fn transaction(
        &mut self,
        _expected_generation: u64,
        _operations: &[Mutation<'_>],
    ) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    /// Atomic content replacement. An indeterminate failure must block further
    /// backend mutations until recovery; no cache is changed after an error.
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error>;
    /// Atomic replacement rename. Unsupported backends fail without mutation.
    fn rename(&mut self, _old: &str, _new: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    fn remove(&mut self, _path: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileHandle {
    slot: usize,
    generation: u64,
}
struct Entry {
    generation: u64,
    refs: u32,
    retired: bool,
    inode: usize,
    offset: usize,
}
impl Entry {
    const EMPTY: Self = Self {
        generation: 1,
        refs: 0,
        retired: false,
        inode: 0,
        offset: 0,
    };
}
struct Inode {
    id: u64,
    opens: u32,
    path: [u8; PATH_BYTES],
    path_len: usize,
    bytes: [u8; FILE_BYTES],
    len: usize,
}
impl Inode {
    const EMPTY: Self = Self {
        id: 0,
        opens: 0,
        path: [0; PATH_BYTES],
        path_len: 0,
        bytes: [0; FILE_BYTES],
        len: 0,
    };
    fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
    fn set_path(&mut self, path: &str) {
        self.path.fill(0);
        self.path[..path.len()].copy_from_slice(path.as_bytes());
        self.path_len = path.len();
    }
}
pub struct Files<const N: usize> {
    entries: [Entry; N],
    inodes: [Inode; N],
    next_inode: u64,
}
impl<const N: usize> Default for Files<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Files<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { Entry::EMPTY }; N],
            inodes: [const { Inode::EMPTY }; N],
            next_inode: 1,
        }
    }
    fn linked(&self, path: &str) -> Option<usize> {
        self.inodes
            .iter()
            .position(|i| i.opens != 0 && i.path_len != 0 && i.path() == path)
    }
    pub fn is_open(&self, path: &str) -> bool {
        self.linked(path).is_some()
    }
    fn get(&mut self, h: FileHandle) -> Result<(&mut Entry, &mut Inode), Error> {
        let e = self.entries.get_mut(h.slot).ok_or(Error::InvalidHandle)?;
        if e.refs == 0 || e.generation != h.generation {
            return Err(Error::InvalidHandle);
        }
        let inode = self
            .inodes
            .get_mut(e.inode)
            .filter(|i| i.opens != 0)
            .ok_or(Error::InvalidHandle)?;
        Ok((e, inode))
    }
    pub fn open(
        &mut self,
        backend: &mut impl Backend,
        path: &str,
        create: bool,
        truncate: bool,
    ) -> Result<FileHandle, Error> {
        self.open_exclusive(backend, path, create, truncate, false)
    }
    pub fn open_exclusive(
        &mut self,
        backend: &mut impl Backend,
        path: &str,
        create: bool,
        truncate: bool,
        exclusive: bool,
    ) -> Result<FileHandle, Error> {
        if !valid_path(path) {
            return Err(Error::InvalidPath);
        }
        let linked = self.linked(path);
        if exclusive && linked.is_some() {
            return Err(Error::Exists);
        }
        let slot = self
            .entries
            .iter()
            .position(|e| e.refs == 0 && !e.retired)
            .ok_or(Error::Full)?;
        let inode_slot = if let Some(index) = linked {
            self.inodes[index].opens.checked_add(1).ok_or(Error::Full)?;
            if truncate && self.inodes[index].len != 0 {
                backend.put(path, &[])?;
                self.inodes[index].bytes.fill(0);
                self.inodes[index].len = 0;
            }
            index
        } else {
            let index = self
                .inodes
                .iter()
                .position(|i| i.opens == 0)
                .ok_or(Error::Full)?;
            let next = self.next_inode.checked_add(1).ok_or(Error::Full)?;
            let mut bytes = [0; FILE_BYTES];
            let mut len = match backend.read(path, &mut bytes) {
                Ok(_) if exclusive => return Err(Error::Exists),
                Ok(n) if n <= FILE_BYTES => n,
                Ok(_) => return Err(Error::TooLarge),
                Err(Error::NotFound) if create => {
                    backend.put(path, &[])?;
                    0
                }
                Err(error) => return Err(error),
            };
            bytes[len..].fill(0);
            if truncate && len != 0 {
                backend.put(path, &[])?;
                bytes.fill(0);
                len = 0;
            }
            let inode = &mut self.inodes[index];
            inode.id = self.next_inode;
            self.next_inode = next;
            inode.set_path(path);
            inode.bytes = bytes;
            inode.len = len;
            index
        };
        self.inodes[inode_slot].opens = self.inodes[inode_slot]
            .opens
            .checked_add(1)
            .ok_or(Error::Full)?;
        let entry = &mut self.entries[slot];
        entry.inode = inode_slot;
        entry.offset = 0;
        entry.refs = 1;
        Ok(FileHandle {
            slot,
            generation: entry.generation,
        })
    }
    pub fn retain(&mut self, h: FileHandle) -> Result<(), Error> {
        let (e, _) = self.get(h)?;
        e.refs = e.refs.checked_add(1).ok_or(Error::Full)?;
        Ok(())
    }
    pub fn close(&mut self, h: FileHandle) -> Result<(), Error> {
        let (e, inode) = self.get(h)?;
        e.refs -= 1;
        if e.refs == 0 {
            inode.opens -= 1;
            if inode.opens == 0 {
                inode.bytes.fill(0);
                inode.path.fill(0);
                inode.path_len = 0;
                inode.len = 0;
                inode.id = 0;
            }
            e.offset = 0;
            match e.generation.checked_add(1) {
                Some(g) => e.generation = g,
                None => e.retired = true,
            }
        }
        Ok(())
    }
    pub fn read(&mut self, h: FileHandle, out: &mut [u8]) -> Result<usize, Error> {
        let (e, inode) = self.get(h)?;
        let count = out.len().min(inode.len.saturating_sub(e.offset));
        let from = e.offset.min(inode.len);
        out[..count].copy_from_slice(&inode.bytes[from..from + count]);
        e.offset += count;
        Ok(count)
    }
    pub fn write(
        &mut self,
        backend: &mut impl Backend,
        h: FileHandle,
        data: &[u8],
    ) -> Result<usize, Error> {
        let (e, inode) = self.get(h)?;
        let end = e
            .offset
            .checked_add(data.len())
            .filter(|end| *end <= FILE_BYTES)
            .ok_or(Error::TooLarge)?;
        if data.is_empty() {
            return Ok(0);
        }
        let mut next = inode.bytes;
        if e.offset > inode.len {
            next[inode.len..e.offset].fill(0);
        }
        next[e.offset..end].copy_from_slice(data);
        let len = inode.len.max(end);
        if inode.path_len != 0 {
            backend.put(inode.path(), &next[..len])?;
        }
        inode.bytes = next;
        inode.len = len;
        e.offset = end;
        Ok(data.len())
    }
    /// Resize shared inode contents without moving any open-description offset.
    /// Linked data is published only after the namespace backend commits. An
    /// unlinked open inode stays anonymous and cannot recreate its old name.
    pub fn resize(
        &mut self,
        backend: &mut impl Backend,
        h: FileHandle,
        length: usize,
    ) -> Result<(), Error> {
        let (_, inode) = self.get(h)?;
        if length > FILE_BYTES {
            return Err(Error::TooLarge);
        }
        let mut next = inode.bytes;
        // Clear removed bytes too, so shrink/regrow cannot reveal old contents.
        next[length.min(inode.len)..].fill(0);
        if inode.path_len != 0 {
            backend.put(inode.path(), &next[..length])?;
        }
        inode.bytes = next;
        inode.len = length;
        Ok(())
    }
    pub fn seek(&mut self, h: FileHandle, offset: i64, origin: u64) -> Result<u64, Error> {
        let (e, inode) = self.get(h)?;
        let base = match origin {
            0 => 0,
            1 => e.offset,
            2 => inode.len,
            _ => return Err(Error::TooLarge),
        };
        let next = (base as i64)
            .checked_add(offset)
            .filter(|n| *n >= 0 && *n <= FILE_BYTES as i64)
            .ok_or(Error::TooLarge)?;
        e.offset = next as usize;
        Ok(next as u64)
    }
    pub fn info(&mut self, h: FileHandle) -> Result<(usize, usize), Error> {
        let (e, inode) = self.get(h)?;
        Ok((inode.len, e.offset))
    }
    pub fn inode(&mut self, h: FileHandle) -> Result<u64, Error> {
        Ok(self.get(h)?.1.id)
    }
    /// This bounded namespace has at most one name per inode. Retained open
    /// descriptions still address an anonymous inode after unlink/replacement.
    pub fn link_count(&mut self, h: FileHandle) -> Result<u64, Error> {
        Ok(u64::from(self.get(h)?.1.path_len != 0))
    }
    pub fn read_path(
        &self,
        backend: &mut impl Backend,
        path: &str,
        out: &mut [u8; FILE_BYTES],
    ) -> Result<usize, Error> {
        if !valid_path(path) {
            return Err(Error::InvalidPath);
        }
        if let Some(index) = self.linked(path) {
            let inode = &self.inodes[index];
            out[..inode.len].copy_from_slice(&inode.bytes[..inode.len]);
            Ok(inode.len)
        } else {
            backend.read(path, out).and_then(|n| {
                if n <= FILE_BYTES {
                    Ok(n)
                } else {
                    Err(Error::TooLarge)
                }
            })
        }
    }
    /// Validate a bounded cache-transition plan, commit once, then publish all
    /// cache changes without a fallible step. The exclusive caller guard must
    /// span this entire call. Open inode IDs, references and shared offsets are
    /// retained; renamed links follow the inode and removed links stay anonymous.
    pub fn transaction(
        &mut self,
        backend: &mut impl Backend,
        expected_generation: u64,
        operations: &[Mutation<'_>],
    ) -> Result<(), Error> {
        if operations.is_empty() || operations.len() > 2 {
            return Err(Error::TooLarge);
        }
        let mut names = [CacheName::EMPTY; N];
        for (name, inode) in names.iter_mut().zip(&self.inodes) {
            if inode.opens != 0 && inode.path_len != 0 {
                name.path = inode.path;
                name.len = inode.path_len;
            }
        }
        for operation in operations {
            match *operation {
                Mutation::Create { path, bytes } | Mutation::Replace { path, bytes } => {
                    if !valid_path(path) {
                        return Err(Error::InvalidPath);
                    }
                    if bytes.len() > FILE_BYTES {
                        return Err(Error::TooLarge);
                    }
                    if matches!(operation, Mutation::Create { .. })
                        && names.iter().any(|name| name.matches(path))
                    {
                        return Err(Error::Exists);
                    }
                }
                Mutation::Rename { old, new, replace } => {
                    if !valid_path(old) || !valid_path(new) {
                        return Err(Error::InvalidPath);
                    }
                    if old != new {
                        let source = names.iter().position(|name| name.matches(old));
                        if let Some(destination) = names.iter().position(|name| name.matches(new)) {
                            if !replace {
                                return Err(Error::Exists);
                            }
                            names[destination] = CacheName::EMPTY;
                        }
                        if let Some(source) = source {
                            names[source].set(new);
                        }
                    }
                }
                Mutation::Remove { path } => {
                    if !valid_path(path) {
                        return Err(Error::InvalidPath);
                    }
                    if let Some(index) = names.iter().position(|name| name.matches(path)) {
                        names[index] = CacheName::EMPTY;
                    }
                }
            }
        }
        backend.transaction(expected_generation, operations)?;
        // The ordered dry run above guarantees all copy bounds and cache name
        // transitions. Nothing here can fail after the durable receipt.
        for operation in operations {
            match *operation {
                Mutation::Create { path, bytes } | Mutation::Replace { path, bytes } => {
                    if let Some(index) = self.linked(path) {
                        let inode = &mut self.inodes[index];
                        inode.bytes.fill(0);
                        inode.bytes[..bytes.len()].copy_from_slice(bytes);
                        inode.len = bytes.len();
                    }
                }
                Mutation::Rename { old, new, .. } if old != new => {
                    let source = self.linked(old);
                    let destination = self.linked(new);
                    if let Some(index) = destination {
                        self.inodes[index].set_path("");
                    }
                    if let Some(index) = source {
                        self.inodes[index].set_path(new);
                    }
                }
                Mutation::Remove { path } => {
                    if let Some(index) = self.linked(path) {
                        self.inodes[index].set_path("");
                    }
                }
                Mutation::Rename { .. } => {}
            }
        }
        Ok(())
    }
    pub fn replace(
        &mut self,
        backend: &mut impl Backend,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), Error> {
        if !valid_path(path) {
            return Err(Error::InvalidPath);
        }
        if bytes.len() > FILE_BYTES {
            return Err(Error::TooLarge);
        }
        backend.put(path, bytes)?;
        if let Some(index) = self.linked(path) {
            let inode = &mut self.inodes[index];
            inode.bytes.fill(0);
            inode.bytes[..bytes.len()].copy_from_slice(bytes);
            inode.len = bytes.len();
        }
        Ok(())
    }
    pub fn rename(
        &mut self,
        backend: &mut impl Backend,
        old: &str,
        new: &str,
    ) -> Result<(), Error> {
        if !valid_path(old) || !valid_path(new) {
            return Err(Error::InvalidPath);
        }
        backend.rename(old, new)?;
        if old == new {
            return Ok(());
        }
        let source = self.linked(old);
        let destination = self.linked(new);
        if let Some(index) = destination {
            self.inodes[index].set_path("");
        }
        if let Some(index) = source {
            self.inodes[index].set_path(new);
        }
        Ok(())
    }
    pub fn remove(&mut self, backend: &mut impl Backend, path: &str) -> Result<(), Error> {
        if !valid_path(path) {
            return Err(Error::InvalidPath);
        }
        backend.remove(path)?;
        if let Some(index) = self.linked(path) {
            self.inodes[index].set_path("");
        }
        Ok(())
    }
}
fn valid_path(path: &str) -> bool {
    path.len() > 1
        && path.len() <= PATH_BYTES
        && path.starts_with('/')
        && path[1..].split('/').all(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Disk {
        bytes: [u8; FILE_BYTES],
        len: Option<usize>,
        fail: bool,
    }
    impl Disk {
        fn new() -> Self {
            Self {
                bytes: [0; FILE_BYTES],
                len: None,
                fail: false,
            }
        }
    }
    impl Backend for Disk {
        fn read(&mut self, _: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, Error> {
            let n = self.len.ok_or(Error::NotFound)?;
            out[..n].copy_from_slice(&self.bytes[..n]);
            Ok(n)
        }
        fn put(&mut self, _: &str, bytes: &[u8]) -> Result<(), Error> {
            if self.fail {
                return Err(Error::Io);
            }
            self.bytes.fill(0);
            self.bytes[..bytes.len()].copy_from_slice(bytes);
            self.len = Some(bytes.len());
            Ok(())
        }
    }
    #[test]
    fn duplicate_shared_offset_final_close_stale_and_zero_holes() {
        let mut f = Files::<1>::new();
        let mut d = Disk::new();
        let h = f.open(&mut d, "/native/a", true, false).unwrap();
        f.retain(h).unwrap();
        f.seek(h, 3, 0).unwrap();
        f.write(&mut d, h, b"xy").unwrap();
        assert_eq!(&d.bytes[..5], b"\0\0\0xy");
        f.seek(h, 0, 0).unwrap();
        let mut out = [0xff; 8];
        assert_eq!(f.read(h, &mut out).unwrap(), 5);
        assert_eq!(&out[..5], b"\0\0\0xy");
        f.close(h).unwrap();
        assert_eq!(f.info(h), Ok((5, 5)));
        f.close(h).unwrap();
        assert_eq!(f.info(h), Err(Error::InvalidHandle));
        let h2 = f.open(&mut d, "/native/a", false, false).unwrap();
        assert_ne!(h, h2);
    }
    #[test]
    fn failed_commit_preserves_cache_and_offset() {
        let mut f = Files::<1>::new();
        let mut d = Disk::new();
        let h = f.open(&mut d, "/a", true, false).unwrap();
        f.write(&mut d, h, b"old").unwrap();
        f.seek(h, 0, 0).unwrap();
        d.fail = true;
        assert_eq!(f.write(&mut d, h, b"new"), Err(Error::Io));
        assert_eq!(f.info(h), Ok((3, 0)));
        let mut out = [0; 3];
        f.read(h, &mut out).unwrap();
        assert_eq!(&out, b"old");
    }
    #[test]
    fn bounds_no_mutation_and_path_exclusion() {
        let mut f = Files::<1>::new();
        let mut d = Disk::new();
        for p in ["", "/", "/a/../b", "/a//b", "/a/", "/é"] {
            assert_eq!(f.open(&mut d, p, true, false), Err(Error::InvalidPath));
        }
        assert_eq!(d.len, None);
        let h = f.open(&mut d, "/a", true, false).unwrap();
        assert_eq!(f.open(&mut d, "/a", true, true), Err(Error::Full));
        assert_eq!(f.open(&mut d, "/b", true, false), Err(Error::Full));
        assert_eq!(f.seek(h, -1, 0), Err(Error::TooLarge));
        f.seek(h, FILE_BYTES as i64, 0).unwrap();
        assert_eq!(f.write(&mut d, h, b"x"), Err(Error::TooLarge));
        assert_eq!(d.len, Some(0));
        assert_eq!(f.read(h, &mut [0; 1]), Ok(0));
    }

    #[test]
    fn reference_overflow_and_generation_retirement_never_reuse_authority() {
        let mut f = Files::<1>::new();
        let mut d = Disk::new();
        let h = f.open(&mut d, "/a", true, false).unwrap();
        f.entries[0].refs = u32::MAX;
        assert_eq!(f.retain(h), Err(Error::Full));
        assert_eq!(f.entries[0].refs, u32::MAX);
        f.entries[0].refs = 1;
        f.entries[0].generation = u64::MAX;
        let last = FileHandle {
            slot: 0,
            generation: u64::MAX,
        };
        f.close(last).unwrap();
        assert_eq!(f.open(&mut d, "/b", true, false), Err(Error::Full));
        assert_eq!(f.info(last), Err(Error::InvalidHandle));
    }
    extern crate std;
    use std::{collections::BTreeMap, string::String, vec::Vec};
    #[derive(Default)]
    struct Namespace {
        files: BTreeMap<String, Vec<u8>>,
        fail: bool,
        mutations: usize,
        transaction_calls: usize,
    }
    impl Backend for Namespace {
        fn transaction(&mut self, expected: u64, operations: &[Mutation<'_>]) -> Result<(), Error> {
            self.transaction_calls += 1;
            if self.fail {
                return Err(Error::Io);
            }
            if expected != self.mutations as u64 {
                return Err(Error::Busy);
            }
            let mut next = self.files.clone();
            for operation in operations {
                match *operation {
                    Mutation::Create { path, bytes } => {
                        if next.contains_key(path) {
                            return Err(Error::Exists);
                        }
                        next.insert(String::from(path), bytes.to_vec());
                    }
                    Mutation::Replace { path, bytes } => {
                        if !next.contains_key(path) {
                            return Err(Error::NotFound);
                        }
                        next.insert(String::from(path), bytes.to_vec());
                    }
                    Mutation::Rename { old, new, replace } => {
                        let data = next.get(old).ok_or(Error::NotFound)?.clone();
                        if old != new {
                            if !replace && next.contains_key(new) {
                                return Err(Error::Exists);
                            }
                            next.remove(old);
                            next.insert(String::from(new), data);
                        }
                    }
                    Mutation::Remove { path } => {
                        next.remove(path).ok_or(Error::NotFound)?;
                    }
                }
            }
            self.files = next;
            self.mutations += 1;
            Ok(())
        }

        fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, Error> {
            let bytes = self.files.get(path).ok_or(Error::NotFound)?;
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
        fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
            if self.fail {
                return Err(Error::Io);
            }
            self.files.insert(String::from(path), bytes.to_vec());
            self.mutations += 1;
            Ok(())
        }
        fn rename(&mut self, old: &str, new: &str) -> Result<(), Error> {
            if self.fail {
                return Err(Error::Io);
            }
            let bytes = self.files.get(old).ok_or(Error::NotFound)?.clone();
            self.files.remove(old);
            self.files.insert(String::from(new), bytes);
            self.mutations += 1;
            Ok(())
        }
        fn remove(&mut self, path: &str) -> Result<(), Error> {
            if self.fail {
                return Err(Error::Io);
            }
            self.files.remove(path).ok_or(Error::NotFound)?;
            self.mutations += 1;
            Ok(())
        }
    }
    #[test]
    fn resize_shrink_regrow_shared_inode_and_description_offsets() {
        let mut files = Files::<4>::new();
        let mut backend = Namespace::default();
        let a = files.open(&mut backend, "/resize", true, false).unwrap();
        files.write(&mut backend, a, b"abcdef").unwrap();
        let b = files.open(&mut backend, "/resize", false, false).unwrap();
        files.seek(b, 2, 0).unwrap();
        files.retain(a).unwrap(); // duplicate aliases this open description
        let identity = files.inode(a).unwrap();
        files.resize(&mut backend, b, 3).unwrap();
        assert_eq!(files.info(a), Ok((3, 6)));
        assert_eq!(files.info(b), Ok((3, 2)));
        assert_eq!(&backend.files["/resize"], b"abc");
        files.resize(&mut backend, a, 8).unwrap();
        assert_eq!(files.info(a), Ok((8, 6)));
        assert_eq!(files.info(b), Ok((8, 2)));
        assert_eq!(files.inode(b), Ok(identity));
        assert_eq!(&backend.files["/resize"], b"abc\0\0\0\0\0");
        let c = files.open(&mut backend, "/resize", false, false).unwrap();
        let mut bytes = [0xff; 8];
        assert_eq!(files.read(c, &mut bytes), Ok(8));
        assert_eq!(&bytes, b"abc\0\0\0\0\0");
        files.close(a).unwrap(); // surviving duplicate remains live
        assert_eq!(files.info(a), Ok((8, 6)));
        files.resize(&mut backend, a, 0).unwrap();
        assert_eq!(files.info(a), Ok((0, 6)));
        assert_eq!(files.info(b), Ok((0, 2)));
        files.close(a).unwrap();
        assert_eq!(files.resize(&mut backend, a, 1), Err(Error::InvalidHandle));
    }
    #[test]
    fn resize_detached_inode_never_changes_recreated_name() {
        let mut files = Files::<3>::new();
        let mut backend = Namespace::default();
        let old = files.open(&mut backend, "/detached", true, false).unwrap();
        files.write(&mut backend, old, b"secret").unwrap();
        files.remove(&mut backend, "/detached").unwrap();
        let current = files.open(&mut backend, "/detached", true, false).unwrap();
        files.write(&mut backend, current, b"new").unwrap();
        let mutations = backend.mutations;
        backend.fail = true; // anonymous resize requires no namespace commit
        files.resize(&mut backend, old, 2).unwrap();
        files.resize(&mut backend, old, 6).unwrap();
        assert_eq!(files.info(old), Ok((6, 6)));
        assert_eq!(files.link_count(old), Ok(0));
        assert_eq!(files.info(current), Ok((3, 3)));
        assert_eq!(&backend.files["/detached"], b"new");
        assert_eq!(backend.mutations, mutations);
        files.seek(old, 0, 0).unwrap();
        let mut bytes = [0xff; 6];
        assert_eq!(files.read(old, &mut bytes), Ok(6));
        assert_eq!(&bytes, b"se\0\0\0\0");
    }
    #[test]
    fn resize_rejected_length_or_commit_preserves_all_cached_state() {
        let mut files = Files::<2>::new();
        let mut backend = Namespace::default();
        let a = files.open(&mut backend, "/atomic", true, false).unwrap();
        files.write(&mut backend, a, b"original").unwrap();
        let b = files.open(&mut backend, "/atomic", false, false).unwrap();
        files.seek(b, 3, 0).unwrap();
        let identity = files.inode(a);
        let mutations = backend.mutations;
        assert_eq!(
            files.resize(&mut backend, a, usize::MAX),
            Err(Error::TooLarge)
        );
        assert_eq!(
            files.resize(&mut backend, a, FILE_BYTES + 1),
            Err(Error::TooLarge)
        );
        backend.fail = true;
        for length in [0, 4, 8, FILE_BYTES] {
            assert_eq!(files.resize(&mut backend, a, length), Err(Error::Io));
            assert_eq!(files.info(a), Ok((8, 8)));
            assert_eq!(files.info(b), Ok((8, 3)));
            assert_eq!(files.inode(a), identity);
            assert_eq!(&backend.files["/atomic"], b"original");
            assert_eq!(backend.mutations, mutations);
            let inode = files.get(a).unwrap().1;
            assert_eq!(&inode.bytes[..8], b"original");
            assert!(inode.bytes[8..].iter().all(|byte| *byte == 0));
        }
        backend.fail = false;
        files.resize(&mut backend, a, FILE_BYTES).unwrap();
        assert_eq!(files.info(a), Ok((FILE_BYTES, 8)));
        assert_eq!(files.info(b), Ok((FILE_BYTES, 3)));
        assert!(backend.files["/atomic"][8..].iter().all(|byte| *byte == 0));
    }
    #[test]
    fn independent_opens_share_inode_and_live_contents_but_not_offsets() {
        let mut f = Files::<4>::new();
        let mut d = Namespace::default();
        let a = f.open(&mut d, "/a", true, false).unwrap();
        f.write(&mut d, a, b"first").unwrap();
        let b = f.open(&mut d, "/a", false, false).unwrap();
        assert_eq!(f.inode(a), f.inode(b));
        assert_eq!(f.info(a), Ok((5, 5)));
        assert_eq!(f.info(b), Ok((5, 0)));
        f.write(&mut d, b, b"LAST").unwrap();
        f.seek(a, 0, 0).unwrap();
        let mut out = [0; 5];
        assert_eq!(f.read(a, &mut out), Ok(5));
        assert_eq!(&out, b"LASTt");
        f.retain(b).unwrap();
        assert_eq!(f.read(b, &mut out), Ok(1));
        assert_eq!(f.info(b), Ok((5, 5)));
        f.replace(&mut d, "/a", b"x").unwrap();
        assert_eq!(f.info(a), Ok((1, 5)));
        assert_eq!(f.info(b), Ok((1, 5)));
        let c = f.open(&mut d, "/a", false, true).unwrap();
        assert_eq!(f.info(a), Ok((0, 5)));
        assert_eq!(f.info(c), Ok((0, 0)));
    }
    #[test]
    fn open_rename_follows_name_and_unlink_never_recreates_anonymous_inode() {
        let mut f = Files::<4>::new();
        let mut d = Namespace::default();
        let a = f.open(&mut d, "/a", true, false).unwrap();
        f.write(&mut d, a, b"old").unwrap();
        let old_id = f.inode(a).unwrap();
        assert_eq!(f.link_count(a), Ok(1));
        f.rename(&mut d, "/a", "/b").unwrap();
        assert_eq!(f.link_count(a), Ok(1));
        assert!(!d.files.contains_key("/a"));
        f.write(&mut d, a, b"!").unwrap();
        assert_eq!(&d.files["/b"], b"old!");
        f.remove(&mut d, "/b").unwrap();
        assert_eq!(f.link_count(a), Ok(0));
        let mutations = d.mutations;
        f.write(&mut d, a, b"anonymous").unwrap();
        assert_eq!(d.mutations, mutations);
        assert!(!d.files.contains_key("/b"));
        let b = f.open(&mut d, "/b", true, false).unwrap();
        assert_ne!(f.inode(b).unwrap(), old_id);
        assert_eq!(f.link_count(b), Ok(1));
        assert_eq!(f.link_count(a), Ok(0));
        f.write(&mut d, b, b"new").unwrap();
        f.close(a).unwrap();
        assert_eq!(&d.files["/b"], b"new");
        assert!(f
            .inodes
            .iter()
            .filter(|i| i.opens == 0)
            .all(|i| i.bytes.iter().all(|b| *b == 0)));
    }
    #[test]
    fn replacement_rename_keeps_old_destination_open_as_separate_anonymous_inode() {
        let mut f = Files::<4>::new();
        let mut d = Namespace::default();
        let a = f.open(&mut d, "/a", true, false).unwrap();
        let b = f.open(&mut d, "/b", true, false).unwrap();
        f.write(&mut d, a, b"source").unwrap();
        f.write(&mut d, b, b"target").unwrap();
        let old_destination = f.inode(b).unwrap();
        f.rename(&mut d, "/a", "/b").unwrap();
        let c = f.open(&mut d, "/b", false, false).unwrap();
        assert_eq!(f.link_count(a), Ok(1));
        assert_eq!(f.link_count(b), Ok(0));
        assert_eq!(f.link_count(c), Ok(1));
        assert_eq!(f.inode(c), f.inode(a));
        assert_ne!(f.inode(c).unwrap(), old_destination);
        f.seek(b, 0, 0).unwrap();
        f.write(&mut d, b, b"private").unwrap();
        assert_eq!(&d.files["/b"], b"source");
        f.write(&mut d, a, b"!").unwrap();
        assert_eq!(&d.files["/b"], b"source!");
        f.seek(b, 0, 0).unwrap();
        let mut out = [0; 7];
        assert_eq!(f.read(b, &mut out), Ok(7));
        assert_eq!(&out, b"private");
    }
    #[test]
    fn failed_namespace_commits_leave_names_inodes_offsets_and_content_unchanged() {
        let mut f = Files::<4>::new();
        let mut d = Namespace::default();
        let a = f.open(&mut d, "/a", true, false).unwrap();
        f.write(&mut d, a, b"old").unwrap();
        let id = f.inode(a);
        d.fail = true;
        assert_eq!(f.rename(&mut d, "/a", "/b"), Err(Error::Io));
        assert_eq!(f.remove(&mut d, "/a"), Err(Error::Io));
        assert_eq!(f.replace(&mut d, "/a", b"new"), Err(Error::Io));
        assert_eq!(f.open(&mut d, "/a", false, true), Err(Error::Io));
        assert!(f.is_open("/a"));
        assert!(!f.is_open("/b"));
        assert_eq!(f.inode(a), id);
        assert_eq!(f.link_count(a), Ok(1));
        assert_eq!(f.info(a), Ok((3, 3)));
        let mut out = [0; FILE_BYTES];
        assert_eq!(f.read_path(&mut d, "/a", &mut out), Ok(3));
        assert_eq!(&out[..3], b"old");
        assert_eq!(&d.files["/a"], b"old");
    }
    #[test]
    fn atomic_move_restore_and_purge_preserve_inode_and_shared_description_offsets() {
        let mut files = Files::<4>::new();
        let mut disk = Namespace::default();
        let source = files
            .open(&mut disk, "/native/source", true, false)
            .unwrap();
        files.write(&mut disk, source, b"old").unwrap();
        files.retain(source).unwrap();
        let inode = files.inode(source).unwrap();
        let metadata = files.open(&mut disk, "/trash/meta", true, false).unwrap();
        files.write(&mut disk, metadata, b"record0").unwrap();
        let generation = disk.mutations as u64;
        files
            .transaction(
                &mut disk,
                generation,
                &[
                    Mutation::Rename {
                        old: "/native/source",
                        new: "/trash/data",
                        replace: false,
                    },
                    Mutation::Replace {
                        path: "/trash/meta",
                        bytes: b"record1",
                    },
                ],
            )
            .unwrap();
        assert_eq!(disk.mutations as u64, generation + 1);
        assert!(!files.is_open("/native/source"));
        assert!(files.is_open("/trash/data"));
        assert_eq!(files.inode(source), Ok(inode));
        assert_eq!(files.info(source), Ok((3, 3)));
        assert_eq!(files.info(metadata), Ok((7, 7)));
        let mut data = [0; FILE_BYTES];
        assert_eq!(files.read_path(&mut disk, "/trash/meta", &mut data), Ok(7));
        assert_eq!(&data[..7], b"record1");
        files.write(&mut disk, source, b"!").unwrap();
        assert_eq!(&disk.files["/trash/data"], b"old!");
        assert!(!disk.files.contains_key("/native/source"));
        let generation = disk.mutations as u64;
        files
            .transaction(
                &mut disk,
                generation,
                &[
                    Mutation::Rename {
                        old: "/trash/data",
                        new: "/native/restored",
                        replace: false,
                    },
                    Mutation::Remove {
                        path: "/trash/meta",
                    },
                ],
            )
            .unwrap();
        assert_eq!(disk.mutations as u64, generation + 1);
        assert_eq!(files.inode(source), Ok(inode));
        assert_eq!(files.info(source), Ok((4, 4)));
        assert!(!files.is_open("/trash/meta"));
        files.seek(metadata, 0, 0).unwrap();
        let generation = disk.mutations;
        files.write(&mut disk, metadata, b"private").unwrap();
        assert_eq!(disk.mutations, generation);
        assert!(!disk.files.contains_key("/trash/meta"));
        let generation = disk.mutations as u64;
        files
            .transaction(
                &mut disk,
                generation,
                &[
                    Mutation::Rename {
                        old: "/native/restored",
                        new: "/trash/data",
                        replace: false,
                    },
                    Mutation::Create {
                        path: "/trash/meta",
                        bytes: b"record2",
                    },
                ],
            )
            .unwrap();
        let generation = disk.mutations as u64;
        files
            .transaction(
                &mut disk,
                generation,
                &[
                    Mutation::Remove {
                        path: "/trash/data",
                    },
                    Mutation::Remove {
                        path: "/trash/meta",
                    },
                ],
            )
            .unwrap();
        assert_eq!(disk.mutations as u64, generation + 1);
        assert!(!files.is_open("/trash/data"));
        let generation = disk.mutations;
        files.seek(source, 0, 0).unwrap();
        files.write(&mut disk, source, b"anonymous").unwrap();
        assert_eq!(disk.mutations, generation);
        assert!(!disk.files.contains_key("/trash/data"));
        files.close(source).unwrap();
        assert_eq!(files.info(source), Ok((9, 9)));
        files.close(source).unwrap();
    }
    #[test]
    fn transaction_failure_or_invalid_second_operation_preserves_entire_cache_plan() {
        let mut files = Files::<4>::new();
        let mut disk = Namespace::default();
        let source = files
            .open(&mut disk, "/native/source", true, false)
            .unwrap();
        files.write(&mut disk, source, b"source").unwrap();
        let target = files
            .open(&mut disk, "/native/target", true, false)
            .unwrap();
        files.write(&mut disk, target, b"target").unwrap();
        let source_inode = files.inode(source);
        let target_inode = files.inode(target);
        let generation = disk.mutations as u64;
        let before = disk.files.clone();
        for (operations, error) in [
            (
                [
                    Mutation::Rename {
                        old: "/native/source",
                        new: "/trash/data",
                        replace: false,
                    },
                    Mutation::Create {
                        path: "/native/target",
                        bytes: b"bad",
                    },
                ],
                Error::Exists,
            ),
            (
                [
                    Mutation::Rename {
                        old: "/native/source",
                        new: "/trash/data",
                        replace: false,
                    },
                    Mutation::Remove { path: "/missing" },
                ],
                Error::NotFound,
            ),
        ] {
            assert_eq!(
                files.transaction(&mut disk, generation, &operations),
                Err(error)
            );
            assert_eq!(disk.files, before);
            assert_eq!(disk.mutations as u64, generation);
            assert!(files.is_open("/native/source"));
            assert!(!files.is_open("/trash/data"));
        }
        disk.fail = true;
        assert_eq!(
            files.transaction(
                &mut disk,
                generation,
                &[
                    Mutation::Rename {
                        old: "/native/source",
                        new: "/native/target",
                        replace: true
                    },
                    Mutation::Create {
                        path: "/trash/meta",
                        bytes: b"record"
                    }
                ]
            ),
            Err(Error::Io)
        );
        assert_eq!(files.inode(source), source_inode);
        assert_eq!(files.inode(target), target_inode);
        assert_eq!(files.info(source), Ok((6, 6)));
        assert_eq!(files.info(target), Ok((6, 6)));
        assert_eq!(disk.files, before);
        let calls = disk.transaction_calls;
        assert_eq!(
            files.transaction(
                &mut disk,
                generation,
                &[
                    Mutation::Remove {
                        path: "/native/source"
                    },
                    Mutation::Create {
                        path: "/../bad",
                        bytes: b"bad"
                    }
                ]
            ),
            Err(Error::InvalidPath)
        );
        assert_eq!(disk.transaction_calls, calls);
        assert!(files.is_open("/native/source"));
    }
}
