#![no_std]
#![no_main]
mod serial;
mod uefi;
use core::{
    arch::{asm, global_asm},
    fmt::{self, Arguments, Write},
    mem::MaybeUninit,
    panic::PanicInfo,
};
use next_abi::boot::{BootInfo, Framebuffer, MemoryMap};
struct Serial;
impl Write for Serial {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        serial::write_serial(text);
        Ok(())
    }
}
macro_rules! log { ($($a:tt)*) => {{ let _ = writeln!(Serial, $($a)*); }}; }
static mut BOOT: MaybeUninit<BootInfo> = MaybeUninit::uninit();
// Firmware ABI passes RCX/RDX; the kernel receives RDI using SysV64 and owns
// its stack. No loader code is called after the jump, and IF remains clear.
global_asm!(
    ".global next_enter_kernel",
    "next_enter_kernel:",
    "cli",
    "cld",
    "mov rdi, rcx",
    "jmp rdx",
);
extern "efiapi" {
    fn next_enter_kernel(boot: *const BootInfo, entry: u64) -> !;
}
/// # Safety
/// Firmware must supply a live image handle and ABI-conforming system table.
#[no_mangle]
pub unsafe extern "efiapi" fn efi_main(image: usize, table: *mut uefi::SystemTable) -> usize {
    serial::init_serial();
    log!("NEXT BOOT development 0.2 | unsigned EFI loader | owner-signed development payloads | Secure Boot/TPM not certified");
    // Complete all file reads, validation and allocations before acquiring the
    // final memory-map key. Do not execute an unvalidated image.
    let deployment = match uefi::load_deployment(image, table) {
        Ok(deployment) => deployment,
        Err(error) => fail(uefi::FailureCode::Deployment, format_args!("{}", error)),
    };
    let slot_name = match deployment.slot {
        next_package::ab::Slot::A => "A",
        next_package::ab::Slot::B => "B",
    };
    let state_origin = if deployment.initialized {
        "initialized"
    } else {
        "persistent"
    };
    log!(
        "NEXT BOOT A/B READY slot={} mode={:?} sequence={} attempts_left={} generation={} state={}",
        slot_name,
        deployment.mode,
        deployment.sequence,
        deployment.attempts_left,
        deployment.image.generation,
        state_origin
    );
    log!("NEXT BOOT: SIGNATURE PASS Ed25519 + SHA-256 owner development key; EFI loader unsigned; Secure Boot/TPM not certified");
    let kernel = match uefi::load_kernel(image, table, deployment.slot, &deployment.kernel) {
        Ok(kernel) => kernel,
        Err(error) => fail(uefi::FailureCode::Kernel, format_args!("{}", error)),
    };
    log!(
        "NEXT BOOT: ELF LOAD PASS entry={:#x} base={:#x} bytes={}",
        kernel.entry,
        kernel.range.base,
        kernel.range.byte_len
    );
    let initrd = match uefi::load_initrd(image, table, deployment.slot, &kernel, &deployment.initrd)
    {
        Ok(range) => range,
        Err(error) => fail(uefi::FailureCode::Initrd, format_args!("{}", error)),
    };
    log!(
        "NEXT BOOT: INITRD LOAD PASS base={:#x} bytes={} CRC archive authenticated by signed deployment",
        initrd.base,
        initrd.byte_len
    );
    let handoff = match uefi::prepare_and_exit(image, table) {
        Ok(h) => h,
        Err(error) => fail(
            uefi::FailureCode::FirmwareHandoff,
            format_args!("{}", error),
        ),
    };
    let mut boot = BootInfo::new();
    boot.framebuffer = Framebuffer {
        base: handoff.framebuffer.base as u64,
        byte_len: handoff.framebuffer.byte_len as u64,
        width: handoff.framebuffer.width,
        height: handoff.framebuffer.height,
        stride: handoff.framebuffer.stride,
        pixel_format: u32::from(handoff.framebuffer.is_bgr),
        reserved: 0,
    };
    boot.memory_map = MemoryMap {
        base: handoff.memory_map as u64,
        byte_len: handoff.memory_map_len as u64,
        descriptor_size: handoff.descriptor_size as u32,
        descriptor_version: handoff.descriptor_version,
        key: 0,
    };
    boot.acpi_rsdp = handoff.rsdp as u64;
    boot.kernel = kernel.range;
    boot.loader = kernel.loader;
    boot.initrd = initrd;
    boot.serial_port = 0x3f8;
    if handoff.ap_trampoline_reserved {
        boot.header.flags |= next_abi::boot::AP_TRAMPOLINE_RESERVED;
        log!("NEXT AP TRAMPOLINE RESERVED base=0x80000 bytes=4096; APs not started");
    } else {
        log!("NEXT AP TRAMPOLINE UNAVAILABLE; BSP retained");
    }
    if handoff.ap_bootstrap_scratch_reserved {
        boot.header.flags |= next_abi::boot::AP_BOOTSTRAP_SCRATCH_RESERVED;
        log!("NEXT AP BOOTSTRAP SCRATCH RESERVED base=0x81000 bytes=8192; APs not started");
    } else {
        log!("NEXT AP BOOTSTRAP SCRATCH UNAVAILABLE; BSP retained");
    }
    boot.seal();
    if let Err(error) = boot.validate() {
        fail(
            uefi::FailureCode::BootRecord,
            format_args!("BootInfo {:?}", error),
        );
    }
    let pointer = core::ptr::addr_of_mut!(BOOT).cast::<BootInfo>();
    pointer.write(boot);
    next_enter_kernel(pointer, kernel.entry)
}
fn halt() -> ! {
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

/// Emit a stable recovery identifier before the human-readable diagnosis, then
/// stop in place. The serial transcript is the loader's available writable log
/// channel; firmware reset loops are intentionally avoided.
fn fail(code: uefi::FailureCode, detail: Arguments<'_>) -> ! {
    let _ = code.write_record(&mut Serial);
    log!("NEXT BOOT FAILED: {}", detail);
    halt()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    fail(uefi::FailureCode::Panic, format_args!("panic {}", info))
}
