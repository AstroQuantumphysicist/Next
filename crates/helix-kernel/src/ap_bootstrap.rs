//! Serialized low bootstrap storage, not AP-start permission. The mode-specific
//! tables must be selected at the reviewed mode boundary; this does not solve
//! asynchronous delivery while CR0/EFER changes descriptor interpretation.
use next_abi::boot::{BootInfo, MemoryRegions, AP_BOOTSTRAP_SCRATCH_BASE};
pub const RECORD_BYTES: usize = 16;
pub const RECORDS: usize = 16;
pub const METADATA: u64 = AP_BOOTSTRAP_SCRATCH_BASE + 4096;
pub const REAL_IVT: u64 = METADATA + 256;
pub const PROTECTED_IDT: u64 = REAL_IVT + 1024;
pub const PRIVATE_GDT: u64 = PROTECTED_IDT + 2048;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Ownership,
    Image,
    Roster,
    Mailbox,
    Root,
    A20,
    Ipi,
}
#[derive(Clone, Copy)]
pub struct CpuRecord {
    pub apic_id: u32,
    pub mailbox: u64,
}
/// All pointers must belong to one retained, supervisor-only mailbox allocation.
/// Each CPU gets its own low stack; descriptor bytes are shared immutable data,
/// and exception handlers park without touching any BSP or runtime state.
#[allow(clippy::too_many_arguments)]
pub fn serialize(
    boot: &BootInfo,
    regions: &MemoryRegions,
    cpus: &[CpuRecord],
    bsp: u32,
    mailboxes: core::ops::Range<u64>,
    mailbox_stride: u64,
    source_len: usize,
    real_offset: usize,
    protected_offset: usize,
) -> Result<[u8; 4096], Error> {
    if boot
        .ap_bootstrap_scratch(regions)
        .map_err(|_| Error::Ownership)?
        .is_none()
    {
        return Err(Error::Ownership);
    }
    if source_len == 0
        || source_len > 4096
        || real_offset >= source_len
        || protected_offset >= source_len
    {
        return Err(Error::Image);
    }
    if cpus.is_empty() || cpus.len() > RECORDS || bsp > 255 {
        return Err(Error::Roster);
    }
    if mailbox_stride < 128
        || !mailbox_stride.is_multiple_of(64)
        || mailboxes.start < 0x20_0000
        || mailboxes.end > 0x1_0000_0000
        || mailboxes.end <= mailboxes.start
        || !mailboxes.start.is_multiple_of(64)
    {
        return Err(Error::Mailbox);
    }
    let mut bytes = [0; 4096];
    for (i, cpu) in cpus.iter().enumerate() {
        if cpu.apic_id > 255
            || cpu.apic_id == bsp
            || cpus[..i].iter().any(|c| c.apic_id == cpu.apic_id)
        {
            return Err(Error::Roster);
        }
        let end = cpu
            .mailbox
            .checked_add(mailbox_stride)
            .ok_or(Error::Mailbox)?;
        if cpu.mailbox < mailboxes.start
            || end > mailboxes.end
            || !(cpu.mailbox - mailboxes.start).is_multiple_of(mailbox_stride)
            || cpus[..i].iter().any(|c| c.mailbox == cpu.mailbox)
        {
            return Err(Error::Mailbox);
        }
        let offset = i * RECORD_BYTES;
        let ss = ((AP_BOOTSTRAP_SCRATCH_BASE + i as u64 * 256) / 16) as u16;
        bytes[offset..offset + 4].copy_from_slice(&cpu.apic_id.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&(cpu.mailbox as u32).to_le_bytes());
        bytes[offset + 8..offset + 10].copy_from_slice(&ss.to_le_bytes());
        bytes[offset + 10..offset + 12].copy_from_slice(&256u16.to_le_bytes());
        bytes[offset + 12..offset + 14].copy_from_slice(&1u16.to_le_bytes());
    }
    for vector in 0..256 {
        let real = 256 + vector * 4;
        bytes[real..real + 2].copy_from_slice(&(real_offset as u16).to_le_bytes());
        bytes[real + 2..real + 4].copy_from_slice(&0x8000u16.to_le_bytes());
        let pm = 1280 + vector * 8;
        let address = 0x80000 + protected_offset as u32;
        bytes[pm..pm + 2].copy_from_slice(&(address as u16).to_le_bytes());
        bytes[pm + 2..pm + 4].copy_from_slice(&8u16.to_le_bytes());
        bytes[pm + 5] = 0x8e;
        bytes[pm + 6..pm + 8].copy_from_slice(&((address >> 16) as u16).to_le_bytes());
    }
    for (i, descriptor) in [
        0u64,
        0x00cf9b000000ffff,
        0x00cf93000000ffff,
        0x00af9b000000ffff,
    ]
    .iter()
    .enumerate()
    {
        bytes[3328 + i * 8..3336 + i * 8].copy_from_slice(&descriptor.to_le_bytes());
    }
    Ok(bytes)
}
/// Actual page-table root geometry before any AP CR3 load. Immutable source,
/// scratch and private stack need supervisor mappings; root itself below4GiB.
pub fn validate_root(root: u64, code: u64, scratch: u64, stack: u64) -> Result<(), Error> {
    const NX: u64 = 1 << 63;
    if !(0x20_0000..0x1_0000_0000).contains(&root)
        || !root.is_multiple_of(4096)
        || code & (NX | 7) != 1
        || scratch & (NX | 7) != NX | 3
        || stack & (NX | 7) != NX | 3
    {
        return Err(Error::Root);
    }
    Ok(())
}
/// Two genuinely owned volatile markers separated by bit20 prove A20 address
/// separation. A port92 readback alone proves only a chipset register setting.
pub fn validate_a20(
    first: u64,
    second: u64,
    observed_first: u64,
    observed_second: u64,
) -> Result<(), Error> {
    if first == second || observed_first != first || observed_second != second {
        Err(Error::A20)
    } else {
        Ok(())
    }
}
/// No architectural observation on the BSP proves AP reset-LINT state or that
/// an external VMM will refrain from injection. Keep those named assumptions
/// distinct from the observable PC source gates and software IPI ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Q35NmiObservation {
    pub cmos_nmi_masked: bool,
    pub pc_sources_disabled: bool,
    pub lint0_masked: bool,
    pub lint1_masked: bool,
}
impl Q35NmiObservation {
    pub const fn local_sources_closed(self) -> bool {
        self.cmos_nmi_masked && self.pc_sources_disabled && self.lint0_masked && self.lint1_masked
    }
    pub const fn proves_ap_preentry_quiescence(self) -> bool {
        false
    }
}
/// Non-forgeable transition ownership for the future single ICR writer. NMI
/// delivery is refused throughout transition, including timeout quarantine.
/// No existing hardware IPI sender or AP activation is introduced by this API.
pub struct IpiOwnership {
    transitioning: bool,
}
impl IpiOwnership {
    pub const fn new() -> Self {
        Self {
            transitioning: false,
        }
    }
    pub fn begin(&mut self) -> Result<(), Error> {
        if self.transitioning {
            return Err(Error::Ipi);
        }
        self.transitioning = true;
        Ok(())
    }
    pub fn authorize(&self, icr: u32) -> Result<(), Error> {
        let mode = (icr >> 8) & 7;
        // Only fixed, NMI (outside transition), INIT and SIPI are supported.
        if !matches!(mode, 0 | 4 | 5 | 6) || (self.transitioning && mode == 4) {
            Err(Error::Ipi)
        } else {
            Ok(())
        }
    }
}
impl Default for IpiOwnership {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (BootInfo, [u8; 40]) {
        use next_abi::boot::*;
        let mut b = BootInfo::new();
        b.framebuffer = Framebuffer {
            base: 0xe0000000,
            byte_len: 640 * 480 * 4,
            width: 640,
            height: 480,
            stride: 640,
            pixel_format: 0,
            reserved: 0,
        };
        b.memory_map = MemoryMap {
            base: 0x400000,
            byte_len: 40,
            descriptor_size: 40,
            descriptor_version: 1,
            key: 0,
        };
        b.kernel = PhysicalRange {
            base: 0x4000000,
            byte_len: 4096,
        };
        b.loader = PhysicalRange {
            base: 0x600000,
            byte_len: 4096,
        };
        b.initrd = PhysicalRange {
            base: 0x800000,
            byte_len: 4096,
        };
        b.header.flags = AP_TRAMPOLINE_RESERVED | AP_BOOTSTRAP_SCRATCH_RESERVED;
        b.seal();
        let mut d = [0; 40];
        d[..4].copy_from_slice(&2u32.to_le_bytes());
        d[8..16].copy_from_slice(&0x80000u64.to_le_bytes());
        d[24..32].copy_from_slice(&3u64.to_le_bytes());
        (b, d)
    }
    #[test]
    fn all_records_and_vectors_decode_with_private_stack_bounds() {
        let (b, d) = fixture();
        let m = MemoryRegions::parse(&d, 40).unwrap();
        let cpus: [CpuRecord; 16] = core::array::from_fn(|i| CpuRecord {
            apic_id: i as u32 + 1,
            mailbox: 0x4400000 + i as u64 * 128,
        });
        let data = serialize(&b, &m, &cpus, 0, 0x4400000..0x4400800, 128, 512, 400, 416).unwrap();
        for i in 0..16 {
            let n = i * 16;
            assert_eq!(
                u32::from_le_bytes(data[n..n + 4].try_into().unwrap()),
                i as u32 + 1
            );
            assert_eq!(
                u32::from_le_bytes(data[n + 4..n + 8].try_into().unwrap()),
                0x4400000 + i as u32 * 128
            );
            let ss = u16::from_le_bytes(data[n + 8..n + 10].try_into().unwrap());
            assert_eq!(u64::from(ss) * 16, 0x81000 + i as u64 * 256);
            assert_eq!(&data[n + 10..n + 16], &[0, 1, 1, 0, 0, 0]);
        }
        for v in 0..256 {
            assert_eq!(&data[256 + v * 4..260 + v * 4], &[0x90, 1, 0, 0x80]);
            assert_eq!(
                &data[1280 + v * 8..1288 + v * 8],
                &[0xa0, 1, 8, 0, 0, 0x8e, 8, 0]
            );
        }
        assert!(data[3360..].iter().all(|b| *b == 0));
        assert_eq!(PRIVATE_GDT, 0x82d00);
        // Each non-null descriptor has Accessed already set, preventing a CPU
        // descriptor-load write after paging protects immutable GDT storage.
        for index in 1..4 {
            let descriptor =
                u64::from_le_bytes(data[3328 + index * 8..3336 + index * 8].try_into().unwrap());
            assert_eq!((descriptor >> 40) & 1, 1);
            assert_eq!((descriptor >> 47) & 1, 1);
        }
    }
    #[test]
    fn roster_pointer_image_and_unowned_handoff_fail_closed() {
        let (mut b, d) = fixture();
        let m = MemoryRegions::parse(&d, 40).unwrap();
        let good = CpuRecord {
            apic_id: 1,
            mailbox: 0x4400000,
        };
        let run = |b: &BootInfo, cs: &[CpuRecord], len, real| {
            serialize(b, &m, cs, 0, 0x4400000..0x4400800, 128, len, real, 416)
        };
        assert_eq!(run(&b, &[good, good], 512, 400), Err(Error::Roster));
        for id in [0, 256] {
            assert_eq!(
                run(
                    &b,
                    &[CpuRecord {
                        apic_id: id,
                        ..good
                    }],
                    512,
                    400
                ),
                Err(Error::Roster)
            );
        }
        for pointer in [0, 0x43fffff, 0x4400001, 0x4400800, u64::MAX] {
            assert_eq!(
                run(
                    &b,
                    &[CpuRecord {
                        mailbox: pointer,
                        ..good
                    }],
                    512,
                    400
                ),
                Err(Error::Mailbox)
            );
        }
        for (len, offset) in [(0, 0), (4097, 400), (512, 512)] {
            assert_eq!(run(&b, &[good], len, offset), Err(Error::Image));
        }
        b.header.flags = next_abi::boot::AP_TRAMPOLINE_RESERVED;
        b.seal();
        assert_eq!(run(&b, &[good], 512, 400), Err(Error::Ownership));
    }
    #[test]
    fn root_permissions_and_a20_alias_faults() {
        let nx = 1u64 << 63;
        assert_eq!(validate_root(0x400000, 1, nx | 3, nx | 3), Ok(()));
        for bad in [0, 3, 5, nx | 1] {
            assert_eq!(
                validate_root(0x400000, bad, nx | 3, nx | 3),
                Err(Error::Root)
            );
        }
        for root in [0, 0x400001, 0x1_0000_0000] {
            assert_eq!(validate_root(root, 1, nx | 3, nx | 3), Err(Error::Root));
        }
        assert_eq!(validate_a20(0x123, 0x456, 0x123, 0x456), Ok(()));
        assert_eq!(validate_a20(0x123, 0x456, 0x123, 0x123), Err(Error::A20));
        assert_eq!(validate_a20(0x123, 0x123, 0x123, 0x123), Err(Error::A20));
    }
    #[test]
    fn nmi_ownership_does_not_promote_bsp_observation() {
        let closed = Q35NmiObservation {
            cmos_nmi_masked: true,
            pc_sources_disabled: true,
            lint0_masked: true,
            lint1_masked: true,
        };
        assert!(closed.local_sources_closed());
        assert!(!closed.proves_ap_preentry_quiescence());
        let mut owner = IpiOwnership::new();
        assert_eq!(owner.authorize(4 << 8), Ok(()));
        owner.begin().unwrap();
        assert_eq!(owner.begin(), Err(Error::Ipi));
        assert_eq!(owner.authorize(4 << 8), Err(Error::Ipi));
        for delivery in [0, 5, 6] {
            assert_eq!(owner.authorize(delivery << 8), Ok(()));
        }
        for delivery in [1, 2, 3, 7] {
            assert_eq!(owner.authorize(delivery << 8), Err(Error::Ipi));
        }
    }
}
