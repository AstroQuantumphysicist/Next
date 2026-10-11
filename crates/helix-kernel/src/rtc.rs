//! q35 development-profile CMOS UTC anchor. Century register 0x32 is an
//! explicit platform policy, not generic FADT discovery. No RTC data, alarm,
//! status, or clock-setting register is written. Precision is one second;
//! elapsed subsecond time belongs to the monotonic timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    Inconsistent,
    InvalidBcd,
    Calendar,
    Year,
    Battery,
    Updating,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnixTime {
    pub seconds: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calendar {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}
const fn leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}
fn month_days(year: u16, month: u8) -> Option<u8> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if leap(year) { 29 } else { 28 }),
        _ => None,
    }
}
impl Calendar {
    pub fn to_unix(self) -> Result<UnixTime, Error> {
        if !(1970..=9999).contains(&self.year) {
            return Err(Error::Year);
        }
        let max_day = month_days(self.year, self.month).ok_or(Error::Calendar)?;
        if self.day == 0
            || self.day > max_day
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
        {
            return Err(Error::Calendar);
        }
        // The bounded year range caps this loop at 8029 iterations and avoids
        // signed pre-epoch arithmetic or unchecked multiplication.
        let mut days = 0u64;
        for year in 1970..self.year {
            days += if leap(year) { 366 } else { 365 };
        }
        for month in 1..self.month {
            days += month_days(self.year, month).ok_or(Error::Calendar)? as u64;
        }
        days += self.day as u64 - 1;
        let seconds = days
            .checked_mul(86400)
            .and_then(|s| {
                s.checked_add(
                    self.hour as u64 * 3600 + self.minute as u64 * 60 + self.second as u64,
                )
            })
            .ok_or(Error::Year)?;
        Ok(UnixTime { seconds })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Raw {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
    status_d: u8,
}
fn numeric(value: u8, binary: bool) -> Result<u8, Error> {
    if binary {
        return Ok(value);
    }
    if value & 15 > 9 || value >> 4 > 9 {
        return Err(Error::InvalidBcd);
    }
    Ok((value >> 4) * 10 + (value & 15))
}
fn decode(raw: Raw) -> Result<Calendar, Error> {
    if raw.status_d & 0x80 == 0 {
        return Err(Error::Battery);
    }
    if raw.status_b & 0x80 != 0 {
        return Err(Error::Updating);
    }
    let binary = raw.status_b & 4 != 0;
    let mode24 = raw.status_b & 2 != 0;
    let pm = raw.hour & 0x80 != 0;
    if mode24 && pm {
        return Err(Error::Calendar);
    }
    let mut hour = numeric(raw.hour & 0x7f, binary)?;
    if !mode24 {
        if !(1..=12).contains(&hour) {
            return Err(Error::Calendar);
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    let century = numeric(raw.century, binary)?;
    let year = numeric(raw.year, binary)?;
    if year > 99 || !(19..=99).contains(&century) {
        return Err(Error::Year);
    }
    let calendar = Calendar {
        year: century as u16 * 100 + year as u16,
        month: numeric(raw.month, binary)?,
        day: numeric(raw.day, binary)?,
        hour,
        minute: numeric(raw.minute, binary)?,
        second: numeric(raw.second, binary)?,
    };
    calendar.to_unix()?;
    Ok(calendar)
}
#[cfg(target_os = "none")]
unsafe fn port_in(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
    value
}
#[cfg(target_os = "none")]
unsafe fn select(value: u8) {
    core::arch::asm!("out dx, al", in("dx") 0x70u16, in("al") value, options(nomem, nostack));
}
#[cfg(target_os = "none")]
unsafe fn register(index: u8) -> u8 {
    select(0x80 | index); // Mask NMI while changing its shared CMOS selector.
    port_in(0x71)
}
#[cfg(target_os = "none")]
unsafe fn snapshot() -> Result<Raw, Error> {
    let mut ready = false;
    for _ in 0..10000 {
        if register(0x0a) & 0x80 == 0 {
            ready = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !ready {
        return Err(Error::Timeout);
    }
    let raw = Raw {
        second: register(0),
        minute: register(2),
        hour: register(4),
        day: register(7),
        month: register(8),
        year: register(9),
        century: register(0x32),
        status_b: register(0x0b),
        status_d: register(0x0d),
    };
    if register(0x0a) & 0x80 != 0 {
        return Err(Error::Updating);
    }
    Ok(raw)
}
/// # Safety
/// Sole q35 bootstrap CPU owns CMOS access; firmware has exited and no other
/// CPU/NMI handler touches ports 0x70/0x71. The development RTC stores UTC and
/// implements readable selector state and century register 0x32. IF and the
/// exact selector/NMI mask are restored on success and every bounded failure.
#[cfg(target_os = "none")]
pub unsafe fn read_utc() -> Result<UnixTime, Error> {
    let flags: u64;
    core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    core::arch::asm!("cli", options(nostack));
    let previous = port_in(0x70);
    let result = (|| {
        for _ in 0..8 {
            let first = match snapshot() {
                Ok(value) => value,
                Err(Error::Updating) => continue,
                Err(error) => return Err(error),
            };
            let second = match snapshot() {
                Ok(value) => value,
                Err(Error::Updating) => continue,
                Err(error) => return Err(error),
            };
            if first == second {
                return decode(first)?.to_unix();
            }
        }
        Err(Error::Inconsistent)
    })();
    select(previous);
    if flags & (1 << 9) != 0 {
        core::arch::asm!("sti", options(nostack));
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn raw() -> Raw {
        Raw {
            second: 0x56,
            minute: 0x34,
            hour: 0x12,
            day: 0x29,
            month: 0x02,
            year: 0x24,
            century: 0x20,
            status_b: 2,
            status_d: 0x80,
        }
    }
    #[test]
    fn independent_epoch_vectors_and_gregorian_centuries() {
        for (year, month, day, expected) in [
            (1970, 1, 1, 0),
            (2000, 1, 1, 946684800),
            (2024, 2, 29, 1709164800),
            (2100, 3, 1, 4107542400),
            (9999, 12, 31, 253402214400),
        ] {
            assert_eq!(
                Calendar {
                    year,
                    month,
                    day,
                    hour: 0,
                    minute: 0,
                    second: 0
                }
                .to_unix()
                .unwrap()
                .seconds,
                expected
            );
        }
        assert!(leap(2000));
        assert!(!leap(1900));
        assert!(!leap(2100));
        assert!(leap(2400));
    }
    #[test]
    fn bcd_binary_and_twelve_hour_noon_midnight_pm() {
        let bcd = decode(raw()).unwrap();
        assert_eq!(bcd.hour, 12);
        let mut value = raw();
        value.status_b = 0;
        assert_eq!(decode(value).unwrap().hour, 0);
        value.hour = 0x92;
        assert_eq!(decode(value).unwrap().hour, 12);
        value.hour = 0x81;
        assert_eq!(decode(value).unwrap().hour, 13);
        value.status_b = 4;
        value.year = 24;
        value.century = 20;
        value.month = 2;
        value.day = 29;
        value.minute = 34;
        value.second = 56;
        value.hour = 0x80 | 12;
        assert_eq!(decode(value).unwrap(), bcd);
        value.status_b = 6;
        value.hour = 23;
        assert_eq!(decode(value).unwrap().hour, 23);
    }
    #[test]
    fn malformed_bcd_dates_modes_and_missing_century_fail() {
        for invalid in [0x0a, 0xa0, 0xff] {
            assert_eq!(numeric(invalid, false), Err(Error::InvalidBcd));
        }
        let mut value = raw();
        value.century = 0;
        assert_eq!(decode(value), Err(Error::Year));
        value = raw();
        value.year = 0x23;
        assert_eq!(decode(value), Err(Error::Calendar));
        value = raw();
        value.century = 0x21;
        value.year = 0;
        assert_eq!(decode(value), Err(Error::Calendar));
        for (hour, mode) in [(0, 0), (0x13, 0), (0x92, 2), (0x24, 2)] {
            value = raw();
            value.hour = hour;
            value.status_b = mode;
            assert_eq!(decode(value), Err(Error::Calendar));
        }
        value = raw();
        value.status_d = 0;
        assert_eq!(decode(value), Err(Error::Battery));
        value = raw();
        value.status_b |= 0x80;
        assert_eq!(decode(value), Err(Error::Updating));
        for calendar in [
            Calendar {
                year: 1969,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
            },
            Calendar {
                year: 10000,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
        ] {
            assert_eq!(calendar.to_unix(), Err(Error::Year));
        }
        for (month, day, hour, minute, second) in [
            (0, 1, 0, 0, 0),
            (13, 1, 0, 0, 0),
            (4, 31, 0, 0, 0),
            (1, 0, 0, 0, 0),
            (1, 1, 24, 0, 0),
            (1, 1, 0, 60, 0),
            (1, 1, 0, 0, 60),
        ] {
            assert_eq!(
                Calendar {
                    year: 2024,
                    month,
                    day,
                    hour,
                    minute,
                    second
                }
                .to_unix(),
                Err(Error::Calendar)
            );
        }
    }
}
