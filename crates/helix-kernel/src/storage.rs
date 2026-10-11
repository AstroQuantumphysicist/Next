//! Bounded, polling legacy virtio-blk bootstrap I/O. No filesystem mutation.
//! FLUSH success is best-effort propagation to a compliant device, not a claim
//! about host/controller power-loss protection. Timeout writes are indeterminate.
use core::{
    arch::asm,
    cell::UnsafeCell,
    ptr,
    sync::atomic::{fence, AtomicBool, Ordering},
};

const PAGE: usize = 4096;
const MAX_QUEUE: usize = 1024;
const QUEUE_BYTES: usize = PAGE * 8;
const POLLS: usize = 5_000_000;
const FEATURE_READ_ONLY: u32 = 1 << 5;
const FEATURE_FLUSH: u32 = 1 << 9;
static CLAIMED: AtomicBool = AtomicBool::new(false);
#[repr(C, align(4096))]
struct Dma<T>(UnsafeCell<T>);
// The boot CPU owns these buffers under CLAIMED; device writes use volatile accesses.
unsafe impl<T> Sync for Dma<T> {}
static QUEUE: Dma<[u8; QUEUE_BYTES]> = Dma(UnsafeCell::new([0; QUEUE_BYTES]));
#[repr(C)]
struct Request {
    kind: u32,
    reserved: u32,
    sector: u64,
    data: [u8; 512],
    status: u8,
}
static REQUEST: Dma<Request> = Dma(UnsafeCell::new(Request {
    kind: 0,
    reserved: 0,
    sector: 0,
    data: [0; 512],
    status: 0xff,
}));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    pub capacity_sectors: u64,
    pub boot_signature: bool,
    pub bytes_per_sector: u16,
    /// A structurally valid FAT32 BPB at LBA 0, not a mounted filesystem.
    pub fat32_bpb: bool,
    /// Any protective 0xEE entry prevents superfloppy write authorization,
    /// including malformed/hybrid MBRs or a forged simultaneous FAT BPB.
    pub protective_mbr: bool,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    Busy,
    NotFound,
    InvalidBar,
    InvalidCapacity,
    QueueSize(u16),
    SectorOutOfRange,
    QueueAlreadyActive,
    DmaAddress,
    DeviceRejected,
    Timeout,
    InvalidCompletion,
    ReadFailed(u8),
    WriteFailed(u8),
    FlushFailed(u8),
    ReadOnly,
    FlushUnsupported,
}
impl core::fmt::Display for StorageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

unsafe fn in8(port: u16) -> u8 {
    let v;
    asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn in16(port: u16) -> u16 {
    let v;
    asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn in32(port: u16) -> u32 {
    let v;
    asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn out8(port: u16, v: u8) {
    asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags));
}
unsafe fn out16(port: u16, v: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags));
}
unsafe fn out32(port: u16, v: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags));
}
fn pci_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | (offset as u32 & 0xfc)
}
unsafe fn pci_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    out32(0xcf8, pci_address(bus, device, function, offset));
    in32(0xcfc)
}

/// Locate a legacy virtio-blk PCI device and read its LBA 0.
///
/// # Safety
/// See `read_sector`: exclusive boot-CPU device ownership and identity DMA
/// mapping are required. No writes are issued to the device.
pub unsafe fn probe_and_read_boot_sector() -> Result<DiskInfo, StorageError> {
    let mut sector = [0; 512];
    let (capacity, bus, device, function) = with_device(Operation::Read, 0, &mut sector)?;
    Ok(parse_sector(&sector, capacity, bus, device, function))
}

/// Read one 512-byte legacy sector. The device/queue are reset per request.
/// This deliberately favors a simple bootstrap implementation over throughput.
///
/// # Safety
/// Run only on the boot CPU in ring 0 after ExitBootServices, with exclusive
/// PCI/device ownership and identity-mapped static DMA buffers below 4 GiB.
/// This assumes coherent x86 DMA without an active IOMMU. The static buffers
/// remain reserved on failure; a failed reset permanently retains their lock.
pub unsafe fn read_sector(lba: u64, output: &mut [u8; 512]) -> Result<(), StorageError> {
    with_device(Operation::Read, lba, output).map(|_| ())
}

