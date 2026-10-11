#![no_std]
//! Freestanding native development runtime. Calls the Next interrupt ABI;
//! this is not a Linux, POSIX, or C runtime compatibility layer.
pub mod sdk;
use core::arch::asm;
pub use next_abi::{native::StartInfo, Status, Syscall, Timespec};

#[used]
#[link_section = ".note.next"]
static ABI_NOTE: [u8; 36] = next_abi::native::NOTE;

/// The interrupt preserves all registers except rax (the result), rcx, and r11.
/// A compiler memory barrier is required because the kernel reads and writes
/// caller buffers; intentionally do not mark this assembly `nomem`.
#[inline]
pub fn call(number: Syscall, args: [u64; 6]) -> i64 {
    let mut result = number as u64;
    unsafe {
        asm!("int 0x80", inlateout("rax") result,
            in("rdi") args[0], in("rsi") args[1], in("rdx") args[2],
            in("r10") args[3], in("r8") args[4], in("r9") args[5],
            lateout("rcx") _, lateout("r11") _);
    }
    result as i64
}
pub fn log(handle: u64, text: &str) -> i64 {
    call(
        Syscall::LogWrite,
        [handle, text.as_ptr() as u64, text.len() as u64, 0, 0, 0],
    )
}
pub fn exit(code: i64) -> ! {
    call(Syscall::Exit, [code as u64, 0, 0, 0, 0, 0]);
    unsafe {
        asm!("ud2", options(noreturn));
    }
}
pub fn yield_now() {
    call(Syscall::ThreadYield, [0; 6]);
}
pub fn clock() -> i64 {
    call(Syscall::ClockRead, [0; 6])
}
/// UTC realtime; returns a status and writes the typed result on success only.
pub fn clock_realtime(output: &mut Timespec) -> i64 {
    call(
        Syscall::ClockRealtime,
        [output as *mut Timespec as u64, 0, 0, 0, 0, 0],
    )
}
pub fn sleep_until(deadline: u64) {
    call(Syscall::ThreadSleep, [deadline, 0, 0, 0, 0, 0]);
}
pub fn spawn(launcher: u64, path: &str) -> i64 {
    call(
        Syscall::ProcessSpawn,
        [launcher, path.as_ptr() as u64, path.len() as u64, 0, 0, 0],
    )
}
/// Poll a process result without conflating a signed exit code with syscall
/// status. A successful wait writes the retained result; it does not close
/// the process handle or consume the exit result.
pub fn wait_result(handle: u64, exit_code: &mut i64) -> i64 {
    call(
        Syscall::ObjectWait,
        [handle, exit_code as *mut i64 as u64, 0, 0, 0, 0],
    )
}

pub fn kill(handle: u64, code: i64) -> i64 {
    call(Syscall::ProcessKill, [handle, code as u64, 0, 0, 0, 0])
}
pub fn close(handle: u64) -> i64 {
    call(Syscall::HandleClose, [handle, 0, 0, 0, 0, 0])
}

/// Privileged namespace open. Ordinary applications receive scoped file
/// capabilities through IPC instead of a launcher capability.
pub fn file_open(launcher: u64, path: &str, flags: u64, rights: next_abi::Rights) -> i64 {
    call(
        Syscall::FileOpen,
        [
            launcher,
            path.as_ptr() as u64,
            path.len() as u64,
            flags,
            rights.0,
            0,
        ],
    )
}
pub fn file_read(handle: u64, out: &mut [u8]) -> i64 {
    call(
        Syscall::FileRead,
        [handle, out.as_mut_ptr() as u64, out.len() as u64, 0, 0, 0],
    )
}
pub fn file_write(handle: u64, bytes: &[u8]) -> i64 {
    call(
        Syscall::FileWrite,
        [handle, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0],
    )
}
pub fn file_seek(handle: u64, offset: i64, origin: u64) -> i64 {
    call(Syscall::FileSeek, [handle, offset as u64, origin, 0, 0, 0])
}
pub fn file_stat(handle: u64, info: &mut [u64; 2]) -> i64 {
    call(
        Syscall::FileStat,
        [handle, info.as_mut_ptr() as u64, 0, 0, 0, 0],
    )
}
pub fn ready(launcher: u64) -> i64 {
    call(Syscall::SystemReady, [launcher, 0, 0, 0, 0, 0])
}

#[macro_export]
macro_rules! entry {
    ($main:path) => {
        core::arch::global_asm!(
            ".section .text.entry,\"ax\"",
            ".global _start",
            "_start:",
            "xor rbp, rbp",
            "call next_user_entry",
            "ud2"
        );
        #[no_mangle]
        unsafe extern "C" fn next_user_entry(start: *const $crate::StartInfo) -> ! {
            let info = unsafe { &*start };
            if !info.validate() {
                $crate::exit(-1);
            }
            $main(info)
        }
        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
            $crate::exit(-11)
        }
    };
}

