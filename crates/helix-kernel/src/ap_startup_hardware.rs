//! Unactivated AP readiness scaffold. BSP prepares private state; AP assembly
//! acknowledges identity then parks. No SIPI sender or scheduler exists here.
#![allow(dead_code)]
use core::arch::global_asm;
use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
#[path = "ap_bootstrap.rs"]
mod bootstrap;
#[path = "ap_icr.rs"]
pub mod icr;
#[path = "ap_nmi_sources.rs"]
pub mod nmi_sources;
#[path = "ap_root.rs"]
mod private_root;
#[path = "ap_startup.rs"]
mod protocol;
#[path = "ap_transition.rs"]
mod transition;
use protocol::{Error, GUARD_BYTES, MAX_APS, STACK_SLOT_BYTES};
#[repr(C, align(4096))]
struct Stacks([u8; MAX_APS * STACK_SLOT_BYTES]);
static mut AP_STACKS: Stacks = Stacks([0; MAX_APS * STACK_SLOT_BYTES]);
#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}
#[repr(C, align(4096))]
struct Mailbox {
    state: AtomicU32,
    expected: AtomicU32,
    token: AtomicU64,
    ack: AtomicU64,
    observed: AtomicU32,
    failure: AtomicU32,
    stack: AtomicU64,
    cr3: AtomicU64,
    idtr: Idtr,
    padding: [u8; 6],
    hpet_counter: AtomicU64,
    deadline: AtomicU64,
}
impl Mailbox {
    const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            expected: AtomicU32::new(0),
            token: AtomicU64::new(0),
            ack: AtomicU64::new(0),
            observed: AtomicU32::new(0),
            failure: AtomicU32::new(0),
            stack: AtomicU64::new(0),
            cr3: AtomicU64::new(0),
            idtr: Idtr { limit: 0, base: 0 },
            padding: [0; 6],
            hpet_counter: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
        }
    }
}
static mut MAILBOXES: [Mailbox; MAX_APS] = [const { Mailbox::new() }; MAX_APS];
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    mid: u16,
    high: u32,
    reserved: u32,
}
const EMPTY: Gate = Gate {
    low: 0,
    selector: 0,
    ist: 0,
    flags: 0,
    mid: 0,
    high: 0,
    reserved: 0,
};
#[repr(C, align(4096))]
struct Tables([[Gate; 256]; MAX_APS]);
static mut AP_IDTS: Tables = Tables([[EMPTY; 256]; MAX_APS]);
const _: () = assert!(core::mem::size_of::<Mailbox>() == 4096);
const _: () = assert!(core::mem::size_of::<Gate>() == 16);
const _: () = assert!(core::mem::offset_of!(Mailbox, idtr) == 48);
static mut NEXT_TOKEN: u64 = 1;
// Both witness cells are genuinely retained kernel allocation, with physical
// addresses differing only by A20. No unchecked low-memory alias is touched.
#[repr(C, align(2097152))]
struct A20Witness([u8; 1048584]);
static mut A20_WITNESS: A20Witness = A20Witness([0; 1048584]);
global_asm!(include_str!("ap_startup.S"),
    mailboxes = sym MAILBOXES,
    state = const core::mem::offset_of!(Mailbox, state),
    expected = const core::mem::offset_of!(Mailbox, expected),
    token = const core::mem::offset_of!(Mailbox, token),
    ack = const core::mem::offset_of!(Mailbox, ack),
    observed = const core::mem::offset_of!(Mailbox, observed),
    failure = const core::mem::offset_of!(Mailbox, failure),
    stack = const core::mem::offset_of!(Mailbox, stack),
    cr3 = const core::mem::offset_of!(Mailbox, cr3),
    idtr = const core::mem::offset_of!(Mailbox, idtr),
    slot_bytes = const core::mem::size_of::<Mailbox>(),
    hpet = const core::mem::offset_of!(Mailbox, hpet_counter),
    deadline = const core::mem::offset_of!(Mailbox, deadline),
);
unsafe extern "C" {
    static next_ap_blob_start: u8;
    static next_ap_blob_end: u8;
    static next_ap_emergency16: u8;
    static next_ap_emergency32: u8;
    static next_ap_emergency64: u8;
    fn next_ap_private_fault();
}
/// A source-stage diagnostic, not a hardware readiness or AP-start permission.
pub fn activation_blocker() -> &'static str {
    "Unsupported early NMI transition: pre-entry quiescence and mode-boundary exception coverage unverified"
}
pub fn stack_guard(address: u64) -> bool {
    protocol::stack_guard(addr_of!(AP_STACKS) as u64, address)
}
/// Immutable source only; no copying or executable mapping occurs here.
pub fn image() -> Result<&'static [u8], Error> {
    let start = addr_of!(next_ap_blob_start) as usize;
    let end = addr_of!(next_ap_blob_end) as usize;
    let len = end.checked_sub(start).ok_or(Error::Layout)?;
    if len == 0 || len > 4096 {
        return Err(Error::Layout);
    }
    Ok(unsafe { core::slice::from_raw_parts(start as *const u8, len) })
}
/// CPU-private preparation. This function deliberately refuses operation until
/// the reservation mapping is activated by a separately reviewed startup caller.
/// # Safety
/// BSP, IF clear, pre-SMP. MADT identity validated/enabled, HPET timeout owner
/// established. The mapped immutable image must be exactly `image()` and its
/// scratch/stack/mailbox addresses must remain retained even after quarantine.
pub unsafe fn prepare(
    index: usize,
    cpu: &crate::acpi::Cpu,
    bsp_id: u32,
    hpet: &crate::hpet::HpetInfo,
    deadline: u64,
) -> Result<u64, Error> {
    if !transition::activation_supported() {
        return Err(Error::NmiTransition);
    }
    let apic_id = cpu.apic_id;
    let flags: u64;
    let current_cr3: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("mov {}, cr3", out(reg) current_cr3, options(nomem, nostack, preserves_flags));
    if flags & (1 << 9) != 0 || current_cr3 != super::kernel_cr3() || !super::AP_TRAMPOLINE_MAPPED {
        return Err(Error::State);
    }
    if index >= MAX_APS || !cpu.enabled || apic_id > 255 || apic_id == bsp_id {
        return Err(Error::Identity);
    }
    let source = image()?;
    let mapped = core::slice::from_raw_parts(
        next_abi::boot::AP_TRAMPOLINE_BASE as *const u8,
        source.len(),
    );
    if mapped != source {
        return Err(Error::Layout);
    }
    // Caller supplies the boot-validated owned UC HPET; the architectural
    // aperture check additionally prevents a malformed arbitrary MMIO pointer.
    if !(0xfed0_0000..0xfee0_0000).contains(&hpet.address) || !hpet.address.is_multiple_of(4096) {
        return Err(Error::Layout);
    }
    let counter_address = hpet.address.checked_add(0xf0).ok_or(Error::Layout)?;
    let now = core::ptr::read_volatile(counter_address as *const u64);
    if deadline <= now {
        return Err(Error::Deadline);
    }
    let cr3 = super::kernel_cr3();
    if cr3 > u32::MAX as u64 || !cr3.is_multiple_of(4096) {
        return Err(Error::Layout);
    }
    let slots = addr_of_mut!(MAILBOXES).cast::<Mailbox>();
    for i in 0..MAX_APS {
        let s = &*slots.add(i);
        let state = s.state.load(Ordering::Acquire);
        if matches!(state, 1 | 2) || (state != 0 && s.expected.load(Ordering::Relaxed) == apic_id) {
            return Err(Error::State);
        }
    }
    let s = &mut *slots.add(index);
    if s.state.load(Ordering::Acquire) != 0 {
        return Err(Error::State);
    }
    let following = NEXT_TOKEN.checked_add(1).ok_or(Error::Token)?;
    let token = NEXT_TOKEN;
    let stack = protocol::stack_top(addr_of!(AP_STACKS) as u64, index)?;
    if stack > u32::MAX as u64 {
        return Err(Error::Layout);
    }
    // Read actual owned kernel leaves: both guards absent, all stack pages RW/NX.
    for page in 0..STACK_SLOT_BYTES / 4096 {
        let address = stack - (GUARD_BYTES + protocol::STACK_BYTES) as u64 + page as u64 * 4096;
        let leaf = super::kernel_identity_leaf(address).ok_or(Error::Layout)?;
        if stack_guard(address) {
            if leaf != 0 {
                return Err(Error::Layout);
            }
        } else if leaf & ((1 << 63) | 7) != (1 << 63) | 3 {
            return Err(Error::Layout);
        }
    }
    let idt = addr_of_mut!(AP_IDTS.0).cast::<Gate>().add(index * 256);
    let fault = next_ap_private_fault as *const () as u64;
    for vector in 0..256 {
        idt.add(vector).write(Gate {
            low: fault as u16,
            selector: 0x18,
            ist: 0,
            flags: 0x8e,
            mid: (fault >> 16) as u16,
            high: (fault >> 32) as u32,
            reserved: 0,
        });
    }
    s.idtr = Idtr {
        limit: 4095,
        base: idt as u64,
    };
    s.expected.store(apic_id, Ordering::Relaxed);
    s.token.store(token, Ordering::Relaxed);
    s.ack.store(0, Ordering::Relaxed);
    s.observed.store(u32::MAX, Ordering::Relaxed);
    s.failure.store(0, Ordering::Relaxed);
    s.stack.store(stack, Ordering::Relaxed);
    s.cr3.store(cr3, Ordering::Relaxed);
    s.hpet_counter.store(counter_address, Ordering::Relaxed);
    s.deadline.store(deadline, Ordering::Relaxed);
    if core::ptr::read_volatile(counter_address as *const u64) >= deadline {
        return Err(Error::Deadline);
    }
    NEXT_TOKEN = following;
    s.state.store(1, Ordering::Release);
    Ok(token)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Pending,
    Ready { apic_id: u32, token: u64 },
    Quarantined { failure: u32 },
}
/// Acquire checks do not turn a late/stale acknowledgment into readiness.
/// # Safety
/// BSP is the sole request/timeout owner; index/token come from `prepare`.
pub unsafe fn observe(index: usize, token: u64) -> Result<Observation, Error> {
    if index >= MAX_APS || token == 0 {
        return Err(Error::Token);
    }
    let s = &*addr_of!(MAILBOXES).cast::<Mailbox>().add(index);
    if s.token.load(Ordering::Acquire) != token {
        return Err(Error::Token);
    }
    match s.state.load(Ordering::Acquire) {
        1 | 2 => Ok(Observation::Pending),
        3 if s.ack.load(Ordering::Acquire) == token
            && s.observed.load(Ordering::Acquire) == s.expected.load(Ordering::Relaxed) =>
        {
            Ok(Observation::Ready {
                apic_id: s.observed.load(Ordering::Relaxed),
                token,
            })
        }
        4 => Ok(Observation::Quarantined {
            failure: s.failure.load(Ordering::Acquire),
        }),
        _ => Err(Error::State),
    }
}
/// Deadline comes from the caller's independent HPET wall clock. Permanent
/// quarantine retains stack/IDT/mailbox storage regardless of a late AP.
/// # Safety
/// BSP only, token belongs to this request; `expired` is a verified deadline.
pub unsafe fn quarantine(index: usize, token: u64, expired: bool) -> Result<Observation, Error> {
    if !expired {
        return Err(Error::Deadline);
    }
    let observation = observe(index, token)?;
    if observation != Observation::Pending {
        return Ok(observation);
    }
    let s = &*addr_of!(MAILBOXES).cast::<Mailbox>().add(index);
    for expected in [1, 2] {
        if s.state
            .compare_exchange(expected, 4, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break;
        }
    }
    observe(index, token)
}

