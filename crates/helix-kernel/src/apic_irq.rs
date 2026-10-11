//! Transaction contract for switching the existing PIT IRQ0 from PIC to a
//! validated IOAPIC route. Hardware integration is staged separately; this
//! module alone never claims interrupt delivery or changes controller registers.
use crate::apic::{masked_entry, IoInfo};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Bounds,
    Route,
    Busy,
    Hardware,
    NoDelivery,
    UnsafeRollback,
    Pending,
    NoFallback,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PitRoute {
    pub address: u64,
    pub gsi: u32,
    pub low_register: u8,
    pub high_register: u8,
    pub low_masked: u32,
    pub low_active: u32,
    pub high: u32,
}
impl PitRoute {
    pub fn prepare(info: &IoInfo, previous: u64) -> Result<Self, Error> {
        if info.len == 0 || info.len > 4 || info.plan.len > 16 {
            return Err(Error::Bounds);
        }
        let mut candidates = info.plan.routes[..info.plan.len]
            .iter()
            .filter(|r| r.source == 0);
        let route = *candidates.next().ok_or(Error::Route)?;
        if candidates.next().is_some()
            || route.active_low
            || route.level
            || route.controller as usize >= info.len
        {
            return Err(Error::Route);
        }
        let controller = info.controllers[route.controller as usize];
        if controller.firmware.gsi_base.checked_add(route.pin as u32) != Some(route.gsi) {
            return Err(Error::Route);
        }
        let (low_register, high_register) = controller
            .redirection_registers(route.pin)
            .map_err(|_| Error::Route)?;
        if previous & (1 << 16) == 0 || previous & ((1 << 12) | (1 << 14)) != 0 {
            return Err(Error::Busy);
        }
        let entry =
            masked_entry(route, 0x20, info.plan.destination as u32).map_err(|_| Error::Route)?;
        Ok(Self {
            address: controller.firmware.address as u64,
            gsi: route.gsi,
            low_register,
            high_register,
            low_masked: entry as u32,
            low_active: entry as u32 & !(1 << 16),
            high: (entry >> 32) as u32,
        })
    }
}
/// Writable bits only: delivery status and remote IRR are hardware-owned.
pub fn redirection_matches(actual: u64, expected: u64) -> bool {
    const WRITABLE: u64 = 0xff00_0000_0001_afff;
    actual & WRITABLE == expected & WRITABLE
}
/// Hardware owner must implement each mutation under IF-clear register guards,
/// read back controls, preserve IOREGSEL, and save the complete original state.
/// configure_masked writes a masked low word before changing the destination,
/// then destination-high and final masked-low. unmask is the last write.
/// apic_deliveries counts IRQ0 handlers observing LAPIC ISR[0x20] and issuing
/// EOI, not elapsed PIT ticks. pic_deliveries counts actual PIC ISR IRQ0.
/// pending includes both LAPIC IRR and ISR for vector0x20. polling permits IRQs
/// and has no allocation/blocking; each wait is additionally bounded here.
pub trait Hardware {
    fn configure_masked(&mut self, route: PitRoute) -> Result<(), Error>;
    fn publish_eoi_owner(&mut self) -> Result<(), Error>;
    fn enable_local(&mut self) -> Result<(), Error>;
    fn mask_pic_irq0(&mut self) -> Result<(), Error>;
    fn unmask_route(&mut self, route: PitRoute) -> Result<(), Error>;
    fn apic_deliveries(&self) -> u64;
    fn pic_deliveries(&self) -> u64;
    fn pending(&self) -> bool;
    fn poll(&mut self);
    /// An independent clock deadline; hardware must not depend on PIT delivery.
    /// Each activation, drain and fallback phase starts its own bounded window.
    fn wait_expired(&self) -> bool {
        false
    }
    fn mask_route(&mut self, route: PitRoute) -> Result<(), Error>;
    fn restore_route(&mut self) -> Result<(), Error>;
    fn restore_local(&mut self) -> Result<(), Error>;
    fn withdraw_eoi_owner(&mut self);
    fn restore_pic(&mut self) -> Result<(), Error>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub apic_interrupts: u64,
}
fn rollback(hardware: &mut impl Hardware, route: PitRoute, budget: usize) -> Result<(), Error> {
    // Failure to prove masking is a fatal ownership failure: never introduce a
    // second active source by guessing that PIC fallback is safe.
    hardware
        .mask_route(route)
        .map_err(|_| Error::UnsafeRollback)?;
    for _ in 0..budget {
        if !hardware.pending() {
            break;
        }
        hardware.poll();
        if hardware.wait_expired() {
            break;
        }
    }
    let pending = hardware.pending();
    hardware
        .restore_route()
        .map_err(|_| Error::UnsafeRollback)?;
    if !pending {
        hardware
            .restore_local()
            .map_err(|_| Error::UnsafeRollback)?;
        hardware.withdraw_eoi_owner();
    }
    // If the vector did not drain, keep LAPIC enabled and retain its EOI owner.
    // Original route was masked, so enabling PIC creates no new APIC source.
    let before = hardware.pic_deliveries();
    hardware.restore_pic().map_err(|_| Error::UnsafeRollback)?;
    let mut fallback = false;
    for _ in 0..budget {
        hardware.poll();
        if hardware.wait_expired() {
            break;
        }
        if hardware.pic_deliveries().saturating_sub(before) >= 3 {
            fallback = true;
            break;
        }
    }
    if !fallback {
        return Err(Error::NoFallback);
    }
    if pending {
        return Err(Error::Pending);
    }
    Ok(())
}
/// Attempt a real route and retain it only after three ISR-observed deliveries.
/// On ordinary failure, restore controls and verify three genuine PIC IRQs.
/// UnsafeRollback/NoFallback require a boot diagnostic and refusal to proceed.
/// Pending retains the EOI owner for late APIC IRQs and reports degraded state.
pub fn activate(
    hardware: &mut impl Hardware,
    route: PitRoute,
    budget: usize,
) -> Result<Delivery, Error> {
    if budget == 0 || budget > 100_000_000 {
        return Err(Error::Bounds);
    }
    let before = hardware.apic_deliveries();
    let attempt = (|| {
        hardware.configure_masked(route)?;
        hardware.publish_eoi_owner()?;
        hardware.enable_local()?;
        hardware.mask_pic_irq0()?;
        hardware.unmask_route(route)?;
        for _ in 0..budget {
            hardware.poll();
            if hardware.wait_expired() {
                break;
            }
            let delivered = hardware.apic_deliveries().saturating_sub(before);
            if delivered >= 3 {
                return Ok(Delivery {
                    apic_interrupts: delivered,
                });
            }
        }
        Err(Error::NoDelivery)
    })();
    if let Err(error) = attempt {
        rollback(hardware, route, budget)?;
        return Err(error);
    }
    attempt
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acpi::IoApic,
        apic::{Controller, Plan, Route},
    };
    fn route_fixture() -> (IoInfo, PitRoute) {
        let controller = Controller {
            firmware: IoApic {
                id: 0,
                address: 0xfec00000,
                gsi_base: 0,
            },
            pins: 24,
        };
        let route = Route {
            source: 0,
            gsi: 2,
            controller: 0,
            pin: 2,
            active_low: false,
            level: false,
        };
        let info = IoInfo {
            controllers: [controller; 4],
            len: 1,
            plan: Plan {
                routes: [route; 16],
                len: 1,
                destination: 0,
            },
        };
        let prepared = PitRoute::prepare(&info, 1 << 16).unwrap();
        (info, prepared)
    }
    #[derive(Default)]
    struct Model {
        step: usize,
        fail_step: Option<usize>,
        fail_after: bool,
        rollback_step: usize,
        fail_rollback: Option<usize>,
        rollback_after: bool,
        configured: bool,
        owner: bool,
        local: bool,
        pic_masked: bool,
        active: bool,
        queued: usize,
        stuck: bool,
        apic: u64,
        pic: u64,
        no_apic: bool,
        no_pic: bool,
        unsafe_mask: bool,
        inject_after_delivery: bool,
    }
    impl Model {
        fn restoring(&mut self, change: impl FnOnce(&mut Self)) -> Result<(), Error> {
            let fail = self.fail_rollback == Some(self.rollback_step);
            self.rollback_step += 1;
            if fail && !self.rollback_after {
                return Err(Error::Hardware);
            }
            change(self);
            if fail {
                Err(Error::Hardware)
            } else {
                Ok(())
            }
        }
        fn mutation(&mut self, change: impl FnOnce(&mut Self)) -> Result<(), Error> {
            let fail = self.fail_step == Some(self.step);
            self.step += 1;
            if fail && !self.fail_after {
                return Err(Error::Hardware);
            }
            change(self);
            if fail {
                Err(Error::Hardware)
            } else {
                Ok(())
            }
        }
    }
    impl Hardware for Model {
        fn configure_masked(&mut self, _: PitRoute) -> Result<(), Error> {
            self.mutation(|s| {
                s.configured = true;
                s.active = false;
            })
        }
        fn publish_eoi_owner(&mut self) -> Result<(), Error> {
            self.mutation(|s| s.owner = true)
        }
        fn enable_local(&mut self) -> Result<(), Error> {
            self.mutation(|s| {
                assert!(s.owner);
                s.local = true;
            })
        }
        fn mask_pic_irq0(&mut self) -> Result<(), Error> {
            self.mutation(|s| s.pic_masked = true)
        }
        fn unmask_route(&mut self, _: PitRoute) -> Result<(), Error> {
            self.mutation(|s| {
                assert!(s.owner && s.local && s.pic_masked);
                s.active = true;
                s.queued += 1;
            })?;
            if self.inject_after_delivery {
                self.poll();
                assert_eq!(self.apic, 1);
                return Err(Error::Hardware);
            }
            Ok(())
        }
        fn apic_deliveries(&self) -> u64 {
            self.apic
        }
        fn pic_deliveries(&self) -> u64 {
            self.pic
        }
        fn pending(&self) -> bool {
            self.queued != 0
        }
        fn poll(&mut self) {
            if self.local
                && self.owner
                && !self.no_apic
                && !self.stuck
                && (self.active || self.queued != 0)
            {
                self.apic += 1;
                self.queued = self.queued.saturating_sub(1);
            }
            if !self.pic_masked && !self.no_pic {
                self.pic += 1;
            }
        }
        fn mask_route(&mut self, _: PitRoute) -> Result<(), Error> {
            if self.unsafe_mask {
                return Err(Error::Hardware);
            }
            self.restoring(|s| s.active = false)
        }
        fn restore_route(&mut self) -> Result<(), Error> {
            assert!(!self.active);
            self.restoring(|s| s.configured = false)
        }
        fn restore_local(&mut self) -> Result<(), Error> {
            assert!(!self.pending());
            self.restoring(|s| s.local = false)
        }
        fn withdraw_eoi_owner(&mut self) {
            assert!(!self.pending());
            self.owner = false;
        }
        fn restore_pic(&mut self) -> Result<(), Error> {
            assert!(!self.active);
            self.restoring(|s| s.pic_masked = false)
        }
    }
    #[test]
    fn every_mutation_failure_before_and_after_restores_pic_and_drains_pending() {
        let (_, route) = route_fixture();
        for step in 0..5 {
            for fail_after in [false, true] {
                let mut hw = Model {
                    fail_step: Some(step),
                    fail_after,
                    ..Model::default()
                };
                assert_eq!(activate(&mut hw, route, 8), Err(Error::Hardware));
                assert!(!hw.active && !hw.configured && !hw.owner && !hw.local && !hw.pic_masked);
                assert_eq!(hw.queued, 0);
                assert!(hw.pic >= 3);
            }
        }
    }
    #[test]
    fn every_failed_rollback_mutation_refuses_unverified_recovery() {
        let (_, route) = route_fixture();
        for step in 0..4 {
            for after in [false, true] {
                let mut hw = Model {
                    fail_step: Some(4),
                    fail_after: true,
                    fail_rollback: Some(step),
                    rollback_after: after,
                    ..Model::default()
                };
                assert_eq!(activate(&mut hw, route, 8), Err(Error::UnsafeRollback));
                if step < 3 || !after {
                    assert!(hw.pic_masked);
                }
                if step != 0 || after {
                    assert!(!hw.active);
                }
            }
        }
    }
    #[test]
    fn injected_failure_after_one_real_apic_irq_restores_and_proves_three_pic_irqs() {
        let (_, route) = route_fixture();
        let mut hw = Model {
            inject_after_delivery: true,
            ..Model::default()
        };
        assert_eq!(activate(&mut hw, route, 8), Err(Error::Hardware));
        assert_eq!(hw.apic, 1);
        assert_eq!(hw.pic, 3);
        assert!(!hw.active && !hw.configured && !hw.owner && !hw.local && !hw.pic_masked);
        assert!(!hw.pending());
    }
    #[test]
    fn success_requires_actual_apic_delivery_and_stalled_route_proves_pic_fallback() {
        let (_, route) = route_fixture();
        let mut hw = Model::default();
        assert_eq!(
            activate(&mut hw, route, 8),
            Ok(Delivery { apic_interrupts: 3 })
        );
        assert!(hw.active && hw.owner && hw.local && hw.pic_masked);
        let mut absent = Model {
            no_apic: true,
            ..Model::default()
        };
        assert_eq!(activate(&mut absent, route, 8), Err(Error::Pending));
        assert!(!absent.active && !absent.pic_masked && absent.owner && absent.local);
        assert!(absent.pic >= 3);
        let mut dead = Model {
            no_apic: true,
            no_pic: true,
            ..Model::default()
        };
        assert_eq!(activate(&mut dead, route, 8), Err(Error::NoFallback));
    }
    #[test]
    fn pending_owner_is_retained_and_unproven_mask_never_enables_pic() {
        let (_, route) = route_fixture();
        let mut hw = Model {
            fail_step: Some(4),
            fail_after: true,
            stuck: true,
            ..Model::default()
        };
        assert_eq!(activate(&mut hw, route, 8), Err(Error::Pending));
        assert!(hw.owner && hw.local && hw.pending() && !hw.pic_masked && !hw.active);
        let mut unsafe_hw = Model {
            fail_step: Some(4),
            fail_after: true,
            unsafe_mask: true,
            ..Model::default()
        };
        assert_eq!(
            activate(&mut unsafe_hw, route, 8),
            Err(Error::UnsafeRollback)
        );
        assert!(unsafe_hw.pic_masked);
    }
    #[test]
    fn preparation_refuses_live_level_polarity_and_inconsistent_resource_routes() {
        let (mut info, prepared) = route_fixture();
        assert_eq!(
            (prepared.low_register, prepared.high_register),
            (0x14, 0x15)
        );
        assert_eq!((prepared.low_masked, prepared.low_active), (0x10020, 0x20));
        for previous in [0, (1 << 16) | (1 << 12), (1 << 16) | (1 << 14)] {
            assert_eq!(PitRoute::prepare(&info, previous), Err(Error::Busy));
        }
        info.plan.routes[0].level = true;
        assert_eq!(PitRoute::prepare(&info, 1 << 16), Err(Error::Route));
        info.plan.routes[0].level = false;
        info.plan.routes[0].active_low = true;
        assert_eq!(PitRoute::prepare(&info, 1 << 16), Err(Error::Route));
        info.plan.routes[0].active_low = false;
        info.plan.routes[0].gsi = 3;
        assert_eq!(PitRoute::prepare(&info, 1 << 16), Err(Error::Route));
        assert!(redirection_matches(
            0x10020 | (1 << 12) | (1 << 14),
            0x10020
        ));
        assert!(!redirection_matches(0x10021, 0x10020));
    }
}
