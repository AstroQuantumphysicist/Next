#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(spin);
fn spin(info: &rt::StartInfo) -> ! {
    rt::log(info.log, "SPIN: running without voluntary yields\n");
    unsafe {
        core::arch::asm!(
            "mov r12, 0x123456789abcdef0",
            "mov r13, r12",
            "movq xmm15, r12",
            "2:",
            "cmp r12, r13",
            "jne 3f",
            "movq r14, xmm15",
            "cmp r14, r13",
            "jne 3f",
            "pause",
            "jmp 2b",
            "3:",
            "ud2",
            options(noreturn)
        );
    }
}