/// Serialize an owned low scratch page using exact immutable handler symbols.
/// This stages records/tables only: it neither maps code RX nor starts an AP.
/// Private low stacks are disjoint. IVT/GDT/IDT descriptor bytes are shared
/// immutable storage; their handlers park without accessing another CPU's state.
/// # Safety
/// BSP only, IF clear, pre-SMP. `cpus` must be firmware-validated enabled MADT
/// identities. The caller retains scratch forever after staging; no AP may be
/// released until reviewed NMI and mode-switch coverage exists.
pub unsafe fn stage_low_records(
    boot: &next_abi::boot::BootInfo,
    regions: &next_abi::boot::MemoryRegions<'_>,
    cpus: &[crate::acpi::Cpu],
    bsp_id: u32,
) -> Result<(), bootstrap::Error> {
    let flags: u64;
    let cr3: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    if flags & (1 << 9) != 0
        || cr3 != super::kernel_cr3()
        || cpus.is_empty()
        || cpus.len() > MAX_APS
    {
        return Err(bootstrap::Error::Roster);
    }
    // The original first huge leaf must still be supervisor RW/NX identity.
    // Staging refuses execution-mapping mutation/reuse and any published slot.
    if super::AP_TRAMPOLINE_MAPPED
        || super::LOW_PD[0].0[0] & !0x60 != (1 << 63) | 0x83
        || (0..MAX_APS).any(|i| PRIVATE_ROOTS[i].is_some())
    {
        return Err(bootstrap::Error::Root);
    }
    for i in 0..MAX_APS {
        let m = &*addr_of!(MAILBOXES).cast::<Mailbox>().add(i);
        if m.state.load(Ordering::Acquire) != 0 {
            return Err(bootstrap::Error::Roster);
        }
    }
    let mut records = [bootstrap::CpuRecord {
        apic_id: 0,
        mailbox: 0,
    }; MAX_APS];
    let base = addr_of!(MAILBOXES) as u64;
    for (i, cpu) in cpus.iter().enumerate() {
        if !cpu.enabled {
            return Err(bootstrap::Error::Roster);
        }
        records[i] = bootstrap::CpuRecord {
            apic_id: cpu.apic_id,
            mailbox: base + i as u64 * core::mem::size_of::<Mailbox>() as u64,
        };
    }
    let source = image().map_err(|_| bootstrap::Error::Image)?;
    let source_start = source.as_ptr() as usize;
    let real = (addr_of!(next_ap_emergency16) as usize)
        .checked_sub(source_start)
        .ok_or(bootstrap::Error::Image)?;
    let protected = (addr_of!(next_ap_emergency32) as usize)
        .checked_sub(source_start)
        .ok_or(bootstrap::Error::Image)?;
    let mut bytes = bootstrap::serialize(
        boot,
        regions,
        &records[..cpus.len()],
        bsp_id,
        base..base + (MAX_APS * core::mem::size_of::<Mailbox>()) as u64,
        core::mem::size_of::<Mailbox>() as u64,
        source.len(),
        real,
        protected,
    )?;
    let first = addr_of_mut!(A20_WITNESS).cast::<u64>();
    let second = first.byte_add(1 << 20);
    let first_address = first as u64;
    let second_address = second as u64;
    if first_address ^ second_address != 1 << 20 || second_address >= 0x1_0000_0000 {
        return Err(bootstrap::Error::A20);
    }
    for address in [first_address, second_address] {
        let leaf = super::kernel_identity_leaf(address).ok_or(bootstrap::Error::Root)?;
        if leaf & ((1 << 63) | 7) != (1 << 63) | 3 {
            return Err(bootstrap::Error::Root);
        }
    }
    const FIRST: u64 = 0x6e657874_41323031;
    const SECOND: u64 = 0x6e657874_41323032;
    core::ptr::write_volatile(first, FIRST);
    core::ptr::write_volatile(second, SECOND);
    bootstrap::validate_a20(
        FIRST,
        SECOND,
        core::ptr::read_volatile(first),
        core::ptr::read_volatile(second),
    )?;
    // Low metadata +0xd20 holds bounded physical addresses and expected values.
    // The AP's protected-mode source checks these before any high mailbox read.
    for (i, value) in [first_address, second_address, FIRST, SECOND]
        .iter()
        .enumerate()
    {
        bytes[3360 + i * 8..3368 + i * 8].copy_from_slice(&value.to_le_bytes());
    }
    // Real-mode (6-byte) and protected-mode (6-byte) table descriptors.
    bytes[3392..3394].copy_from_slice(&1023u16.to_le_bytes());
    bytes[3394..3398].copy_from_slice(&(bootstrap::REAL_IVT as u32).to_le_bytes());
    bytes[3398..3400].copy_from_slice(&2047u16.to_le_bytes());
    bytes[3400..3404].copy_from_slice(&(bootstrap::PROTECTED_IDT as u32).to_le_bytes());
    let long = (addr_of!(next_ap_emergency64) as usize)
        .checked_sub(source_start)
        .ok_or(bootstrap::Error::Image)?;
    let bridge = transition::source_nmi_table(source.len(), [real, protected, long])
        .map_err(|_| bootstrap::Error::Image)?;
    bytes[3440..3488].copy_from_slice(&bridge);
    bytes[3416..3418].copy_from_slice(&47u16.to_le_bytes());
    bytes[3418..3422].copy_from_slice(&0x82d70u32.to_le_bytes());
    let scratch = next_abi::boot::AP_BOOTSTRAP_SCRATCH_BASE as *mut u8;
    core::ptr::write_bytes(scratch, 0, 8192);
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), scratch.add(4096), bytes.len());
    core::sync::atomic::fence(Ordering::SeqCst);
    Ok(())
}

