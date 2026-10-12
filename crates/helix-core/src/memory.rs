//! Physical-page bookkeeping for a bounded contiguous window.
//! Reserved/firmware/absent pages never enter the free pool. The caller must
//! zero pages before user mapping and exclude live boot/allocator metadata.
//!
//! `pin`/`unpin` form the core lifetime contract for asynchronous users such as
//! DMA: every submitted operation must pin its allocated page before handing it
//! to a device, then unpin exactly once after completion or cancellation has
//! stopped device access; a synchronous submission failure must roll its pin
//! back immediately. Free and permanent-retain operations fail while any pin
//! remains. A leaked pin therefore withholds a page instead of allowing a
//! use-after-free. All operations require exclusive allocator access, so the
//! kernel integration must serialize allocator metadata updates. Existing
//! virtio and AC97 paths use statically owned DMA storage and do not yet route
//! their buffers through this allocator contract.
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
    Pinned,
    NotPinned,
    PinCountOverflow,
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
    /// References held by submitted asynchronous I/O. The fixed width makes
    /// both the metadata and the maximum number of simultaneous pins bounded.
    pins: [u16; N],
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
            pins: [0; N],
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
        debug_assert_eq!(self.pins[index], 0);
        Ok(self.base + index as u64 * PAGE_SIZE)
    }
    /// Record one in-flight asynchronous user of an allocated page.
    ///
    /// Call this before publishing the page address to a device. The caller
    /// must call `unpin` only once it has established that the device can no
    /// longer access the page. Pins are counted so independent operations may
    /// safely share a page until all of them complete.
    pub fn pin(&mut self, address: u64) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated {
            return Err(MemoryError::NotAllocated);
        }
        self.pins[index] = self.pins[index]
            .checked_add(1)
            .ok_or(MemoryError::PinCountOverflow)?;
        Ok(())
    }
    /// Release one completed/cancelled asynchronous use of an allocated page.
    pub fn unpin(&mut self, address: u64) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated || self.pins[index] == 0 {
            return Err(MemoryError::NotPinned);
        }
        self.pins[index] -= 1;
        Ok(())
    }
    /// Number of outstanding asynchronous users currently recorded for a page.
    pub fn pin_count(&self, address: u64) -> Result<u16, MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated {
            return Err(MemoryError::NotAllocated);
        }
        Ok(self.pins[index])
    }
    /// Permanently retain an allocated page. Published page-table/quarantine
    /// ownership cannot be undone with `free`, including by its original owner.
    pub fn retain(&mut self, address: u64) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated {
            return Err(MemoryError::NotAllocated);
        }
        if self.pins[index] != 0 {
            return Err(MemoryError::Pinned);
        }
        self.states[index] = PageState::Reserved;
        Ok(())
    }
    pub fn free(&mut self, address: u64) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        if self.states[index] != PageState::Allocated {
            return Err(MemoryError::NotAllocated);
        }
        if self.pins[index] != 0 {
            return Err(MemoryError::Pinned);
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
    fn one_in_flight_pin_blocks_free_and_reuse_until_completion() {
        let mut pages = PageAllocator::<2>::new(0x100000, &[usable(0x100000, 2)]).unwrap();
        let submitted = pages.allocate().unwrap();

        // Submission takes ownership before the address is handed to I/O.
        pages.pin(submitted).unwrap();
        assert_eq!(pages.pin_count(submitted), Ok(1));
        assert_eq!(pages.free(submitted), Err(MemoryError::Pinned));
        assert_eq!(pages.retain(submitted), Err(MemoryError::Pinned));
        assert_eq!(pages.reserve(submitted, 1), Err(MemoryError::Unavailable));
        assert_eq!(pages.pin_count(submitted), Ok(1));

        // Delayed completion: allocator pressure must not recycle this page.
        let other = pages.allocate().unwrap();
        assert_ne!(other, submitted);
        assert_eq!(pages.allocate(), Err(MemoryError::Exhausted));
        pages.free(other).unwrap();
        assert_eq!(pages.allocate(), Ok(other));

        // The completion path releases ownership, after which ordinary free
        // and reuse are permitted again.
        pages.unpin(submitted).unwrap();
        assert_eq!(pages.pin_count(submitted), Ok(0));
        pages.free(submitted).unwrap();
        assert_eq!(pages.allocate(), Ok(submitted));
    }
    #[test]
    fn multiple_in_flight_uses_require_matching_completions() {
        let mut pages = PageAllocator::<1>::new(0x200000, &[usable(0x200000, 1)]).unwrap();
        let address = pages.allocate().unwrap();
        pages.pin(address).unwrap();
        pages.pin(address).unwrap();
        assert_eq!(pages.pin_count(address), Ok(2));

        // Completion of one request cannot release a page still owned by the
        // other request.
        pages.unpin(address).unwrap();
        assert_eq!(pages.pin_count(address), Ok(1));
        assert_eq!(pages.free(address), Err(MemoryError::Pinned));
        pages.unpin(address).unwrap();
        assert_eq!(pages.free(address), Ok(()));
    }
    #[test]
    fn unmatched_or_duplicate_completion_cannot_change_page_ownership() {
        let mut pages = PageAllocator::<1>::new(0x300000, &[usable(0x300000, 1)]).unwrap();
        let address = pages.allocate().unwrap();
        assert_eq!(pages.unpin(address), Err(MemoryError::NotPinned));
        pages.pin(address).unwrap();
        pages.unpin(address).unwrap();
        assert_eq!(pages.unpin(address), Err(MemoryError::NotPinned));
        assert_eq!(pages.pin_count(address), Ok(0));
        pages.free(address).unwrap();
        assert_eq!(pages.unpin(address), Err(MemoryError::NotPinned));
    }
    #[test]
    fn pin_counter_is_bounded_and_overflow_does_not_drop_existing_ownership() {
        let mut pages = PageAllocator::<1>::new(0x400000, &[usable(0x400000, 1)]).unwrap();
        let address = pages.allocate().unwrap();
        for _ in 0..u16::MAX {
            pages.pin(address).unwrap();
        }
        assert_eq!(pages.pin(address), Err(MemoryError::PinCountOverflow));
        assert_eq!(pages.pin_count(address), Ok(u16::MAX));
        assert_eq!(pages.free(address), Err(MemoryError::Pinned));
        for _ in 0..u16::MAX {
            pages.unpin(address).unwrap();
        }
        pages.free(address).unwrap();
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
