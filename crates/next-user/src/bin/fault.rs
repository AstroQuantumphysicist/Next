#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(fault);
fn fault(info: &rt::StartInfo) -> ! {
    rt::log(info.log, "FAULT: deliberately accessing an unmapped page\n");
    unsafe {
        core::ptr::write_volatile(
            (next_abi::native::IMAGE_BASE + next_abi::native::IMAGE_BYTES as u64) as *mut u64,
            1,
        );
    }
    rt::exit(108)
}
