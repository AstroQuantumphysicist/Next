//! Single-CPU x86-64 isolation, context entry, and interrupt gates.
//! The built-in protection probes run before native process gates are enabled.
//! Low identity pages remain supervisor-only; userspace has separate roots,
//! owned integer/FX state, and bounded syscall and fault callbacks.
//! This ABI saves x87/MMX/SSE (FX512), disables AVX/XSAVE and user FSGSBASE,
//! and explicitly saves/restores owned FS/GS TLS bases. PKU/CET/UINTR are disabled;
//! inert PKRU state is reset on entry rather than preserved as a user feature.
#[path = "ap_trampoline.rs"]
mod ap_trampoline;
#[path = "guarded_stack.rs"]
mod guarded_stack;
pub use ap_trampoline::Error as ApTrampolineError;
#[path = "ap_startup_hardware.rs"]
pub mod ap_startup;
use core::arch::{asm, global_asm};
use core::ptr::{addr_of, addr_of_mut};
const USER_CODE: u64 = 0x40_0000_0000;
const USER_STACK: u64 = USER_CODE + 0x2000;
const NX: u64 = 1 << 63;
// The development context format owns x87/MMX/SSE only. Disable controls
// exposing TLS bases or extended per-thread state that it does not preserve.
const UNSAVED_USER_FEATURES: u64 = (1 << 16) | (1 << 18) | (1 << 22) | (1 << 23) | (1 << 25);
const FX_FEATURES: u64 = (1 << 9) | (1 << 10);
#[repr(C, align(4096))]
struct Page([u8; 4096]);
#[repr(C, align(4096))]
struct Table([u64; 512]);
static mut PML4: Table = Table([0; 512]);
static mut PDPT: Table = Table([0; 512]);
static mut LOW_PD: [Table; 4] = [const { Table([0; 512]) }; 4];
static mut AP_TRAMPOLINE_PT: Table = Table([0; 512]);
static mut AP_TRAMPOLINE_MAPPED: bool = false;
static mut KERNEL_PT: [Table; 8] = [const { Table([0; 512]) }; 8];
static mut USER_PD: Table = Table([0; 512]);
static mut USER_PT: Table = Table([0; 512]);
static mut CODE: Page = Page([0; 4096]);
static mut STACK: Page = Page([0; 4096]);
const IRQ_STACK_BYTES: usize = 65536;
const IRQ_STACK_TOP: usize = 4096 + IRQ_STACK_BYTES;
const IRQ_STACK_TOTAL: usize = IRQ_STACK_TOP + 4096;
#[repr(C, align(4096))]
struct InterruptStack([u8; IRQ_STACK_TOTAL]);
static mut IRQ_STACK: InterruptStack = InterruptStack([0; IRQ_STACK_TOTAL]);
const EMERGENCY_STACK_BYTES: usize = 16384;
const EMERGENCY_STACK_TOTAL: usize = EMERGENCY_STACK_BYTES + 8192;
#[repr(C, align(4096))]
struct EmergencyStack([u8; EMERGENCY_STACK_TOTAL]);
// Dedicated fatal #DF IST; never shared with the normal IRQ/syscall stack.
static mut EMERGENCY_STACK: EmergencyStack = EmergencyStack([0; EMERGENCY_STACK_TOTAL]);
// TSS architectural layout: reserved u32, RSP[3], reserved u64,
// IST[7], reserved u64/u16, I/O bitmap offset u16.
#[repr(C, packed)]
struct Tss {
    reserved: u32,
    rsp: [u64; 3],
    reserved2: u64,
    ist: [u64; 7],
    reserved3: u64,
    reserved4: u16,
    io_offset: u16,
}
static mut TSS: Tss = Tss {
    reserved: 0,
    rsp: [0; 3],
    reserved2: 0,
    ist: [0; 7],
    reserved3: 0,
    reserved4: 0,
    io_offset: 104,
};
static mut GDT: [u64; 7] = [
    0,
    0x00af9a000000ffff,
    0x00cf92000000ffff,
    0x00cff2000000ffff,
    0x00affa000000ffff,
    0,
    0,
];
#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    mid: u16,
    high: u32,
    reserved: u32,
}
const EMPTY: Gate = Gate {
    low: 0,
    selector: 0,
    ist: 0,
    flags: 0,
    mid: 0,
    high: 0,
    reserved: 0,
};
static mut IDT: [Gate; 256] = [EMPTY; 256];
#[no_mangle]
static mut ARCH_RETURN_RSP: u64 = 0;
#[no_mangle]
static mut ARCH_PROCESS_RETURN_RSP: u64 = 0;
#[no_mangle]
static mut ARCH_PROCESS_FX: *mut u8 = core::ptr::null_mut();
#[no_mangle]
static mut ARCH_SYSCALL_USER_RSP: u64 = 0;
#[no_mangle]
static mut ARCH_SYSCALL_KERNEL_RSP: u64 = 0;
static SYSCALL_AVAILABLE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Complete integer context, including the hardware privilege-return frame.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CpuFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}
#[repr(C)]
pub struct FaultFrame {
    pub registers: [u64; 15],
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}
const _: () = assert!(core::mem::size_of::<CpuFrame>() == 160);
const _: () = assert!(core::mem::size_of::<FaultFrame>() == 176);
static mut STARTED: bool = false;
static mut EXPECT_FAULT: bool = false;
static mut EXPECT_ADDRESS: u64 = 0;
static mut EXPECT_FLAGS: u64 = 0;
static mut EXPECT_CS: u64 = 35;
static mut EXPECT_RIP: u64 = 0;
const PROTECTION_RETURN: u64 = 0x5046;
#[derive(Debug)]
pub enum ArchError {
    AlreadyRun,
    NoNx,
    AddressRange,
    CodeSize,
    UserFailure,
    ProtectionFailure,
    UnsupportedPagingMode,
    InheritedSmap,
    KernelLayout,
    ContextProfile,
}

