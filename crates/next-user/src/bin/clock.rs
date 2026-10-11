#![no_std]
#![no_main]
use core::fmt::{self, Write};
use next_user as rt;
rt::entry!(clock);
struct Text {
    bytes: [u8; 256],
    len: usize,
}
impl Write for Text {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.len() > self.bytes.len() - self.len {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
fn clock(info: &rt::StartInfo) -> ! {
    // Clock copyout must reject unmapped, read-only and cross-guard ranges.
    static READ_ONLY: [u8; 16] = *b"CLOCK-READ-ONLY!";
    for pointer in [
        0,
        READ_ONLY.as_ptr() as u64,
        next_abi::native::STACK_TOP - 8,
    ] {
        if rt::call(rt::Syscall::ClockRealtime, [pointer, 0, 0, 0, 0, 0])
            != rt::Status::InvalidArgument as i64
        {
            rt::exit(3);
        }
    }
    rt::log(info.log, "CLOCK: native realtime copyout checks passed\n");
    let nanoseconds = rt::clock();
    if nanoseconds < 0 {
        rt::log(info.log, "Clock unavailable\n");
        rt::exit(1);
    }
    let mut text = Text {
        bytes: [0; 256],
        len: 0,
    };
    let _ = write!(
        text,
        "Next Clock / native ABI {}.{} / PID {}\nMonotonic uptime: {}.{:09} seconds\n",
        info.abi_major,
        info.abi_minor,
        info.pid,
        nanoseconds / 1_000_000_000,
        nanoseconds % 1_000_000_000
    );
    let mut utc = rt::Timespec::default();
    let status = rt::clock_realtime(&mut utc);
    if status == 0 && utc.nanoseconds >= 0 && utc.nanoseconds < 1_000_000_000 {
        let _ = writeln!(
            text,
            "UTC realtime: {}.{:09} Unix seconds",
            utc.seconds, utc.nanoseconds
        );
    } else {
        let _ = writeln!(text, "UTC realtime unavailable (native status {})", status);
    }
    let message =
        core::str::from_utf8(&text.bytes[..text.len]).unwrap_or("Clock formatting failed\n");
    if rt::log(info.log, message) < 0 {
        rt::exit(2);
    }
    rt::exit(0)
}
