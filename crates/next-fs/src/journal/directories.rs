//! Explicit bounded directories over the current COW Store.
//!
//! The caller supplies trusted owner authority and an exact namespace generation.
//! Checksummed metadata is validated but never grants authority. This storage API
//! does not by itself provide VFS cache, open-inode, or UI publication semantics.
use super::transaction::{Mutation, Receipt, TransactionError};
use super::{FileInfo, Store, WritableBlockDevice, MAX_FILES};
const RECORD: usize = 96;
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Store(super::Error<E>),
    Path,
    Owner,
    Corrupt,
    MissingParent,
    NotDirectory,
    NotFound,
    Exists,
    NotEmpty,
    Full,
    Conflict,
}
impl<E> From<super::Error<E>> for Error<E> {
    fn from(e: super::Error<E>) -> Self {
        Self::Store(e)
    }
}
impl<E> From<TransactionError<E>> for Error<E> {
    fn from(error: TransactionError<E>) -> Self {
        match error {
            TransactionError::Conflict => Self::Conflict,
            TransactionError::Count => Self::Corrupt,
            TransactionError::Journal(e) => Self::Store(e),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    name: [u8; 31],
    len: u8,
    pub kind: Kind,
    pub size: u32,
    pub id: u64,
}
impl Entry {
    const EMPTY: Self = Self {
        name: [0; 31],
        len: 0,
        kind: Kind::File,
        size: 0,
        id: 0,
    };
    fn new(path: &str, kind: Kind, size: u32, id: u64) -> Self {
        let mut e = Self {
            kind,
            size,
            id,
            ..Self::EMPTY
        };
        e.len = path.len() as u8;
        e.name[..path.len()].copy_from_slice(path.as_bytes());
        e
    }
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.name[..self.len as usize]).unwrap_or("")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listing {
    pub folder: Entry,
    pub generation: u64,
    pub entries: [Entry; MAX_FILES],
    pub count: usize,
}
#[derive(Clone, Copy)]
struct Node {
    entry: Entry,
    key: [u8; 24],
    key_len: u8,
}
impl Node {
    const EMPTY: Self = Self {
        entry: Entry::EMPTY,
        key: [0; 24],
        key_len: 0,
    };
    fn key(&self) -> &str {
        core::str::from_utf8(&self.key[..self.key_len as usize]).unwrap_or("")
    }
}
fn valid(path: &str) -> bool {
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
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|p| p.0).unwrap_or("")
}
fn key(id: u64) -> [u8; 24] {
    let mut out = *b"/dirs/m.0000000000000000";
    let hex = b"0123456789abcdef";
    for i in 0..16 {
        out[8 + i] = hex[((id >> ((15 - i) * 4)) & 15) as usize];
    }
    out
}
fn key_id(path: &str) -> Option<u64> {
    let raw = path.as_bytes();
    if raw.len() != 24 || &raw[..8] != b"/dirs/m." {
        return None;
    }
    let mut id = 0u64;
    for b in &raw[8..] {
        let n = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            _ => return None,
        };
        id = (id << 4) | u64::from(n);
    }
    if id == 0 {
        None
    } else {
        Some(id)
    }
}
fn encode(owner: u64, id: u64, path: &str) -> [u8; RECORD] {
    let mut out = [0; RECORD];
    out[..8].copy_from_slice(b"NXTDIR01");
    out[8..10].copy_from_slice(&1u16.to_le_bytes());
    out[10..12].copy_from_slice(&(RECORD as u16).to_le_bytes());
    out[16..24].copy_from_slice(&owner.to_le_bytes());
    out[24..32].copy_from_slice(&id.to_le_bytes());
    out[32] = path.len() as u8;
    out[33..33 + path.len()].copy_from_slice(path.as_bytes());
    let crc = super::crc32(&out[..92]);
    out[92..].copy_from_slice(&crc.to_le_bytes());
    out
}
fn decode<E>(raw: &[u8], owner: u64, id: u64) -> Result<Entry, Error<E>> {
    if raw.len() != RECORD
        || &raw[..8] != b"NXTDIR01"
        || raw[8..10] != 1u16.to_le_bytes()
        || raw[10..12] != (RECORD as u16).to_le_bytes()
        || raw[12..16].iter().any(|b| *b != 0)
        || raw[64..92].iter().any(|b| *b != 0)
        || u64::from_le_bytes(raw[16..24].try_into().unwrap()) != owner
        || u64::from_le_bytes(raw[24..32].try_into().unwrap()) != id
        || super::crc32(&raw[..92]) != u32::from_le_bytes(raw[92..].try_into().unwrap())
    {
        return Err(Error::Corrupt);
    }
    let n = raw[32] as usize;
    if n > 31 || raw[33 + n..64].iter().any(|b| *b != 0) {
        return Err(Error::Corrupt);
    }
    let path = core::str::from_utf8(&raw[33..33 + n]).map_err(|_| Error::Corrupt)?;
    if !valid(path) {
        return Err(Error::Corrupt);
    }
    Ok(Entry::new(path, Kind::Directory, 0, id))
}
struct Namespace {
    nodes: [Node; MAX_FILES],
    count: usize,
    used: usize,
}
impl Namespace {
    fn find(&self, path: &str) -> Option<Node> {
        self.nodes[..self.count]
            .iter()
            .copied()
            .find(|n| n.entry.path() == path)
    }
    fn directory<E>(&self, path: &str) -> Result<(), Error<E>> {
        if path == "/native" {
            return Ok(());
        }
        match self.find(path) {
            Some(n) if n.entry.kind == Kind::Directory => Ok(()),
            Some(_) => Err(Error::<E>::NotDirectory),
            None => Err(Error::<E>::MissingParent),
        }
    }
    fn vacant<E>(&self, path: &str) -> Result<(), Error<E>> {
        if self.find(path).is_some() {
            Err(Error::<E>::Exists)
        } else {
            Ok(())
        }
    }
    fn empty<E>(&self, path: &str) -> Result<(), Error<E>> {
        if self.nodes[..self.count]
            .iter()
            .any(|n| parent(n.entry.path()) == path)
        {
            Err(Error::<E>::NotEmpty)
        } else {
            Ok(())
        }
    }
}
fn scan<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
) -> Result<Namespace, Error<D::Error>> {
    if owner == 0 {
        return Err(Error::Owner);
    }
    let mut infos = [FileInfo::EMPTY; MAX_FILES];
    let used = store.list(&mut infos)?;
    let mut ns = Namespace {
        nodes: [Node::EMPTY; MAX_FILES],
        count: 0,
        used,
    };
    let mut bytes = [0u8; 4096];
    for info in &infos[..used] {
        let path = info.path();
        if path == "/native" {
            return Err(Error::Corrupt);
        }
        let directory = path == "/dirs" || path.starts_with("/dirs/");
        if !directory && !path.starts_with("/native/") {
            continue;
        }
        let len = store.read_file(path, &mut bytes)?;
        let mut node = Node::EMPTY;
        node.entry = if directory {
            let id = key_id(path).ok_or(Error::Corrupt)?;
            node.key = key(id);
            node.key_len = 24;
            decode(&bytes[..len], owner, id)?
        } else {
            if !valid(path) {
                return Err(Error::Corrupt);
            }
            Entry::new(path, Kind::File, info.size, 0)
        };
        if ns.find(node.entry.path()).is_some() {
            return Err(Error::Corrupt);
        }
        ns.nodes[ns.count] = node;
        ns.count += 1;
    }
    // Explicit parents are mandatory. Existing opaque pref/Trash objects are not
    // interpreted as directories and never grant ancestry or namespace rights.
    for node in &ns.nodes[..ns.count] {
        ns.directory(parent(node.entry.path()))?;
    }
    Ok(ns)
}
fn preflight<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    generation: u64,
) -> Result<Namespace, Error<D::Error>> {
    if store.generation() != generation {
        return Err(Error::Conflict);
    }
    scan(store, owner)
}
fn checked<E>(path: &str) -> Result<(), Error<E>> {
    if valid(path) {
        Ok(())
    } else {
        Err(Error::<E>::Path)
    }
}
pub fn list<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    path: &str,
) -> Result<Listing, Error<D::Error>> {
    if path != "/native" {
        checked(path)?;
    }
    let ns = scan(store, owner)?;
    let folder = if path == "/native" {
        Entry::new("/native", Kind::Directory, 0, 0)
    } else {
        ns.directory(parent(path))?;
        let entry = ns.find(path).ok_or(Error::NotFound)?.entry;
        if entry.kind != Kind::Directory {
            return Err(Error::NotDirectory);
        }
        entry
    };
    let mut result = Listing {
        folder,
        generation: store.generation(),
        entries: [Entry::EMPTY; MAX_FILES],
        count: 0,
    };
    for n in &ns.nodes[..ns.count] {
        if parent(n.entry.path()) == path {
            let mut i = result.count;
            while i > 0 && result.entries[i - 1].path() > n.entry.path() {
                result.entries[i] = result.entries[i - 1];
                i -= 1;
            }
            result.entries[i] = n.entry;
            result.count += 1;
        }
    }
    Ok(result)
}
pub fn mkdir<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    generation: u64,
    path: &str,
) -> Result<Receipt, Error<D::Error>> {
    checked(path)?;
    let ns = preflight(store, owner, generation)?;
    ns.directory(parent(path))?;
    ns.vacant(path)?;
    if ns.used == MAX_FILES {
        return Err(Error::Full);
    }
    let id = generation.checked_add(1).ok_or(Error::Conflict)?;
    let private = key(id);
    let private = core::str::from_utf8(&private).unwrap();
    let raw = encode(owner, id, path);
    Ok(store.directory_transaction(
        generation,
        &[Mutation::Create {
            path: private,
            bytes: &raw,
        }],
    )?)
}
pub fn create_file<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    generation: u64,
    path: &str,
    bytes: &[u8],
) -> Result<Receipt, Error<D::Error>> {
    checked(path)?;
    let ns = preflight(store, owner, generation)?;
    ns.directory(parent(path))?;
    ns.vacant(path)?;
    if ns.used == MAX_FILES {
        return Err(Error::Full);
    }
    Ok(store.directory_transaction(generation, &[Mutation::Create { path, bytes }])?)
}
pub fn remove_file<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    generation: u64,
    path: &str,
) -> Result<Receipt, Error<D::Error>> {
    checked(path)?;
    let ns = preflight(store, owner, generation)?;
    let n = ns.find(path).ok_or(Error::NotFound)?;
    if n.entry.kind != Kind::File {
        return Err(Error::<D::Error>::NotDirectory);
    }
    Ok(store.directory_transaction(generation, &[Mutation::Remove { path }])?)
}
pub fn rmdir<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    generation: u64,
    path: &str,
) -> Result<Receipt, Error<D::Error>> {
    checked(path)?;
    let ns = preflight(store, owner, generation)?;
    let n = ns.find(path).ok_or(Error::NotFound)?;
    if n.entry.kind != Kind::Directory {
        return Err(Error::<D::Error>::NotDirectory);
    }
    ns.empty(path)?;
    Ok(store.directory_transaction(generation, &[Mutation::Remove { path: n.key() }])?)
}
pub fn rename_empty<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    generation: u64,
    old: &str,
    new: &str,
) -> Result<Receipt, Error<D::Error>> {
    checked(old)?;
    checked(new)?;
    if new
        .strip_prefix(old)
        .is_some_and(|suffix| suffix.starts_with('/'))
    {
        return Err(Error::<D::Error>::Path);
    }
    let ns = preflight(store, owner, generation)?;
    let n = ns.find(old).ok_or(Error::NotFound)?;
    if n.entry.kind != Kind::Directory {
        return Err(Error::<D::Error>::NotDirectory);
    }
    ns.empty(old)?;
    ns.directory(parent(new))?;
    ns.vacant(new)?;
    let raw = encode(owner, n.entry.id, new);
    Ok(store.directory_transaction(
        generation,
        &[Mutation::Replace {
            path: n.key(),
            bytes: &raw,
        }],
    )?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderSnapshot {
    pub folder: Entry,
    pub generation: u64,
}
pub fn snapshot<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    path: &str,
) -> Result<FolderSnapshot, Error<D::Error>> {
    let listing = list(store, owner, path)?;
    Ok(FolderSnapshot {
        folder: listing.folder,
        generation: listing.generation,
    })
}
fn validate_snapshot<E>(ns: &Namespace, snapshot: FolderSnapshot) -> Result<(), Error<E>> {
    let path = snapshot.folder.path();
    ns.directory(path)?;
    let actual = if path == "/native" {
        Entry::new("/native", Kind::Directory, 0, 0)
    } else {
        ns.find(path).ok_or(Error::NotFound)?.entry
    };
    if actual != snapshot.folder {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub fn mkdir_at<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    parent_snapshot: FolderSnapshot,
    path: &str,
) -> Result<Receipt, Error<D::Error>> {
    checked(path)?;
    let ns = preflight(store, owner, parent_snapshot.generation)?;
    validate_snapshot(&ns, parent_snapshot)?;
    if parent(path) != parent_snapshot.folder.path() {
        return Err(Error::<D::Error>::Path);
    }
    mkdir(store, owner, parent_snapshot.generation, path)
}
pub fn rename_snapshot<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    source: FolderSnapshot,
    new: &str,
) -> Result<Receipt, Error<D::Error>> {
    let ns = preflight(store, owner, source.generation)?;
    validate_snapshot(&ns, source)?;
    rename_empty(store, owner, source.generation, source.folder.path(), new)
}

/// Removes the exact empty directory named by a current folder snapshot.
pub fn rmdir_snapshot<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    source: FolderSnapshot,
) -> Result<Receipt, Error<D::Error>> {
    let ns = preflight(store, owner, source.generation)?;
    validate_snapshot(&ns, source)?;
    if source.folder.path() == "/native" {
        return Err(Error::Path);
    }
    let node = ns.find(source.folder.path()).ok_or(Error::NotFound)?;
    if node.entry.kind != Kind::Directory {
        return Err(Error::NotDirectory);
    }
    ns.empty(source.folder.path())?;
    Ok(store.directory_transaction(source.generation, &[Mutation::Remove { path: node.key() }])?)
}