// These routines use SysV64 explicitly even though the image entry uses Win64.
// User interrupt saves caller-saved registers; Rust preserves the remaining
// nonvolatile registers. The TSS supplies an owned kernel interrupt stack.
global_asm!(
    r#"
.global arch_load_gdt
arch_load_gdt:
    lgdt [rdi]
    push 8
    lea rax, [rip + 2f]
    push rax
    retfq
2:
    mov ax, 16
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor eax, eax
    mov fs, ax
    mov gs, ax
    mov ax, 40
    ltr ax
    ret
.global arch_enter_user
arch_enter_user:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [rip + ARCH_RETURN_RSP], rsp
    push 27
    push rsi
    push 2
    push 35
    push rdi
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi
    xor ebp, ebp
    xor r8d, r8d
    xor r9d, r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    iretq
.global arch_int80
arch_int80:
    cld
    cmp rax, 2
    je 3f
    push rax
    push rcx
    push rdx
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    mov rdx, rsi
    mov rsi, rdi
    mov rdi, rax
    call arch_dispatch
    mov [rsp + 64], rax
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rax
    iretq
3:
    mov rax, rdi
    mov rsp, [rip + ARCH_RETURN_RSP]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
.global arch_fatal
arch_fatal:
    cli
    cld
    and rsp, -16
    call arch_fault
4:
    hlt
    jmp 4b
.global arch_page_fault
arch_page_fault:
    cld
    mov rdi, cr2
    mov rsi, [rsp]
    mov rdx, [rsp + 16]
    mov rcx, [rsp + 8]
    and rsp, -16
    call arch_expected_fault
    test al, al
    jz arch_fatal
    mov eax, 0x5046
    mov rsp, [rip + ARCH_RETURN_RSP]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
.global arch_kernel_probe
arch_kernel_probe:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [rip + ARCH_RETURN_RSP], rsp
    test rsi, rsi
    jnz 11f
.global arch_kernel_write_instruction
arch_kernel_write_instruction:
    mov byte ptr [rdi], 0
    jmp arch_fatal
11:
    jmp rdi
.global arch_demo_start
.global arch_demo_end
.global arch_probe_supervisor
.global arch_probe_write
.global arch_probe_execute
arch_demo_start:
    push 0x12345678
    pop rdx
    cmp rdx, 0x12345678
    jne 5f
    mov ax, cs
    and eax, 3
    cmp eax, 3
    jne 5f
    mov eax, 1
    lea rdi, [rip + 6f]
    mov esi, 39
    int 0x80
    test rax, rax
    jne 5f
    mov eax, 1
    xor edi, edi
    mov esi, 8
    int 0x80
    cmp rax, -1
    jne 5f
    mov eax, 1
    lea rdi, [rip + 8f]
    mov esi, 45
    int 0x80
    test rax, rax
    jne 5f
    xor edi, edi
    jmp 10f
5:
    mov edi, 1
10:
    mov eax, 2
    int 0x80
    ud2
6:
    .ascii "HELIX USER: ring3 isolated native code\n"
7:
8:
    .ascii "HELIX USER: invalid syscall pointer rejected\n"
9:
arch_probe_supervisor:
    mov eax, 0x100000
    mov rax, [rax]
    mov edi, 1
    mov eax, 2
    int 0x80
    ud2
arch_probe_write:
    mov rax, 0x4000000000
    mov byte ptr [rax], 0
    mov edi, 1
    mov eax, 2
    int 0x80
    ud2
arch_probe_execute:
    mov rax, 0x4000002000
    jmp rax
arch_demo_end:
"#
);
extern "sysv64" {
    fn arch_load_gdt(descriptor: *const Descriptor);
    fn arch_enter_user(code: u64, stack: u64) -> u64;
    fn arch_int80();
    fn arch_fatal();
    fn arch_page_fault();
    fn arch_kernel_probe(address: u64, execute: u64) -> u64;
    static arch_kernel_write_instruction: u8;
    static arch_demo_start: u8;
    static arch_demo_end: u8;
    static arch_probe_supervisor: u8;
    static arch_probe_write: u8;
    static arch_probe_execute: u8;
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

global_asm!(
    r#"
.macro native_push
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
.endm
.macro native_pop
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax
.endm
.global arch_enter_process
arch_enter_process:
    cli
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    sub rsp, 520
    fxsave64 [rsp]
    mov [rip + ARCH_PROCESS_RETURN_RSP], rsp
    mov [rip + ARCH_PROCESS_FX], rsi
    fxrstor64 [rsi]
    mov rsp, rdi
    native_pop
    iretq
.global arch_native_unwind
arch_native_unwind:
    cli
    mov rax, [rip + ARCH_PROCESS_FX]
    fxsave64 [rax]
    mov rsp, [rip + ARCH_PROCESS_RETURN_RSP]
    fxrstor64 [rsp]
    add rsp, 520
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
.global arch_native_int80
arch_native_int80:
    cld
    native_push
    mov rbx, rsp
    and rsp, -16
    sub rsp, 512
    fxsave64 [rsp]
    mov rdi, rbx
    call native_dispatch
    fxrstor64 [rsp]
    test rax, rax
    jnz arch_native_unwind
    mov rsp, rbx
    native_pop
    iretq
// SYSCALL does not switch RSP. SFMASK masks interrupts and tracing before
// the first instruction; only supervisor-owned globals are touched here.
.global arch_linux_syscall
// Original NT API gate. Windows calling convention arguments stay in their
// registers; the complete Win64 nonvolatile GPR/SSE set survives this entry.
.global arch_nt_int81
arch_nt_int81:
    cld
    native_push
    mov rbx, rsp
    and rsp, -16
    sub rsp, 512
    fxsave64 [rsp]
    mov rdi, rbx
    call nt_dispatch
    fxrstor64 [rsp]
    test rax, rax
    jnz arch_native_unwind
    mov rsp, rbx
    native_pop
    iretq
arch_linux_syscall:
    mov [rip + ARCH_SYSCALL_USER_RSP], rsp
    mov rsp, [rip + ARCH_SYSCALL_KERNEL_RSP]
    push 27
    push qword ptr [rip + ARCH_SYSCALL_USER_RSP]
    push r11
    push 35
    push rcx
    native_push
    cld
    mov rbx, rsp
    and rsp, -16
    sub rsp, 512
    fxsave64 [rsp]
    mov rdi, rbx
    call linux_dispatch
    fxrstor64 [rsp]
    test rax, rax
    jnz arch_native_unwind
    mov rsp, rbx
    native_pop
    iretq
.macro native_no_error vector
.global arch_native_fault_\vector
arch_native_fault_\vector:
    push 0
    push \vector
    jmp arch_native_fault_common
.endm
.macro native_error vector
.global arch_native_fault_\vector
arch_native_fault_\vector:
    push \vector
    jmp arch_native_fault_common
.endm
native_no_error 0
native_no_error 1
native_no_error 3
native_no_error 4
native_no_error 5
native_no_error 6
native_no_error 7
native_error 8
native_error 10
native_error 11
native_error 12
native_error 13
native_error 14
native_no_error 16
native_error 17
native_no_error 19
arch_native_fault_common:
    cld
    native_push
    mov rbx, rsp
    and rsp, -16
    sub rsp, 512
    fxsave64 [rsp]
    mov rdi, rbx
    mov rsi, cr2
    call native_fault
    fxrstor64 [rsp]
    test rax, rax
    jnz arch_native_unwind
    mov rdi, rbx
    mov rsi, cr2
    call arch_kernel_fault
"#
);
extern "sysv64" {
    fn arch_enter_process(frame: *const CpuFrame, fx: *mut u8);
    fn arch_native_int80();
    fn arch_nt_int81();
    fn arch_linux_syscall();
    fn arch_native_fault_0();
    fn arch_native_fault_1();
    fn arch_native_fault_3();
    fn arch_native_fault_4();
    fn arch_native_fault_5();
    fn arch_native_fault_6();
    fn arch_native_fault_7();
    fn arch_native_fault_8();
    fn arch_native_fault_10();
    fn arch_native_fault_11();
    fn arch_native_fault_12();
    fn arch_native_fault_13();
    fn arch_native_fault_14();
    fn arch_native_fault_16();
    fn arch_native_fault_17();
    fn arch_native_fault_19();
}

/// # Safety
/// Sole CPU, a validated CPL3 frame, and a writable aligned 512-byte FX area.
/// No mutable runtime borrow may remain live while callbacks execute.
pub unsafe fn enter_process(frame: *const CpuFrame, fx: *mut u8, fs: u64, gs: u64) {
    asm!("cli", options(nostack));
    reset_tls_bases();
    install_tls_bases(fs, gs);
    arch_enter_process(frame, fx);
    // Kernel Rust runs without TLS; never carry one process's bases into
    // another process or an interactive kernel task after context return.
    install_tls_bases(0, 0);
}
pub fn kernel_cr3() -> u64 {
    addr_of!(PML4) as u64
}
/// # Safety
/// Both outputs own 512-entry, page-aligned tables, with interrupts masked.
pub unsafe fn copy_kernel_root(pml4: *mut u64, pdpt: *mut u64) {
    core::ptr::write_bytes(pml4, 0, 512);
    core::ptr::write_bytes(pdpt, 0, 512);
    pml4.write(pdpt as u64 | 7);
    for index in 0..4 {
        pdpt.add(index)
            .write((addr_of!(PDPT.0) as *const u64).add(index).read());
    }
}
/// # Safety
/// CR3 is a validated owned root; all live kernel mappings remain present.
pub unsafe fn activate(cr3: u64) {
    asm!("mov cr3, {}", in(reg) cr3, options(nostack));
}
/// Change a native image's identity alias; the caller owns its lifecycle.
/// # Safety
/// Interrupts masked; address is an owned data page, with no live user writes
/// when its executable alias is being changed or reclaimed.
pub unsafe fn protect_physical(address: u64, write: bool) -> bool {
    let data_start: u64;
    let kernel_end: u64;
    asm!("lea {}, [rip + {symbol}]", out(reg) data_start,
        symbol = sym __data_start, options(nomem, nostack, preserves_flags));
    asm!("lea {}, [rip + {symbol}]", out(reg) kernel_end,
        symbol = sym __kernel_end, options(nomem, nostack, preserves_flags));
    if !address.is_multiple_of(4096) || !(data_start..kernel_end).contains(&address) {
        return false;
    }
    let slot = ((address - next_abi::kernel::KERNEL_BASE) / 4096) as usize;
    if slot >= 8 * 512 {
        return false;
    }
    let entry = addr_of_mut!(KERNEL_PT).cast::<u64>().add(slot);
    let old = entry.read();
    // Never turn supervisor executable or originally read-only kernel data
    // into writable memory. Native images reside in the RW/NX data segment.
    if old & 1 == 0 || old & NX == 0 {
        return false;
    }
    entry.write((old & !2) | if write { 2 } else { 0 });
    asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
    true
}
/// # Safety
/// Sole CPU, interrupts masked, probes finished, native callbacks installed.
pub unsafe fn install_native_gates() {
    install_syscall_entry();
    let idt = addr_of_mut!(IDT).cast::<Gate>();
    idt.add(0x80)
        .write(gate(arch_native_int80 as *const () as u64, 0xee));
    idt.add(0x81)
        .write(gate(arch_nt_int81 as *const () as u64, 0xee));
    let mut double_fault = gate(arch_native_fault_8 as *const () as u64, 0x8e);
    double_fault.ist = 1;
    idt.add(8).write(double_fault);
    for (vector, handler) in [
        (0, arch_native_fault_0 as *const ()),
        (1, arch_native_fault_1 as *const ()),
        (3, arch_native_fault_3 as *const ()),
        (4, arch_native_fault_4 as *const ()),
        (5, arch_native_fault_5 as *const ()),
        (6, arch_native_fault_6 as *const ()),
        (7, arch_native_fault_7 as *const ()),
        (10, arch_native_fault_10 as *const ()),
        (11, arch_native_fault_11 as *const ()),
        (12, arch_native_fault_12 as *const ()),
        (13, arch_native_fault_13 as *const ()),
        (14, arch_native_fault_14 as *const ()),
        (16, arch_native_fault_16 as *const ()),
        (17, arch_native_fault_17 as *const ()),
        (19, arch_native_fault_19 as *const ()),
    ] {
        let flags = if vector == 3 || vector == 4 {
            0xee
        } else {
            0x8e
        };
        idt.add(vector).write(gate(handler as u64, flags));
    }
}

fn gate(address: u64, flags: u8) -> Gate {
    Gate {
        low: address as u16,
        selector: 8,
        ist: 0,
        flags,
        mid: (address >> 16) as u16,
        high: (address >> 32) as u32,
        reserved: 0,
    }
}
global_asm!(
    ".global arch_apic_spurious_irq",
    "arch_apic_spurious_irq:",
    "iretq",
);
extern "sysv64" {
    fn arch_apic_spurious_irq();
}
/// Install the owned BSP spurious-vector gate before enabling an APIC route.
/// A spurious IRQ has no ISR bit and must return without sending EOI.
/// # Safety
/// Sole BSP after the owned IDT is installed; no CPU concurrently changes it.
/// This preserves IF and modifies only the supervisor vector0xff gate.
pub unsafe fn install_apic_spurious_gate() -> bool {
    let flags: u64;
    asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    asm!("cli", options(nostack));
    let mut descriptor = Descriptor { limit: 0, base: 0 };
    asm!("sidt [{}]", in(reg) &mut descriptor, options(nostack));
    let idt = addr_of_mut!(IDT).cast::<Gate>();
    let valid = descriptor.base == idt as u64 && descriptor.limit == 4095;
    if valid {
        idt.add(0xff)
            .write(gate(arch_apic_spurious_irq as *const () as u64, 0x8e));
    }
    if flags & (1 << 9) != 0 {
        asm!("sti", options(nostack));
    }
    valid
}
#[no_mangle]
extern "sysv64" fn arch_fault() -> ! {
    super::input::write_serial("HELIX FATAL: isolation CPU exception; halted\n");
    super::halt()
}
#[no_mangle]
extern "sysv64" fn arch_kernel_fault(frame: *const FaultFrame, address: u64) -> ! {
    use core::fmt::Write;
    unsafe { install_tls_bases(0, 0) }
    let frame = unsafe { &*frame };
    let boot_base = addr_of!(super::KERNEL_STACK) as u64;
    let irq_base = addr_of!(IRQ_STACK) as u64;
    let _ = writeln!(super::Serial,
        "HELIX FATAL: kernel exception vector={} error={:#x} rip={:#x} cs={:#x} rsp={:#x} cr2={:#x}; boot_stack={:#x}..{:#x} irq_stack={:#x}..{:#x}",
        frame.vector, frame.error, frame.rip, frame.cs, frame.rsp, address,
        boot_base + 4096, boot_base + 69632,
        irq_base + 4096, irq_base + IRQ_STACK_TOP as u64);
    super::halt()
}
#[no_mangle]
extern "sysv64" fn arch_expected_fault(address: u64, flags: u64, cs: u64, rip: u64) -> bool {
    // No general recovery: only the precisely registered user-mode test fault
    // may unwind this one-shot execution. All kernel/unexpected faults halt.
    unsafe {
        let user_instruction = (USER_CODE..USER_CODE + 4096).contains(&rip)
            || (EXPECT_FLAGS == 0x15 && rip == USER_STACK);
        EXPECT_FAULT
            && cs == EXPECT_CS
            && address == EXPECT_ADDRESS
            && flags == EXPECT_FLAGS
            && if EXPECT_CS == 35 {
                user_instruction
            } else {
                rip == EXPECT_RIP
            }
    }
}
#[no_mangle]
extern "sysv64" fn arch_dispatch(op: u64, pointer: u64, length: u64) -> u64 {
    if op != 1 || length > 256 {
        return u64::MAX;
    }
    let Some(end) = pointer.checked_add(length) else {
        return u64::MAX;
    };
    // Exact registered read-only user region; never dereference arbitrary pointers.
    if pointer < USER_CODE || end > USER_CODE + 4096 {
        return u64::MAX;
    }
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return u64::MAX;
    };
    super::input::write_serial(text);
    0
}

