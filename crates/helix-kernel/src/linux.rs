//! Original, deliberately small Linux x86-64 syscall compatibility profile.
//! Only explicit /compat/linux ELF paths select it. No Linux kernel code is used.
//! AVX/XSAVE are outside the contract: OSXSAVE/FSGSBASE remain disabled.
//! arch_prctl sets owned FS/GS bases; x87/MMX/SSE and TLS state are isolated.
use next_abi::{
    elf::{Elf64, ElfError, PF_R, PF_W, PF_X, PT_LOAD},
    native::{IMAGE_BASE, IMAGE_BYTES, IMAGE_PAGES, MAX_FILE_BYTES},
    Status,
};
pub const ENOSYS: u64 = (-38i64) as u64;
pub const EFAULT: u64 = (-14i64) as u64;
pub const EBADF: u64 = (-9i64) as u64;
pub const EINVAL: u64 = (-22i64) as u64;
pub const ENOMEM: u64 = (-12i64) as u64;
pub const EPERM: u64 = (-1i64) as u64;
pub const ENODEV: u64 = (-19i64) as u64;
pub const EOVERFLOW: u64 = (-75i64) as u64;
pub const EACCES: u64 = (-13i64) as u64;
pub const ENOENT: u64 = (-2i64) as u64;
pub const EAGAIN: u64 = (-11i64) as u64;
pub const EIO: u64 = (-5i64) as u64;
pub const EFBIG: u64 = (-27i64) as u64;
/// Linux x86-64 `struct utsname`: six 65-byte NUL-terminated fields.
pub const UTSNAME_BYTES: usize = 6 * 65;

