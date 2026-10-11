#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(probe);
static mut INITIALIZED: u64 = 0x1122334455667788;
static mut ZEROED: u64 = 0;
fn probe(info: &rt::StartInfo) -> ! {
    let mut service_bytes = [0xa5; next_abi::service::MAX_BYTES];
    if rt::service_configuration(&mut service_bytes) != rt::Status::AccessDenied as i64
        || service_bytes != [0xa5; next_abi::service::MAX_BYTES]
    {
        rt::exit(210);
    }

    rt::log(info.log, "PROBE: separate native executable running\n");
    unsafe {
        if core::ptr::read_volatile(core::ptr::addr_of!(INITIALIZED)) != 0x1122334455667788
            || core::ptr::read_volatile(core::ptr::addr_of!(ZEROED)) != 0
        {
            rt::exit(111);
        }
        core::ptr::write_volatile(core::ptr::addr_of_mut!(INITIALIZED), 1);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(ZEROED), 2);
    }
    if !rt::context_probe() {
        rt::exit(112);
    }
    rt::log(
        info.log,
        "PROBE: data/BSS and integer/SIMD yield checks passed\n",
    );
    // Native personality cannot select Linux or obtain authority through the
    // CPU's other syscall gate. RCX/R11 are architectural SYSCALL clobbers.
    let mut alternate_gate_result = 39u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") alternate_gate_result,
            lateout("rcx") _, lateout("r11") _);
    }
    if alternate_gate_result as i64 != rt::Status::NotSupported as i64 {
        rt::exit(120);
    }
    let cpu = unsafe { core::arch::x86_64::__cpuid(1) };
    if cpu.ecx & (1 << 27) != 0 {
        rt::exit(121);
    }
    rt::log(info.log, "PROBE: restricted CPU state profile passed\n");
    tls_probe(info);
    let memory = rt::call(rt::Syscall::MemoryMap, [0, 8192, 3, 0, 0, 0]);
    if memory <= 0 {
        rt::exit(132);
    }
    unsafe {
        let bytes = core::slice::from_raw_parts_mut(memory as *mut u8, 8192);
        if bytes.iter().any(|b| *b != 0) {
            rt::exit(133);
        }
        bytes[4095] = 0x12;
        bytes[4096] = 0x34;
        if bytes[4095] != 0x12 || bytes[4096] != 0x34 {
            rt::exit(134);
        }
    }
    if rt::call(
        rt::Syscall::MemoryUnmap,
        [memory as u64 + 1, 4096, 0, 0, 0, 0],
    ) != rt::Status::InvalidArgument as i64
    {
        rt::exit(135);
    }
    if rt::call(rt::Syscall::MemoryUnmap, [memory as u64, 8192, 0, 0, 0, 0]) != 0 {
        rt::exit(136);
    }
    if rt::call(rt::Syscall::MemoryMap, [0, 4096, 7, 0, 0, 0]) != rt::Status::AccessDenied as i64 {
        rt::exit(137);
    }
    rt::log(
        info.log,
        "PROBE: anonymous memory zeroing/range/W^X checks passed\n",
    );
    let bad_pointer = rt::call(rt::Syscall::LogWrite, [info.log, u64::MAX, 32, 0, 0, 0]);
    if bad_pointer != rt::Status::InvalidArgument as i64 {
        rt::exit(101);
    }
    if rt::spawn(0, "/system/spin.elf") != rt::Status::InvalidHandle as i64 {
        rt::exit(102);
    }
    if rt::ready(info.log) != rt::Status::AccessDenied as i64 {
        rt::exit(103);
    }
    let crossing = rt::call(
        rt::Syscall::LogWrite,
        [info.log, next_abi::native::STACK_TOP - 16, 32, 0, 0, 0],
    );
    if crossing != rt::Status::InvalidArgument as i64 {
        rt::exit(118);
    }
    if rt::call(
        rt::Syscall::HandleDuplicate,
        [info.log, 1u64 << 63, 0, 0, 0, 0],
    ) != rt::Status::InvalidArgument as i64
    {
        rt::exit(119);
    }
    let duplicated = rt::call(
        rt::Syscall::HandleDuplicate,
        [info.log, next_abi::Rights::WRITE.0, 0, 0, 0, 0],
    );
    if duplicated <= 0 {
        rt::exit(104);
    }
    if rt::log(
        duplicated as u64,
        "PROBE: bounded user-copy and capability checks passed\n",
    ) < 0
    {
        rt::exit(105);
    }
    if rt::close(duplicated as u64) != 0 {
        rt::exit(106);
    }
    if rt::log(duplicated as u64, "stale handle") != rt::Status::InvalidHandle as i64 {
        rt::exit(107);
    }
    if info.endpoint != 0 {
        let mut malformed = rt::ipc_frame(false);
        malformed[4] = 2;
        if rt::send(info.endpoint, &malformed) != rt::Status::InvalidArgument as i64 {
            rt::exit(113);
        }
        let readonly = rt::call(
            rt::Syscall::HandleDuplicate,
            [info.endpoint, next_abi::Rights::READ.0, 0, 0, 0, 0],
        );
        if readonly <= 0 {
            rt::exit(114);
        }
        if rt::send(readonly as u64, &rt::ipc_frame(false)) != rt::Status::AccessDenied as i64 {
            rt::exit(115);
        }
        rt::close(readonly as u64);
        let bad_copyout = rt::call(
            rt::Syscall::ChannelReceive,
            [info.endpoint, next_abi::native::IMAGE_BASE, 52, 0, 0, 0],
        );
        if bad_copyout != rt::Status::InvalidArgument as i64 {
            rt::exit(116);
        }
        let mut request = [0; 60];
        loop {
            let count = rt::receive(info.endpoint, &mut request);
            if count == rt::Status::WouldBlock as i64 {
                rt::yield_now();
                continue;
            }
            if count != 60
                || request[..28] != rt::ipc_frame(false)[..28]
                || request[28..32] != [2, 0, 0, 0]
                || request[32..44] != rt::ipc_frame(false)[32..44]
            {
                rt::exit(109);
            }
            break;
        }
        let received = u64::from_le_bytes(request[44..52].try_into().unwrap_or([0; 8]));
        if rt::log(received, "forbidden transferred log write") != rt::Status::AccessDenied as i64 {
            rt::exit(124);
        }
        if rt::close(received) != 0 {
            rt::exit(125);
        }
        let file = u64::from_le_bytes(request[52..60].try_into().unwrap_or([0; 8]));
        let readonly_memory = rt::call(rt::Syscall::MemoryMap, [0, 4096, 1, 0, 0, 0]);
        if readonly_memory <= 0 {
            rt::exit(138);
        }
        if rt::call(
            rt::Syscall::FileRead,
            [file, readonly_memory as u64, 32, 0, 0, 0],
        ) != rt::Status::InvalidArgument as i64
        {
            rt::exit(139);
        }
        if rt::call(
            rt::Syscall::MemoryUnmap,
            [readonly_memory as u64, 4096, 0, 0, 0, 0],
        ) != 0
        {
            rt::exit(140);
        }
        let mut content = [0; 64];
        if rt::file_read(file, &mut content) != rt::FILE_PROBE_CONTENT.len() as i64
            || content[..rt::FILE_PROBE_CONTENT.len()] != *rt::FILE_PROBE_CONTENT
        {
            rt::exit(128);
        }
        if rt::file_write(file, b"forbidden") != rt::Status::AccessDenied as i64 {
            rt::exit(129);
        }
        if rt::close(file) != 0 {
            rt::exit(130);
        }
        if rt::file_open(info.log, "/native/forbidden", 3, next_abi::Rights::WRITE)
            != rt::Status::AccessDenied as i64
        {
            rt::exit(131);
        }
        rt::log(
            info.log,
            "PROBE: transferred readonly file rights enforced\n",
        );
        let moved = rt::call(
            rt::Syscall::HandleDuplicate,
            [
                info.log,
                next_abi::Rights::WRITE.union(next_abi::Rights::TRANSFER).0,
                0,
                0,
                0,
                0,
            ],
        );
        if moved <= 0 {
            rt::exit(126);
        }
        if rt::send_with_rights(
            info.endpoint,
            &rt::ipc_frame_with_handle(true, moved as u64),
            &[next_abi::Rights::WRITE.0],
        ) != 0
        {
            rt::exit(110);
        }
        if rt::log(moved as u64, "stale moved capability") != rt::Status::InvalidHandle as i64 {
            rt::exit(127);
        }

        rt::close(info.endpoint);
        if rt::send(info.endpoint, &rt::ipc_frame(false)) != rt::Status::InvalidHandle as i64 {
            rt::exit(117);
        }
        rt::log(
            info.log,
            "PROBE: framed IPC authority/copyout checks passed\n",
        );
    }
    rt::yield_now();
    rt::exit(42)
}

