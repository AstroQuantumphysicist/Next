#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(offend);
fn offend(_: &rt::StartInfo) -> ! {
    unsafe {
        core::arch::asm!(
            "mov rax, 0x40000ff000; mov rax, [rax]",
            "ud2",
            options(noreturn)
        );
    }
}
