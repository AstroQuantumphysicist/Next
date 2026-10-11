//! Monotonic clock conversion for a periodic hardware counter. Wall-clock
//! settings never enter this contract. Saturation preserves ordering at limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickRate {
    source_hz: u32,
    divisor: u32,
}
impl TickRate {
    pub const fn new(source_hz: u32, divisor: u32) -> Option<Self> {
        if source_hz == 0 || divisor == 0 {
            None
        } else {
            Some(Self { source_hz, divisor })
        }
    }
    pub fn nanoseconds(self, ticks: u64) -> u64 {
        let value = u128::from(ticks) * u128::from(self.divisor) * 1_000_000_000;
        (value / u128::from(self.source_hz)).min(u128::from(u64::MAX)) as u64
    }
    pub fn deadline(self, ticks: u64, duration_ns: u64) -> u64 {
        self.nanoseconds(ticks).saturating_add(duration_ns)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_fractional_hardware_period_without_accumulated_rounding() {
        let rate = TickRate::new(1_193_182, 11_931).unwrap();
        assert_eq!(rate.nanoseconds(0), 0);
        assert_eq!(rate.nanoseconds(100), 999_931_276);
        assert!(rate.nanoseconds(100) > 100 * rate.nanoseconds(1));
        assert!(TickRate::new(0, 1).is_none());
        assert!(TickRate::new(1, 0).is_none());
    }
    #[test]
    fn deadline_and_conversion_saturate_without_wrapping() {
        let rate = TickRate::new(1, u32::MAX).unwrap();
        assert_eq!(rate.nanoseconds(u64::MAX), u64::MAX);
        assert_eq!(rate.deadline(u64::MAX, 1), u64::MAX);
        assert_eq!(TickRate::new(10, 1).unwrap().deadline(1, 500), 100_000_500);
    }
}
