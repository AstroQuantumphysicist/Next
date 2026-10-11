#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(negative);
fn negative(_: &rt::StartInfo) -> ! {
    // -7 is also the syscall WouldBlock status. Process results travel through
    // a separate output parameter and must remain distinguishable from status.
    rt::exit(-7)
}