static mut PRIVATE_ROOTS: [Option<private_root::RetainedRoot>; MAX_APS] = [const { None }; MAX_APS];
struct RootStorage<'a, 'm, const N: usize> {
    allocator: &'a mut helix_core::memory::PageAllocator<N>,
    boot: &'a next_abi::boot::BootInfo,
    regions: &'a next_abi::boot::MemoryRegions<'m>,
    pages: [u64; private_root::MAX_TABLES],
    count: usize,
}
impl<const N: usize> private_root::Storage for RootStorage<'_, '_, N> {
    fn allocate(&mut self) -> Result<u64, private_root::Error> {
        if self.count == self.pages.len() {
            return Err(private_root::Error::Allocation);
        }
        let address = self
            .allocator
            .allocate()
            .map_err(|_| private_root::Error::Allocation)?;
        let safe = (0..self.regions.len())
            .filter_map(|i| self.regions.get(i))
            .any(|r| {
                r.is_conventional()
                    && r.attributes & (1 << 63) == 0
                    && r.base <= address
                    && r.end().is_ok_and(|end| {
                        address
                            .checked_add(4096)
                            .is_some_and(|finish| finish <= end)
                    })
            })
            && [self.boot.kernel, self.boot.loader, self.boot.initrd]
                .iter()
                .all(|r| {
                    address.checked_add(4096).is_some_and(|end| {
                        end <= r.base || address >= r.base.saturating_add(r.byte_len)
                    })
                })
            && (0x100000..0xc0000000).contains(&address)
            && address.checked_add(4096).is_some_and(|end| {
                end <= self.boot.memory_map.base
                    || address
                        >= self
                            .boot
                            .memory_map
                            .base
                            .saturating_add(self.boot.memory_map.byte_len)
            })
            && address.checked_add(4096).is_some_and(|end| {
                end <= self.boot.framebuffer.base
                    || address
                        >= self
                            .boot
                            .framebuffer
                            .base
                            .saturating_add(self.boot.framebuffer.byte_len)
            });
        if !safe {
            let _ = self.allocator.free(address);
            return Err(private_root::Error::Storage);
        }
        // Only unchanged, owned BSP supervisor RW/NX identity huge mappings may
        // back allocator pages. No dereference precedes these checks.
        let slot = (address / 0x200000) as usize;
        let leaf = unsafe { super::LOW_PD[slot / 512].0[slot % 512] };
        if leaf & !0x60 != (address & !0x1fffff) | (1 << 63) | 0x83 {
            let _ = self.allocator.free(address);
            return Err(private_root::Error::Storage);
        }
        self.pages[self.count] = address;
        self.count += 1;
        Ok(address)
    }
    fn release(&mut self, address: u64) {
        if let Some(index) = self.pages[..self.count].iter().position(|p| *p == address) {
            self.pages[index] = self.pages[self.count - 1];
            self.count -= 1;
            let _ = self.allocator.free(address);
        }
    }
    fn read(&self, page: u64, index: usize) -> Result<u64, private_root::Error> {
        if index >= 512 || !self.pages[..self.count].contains(&page) {
            return Err(private_root::Error::Storage);
        }
        Ok(unsafe { core::ptr::read_volatile((page as *const u64).add(index)) })
    }
    fn write(&mut self, page: u64, index: usize, value: u64) -> Result<(), private_root::Error> {
        if index >= 512 || !self.pages[..self.count].contains(&page) {
            return Err(private_root::Error::Storage);
        }
        unsafe {
            core::ptr::write_volatile((page as *mut u64).add(index), value);
        }
        Ok(())
    }
}
/// Stage one genuinely private park-only AP root, retaining allocated physical
/// tables irreversibly. No CR3/mailbox publication, RX activation or SIPI occurs.
/// # Safety
/// BSP IF clear/pre-SMP, exclusive allocator owner derived from final map. HPET
/// is the validated UC device. Low records must have exactly one staged AP in
/// slot0: current loader owns only one page of low emergency stacks. Expanding
/// to further APs requires separately page-owned low stacks, not byte isolation.
pub unsafe fn stage_private_root<const N: usize>(
    boot: &next_abi::boot::BootInfo,
    regions: &next_abi::boot::MemoryRegions<'_>,
    allocator: &mut helix_core::memory::PageAllocator<N>,
    cpu: &crate::acpi::Cpu,
    bsp_id: u32,
    hpet: &crate::hpet::HpetInfo,
) -> Result<u64, private_root::Error> {
    use private_root::{Error, Permission as P, Region};
    let flags: u64;
    let cr3: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem,nostack,preserves_flags));
    if flags & (1 << 9) != 0
        || cr3 != super::kernel_cr3()
        || super::AP_TRAMPOLINE_MAPPED
        || PRIVATE_ROOTS[0].is_some()
        || !cpu.enabled
        || cpu.apic_id > 255
        || cpu.apic_id == bsp_id
        || boot
            .ap_bootstrap_scratch(regions)
            .map_err(|_| Error::Layout)?
            .is_none()
    {
        return Err(Error::State);
    }
    if super::LOW_PD[0].0[0] & !0x60 != (1 << 63) | 0x83 {
        return Err(Error::Mapping);
    }
    let metadata = bootstrap::METADATA as *const u8;
    let source = image().map_err(|_| Error::Layout)?;
    let source_base = source.as_ptr() as usize;
    let real_offset = (addr_of!(next_ap_emergency16) as usize)
        .checked_sub(source_base)
        .ok_or(Error::Layout)?;
    let protected_offset = (addr_of!(next_ap_emergency32) as usize)
        .checked_sub(source_base)
        .ok_or(Error::Layout)?;
    let mailbox_base = addr_of!(MAILBOXES) as u64;
    let stride = core::mem::size_of::<Mailbox>() as u64;
    let single_record = [bootstrap::CpuRecord {
        apic_id: cpu.apic_id,
        mailbox: mailbox_base,
    }];
    let mut expected = bootstrap::serialize(
        boot,
        regions,
        &single_record,
        bsp_id,
        mailbox_base..mailbox_base + MAX_APS as u64 * stride,
        stride,
        source.len(),
        real_offset,
        protected_offset,
    )
    .map_err(|_| Error::Layout)?;
    let witness_base = addr_of!(A20_WITNESS) as u64;
    for (i, value) in [
        witness_base,
        witness_base + (1 << 20),
        0x6e657874_41323031u64,
        0x6e657874_41323032u64,
    ]
    .iter()
    .enumerate()
    {
        expected[3360 + i * 8..3368 + i * 8].copy_from_slice(&value.to_le_bytes());
    }
    expected[3392..3394].copy_from_slice(&1023u16.to_le_bytes());
    expected[3394..3398].copy_from_slice(&(bootstrap::REAL_IVT as u32).to_le_bytes());
    expected[3398..3400].copy_from_slice(&2047u16.to_le_bytes());
    expected[3400..3404].copy_from_slice(&(bootstrap::PROTECTED_IDT as u32).to_le_bytes());
    let long_offset = (addr_of!(next_ap_emergency64) as usize)
        .checked_sub(source_base)
        .ok_or(Error::Layout)?;
    let bridge =
        transition::source_nmi_table(source.len(), [real_offset, protected_offset, long_offset])
            .map_err(|_| Error::Layout)?;
    expected[3440..3488].copy_from_slice(&bridge);
    expected[3416..3418].copy_from_slice(&47u16.to_le_bytes());
    expected[3418..3422].copy_from_slice(&0x82d70u32.to_le_bytes());
    if core::slice::from_raw_parts(metadata, 4096) != expected {
        return Err(Error::Layout);
    }
    if core::ptr::read_unaligned(metadata.cast::<u32>()) != cpu.apic_id
        || core::ptr::read_unaligned(metadata.add(12).cast::<u16>()) != 1
    {
        return Err(Error::State);
    }
    for i in 1..MAX_APS {
        if core::ptr::read_unaligned(metadata.add(i * 16 + 12).cast::<u16>()) != 0 {
            return Err(Error::State);
        }
    }
    for i in 0..MAX_APS {
        if (&*addr_of!(MAILBOXES).cast::<Mailbox>().add(i))
            .state
            .load(Ordering::Acquire)
            != 0
        {
            return Err(Error::State);
        }
    }
    if !(0xfed00000..0xfee00000).contains(&hpet.address) || !hpet.address.is_multiple_of(4096) {
        return Err(Error::Layout);
    }
    let mmio_slot = (hpet.address / 0x200000) as usize;
    let mmio_leaf = super::LOW_PD[mmio_slot / 512].0[mmio_slot % 512];
    if mmio_leaf & !0x60 != (hpet.address & !0x1fffff) | (1 << 63) | 0x9b {
        return Err(Error::Layout);
    }
    macro_rules! boundary { ($symbol:ident) => {{ let address:u64; core::arch::asm!("lea {}, [rip + {symbol}]",out(reg)address,symbol=sym super::$symbol,options(nomem,nostack,preserves_flags));address }}; }
    let text = boundary!(__text_start);
    let text_end = boundary!(__text_end);
    let ro = boundary!(__rodata_start);
    let ro_end = boundary!(__rodata_end);
    let stack = addr_of!(AP_STACKS) as u64 + GUARD_BYTES as u64;
    let stack_end = stack + protocol::STACK_BYTES as u64;
    let mailbox = addr_of!(MAILBOXES) as u64;
    let idt = addr_of!(AP_IDTS) as u64;
    let witness = addr_of!(A20_WITNESS) as u64;
    let regions_to_map = [
        Region {
            start: 0x80000,
            end: 0x81000,
            permission: P::ReadExecute,
        },
        Region {
            start: 0x81000,
            end: 0x82000,
            permission: P::ReadWrite,
        },
        Region {
            start: 0x82000,
            end: 0x83000,
            permission: P::ReadOnly,
        },
        Region {
            start: text,
            end: text_end,
            permission: P::ReadExecute,
        },
        Region {
            start: ro,
            end: ro_end,
            permission: P::ReadOnly,
        },
        Region {
            start: stack,
            end: stack_end,
            permission: P::ReadWrite,
        },
        Region {
            start: mailbox,
            end: mailbox + 4096,
            permission: P::ReadWrite,
        },
        Region {
            start: idt,
            end: idt + 4096,
            permission: P::ReadOnly,
        },
        Region {
            start: witness,
            end: witness + 4096,
            permission: P::ReadOnly,
        },
        Region {
            start: witness + (1 << 20),
            end: witness + (1 << 20) + 4096,
            permission: P::ReadOnly,
        },
        Region {
            start: hpet.address,
            end: hpet.address + 4096,
            permission: P::MmioReadOnly,
        },
    ];
    let guards = [stack - 4096, stack_end];
    let layout = private_root::Layout {
        regions: &regions_to_map,
        guards: &guards,
        stack: stack..stack_end,
    };
    let mut storage = RootStorage {
        allocator,
        boot,
        regions,
        pages: [0; private_root::MAX_TABLES],
        count: 0,
    };
    let root = private_root::stage(&mut storage, &layout)?;
    // Every page came from this exclusively borrowed allocator. Retention
    // forbids even the caller from later freeing a quarantined root page.
    for page in root.table_pages() {
        storage
            .allocator
            .retain(*page)
            .map_err(|_| Error::Storage)?;
    }
    let address = root.address();
    PRIVATE_ROOTS[0] = Some(root);
    Ok(address)
}
