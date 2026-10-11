//! Bounded polling COM1 and PS/2 keyboard/mouse input for the development desktop.
//! Port access requires ring 0 on x86-64. No USB keyboard driver is provided.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use prism::Key;

const COM1: u16 = 0x3f8;
const TRANSMIT_POLLS: usize = 2048;
const TRANSMIT_BYTES: usize = 16 * 1024;
static SERIAL_READY: AtomicBool = AtomicBool::new(false);
struct SerialTx(UnsafeCell<helix_core::uart::Tx<TRANSMIT_BYTES>>);
// Only the nonblocking guard accesses the queue. An interrupted writer never
// waits for its interrupted owner; contention rejects and counts the chunk.
unsafe impl Sync for SerialTx {}
static SERIAL_TX: SerialTx = SerialTx(UnsafeCell::new(helix_core::uart::Tx::new()));
static SERIAL_LOCK: AtomicBool = AtomicBool::new(false);
static SERIAL_CONTENTION_DROPS: AtomicU64 = AtomicU64::new(0);
static SERIAL_ACCEPTED: AtomicU64 = AtomicU64::new(0);
static SERIAL_TRANSMITTED: AtomicU64 = AtomicU64::new(0);
static SERIAL_DROPPED: AtomicU64 = AtomicU64::new(0);
static SERIAL_STALLED: AtomicU64 = AtomicU64::new(0);
static SERIAL_ABSENT: AtomicU64 = AtomicU64::new(0);
static SERIAL_PENDING: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SerialMetrics {
    pub accepted: u64,
    pub transmitted: u64,
    pub dropped: u64,
    pub stalled: u64,
    pub absent: u64,
    pub pending: u64,
}
struct SerialGuard;
impl SerialGuard {
    fn acquire() -> Option<Self> {
        SERIAL_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for SerialGuard {
    fn drop(&mut self) {
        SERIAL_LOCK.store(false, Ordering::Release);
    }
}
fn publish_serial(tx: &helix_core::uart::Tx<TRANSMIT_BYTES>) {
    let metrics = tx.metrics();
    SERIAL_ACCEPTED.store(metrics.accepted, Ordering::Relaxed);
    SERIAL_TRANSMITTED.store(metrics.transmitted, Ordering::Relaxed);
    SERIAL_DROPPED.store(metrics.dropped, Ordering::Relaxed);
    SERIAL_STALLED.store(metrics.stalled_polls, Ordering::Relaxed);
    SERIAL_ABSENT.store(metrics.absent_polls, Ordering::Relaxed);
    SERIAL_PENDING.store(tx.pending() as u64, Ordering::Relaxed);
}
fn pump_serial_locked(tx: &mut helix_core::uart::Tx<TRANSMIT_BYTES>, budget: usize) {
    tx.poll(
        budget,
        || {
            if SERIAL_READY.load(Ordering::Acquire) {
                unsafe { port_in(COM1 + 5) }
            } else {
                0xff
            }
        },
        |byte| unsafe { port_out(COM1, byte) },
    );
}
/// At most 2048 status reads per call. Pending bytes survive backend stalls.
pub fn pump_serial() {
    let Some(_guard) = SerialGuard::acquire() else {
        return;
    };
    let tx = unsafe { &mut *SERIAL_TX.0.get() };
    pump_serial_locked(tx, TRANSMIT_POLLS);
    publish_serial(tx);
}
/// A coherent queue snapshot when available; an IRQ caller that interrupts an
/// owner receives the last published snapshot, plus current contention drops.
pub fn serial_metrics() -> SerialMetrics {
    if let Some(_guard) = SerialGuard::acquire() {
        let tx = unsafe { &*SERIAL_TX.0.get() };
        let metrics = tx.metrics();
        return SerialMetrics {
            accepted: metrics.accepted,
            transmitted: metrics.transmitted,
            dropped: metrics
                .dropped
                .saturating_add(SERIAL_CONTENTION_DROPS.load(Ordering::Relaxed)),
            stalled: metrics.stalled_polls,
            absent: metrics.absent_polls,
            pending: tx.pending() as u64,
        };
    }
    SerialMetrics {
        accepted: SERIAL_ACCEPTED.load(Ordering::Relaxed),
        transmitted: SERIAL_TRANSMITTED.load(Ordering::Relaxed),
        dropped: SERIAL_DROPPED
            .load(Ordering::Relaxed)
            .saturating_add(SERIAL_CONTENTION_DROPS.load(Ordering::Relaxed)),
        stalled: SERIAL_STALLED.load(Ordering::Relaxed),
        absent: SERIAL_ABSENT.load(Ordering::Relaxed),
        pending: SERIAL_PENDING.load(Ordering::Relaxed),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseEvent {
    pub x: i32,
    pub y: i32,
    /// Standard PS/2 button bits: left=1, right=2, middle=4.
    pub buttons: u8,
    pub moved: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    Key(Key),
    SerialKey(Key),
    Mouse(MouseEvent),
    /// Development-only readback of the last completed trusted semantic frame.
    InspectAccessibility,
}

/// Standard three-byte PS/2 packets, with a bounded viewport and no allocation.
pub struct MouseDecoder {
    packet: [u8; 3],
    count: usize,
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    buttons: u8,
}
impl MouseDecoder {
    pub const fn new(width: i32, height: i32) -> Self {
        let width = if width > 0 { width } else { 1 };
        let height = if height > 0 { height } else { 1 };
        Self {
            packet: [0; 3],
            count: 0,
            width,
            height,
            x: width / 2,
            y: height / 2,
            buttons: 0,
        }
    }
    pub fn reset_packet(&mut self) {
        self.count = 0;
    }
    pub fn feed(&mut self, byte: u8) -> Option<MouseEvent> {
        if self.count == 0 && byte & 8 == 0 {
            return None;
        }
        self.packet[self.count] = byte;
        self.count += 1;
        if self.count != 3 {
            return None;
        }
        self.count = 0;
        let [flags, x, y] = self.packet;
        if flags & 0xc0 != 0 {
            return None;
        }
        let dx = i32::from(x) - if flags & 0x10 != 0 { 256 } else { 0 };
        let dy = i32::from(y) - if flags & 0x20 != 0 { 256 } else { 0 };
        let next_x = self.x.saturating_add(dx).clamp(0, self.width - 1);
        let next_y = self.y.saturating_sub(dy).clamp(0, self.height - 1);
        let buttons = flags & 7;
        let moved = next_x != self.x || next_y != self.y;
        let changed = buttons != self.buttons;
        self.x = next_x;
        self.y = next_y;
        self.buttons = buttons;
        if !moved && !changed {
            return None;
        }
        Some(MouseEvent {
            x: self.x,
            y: self.y,
            buttons,
            moved,
        })
    }
}

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

/// Best-effort formatting sink: queue each whole chunk atomically, translating
/// LF to CRLF. Bounded pre/post flushing totals 2048 status reads. Full queues or
/// reentrant writers reject the chunk and expose its byte count in metrics.
pub fn write_serial(text: &str) {
    let Some(_guard) = SerialGuard::acquire() else {
        let rejected =
            text.len()
                .saturating_add(text.bytes().filter(|&b| b == b'\n').count()) as u64;
        let _ = SERIAL_CONTENTION_DROPS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
            Some(old.saturating_add(rejected))
        });
        return;
    };
    let tx = unsafe { &mut *SERIAL_TX.0.get() };
    pump_serial_locked(tx, TRANSMIT_POLLS / 2);
    tx.enqueue_crlf(text);
    pump_serial_locked(tx, TRANSMIT_POLLS / 2);
    publish_serial(tx);
}

/// Stateful US-layout scancode-set-1 decoder, independent of hardware.
#[derive(Default)]
pub struct ScancodeDecoder {
    left_shift: bool,
    right_shift: bool,
    caps: bool,
    caps_down: bool,
    extended: bool,
    pause_bytes: u8,
    left_ctrl: bool,
    right_ctrl: bool,
    left_alt: bool,
    right_alt: bool,
    left_super: bool,
    right_super: bool,
    super_used: bool,
    window_switch_used: bool,
}

impl ScancodeDecoder {
    pub const fn new() -> Self {
        Self {
            left_shift: false,
            right_shift: false,
            caps: false,
            caps_down: false,
            extended: false,
            pause_bytes: 0,
            left_ctrl: false,
            right_ctrl: false,
            left_alt: false,
            right_alt: false,
            left_super: false,
            right_super: false,
            super_used: false,
            window_switch_used: false,
        }
    }

    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.left_shift || self.right_shift,
            ctrl: self.left_ctrl || self.right_ctrl,
            alt: self.left_alt || self.right_alt,
            super_key: self.left_super || self.right_super,
        }
    }