pub fn spawn_with_endpoint(launcher: u64, path: &str, endpoint: u64) -> i64 {
    call(
        Syscall::ProcessSpawn,
        [
            launcher,
            path.as_ptr() as u64,
            path.len() as u64,
            endpoint,
            0,
            0,
        ],
    )
}
pub fn channel_pair(pair: &mut [u64; 2]) -> i64 {
    call(
        Syscall::ChannelCreate,
        [pair.as_mut_ptr() as u64, 0, 0, 0, 0, 0],
    )
}
pub fn send(endpoint: u64, bytes: &[u8]) -> i64 {
    call(
        Syscall::ChannelSend,
        [endpoint, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0],
    )
}
/// Atomically move attached capabilities with independently attenuated rights.
/// The header's handle count must match the rights array length.
pub fn send_with_rights(endpoint: u64, bytes: &[u8], rights: &[u64]) -> i64 {
    call(
        Syscall::ChannelSend,
        [
            endpoint,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            rights.as_ptr() as u64,
            0,
            0,
        ],
    )
}
pub fn ipc_frame_with_handle(reply: bool, handle: u64) -> [u8; 52] {
    let mut bytes = [0; 52];
    bytes[..44].copy_from_slice(&ipc_frame(reply));
    bytes[28] = 1;
    bytes[44..52].copy_from_slice(&handle.to_le_bytes());
    bytes
}
pub fn receive(endpoint: u64, bytes: &mut [u8]) -> i64 {
    call(
        Syscall::ChannelReceive,
        [
            endpoint,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
            0,
        ],
    )
}
/// One explicit little-endian protocol frame: opcode1, correlation1,
/// no deadline, no embedded handles, and a four-byte ping/pong payload.
pub fn ipc_frame(reply: bool) -> [u8; 44] {
    let mut bytes = [0; 44];
    bytes[..4].copy_from_slice(b"NXIP");
    bytes[4] = 1;
    bytes[8] = 1;
    bytes[12] = u8::from(reply);
    bytes[16] = 1;
    bytes[24] = 4;
    bytes[40..].copy_from_slice(if reply { b"pong" } else { b"ping" });
    bytes
}

/// Check an integer and SIMD register across a real process yield. The
/// kernel must restore the entire context before returning to this code.
pub fn context_probe() -> bool {
    let result: u64;
    unsafe {
        asm!(
            "mov r12, 0x123456789abcdef0",
            "movq xmm15, r12",
            "int 0x80",
            "mov r13, 0x123456789abcdef0",
            "xor eax, eax",
            "cmp r12, r13",
            "jne 2f",
            "movq r14, xmm15",
            "cmp r14, r13",
            "jne 2f",
            "mov eax, 1",
            "2:",
            inlateout("rax") Syscall::ThreadYield as u64 => result,
            lateout("r12") _, lateout("r13") _, lateout("r14") _,
            lateout("xmm15") _, lateout("rcx") _, lateout("r11") _);
    }
    result == 1
}

pub const FILE_PROBE_CONTENT: &[u8] = b"native capability file\n";
pub fn ipc_frame_with_handles(reply: bool, handles: [u64; 2]) -> [u8; 60] {
    let mut bytes = [0; 60];
    bytes[..44].copy_from_slice(&ipc_frame(reply));
    bytes[28] = 2;
    bytes[44..52].copy_from_slice(&handles[0].to_le_bytes());
    bytes[52..60].copy_from_slice(&handles[1].to_le_bytes());
    bytes
}

pub fn event_create(flags: u64, rights: next_abi::Rights) -> i64 {
    call(Syscall::EventCreate, [flags, rights.0, 0, 0, 0, 0])
}
pub fn event_signal(handle: u64) -> i64 {
    call(Syscall::EventSignal, [handle, 0, 0, 0, 0, 0])
}
pub fn event_reset(handle: u64) -> i64 {
    call(Syscall::EventReset, [handle, 0, 0, 0, 0, 0])
}

/// Request trusted file selection, without ambient namespace authority.
pub fn portal_request_open(access: u64, deadline: u64) -> i64 {
    call(Syscall::PortalRequestOpen, [access, deadline, 0, 0, 0, 0])
}
/// Poll an owner-bound chooser request for its scoped file capability.
pub fn portal_poll(request: u64) -> i64 {
    call(Syscall::PortalPoll, [request, 0, 0, 0, 0, 0])
}
pub fn portal_cancel(request: u64) -> i64 {
    call(Syscall::PortalCancel, [request, 0, 0, 0, 0, 0])
}

/// Retrieve PID1's exact authenticated supervisor configuration bytes.
pub fn service_configuration(output: &mut [u8]) -> i64 {
    call(
        Syscall::ServiceConfiguration,
        [output.as_mut_ptr() as u64, output.len() as u64, 0, 0, 0, 0],
    )
}