/// Renames an empty directory while binding both source and destination parent
/// to the same trusted generation and immutable directory identities.
pub fn rename_between_snapshots<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    owner: u64,
    source: FolderSnapshot,
    destination_parent: FolderSnapshot,
    new: &str,
) -> Result<Receipt, Error<D::Error>> {
    if source.generation != destination_parent.generation {
        return Err(Error::Conflict);
    }
    checked(new)?;
    if parent(new) != destination_parent.folder.path() {
        return Err(Error::Path);
    }
    let ns = preflight(store, owner, source.generation)?;
    validate_snapshot(&ns, source)?;
    validate_snapshot(&ns, destination_parent)?;
    if source.folder.path() == "/native" {
        return Err(Error::Path);
    }
    let node = ns.find(source.folder.path()).ok_or(Error::NotFound)?;
    if node.entry.kind != Kind::Directory {
        return Err(Error::NotDirectory);
    }
    if new
        .strip_prefix(source.folder.path())
        .is_some_and(|suffix| suffix.starts_with('/'))
    {
        return Err(Error::Path);
    }
    ns.empty(source.folder.path())?;
    ns.vacant(new)?;
    let raw = encode(owner, node.entry.id, new);
    Ok(store.directory_transaction(
        source.generation,
        &[Mutation::Replace {
            path: node.key(),
            bytes: &raw,
        }],
    )?)
}