/// Write one complete sector, without implicitly claiming durable persistence.
/// Call `flush` explicitly and handle unsupported flush as a durability failure.
/// A timeout or failed completion may already have modified disk contents.
///
/// # Safety
/// Same device ownership/DMA requirements as `read_sector`. The caller must
/// authorize the destination and preserve filesystem transaction ordering.
pub unsafe fn write_sector(lba: u64, input: &[u8; 512]) -> Result<(), StorageError> {
    let mut sector = *input;
    with_device(Operation::Write, lba, &mut sector).map(|_| ())
}

/// Request cache propagation using the negotiated VIRTIO_BLK_F_FLUSH feature.
/// Success means the device acknowledged its flush contract. Unsupported
/// hardware is reported explicitly; no synthetic successful flush is returned.
///
/// # Safety
/// Same exclusive device ownership and DMA requirements as `read_sector`.
pub unsafe fn flush() -> Result<(), StorageError> {
    with_device(Operation::Flush, 0, &mut [0; 512]).map(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Read,
    Write,
    Flush,
}
impl Operation {
    fn kind(self) -> u32 {
        match self {
            Self::Read => 0,
            Self::Write => 1,
            Self::Flush => 4,
        }
    }
    fn writable_bytes(self) -> u32 {
        if self == Self::Read {
            513
        } else {
            1
        }
    }
    fn check(self, features: u32, lba: u64, capacity: u64) -> Result<(), StorageError> {
        if capacity == 0 {
            return Err(StorageError::InvalidCapacity);
        }
        if self != Self::Flush && lba >= capacity {
            return Err(StorageError::SectorOutOfRange);
        }
        if self == Self::Write && features & FEATURE_READ_ONLY != 0 {
            return Err(StorageError::ReadOnly);
        }
        if self == Self::Flush && features & FEATURE_FLUSH == 0 {
            return Err(StorageError::FlushUnsupported);
        }
        Ok(())
    }
    fn completion(self, idx: u16, id: u32, len: u32, status: u8) -> Result<(), StorageError> {
        if idx != 1 || id != 0 || status > 2 {
            return Err(StorageError::InvalidCompletion);
        }
        if status != 0 {
            return Err(match self {
                Self::Read => StorageError::ReadFailed(status),
                Self::Write => StorageError::WriteFailed(status),
                Self::Flush => StorageError::FlushFailed(status),
            });
        }
        if len != self.writable_bytes() {
            return Err(StorageError::InvalidCompletion);
        }
        Ok(())
    }
}

struct Claim;
impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}
unsafe fn with_device(
    operation: Operation,
    lba: u64,
    output: &mut [u8; 512],
) -> Result<(u64, u8, u8, u8), StorageError> {
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return Err(StorageError::Busy);
    }
    let claim = Claim;
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            let first = pci_read(bus, device, 0, 0);
            if first as u16 == 0xffff {
                continue;
            }
            let multifunction = pci_read(bus, device, 0, 0x0c) & (0x80 << 16) != 0;
            for function in 0..if multifunction { 8 } else { 1 } {
                if pci_read(bus, device, function, 0) != 0x1001_1af4 {
                    continue;
                }
                let bar = pci_read(bus, device, function, 0x10);
                if bar & 1 == 0 || bar & !3 == 0 || bar & !3 > u16::MAX as u32 - 32 {
                    return Err(StorageError::InvalidBar);
                }
                let base = (bar & !3) as u16;
                let command = pci_read(bus, device, function, 4) as u16;
                out32(0xcf8, pci_address(bus, device, function, 4));
                // A 16-bit write avoids clearing PCI status write-one bits.
                out16(0xcfc, command | 5);
                let result = request(base, operation, lba, output);
                // Reset releases the queue; static buffers are never freed.
                out8(base + 18, 0);
                fence(Ordering::SeqCst);
                if in8(base + 18) != 0 {
                    // Do not let a subsequent request alias live DMA buffers.
                    core::mem::forget(claim);
                    return Err(StorageError::DeviceRejected);
                }
                return result.map(|capacity| (capacity, bus, device, function));
            }
        }
    }
    Err(StorageError::NotFound)
}

