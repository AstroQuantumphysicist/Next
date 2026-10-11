//! Bounded, transactional supervisor-only AP roots. Table storage is allocated
//! on demand. A retained root has no release API: after publication/quarantine
//! its physical pages must never return to an allocator.
pub const PAGE: u64 = 4096;
pub const NX: u64 = 1 << 63;
pub const MAX_TABLES: usize = 32;
pub const MAX_REGIONS: usize = 12;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Layout,
    Alias,
    Bounds,
    Allocation,
    Storage,
    Mapping,
    State,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    ReadExecute,
    ReadOnly,
    ReadWrite,
    MmioReadOnly,
}
impl Permission {
    const fn flags(self) -> u64 {
        match self {
            Self::ReadExecute => 1,
            Self::ReadOnly => 1 | NX,
            Self::ReadWrite => 3 | NX,
            Self::MmioReadOnly => 0x19 | NX,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub permission: Permission,
}
#[derive(Clone)]
pub struct Layout<'a> {
    pub regions: &'a [Region],
    pub guards: &'a [u64],
    pub stack: core::ops::Range<u64>,
}
/// Backend owns every allocated page exclusively. Read/write must reject
/// anything outside that live allocation set; failures never publish a root.
pub trait Storage {
    fn allocate(&mut self) -> Result<u64, Error>;
    fn release(&mut self, address: u64);
    fn read(&self, page: u64, index: usize) -> Result<u64, Error>;
    fn write(&mut self, page: u64, index: usize, value: u64) -> Result<(), Error>;
}
pub struct RetainedRoot {
    root: u64,
    pages: [u64; MAX_TABLES],
    count: usize,
}
impl RetainedRoot {
    pub fn address(&self) -> u64 {
        self.root
    }
    pub fn table_pages(&self) -> &[u64] {
        &self.pages[..self.count]
    }
}
fn valid_page(address: u64) -> bool {
    (0x100000..0x1_0000_0000).contains(&address) && address.is_multiple_of(PAGE)
}
fn validate(layout: &Layout<'_>) -> Result<(), Error> {
    if layout.regions.is_empty()
        || layout.regions.len() > MAX_REGIONS
        || layout.guards.len() > 4
        || layout.stack.start < PAGE
        || layout.stack.start >= layout.stack.end
        || !layout.stack.start.is_multiple_of(PAGE)
        || !layout.stack.end.is_multiple_of(PAGE)
    {
        return Err(Error::Layout);
    }
    let total = layout.regions.iter().try_fold(0u64, |total, r| {
        total.checked_add(r.end.checked_sub(r.start)?)
    });
    if total.is_none_or(|bytes| bytes > 8192 * PAGE) {
        return Err(Error::Bounds);
    }
    let mut stack_found = false;
    for (i, r) in layout.regions.iter().enumerate() {
        if r.start == 0
            || r.start >= r.end
            || r.end > 0x1_0000_0000
            || !r.start.is_multiple_of(PAGE)
            || !r.end.is_multiple_of(PAGE)
        {
            return Err(Error::Bounds);
        }
        if layout.regions[..i]
            .iter()
            .any(|o| r.start < o.end && o.start < r.end)
        {
            return Err(Error::Alias);
        }
        if r.start == layout.stack.start
            && r.end == layout.stack.end
            && r.permission == Permission::ReadWrite
        {
            stack_found = true;
        }
        if layout
            .guards
            .iter()
            .any(|g| !g.is_multiple_of(PAGE) || r.start <= *g && *g < r.end)
        {
            return Err(Error::Layout);
        }
    }
    if !stack_found
        || !layout.guards.contains(&(layout.stack.start - PAGE))
        || !layout.guards.contains(&layout.stack.end)
    {
        return Err(Error::Layout);
    }
    Ok(())
}
struct Builder<'a, S: Storage> {
    storage: &'a mut S,
    pages: [u64; MAX_TABLES],
    count: usize,
}
impl<S: Storage> Builder<'_, S> {
    fn allocate(&mut self) -> Result<u64, Error> {
        if self.count == MAX_TABLES {
            return Err(Error::Allocation);
        }
        let address = self.storage.allocate()?;
        // Invalid/duplicate returned storage cannot be safely freed twice.
        if self.pages[..self.count].contains(&address) {
            return Err(Error::Storage);
        }
        if !valid_page(address) {
            self.storage.release(address);
            return Err(Error::Storage);
        }
        self.pages[self.count] = address;
        self.count += 1;
        for i in 0..512 {
            self.storage.write(address, i, 0)?;
        }
        Ok(address)
    }
    fn owned(&self, address: u64) -> bool {
        self.pages[..self.count].contains(&address)
    }
    fn child(&mut self, parent: u64, index: usize) -> Result<u64, Error> {
        let value = self.storage.read(parent, index)?;
        if value == 0 {
            let address = self.allocate()?;
            self.storage.write(parent, index, address | 3)?;
            Ok(address)
        } else {
            let address = value & !0xfff;
            if value & 0xfff != 3 || !self.owned(address) {
                return Err(Error::Mapping);
            }
            Ok(address)
        }
    }
    fn map(&mut self, root: u64, address: u64, flags: u64) -> Result<(), Error> {
        let pdpt = self.child(root, 0)?;
        let pd = self.child(pdpt, ((address >> 30) & 511) as usize)?;
        let pt = self.child(pd, ((address >> 21) & 511) as usize)?;
        let index = ((address >> 12) & 511) as usize;
        let prior = self.storage.read(pt, index)?;
        if prior != 0 && prior != address | flags {
            return Err(Error::Alias);
        }
        self.storage.write(pt, index, address | flags)
    }
    fn rollback(&mut self) {
        for page in self.pages[..self.count].iter().rev() {
            self.storage.release(*page);
        }
        self.count = 0;
    }
}
/// Builds exact identity mappings, with all unspecified addresses absent.
/// Tables themselves acquire supervisor RO/NX identity leaves, preventing the
/// AP from rewriting its own page tables. No shared BSP root/subtree is used.
pub fn stage<S: Storage>(storage: &mut S, layout: &Layout<'_>) -> Result<RetainedRoot, Error> {
    validate(layout)?;
    let mut b = Builder {
        storage,
        pages: [0; MAX_TABLES],
        count: 0,
    };
    let result = (|| {
        let root = b.allocate()?;
        for region in layout.regions {
            for address in (region.start..region.end).step_by(PAGE as usize) {
                b.map(root, address, region.permission.flags())?;
            }
        }
        // Mapping table pages can itself allocate new tables. Bounded fixed
        // point traversal includes every newly acquired page before publication.
        let mut index = 0;
        while index < b.count {
            let page = b.pages[index];
            if layout
                .regions
                .iter()
                .any(|r| r.start <= page && page < r.end)
                || layout.guards.contains(&page)
            {
                return Err(Error::Alias);
            }
            b.map(root, page, Permission::ReadOnly.flags())?;
            index += 1;
        }
        audit(b.storage, root, &b.pages[..b.count], layout)?;
        Ok(RetainedRoot {
            root,
            pages: b.pages,
            count: b.count,
        })
    })();
    if result.is_err() {
        b.rollback();
    }
    result
}
fn expected(layout: &Layout<'_>, pages: &[u64], address: u64) -> Option<u64> {
    layout
        .regions
        .iter()
        .find(|r| r.start <= address && address < r.end)
        .map(|r| r.permission.flags())
        .or_else(|| {
            pages
                .contains(&address)
                .then_some(Permission::ReadOnly.flags())
        })
}
/// Independent walk rejects user/huge/global/cache/reserved bits, foreign child
/// pages, aliases, unmapped requirements and extra mappings before publication.
#[allow(clippy::too_many_arguments)]
pub fn audit<S: Storage>(
    storage: &S,
    root: u64,
    pages: &[u64],
    layout: &Layout<'_>,
) -> Result<(), Error> {
    validate(layout)?;
    if pages.is_empty()
        || pages.len() > MAX_TABLES
        || !pages.contains(&root)
        || pages
            .iter()
            .enumerate()
            .any(|(i, p)| !valid_page(*p) || pages[..i].contains(p))
    {
        return Err(Error::Storage);
    }
    let mut leaves = 0usize;
    let mut visited = [0u64; MAX_TABLES];
    let mut visit_count = 0usize;
    #[allow(clippy::too_many_arguments)]
    fn walk<S: Storage>(
        storage: &S,
        page: u64,
        level: usize,
        base: u64,
        pages: &[u64],
        layout: &Layout<'_>,
        visited: &mut [u64; MAX_TABLES],
        count: &mut usize,
        leaves: &mut usize,
    ) -> Result<(), Error> {
        if visited[..*count].contains(&page) || *count == MAX_TABLES {
            return Err(Error::Alias);
        }
        visited[*count] = page;
        *count += 1;
        for index in 0..512 {
            let value = storage.read(page, index)?;
            if value == 0 {
                continue;
            }
            let address = base | ((index as u64) << (12 + 9 * (level - 1)));
            if level == 1 {
                let flags = expected(layout, pages, address).ok_or(Error::Mapping)?;
                if value != address | flags {
                    return Err(Error::Mapping);
                }
                *leaves += 1;
            } else {
                let child = value & !0xfff;
                if value & 0xfff != 3 || !pages.contains(&child) || address >= 0x1_0000_0000 {
                    return Err(Error::Mapping);
                }
                walk(
                    storage,
                    child,
                    level - 1,
                    address,
                    pages,
                    layout,
                    visited,
                    count,
                    leaves,
                )?;
            }
        }
        Ok(())
    }
    walk(
        storage,
        root,
        4,
        0,
        pages,
        layout,
        &mut visited,
        &mut visit_count,
        &mut leaves,
    )?;
    let required: usize = layout
        .regions
        .iter()
        .map(|r| ((r.end - r.start) / PAGE) as usize)
        .sum::<usize>()
        + pages.len();
    if leaves != required || visit_count != pages.len() {
        return Err(Error::Mapping);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{boxed::Box, collections::BTreeMap};
    struct Memory {
        tables: BTreeMap<u64, Box<[u64; 512]>>,
        next: u64,
        allocated: usize,
        released: usize,
        writes: usize,
        fail_alloc: Option<usize>,
        fail_write: Option<usize>,
    }
    impl Memory {
        fn new() -> Self {
            Self {
                tables: BTreeMap::new(),
                next: 0x2000000,
                allocated: 0,
                released: 0,
                writes: 0,
                fail_alloc: None,
                fail_write: None,
            }
        }
    }
    impl Storage for Memory {
        fn allocate(&mut self) -> Result<u64, Error> {
            if self.fail_alloc == Some(self.allocated) {
                return Err(Error::Allocation);
            }
            let page = self.next;
            self.next += PAGE;
            self.allocated += 1;
            assert!(self
                .tables
                .insert(page, Box::new([u64::MAX; 512]))
                .is_none());
            Ok(page)
        }
        fn release(&mut self, address: u64) {
            assert!(self.tables.remove(&address).is_some());
            self.released += 1;
        }
        fn read(&self, page: u64, index: usize) -> Result<u64, Error> {
            self.tables
                .get(&page)
                .and_then(|t| t.get(index))
                .copied()
                .ok_or(Error::Storage)
        }
        fn write(&mut self, page: u64, index: usize, value: u64) -> Result<(), Error> {
            if self.fail_write == Some(self.writes) {
                return Err(Error::Storage);
            }
            self.writes += 1;
            let t = self.tables.get_mut(&page).ok_or(Error::Storage)?;
            *t.get_mut(index).ok_or(Error::Storage)? = value;
            Ok(())
        }
    }
    fn regions() -> [Region; 6] {
        [
            Region {
                start: 0x80000,
                end: 0x81000,
                permission: Permission::ReadExecute,
            },
            Region {
                start: 0x81000,
                end: 0x82000,
                permission: Permission::ReadWrite,
            },
            Region {
                start: 0x82000,
                end: 0x83000,
                permission: Permission::ReadOnly,
            },
            Region {
                start: 0x4000000,
                end: 0x4002000,
                permission: Permission::ReadExecute,
            },
            Region {
                start: 0x4002000,
                end: 0x4003000,
                permission: Permission::ReadOnly,
            },
            Region {
                start: 0x4011000,
                end: 0x4021000,
                permission: Permission::ReadWrite,
            },
        ]
    }
    fn layout<'a>(rs: &'a [Region], guards: &'a [u64]) -> Layout<'a> {
        Layout {
            regions: rs,
            guards,
            stack: 0x4011000..0x4021000,
        }
    }
    fn leaf_location(m: &Memory, root: u64, address: u64) -> (u64, usize) {
        let pdpt = m.read(root, 0).unwrap() & !0xfff;
        let pd = m.read(pdpt, ((address >> 30) & 511) as usize).unwrap() & !0xfff;
        let pt = m.read(pd, ((address >> 21) & 511) as usize).unwrap() & !0xfff;
        (pt, ((address >> 12) & 511) as usize)
    }
    #[test]
    fn exact_identity_permissions_guards_and_no_shared_subtrees() {
        let rs = regions();
        let guards = [0x4010000, 0x4021000];
        let l = layout(&rs, &guards);
        let mut memory = Memory::new();
        let root = stage(&mut memory, &l).unwrap();
        audit(&memory, root.address(), root.table_pages(), &l).unwrap();
        for r in rs {
            for address in (r.start..r.end).step_by(4096) {
                let (pt, index) = leaf_location(&memory, root.address(), address);
                assert_eq!(memory.read(pt, index), Ok(address | r.permission.flags()));
            }
        }
        for guard in guards {
            let (pt, index) = leaf_location(&memory, root.address(), guard);
            assert_eq!(memory.read(pt, index), Ok(0));
        }
        for page in root.table_pages() {
            let (pt, index) = leaf_location(&memory, root.address(), *page);
            assert_eq!(memory.read(pt, index), Ok(*page | NX | 1));
        }
        let second = stage(&mut memory, &l).unwrap();
        assert!(root
            .table_pages()
            .iter()
            .all(|p| !second.table_pages().contains(p)));
        assert_ne!(root.address(), second.address());
        assert_eq!(memory.released, 0);
    }
    #[test]
    fn every_allocation_failure_and_write_phase_rolls_back() {
        let rs = regions();
        let guards = [0x4010000, 0x4021000];
        let l = layout(&rs, &guards);
        let mut baseline = Memory::new();
        let root = stage(&mut baseline, &l).unwrap();
        for fail in 0..root.table_pages().len() {
            let mut m = Memory::new();
            m.fail_alloc = Some(fail);
            assert!(stage(&mut m, &l).is_err());
            assert!(m.tables.is_empty());
            assert_eq!(m.allocated, m.released);
        }
        // Fail each actual initialization, parent-link and leaf write in turn.
        for fail in 0..baseline.writes {
            let mut m = Memory::new();
            m.fail_write = Some(fail);
            assert!(stage(&mut m, &l).is_err());
            assert!(m.tables.is_empty());
            assert_eq!(m.allocated, m.released);
        }
    }
    #[test]
    fn independent_audit_rejects_user_wx_alias_extra_and_missing_leaves() {
        let rs = regions();
        let guards = [0x4010000, 0x4021000];
        let l = layout(&rs, &guards);
        let mut m = Memory::new();
        let root = stage(&mut m, &l).unwrap();
        let (pt, index) = leaf_location(&m, root.address(), 0x80000);
        for value in [
            0,
            0x80000 | 3,
            0x80000 | 5,
            0x81000 | 1,
            0x80000 | NX | 1,
            0x80000 | 0x101,
        ] {
            m.write(pt, index, value).unwrap();
            assert_eq!(
                audit(&m, root.address(), root.table_pages(), &l),
                Err(Error::Mapping)
            );
        }
        m.write(pt, index, 0x80001).unwrap();
        m.write(pt, 1, 0x1000 | NX | 1).unwrap();
        assert_eq!(
            audit(&m, root.address(), root.table_pages(), &l),
            Err(Error::Mapping)
        );
        m.write(pt, 1, 0).unwrap();
        let child = m.read(root.address(), 0).unwrap();
        m.write(root.address(), 0, child | 4).unwrap();
        assert_eq!(
            audit(&m, root.address(), root.table_pages(), &l),
            Err(Error::Mapping)
        );
    }
    #[test]
    fn table_owner_and_recursive_walk_reject_overlap_cycles_and_foreign_storage() {
        let rs = regions();
        let guards = [0x4010000, 0x4021000];
        let l = layout(&rs, &guards);
        let mut m = Memory::new();
        m.next = 0x4000000;
        assert!(stage(&mut m, &l).is_err());
        assert!(m.tables.is_empty());
        let mut m = Memory::new();
        m.next = 0x80000;
        assert!(stage(&mut m, &l).is_err());
        assert!(m.tables.is_empty());
        let mut m = Memory::new();
        let root = stage(&mut m, &l).unwrap();
        m.write(root.address(), 0, root.address() | 3).unwrap();
        assert_eq!(
            audit(&m, root.address(), root.table_pages(), &l),
            Err(Error::Alias)
        );
        m.write(root.address(), 0, 0x3000000 | 3).unwrap();
        assert_eq!(
            audit(&m, root.address(), root.table_pages(), &l),
            Err(Error::Mapping)
        );
    }
    #[test]
    fn overlapping_unbounded_and_missing_guard_layouts_allocate_nothing() {
        let mut rs = regions();
        let guards = [0x4010000, 0x4021000];
        let mut m = Memory::new();
        rs[2].start = 0x81000;
        assert!(stage(&mut m, &layout(&rs, &guards)).is_err());
        assert_eq!(m.allocated, 0);
        rs = regions();
        rs[3].end = 0x80000000;
        assert!(stage(&mut m, &layout(&rs, &guards)).is_err());
        assert_eq!(m.allocated, 0);
        rs = regions();
        assert!(stage(&mut m, &layout(&rs, &guards[..1])).is_err());
        assert_eq!(m.allocated, 0);
    }
}