    fn reset_after_error(&mut self) {
        let caps = self.caps;
        *self = Self::new();
        self.caps = caps;
    }

    /// Decode one byte. Ordinary releases are silent; releasing the last Alt
    /// completes a window-switch gesture, and bare Super toggles Home.
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        if self.pause_bytes != 0 {
            self.pause_bytes -= 1;
            return None;
        }
        if byte == 0xe0 {
            self.extended = true;
            return None;
        }
        if byte == 0xe1 {
            self.extended = false;
            self.pause_bytes = 5;
            return None;
        }
        let extended = self.extended;
        self.extended = false;
        let released = byte & 0x80 != 0;
        let code = byte & 0x7f;
        if extended {
            if !released && self.modifiers().super_key && !matches!(code, 0x5b | 0x5c) {
                self.super_used = true;
            }
            match code {
                0x1d => self.right_ctrl = !released,
                0x38 => {
                    self.right_alt = !released;
                    if released && !self.modifiers().alt && self.window_switch_used {
                        self.window_switch_used = false;
                        return Some(Key::WindowSwitchEnd);
                    }
                }
                0x5b | 0x5c => {
                    let was_down = self.modifiers().super_key;
                    if code == 0x5b {
                        self.left_super = !released;
                    } else {
                        self.right_super = !released;
                    }
                    if !released && !was_down {
                        self.super_used = false;
                    }
                    if released && was_down && !self.modifiers().super_key && !self.super_used {
                        return Some(Key::Menu);
                    }
                }
                _ if released => {}
                0x47 => return Some(Key::Home),
                0x4f => return Some(Key::End),
                0x53 => return Some(Key::Delete),
                0x48 => {
                    return Some(if self.modifiers().super_key {
                        Key::SnapMaximize
                    } else {
                        Key::Up
                    })
                }
                0x50 => {
                    return Some(if self.modifiers().super_key {
                        Key::SnapRestore
                    } else {
                        Key::Down
                    })
                }
                0x4b => {
                    return Some(if self.modifiers().super_key {
                        Key::SnapLeft
                    } else {
                        Key::Left
                    })
                }
                0x4d => {
                    return Some(if self.modifiers().super_key {
                        Key::SnapRight
                    } else {
                        Key::Right
                    })
                }
                0x5d => return Some(Key::Menu),
                0x1c => return Some(Key::Enter),
                _ => {}
            }
            return None;
        }
        match code {
            0x1d => {
                self.left_ctrl = !released;
                return None;
            }
            0x38 => {
                self.left_alt = !released;
                if released && !self.modifiers().alt && self.window_switch_used {
                    self.window_switch_used = false;
                    return Some(Key::WindowSwitchEnd);
                }
                return None;
            }
            0x2a => {
                self.left_shift = !released;
                return None;
            }
            0x36 => {
                self.right_shift = !released;
                return None;
            }
            0x3a => {
                if !released && !self.caps_down {
                    self.caps = !self.caps;
                }
                self.caps_down = !released;
                return None;
            }
            _ => {}
        }
        if released {
            return None;
        }
        if self.modifiers().super_key {
            self.super_used = true;
            return match code {
                0x39 => Some(Key::OpenSearch),
                0x17 => Some(Key::OpenSettings),
                0x12 => Some(Key::OpenFiles),
                0x1c => Some(Key::OpenTerminal),
                _ => None,
            };
        }
        if code == 0x3e && self.modifiers().alt {
            return Some(Key::CloseWindow);
        }
        match code {
            0x01 => return Some(Key::Escape),
            0x44 => return Some(Key::Menu),
            0x42 => return Some(Key::Notifications),
            0x0e => return Some(Key::Backspace),
            0x0f => {
                if self.modifiers().alt {
                    self.window_switch_used = true;
                }
                return Some(if self.left_alt || self.right_alt {
                    if self.left_shift || self.right_shift {
                        Key::WindowPrevious
                    } else {
                        Key::WindowNext
                    }
                } else if self.left_shift || self.right_shift {
                    Key::BackTab
                } else {
                    Key::Tab
                });
            }
            0x1c => return Some(Key::Enter),
            _ => {}
        }
        let (normal, shifted) = match code {
            0x02 => ('1', '!'),
            0x03 => ('2', '@'),
            0x04 => ('3', '#'),
            0x05 => ('4', '$'),
            0x06 => ('5', '%'),
            0x07 => ('6', '^'),
            0x08 => ('7', '&'),
            0x09 => ('8', '*'),
            0x0a => ('9', '('),
            0x0b => ('0', ')'),
            0x0c => ('-', '_'),
            0x0d => ('=', '+'),
            0x10 => ('q', 'Q'),
            0x11 => ('w', 'W'),
            0x12 => ('e', 'E'),
            0x13 => ('r', 'R'),
            0x14 => ('t', 'T'),
            0x15 => ('y', 'Y'),
            0x16 => ('u', 'U'),
            0x17 => ('i', 'I'),
            0x18 => ('o', 'O'),
            0x19 => ('p', 'P'),
            0x1a => ('[', '{'),
            0x1b => (']', '}'),
            0x1e => ('a', 'A'),
            0x1f => ('s', 'S'),
            0x20 => ('d', 'D'),
            0x21 => ('f', 'F'),
            0x22 => ('g', 'G'),
            0x23 => ('h', 'H'),
            0x24 => ('j', 'J'),
            0x25 => ('k', 'K'),
            0x26 => ('l', 'L'),
            0x27 => (';', ':'),
            0x28 => ('\'', '"'),
            0x29 => ('`', '~'),
            0x2b => ('\\', '|'),
            0x2c => ('z', 'Z'),
            0x2d => ('x', 'X'),
            0x2e => ('c', 'C'),
            0x2f => ('v', 'V'),
            0x30 => ('b', 'B'),
            0x31 => ('n', 'N'),
            0x32 => ('m', 'M'),
            0x33 => (',', '<'),
            0x34 => ('.', '>'),
            0x35 => ('/', '?'),
            0x39 => (' ', ' '),
            _ => return None,
        };
        let shift = self.left_shift || self.right_shift;
        let uppercase = if normal.is_ascii_lowercase() {
            shift ^ self.caps
        } else {
            shift
        };
        Some(Key::Character(if uppercase { shifted } else { normal }))
    }
}

