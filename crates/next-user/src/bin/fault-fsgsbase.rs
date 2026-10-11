#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(fault);
fn fault(info: &rt::StartInfo) -> ! {
    rt::log(
        info.log,
        "FAULT-FSGSBASE: untracked segment-base instructions disabled\n",
    );
    // WRFSBASE RAX. The development context does not support user FS/GS bases;
    // CR4.FSGSBASE must prohibit this instruction on capable CPUs as well.
    unsafe {
        core::arch::asm!(".byte 0xf3, 0x48, 0x0f, 0xae, 0xd0",in("rax")0u64);
    }
    rt::exit(123)
}
