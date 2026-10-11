//! q35 development source-route observation, never an activation permit.
//! QEMU10.0.13 resets LVT masks on INIT, but bus NMI/SMI/INIT deliveries bypass
//! them. Inspecting BSP LINT cannot certify AP pre-entry or external VMM control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Bounds,
    Unstable,
    UnmaskedNonmaskable,
    Firmware,
    Interrupts,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Redirection {
    pub low: u32,
    pub high: u32,
}
/// Allow only fixed/lowest-priority/ExtINT unmasked delivery. All other active
/// modes can bypass IF or reset/start CPUs. A masked route cannot deliver.
pub fn check_route(first: Redirection, second: Redirection) -> Result<(), Error> {
    // Delivery-status and remote-IRR may change while observing a live fixed
    // PIT route. They do not change mode/destination/mask ownership.
    let transient = (1 << 12) | (1 << 14);
    if (first.low ^ second.low) & !transient != 0 || first.high != second.high {
        return Err(Error::Unstable);
    }
    if second.low & (1 << 16) == 0 && !matches!((second.low >> 8) & 7, 0 | 1 | 7) {
        return Err(Error::UnmaskedNonmaskable);
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    controllers: usize,
    routes: usize,
}
impl Observation {
    pub fn controller_count(self) -> usize {
        self.controllers
    }
    pub fn route_count(self) -> usize {
        self.routes
    }
    /// A finite read snapshot does not own future writes or VMM injections.
    pub const fn authorizes_activation(self) -> bool {
        false
    }
}
/// # Safety
/// BSP IF clear/pre-SMP, sole owner of IOREGSEL and route writes. Validated MADT
/// and final map retained after firmware exit. No redirection entry is changed;
/// selector writes necessary for indexed reads are restored on every path.
#[cfg(target_os = "none")]
pub unsafe fn observe(
    topology: &crate::acpi::Topology,
    regions: &next_abi::boot::MemoryRegions<'_>,
) -> Result<Observation, Error> {
    let flags: u64;
    core::arch::asm!("pushfq", "pop {}",out(reg)flags,options(preserves_flags));
    if flags & (1 << 9) != 0 {
        return Err(Error::Interrupts);
    }
    let io = crate::apic::probe_io(topology, regions).map_err(|_| Error::Firmware)?;
    if io.len == 0 || io.len > 4 {
        return Err(Error::Bounds);
    }
    let mut routes = 0;
    for controller in &io.controllers[..io.len] {
        let selector = controller.firmware.address as *mut u32;
        let window = (controller.firmware.address as u64 + 0x10) as *const u32;
        let previous = core::ptr::read_volatile(selector);
        let result = (|| {
            for pin in 0..controller.pins {
                let (low, high) = controller
                    .redirection_registers(pin)
                    .map_err(|_| Error::Bounds)?;
                let read = |register: u8| {
                    core::ptr::write_volatile(selector, register as u32);
                    core::ptr::read_volatile(window)
                };
                let first = Redirection {
                    low: read(low),
                    high: read(high),
                };
                let second = Redirection {
                    low: read(low),
                    high: read(high),
                };
                check_route(first, second)?;
                routes += 1;
            }
            Ok(())
        })();
        core::ptr::write_volatile(selector, previous);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        result?;
    }
    Ok(Observation {
        controllers: io.len,
        routes,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_delivery_modes_require_masking_except_maskable_paths() {
        for mode in 0..8 {
            let active = Redirection {
                low: (mode << 8) | 0x51,
                high: 0x01000000,
            };
            assert_eq!(
                check_route(active, active),
                if matches!(mode, 0 | 1 | 7) {
                    Ok(())
                } else {
                    Err(Error::UnmaskedNonmaskable)
                }
            );
            let masked = Redirection {
                low: active.low | (1 << 16),
                ..active
            };
            assert_eq!(check_route(masked, masked), Ok(()));
        }
    }
    #[test]
    fn mutable_mode_mask_and_destination_are_not_quiescence_evidence() {
        let first = Redirection {
            low: 0x51,
            high: 0x01000000,
        };
        for bit in [0, 8, 9, 10, 11, 13, 15, 16, 17] {
            assert_eq!(
                check_route(
                    first,
                    Redirection {
                        low: first.low ^ (1 << bit),
                        ..first
                    }
                ),
                Err(Error::Unstable)
            );
        }
        assert_eq!(
            check_route(
                first,
                Redirection {
                    high: 0x02000000,
                    ..first
                }
            ),
            Err(Error::Unstable)
        );
        assert_eq!(
            check_route(
                first,
                Redirection {
                    low: first.low | (1 << 12) | (1 << 14),
                    ..first
                }
            ),
            Ok(())
        );
        assert!(!Observation {
            controllers: 1,
            routes: 24
        }
        .authorizes_activation());
    }
}
