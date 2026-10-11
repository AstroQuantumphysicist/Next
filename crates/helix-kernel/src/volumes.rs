//! Read-only GPT discovery and FAT reads relative to the validated ESP.
//! This module has no formatting, repair, block-write or mount-write interface.
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use next_fs::{gpt, BlockDevice, FsError, Volume};

#[derive(Debug, PartialEq, Eq)]
pub enum ReadError<E> {
    Device(E),
    Bounds,
    BlockSize,
}

/// A relative read-only device whose every request is checked against its extent.
pub struct ReadExtent<D> {
    device: D,
    start: u64,
    count: u64,
}
impl<D: BlockDevice> ReadExtent<D> {
    pub fn new(device: D, start: u64, count: u64) -> Result<Self, ReadError<D::Error>> {
        if count == 0 || start.checked_add(count).is_none() || start + count > device.sector_count()
        {
            return Err(ReadError::Bounds);
        }
        Ok(Self {
            device,
            start,
            count,
        })
    }
}
impl<D: BlockDevice> BlockDevice for ReadExtent<D> {
    type Error = ReadError<D::Error>;
    fn sector_count(&self) -> u64 {
        self.count
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
        if lba >= self.count {
            return Err(ReadError::Bounds);
        }
        let absolute = self.start.checked_add(lba).ok_or(ReadError::Bounds)?;
        // Also defend against a backend whose media geometry changed after construction.
        if absolute >= self.device.sector_count() {
            return Err(ReadError::Bounds);
        }
        self.device
            .read_sector(absolute, out)
            .map_err(ReadError::Device)
    }
}

struct Reader<'a, D>(&'a mut D);
impl<D: BlockDevice> gpt::BlockReader for Reader<'_, D> {
    type Error = ReadError<D::Error>;
    fn block_size(&self) -> usize {
        512
    }
    fn block_count(&self) -> u64 {
        self.0.sector_count()
    }
    fn read_block(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Self::Error> {
        if lba >= self.0.sector_count() {
            return Err(ReadError::Bounds);
        }
        let output: &mut [u8; 512] = out.try_into().map_err(|_| ReadError::BlockSize)?;
        self.0.read_sector(lba, output).map_err(ReadError::Device)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryError<E> {
    Busy,
    Gpt(gpt::Error<ReadError<E>>),
    Extent(ReadError<E>),
    Fat(FsError<ReadError<E>>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub disk_guid: gpt::Guid,
    pub esp: gpt::Partition,
    pub data: gpt::Partition,
    pub recovery: Option<gpt::Partition>,
    pub note_bytes: usize,
}

/// Validate both GPT copies, select by unique type GUID, and read /NEXT.TXT.
/// Errors never fall back to a guessed volume or authorize mutation.
pub fn discover_with_scratch<D: BlockDevice>(
    mut device: D,
    out: &mut [u8],
    table: &mut gpt::Gpt,
    workspace: &mut gpt::Workspace,
) -> Result<Discovery, DiscoveryError<D::Error>> {
    gpt::discover_with_workspace(&mut Reader(&mut device), table, workspace)
        .map_err(DiscoveryError::Gpt)?;
    let layout = table.next_layout().map_err(DiscoveryError::Gpt)?;
    let extent = ReadExtent::new(device, layout.esp.first_lba, layout.esp.block_count())
        .map_err(DiscoveryError::Extent)?;
    let mut fat = Volume::mount(extent).map_err(DiscoveryError::Fat)?;
    let note_bytes = fat
        .read_file("/NEXT.TXT", out)
        .map_err(DiscoveryError::Fat)?;
    Ok(Discovery {
        disk_guid: table.disk_guid,
        esp: layout.esp,
        data: layout.data,
        recovery: layout.recovery,
        note_bytes,
    })
}

#[cfg(test)]
pub fn discover_and_read<D: BlockDevice>(
    device: D,
    out: &mut [u8],
) -> Result<Discovery, DiscoveryError<D::Error>> {
    discover_with_scratch(
        device,
        out,
        &mut gpt::Gpt::empty(),
        &mut gpt::Workspace::new(),
    )
}

struct Buffers {
    table: gpt::Gpt,
    workspace: gpt::Workspace,
}
struct Scratch(UnsafeCell<Buffers>);
// The exclusive CLAIMED guard is the only way to borrow this caller-owned output.
unsafe impl Sync for Scratch {}
static SCRATCH: Scratch = Scratch(UnsafeCell::new(Buffers {
    table: gpt::Gpt::empty(),
    workspace: gpt::Workspace::new(),
}));
static CLAIMED: AtomicBool = AtomicBool::new(false);
struct Claim;
impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}

struct BootReader {
    capacity: u64,
}
impl BlockDevice for BootReader {
    type Error = crate::storage::StorageError;
    fn sector_count(&self) -> u64 {
        self.capacity
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), Self::Error> {
        if lba >= self.capacity {
            return Err(crate::storage::StorageError::SectorOutOfRange);
        }
        // Constructed only by probe under its documented storage ownership requirements.
        unsafe { crate::storage::read_sector(lba, out) }
    }
}
/// Guest read-only entry point. Capacity must come from the probed device.
///
/// # Safety
/// Caller must satisfy storage::read_sector's boot-CPU ownership and DMA mapping
/// requirements, and must not run this stack-bounded parser on an IRQ stack.
pub unsafe fn probe(
    capacity: u64,
    out: &mut [u8],
) -> Result<Discovery, DiscoveryError<crate::storage::StorageError>> {
    CLAIMED
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| DiscoveryError::Busy)?;
    let _claim = Claim;
    // Exclusive guard lives until all references and filesystem reads are finished.
    let Buffers { table, workspace } = unsafe { &mut *SCRATCH.0.get() };
    discover_with_scratch(BootReader { capacity }, out, table, workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_scratch_claim_is_exclusive_and_released_after_validation_error() {
        let mut out = [0; 512];
        // Geometry rejects before any hardware access, so this checks the actual
        // guest entry-point lifetime without performing host port I/O.
        assert!(matches!(
            unsafe { probe(0, &mut out) },
            Err(DiscoveryError::Gpt(gpt::Error::Geometry))
        ));
        assert!(!CLAIMED.load(Ordering::Acquire));
        assert!(!CLAIMED.swap(true, Ordering::Acquire));
        let claim = Claim;
        assert!(matches!(
            unsafe { probe(0, &mut out) },
            Err(DiscoveryError::Busy)
        ));
        assert!(CLAIMED.load(Ordering::Acquire));
        drop(claim);
        assert!(!CLAIMED.load(Ordering::Acquire));
        assert!(matches!(
            unsafe { probe(0, &mut out) },
            Err(DiscoveryError::Gpt(gpt::Error::Geometry))
        ));
        assert!(!CLAIMED.load(Ordering::Acquire));
    }
}