unsafe fn request(
    base: u16,
    operation: Operation,
    lba: u64,
    output: &mut [u8; 512],
) -> Result<u64, StorageError> {
    out8(base + 18, 0);
    if in8(base + 18) != 0 {
        return Err(StorageError::DeviceRejected);
    }
    out8(base + 18, 1); // ACKNOWLEDGE
    out8(base + 18, 3); // DRIVER
    let features = in32(base);
    // Honor read-only devices and request only the implemented flush extension.
    out32(base + 4, features & (FEATURE_READ_ONLY | FEATURE_FLUSH));
    out16(base + 14, 0); // request queue
    let count = in16(base + 12) as usize;
    let (avail, used) = queue_layout(count)?;
    if in32(base + 8) != 0 {
        return Err(StorageError::QueueAlreadyActive);
    }
    let capacity = in32(base + 20) as u64 | ((in32(base + 24) as u64) << 32);
    operation.check(features, lba, capacity)?;
    let queue = QUEUE.0.get().cast::<u8>();
    let request = REQUEST.0.get();
    let queue_end = (queue as usize)
        .checked_add(QUEUE_BYTES)
        .ok_or(StorageError::DmaAddress)?;
    let request_end = (request as usize)
        .checked_add(core::mem::size_of::<Request>())
        .ok_or(StorageError::DmaAddress)?;
    if queue_end > u32::MAX as usize || request_end > u32::MAX as usize {
        return Err(StorageError::DmaAddress);
    }
    // Reset completed before clearing or reusing any device-visible memory.
    for index in 0..QUEUE_BYTES {
        ptr::write_volatile(queue.add(index), 0);
    }
    ptr::write_volatile(ptr::addr_of_mut!((*request).kind), operation.kind());
    ptr::write_volatile(ptr::addr_of_mut!((*request).reserved), 0);
    ptr::write_volatile(ptr::addr_of_mut!((*request).sector), lba);
    ptr::write_volatile(ptr::addr_of_mut!((*request).status), 0xff);
    if operation == Operation::Write {
        for (index, byte) in output.iter().enumerate() {
            ptr::write_volatile(
                ptr::addr_of_mut!((*request).data).cast::<u8>().add(index),
                *byte,
            );
        }
    }
    // A flush has no data descriptor; the header points directly to status.
    descriptor(
        queue,
        0,
        request as u64,
        16,
        1,
        if operation == Operation::Flush { 2 } else { 1 },
    );
    if operation != Operation::Flush {
        descriptor(
            queue,
            1,
            ptr::addr_of_mut!((*request).data) as u64,
            512,
            if operation == Operation::Read { 3 } else { 1 },
            2,
        );
    }
    descriptor(
        queue,
        2,
        ptr::addr_of_mut!((*request).status) as u64,
        1,
        2,
        0,
    );
    ptr::write_volatile(queue.add(avail).cast::<u16>(), 1); // NO_INTERRUPT
    ptr::write_volatile(queue.add(avail + 4).cast::<u16>(), 0); // head descriptor
    fence(Ordering::SeqCst);
    out32(base + 8, (queue as usize / PAGE) as u32);
    out8(base + 18, 7); // DRIVER_OK
    if in8(base + 18) & 0xc0 != 0 {
        return Err(StorageError::DeviceRejected);
    }
    ptr::write_volatile(queue.add(avail + 2).cast::<u16>(), 1);
    fence(Ordering::SeqCst);
    out16(base + 16, 0);
    for _ in 0..POLLS {
        if ptr::read_volatile(queue.add(used + 2).cast::<u16>()) != 0 {
            fence(Ordering::SeqCst);
            let idx = ptr::read_volatile(queue.add(used + 2).cast::<u16>());
            let id = ptr::read_volatile(queue.add(used + 4).cast::<u32>());
            let len = ptr::read_volatile(queue.add(used + 8).cast::<u32>());
            let status = ptr::read_volatile(ptr::addr_of!((*request).status));
            operation.completion(idx, id, len, status)?;
            if operation == Operation::Read {
                for (index, byte) in output.iter_mut().enumerate() {
                    *byte =
                        ptr::read_volatile(ptr::addr_of!((*request).data).cast::<u8>().add(index));
                }
            }
            return Ok(capacity);
        }
        core::hint::spin_loop();
    }
    Err(StorageError::Timeout)
}
fn queue_layout(count: usize) -> Result<(usize, usize), StorageError> {
    if !(3..=MAX_QUEUE).contains(&count) || !count.is_power_of_two() {
        return Err(StorageError::QueueSize(count as u16));
    }
    let avail = count * 16;
    let used = (avail + 6 + count * 2 + PAGE - 1) & !(PAGE - 1);
    if used + 6 + count * 8 > QUEUE_BYTES {
        return Err(StorageError::QueueSize(count as u16));
    }
    Ok((avail, used))
}