struct InputState {
    decoder: ScancodeDecoder,
    last_cr: bool,
    mouse: MouseDecoder,
    mouse_ready: bool,
    serial_first: bool,
    diagnostic_down: bool,
}
impl InputState {
    fn decode_keyboard(&mut self, byte: u8) -> Option<InputEvent> {
        if !self.decoder.extended && self.decoder.pause_bytes == 0 && byte & 0x7f == 0x43 {
            if byte & 0x80 != 0 {
                self.diagnostic_down = false;
                return None;
            }
            let first = !self.diagnostic_down;
            self.diagnostic_down = true;
            let modifiers = self.decoder.modifiers();
            return (first
                && !modifiers.shift
                && !modifiers.ctrl
                && !modifiers.alt
                && !modifiers.super_key)
                .then_some(InputEvent::InspectAccessibility);
        }
        self.decoder.feed(byte).map(InputEvent::Key)
    }
    /// Alternation bounds service delay under continuous serial traffic. Each
    /// attempted device consumes at most 32 bytes; decoder prefixes persist.
    fn poll_with(
        &mut self,
        serial_ready: bool,
        mut read: impl FnMut(u16) -> u8,
    ) -> Option<InputEvent> {
        let serial_first = self.serial_first;
        self.serial_first = !serial_first;
        for serial in [serial_first, !serial_first] {
            let mut event = None;
            if serial && serial_ready {
                for _ in 0..32 {
                    let status = read(COM1 + 5);
                    if status == 0xff || status & 1 == 0 {
                        break;
                    }
                    let byte = read(COM1);
                    if status & 0x1e != 0 {
                        continue;
                    } // Discard framing/parity/error bytes.
                    event = ascii_key(byte, &mut self.last_cr).map(InputEvent::SerialKey);
                    if event.is_some() {
                        break;
                    }
                }
            } else if !serial {
                for _ in 0..32 {
                    let status = read(0x64);
                    if status == 0xff || status & 1 == 0 {
                        break;
                    }
                    let byte = read(0x60);
                    if status & 0xc0 != 0 {
                        if status & 0x20 != 0 {
                            self.mouse.reset_packet();
                        } else {
                            self.decoder.reset_after_error();
                            self.diagnostic_down = false;
                        }
                        continue;
                    } // Parity and timeout errors. Auxiliary bytes never enter keyboard decoder.
                    event = if status & 0x20 != 0 {
                        if self.mouse_ready {
                            self.mouse.feed(byte).map(InputEvent::Mouse)
                        } else {
                            None
                        }
                    } else {
                        self.decode_keyboard(byte)
                    };
                    if event.is_some() {
                        break;
                    }
                }
            }
            if event.is_some() {
                return event;
            }
        }
        None
    }
}

