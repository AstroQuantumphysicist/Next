#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(fault);
fn fault(info: &rt::StartInfo) -> ! {
    rt::log(
        info.log,
        "FAULT-AVX: AVX is outside the FXSAVE process profile\n",
    );
    // VZEROALL. Raw bytes avoid requesting an AVX code-generation profile.
    // OSXSAVE is disabled even when hardware advertises AVX support.
    unsafe {
        core::arch::asm!(".byte 0xc5, 0xfc, 0x77");
    }
    rt::exit(122)
}