unsafe fn descriptor(queue: *mut u8, index: usize, address: u64, len: u32, flags: u16, next: u16) {
    let p = queue.add(index * 16);
    ptr::write_volatile(p.cast::<u64>(), address);
    ptr::write_volatile(p.add(8).cast::<u32>(), len);
    ptr::write_volatile(p.add(12).cast::<u16>(), flags);
    ptr::write_volatile(p.add(14).cast::<u16>(), next);
}

fn parse_sector(bytes: &[u8; 512], capacity: u64, bus: u8, device: u8, function: u8) -> DiskInfo {
    let u16_at = |i| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let signature = bytes[510] == 0x55 && bytes[511] == 0xaa;
    let bps = u16_at(11);
    let cluster = bytes[13] as u64;
    let reserved = u16_at(14) as u64;
    let fats = bytes[16] as u64;
    let total = u32_at(32) as u64;
    let fat_size = u32_at(36) as u64;
    let data_start = reserved + fats * fat_size;
    let clusters = if cluster != 0 && total > data_start {
        (total - data_start) / cluster
    } else {
        0
    };
    let root = u32_at(44) as u64;
    let fat32 = signature
        && bps == 512
        && cluster.is_power_of_two()
        && cluster <= 128
        && reserved > 0
        && (1..=2).contains(&fats)
        && u16_at(17) == 0
        && u16_at(19) == 0
        && u16_at(22) == 0
        && fat_size > 0
        && total <= capacity
        && (65525..0x0fff_fff5).contains(&clusters)
        && fat_size * 512 / 4 >= clusters + 2
        && root >= 2
        && root < clusters + 2;
    DiskInfo {
        capacity_sectors: capacity,
        boot_signature: signature,
        bytes_per_sector: 512,
        fat32_bpb: fat32,
        protective_mbr: (0..4).any(|slot| bytes[446 + slot * 16 + 4] == 0xee),
        pci_bus: bus,
        pci_device: device,
        pci_function: function,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authorization_checks_capacity_and_never_invents_flush_support() {
        assert_eq!(
            Operation::Read.check(0, 0, 0),
            Err(StorageError::InvalidCapacity)
        );
        for operation in [Operation::Read, Operation::Write] {
            assert_eq!(
                operation.check(FEATURE_FLUSH, 100, 100),
                Err(StorageError::SectorOutOfRange)
            );
            assert_eq!(
                operation.check(FEATURE_FLUSH, u64::MAX, 100),
                Err(StorageError::SectorOutOfRange)
            );
            assert_eq!(operation.check(FEATURE_FLUSH, 99, 100), Ok(()));
        }
        assert_eq!(Operation::Read.check(FEATURE_READ_ONLY, 0, 100), Ok(()));
        assert_eq!(
            Operation::Write.check(FEATURE_READ_ONLY | FEATURE_FLUSH, 0, 100),
            Err(StorageError::ReadOnly)
        );
        assert_eq!(
            Operation::Flush.check(0, 0, 100),
            Err(StorageError::FlushUnsupported)
        );
        // Flush is whole-device and must not validate a meaningless sector.
        assert_eq!(Operation::Flush.check(FEATURE_FLUSH, u64::MAX, 100), Ok(()));
    }
    #[test]
    fn completion_never_confuses_write_acknowledgement_with_read_data() {
        assert_eq!(Operation::Read.completion(1, 0, 513, 0), Ok(()));
        assert_eq!(
            Operation::Read.completion(1, 0, 1, 0),
            Err(StorageError::InvalidCompletion)
        );
        for operation in [Operation::Write, Operation::Flush] {
            assert_eq!(operation.completion(1, 0, 1, 0), Ok(()));
            for len in [0, 512, 513, u32::MAX] {
                assert_eq!(
                    operation.completion(1, 0, len, 0),
                    Err(StorageError::InvalidCompletion)
                );
            }
        }
    }
    #[test]
    fn stale_wrong_head_and_unwritten_status_are_rejected() {
        for operation in [Operation::Read, Operation::Write, Operation::Flush] {
            for idx in [0, 2, u16::MAX] {
                assert_eq!(
                    operation.completion(idx, 0, operation.writable_bytes(), 0),
                    Err(StorageError::InvalidCompletion)
                );
            }
            for id in [1, 2, u32::MAX] {
                assert_eq!(
                    operation.completion(1, id, operation.writable_bytes(), 0),
                    Err(StorageError::InvalidCompletion)
                );
            }
            for status in [3, 0xff] {
                assert_eq!(
                    operation.completion(1, 0, operation.writable_bytes(), status),
                    Err(StorageError::InvalidCompletion)
                );
            }
        }
        // Failed reads need not return their data buffer; preserve device error.
        assert_eq!(
            Operation::Read.completion(1, 0, 1, 1),
            Err(StorageError::ReadFailed(1))
        );
        assert_eq!(
            Operation::Write.completion(1, 0, 1, 1),
            Err(StorageError::WriteFailed(1))
        );
        assert_eq!(
            Operation::Flush.completion(1, 0, 1, 2),
            Err(StorageError::FlushFailed(2))
        );
    }
    #[test]
    fn queue_layout_fits_full_default_virtqueue() {
        assert_eq!(queue_layout(256), Ok((4096, 8192)));
        assert_eq!(queue_layout(1024), Ok((16384, 20480)));
        assert_eq!(queue_layout(2048), Err(StorageError::QueueSize(2048)));
        assert_eq!(queue_layout(3), Err(StorageError::QueueSize(3)));
    }
    #[test]
    fn claim_releases_buffers_for_subsequent_reads() {
        assert!(!CLAIMED.swap(true, Ordering::AcqRel));
        let claim = Claim;
        assert!(CLAIMED.load(Ordering::Acquire));
        drop(claim);
        assert!(!CLAIMED.load(Ordering::Acquire));
    }
    #[test]
    fn signature_alone_does_not_establish_a_filesystem() {
        let mut sector = [0; 512];
        sector[510] = 0x55;
        sector[511] = 0xaa;
        let info = parse_sector(&sector, 100, 0, 1, 0);
        assert!(info.boot_signature);
        assert!(!info.fat32_bpb);
    }
    #[test]
    fn fat32_geometry_must_fit_capacity_and_fat() {
        let mut sector = [0; 512];
        sector[510] = 0x55;
        sector[511] = 0xaa;
        sector[11..13].copy_from_slice(&512u16.to_le_bytes());
        sector[13] = 1;
        sector[14..16].copy_from_slice(&32u16.to_le_bytes());
        sector[16] = 2;
        sector[32..36].copy_from_slice(&100_000u32.to_le_bytes());
        sector[36..40].copy_from_slice(&800u32.to_le_bytes());
        sector[44..48].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_sector(&sector, 100_000, 0, 1, 0).fat32_bpb);
        assert!(!parse_sector(&sector, 100_000, 0, 1, 0).protective_mbr);
        for slot in 0..4 {
            let mut forged = sector;
            forged[446 + slot * 16 + 4] = 0xee;
            let info = parse_sector(&forged, 100_000, 0, 1, 0);
            assert!(info.fat32_bpb);
            assert!(info.protective_mbr);
            // Detection is conservative even when the MBR signature is corrupt.
            forged[510] = 0;
            assert!(parse_sector(&forged, 100_000, 0, 1, 0).protective_mbr);
        }
        assert!(!parse_sector(&sector, 99_999, 0, 1, 0).fat32_bpb);
        sector[36..40].copy_from_slice(&1u32.to_le_bytes());
        assert!(!parse_sector(&sector, 100_000, 0, 1, 0).fat32_bpb);
        sector[13] = 3;
        assert!(!parse_sector(&sector, 100_000, 0, 1, 0).fat32_bpb);
    }
}
