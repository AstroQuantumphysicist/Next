//! Actual BSP IRQ0 routing for the q35 development profile. PIT remains the
//! frequency source. Only a previously masked edge/high IOAPIC pin is claimed;
//! no other ISA route, AP, local timer or firmware NMI setting is changed.
use crate::{
    apic::{IoInfo, LocalInfo},
    apic_irq::{self, Hardware, PitRoute},
    hpet::HpetInfo,
};
use core::{
    arch::asm,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static EOI_OWNER: AtomicU64 = AtomicU64::new(0);
static APIC_DELIVERIES: AtomicU64 = AtomicU64::new(0);
static PIC_DELIVERIES: AtomicU64 = AtomicU64::new(0);
static HOOK_SEEN: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Resource,
    Hook,
    Gate,
    Reference,
    Busy,
    InjectionPolicy,
    Routing(apic_irq::Error),
}
impl Error {
    pub fn fatal(self) -> bool {
        matches!(
            self,
            Self::Routing(apic_irq::Error::UnsafeRollback | apic_irq::Error::NoFallback)
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub gsi: u32,
    pub apic_interrupts: u64,
    pub pic_interrupts: u64,
    pub owner_retained: bool,
    pub result: Result<apic_irq::Delivery, apic_irq::Error>,
    pub injected: bool,
    pub restored_controls: bool,
    pub rollback_pic_interrupts: u64,
}
unsafe fn input(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
    value
}
unsafe fn output(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
}
unsafe fn guard<T>(operation: impl FnOnce() -> T) -> T {
    let flags: u64;
    asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    asm!("cli", options(nostack));
    let result = operation();
    if flags & (1 << 9) != 0 {
        asm!("sti", options(nostack));
    }
    result
}
unsafe fn local_read(address: u64, offset: u64) -> u32 {
    core::ptr::read_volatile((address + offset) as *const u32)
}
unsafe fn local_write(address: u64, offset: u64, value: u32) {
    core::ptr::write_volatile((address + offset) as *mut u32, value);
    let _ = local_read(address, 0x20);
}
unsafe fn indexed_read(address: u64, index: u8) -> u32 {
    let selector = address as *mut u32;
    let previous = core::ptr::read_volatile(selector);
    core::ptr::write_volatile(selector, index as u32);
    let value = core::ptr::read_volatile((address + 0x10) as *const u32);
    core::ptr::write_volatile(selector, previous);
    value
}
unsafe fn indexed_write(address: u64, index: u8, value: u32) {
    let selector = address as *mut u32;
    let previous = core::ptr::read_volatile(selector);
    core::ptr::write_volatile(selector, index as u32);
    core::ptr::write_volatile((address + 0x10) as *mut u32, value);
    let _ = core::ptr::read_volatile((address + 0x10) as *const u32);
    core::ptr::write_volatile(selector, previous);
}
unsafe fn pic_in_service() -> bool {
    output(0x20, 0x0b); // OCW3 select ISR, no change to PIC routing or masks.
    input(0x20) & 1 != 0
}
fn increment(counter: &AtomicU64) {
    // Sole interrupt-gate writer. No counter reset or wrapping generations.
    counter.store(
        counter.load(Ordering::Relaxed).saturating_add(1),
        Ordering::Relaxed,
    );
}
/// # Safety
/// Called only by IRQ0's interrupt gate on the sole BSP before PIC EOI. IF is
/// clear and all live GPR/FX state is saved by the existing assembly handler.
/// Owner address is a permanent owned UC mapping published before unmasking.
pub unsafe fn acknowledge_irq0() {
    HOOK_SEEN.store(true, Ordering::Relaxed);
    let pic = pic_in_service();
    if pic {
        increment(&PIC_DELIVERIES);
    }
    let address = EOI_OWNER.load(Ordering::Acquire);
    if address != 0 && local_read(address, 0x110) & 1 != 0 {
        // An ExtINT/PIC delivery is never counted as proof of IOAPIC routing.
        if !pic {
            increment(&APIC_DELIVERIES);
        }
        local_write(address, 0xb0, 0);
    }
}
struct Backend {
    route: PitRoute,
    previous_route: u64,
    local: u64,
    old_svr: u32,
    old_tpr: u32,
    old_pic: u8,
    reference: HpetInfo,
    started: u64,
    expired: bool,
    draining: bool,
    injection: crate::apic_fault_injection::Stage,
    injected: bool,
    restored: u8,
    fallback_before: Option<u64>,
}
impl Backend {
    fn counter(&self) -> u64 {
        unsafe { core::ptr::read_volatile((self.reference.address + 0xf0) as *const u64) }
    }
    fn reset_wait(&mut self) {
        self.started = self.counter();
        self.expired = false;
    }
    unsafe fn entry(&self) -> u64 {
        let low = indexed_read(self.route.address, self.route.low_register);
        let high = indexed_read(self.route.address, self.route.high_register);
        (high as u64) << 32 | low as u64
    }
    unsafe fn set_entry(&self, low: u32, high: u32) -> Result<(), apic_irq::Error> {
        // Mask old low first, then destination-high, then final masked control.
        let old = indexed_read(self.route.address, self.route.low_register);
        indexed_write(self.route.address, self.route.low_register, old | (1 << 16));
        indexed_write(self.route.address, self.route.high_register, high);
        indexed_write(self.route.address, self.route.low_register, low | (1 << 16));
        let expected = (high as u64) << 32 | (low | (1 << 16)) as u64;
        if !apic_irq::redirection_matches(self.entry(), expected) {
            return Err(apic_irq::Error::Hardware);
        }
        Ok(())
    }
}
impl Hardware for Backend {
    fn configure_masked(&mut self, route: PitRoute) -> Result<(), apic_irq::Error> {
        unsafe { guard(|| self.set_entry(route.low_masked, route.high)) }
    }
    fn publish_eoi_owner(&mut self) -> Result<(), apic_irq::Error> {
        EOI_OWNER.store(self.local, Ordering::Release);
        Ok(())
    }
    fn enable_local(&mut self) -> Result<(), apic_irq::Error> {
        unsafe {
            guard(|| {
                let svr = (self.old_svr & !255) | 255 | (1 << 8);
                local_write(self.local, 0xf0, svr);
                local_write(self.local, 0x80, 0);
                if local_read(self.local, 0xf0) & 0x1ff != svr & 0x1ff
                    || local_read(self.local, 0x80) & 255 != 0
                {
                    return Err(apic_irq::Error::Hardware);
                }
                Ok(())
            })
        }
    }
    fn mask_pic_irq0(&mut self) -> Result<(), apic_irq::Error> {
        unsafe {
            guard(|| {
                output(0x21, self.old_pic | 1);
                if input(0x21) != self.old_pic | 1 {
                    return Err(apic_irq::Error::Hardware);
                }
                Ok(())
            })
        }
    }
    fn unmask_route(&mut self, route: PitRoute) -> Result<(), apic_irq::Error> {
        let before = self.apic_deliveries();
        self.reset_wait();
        unsafe {
            guard(|| {
                indexed_write(route.address, route.low_register, route.low_active);
                let expected = (route.high as u64) << 32 | route.low_active as u64;
                if !apic_irq::redirection_matches(self.entry(), expected) {
                    return Err(apic_irq::Error::Hardware);
                }
                Ok(())
            })
        }?;
        if self.injection == crate::apic_fault_injection::Stage::AfterUnmask {
            // Disposable signed debug build only. Prove the hardware route
            // delivered a real IRQ before forcing ordinary transaction failure.
            for _ in 0..5_000_000 {
                self.poll();
                if self.wait_expired() {
                    return Err(apic_irq::Error::NoDelivery);
                }
                if crate::apic_fault_injection::should_inject(
                    self.injection,
                    self.apic_deliveries().saturating_sub(before),
                ) {
                    self.injected = true;
                    return Err(apic_irq::Error::Hardware);
                }
            }
            return Err(apic_irq::Error::NoDelivery);
        }
        Ok(())
    }
    fn apic_deliveries(&self) -> u64 {
        APIC_DELIVERIES.load(Ordering::Relaxed)
    }
    fn pic_deliveries(&self) -> u64 {
        PIC_DELIVERIES.load(Ordering::Relaxed)
    }
    fn pending(&self) -> bool {
        // A readback proves the pin was masked, but a previously emitted APIC
        // message may still be in flight. Retain EOI ownership for a 1ms HPET
        // settling interval before accepting empty IRR/ISR as drained.
        if self.draining {
            let elapsed = self.counter().wrapping_sub(self.started);
            if elapsed >= 1u64 << 63
                || elapsed as u128 * (self.reference.period_fs as u128) < 1_000_000_000_000
            {
                return true;
            }
        }
        unsafe {
            guard(|| (local_read(self.local, 0x110) | local_read(self.local, 0x210)) & 1 != 0)
        }
    }
    fn poll(&mut self) {
        let elapsed = self.counter().wrapping_sub(self.started);
        self.expired = elapsed >= 1u64 << 63
            || elapsed as u128 * self.reference.period_fs as u128 >= 200_000_000_000_000;
        core::hint::spin_loop();
    }
    fn wait_expired(&self) -> bool {
        self.expired
    }
    fn mask_route(&mut self, route: PitRoute) -> Result<(), apic_irq::Error> {
        let result = unsafe {
            guard(|| {
                let low = indexed_read(route.address, route.low_register);
                indexed_write(route.address, route.low_register, low | (1 << 16));
                if indexed_read(route.address, route.low_register) & (1 << 16) == 0 {
                    return Err(apic_irq::Error::Hardware);
                }
                Ok(())
            })
        };
        self.reset_wait();
        self.draining = true;
        result
    }
    fn restore_route(&mut self) -> Result<(), apic_irq::Error> {
        unsafe {
            guard(|| {
                self.set_entry(
                    self.previous_route as u32,
                    (self.previous_route >> 32) as u32,
                )
            })
        }?;
        self.restored |= 1;
        Ok(())
    }
    fn restore_local(&mut self) -> Result<(), apic_irq::Error> {
        unsafe {
            guard(|| {
                local_write(self.local, 0x80, self.old_tpr);
                local_write(self.local, 0xf0, self.old_svr);
                if local_read(self.local, 0x80) & 255 != self.old_tpr & 255
                    || local_read(self.local, 0xf0) & 0x1ff != self.old_svr & 0x1ff
                {
                    return Err(apic_irq::Error::Hardware);
                }
                Ok(())
            })
        }?;
        self.restored |= 2;
        Ok(())
    }
    fn withdraw_eoi_owner(&mut self) {
        EOI_OWNER.store(0, Ordering::Release);
    }
    fn restore_pic(&mut self) -> Result<(), apic_irq::Error> {
        self.reset_wait();
        self.draining = false;
        unsafe {
            guard(|| {
                self.fallback_before = Some(PIC_DELIVERIES.load(Ordering::Relaxed));
                output(0x21, self.old_pic);
                if input(0x21) != self.old_pic {
                    return Err(apic_irq::Error::Hardware);
                }
                Ok(())
            })
        }?;
        self.restored |= 4;
        Ok(())
    }
}
/// # Safety
/// Sole BSP before userspace scheduling; successful permanent UC LAPIC, IOAPIC
/// and HPET probes. IRQ0 calls acknowledge_irq0 before PIC EOI. The owned IDT
/// has a kernel interrupt gate at vector0xff returning with IRETQ and no EOI.
/// Spurious gate must be installed before calling, and IF must be set so actual
/// IRQs can be delivered. No other owner modifies these controllers.
pub unsafe fn route_pit(
    info: &IoInfo,
    local: LocalInfo,
    reference: HpetInfo,
    regions: &next_abi::boot::MemoryRegions<'_>,
    spurious_gate_installed: bool,
) -> Result<Report, Error> {
    use crate::hpet::{authorize_development_mmio, DevelopmentResource};
    let injection =
        crate::apic_fault_injection::configured().map_err(|_| Error::InjectionPolicy)?;
    if !spurious_gate_installed {
        return Err(Error::Gate);
    }
    if !HOOK_SEEN.load(Ordering::Relaxed) {
        return Err(Error::Hook);
    }
    let flags: u64;
    asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    if flags & (1 << 9) == 0 || EOI_OWNER.load(Ordering::Acquire) != 0 {
        return Err(Error::Busy);
    }
    if info.len == 0 || info.len > 4 || info.plan.len > 16 {
        return Err(Error::Resource);
    }
    let candidate = info.plan.routes[..info.plan.len]
        .iter()
        .find(|r| r.source == 0)
        .ok_or(Error::Resource)?;
    if candidate.controller as usize >= info.len {
        return Err(Error::Resource);
    }
    let controller = info.controllers[candidate.controller as usize];
    let address = controller.firmware.address as u64;
    if !authorize_development_mmio(regions, DevelopmentResource::IoApic, address, 4096)
        || !authorize_development_mmio(regions, DevelopmentResource::LocalApic, local.address, 4096)
        || !authorize_development_mmio(regions, DevelopmentResource::Hpet, reference.address, 4096)
    {
        return Err(Error::Resource);
    }
    if reference.period_fs == 0
        || reference.period_fs > 100_000_000
        || reference.frequency_hz != 1_000_000_000_000_000 / reference.period_fs as u64
        || core::ptr::read_volatile((reference.address + 0x10) as *const u64) & 3 != 1
    {
        return Err(Error::Reference);
    }
    let (low_index, high_index) = controller
        .redirection_registers(candidate.pin)
        .map_err(|_| Error::Resource)?;
    let (previous, old_svr, old_tpr, old_pic, pending) = guard(|| {
        let entry = (indexed_read(address, high_index) as u64) << 32
            | indexed_read(address, low_index) as u64;
        (
            entry,
            local_read(local.address, 0xf0),
            local_read(local.address, 0x80),
            input(0x21),
            (local_read(local.address, 0x110) | local_read(local.address, 0x210)) & 1 != 0
                || pic_in_service(),
        )
    });
    if pending || old_pic & 1 != 0 {
        return Err(Error::Busy);
    }
    let route = PitRoute::prepare(info, previous).map_err(Error::Routing)?;
    let before_apic = APIC_DELIVERIES.load(Ordering::Relaxed);
    let before_pic = PIC_DELIVERIES.load(Ordering::Relaxed);
    let mut hardware = Backend {
        route,
        previous_route: previous,
        local: local.address,
        old_svr,
        old_tpr,
        old_pic,
        reference,
        started: 0,
        expired: false,
        draining: false,
        injection,
        injected: false,
        restored: 0,
        fallback_before: None,
    };
    let result = apic_irq::activate(&mut hardware, route, 5_000_000);
    if let Err(error @ (apic_irq::Error::UnsafeRollback | apic_irq::Error::NoFallback)) = result {
        return Err(Error::Routing(error));
    }
    Ok(Report {
        gsi: route.gsi,
        apic_interrupts: APIC_DELIVERIES
            .load(Ordering::Relaxed)
            .saturating_sub(before_apic),
        pic_interrupts: PIC_DELIVERIES
            .load(Ordering::Relaxed)
            .saturating_sub(before_pic),
        owner_retained: EOI_OWNER.load(Ordering::Acquire) != 0,
        result,
        injected: hardware.injected,
        restored_controls: hardware.restored == 7,
        rollback_pic_interrupts: hardware.fallback_before.map_or(0, |before| {
            PIC_DELIVERIES
                .load(Ordering::Relaxed)
                .saturating_sub(before)
        }),
    })
}
