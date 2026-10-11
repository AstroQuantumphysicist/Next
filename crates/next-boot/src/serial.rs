//! Polling COM1 and legacy PS/2 keyboard support for the bootstrap console.
//! Port access requires ring 0 on x86-64. No USB keyboard driver is provided.
use core::sync::atomic::{AtomicBool, Ordering};

const COM1: u16 = 0x3f8;
const TRANSMIT_POLLS: usize = 10_000;
static SERIAL_READY: AtomicBool = AtomicBool::new(false);

/// Read one byte from an x86 I/O port.
///
/// # Safety
/// The caller must run with permission to access the requested hardware port.
pub unsafe fn port_in(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value,
        options(nomem, nostack, preserves_flags));
    value
}

/// Write one byte to an x86 I/O port.
///
/// # Safety
/// The caller must own the device and have permission to access its ports.
pub unsafe fn port_out(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value,
        options(nomem, nostack, preserves_flags));
}

/// Configure COM1 for 115200 baud, eight data bits, no parity, one stop bit.
/// Called by the privileged bootstrap on its single boot CPU.
pub fn init_serial() {
    unsafe {
        // An absent ISA UART normally reads as all ones. Avoid polling it.
        if port_in(COM1 + 5) == 0xff {
            return;
        }
        port_out(COM1 + 1, 0); // UART interrupts off: polling is independent of IF.
        port_out(COM1 + 3, 0x80);
        port_out(COM1, 1);
        port_out(COM1 + 1, 0);
        port_out(COM1 + 3, 3);
        port_out(COM1 + 2, 0xc7);
        port_out(COM1 + 4, 3);
    }
    SERIAL_READY.store(true, Ordering::Release);
}

fn serial_byte(byte: u8) {
    for _ in 0..TRANSMIT_POLLS {
        let status = unsafe { port_in(COM1 + 5) };
        if status == 0xff {
            return;
        }
        if status & 0x20 != 0 {
            unsafe {
                port_out(COM1, byte);
            }
            return;
        }
        core::hint::spin_loop();
    }
    // Drop a byte if the UART stalls rather than hanging the kernel.
}

/// Write to COM1 with a bounded wait per byte; convert LF to CRLF.
pub fn write_serial(text: &str) {
    if !SERIAL_READY.load(Ordering::Acquire) {
        return;
    }
    for byte in text.bytes() {
        if byte == b'\n' {
            serial_byte(b'\r');
        }
        serial_byte(byte);
    }
}