/// Install an owned address space and run the built-in ring-3 test once.
///
/// # Safety
/// Must run on the sole bootstrap CPU after ExitBootServices with IF clear.
/// The image, kernel stack, framebuffer and all live identity addresses must
/// fit below 4 GiB. LoaderCode/LoaderData must remain reserved permanently.
/// This replaces CR3/GDT/TSS/IDT and keeps them installed after return. No
/// firmware services or firmware selectors may be used afterward.
pub unsafe fn run_user_demo() -> Result<(), ArchError> {
    if STARTED {
        return Err(ArchError::AlreadyRun);
    }
    let inherited_cr4: u64;
    asm!("mov {}, cr4", out(reg) inherited_cr4, options(nomem, nostack, preserves_flags));
    // This bootstrap constructs four levels, and its bounded syscall copy does
    // not yet implement SMAP access windows. Reject incompatible inherited CPU
    // states before changing firmware mappings rather than faulting afterward.
    if inherited_cr4 & (1 << 12) != 0 {
        return Err(ArchError::UnsupportedPagingMode);
    }
    if inherited_cr4 & (1 << 21) != 0 {
        return Err(ArchError::InheritedSmap);
    }
    let highest = core::arch::x86_64::__cpuid(0x80000000).eax;
    if highest < 0x80000001 || core::arch::x86_64::__cpuid(0x80000001).edx & (1 << 20) == 0 {
        return Err(ArchError::NoNx);
    }
    let stack_pointer: u64;
    asm!("mov {}, rsp",out(reg) stack_pointer,options(nomem,nostack,preserves_flags));
    for address in [
        addr_of!(PML4) as u64 + core::mem::size_of::<Table>() as u64,
        addr_of!(PDPT) as u64 + core::mem::size_of::<Table>() as u64,
        addr_of!(LOW_PD) as u64 + core::mem::size_of::<[Table; 4]>() as u64,
        addr_of!(USER_PD) as u64 + core::mem::size_of::<Table>() as u64,
        addr_of!(USER_PT) as u64 + core::mem::size_of::<Table>() as u64,
        addr_of!(GDT) as u64 + core::mem::size_of::<[u64; 7]>() as u64,
        addr_of!(IDT) as u64 + core::mem::size_of::<[Gate; 256]>() as u64,
        addr_of!(TSS) as u64 + core::mem::size_of::<Tss>() as u64,
        addr_of!(IRQ_STACK) as u64 + IRQ_STACK_TOTAL as u64,
        addr_of!(STACK) as u64 + 4096,
        addr_of!(CODE) as u64 + 4096,
        stack_pointer,
        arch_fatal as *const () as u64,
    ] {
        if address >= 1 << 32 {
            return Err(ArchError::AddressRange);
        }
    }
    let code_start = addr_of!(arch_demo_start) as usize;
    let code_len = (addr_of!(arch_demo_end) as usize)
        .checked_sub(code_start)
        .ok_or(ArchError::CodeSize)?;
    if code_len == 0 || code_len > 4096 {
        return Err(ArchError::CodeSize);
    }
    core::ptr::copy_nonoverlapping(
        code_start as *const u8,
        addr_of_mut!(CODE.0) as *mut u8,
        code_len,
    );
    // Check linker ownership and page alignment before deriving permissions.
    // Linker boundary symbols may alias. Inline assembly avoids Rust/LLVM's
    // distinct-static-address assumptions folding valid adjacent boundaries.
    macro_rules! boundary {
        ($symbol:ident) => {{
            let address: u64;
            asm!("lea {}, [rip + {symbol}]", out(reg) address, symbol = sym $symbol,
                options(nomem, nostack, preserves_flags));
            address
        }};
    }
    let kernel_start = boundary!(__kernel_start);
    let text_start = boundary!(__text_start);
    let text_end = boundary!(__text_end);
    let ro_start = boundary!(__rodata_start);
    let ro_end = boundary!(__rodata_end);
    let data_start = boundary!(__data_start);
    let kernel_end = boundary!(__kernel_end);
    if kernel_start != next_abi::kernel::KERNEL_BASE
        || text_start != kernel_start
        || text_end != ro_start
        || ro_end != data_start
        || text_start >= text_end
        || ro_start >= ro_end
        || data_start >= kernel_end
        || kernel_end > kernel_start + next_abi::kernel::MAX_KERNEL_MEMORY
        || [kernel_start, text_end, ro_end, kernel_end]
            .iter()
            .any(|a| !a.is_multiple_of(4096))
    {
        return Err(ArchError::KernelLayout);
    }
    let emergency = guarded_stack::Layout::new(
        addr_of!(EMERGENCY_STACK) as u64,
        EMERGENCY_STACK_BYTES as u64,
    )
    .map_err(|_| ArchError::KernelLayout)?;
    if (addr_of!(EMERGENCY_STACK) as u64) < data_start
        || emergency.end() > kernel_end
        || emergency.frame(176 + 512 + 15 + 512, 16).is_err()
    {
        return Err(ArchError::KernelLayout);
    }
    STARTED = true;
    let root = addr_of_mut!(PML4.0) as *mut u64;
    let pdpt = addr_of_mut!(PDPT.0) as *mut u64;
    root.write(pdpt as u64 | 7);
    for gigabyte in 0..4 {
        let pd = addr_of_mut!(LOW_PD)
            .cast::<Table>()
            .add(gigabyte)
            .cast::<u64>();
        pdpt.add(gigabyte).write(pd as u64 | 3);
        for index in 0..512 {
            pd.add(index)
                .write(((gigabyte * 512 + index) as u64 * 0x200000) | 0x83);
            let entry = pd.add(index);
            entry.write(entry.read() | NX);
        }
    }
    // Split the kernel's huge-page spans. All remaining low huge pages are
    // supervisor RW/NX; firmware and MMIO can never be instruction sources.
    let first = kernel_start / 0x200000;
    let chunks = (kernel_end - kernel_start).div_ceil(0x200000) as usize;
    for chunk in 0..chunks {
        let table = addr_of_mut!(KERNEL_PT)
            .cast::<Table>()
            .add(chunk)
            .cast::<u64>();
        for index in 0..512 {
            let address = (first + chunk as u64) * 0x200000 + index as u64 * 4096;
            let flags = if (text_start..text_end).contains(&address) {
                1
            } else if (ro_start..ro_end).contains(&address) || address == addr_of!(CODE) as u64 {
                1 | NX
            } else {
                3 | NX
            };
            let stack_base = addr_of!(super::KERNEL_STACK) as u64;
            let irq_base = addr_of!(IRQ_STACK) as u64;
            let guard = address == stack_base
                || address == stack_base + 69632
                || address == irq_base
                || address == irq_base + IRQ_STACK_TOP as u64
                || emergency.guard(address)
                || ap_startup::stack_guard(address);
            table
                .add(index)
                .write(if guard { 0 } else { address | flags });
        }
        let slot = (first + chunk as u64) as usize;
        addr_of_mut!(LOW_PD)
            .cast::<Table>()
            .add(slot / 512)
            .cast::<u64>()
            .add(slot % 512)
            .write(table as u64 | 3);
    }
    pdpt.add(256).write(addr_of!(USER_PD) as u64 | 7);
    (addr_of_mut!(USER_PD.0) as *mut u64).write(addr_of!(USER_PT) as u64 | 7);
    let user_pt = addr_of_mut!(USER_PT.0) as *mut u64;
    user_pt.write(addr_of!(CODE) as u64 | 5); // Present, user, read-only, executable.
    user_pt.add(2).write(addr_of!(STACK) as u64 | 7 | NX);
    // EFER.NXE, CR0.WP. Never disable verification or protection bits.
    let low: u32;
    let high: u32;
    asm!("rdmsr",in("ecx") 0xc0000080u32,out("eax") low,out("edx") high,options(nostack));
    asm!("wrmsr",in("ecx") 0xc0000080u32,in("eax") low | (1<<11),in("edx") high,options(nostack));
    let cr0: u64;
    asm!("mov {}, cr0",out(reg) cr0,options(nomem,nostack));
    asm!("mov cr0, {}",in(reg) cr0 | (1<<16),options(nostack));
    // CR3 replacement alone does not invalidate global firmware translations.
    // Clear PGE to flush them. Preserve paging controls, but enforce the
    // explicitly saved FX512 profile instead of inheriting optional user state.
    let owned_cr4 = (inherited_cr4 & !UNSAVED_USER_FEATURES) | FX_FEATURES;
    asm!("mov cr4, {}", in(reg) owned_cr4 & !(1 << 7), options(nostack));
    asm!("mov cr3, {}",in(reg) root as u64,options(nostack));
    asm!("mov cr4, {}", in(reg) owned_cr4, options(nostack));
    reset_tls_bases();
    let verified_cr4: u64;
    asm!("mov {}, cr4",out(reg) verified_cr4,options(nomem,nostack,preserves_flags));
    if verified_cr4 != owned_cr4
        || [0xc000_0100, 0xc000_0101, 0xc000_0102]
            .into_iter()
            .any(|index| read_msr(index) != 0)
    {
        return Err(ArchError::ContextProfile);
    }
    super::input::write_serial("HELIX: CPU CONTEXT PROFILE PASS FX512 only; OSXSAVE=0 FSGSBASE=0 PKE=0 CET=0 UINTR=0 FSBASE=0 GSBASE=0 KERNELGSBASE=0 PKRU reset; inherited TLS bases zero; explicit profile TLS restored\n");
    let irq_top = addr_of!(IRQ_STACK) as u64 + IRQ_STACK_TOP as u64;
    core::ptr::write_unaligned(addr_of_mut!(TSS.rsp).cast::<u64>(), irq_top);
    core::ptr::write_unaligned(addr_of_mut!(TSS.ist).cast::<u64>(), emergency.top());
    let tss_base = addr_of!(TSS) as u64;
    let gdt = addr_of_mut!(GDT) as *mut u64;
    gdt.add(5).write(
        103 | ((tss_base & 0xffffff) << 16) | (0x89 << 40) | (((tss_base >> 24) & 0xff) << 56),
    );
    gdt.add(6).write(tss_base >> 32);
    let descriptor = Descriptor {
        limit: 55,
        base: gdt as u64,
    };
    arch_load_gdt(&descriptor);
    let idt = addr_of_mut!(IDT) as *mut Gate;
    for index in 0..256 {
        idt.add(index)
            .write(gate(arch_fatal as *const () as u64, 0x8e));
    }
    idt.add(0x80)
        .write(gate(arch_int80 as *const () as u64, 0xee));
    idt.add(14)
        .write(gate(arch_page_fault as *const () as u64, 0x8e));
    let mut double_fault = gate(arch_native_fault_8 as *const () as u64, 0x8e);
    double_fault.ist = 1;
    idt.add(8).write(double_fault);
    idt.add(0x20).write(gate(
        super::timer::helix_timer_irq as *const () as u64,
        0x8e,
    ));
    let descriptor = Descriptor {
        limit: 4095,
        base: idt as u64,
    };
    asm!("lidt [{}]",in(reg) &descriptor,options(readonly,nostack));
    super::input::write_serial(
        "HELIX: KERNEL MAPPINGS PASS RX text, RO NX data, RW NX memory, guarded stack\n",
    );
    EXPECT_CS = 8;
    for (address, execute, flags, message) in [
        (text_start, 0, 3, "HELIX: KERNEL PROTECTION PASS RX text write denied by CPU page fault\n"),
        (ro_start, 0, 3, "HELIX: KERNEL PROTECTION PASS RO data write denied by CPU page fault\n"),
        (addr_of!(CODE) as u64, 0, 3, "HELIX: KERNEL PROTECTION PASS user code physical alias write denied by CPU page fault\n"),
        (addr_of!(STACK) as u64, 1, 0x11, "HELIX: KERNEL PROTECTION PASS NX data execution denied by CPU page fault\n"),
        (addr_of!(super::KERNEL_STACK) as u64, 0, 2, "HELIX: KERNEL PROTECTION PASS kernel stack guard write denied by CPU page fault\n"),
        (addr_of!(IRQ_STACK) as u64, 0, 2, "HELIX: KERNEL PROTECTION PASS interrupt stack guard write denied by CPU page fault\n"),
        (addr_of!(EMERGENCY_STACK) as u64, 0, 2, "HELIX: KERNEL PROTECTION PASS emergency IST lower guard write denied by CPU page fault\n"),
        (emergency.top(), 0, 2, "HELIX: KERNEL PROTECTION PASS emergency IST upper guard write denied by CPU page fault\n"),
    ] {
        EXPECT_FAULT = true;
        EXPECT_ADDRESS = address;
        EXPECT_FLAGS = flags;
        EXPECT_RIP = if execute == 0 { addr_of!(arch_kernel_write_instruction) as u64 } else { address };
        let result = arch_kernel_probe(address, execute);
        EXPECT_FAULT = false;
        if result != PROTECTION_RETURN { return Err(ArchError::ProtectionFailure); }
        super::input::write_serial(message);
    }
    EXPECT_CS = 35;
    super::input::write_serial(
        "HELIX: entering ring3; supervisor-only low memory, user RX code, RW NX stack\n",
    );
    if arch_enter_user(USER_CODE, USER_STACK + 4096) != 0 {
        return Err(ArchError::UserFailure);
    }
    for (entry, address, flags, message) in [
        (
            addr_of!(arch_probe_supervisor) as usize,
            0x100000,
            5,
            "HELIX: USER PROTECTION PASS supervisor read denied by CPU page fault\n",
        ),
        (
            addr_of!(arch_probe_write) as usize,
            USER_CODE,
            7,
            "HELIX: USER PROTECTION PASS RX code write denied by CPU page fault\n",
        ),
        (
            addr_of!(arch_probe_execute) as usize,
            USER_STACK,
            0x15,
            "HELIX: USER PROTECTION PASS NX stack execution denied by CPU page fault\n",
        ),
    ] {
        EXPECT_FAULT = true;
        EXPECT_ADDRESS = address;
        EXPECT_FLAGS = flags;
        let result = arch_enter_user(USER_CODE + (entry - code_start) as u64, USER_STACK + 4096);
        EXPECT_FAULT = false;
        if result != PROTECTION_RETURN {
            return Err(ArchError::ProtectionFailure);
        }
        super::input::write_serial(message);
    }
    super::input::write_serial(
        "HELIX: USER SELFTEST PASS CPL3 entry, bounded syscall, pointer denial, exit\n",
    );
    Ok(())
}

