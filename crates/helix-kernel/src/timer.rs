//! Single-bootstrap-CPU PIT/PIC clock. It is neither APIC discovery nor a
//! scheduler. The IRQ path is bounded, allocation-free, and never takes locks.
use super::input::port_out;
use core::{
    arch::{asm, global_asm},
    sync::atomic::{AtomicU64, Ordering},
};
use helix_core::time::TickRate;
pub const PIT_SOURCE_HZ: u32 = 1_193_182;
pub const PIT_DIVISOR: u32 = 11_931;
#[no_mangle]
static HELIX_TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
pub fn ticks() -> u64 {
    HELIX_TIMER_TICKS.load(Ordering::Acquire)
}
pub fn nanoseconds() -> u64 {
    TickRate::new(PIT_SOURCE_HZ, PIT_DIVISOR)
        .unwrap()
        .nanoseconds(ticks())
}
// Preserve the whole interrupted integer and x87/SSE context. The kernel's
// target uses no AVX. RSP is restored from callee-preserved RBX after aligning
// the temporary FXSAVE area and the SysV64 call frame.
global_asm!(
    r#"
.global helix_timer_irq
helix_timer_irq:
    cld
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
    mov rbx, rsp
    and rsp, -16
    sub rsp, 512
    fxsave64 [rsp]
    call helix_timer_tick
    mov rdi, rbx
    call native_on_timer
    fxrstor64 [rsp]
    test rax, rax
    jnz arch_native_unwind
    mov rsp, rbx
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
    iretq
.global helix_timer_context_probe
helix_timer_context_probe:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    sub rsp, 520
    fxsave64 [rsp]
    mov rax, 0x1122334455667788
    movq xmm0, rax
    mov r13, [rip + HELIX_TIMER_TICKS]
    add r13, 3
    mov r14, 50000000
    mov eax, 0x11
    mov ebx, 0x22
    mov ecx, 0x33
    mov edx, 0x44
    mov ebp, 0x55
    mov esi, 0x66
    mov edi, 0x77
    mov r8d, 0x88
    mov r9d, 0x99
    mov r10d, 0xaa
    mov r11d, 0xbb
    mov r15d, 0xff
1:
    mov r12, [rip + HELIX_TIMER_TICKS]
    cmp r12, r13
    jae 2f
    dec r14
    jz 3f
    pause
    jmp 1b
2:
    cmp rax, 0x11
    jne 3f
    cmp rbx, 0x22
    jne 3f
    cmp rcx, 0x33
    jne 3f
    cmp rdx, 0x44
    jne 3f
    cmp rbp, 0x55
    jne 3f
    cmp rsi, 0x66
    jne 3f
    cmp rdi, 0x77
    jne 3f
    cmp r8, 0x88
    jne 3f
    cmp r9, 0x99
    jne 3f
    cmp r10, 0xaa
    jne 3f
    cmp r11, 0xbb
    jne 3f
    cmp r15, 0xff
    jne 3f
    movq r12, xmm0
    mov rax, 0x1122334455667788
    cmp rax, r12
    jne 3f
    xor eax, eax
    jmp 4f
3:
    mov eax, 1
4:
    fxrstor64 [rsp]
    add rsp, 520
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
"#
);
extern "sysv64" {
    pub fn helix_timer_irq();
    fn helix_timer_context_probe() -> u64;
}
#[no_mangle]
extern "sysv64" fn helix_timer_tick() {
    // One writer, IF clear inside the interrupt gate. The separate scheduling
    // callback runs after acknowledgment; no wall-clock state changes ticks.
    HELIX_TIMER_TICKS.store(ticks().saturating_add(1), Ordering::Release);
    unsafe {
        super::apic_irq_hardware::acknowledge_irq0();
        port_out(0x20, 0x20);
    }
}
unsafe fn delay_io() {
    port_out(0x80, 0);
}
/// Enable the VM development timer profile after installing the owned IDT.
/// # Safety
/// Sole boot CPU, IF clear, no remaining firmware calls or ring-3 probes;
/// PIC/PIT ports exclusively owned and IRQ vector 0x20 registered in the IDT.
pub unsafe fn start() -> Result<(), &'static str> {
    let features = core::arch::x86_64::__cpuid(1).edx;
    let cr0: u64;
    let cr4: u64;
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    if features & (1 << 24) == 0 || cr0 & ((1 << 2) | (1 << 3)) != 0 || cr4 & (1 << 9) == 0 {
        return Err("FXSAVE state unsupported by inherited CPU configuration");
    }
    port_out(0x21, 0xff);
    port_out(0xa1, 0xff);
    port_out(0x20, 0x11);
    delay_io();
    port_out(0xa0, 0x11);
    delay_io();
    port_out(0x21, 0x20);
    delay_io();
    port_out(0xa1, 0x28);
    delay_io();
    port_out(0x21, 4);
    delay_io();
    port_out(0xa1, 2);
    delay_io();
    port_out(0x21, 1);
    delay_io();
    port_out(0xa1, 1);
    delay_io();
    port_out(0x21, 0xfe);
    port_out(0xa1, 0xff);
    port_out(0x43, 0x34); // Channel 0, low/high byte, periodic rate generator.
    port_out(0x40, PIT_DIVISOR as u8);
    port_out(0x40, (PIT_DIVISOR >> 8) as u8);
    asm!("sti", options(nostack));
    let start = ticks();
    for _ in 0..100_000_000 {
        if ticks() >= start.saturating_add(3) {
            if helix_timer_context_probe() == 0 {
                return Ok(());
            }
            asm!("cli", options(nostack));
            return Err("timer interrupt corrupted CPU context or stopped advancing");
        }
        core::hint::spin_loop();
    }
    asm!("cli", options(nostack));
    Err("PIT did not deliver three interrupts within bounded startup wait")
}
