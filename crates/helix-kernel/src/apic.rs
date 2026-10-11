//! Staged APIC discovery and routing plans. No route is installed, no PIC is
//! disabled, and no application processor is started by this module.
use crate::acpi::{InterruptOverride, IoApic, Topology};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Capacity,
    Address,
    Version,
    Pins,
    Overlap,
    Duplicate,
    Flags,
    Vector,
    Destination,
    UnmappedGsi,
    CpuMode,
    PhysicalRange,
    CachePolicy,
    Identity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controller {
    pub firmware: IoApic,
    pub pins: u16,
}
impl Controller {
    /// IOAPICVER bits 23:16 contain the highest redirection index. IOREGSEL is
    /// eight bits, so entries whose high register exceeds 0xff are rejected.
    pub fn from_version(firmware: IoApic, version: u32) -> Result<Self, Error> {
        if firmware.address < 0xc0000000 || !firmware.address.is_multiple_of(4096) {
            return Err(Error::Address);
        }
        if version & 0xff < 0x10 || version & 0xff == 0xff || version & 0xff00_ff00 != 0 {
            return Err(Error::Version);
        }
        let pins = ((version >> 16) & 255) as u16 + 1;
        if pins > 120 || firmware.gsi_base.checked_add(pins as u32).is_none() {
            return Err(Error::Pins);
        }
        Ok(Self { firmware, pins })
    }
    pub fn redirection_registers(self, pin: u16) -> Result<(u8, u8), Error> {
        if pin >= self.pins || pin >= 120 {
            return Err(Error::Pins);
        }
        let low = 0x10 + 2 * pin;
        Ok((low as u8, (low + 1) as u8))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub source: u8,
    pub gsi: u32,
    pub controller: u8,
    pub pin: u16,
    pub active_low: bool,
    pub level: bool,
}
const EMPTY_ROUTE: Route = Route {
    source: 0,
    gsi: 0,
    controller: 0,
    pin: 0,
    active_low: false,
    level: false,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub routes: [Route; 16],
    pub len: usize,
    pub destination: u8,
}
/// ISA conforming defaults mean active high, edge triggered. Reserved polarity
/// and trigger values fail even when the other half is conforming.
pub fn isa_flags(flags: u16) -> Result<(bool, bool), Error> {
    if flags & !15 != 0 || flags & 3 == 2 || (flags >> 2) & 3 == 2 {
        return Err(Error::Flags);
    }
    Ok((flags & 3 == 3, (flags >> 2) & 3 == 3))
}
/// A masked fixed-delivery physical-destination redirection entry. Bit 16 is
/// always set; installing/unmasking this plan requires a future IRQ owner.
pub fn masked_entry(route: Route, vector: u8, destination: u32) -> Result<u64, Error> {
    if !(32..=254).contains(&vector) {
        return Err(Error::Vector);
    }
    if destination > 254 {
        return Err(Error::Destination);
    }
    Ok(vector as u64
        | (1 << 16)
        | ((route.active_low as u64) << 13)
        | ((route.level as u64) << 15)
        | ((destination as u64) << 56))
}
fn validate_controllers(controllers: &[Controller]) -> Result<(), Error> {
    if controllers.is_empty() || controllers.len() > 4 {
        return Err(Error::Capacity);
    }
    for (i, a) in controllers.iter().enumerate() {
        if a.pins == 0 || a.pins > 120 {
            return Err(Error::Pins);
        }
        let end = a
            .firmware
            .gsi_base
            .checked_add(a.pins as u32)
            .ok_or(Error::Pins)?;
        for b in &controllers[..i] {
            if a.firmware.id == b.firmware.id || a.firmware.address == b.firmware.address {
                return Err(Error::Duplicate);
            }
            let bend = b
                .firmware
                .gsi_base
                .checked_add(b.pins as u32)
                .ok_or(Error::Pins)?;
            if a.firmware.gsi_base < bend && b.firmware.gsi_base < end {
                return Err(Error::Overlap);
            }
        }
    }
    Ok(())
}
pub fn plan_isa(topology: &Topology, controllers: &[Controller]) -> Result<Plan, Error> {
    if topology.cpu_count > topology.cpus.len()
        || topology.io_apic_count > topology.io_apics.len()
        || topology.override_count > topology.overrides.len()
    {
        return Err(Error::Capacity);
    }
    if topology.bootstrap_apic_id > 254
        || !topology.cpus[..topology.cpu_count]
            .iter()
            .any(|cpu| cpu.enabled && cpu.apic_id == topology.bootstrap_apic_id)
    {
        return Err(Error::Destination);
    }
    validate_controllers(controllers)?;
    if controllers.len() != topology.io_apic_count
        || controllers
            .iter()
            .any(|c| !topology.io_apics[..topology.io_apic_count].contains(&c.firmware))
    {
        return Err(Error::Identity);
    }
    let overrides = &topology.overrides[..topology.override_count];
    for (i, iso) in overrides.iter().enumerate() {
        if iso.source >= 16 {
            return Err(Error::Flags);
        }
        isa_flags(iso.flags)?;
        if overrides[..i]
            .iter()
            .any(|b| b.source == iso.source || b.gsi == iso.gsi)
        {
            return Err(Error::Duplicate);
        }
    }
    let mut plan = Plan {
        routes: [EMPTY_ROUTE; 16],
        len: 0,
        destination: topology.bootstrap_apic_id as u8,
    };
    for source in 0..16 {
        let override_entry = overrides.iter().find(|iso| iso.source == source);
        if source == 2 && topology.pcat_compatible && override_entry.is_none() {
            continue;
        }
        let iso = override_entry.copied().unwrap_or(InterruptOverride {
            source,
            gsi: source as u32,
            flags: 0,
        });
        let (active_low, level) = isa_flags(iso.flags)?;
        if plan.routes[..plan.len].iter().any(|r| r.gsi == iso.gsi) {
            return Err(Error::Duplicate);
        }
        let (index, controller) = controllers
            .iter()
            .enumerate()
            .find(|(_, c)| {
                iso.gsi >= c.firmware.gsi_base && iso.gsi - c.firmware.gsi_base < c.pins as u32
            })
            .ok_or(Error::UnmappedGsi)?;
        plan.routes[plan.len] = Route {
            source,
            gsi: iso.gsi,
            controller: index as u8,
            pin: (iso.gsi - controller.firmware.gsi_base) as u16,
            active_low,
            level,
        };
        plan.len += 1;
    }
    Ok(plan)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalInfo {
    pub address: u64,
    pub apic_id: u8,
    pub version: u8,
    pub lvt_entries: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoInfo {
    pub controllers: [Controller; 4],
    pub len: usize,
    pub plan: Plan,
}
fn controller_identity(firmware: IoApic, raw_id: u32, version: u32) -> Result<Controller, Error> {
    if raw_id & 0x00ff_ffff != 0 || raw_id >> 24 != firmware.id as u32 {
        return Err(Error::Identity);
    }
    Controller::from_version(firmware, version)
}
/// # Safety
/// Sole bootstrap CPU owns the validated MADT IOAPIC resources after firmware
/// exit. Other CPUs/handlers must not access IOREGSEL. Interrupts remain masked
/// throughout indexed reads and selector restoration. Only the index selector
/// is written; no redirection entry or interrupt controller setting changes.
#[cfg(target_os = "none")]
pub unsafe fn probe_io(
    topology: &Topology,
    regions: &next_abi::boot::MemoryRegions<'_>,
) -> Result<IoInfo, Error> {
    if topology.io_apic_count == 0 || topology.io_apic_count > 4 {
        return Err(Error::Capacity);
    }
    let empty = Controller {
        firmware: IoApic {
            id: 0,
            address: 0,
            gsi_base: 0,
        },
        pins: 0,
    };
    let mut controllers = [empty; 4];
    for firmware in &topology.io_apics[..topology.io_apic_count] {
        if !crate::hpet::authorize_development_mmio(
            regions,
            crate::hpet::DevelopmentResource::IoApic,
            firmware.address as u64,
            4096,
        ) {
            return Err(Error::PhysicalRange);
        }
    }
    let flags: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("cli", options(nostack));
    let result = (|| {
        for (i, firmware) in topology.io_apics[..topology.io_apic_count]
            .iter()
            .enumerate()
        {
            if !crate::arch::mark_mmio_uncached(firmware.address as u64) {
                return Err(Error::CachePolicy);
            }
            let selector = firmware.address as *mut u32;
            let window = (firmware.address as u64 + 0x10) as *const u32;
            let previous = core::ptr::read_volatile(selector);
            core::ptr::write_volatile(selector, 0);
            let id = core::ptr::read_volatile(window);
            core::ptr::write_volatile(selector, 1);
            let version = core::ptr::read_volatile(window);
            core::ptr::write_volatile(selector, previous);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            controllers[i] = controller_identity(*firmware, id, version)?;
        }
        let plan = plan_isa(topology, &controllers[..topology.io_apic_count])?;
        Ok(IoInfo {
            controllers,
            len: topology.io_apic_count,
            plan,
        })
    })();
    if flags & (1 << 9) != 0 {
        core::arch::asm!("sti", options(nostack));
    }
    result
}
fn local_info(address: u64, expected_id: u32, id: u32, version: u32) -> Result<LocalInfo, Error> {
    if expected_id > 254 || id & 0x00ff_ffff != 0 || id >> 24 != expected_id {
        return Err(Error::Identity);
    }
    let revision = version as u8;
    let max_lvt = (version >> 16) as u8;
    if !(0x10..=0x20).contains(&revision)
        || !(2..=6).contains(&max_lvt)
        || version & !0x01ff_00ff != 0
    {
        return Err(Error::Version);
    }
    Ok(LocalInfo {
        address,
        apic_id: expected_id as u8,
        version: revision,
        lvt_entries: max_lvt + 1,
    })
}
/// # Safety
/// Sole bootstrap CPU, validated firmware topology, retained UEFI memory map.
/// This reads ID/version only after proving xAPIC mode, MSR base agreement,
/// an explicitly authorized q35 MMIO resource and an owned UC mapping. It
/// writes no APIC registers and does not qualify other hardware platforms.
#[cfg(target_os = "none")]
pub unsafe fn probe_local(
    topology: &Topology,
    regions: &next_abi::boot::MemoryRegions<'_>,
) -> Result<LocalInfo, Error> {
    if core::arch::x86_64::__cpuid(1).edx & (1 << 9) == 0 {
        return Err(Error::CpuMode);
    }
    let low: u32;
    let high: u32;
    core::arch::asm!("rdmsr", in("ecx") 0x1bu32, out("eax") low, out("edx") high, options(nostack));
    let base = ((high as u64) << 32) | low as u64;
    if base & (1 << 11) == 0 || base & (1 << 10) != 0 || base & (1 << 8) == 0 {
        return Err(Error::CpuMode);
    }
    if base & 0x000f_ffff_ffff_f000 != topology.lapic_address {
        return Err(Error::Address);
    }
    if !crate::hpet::authorize_development_mmio(
        regions,
        crate::hpet::DevelopmentResource::LocalApic,
        topology.lapic_address,
        4096,
    ) {
        return Err(Error::PhysicalRange);
    }
    let flags: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("cli", options(nostack));
    let mapped = crate::arch::mark_mmio_uncached(topology.lapic_address);
    if flags & (1 << 9) != 0 {
        core::arch::asm!("sti", options(nostack));
    }
    if !mapped {
        return Err(Error::CachePolicy);
    }
    let id = core::ptr::read_volatile((topology.lapic_address + 0x20) as *const u32);
    let version = core::ptr::read_volatile((topology.lapic_address + 0x30) as *const u32);
    local_info(
        topology.lapic_address,
        topology.bootstrap_apic_id,
        id,
        version,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::acpi::Cpu;
    fn fixture() -> (Topology, Controller) {
        let firmware = IoApic {
            id: 0,
            address: 0xfec00000,
            gsi_base: 0,
        };
        let topology = Topology {
            cpus: [Cpu {
                apic_id: 0,
                uid: 0,
                enabled: true,
                online_capable: false,
            }; 16],
            cpu_count: 1,
            io_apics: [firmware; 4],
            io_apic_count: 1,
            overrides: [InterruptOverride {
                source: 0,
                gsi: 2,
                flags: 0,
            }; 16],
            override_count: 1,
            lapic_address: 0xfee00000,
            bootstrap_apic_id: 0,
            pcat_compatible: true,
            unknown_madt_entries: 0,
            validated_tables: 1,
            uses_xsdt: true,
            hpet: None,
        };
        (
            topology,
            Controller::from_version(firmware, 0x0017_0011).unwrap(),
        )
    }
    #[test]
    fn qemu_isa_timer_override_skips_pic_cascade_without_aliasing() {
        let (t, c) = fixture();
        let plan = plan_isa(&t, &[c]).unwrap();
        assert_eq!(plan.len, 15);
        assert_eq!(
            plan.routes[0],
            Route {
                source: 0,
                gsi: 2,
                controller: 0,
                pin: 2,
                active_low: false,
                level: false
            }
        );
        assert!(!plan.routes[..plan.len].iter().any(|r| r.source == 2));
        assert_eq!(masked_entry(plan.routes[0], 32, 0).unwrap(), 0x0001_0020);
        assert_eq!(c.redirection_registers(23).unwrap(), (0x3e, 0x3f));
        assert_eq!(c.redirection_registers(24), Err(Error::Pins));
    }
    #[test]
    fn isa_flags_cover_all_encodings_and_reserved_bits() {
        for flags in 0..16 {
            if flags & 3 == 2 || flags >> 2 == 2 {
                assert_eq!(isa_flags(flags), Err(Error::Flags));
            } else {
                assert_eq!(isa_flags(flags), Ok((flags & 3 == 3, flags >> 2 == 3)));
            }
        }
        assert_eq!(isa_flags(16), Err(Error::Flags));
        assert_eq!(
            masked_entry(
                Route {
                    active_low: true,
                    level: true,
                    ..EMPTY_ROUTE
                },
                0x51,
                7
            )
            .unwrap(),
            0x0700_0000_0001_a051
        );
        for vector in [0, 31, 255] {
            assert_eq!(masked_entry(EMPTY_ROUTE, vector, 0), Err(Error::Vector));
        }
        for destination in [255, 256, u32::MAX] {
            assert_eq!(
                masked_entry(EMPTY_ROUTE, 32, destination),
                Err(Error::Destination)
            );
        }
    }
    #[test]
    fn duplicate_iso_and_default_gsi_collisions_fail_closed() {
        let (mut t, c) = fixture();
        t.override_count = 2;
        t.overrides[1] = InterruptOverride {
            source: 1,
            gsi: 2,
            flags: 0,
        };
        assert_eq!(plan_isa(&t, &[c]), Err(Error::Duplicate));
        t.override_count = 1;
        t.overrides[0].gsi = 1;
        assert_eq!(plan_isa(&t, &[c]), Err(Error::Duplicate));
        t.overrides[0].gsi = 100;
        assert_eq!(plan_isa(&t, &[c]), Err(Error::UnmappedGsi));
        t.override_count = 17;
        assert_eq!(plan_isa(&t, &[c]), Err(Error::Capacity));
    }
    #[test]
    fn controllers_require_nonoverlapping_bounded_pin_registers() {
        let (_, c) = fixture();
        let mut other = c;
        other.firmware.id = 1;
        other.firmware.address += 4096;
        other.firmware.gsi_base = 23;
        assert_eq!(validate_controllers(&[c, other]), Err(Error::Overlap));
        other.firmware.gsi_base = 24;
        assert_eq!(validate_controllers(&[c, other]), Ok(()));
        let mut bad = c.firmware;
        bad.gsi_base = u32::MAX;
        assert_eq!(Controller::from_version(bad, 0x0017_0011), Err(Error::Pins));
        assert_eq!(
            Controller::from_version(c.firmware, 0x0078_0011),
            Err(Error::Pins)
        );
        let maximum = Controller::from_version(c.firmware, 0x0077_0011).unwrap();
        assert_eq!(maximum.redirection_registers(119).unwrap(), (0xfe, 0xff));
        assert_eq!(
            Controller::from_version(c.firmware, 0x0100_0011),
            Err(Error::Version)
        );
        assert_eq!(controller_identity(c.firmware, 0, 0x0017_0011), Ok(c));
        assert_eq!(
            controller_identity(c.firmware, 1 << 24, 0x0017_0011),
            Err(Error::Identity)
        );
        assert_eq!(
            controller_identity(c.firmware, 1, 0x0017_0011),
            Err(Error::Identity)
        );
    }
    #[test]
    fn local_id_version_and_physical_destination_are_checked() {
        assert_eq!(
            local_info(0xfee00000, 3, 3 << 24, 0x0005_0014).unwrap(),
            LocalInfo {
                address: 0xfee00000,
                apic_id: 3,
                version: 0x14,
                lvt_entries: 6
            }
        );
        assert_eq!(
            local_info(0xfee00000, 3, 2 << 24, 0x0005_0014),
            Err(Error::Identity)
        );
        assert_eq!(
            local_info(0xfee00000, 3, (3 << 24) | 1, 0x0005_0014),
            Err(Error::Identity)
        );
        for raw in [0, 0x0007_0014, 0x0001_0014, 0x8005_0014] {
            assert_eq!(local_info(0xfee00000, 0, 0, raw), Err(Error::Version));
        }
        let (mut t, c) = fixture();
        t.bootstrap_apic_id = 256;
        assert_eq!(plan_isa(&t, &[c]), Err(Error::Destination));
        t.bootstrap_apic_id = 1;
        assert_eq!(plan_isa(&t, &[c]), Err(Error::Destination));
    }
}
