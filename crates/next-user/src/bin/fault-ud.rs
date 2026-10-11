#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(offend);
fn offend(_: &rt::StartInfo) -> ! {
    unsafe {
        core::arch::asm!("ud2", "ud2", options(noreturn));
    }
}
