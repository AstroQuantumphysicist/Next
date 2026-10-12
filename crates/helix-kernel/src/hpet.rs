//! HPET counter discovery and bounded advancement probe. Comparator interrupts
//! stay disabled and legacy replacement stays off, so PIT remains the scheduler
//! clock. A validated free-running counter also provides a high-resolution
//! monotonic reading on the bootstrap CPU; this is not calibrated HPET
//! scheduling or SMP timer support.
use crate::acpi::Hpet;
use core::{
    ptr,
    sync::atomic::{fence, AtomicU64, Ordering},
};
use next_abi::boot::MemoryRegions;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    PhysicalRange,
    Capabilities,
    Counter32,
    Period,
    Identity,
    LegacyMode,
    Stopped,
    CachePolicy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HpetInfo {
    pub address: u64,
    pub period_fs: u32,
    pub frequency_hz: u64,
    pub timers: u8,
    pub counter_ticks: u64,
}
/// Explicit q35 development-profile resources, not general hardware approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // APIC register discovery is staged after the HPET probe.
pub enum DevelopmentResource {
    IoApic,
    Hpet,
    LocalApic,
}
/// Firmware sometimes omits chipset MMIO from its UEFI map. Only a validated
/// ACPI resource in its architectural development subaperture may occupy a hole.
/// A hole must have zero overlap with *every* descriptor. A partially described
/// resource is rejected, even when the overlapping part is reserved/MMIO.
pub fn authorize_development_mmio(
    regions: &MemoryRegions<'_>,
    resource: DevelopmentResource,
    address: u64,
    len: usize,
) -> bool {
    let (start, limit) = match resource {
        DevelopmentResource::IoApic => (0xfec00000, 0xfed00000),
        DevelopmentResource::Hpet => (0xfed00000, 0xfee00000),
        DevelopmentResource::LocalApic => (0xfee00000, 0xfef00000),
    };
    let Some(end) = address.checked_add(len as u64) else {
        return false;
    };
    if address < start
        || end > limit
        || len == 0
        || len > 65536
        || !address.is_multiple_of(4096)
        || !len.is_multiple_of(4096)
    {
        return false;
    }
    if mmio_range(regions, address, len) {
        return true;
    }
    (0..regions.len()).all(|i| {
        regions.get(i).is_some_and(|r| {
            r.end()
                .is_ok_and(|rend| r.pages == 0 || r.base >= end || rend <= address)
        })
    })
}
impl HpetInfo {
    pub fn elapsed_ns(self, ticks: u64) -> u64 {
        ((ticks as u128 * self.period_fs as u128) / 1_000_000).min(u64::MAX as u128) as u64
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MonotonicOrigin {
    counter_ticks: u64,
    monotonic_ns: u64,
    period_fs: u32,
}
impl MonotonicOrigin {
    fn at(self, counter_ticks: u64) -> Option<u64> {
        let elapsed_ticks = counter_ticks.wrapping_sub(self.counter_ticks);
        // Accept a natural 64-bit counter wrap, but reject readings at least
        // half a counter-space away, which are indistinguishable from regression.
        if elapsed_ticks >= 1u64 << 63 {
            return None;
        }
        self.monotonic_ns
            .checked_add(counter_nanoseconds(elapsed_ticks, self.period_fs)?)
    }
}
/// MMIO must be represented by reserved or memory-mapped I/O descriptors,
/// never by conventional RAM, loader allocations, or ACPI table storage.
pub fn mmio_range(regions: &MemoryRegions<'_>, address: u64, len: usize) -> bool {
    let Some(end) = address.checked_add(len as u64) else {
        return false;
    };
    if address == 0 || len == 0 || end > 1u64 << 32 {
        return false;
    }
    if (0..regions.len()).any(|i| {
        regions.get(i).is_none_or(|r| {
            r.end().map_or(true, |rend| {
                r.pages != 0
                    && r.base < end
                    && address < rend
                    && (!matches!(r.kind, 0 | 11) || r.attributes & (1u64 << 63) != 0)
            })
        })
    }) {
        return false;
    }
    let mut cursor = address;
    while cursor < end {
        let Some(r) = (0..regions.len()).filter_map(|i| regions.get(i)).find(|r| {
            matches!(r.kind, 0 | 11) && r.base <= cursor && r.end().is_ok_and(|e| cursor < e)
        }) else {
            return false;
        };
        cursor = r.end().unwrap_or(cursor).min(end);
    }
    true
}
fn capabilities(address: u64, block_id: u32, raw: u64) -> Result<HpetInfo, Error> {
    if raw as u32 != block_id {
        return Err(Error::Identity);
    }
    if raw as u8 == 0
        || raw & (1 << 14) != 0
        || raw >> 16 & 0xffff == 0
        || raw >> 16 & 0xffff == 0xffff
    {
        return Err(Error::Capabilities);
    }
    if raw & (1 << 13) == 0 {
        return Err(Error::Counter32);
    }
    let period_fs = (raw >> 32) as u32;
    if period_fs == 0 || period_fs > 100_000_000 {
        return Err(Error::Period);
    }
    Ok(HpetInfo {
        address,
        period_fs,
        frequency_hz: 1_000_000_000_000_000 / period_fs as u64,
        timers: ((raw >> 8) & 31) as u8 + 1,
        counter_ticks: 0,
    })
}
/// # Safety
/// Sole bootstrap CPU owns HPET registers after firmware exit. Descriptor came
/// from validated ACPI in the explicit q35 development aperture; this acquires
/// an owned uncached (UC) MMIO mapping. No driver
/// uses its comparators. This enables its counter with all interrupt sources
/// masked, and leaves PIT routing unchanged.
pub unsafe fn probe(descriptor: Hpet, regions: &MemoryRegions<'_>) -> Result<HpetInfo, Error> {
    if ACCOUNTING_ADDRESS.load(Ordering::Acquire) != 0 {
        return Err(Error::Identity);
    }
    if !authorize_development_mmio(regions, DevelopmentResource::Hpet, descriptor.address, 4096) {
        return Err(Error::PhysicalRange);
    }
    #[cfg(target_os = "none")]
    {
        let flags: u64;
        core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
        core::arch::asm!("cli", options(nostack));
        let mapped = crate::arch::mark_mmio_uncached(descriptor.address);
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti", options(nostack));
        }
        if !mapped {
            return Err(Error::CachePolicy);
        }
    }
    let registers = descriptor.address as *mut u64;
    let mut info = capabilities(
        descriptor.address,
        descriptor.block_id,
        ptr::read_volatile(registers),
    )?;
    let configuration = registers.add(2);
    let old = ptr::read_volatile(configuration);
    if old & 2 != 0 {
        return Err(Error::LegacyMode);
    }
    for timer in 0..info.timers as usize {
        let config = (descriptor.address as *mut u8)
            .add(0x100 + timer * 0x20)
            .cast::<u64>();
        let current = ptr::read_volatile(config);
        ptr::write_volatile(config, current & !((1 << 2) | (1 << 14)));
    }
    fence(Ordering::SeqCst);
    ptr::write_volatile(configuration, (old & !2) | 1);
    fence(Ordering::SeqCst);
    let counter = (descriptor.address as *const u8).add(0xf0).cast::<u64>();
    let first = ptr::read_volatile(counter);
    for _ in 0..1_000_000 {
        let now = ptr::read_volatile(counter);
        let delta = now.wrapping_sub(first);
        if delta != 0 && delta < 1u64 << 63 {
            info.counter_ticks = delta;
            // Bridge the validated counter to the existing boot-relative PIT
            // epoch. Read the PIT first so switching sources cannot move the
            // public monotonic clock behind the value already observed there.
            let monotonic_ns = crate::timer::nanoseconds();
            let counter_ticks = ptr::read_volatile(counter);
            if counter_ticks.wrapping_sub(first) == 0
                || counter_ticks.wrapping_sub(first) >= 1u64 << 63
            {
                ptr::write_volatile(configuration, old);
                fence(Ordering::SeqCst);
                return Err(Error::Stopped);
            }
            MONOTONIC_BASE_NS.store(monotonic_ns, Ordering::Relaxed);
            MONOTONIC_BASE_COUNTER.store(counter_ticks, Ordering::Relaxed);
            // Publish only after ACPI/resource/UC/capability/advancement checks.
            // This clock measures dispatch occupancy; PIT still delivers IRQs.
            ACCOUNTING_PERIOD_FS.store(info.period_fs as u64, Ordering::Relaxed);
            ACCOUNTING_ADDRESS.store(info.address, Ordering::Release);
            return Ok(info);
        }
        core::hint::spin_loop();
    }
    ptr::write_volatile(configuration, old);
    fence(Ordering::SeqCst);
    Err(Error::Stopped)
}
static ACCOUNTING_ADDRESS: AtomicU64 = AtomicU64::new(0);
static ACCOUNTING_PERIOD_FS: AtomicU64 = AtomicU64::new(0);
static MONOTONIC_BASE_COUNTER: AtomicU64 = AtomicU64::new(0);
static MONOTONIC_BASE_NS: AtomicU64 = AtomicU64::new(0);
/// Read only the kernel-owned validated counter. No caller supplies an MMIO
/// address, and stopped/legacy-enabled hardware is unavailable, never invented.
pub fn accounting_ns() -> Option<u64> {
    let address = ACCOUNTING_ADDRESS.load(Ordering::Acquire);
    if address == 0 {
        return None;
    }
    #[cfg(target_os = "none")]
    unsafe {
        let config = ptr::read_volatile((address + 0x10) as *const u64);
        if config & 3 != 1 {
            return None;
        }
        let ticks = ptr::read_volatile((address + 0xf0) as *const u64);
        counter_nanoseconds(ticks, ACCOUNTING_PERIOD_FS.load(Ordering::Relaxed) as u32)
    }
    #[cfg(not(target_os = "none"))]
    {
        None
    }
}
/// Read the validated free-running counter in the PIT boot-time epoch. The
/// source is bootstrap-CPU-only; callers retain a PIT fallback if it stops.
pub fn monotonic_ns() -> Option<u64> {
    let address = ACCOUNTING_ADDRESS.load(Ordering::Acquire);
    if address == 0 {
        return None;
    }
    #[cfg(target_os = "none")]
    unsafe {
        let config = ptr::read_volatile((address + 0x10) as *const u64);
        if config & 3 != 1 {
            return None;
        }
        let counter_ticks = ptr::read_volatile((address + 0xf0) as *const u64);
        MonotonicOrigin {
            counter_ticks: MONOTONIC_BASE_COUNTER.load(Ordering::Relaxed),
            monotonic_ns: MONOTONIC_BASE_NS.load(Ordering::Relaxed),
            period_fs: ACCOUNTING_PERIOD_FS.load(Ordering::Relaxed) as u32,
        }
        .at(counter_ticks)
    }
    #[cfg(not(target_os = "none"))]
    {
        None
    }
}
/// Smallest representable monotonic step for the active validated source,
/// rounded up to the nanosecond unit exposed by the POSIX timespec ABI.
pub fn monotonic_resolution_ns() -> Option<u64> {
    if ACCOUNTING_ADDRESS.load(Ordering::Acquire) == 0 {
        return None;
    }
    resolution_ns_from_period(ACCOUNTING_PERIOD_FS.load(Ordering::Relaxed) as u32)
}
fn resolution_ns_from_period(period_fs: u32) -> Option<u64> {
    if period_fs == 0 || period_fs > 100_000_000 {
        return None;
    }
    Some(u64::from(period_fs).div_ceil(1_000_000))
}
fn counter_nanoseconds(ticks: u64, period_fs: u32) -> Option<u64> {
    if period_fs == 0 || period_fs > 100_000_000 {
        return None;
    }
    u64::try_from(u128::from(ticks) * u128::from(period_fs) / 1_000_000).ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    const ID: u32 = 0x8086a201;
    #[test]
    fn accounting_conversion_rejects_clock_end_and_invalid_period() {
        assert_eq!(
            counter_nanoseconds(100_000_000, 10_000_000),
            Some(1_000_000_000)
        );
        assert_eq!(counter_nanoseconds(1, 1), Some(0));
        assert_eq!(counter_nanoseconds(u64::MAX, 100_000_000), None);
        assert_eq!(counter_nanoseconds(1, 0), None);
        assert_eq!(counter_nanoseconds(1, 100_000_001), None);
        assert_eq!(accounting_ns(), None); // host harness never performs MMIO
    }
    #[test]
    fn monotonic_origin_bridges_pit_epoch_and_handles_counter_wrap_safely() {
        let origin = MonotonicOrigin {
            counter_ticks: u64::MAX - 15,
            monotonic_ns: 42_000_000,
            period_fs: 10_000_000,
        };
        assert_eq!(origin.at(u64::MAX - 15), Some(42_000_000));
        // Thirty-two ticks across the hardware wrap are 320 ns at 10 ns/tick.
        assert_eq!(origin.at(16), Some(42_000_320));
        assert_eq!(origin.at(u64::MAX - 16), None);
        assert_eq!(
            MonotonicOrigin {
                counter_ticks: 0,
                monotonic_ns: u64::MAX,
                period_fs: 10_000_000,
            }
            .at(1),
            None
        );
    }
    #[test]
    fn monotonic_resolution_rounds_counter_period_up_to_nanoseconds() {
        assert_eq!(resolution_ns_from_period(10_000_000), Some(10));
        assert_eq!(resolution_ns_from_period(1), Some(1));
        assert_eq!(resolution_ns_from_period(1_000_001), Some(2));
        assert_eq!(resolution_ns_from_period(0), None);
        assert_eq!(resolution_ns_from_period(100_000_001), None);
        assert_eq!(monotonic_resolution_ns(), None); // host harness never publishes MMIO
    }
    #[test]
    fn real_qemu_capability_profile_has_three_64bit_timers_at_100mhz() {
        let i = capabilities(0xfed00000, ID, (10_000_000u64 << 32) | ID as u64).unwrap();
        assert_eq!(i.frequency_hz, 100_000_000);
        assert_eq!(i.timers, 3);
        assert_eq!(i.elapsed_ns(100_000_000), 1_000_000_000);
        assert_eq!(i.elapsed_ns(u64::MAX), u64::MAX);
    }
    #[test]
    fn invalid_frequency_counter_width_identity_and_reserved_bit_are_rejected() {
        for period in [0, 100_000_001, u32::MAX] {
            assert_eq!(
                capabilities(0xfed00000, ID, ((period as u64) << 32) | ID as u64),
                Err(Error::Period)
            );
        }
        let id = ID & !(1 << 13);
        assert_eq!(
            capabilities(0xfed00000, id, (10_000_000u64 << 32) | id as u64),
            Err(Error::Counter32)
        );
        assert_eq!(
            capabilities(0xfed00000, ID, (10_000_000u64 << 32) | (ID ^ 1) as u64),
            Err(Error::Identity)
        );
        let id = ID | (1 << 14);
        assert_eq!(
            capabilities(0xfed00000, id, (10_000_000u64 << 32) | id as u64),
            Err(Error::Capabilities)
        );
    }
    #[test]
    fn mmio_requires_complete_map_coverage_and_rejects_ram_or_overflow() {
        let mut bytes = [0; 40];
        bytes[..4].copy_from_slice(&11u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&0xfed00000u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&1u64.to_le_bytes());
        let r = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(mmio_range(&r, 0xfed00000, 4096));
        assert!(!mmio_range(&r, 0xfed00000, 4097));
        assert!(!mmio_range(&r, u64::MAX, 4096));
        bytes[..4].copy_from_slice(&7u32.to_le_bytes());
        let r = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(!mmio_range(&r, 0xfed00000, 4096));
    }
    #[test]
    fn development_apertures_authorize_only_complete_reserved_ranges_or_empty_holes() {
        fn descriptor(bytes: &mut [u8], kind: u32, base: u64, pages: u64) {
            bytes[..4].copy_from_slice(&kind.to_le_bytes());
            bytes[8..16].copy_from_slice(&base.to_le_bytes());
            bytes[24..32].copy_from_slice(&pages.to_le_bytes());
        }
        let mut bytes = [0; 80];
        descriptor(&mut bytes[..40], 7, 0x100000, 256);
        descriptor(&mut bytes[40..], 9, 0x200000, 1);
        let map = MemoryRegions::parse(&bytes, 40).unwrap();
        for (resource, address) in [
            (DevelopmentResource::IoApic, 0xfec00000),
            (DevelopmentResource::Hpet, 0xfed00000),
            (DevelopmentResource::LocalApic, 0xfee00000),
        ] {
            assert!(authorize_development_mmio(&map, resource, address, 4096));
            assert!(!mmio_range(&map, address, 4096));
        }
        for address in [0, 0xfec00000, 0xfee00000, 0xfef00000, u64::MAX] {
            assert!(!authorize_development_mmio(
                &map,
                DevelopmentResource::Hpet,
                address,
                4096
            ));
        }
        assert!(!authorize_development_mmio(
            &map,
            DevelopmentResource::Hpet,
            0xfed00001,
            4096
        ));
        assert!(!authorize_development_mmio(
            &map,
            DevelopmentResource::Hpet,
            0xfed00000,
            65537
        ));
        for kind in [1, 2, 3, 4, 5, 6, 7, 9, 10, 12, 14] {
            descriptor(&mut bytes[40..], kind, 0xfed00000, 1);
            let map = MemoryRegions::parse(&bytes, 40).unwrap();
            assert!(!authorize_development_mmio(
                &map,
                DevelopmentResource::Hpet,
                0xfed00000,
                4096
            ));
        }
        for kind in [0, 11] {
            descriptor(&mut bytes[40..], kind, 0xfed00000, 1);
            let map = MemoryRegions::parse(&bytes, 40).unwrap();
            assert!(authorize_development_mmio(
                &map,
                DevelopmentResource::Hpet,
                0xfed00000,
                4096
            ));
            assert!(!authorize_development_mmio(
                &map,
                DevelopmentResource::Hpet,
                0xfed00000,
                8192
            ));
            descriptor(&mut bytes[40..], kind, 0xfed01000, 1);
            let map = MemoryRegions::parse(&bytes, 40).unwrap();
            assert!(!authorize_development_mmio(
                &map,
                DevelopmentResource::Hpet,
                0xfed00000,
                8192
            ));
        }
        descriptor(&mut bytes[40..], 11, 0xfed00000, 1);
        bytes[72..80].copy_from_slice(&(1u64 << 63).to_le_bytes());
        let map = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(!authorize_development_mmio(
            &map,
            DevelopmentResource::Hpet,
            0xfed00000,
            4096
        ));
    }
}