/// Genuine native FS-relative storage, independent of Linux arch_prctl.
fn tls_probe(info: &rt::StartInfo) {
    use rt::sdk;
    const VALUE: u64 = 0x746c_732d_6e65_7874;
    let private = VALUE;
    let base = core::ptr::addr_of!(private) as u64;
    if sdk::tls_fs() != Ok(0) {
        rt::exit(200);
    }
    // This stack object stays alive until FS is explicitly cleared below.
    if unsafe { sdk::set_tls_fs(base) }.is_err() {
        rt::exit(201);
    }
    let check = || {
        let loaded: u64;
        unsafe {
            core::arch::asm!("mov {}, fs:[0]", out(reg) loaded,
            options(nostack, readonly, preserves_flags));
        }
        loaded == VALUE && sdk::tls_fs() == Ok(base)
    };
    if !check() || sdk::yield_now().is_err() || !check() {
        rt::exit(202);
    }
    let start = sdk::clock().unwrap_or_else(|_| rt::exit(203));
    if sdk::sleep_until(start + 10_000_000).is_err() || !check() {
        rt::exit(204);
    }
    // Cross multiple PIT ticks without voluntary yields or sleeping; IRQ
    // scheduling must restore FS before the next actual FS-relative load.
    let timer_start = sdk::clock().unwrap_or_else(|_| rt::exit(205));
    while sdk::clock().unwrap_or_else(|_| rt::exit(205)) < timer_start + 30_000_000 {
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        if !check() {
            rt::exit(206);
        }
    }
    for invalid in [0x10_0000, u64::MAX] {
        if unsafe { sdk::set_tls_fs(invalid) }
            != Err(sdk::Error::Status(rt::Status::InvalidArgument))
            || !check()
        {
            rt::exit(207);
        }
    }
    if rt::call(rt::Syscall::ThreadGetTls, [0, 0, 0, 0, 0, 0]) != rt::Status::InvalidArgument as i64
        || !check()
    {
        rt::exit(208);
    }
    if unsafe { sdk::set_tls_fs(0) }.is_err() || sdk::tls_fs() != Ok(0) {
        rt::exit(209);
    }
    rt::log(
        info.log,
        "PROBE: SDK native TLS FS/private/yield/sleep/timer checks passed\n",
    );
}
