#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(offend);
fn offend(_: &rt::StartInfo) -> ! {
    unsafe {
        core::arch::asm!("mov rax, 0x4000100000; jmp rax", "ud2", options(noreturn));
    }
}
