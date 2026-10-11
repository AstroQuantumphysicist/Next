//! Bounded single-BSP LAPIC timer calibration against a validated HPET counter.
//! This runs masked countdowns only. It does not enable APIC IRQ delivery,
//! replace the PIT scheduler clock, start APs, or certify suspend/resume timing.
#[cfg(target_os = "none")]
use crate::{apic::LocalInfo, hpet::HpetInfo};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Resource,
    Reference,
    Busy,
    Stopped,
    Timeout,
    Unstable,
    Bounds,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub reference_ticks: u64,
    pub countdown_ticks: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    pub divider: u8,
    pub divided_hz: u64,
    pub samples: [Sample; 2],
}
impl Calibration {
    /// Round deadlines upward rather than firing before the requested duration.
    /// This returns a count only; it does not program or enable an IRQ timer.
    pub fn counts_for_ns(self, duration: u64) -> Result<u32, Error> {
        if duration == 0 || self.divided_hz == 0 {
            return Err(Error::Bounds);
        }
        let numerator = duration as u128 * self.divided_hz as u128;
        let count = numerator.div_ceil(1_000_000_000);
        u32::try_from(count).map_err(|_| Error::Bounds)
    }
}
fn sample_hz(period_fs: u32, sample: Sample) -> Result<u64, Error> {
    if period_fs == 0
        || period_fs > 100_000_000
        || sample.reference_ticks == 0
        || sample.reference_ticks >= 1u64 << 63
        || sample.countdown_ticks == 0
    {
        return Err(Error::Reference);
    }
    let elapsed_fs = sample.reference_ticks as u128 * period_fs as u128;
    // Reject implausibly short samples and host pauses over 100ms. The actual
    // hardware measurement aims for 10ms, so neither extreme is trustworthy.
    if !(5_000_000_000_000..=100_000_000_000_000).contains(&elapsed_fs) {
        return Err(Error::Reference);
    }
    let hz = sample.countdown_ticks as u128 * 1_000_000_000_000_000 / elapsed_fs;
    // Divided clock bounds cover the q35 reference while rejecting absent or
    // wrapped countdowns. They are an explicit development profile limit.
    if !(100_000..=1_000_000_000).contains(&hz) {
        return Err(Error::Bounds);
    }
    Ok(hz as u64)
}
pub fn validate_samples(period_fs: u32, samples: [Sample; 2]) -> Result<Calibration, Error> {
    let a = sample_hz(period_fs, samples[0])?;
    let b = sample_hz(period_fs, samples[1])?;
    let mean = (a + b) / 2;
    if a.abs_diff(b) as u128 * 100 > mean as u128 * 5 {
        return Err(Error::Unstable);
    }
    Ok(Calibration {
        divider: 16,
        divided_hz: mean,
        samples,
    })
}
#[cfg(target_os = "none")]
unsafe fn read(address: u64, offset: u64) -> u32 {
    core::ptr::read_volatile((address + offset) as *const u32)
}
#[cfg(target_os = "none")]
unsafe fn write(address: u64, offset: u64, value: u32) {
    core::ptr::write_volatile((address + offset) as *mut u32, value);
    // Drain the UC APIC write through a read of an architecturally valid register.
    let _ = read(address, 0x20);
}
#[cfg(target_os = "none")]
unsafe fn interrupt_guard<T>(operation: impl FnOnce() -> T) -> T {
    let flags: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("cli", options(nostack));
    let result = operation();
    if flags & (1 << 9) != 0 {
        core::arch::asm!("sti", options(nostack));
    }
    result
}
#[cfg(target_os = "none")]
struct Restore {
    address: u64,
    lvt: u32,
    divide: u32,
    initial: u32,
}
#[cfg(target_os = "none")]
impl Drop for Restore {
    fn drop(&mut self) {
        unsafe {
            interrupt_guard(|| {
                write(self.address, 0x320, self.lvt | (1 << 16));
                write(self.address, 0x380, 0);
                write(self.address, 0x3e0, self.divide);
                write(self.address, 0x380, self.initial);
                write(self.address, 0x320, self.lvt);
            });
        }
    }
}
/// # Safety
/// Sole BSP owns a stopped, masked LAPIC timer. Inputs came from successful
/// LAPIC/HPET probes with owned UC mappings. No interrupt handler modifies timer
/// controls; the reference HPET counter stays enabled. Existing IF state is
/// preserved, with IRQs masked only during register transactions. The caller
/// must tolerate a bounded calibration pause before scheduling user processes.
#[cfg(target_os = "none")]
pub unsafe fn calibrate(
    local: LocalInfo,
    reference: HpetInfo,
    regions: &next_abi::boot::MemoryRegions<'_>,
) -> Result<Calibration, Error> {
    use crate::hpet::{authorize_development_mmio, DevelopmentResource};
    if !authorize_development_mmio(regions, DevelopmentResource::LocalApic, local.address, 4096)
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
    let restore = interrupt_guard(|| {
        let lvt = read(local.address, 0x320);
        let divide = read(local.address, 0x3e0);
        let initial = read(local.address, 0x380);
        if lvt & (1 << 16) == 0 || initial != 0 || read(local.address, 0x390) != 0 {
            return Err(Error::Busy);
        }
        write(local.address, 0x320, (lvt | (1 << 16)) & !(3 << 17));
        write(local.address, 0x3e0, 3); // divide by16, one-shot and always masked
        Ok(Restore {
            address: local.address,
            lvt,
            divide,
            initial,
        })
    })?;
    let counter = (reference.address + 0xf0) as *const u64;
    let target = 10_000_000_000_000u64.div_ceil(reference.period_fs as u64);
    let mut samples = [Sample {
        reference_ticks: 0,
        countdown_ticks: 0,
    }; 2];
    for sample in &mut samples {
        let (first_count, first_reference) = interrupt_guard(|| {
            write(local.address, 0x380, u32::MAX);
            (
                read(local.address, 0x390),
                core::ptr::read_volatile(counter),
            )
        });
        let mut completed = false;
        for _ in 0..10_000_000 {
            let elapsed = core::ptr::read_volatile(counter).wrapping_sub(first_reference);
            if elapsed >= 1u64 << 63 {
                return Err(Error::Reference);
            }
            if elapsed >= target {
                let current = read(local.address, 0x390);
                if current == 0 || current >= first_count {
                    return Err(Error::Stopped);
                }
                *sample = Sample {
                    reference_ticks: elapsed,
                    countdown_ticks: first_count - current,
                };
                completed = true;
                break;
            }
            core::hint::spin_loop();
        }
        interrupt_guard(|| write(local.address, 0x380, 0));
        if !completed {
            return Err(Error::Timeout);
        }
    }
    let calibration = validate_samples(reference.period_fs, samples);
    drop(restore);
    calibration
}
#[cfg(test)]
mod tests {
    use super::*;
    fn samples() -> [Sample; 2] {
        [
            Sample {
                reference_ticks: 1_000_000,
                countdown_ticks: 625_000,
            },
            Sample {
                reference_ticks: 1_010_000,
                countdown_ticks: 631_250,
            },
        ]
    }
    #[test]
    fn independent_100mhz_reference_and_1ghz_divided_by16_calibrate() {
        let info = validate_samples(10_000_000, samples()).unwrap();
        assert_eq!(info.divider, 16);
        assert_eq!(info.divided_hz, 62_500_000);
        assert_eq!(info.counts_for_ns(1_000_000), Ok(62_500));
        assert_eq!(info.counts_for_ns(1), Ok(1));
        assert_eq!(info.counts_for_ns(0), Err(Error::Bounds));
        assert_eq!(info.counts_for_ns(u64::MAX), Err(Error::Bounds));
    }
    #[test]
    fn mismatched_samples_short_long_stopped_and_wrapped_references_fail() {
        let mut bad = samples();
        bad[1].countdown_ticks *= 2;
        assert_eq!(validate_samples(10_000_000, bad), Err(Error::Unstable));
        for ticks in [0, 1, 10_000_001, 1u64 << 63, u64::MAX] {
            bad = samples();
            bad[0].reference_ticks = ticks;
            assert_eq!(validate_samples(10_000_000, bad), Err(Error::Reference));
        }
        bad = samples();
        bad[0].countdown_ticks = 0;
        assert_eq!(validate_samples(10_000_000, bad), Err(Error::Reference));
        for period in [0, 100_000_001, u32::MAX] {
            assert_eq!(validate_samples(period, samples()), Err(Error::Reference));
        }
    }
    #[test]
    fn plausible_frequency_and_five_percent_tolerance_are_bounded() {
        let mut value = samples();
        value[1] = value[0];
        value[1].countdown_ticks = value[0].countdown_ticks * 104 / 100;
        assert!(validate_samples(10_000_000, value).is_ok());
        value[1].countdown_ticks = value[0].countdown_ticks * 106 / 100;
        assert_eq!(validate_samples(10_000_000, value), Err(Error::Unstable));
        value = samples();
        value[0].countdown_ticks = 1;
        assert_eq!(validate_samples(10_000_000, value), Err(Error::Bounds));
        value[0].countdown_ticks = u32::MAX;
        assert_eq!(validate_samples(10_000_000, value), Err(Error::Bounds));
    }
}