/// Whether the processor supports the original x86-64 SYSCALL entry.
pub fn syscall_available() -> bool {
    SYSCALL_AVAILABLE.load(core::sync::atomic::Ordering::Acquire)
}
unsafe fn write_msr(index: u32, value: u64) {
    asm!("wrmsr", in("ecx") index, in("eax") value as u32,
        in("edx") (value >> 32) as u32, options(nostack));
}
unsafe fn install_syscall_entry() {
    let extensions = core::arch::x86_64::__cpuid(0x8000_0000).eax;
    if extensions < 0x8000_0001 || core::arch::x86_64::__cpuid(0x8000_0001).edx & (1 << 11) == 0 {
        return;
    }
    ARCH_SYSCALL_KERNEL_RSP = addr_of_mut!(IRQ_STACK) as u64 + IRQ_STACK_TOP as u64;
    // STAR kernel CS8/SS16; no SYSRET path is used. IRET restores CS35/SS27.
    write_msr(0xc000_0081, 8u64 << 32);
    write_msr(0xc000_0082, arch_linux_syscall as *const () as u64);
    // Mask TF,IF,DF,NT,RF,AC before the trusted stack switch.
    write_msr(
        0xc000_0084,
        (1 << 8) | (1 << 9) | (1 << 10) | (1 << 14) | (1 << 16) | (1 << 18),
    );
    let low: u32;
    let high: u32;
    asm!("rdmsr",in("ecx") 0xc000_0080u32,out("eax") low,out("edx") high,options(nostack));
    write_msr(0xc000_0080, ((high as u64) << 32) | low as u64 | 1);
    SYSCALL_AVAILABLE.store(true, core::sync::atomic::Ordering::Release);
}

