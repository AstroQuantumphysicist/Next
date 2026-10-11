//! Bootstrap physical-page bookkeeping for a bounded contiguous window.
//! Reserved/firmware/absent pages never enter the free pool. The caller must
//! zero pages before user mapping and exclude live boot/allocator metadata.
pub const PAGE_SIZE: u64 = 4096;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionKind {
    Usable,
    Reserved,
    AcpiReclaim,
    AcpiNvs,
    Mmio,
    FirmwareRuntime,
    Bad,
    LoaderOccupied,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub start: u64,
    pub pages: u64,
    pub kind: RegionKind,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryError {
    Unaligned,
    Overflow,
    Empty,
    Overlap,
    OutsideWindow,
    Unavailable,
    Exhausted,
    NotAllocated,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PageState {
    Reserved,
    Free,
    Allocated,
}
pub struct PageAllocator<const N: usize> {
    base: u64,
    states: [PageState; N],
}

fn end(start: u64, pages: u64) -> Result<u64, MemoryError> {
    if !start.is_multiple_of(PAGE_SIZE) {
        return Err(MemoryError::Unaligned);
    }
    if pages == 0 {
        return Err(MemoryError::Empty);
    }
    start
        .checked_add(pages.checked_mul(PAGE_SIZE).ok_or(MemoryError::Overflow)?)
        .ok_or(MemoryError::Overflow)
}
pub fn validate_regions(regions: &[Region]) -> Result<(), MemoryError> {
    for (index, region) in regions.iter().enumerate() {
        let finish = end(region.start, region.pages)?;
        for other in &regions[..index] {
            let other_end = end(other.start, other.pages)?;
            if region.start < other_end && other.start < finish {
                return Err(MemoryError::Overlap);
            }
        }
    }
    Ok(())
}
impl<const N: usize> PageAllocator<N> {
    pub fn new(base: u64, regions: &[Region]) -> Result<Self, MemoryError> {
        let window_end = end(base, N as u64)?;
        validate_regions(regions)?;
        let mut result = Self {
            base,
            states: [PageState::Reserved; N],
        };
        for region in regions {
            if region.kind != RegionKind::Usable {
                continue;
            }
            let start = region.start.max(base);
            let finish = end(region.start, region.pages)?.min(window_end);
            if start >= finish {
                continue;
            }
            let lo = ((start - base) / PAGE_SIZE) as usize;
            let hi = ((finish - base) / PAGE_SIZE) as usize;
            result.states[lo..hi].fill(PageState::Free);
        }
        Ok(result)
    }
    fn index(&self, address: u64) -> Result<usize, MemoryError> {
        if !address.is_multiple_of(PAGE_SIZE) {
            return Err(MemoryError::Unaligned);
        }
        let offset = address
            .checked_sub(self.base)
            .ok_or(MemoryError::OutsideWindow)?
            / PAGE_SIZE;
        if offset >= N as u64 {
            return Err(MemoryError::OutsideWindow);
        }
        Ok(offset as usize)
    }
    /// Atomic reservation: fails without changing any page if one is allocated.
    /// Already-reserved pages may be reserved again, but can never be freed.
    pub fn reserve(&mut self, start: u64, pages: u64) -> Result<(), MemoryError> {
        let finish = end(start, pages)?;
        let lo = self.index(start)?;
        let last = self.index(finish - PAGE_SIZE)?;
        if self.states[lo..=last].contains(&PageState::Allocated) {
            return Err(MemoryError::Unavailable);
        }
        self.states[lo..=last].fill(PageState::Reserved);
        Ok(())
    }
    pub fn allocate(&mut self) -> Result<u64, MemoryError> {
        let index = self
            .states
            .iter()
            .position(|s| *s == PageState::Free)
            .ok_or(MemoryError::Exhausted)?;
        self.states[index] = PageState::Allocated;
        Ok(self.base + index as u64 * PAGE_SIZE)
    }
    /// Permanently retain an allocated page. Published page-table/quarantine
    /// ownership cannot be undone with `free`, including by its original owner.
    pub fn retain(&mut self, address: u64) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated {
            return Err(MemoryError::NotAllocated);
        }
        self.states[index] = PageState::Reserved;
        Ok(())
    }
    pub fn free(&mut self, address: u64) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated {
            return Err(MemoryError::NotAllocated);
        }
        self.states[index] = PageState::Free;
        Ok(())
    }
    pub fn free_pages(&self) -> usize {
        self.states
            .iter()
            .filter(|s| **s == PageState::Free)
            .count()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn usable(start: u64, pages: u64) -> Region {
        Region {
            start,
            pages,
            kind: RegionKind::Usable,
        }
    }
    #[test]
    fn retained_published_pages_never_reenter_free_pool() {
        let mut pages = PageAllocator::<2>::new(0x100000, &[usable(0x100000, 2)]).unwrap();
        let first = pages.allocate().unwrap();
        pages.retain(first).unwrap();
        assert_eq!(pages.free(first), Err(MemoryError::NotAllocated));
        assert_eq!(pages.retain(first), Err(MemoryError::NotAllocated));
        let second = pages.allocate().unwrap();
        assert_ne!(first, second);
        assert_eq!(pages.allocate(), Err(MemoryError::Exhausted));
        pages.free(second).unwrap();
        assert_eq!(pages.allocate(), Ok(second));
    }
    #[test]
    fn rejects_hostile_maps() {
        assert_eq!(
            validate_regions(&[usable(1, 1)]),
            Err(MemoryError::Unaligned)
        );
        assert_eq!(
            validate_regions(&[usable(0, u64::MAX)]),
            Err(MemoryError::Overflow)
        );
        assert_eq!(
            validate_regions(&[usable(u64::MAX - 4095, 1)]),
            Err(MemoryError::Overflow)
        );
        assert_eq!(
            validate_regions(&[usable(0, 2), usable(4096, 1)]),
            Err(MemoryError::Overlap)
        );
        assert_eq!(validate_regions(&[usable(0, 0)]), Err(MemoryError::Empty));
    }
    #[test]
    fn reserves_and_detects_double_free() {
        let mut allocator = PageAllocator::<4>::new(4096, &[usable(0, 6)]).unwrap();
        allocator.reserve(8192, 1).unwrap();
        assert_eq!(allocator.free_pages(), 3);
        assert_eq!(allocator.free(8192), Err(MemoryError::NotAllocated));
        let address = allocator.allocate().unwrap();
        assert_eq!(address, 4096);
        assert_eq!(allocator.reserve(4096, 3), Err(MemoryError::Unavailable));
        assert_eq!(allocator.free_pages(), 2);
        allocator.free(address).unwrap();
        assert_eq!(allocator.free(address), Err(MemoryError::NotAllocated));
        assert_eq!(allocator.free(0), Err(MemoryError::OutsideWindow));
    }
    #[test]
    fn nonusable_memory_never_allocates() {
        let mut allocator = PageAllocator::<4>::new(
            0,
            &[
                usable(0, 1),
                Region {
                    start: 4096,
                    pages: 2,
                    kind: RegionKind::FirmwareRuntime,
                },
            ],
        )
        .unwrap();
        assert_eq!(allocator.allocate(), Ok(0));
        assert_eq!(allocator.allocate(), Err(MemoryError::Exhausted));
        assert_eq!(allocator.free(8192), Err(MemoryError::NotAllocated));
    }
}
