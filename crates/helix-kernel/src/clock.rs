//! Immutable q35 UTC anchor plus elapsed monotonic time. No RTC failure is
//! replaced with an invented date. The RTC has one-second anchor precision;
//! neither clock adjustments nor production timezone/NTP policy are present.
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};
use next_abi::Timespec;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    AlreadyInitialized,
    Unavailable,
    Rtc(crate::rtc::Error),
    Overflow,
    Regression,
}
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    utc_seconds: i64,
    monotonic_ns: u64,
}
impl Anchor {
    pub fn new(utc_seconds: u64, monotonic_ns: u64) -> Result<Self, Error> {
        Ok(Self {
            utc_seconds: i64::try_from(utc_seconds).map_err(|_| Error::Overflow)?,
            monotonic_ns,
        })
    }
    pub fn at(self, now_ns: u64) -> Result<Timespec, Error> {
        let elapsed = now_ns
            .checked_sub(self.monotonic_ns)
            .ok_or(Error::Regression)?;
        let seconds = self
            .utc_seconds
            .checked_add((elapsed / 1_000_000_000) as i64)
            .ok_or(Error::Overflow)?;
        Ok(Timespec {
            seconds,
            nanoseconds: (elapsed % 1_000_000_000) as i64,
        })
    }
}
struct Global(UnsafeCell<MaybeUninit<Anchor>>);
// One initialize winner publishes immutable storage with release/acquire.
unsafe impl Sync for Global {}
static ANCHOR: Global = Global(UnsafeCell::new(MaybeUninit::uninit()));
static STATE: AtomicU8 = AtomicU8::new(0);
static LAST_MONOTONIC_NS: AtomicU64 = AtomicU64::new(0);
fn select_monotonic_ns(pit_ns: u64, hpet_ns: Option<u64>, previous_ns: u64) -> u64 {
    // If HPET is available, use its validated sub-PIT-tick resolution. When it
    // stalls or becomes unavailable, return to PIT and hold the last observed
    // time until that fallback catches up. This preserves monotonicity across
    // source changes without making scheduler deadlines depend on HPET.
    hpet_ns
        .filter(|value| *value > previous_ns || pit_ns <= previous_ns)
        .unwrap_or(pit_ns)
        .max(previous_ns)
}
fn monotonic_nanoseconds() -> u64 {
    let pit_ns = crate::timer::nanoseconds();
    let previous_ns = LAST_MONOTONIC_NS.load(Ordering::Acquire);
    let candidate = select_monotonic_ns(pit_ns, crate::hpet::monotonic_ns(), previous_ns);
    let raced = LAST_MONOTONIC_NS.fetch_max(candidate, Ordering::AcqRel);
    candidate.max(raced)
}
/// Exactly one attempt. Even an unavailable RTC cannot later be silently
/// substituted/reanchored; consumers see Unavailable for that boot.
#[cfg(target_os = "none")]
pub fn initialize() -> Result<(), Error> {
    STATE
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| Error::AlreadyInitialized)?;
    let result = unsafe { crate::rtc::read_utc() }
        .map_err(Error::Rtc)
        .and_then(|utc| Anchor::new(utc.seconds, monotonic_nanoseconds()));
    match result {
        Ok(anchor) => {
            unsafe {
                (*ANCHOR.0.get()).write(anchor);
            }
            STATE.store(2, Ordering::Release);
            Ok(())
        }
        Err(error) => {
            STATE.store(3, Ordering::Release);
            Err(error)
        }
    }
}
pub fn realtime() -> Result<Timespec, Error> {
    if STATE.load(Ordering::Acquire) != 2 {
        return Err(Error::Unavailable);
    }
    unsafe {
        (*ANCHOR.0.get())
            .assume_init_ref()
            .at(monotonic_nanoseconds())
    }
}
pub fn monotonic() -> Timespec {
    let now = monotonic_nanoseconds();
    Timespec {
        seconds: (now / 1_000_000_000) as i64,
        nanoseconds: (now % 1_000_000_000) as i64,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utc_epoch_and_leap_calendar_anchor_use_elapsed_not_uptime() {
        let epoch = crate::rtc::Calendar {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }
        .to_unix()
        .unwrap();
        assert_eq!(
            Anchor::new(epoch.seconds, 9_000_000_000)
                .unwrap()
                .at(10_234_567_890),
            Ok(Timespec {
                seconds: 1,
                nanoseconds: 234_567_890
            })
        );
        let leap = crate::rtc::Calendar {
            year: 2000,
            month: 2,
            day: 29,
            hour: 23,
            minute: 59,
            second: 59,
        }
        .to_unix()
        .unwrap();
        assert_eq!(leap.seconds, 951868799);
        let clock = Anchor::new(leap.seconds, 50).unwrap();
        assert_eq!(
            clock.at(1_000_000_050),
            Ok(Timespec {
                seconds: 951868800,
                nanoseconds: 0
            })
        );
        assert!(crate::rtc::Calendar {
            year: 2100,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0
        }
        .to_unix()
        .is_err());
    }
    #[test]
    fn elapsed_regression_seconds_overflow_and_max_tick_fail_or_normalize_exactly() {
        assert_eq!(Anchor::new(u64::MAX, 0).unwrap_err(), Error::Overflow);
        assert_eq!(Anchor::new(0, 100).unwrap().at(99), Err(Error::Regression));
        assert_eq!(
            Anchor::new(i64::MAX as u64, 0).unwrap().at(1_000_000_000),
            Err(Error::Overflow)
        );
        let result = Anchor::new(0, 0).unwrap().at(u64::MAX).unwrap();
        assert_eq!(result.seconds, (u64::MAX / 1_000_000_000) as i64);
        assert_eq!(result.nanoseconds, (u64::MAX % 1_000_000_000) as i64);
    }
    #[test]
    fn repeated_reads_preserve_anchor_and_subsecond_progress() {
        let anchor = Anchor::new(1_700_000_000, 5_999_999_999).unwrap();
        assert_eq!(
            anchor.at(5_999_999_999),
            Ok(Timespec {
                seconds: 1_700_000_000,
                nanoseconds: 0
            })
        );
        assert_eq!(
            anchor.at(6_000_000_000),
            Ok(Timespec {
                seconds: 1_700_000_000,
                nanoseconds: 1
            })
        );
        let first = anchor.at(7_123_456_789).unwrap();
        assert_eq!(anchor.at(7_123_456_789), Ok(first));
        assert_eq!(anchor.at(8_123_456_789).unwrap().seconds, first.seconds + 1);
    }
    #[test]
    fn source_selection_keeps_time_monotonic_during_hpet_failover() {
        assert_eq!(select_monotonic_ns(100, Some(101), 100), 101);
        // A stopped HPET must not freeze time after PIT has passed it.
        assert_eq!(select_monotonic_ns(120, Some(101), 110), 120);
        // If fallback lags the last HPET value, hold time rather than regress.
        assert_eq!(select_monotonic_ns(105, None, 110), 110);
        assert_eq!(select_monotonic_ns(111, None, 110), 111);
    }
}