/// Clear inherited selectors, all latent bases, and unsaved protection keys.
unsafe fn reset_tls_bases() {
    asm!("xor eax, eax", "mov fs, ax", "mov gs, ax", out("eax") _, options(nostack));
    for index in [0xc000_0100, 0xc000_0101, 0xc000_0102] {
        write_msr(index, 0);
    }
    // Reset latent PKRU without exposing protection keys to userspace.
    // Some implementations require PKE for WRPKRU; enable it only for this
    // IF-masked CPL0 operation. All kernel accesses use supervisor pages,
    // and the original CR4 is restored before any user context is entered.
    if core::arch::x86_64::__cpuid_count(7, 0).ecx & (1 << 3) != 0 {
        let cr4: u64;
        asm!("mov {}, cr4",out(reg) cr4,options(nomem,nostack,preserves_flags));
        asm!("mov cr4, {}",in(reg) cr4 | (1<<22),options(nostack));
        asm!("wrpkru",in("eax") 0u32,in("ecx") 0u32,in("edx") 0u32,options(nostack));
        asm!("mov cr4, {}",in(reg) cr4,options(nostack));
    }
}
unsafe fn read_msr(index: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!("rdmsr",in("ecx") index,out("eax") low,out("edx") high,options(nostack));
    ((high as u64) << 32) | low as u64
}
/// Read the actual user bases while IF is masked. Kernel code has no FS/GS
/// addressing, so callbacks can capture them before restoring the next task.
pub fn tls_bases() -> (u64, u64) {
    unsafe { (read_msr(0xc000_0100), read_msr(0xc000_0101)) }
}
/// # Safety
/// IF masked; bases validated as owned user mappings or zero by the runtime.
pub unsafe fn install_tls_bases(fs: u64, gs: u64) {
    write_msr(0xc000_0100, fs);
    write_msr(0xc000_0101, gs);
    write_msr(0xc000_0102, 0);
}