/// Translate shared kernel status values into the Linux syscall errno ABI.
/// Keep this mapping at the personality boundary; native status numbers are
/// not Linux errno values.
pub const fn status_errno(error: Status) -> u64 {
    match error {
        Status::InvalidHandle => EBADF,
        Status::InvalidArgument => EINVAL,
        Status::AccessDenied => EACCES,
        Status::NotFound => ENOENT,
        Status::NoMemory => ENOMEM,
        Status::WouldBlock => EAGAIN,
        _ => EIO,
    }
}
/// Linux x86-64 timeval: two signed 64-bit fields (seconds, microseconds).
pub const TIMEVAL_BYTES: usize = 16;
/// Deprecated timezone: two signed 32-bit fields. This UTC profile reports zero.
pub const TIMEZONE_BYTES: usize = 8;
pub struct GettimeofdayRecords {
    pub timeval: Option<[u8; TIMEVAL_BYTES]>,
    pub timezone: Option<[u8; TIMEZONE_BYTES]>,
}
/// Pure record preparation; null timeval must not read an unavailable RTC.
/// Nanoseconds are truncated, never rounded. Callers copy timeval before
/// validating/copying timezone: a later fault preserves the earlier write.
pub fn gettimeofday_records(
    time_requested: bool,
    timezone_requested: bool,
    read_clock: impl FnOnce() -> Result<next_abi::Timespec, u64>,
) -> Result<GettimeofdayRecords, u64> {
    let timeval = if time_requested {
        let time = read_clock()?;
        if !(0..1_000_000_000).contains(&time.nanoseconds) {
            return Err(EINVAL);
        }
        let mut bytes = [0; TIMEVAL_BYTES];
        bytes[..8].copy_from_slice(&time.seconds.to_le_bytes());
        bytes[8..].copy_from_slice(&(time.nanoseconds / 1000).to_le_bytes());
        Some(bytes)
    } else {
        None
    };
    Ok(GettimeofdayRecords {
        timeval,
        timezone: timezone_requested.then_some([0; TIMEZONE_BYTES]),
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GettimeofdayOutput {
    Timeval,
    Timezone,
}
/// Raw Linux order: prepare time, copy timeval, then copy the optional timezone.
/// An invalid later destination must not undo a successful earlier copy.
pub fn gettimeofday_copyout(
    time_requested: bool,
    timezone_requested: bool,
    read_clock: impl FnOnce() -> Result<next_abi::Timespec, u64>,
    mut copyout: impl FnMut(GettimeofdayOutput, &[u8]) -> Result<(), u64>,
) -> Result<(), u64> {
    let records = gettimeofday_records(time_requested, timezone_requested, read_clock)?;
    if let Some(bytes) = records.timeval {
        copyout(GettimeofdayOutput::Timeval, &bytes)?;
    }
    if let Some(bytes) = records.timezone {
        copyout(GettimeofdayOutput::Timezone, &bytes)?;
    }
    Ok(())
}

/// Linux x86-64 clock IDs implemented by this personality. `clockid_t` is a
/// signed 32-bit argument, so classify the syscall's low 32 bits as Linux does.
pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;
pub const TIMESPEC_BYTES: usize = 16;

fn timespec_bytes(time: next_abi::Timespec) -> Result<[u8; TIMESPEC_BYTES], u64> {
    if !(0..1_000_000_000).contains(&time.nanoseconds) {
        return Err(EINVAL);
    }
    let mut bytes = [0; TIMESPEC_BYTES];
    bytes[..8].copy_from_slice(&time.seconds.to_le_bytes());
    bytes[8..].copy_from_slice(&time.nanoseconds.to_le_bytes());
    Ok(bytes)
}

/// Prepare Linux `clock_gettime` output from the selected Helix source. Only
/// realtime and monotonic clocks are currently available; other Linux clock
/// IDs are reported as unsupported with `EINVAL` rather than fabricated.
pub fn clock_gettime_record(
    clock_id: u64,
    read_realtime: impl FnOnce() -> Result<next_abi::Timespec, u64>,
    read_monotonic: impl FnOnce() -> Result<next_abi::Timespec, u64>,
) -> Result<[u8; TIMESPEC_BYTES], u64> {
    let time = match clock_id as u32 as u64 {
        CLOCK_REALTIME => read_realtime()?,
        CLOCK_MONOTONIC => read_monotonic()?,
        _ => return Err(EINVAL),
    };
    timespec_bytes(time)
}

/// Prepare the resolution reported by Linux `clock_getres`. The current PIT
/// source is periodic, so round its fractional nanosecond interval upward.
pub const fn clock_resolution_ns(source_hz: u32, divisor: u32) -> Option<u64> {
    if source_hz == 0 || divisor == 0 {
        return None;
    }
    let period = divisor as u64 * 1_000_000_000;
    Some(period.div_ceil(source_hz as u64))
}

/// Construct a Linux timespec containing the clock's resolution. `None`
/// denotes an invalid/unsupported clock ID. A zero internal resolution is
/// rejected because Linux resolutions must be positive.
pub fn clock_getres_record(clock_id: u64, resolution_ns: u64) -> Result<[u8; TIMESPEC_BYTES], u64> {
    if !matches!(clock_id as u32 as u64, CLOCK_REALTIME | CLOCK_MONOTONIC) || resolution_ns == 0 {
        return Err(EINVAL);
    }
    timespec_bytes(next_abi::Timespec {
        seconds: (resolution_ns / 1_000_000_000) as i64,
        nanoseconds: (resolution_ns % 1_000_000_000) as i64,
    })
}

/// Linux permits a null `res` for `clock_getres`; still validate the clock ID.
pub fn clock_getres_copyout(
    clock_id: u64,
    resolution_ns: u64,
    result_requested: bool,
    mut copyout: impl FnMut(&[u8]) -> Result<(), u64>,
) -> Result<(), u64> {
    let record = clock_getres_record(clock_id, resolution_ns)?;
    if result_requested {
        copyout(&record)?;
    }
    Ok(())
}

/// Perform the single-result copyout after successful source lookup, matching
/// the Linux syscall's error ordering for unsupported IDs and bad pointers.
pub fn clock_gettime_copyout(
    clock_id: u64,
    read_realtime: impl FnOnce() -> Result<next_abi::Timespec, u64>,
    read_monotonic: impl FnOnce() -> Result<next_abi::Timespec, u64>,
    mut copyout: impl FnMut(&[u8]) -> Result<(), u64>,
) -> Result<(), u64> {
    let record = clock_gettime_record(clock_id, read_realtime, read_monotonic)?;
    copyout(&record)
}

pub const ANON_BASE: u64 = IMAGE_BASE + 0x8_0000;
pub const ANON_PAGES: usize = 64;
pub const ANON_END: u64 = ANON_BASE + ANON_PAGES as u64 * 4096;
pub const ALLOCATED: u8 = 0x80;
pub fn mapping_pages(length: u64) -> Option<usize> {
    let pages = length.checked_add(4095)? / 4096;
    if pages == 0 || pages > ANON_PAGES as u64 {
        None
    } else {
        Some(pages as usize)
    }
}
pub fn first_fit(flags: &[u8; ANON_PAGES], pages: usize, hint: usize) -> Option<usize> {
    if pages == 0 || pages > ANON_PAGES {
        return None;
    }
    if hint <= ANON_PAGES - pages && flags[hint..hint + pages].iter().all(|&f| f == 0) {
        return Some(hint);
    }
    (0..=ANON_PAGES - pages).find(|&start| flags[start..start + pages].iter().all(|&f| f == 0))
}
pub fn mapping_flags(protection: u64) -> Result<u32, u64> {
    if protection & !7 != 0 {
        return Err(EINVAL);
    }
    if protection & 6 == 6 {
        return Err(EPERM);
    }
    if protection == 0 {
        return Ok(0);
    }
    Ok(PF_R
        | if protection & 2 != 0 { PF_W } else { 0 }
        | if protection & 4 != 0 { PF_X } else { 0 })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    Native,
    Linux,
    Nt,
}

/// Resolve getpid for this compatibility profile. `pid` is the process
/// record's kernel PID, never its table slot; other calls stay ENOSYS.
pub const fn identity_syscall_result(number: u64, profile: Profile, pid: u64) -> u64 {
    if number == 39 && matches!(profile, Profile::Linux) {
        pid
    } else if number == 39 {
        Status::NotSupported as i64 as u64
    } else {
        ENOSYS
    }
}

/// Stable identity reported to programs in the Linux compatibility profile.
/// The sysname identifies the selected ABI; release/version identify this
/// scoped Helix implementation rather than claiming a general Linux kernel.
/// Every field has Linux's fixed 65-byte layout, including zero padding.
pub fn utsname_bytes() -> [u8; UTSNAME_BYTES] {
    let mut bytes = [0; UTSNAME_BYTES];
    for (index, field) in [
        b"Linux".as_slice(),
        b"next",
        b"1.0-next-linux-abi",
        b"#1 Helix Linux compatibility personality",
        b"x86_64",
        b"",
    ]
    .into_iter()
    .enumerate()
    {
        let start = index * 65;
        bytes[start..start + field.len()].copy_from_slice(field);
    }
    bytes
}

// The bounded PE descriptor is kept inline: this early no-allocator loader
// deliberately spends under 512 bytes instead of allocating a box.
#[allow(clippy::large_enum_variant)]
pub enum Executable<'a> {
    Native(next_abi::native::NativeImage<'a>),
    Linux(LinuxImage<'a>),
    Nt(next_abi::pe::PeImage<'a>),
}
impl Executable<'_> {
    pub fn entry(&self) -> u64 {
        match self {
            Self::Native(i) => i.entry(),
            Self::Linux(i) => i.entry(),
            Self::Nt(i) => i.entry(),
        }
    }
    pub fn page_flags(&self, page: usize) -> u32 {
        match self {
            Self::Native(i) => i.page_flags(page),
            Self::Linux(i) => i.page_flags(page),
            Self::Nt(i) => {
                if (IMAGE_PAGES - 4..IMAGE_PAGES).contains(&page) {
                    PF_R | PF_X
                } else {
                    i.page_flags(page)
                }
            }
        }
    }
    pub fn copy_into(&self, output: &mut [u8]) -> Result<(), ElfError> {
        match self {
            Self::Native(i) => i.copy_into(output),
            Self::Linux(i) => i.copy_into(output),
            Self::Nt(i) => {
                use next_abi::pe::ImportSymbol;
                i.copy_into(output).map_err(|_| ElfError::SegmentSize)?;
                let shim_offset = IMAGE_BYTES - 16384;
                for service in 0..3 {
                    let offset = shim_offset + service * 16;
                    // MOV EAX, private_service; INT 81h; RET. The gateway saves
                    // all GPRs and the complete declared x87/SSE state.
                    output[offset..offset + 8].copy_from_slice(&[
                        0xb8,
                        (service + 1) as u8,
                        0,
                        0,
                        0,
                        0xcd,
                        0x81,
                        0xc3,
                    ]);
                }
                for import in i.imports() {
                    let offset = match import.symbol {
                        ImportSymbol::GetStdHandle => 0,
                        ImportSymbol::WriteFile => 16,
                        ImportSymbol::ExitProcess => 32,
                    };
                    let address = IMAGE_BASE + shim_offset as u64 + offset;
                    let iat = import.iat_rva as usize;
                    output[iat..iat + 8].copy_from_slice(&address.to_le_bytes());
                }
                Ok(())
            }
        }
    }
    pub fn memory_end(&self) -> u64 {
        match self {
            Self::Linux(image) => image.memory_end(),
            Self::Native(_) | Self::Nt(_) => IMAGE_BASE + IMAGE_BYTES as u64,
        }
    }
    pub fn profile(&self) -> Profile {
        match self {
            Self::Native(_) => Profile::Native,
            Self::Linux(_) => Profile::Linux,
            Self::Nt(_) => Profile::Nt,
        }
    }
}
pub struct LinuxImage<'a> {
    elf: Elf64<'a>,
}
impl<'a> LinuxImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ElfError::SegmentSize);
        }
        let elf = Elf64::parse(bytes)?;
        if elf.is_position_independent() {
            return Err(ElfError::Type);
        }
        for i in 0..elf.segment_count() {
            let segment = elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            // Reject PT_INTERP, PT_DYNAMIC and every unsupported runtime requirement.
            if segment.kind != PT_LOAD {
                return Err(ElfError::ProgramHeaders);
            }
            if segment.memory_size == 0 || segment.flags & PF_R == 0 {
                return Err(ElfError::SegmentSize);
            }
            if segment.alignment != 4096 || !segment.virtual_address.is_multiple_of(4096) {
                return Err(ElfError::Alignment);
            }
            if segment.virtual_address < IMAGE_BASE
                || segment.virtual_end()? > IMAGE_BASE + IMAGE_BYTES as u64
            {
                return Err(ElfError::Address);
            }
        }
        Ok(Self { elf })
    }
    pub fn entry(&self) -> u64 {
        self.elf.entry()
    }
    pub fn memory_end(&self) -> u64 {
        (0..self.elf.segment_count())
            .filter_map(|index| self.elf.segment(index))
            .filter_map(|segment| segment.virtual_end().ok())
            .max()
            .unwrap_or(IMAGE_BASE)
    }
    pub fn page_flags(&self, page: usize) -> u32 {
        if page >= IMAGE_PAGES {
            return 0;
        }
        let address = IMAGE_BASE + page as u64 * 4096;
        (0..self.elf.segment_count())
            .filter_map(|i| self.elf.segment(i))
            .find(|s| {
                s.is_load()
                    && address >= s.virtual_address
                    && address < s.virtual_end().unwrap_or(0)
            })
            .map_or(0, |s| s.flags & (PF_R | PF_W | PF_X))
    }
    pub fn copy_into(&self, output: &mut [u8]) -> Result<(), ElfError> {
        if output.len() != IMAGE_BYTES {
            return Err(ElfError::SegmentSize);
        }
        output.fill(0);
        for i in 0..self.elf.segment_count() {
            let segment = self.elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            let start = (segment.virtual_address - IMAGE_BASE) as usize;
            let bytes = self.elf.segment_data(i).ok_or(ElfError::SegmentBounds)?;
            output
                .get_mut(start..start + bytes.len())
                .ok_or(ElfError::SegmentBounds)?
                .copy_from_slice(bytes);
        }
        Ok(())
    }
}
/// Bounded raw-byte stdout/stderr sink, including non-UTF8 bytes.
pub fn write_stdout(bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            serial_byte(b'\r')
        }
        serial_byte(byte)
    }
}
fn serial_byte(byte: u8) {
    for _ in 0..10_000 {
        let status = unsafe { crate::input::port_in(0x3fd) };
        if status == 0xff {
            return;
        }
        if status & 0x20 != 0 {
            unsafe { crate::input::port_out(0x3f8, byte) }
            return;
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    #[test]
    fn bounds_overflow_and_write_execute_requests_fail_closed() {
        assert_eq!(mapping_pages(0), None);
        assert_eq!(mapping_pages(u64::MAX), None);
        assert_eq!(mapping_pages(ANON_PAGES as u64 * 4096 + 1), None);
        assert_eq!(mapping_pages(1), Some(1));
        assert_eq!(mapping_pages(4097), Some(2));
        assert_eq!(mapping_flags(7), Err(EPERM));
        assert_eq!(mapping_flags(8), Err(EINVAL));
        assert_eq!(mapping_flags(0), Ok(0));
        assert_eq!(mapping_flags(3), Ok(PF_R | PF_W));
        assert_eq!(mapping_flags(4), Ok(PF_R | PF_X));
    }
    #[test]
    fn occupied_and_fragmented_pages_are_never_reused() {
        let mut flags = [ALLOCATED; ANON_PAGES];
        assert_eq!(first_fit(&flags, 1, 0), None);
        flags[7..10].fill(0);
        flags[25..27].fill(0);
        assert_eq!(first_fit(&flags, 3, 25), Some(7));
        assert_eq!(first_fit(&flags, 2, 25), Some(25));
        assert_eq!(first_fit(&flags, 4, 0), None);
        assert_eq!(first_fit(&flags, 0, 0), None);
        assert_eq!(first_fit(&flags, ANON_PAGES + 1, 0), None);
    }
}

#[cfg(test)]
mod gettimeofday_tests {
    use super::*;
    #[test]
    fn exact_signed_long_layout_and_truncation() {
        for (seconds, nanoseconds, microseconds) in [
            (0, 0, 0),
            (1, 999, 0),
            (42, 123456789, 123456),
            (i64::MAX, 999999999, 999999),
            (i64::MIN, 1000, 1),
        ] {
            let result = gettimeofday_records(true, true, || {
                Ok(next_abi::Timespec {
                    seconds,
                    nanoseconds,
                })
            })
            .unwrap();
            let timeval = result.timeval.unwrap();
            assert_eq!(
                i64::from_le_bytes(timeval[..8].try_into().unwrap()),
                seconds
            );
            assert_eq!(
                i64::from_le_bytes(timeval[8..].try_into().unwrap()),
                microseconds
            );
            assert_eq!(result.timezone, Some([0; 8]));
        }
    }
    #[test]
    fn null_timeval_skips_clock_for_timezone_only_and_both_null() {
        for timezone_requested in [false, true] {
            let result = gettimeofday_records(false, timezone_requested, || {
                panic!("null timeval read clock")
            })
            .unwrap();
            assert!(result.timeval.is_none());
            assert_eq!(result.timezone, timezone_requested.then_some([0; 8]));
        }
        let result = gettimeofday_records(true, false, || {
            Ok(next_abi::Timespec {
                seconds: 1,
                nanoseconds: 0,
            })
        })
        .unwrap();
        assert!(result.timezone.is_none());
    }
    #[test]
    fn timezone_fault_keeps_written_timeval_and_timeval_fault_stops_timezone() {
        let mut timeval = [0xa5; TIMEVAL_BYTES];
        let mut called = 0;
        let result = gettimeofday_copyout(
            true,
            true,
            || {
                Ok(next_abi::Timespec {
                    seconds: 123,
                    nanoseconds: 456789,
                })
            },
            |field, bytes| {
                called += 1;
                match field {
                    GettimeofdayOutput::Timeval => {
                        timeval.copy_from_slice(bytes);
                        Ok(())
                    }
                    GettimeofdayOutput::Timezone => Err(EFAULT),
                }
            },
        );
        assert_eq!(result, Err(EFAULT));
        assert_eq!(called, 2);
        assert_eq!(i64::from_le_bytes(timeval[..8].try_into().unwrap()), 123);
        assert_eq!(i64::from_le_bytes(timeval[8..].try_into().unwrap()), 456);
        called = 0;
        assert_eq!(
            gettimeofday_copyout(
                true,
                true,
                || Ok(next_abi::Timespec {
                    seconds: 1,
                    nanoseconds: 0
                }),
                |field, _| {
                    called += 1;
                    assert_eq!(field, GettimeofdayOutput::Timeval);
                    Err(EFAULT)
                }
            ),
            Err(EFAULT)
        );
        assert_eq!(called, 1);
    }
    #[test]
    fn clock_errors_and_invalid_fraction_propagate_before_records_exist() {
        for error in [ENODEV, EOVERFLOW] {
            assert!(matches!(gettimeofday_records(true,true,||Err(error)),Err(e) if e==error));
        }
        for nanoseconds in [-1, 1_000_000_000, i64::MAX] {
            assert!(
                matches!(gettimeofday_records(true,true,||Ok(next_abi::Timespec{seconds:1,nanoseconds})),Err(e) if e==EINVAL)
            );
        }
    }
}

#[cfg(test)]
mod clock_syscall_tests {
    use super::*;

    fn timespec(bytes: &[u8]) -> (i64, i64) {
        (
            i64::from_le_bytes(bytes[..8].try_into().unwrap()),
            i64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        )
    }

    #[test]
    fn clock_gettime_selects_only_the_requested_source_and_encodes_timespec() {
        let realtime = clock_gettime_record(
            CLOCK_REALTIME,
            || {
                Ok(next_abi::Timespec {
                    seconds: -7,
                    nanoseconds: 123_456_789,
                })
            },
            || panic!("realtime lookup must not read monotonic"),
        )
        .unwrap();
        assert_eq!(timespec(&realtime), (-7, 123_456_789));

        let monotonic = clock_gettime_record(
            CLOCK_MONOTONIC,
            || panic!("monotonic lookup must not read realtime"),
            || {
                Ok(next_abi::Timespec {
                    seconds: i64::MAX,
                    nanoseconds: 999_999_999,
                })
            },
        )
        .unwrap();
        assert_eq!(timespec(&monotonic), (i64::MAX, 999_999_999));

        // The syscall's clockid_t is a 32-bit signed argument; upper register
        // bits do not change a valid low-32-bit ID.
        let monotonic = clock_gettime_record(
            0x1_0000_0001,
            || panic!("monotonic lookup must not read realtime"),
            || {
                Ok(next_abi::Timespec {
                    seconds: 3,
                    nanoseconds: 4,
                })
            },
        )
        .unwrap();
        assert_eq!(timespec(&monotonic), (3, 4));
    }

    #[test]
    fn unsupported_clock_id_precedes_source_and_copyout_errors() {
        for clock_id in [2, 4, 7, 0xffff_ffff_ffff_fffe, u64::MAX] {
            let mut copied = false;
            assert_eq!(
                clock_gettime_copyout(
                    clock_id,
                    || panic!("unsupported clock must not read realtime"),
                    || panic!("unsupported clock must not read monotonic"),
                    |_| {
                        copied = true;
                        Err(EFAULT)
                    }
                ),
                Err(EINVAL)
            );
            assert!(!copied);
        }

        assert_eq!(
            clock_gettime_copyout(
                CLOCK_MONOTONIC,
                || panic!("monotonic lookup must not read realtime"),
                || Ok(next_abi::Timespec {
                    seconds: 1,
                    nanoseconds: 2,
                }),
                |_| Err(EFAULT)
            ),
            Err(EFAULT)
        );
    }

    #[test]
    fn malformed_source_timespec_is_rejected_before_copyout() {
        for nanoseconds in [-1, 1_000_000_000] {
            let mut copied = false;
            assert_eq!(
                clock_gettime_copyout(
                    CLOCK_REALTIME,
                    || {
                        Ok(next_abi::Timespec {
                            seconds: 1,
                            nanoseconds,
                        })
                    },
                    || panic!("realtime lookup must not read monotonic"),
                    |_| {
                        copied = true;
                        Ok(())
                    }
                ),
                Err(EINVAL)
            );
            assert!(!copied);
        }
        assert_eq!(
            clock_gettime_record(
                CLOCK_REALTIME,
                || Err(ENODEV),
                || panic!("realtime lookup must not read monotonic")
            ),
            Err(ENODEV)
        );
    }

    #[test]
    fn clock_getres_reports_rounded_up_pit_resolution_and_allows_null_result() {
        let resolution = clock_resolution_ns(1_193_182, 11_931).unwrap();
        assert_eq!(resolution, 9_999_313);
        assert_eq!(clock_resolution_ns(0, 1), None);
        assert_eq!(clock_resolution_ns(1, 0), None);

        let record = clock_getres_record(CLOCK_REALTIME, resolution).unwrap();
        assert_eq!(timespec(&record), (0, 9_999_313));
        assert_eq!(clock_getres_record(CLOCK_MONOTONIC, resolution), Ok(record));
        assert_eq!(clock_getres_record(0x1_0000_0001, resolution), Ok(record));
        assert_eq!(clock_getres_record(2, resolution), Err(EINVAL));
        assert_eq!(clock_getres_record(CLOCK_REALTIME, 0), Err(EINVAL));

        let mut copy_count = 0;
        assert_eq!(
            clock_getres_copyout(CLOCK_MONOTONIC, resolution, false, |_| {
                copy_count += 1;
                Err(EFAULT)
            }),
            Ok(())
        );
        assert_eq!(copy_count, 0);
        assert_eq!(
            clock_getres_copyout(7, resolution, false, |_| {
                copy_count += 1;
                Ok(())
            }),
            Err(EINVAL)
        );
        assert_eq!(copy_count, 0);

        let mut copied = [0; TIMESPEC_BYTES];
        assert_eq!(
            clock_getres_copyout(CLOCK_MONOTONIC, resolution, true, |bytes| {
                copied.copy_from_slice(bytes);
                Ok(())
            }),
            Ok(())
        );
        assert_eq!(timespec(&copied), (0, 9_999_313));
    }
}