struct SharedInput {
    busy: AtomicBool,
    state: UnsafeCell<InputState>,
}
// Every access to state is protected by busy, including interrupt/reentrant calls.
unsafe impl Sync for SharedInput {}
static INPUT: SharedInput = SharedInput {
    busy: AtomicBool::new(false),
    state: UnsafeCell::new(InputState {
        decoder: ScancodeDecoder::new(),
        last_cr: false,
        mouse: MouseDecoder::new(1, 1),
        mouse_ready: false,
        serial_first: true,
        diagnostic_down: false,
    }),
};

const CONTROLLER_POLLS: usize = 10_000;
fn controller_write(port: u16, byte: u8) -> bool {
    for _ in 0..CONTROLLER_POLLS {
        let status = unsafe { port_in(0x64) };
        if status == 0xff {
            return false;
        }
        if status & 2 == 0 {
            unsafe {
                port_out(port, byte);
            }
            return true;
        }
        core::hint::spin_loop();
    }
    false
}
fn controller_read(auxiliary: bool) -> Option<u8> {
    for _ in 0..CONTROLLER_POLLS {
        let status = unsafe { port_in(0x64) };
        if status == 0xff {
            return None;
        }
        if status & 1 != 0 {
            let byte = unsafe { port_in(0x60) };
            if status & 0xc0 == 0 && (status & 0x20 != 0) == auxiliary {
                return Some(byte);
            }
        }
        core::hint::spin_loop();
    }
    None
}
fn mouse_command(command: u8) -> bool {
    // RESEND is bounded to three attempts, including the first request.
    for _ in 0..3 {
        if !controller_write(0x64, 0xd4) || !controller_write(0x60, command) {
            return false;
        }
        match controller_read(true) {
            Some(0xfa) => return true,
            Some(0xfe) => {}
            _ => return false,
        }
    }
    false
}
/// Enable the standard PS/2 auxiliary device with IRQ12 masked. Every hardware
/// wait is bounded; an absent or unresponsive device leaves keyboard input live.
/// Boot calls this before accepting input; controller command replies may drain
/// preexisting bytes during setup.
pub fn init_mouse(width: u32, height: u32) -> bool {
    if INPUT
        .busy
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }
    let state = unsafe { &mut *INPUT.state.get() };
    state.mouse_ready = false;
    state.mouse = MouseDecoder::new(
        width.min(i32::MAX as u32) as i32,
        height.min(i32::MAX as u32) as i32,
    );
    for _ in 0..32 {
        let status = unsafe { port_in(0x64) };
        if status == 0xff || status & 1 == 0 {
            break;
        }
        unsafe {
            port_in(0x60);
        }
    }
    let ready = (|| {
        if !controller_write(0x64, 0xa8) || !controller_write(0x64, 0x20) {
            return false;
        }
        let Some(config) = controller_read(false) else {
            return false;
        };
        if !controller_write(0x64, 0x60) || !controller_write(0x60, config & !0x22) {
            return false;
        }
        // Reset selects the standard three-byte protocol even if firmware
        // previously negotiated IntelliMouse wheel/five-button packets.
        mouse_command(0xff)
            && controller_read(true) == Some(0xaa)
            && controller_read(true) == Some(0)
            && mouse_command(0xf5)
            && mouse_command(0xf6)
            && mouse_command(0xf4)
    })();
    state.mouse_ready = ready;
    INPUT.busy.store(false, Ordering::Release);
    ready
}
pub fn modifiers() -> Modifiers {
    if INPUT
        .busy
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Modifiers::default();
    }
    let result = unsafe { (&*INPUT.state.get()).decoder.modifiers() };
    INPUT.busy.store(false, Ordering::Release);
    result
}