/// Invalidate one owned user mapping after a PTE mutation on the sole CPU.
/// # Safety
/// Interrupts masked; the address is in the active process user window.
pub unsafe fn flush_user_page(address: u64) {
    asm!("invlpg [{}]",in(reg) address,options(nostack,preserves_flags));
}
/// Select the architectural UC PAT entry for one owned 2 MiB MMIO identity
/// leaf. Existing process roots reference these same supervisor-only tables.
/// # Safety
/// Boot CPU, IF clear; caller verified that this address belongs to MMIO.
pub unsafe fn mark_mmio_uncached(address: u64) -> bool {
    if !(0xc000_0000..0x1_0000_0000).contains(&address)
        || core::arch::x86_64::__cpuid(1).edx & (1 << 16) == 0
        || (read_msr(0x277) >> 24) & 0xff != 0
    {
        return false;
    }
    let entry = addr_of_mut!(LOW_PD)
        .cast::<Table>()
        .add((address >> 30) as usize)
        .cast::<u64>()
        .add(((address >> 21) & 511) as usize);
    let old = entry.read();
    if old & 0x85 != 0x81 || old & NX == 0 {
        return false;
    }
    // PAT index 3 is PCD=1,PWT=1,PAT=0; never silently assume firmware's PAT.
    asm!("wbinvd", options(nostack));
    entry.write((old & !(1 << 12)) | (1 << 3) | (1 << 4));
    let cr4: u64;
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    asm!("mov cr4, {}", in(reg) cr4 & !(1 << 7), options(nostack));
    let cr3: u64;
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    asm!("mov cr4, {}", in(reg) cr4, options(nostack));
    asm!("wbinvd", options(nostack));
    true
}

