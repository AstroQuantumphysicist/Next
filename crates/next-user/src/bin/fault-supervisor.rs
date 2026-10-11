#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(offend);
fn offend(_: &rt::StartInfo) -> ! {
    unsafe {
        core::arch::asm!(
            "mov eax, 0x100000; mov rax, [rax]",
            "ud2",
            options(noreturn)
        );
    }
}