fn ascii_key(byte: u8, last_cr: &mut bool) -> Option<Key> {
    let previous_cr = *last_cr;
    *last_cr = byte == b'\r';
    match byte {
        b'\r' => Some(Key::Enter),
        b'\n' if previous_cr => None,
        b'\n' => Some(Key::Enter),
        8 | 127 => Some(Key::Backspace),
        b'\t' => Some(Key::Tab),
        27 => Some(Key::Escape),
        32..=126 => Some(Key::Character(byte as char)),
        _ => None,
    }
}

/// Poll at most 32 hardware bytes per device. Works with interrupts disabled.
/// Called only in the privileged bootstrap; contention returns immediately.
pub fn poll_event() -> Option<InputEvent> {
    if INPUT
        .busy
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return None;
    }
    let state = unsafe { &mut *INPUT.state.get() };
    let event = state.poll_with(SERIAL_READY.load(Ordering::Acquire), |port| unsafe {
        port_in(port)
    });
    INPUT.busy.store(false, Ordering::Release);
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_input_state() -> InputState {
        InputState {
            decoder: ScancodeDecoder::new(),
            last_cr: false,
            mouse: MouseDecoder::new(1280, 800),
            mouse_ready: true,
            serial_first: true,
            diagnostic_down: false,
        }
    }
    #[test]
    fn diagnostic_f9_is_plain_physical_make_only_and_preserves_decoder_state() {
        let mut state = test_input_state();
        assert_eq!(
            state.decode_keyboard(0x43),
            Some(InputEvent::InspectAccessibility)
        );
        assert_eq!(state.decode_keyboard(0x43), None);
        assert_eq!(state.decode_keyboard(0xc3), None);
        state.decode_keyboard(0x1d); // Ctrl chord cannot become a diagnostic.
        assert_eq!(state.decode_keyboard(0x43), None);
        state.decode_keyboard(0x9d);
        assert_eq!(state.decode_keyboard(0x43), None); // Held repeat after Ctrl release.
        state.decode_keyboard(0xc3);
        state.decode_keyboard(0xe0);
        assert_eq!(state.decode_keyboard(0x43), None);
        assert_eq!(
            state.decode_keyboard(0x43),
            Some(InputEvent::InspectAccessibility)
        );
        state.decode_keyboard(0xc3);
        state.decode_keyboard(0xe1);
        assert_eq!(state.decode_keyboard(0x43), None); // Pause sequence payload.
        for _ in 0..4 {
            state.decode_keyboard(0);
        }
        assert_eq!(
            state.decode_keyboard(0x1e),
            Some(InputEvent::Key(Key::Character('a')))
        );
    }

    #[test]
    fn sustained_serial_and_keyboard_both_receive_bounded_service() {
        let mut state = test_input_state();
        let mut serial_reads = 0;
        let mut keyboard_reads = 0;
        for turn in 0..64 {
            let event = state.poll_with(true, |port| match port {
                p if p == COM1 + 5 => 1,
                COM1 => {
                    serial_reads += 1;
                    b's'
                }
                0x64 => 1,
                0x60 => {
                    keyboard_reads += 1;
                    0x1e
                }
                _ => panic!("unexpected input port"),
            });
            assert_eq!(
                event,
                Some(if turn % 2 == 0 {
                    InputEvent::SerialKey(Key::Character('s'))
                } else {
                    InputEvent::Key(Key::Character('a'))
                })
            );
        }
        assert_eq!((serial_reads, keyboard_reads), (32, 32));
    }
    #[test]
    fn noisy_serial_has_bounded_budget_and_does_not_starve_a_mouse_packet() {
        let mut state = test_input_state();
        let mut reads = 0;
        let mut packet = [0x0a, 0, 0].into_iter(); // Actual right button, no invented left.
        let event = state.poll_with(true, |port| match port {
            p if p == COM1 + 5 => 1,
            COM1 => {
                reads += 1;
                0
            }
            0x64 => 0x21,
            0x60 => packet.next().expect("bounded complete packet"),
            _ => panic!("unexpected input port"),
        });
        assert_eq!(reads, 32);
        assert!(matches!(
            event,
            Some(InputEvent::Mouse(MouseEvent { buttons: 2, .. }))
        ));
    }

    #[test]
    fn uart_reentrant_guard_never_waits_and_counts_rejected_chunk() {
        let _guard = SerialGuard::acquire().expect("uncontended test queue");
        let before = SERIAL_CONTENTION_DROPS.load(Ordering::Relaxed);
        write_serial("pid=17\n");
        assert_eq!(SERIAL_CONTENTION_DROPS.load(Ordering::Relaxed), before + 8);
        assert!(SerialGuard::acquire().is_none());
    }

    #[test]
    fn shifts_are_independent_and_release_events_are_ignored() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(d.feed(0x1e), Some(Key::Character('a')));
        assert_eq!(d.feed(0x9e), None);
        d.feed(0x2a);
        d.feed(0x36);
        d.feed(0xaa);
        assert_eq!(d.feed(0x1e), Some(Key::Character('A')));
        d.feed(0xb6);
        assert_eq!(d.feed(0x1e), Some(Key::Character('a')));
    }

    #[test]
    fn caps_changes_letters_only_and_does_not_toggle_on_repeat() {
        let mut d = ScancodeDecoder::new();
        d.feed(0x3a);
        d.feed(0x3a);
        assert_eq!(d.feed(0x1e), Some(Key::Character('A')));
        assert_eq!(d.feed(0x02), Some(Key::Character('1')));
        d.feed(0x2a);
        assert_eq!(d.feed(0x1e), Some(Key::Character('a')));
        assert_eq!(d.feed(0x02), Some(Key::Character('!')));
        d.feed(0xaa);
        d.feed(0xba);
        d.feed(0x3a);
        assert_eq!(d.feed(0x1e), Some(Key::Character('a')));
    }

    #[test]
    fn extended_sequences_do_not_invent_shift_or_ascii_events() {
        let mut d = ScancodeDecoder::new();
        for b in [0xe0, 0x2a, 0xe0, 0x37, 0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5] {
            assert_eq!(d.feed(b), None);
        }
        assert_eq!(d.feed(0x1e), Some(Key::Character('a')));
    }

    #[test]
    fn control_keys_and_serial_crlf_are_decoded() {
        let mut d = ScancodeDecoder::new();
        for (code, expected) in [
            (1, Key::Escape),
            (14, Key::Backspace),
            (15, Key::Tab),
            (28, Key::Enter),
        ] {
            assert_eq!(d.feed(code), Some(expected));
        }
        let mut cr = false;
        assert_eq!(ascii_key(b'\r', &mut cr), Some(Key::Enter));
        assert_eq!(ascii_key(b'\n', &mut cr), None);
        assert_eq!(ascii_key(b'\n', &mut cr), Some(Key::Enter));
        assert_eq!(ascii_key(127, &mut cr), Some(Key::Backspace));
        assert_eq!(ascii_key(0xff, &mut cr), None);
    }

    #[test]
    fn extended_navigation_and_modifiers_do_not_leak_fake_shift() {
        let mut d = ScancodeDecoder::new();
        for (code, key) in [
            (0x47, Key::Home),
            (0x4f, Key::End),
            (0x53, Key::Delete),
            (0x48, Key::Up),
            (0x50, Key::Down),
            (0x4b, Key::Left),
            (0x4d, Key::Right),
            (0x5d, Key::Menu),
            (0x1c, Key::Enter),
        ] {
            assert_eq!(d.feed(0xe0), None);
            assert_eq!(d.feed(code), Some(key));
            d.feed(0xe0);
            assert_eq!(d.feed(code | 0x80), None);
        }
        assert_eq!(d.feed(0x44), Some(Key::Menu));
        d.feed(0x1d);
        d.feed(0xe0);
        d.feed(0x1d);
        d.feed(0x9d);
        assert!(d.modifiers().ctrl);
        d.feed(0xe0);
        d.feed(0x9d);
        assert!(!d.modifiers().ctrl);
        d.feed(0x38);
        d.feed(0xe0);
        d.feed(0x38);
        d.feed(0xb8);
        assert!(d.modifiers().alt);
        d.feed(0xe0);
        d.feed(0xb8);
        assert!(!d.modifiers().alt);
        d.feed(0xe0);
        d.feed(0x2a);
        assert!(!d.modifiers().shift);
    }
    fn mouse_packet(d: &mut MouseDecoder, packet: [u8; 3]) -> Option<MouseEvent> {
        assert_eq!(d.feed(packet[0]), None);
        assert_eq!(d.feed(packet[1]), None);
        d.feed(packet[2])
    }
    #[test]
    fn mouse_sync_signed_motion_buttons_and_viewport_bounds() {
        let mut d = MouseDecoder::new(100, 80);
        assert_eq!(d.feed(0), None);
        assert_eq!(
            mouse_packet(&mut d, [8, 10, 5]),
            Some(MouseEvent {
                x: 60,
                y: 35,
                buttons: 0,
                moved: true
            })
        );
        assert_eq!(
            mouse_packet(&mut d, [0x39, 0xf6, 0xfb]),
            Some(MouseEvent {
                x: 50,
                y: 40,
                buttons: 1,
                moved: true
            })
        );
        assert_eq!(mouse_packet(&mut d, [9, 0, 0]), None);
        assert_eq!(
            mouse_packet(&mut d, [8, 0, 0]),
            Some(MouseEvent {
                x: 50,
                y: 40,
                buttons: 0,
                moved: false
            })
        );
        assert_eq!(
            mouse_packet(&mut d, [8, 255, 255]),
            Some(MouseEvent {
                x: 99,
                y: 0,
                buttons: 0,
                moved: true
            })
        );
        assert_eq!(
            mouse_packet(&mut d, [0x38, 0, 0]),
            Some(MouseEvent {
                x: 0,
                y: 79,
                buttons: 0,
                moved: true
            })
        );
    }
    #[test]
    fn mouse_overflow_and_error_reset_do_not_invent_buttons() {
        let mut d = MouseDecoder::new(10, 10);
        assert_eq!(mouse_packet(&mut d, [0xc9, 255, 255]), None);
        assert_eq!(d.feed(9), None);
        d.reset_packet();
        assert_eq!(
            mouse_packet(&mut d, [8, 1, 0]),
            Some(MouseEvent {
                x: 6,
                y: 5,
                buttons: 0,
                moved: true
            })
        );
        let mut tiny = MouseDecoder::new(0, 0);
        assert_eq!(mouse_packet(&mut tiny, [8, 255, 255]), None);
        assert_eq!(
            mouse_packet(&mut tiny, [0xf, 0, 0]),
            Some(MouseEvent {
                x: 0,
                y: 0,
                buttons: 7,
                moved: false
            })
        );
    }
    #[test]
    fn super_arrows_emit_snap_keys_and_independent_releases_restore_navigation() {
        let mut d = ScancodeDecoder::new();
        d.feed(0xe0);
        d.feed(0x5b);
        for (code, key) in [
            (0x4b, Key::SnapLeft),
            (0x4d, Key::SnapRight),
            (0x48, Key::SnapMaximize),
            (0x50, Key::SnapRestore),
        ] {
            d.feed(0xe0);
            assert_eq!(d.feed(code), Some(key));
            d.feed(0xe0);
            assert_eq!(d.feed(code | 0x80), None);
        }
        d.feed(0xe0);
        d.feed(0x5c);
        d.feed(0xe0);
        d.feed(0xdb);
        assert!(d.modifiers().super_key);
        d.feed(0xe0);
        d.feed(0xdc);
        assert!(!d.modifiers().super_key);
        d.feed(0xe0);
        assert_eq!(d.feed(0x4b), Some(Key::Left));
    }
    #[test]
    fn tab_modifiers_emit_focus_and_window_navigation_without_text() {
        let mut decoder = ScancodeDecoder::new();
        assert_eq!(decoder.feed(0x0f), Some(Key::Tab));
        assert_eq!(decoder.feed(0x2a), None);
        assert_eq!(decoder.feed(0x0f), Some(Key::BackTab));
        assert_eq!(decoder.feed(0x38), None);
        assert_eq!(decoder.feed(0x0f), Some(Key::WindowPrevious));
        assert_eq!(decoder.feed(0xaa), None);
        assert_eq!(decoder.feed(0x0f), Some(Key::WindowNext));
        assert_eq!(decoder.feed(0xb8), Some(Key::WindowSwitchEnd));
        assert_eq!(decoder.feed(0x0f), Some(Key::Tab));
    }
    #[test]
    fn switch_finishes_only_on_last_alt_release_and_never_on_plain_alt() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(d.feed(0x38), None);
        assert_eq!(d.feed(0xb8), None);
        d.feed(0x38);
        d.feed(0xe0);
        d.feed(0x38);
        assert_eq!(d.feed(0x0f), Some(Key::WindowNext));
        assert_eq!(d.feed(0xb8), None);
        assert!(d.modifiers().alt);
        d.feed(0xe0);
        assert_eq!(d.feed(0xb8), Some(Key::WindowSwitchEnd));
        d.feed(0xe0);
        assert_eq!(d.feed(0xb8), None);
        assert!(!d.modifiers().alt);
    }
    #[test]
    fn super_release_toggles_home_once_and_chords_do_not_toggle_or_type() {
        let mut d = ScancodeDecoder::new();
        d.feed(0xe0);
        assert_eq!(d.feed(0x5b), None);
        d.feed(0xe0);
        assert_eq!(d.feed(0x5b), None); // repeat
        d.feed(0xe0);
        assert_eq!(d.feed(0xdb), Some(Key::Menu));
        d.feed(0xe0);
        assert_eq!(d.feed(0xdb), None);
        for (scan, key) in [
            (0x39, Key::OpenSearch),
            (0x17, Key::OpenSettings),
            (0x12, Key::OpenFiles),
            (0x1c, Key::OpenTerminal),
        ] {
            d.feed(0xe0);
            d.feed(0x5c);
            assert_eq!(d.feed(scan), Some(key));
            assert_eq!(d.feed(scan | 0x80), None);
            d.feed(0xe0);
            assert_eq!(d.feed(0xdc), None);
        }
        d.feed(0xe0);
        d.feed(0x5b);
        assert_eq!(d.feed(0x1e), None); // unknown Super+A is not text
        d.feed(0xe0);
        assert_eq!(d.feed(0xdb), None);
        assert_eq!(d.feed(0x1e), Some(Key::Character('a')));
        d.feed(0x38);
        assert_eq!(d.feed(0x3e), Some(Key::CloseWindow));
        assert_eq!(d.feed(0xbe), None);
        d.feed(0xb8);
        assert_eq!(d.feed(0x3e), None);
    }
}