/// Publish one supervisor RO/RX trampoline from the authenticated immutable
/// kernel image, leaving every other low page supervisor RW/NX. Never starts APs.
/// This API is deliberately not invoked by the reservation-only boot stage.
/// # Safety
/// BSP before AP startup, IF clear, current root is the owned kernel root.
/// `regions` describes the retained handoff map. `image` is the complete reviewed
/// position-independent AP startup image embedded in the authenticated kernel;
/// linker-span checks establish immutable storage, not startup-code correctness.
/// The caller must separately establish isolated stack/GDT/mailbox/timeout and
/// startup-protocol correctness before activating any AP. No runtime-owned
/// globals or user mapping may acquire a writable alias of this reserved page.
#[allow(dead_code)]
pub unsafe fn map_ap_trampoline(
    boot: &next_abi::boot::BootInfo,
    regions: &next_abi::boot::MemoryRegions<'_>,
    image: &[u8],
) -> Result<(), ApTrampolineError> {
    let flags: u64;
    let cr3: u64;
    let cr0: u64;
    asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
    if flags & (1 << 9) != 0
        || cr3 != kernel_cr3()
        || cr0 & (1 << 16) == 0
        || read_msr(0xc0000080) & (1 << 11) == 0
        || AP_TRAMPOLINE_MAPPED
    {
        return Err(ApTrampolineError::State);
    }
    let pdpt_entry = (addr_of!(PDPT.0) as *const u64).read();
    // Accessed is hardware-owned; all other topology/permission bits must match.
    if pdpt_entry & !(1 << 5) != addr_of!(LOW_PD) as u64 | 3 {
        return Err(ApTrampolineError::Mapping);
    }
    let root_entry = (addr_of!(PML4.0) as *const u64).read();
    if root_entry & !(1 << 5) != addr_of!(PDPT) as u64 | 7 {
        return Err(ApTrampolineError::Mapping);
    }
    let text_start: u64;
    let ro_end: u64;
    asm!("lea {}, [rip + {symbol}]", out(reg) text_start,
        symbol = sym __text_start, options(nomem, nostack, preserves_flags));
    asm!("lea {}, [rip + {symbol}]", out(reg) ro_end,
        symbol = sym __rodata_end, options(nomem, nostack, preserves_flags));
    let directory = addr_of_mut!(LOW_PD).cast::<u64>();
    let plan = ap_trampoline::SplitPlan::new(
        boot,
        regions,
        directory.read(),
        image.as_ptr() as u64,
        image.len(),
        text_start,
        ro_end,
    )?;
    let table = addr_of_mut!(AP_TRAMPOLINE_PT.0).cast::<u64>();
    for index in 0..512 {
        table
            .add(index)
            .write(plan.entry(index).ok_or(ApTrampolineError::Mapping)?);
    }
    // The current huge leaf is still writable and NX. Authenticate/validate all
    // inputs before touching this loader-owned page; never create W+X.
    let destination = next_abi::boot::AP_TRAMPOLINE_BASE as *mut u8;
    core::ptr::write_bytes(destination, 0, next_abi::boot::AP_TRAMPOLINE_BYTES as usize);
    core::ptr::copy_nonoverlapping(image.as_ptr(), destination, plan.image_len());
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    directory.write(table as u64 | 3); // Supervisor table; individual NX leaves.
                                       // A cached 2 MiB translation must not preserve write/execute permissions.
                                       // Clearing PGE also invalidates any inherited global translation.
    let cr4: u64;
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    asm!("mov cr4, {}", in(reg) cr4 & !(1 << 7), options(nostack));
    asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    asm!("mov cr4, {}", in(reg) cr4, options(nostack));
    AP_TRAMPOLINE_MAPPED = true;
    Ok(())
}

/// Read an owned kernel identity leaf for AP private-stack admission. No walk
/// of untrusted tables or arbitrary physical memory is performed.
unsafe fn kernel_identity_leaf(address: u64) -> Option<u64> {
    if !address.is_multiple_of(4096) || address < next_abi::kernel::KERNEL_BASE {
        return None;
    }
    let slot = ((address - next_abi::kernel::KERNEL_BASE) / 4096) as usize;
    if slot >= 8 * 512 {
        return None;
    }
    Some(addr_of!(KERNEL_PT).cast::<u64>().add(slot).read())
}
