//! The small subset of UEFI needed to acquire the framebuffer and leave firmware.
//! All boot-service calls happen here; the returned handoff owns its map pages.
use core::{mem, ptr};

type Status = usize;
const SUCCESS: Status = 0;
const ERROR_BIT: Status = 1 << (usize::BITS - 1);
const BUFFER_TOO_SMALL: Status = ERROR_BIT | 5;
const INVALID_PARAMETER: Status = ERROR_BIT | 2;
const NOT_FOUND: Status = ERROR_BIT | 14;
const EFI_VARIABLE_NON_VOLATILE: u32 = 1;
const EFI_VARIABLE_BOOTSERVICE_ACCESS: u32 = 2;
const EFI_VARIABLE_RUNTIME_ACCESS: u32 = 4;
const AB_VARIABLE_ATTRIBUTES: u32 =
    EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS;
const PAGE_SIZE: usize = 4096;
const SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;

#[repr(C)]
struct TableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    header: TableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: usize,
    con_in: usize,
    console_out_handle: usize,
    con_out: usize,
    standard_error_handle: usize,
    std_err: usize,
    runtime_services: usize,
    boot_services: *mut BootServices,
    number_of_table_entries: usize,
    configuration_table: *const ConfigurationTable,
}

type GetVariable =
    unsafe extern "efiapi" fn(*const u16, *const Guid, *mut u32, *mut usize, *mut u8) -> Status;
type SetVariable =
    unsafe extern "efiapi" fn(*const u16, *const Guid, u32, usize, *const u8) -> Status;

/// EFI Runtime Services prefix through SetVariable (UEFI 2.x ABI).
#[repr(C)]
struct RuntimeServices {
    header: TableHeader,
    get_time: usize,
    set_time: usize,
    get_wakeup_time: usize,
    set_wakeup_time: usize,
    set_virtual_address_map: usize,
    convert_pointer: usize,
    get_variable: GetVariable,
    get_next_variable_name: usize,
    set_variable: SetVariable,
}

#[repr(C)]
struct BootServices {
    header: TableHeader,
    raise_tpl: usize,
    restore_tpl: usize,
    allocate_pages: unsafe extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status,
    get_memory_map:
        unsafe extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status,
    allocate_pool: usize,
    free_pool: usize,
    create_event: usize,
    set_timer: usize,
    wait_for_event: usize,
    signal_event: usize,
    close_event: usize,
    check_event: usize,
    install_protocol_interface: usize,
    reinstall_protocol_interface: usize,
    uninstall_protocol_interface: usize,
    handle_protocol: unsafe extern "efiapi" fn(usize, *const Guid, *mut *mut u8) -> Status,
    reserved: usize,
    register_protocol_notify: usize,
    locate_handle: usize,
    locate_device_path: usize,
    install_configuration_table: usize,
    load_image: usize,
    start_image: usize,
    exit: usize,
    unload_image: usize,
    exit_boot_services: unsafe extern "efiapi" fn(usize, usize) -> Status,
    get_next_monotonic_count: usize,
    stall: usize,
    set_watchdog_timer: unsafe extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    connect_controller: usize,
    disconnect_controller: usize,
    open_protocol: usize,
    close_protocol: usize,
    open_protocol_information: usize,
    protocols_per_handle: usize,
    locate_handle_buffer: usize,
    locate_protocol: unsafe extern "efiapi" fn(*const Guid, *mut u8, *mut *mut u8) -> Status,
    install_multiple_protocol_interfaces: usize,
    uninstall_multiple_protocol_interfaces: usize,
    calculate_crc32: usize,
    copy_mem: usize,
    set_mem: usize,
    create_event_ex: usize,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const GOP_GUID: Guid = Guid {
    data1: 0x9042a9de,
    data2: 0x23dc,
    data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};
const ACPI_20_GUID: Guid = Guid {
    data1: 0x8868e871,
    data2: 0xe4f1,
    data3: 0x11d3,
    data4: [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
};
const ACPI_10_GUID: Guid = Guid {
    data1: 0xeb9d2d30,
    data2: 0x2d88,
    data3: 0x11d3,
    data4: [0x9a, 0x16, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d],
};
const NEXT_BOOT_GUID: Guid = Guid {
    data1: 0x5d3a_4e82,
    data2: 0x9c17,
    data3: 0x4f61,
    data4: [0xa1, 0x78, 0x4e, 0x58, 0x41, 0x42, 0x30, 0x31],
};
const AB_STATE_NAMES: [[u16; 17]; 2] = [
    [
        b'N' as u16,
        b'e' as u16,
        b'x' as u16,
        b't' as u16,
        b'B' as u16,
        b'o' as u16,
        b'o' as u16,
        b't' as u16,
        b'S' as u16,
        b't' as u16,
        b'a' as u16,
        b't' as u16,
        b'e' as u16,
        b'0' as u16,
        0,
        0,
        0,
    ],
    [
        b'N' as u16,
        b'e' as u16,
        b'x' as u16,
        b't' as u16,
        b'B' as u16,
        b'o' as u16,
        b'o' as u16,
        b't' as u16,
        b'S' as u16,
        b't' as u16,
        b'a' as u16,
        b't' as u16,
        b'e' as u16,
        b'1' as u16,
        0,
        0,
        0,
    ],
];

#[repr(C)]
struct ConfigurationTable {
    vendor_guid: Guid,
    vendor_table: *const u8,
}
#[repr(C)]
struct GraphicsOutput {
    query_mode: usize,
    set_mode: usize,
    blt: usize,
    mode: *const GraphicsMode,
}
#[repr(C)]
struct GraphicsMode {
    max_mode: u32,
    mode: u32,
    info: *const GraphicsInfo,
    size_of_info: usize,
    frame_buffer_base: u64,
    frame_buffer_size: usize,
}
#[repr(C)]
struct GraphicsInfo {
    version: u32,
    horizontal_resolution: u32,
    vertical_resolution: u32,
    pixel_format: u32,
    pixel_information: [u32; 4],
    pixels_per_scan_line: u32,
}

// Fail compilation rather than calling firmware through a shifted ABI field.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(mem::size_of::<TableHeader>() == 24);
    assert!(mem::size_of::<SystemTable>() == 120);
    assert!(mem::offset_of!(SystemTable, boot_services) == 96);
    assert!(mem::offset_of!(SystemTable, runtime_services) == 88);
    assert!(mem::size_of::<RuntimeServices>() == 96);
    assert!(mem::offset_of!(RuntimeServices, get_variable) == 72);
    assert!(mem::offset_of!(RuntimeServices, set_variable) == 88);
    assert!(mem::size_of::<BootServices>() == 376);
    assert!(mem::offset_of!(BootServices, get_memory_map) == 56);
    assert!(mem::offset_of!(BootServices, exit_boot_services) == 232);
    assert!(mem::offset_of!(BootServices, set_watchdog_timer) == 256);
    assert!(mem::offset_of!(BootServices, locate_protocol) == 320);
    assert!(mem::size_of::<Guid>() == 16);
    assert!(mem::size_of::<GraphicsInfo>() == 36);
    assert!(mem::size_of::<GraphicsMode>() == 40);
};

struct UefiVariableStore<'a> {
    services: &'a RuntimeServices,
}

impl next_package::ab::MetadataStore for UefiVariableStore<'_> {
    fn read_copy(
        &self,
        index: usize,
        out: &mut [u8; next_package::ab::RECORD_BYTES],
    ) -> Result<(), next_package::ab::Error> {
        let name = AB_STATE_NAMES
            .get(index)
            .ok_or(next_package::ab::Error::Storage)?;
        out.fill(0);
        let mut attributes = 0u32;
        let mut size = out.len();
        let status = unsafe {
            (self.services.get_variable)(
                name.as_ptr(),
                &NEXT_BOOT_GUID,
                &mut attributes,
                &mut size,
                out.as_mut_ptr(),
            )
        };
        if status == NOT_FOUND {
            return Ok(());
        }
        if status != SUCCESS || size != out.len() || attributes != AB_VARIABLE_ATTRIBUTES {
            out.fill(0);
            return Err(next_package::ab::Error::Storage);
        }
        Ok(())
    }

    fn write_copy(
        &mut self,
        index: usize,
        bytes: &[u8; next_package::ab::RECORD_BYTES],
    ) -> Result<(), next_package::ab::Error> {
        let name = AB_STATE_NAMES
            .get(index)
            .ok_or(next_package::ab::Error::Storage)?;
        let status = unsafe {
            (self.services.set_variable)(
                name.as_ptr(),
                &NEXT_BOOT_GUID,
                AB_VARIABLE_ATTRIBUTES,
                bytes.len(),
                bytes.as_ptr(),
            )
        };
        if status != SUCCESS {
            return Err(next_package::ab::Error::Storage);
        }
        let mut readback = [0; next_package::ab::RECORD_BYTES];
        self.read_copy(index, &mut readback)?;
        if &readback != bytes {
            return Err(next_package::ab::Error::Storage);
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Framebuffer {
    pub base: usize,
    pub byte_len: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub is_bgr: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Handoff {
    pub framebuffer: Framebuffer,
    pub memory_map: *const u8,
    pub memory_map_len: usize,
    pub descriptor_size: usize,
    pub descriptor_version: u32,
    /// Physical RSDP address, or zero when the firmware supplies no ACPI table.
    pub rsdp: usize,
    pub ap_trampoline_reserved: bool,
    pub ap_bootstrap_scratch_reserved: bool,
}

#[cfg(test)]
impl Handoff {
    pub fn memory_map_count(&self) -> usize {
        self.memory_map_len / self.descriptor_size
    }

    /// RAM represented by conventional, loader, and boot-service memory.
    /// Other regions (MMIO, reserved, ACPI, runtime services) are excluded.
    ///
    /// # Safety
    /// The map fields must still describe the allocation returned by
    /// `prepare_and_exit`; it must not have been reclaimed or overwritten.
    pub unsafe fn ram_bytes(&self) -> u64 {
        let mut bytes = 0u64;
        for index in 0..self.memory_map_count() {
            // UEFI descriptors may be larger than the standard 40-byte prefix.
            // The retained LoaderData pages stay owned by this handoff.
            unsafe {
                let descriptor = self.memory_map.add(index * self.descriptor_size);
                let kind = ptr::read_unaligned(descriptor.cast::<u32>());
                if matches!(kind, 1 | 2 | 3 | 4 | 7) {
                    let pages = ptr::read_unaligned(descriptor.add(24).cast::<u64>());
                    bytes = bytes.saturating_add(pages.saturating_mul(PAGE_SIZE as u64));
                }
            }
        }
        bytes
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Error {
    InvalidSystemTable,
    RuntimeServices,
    Watchdog(Status),
    GraphicsProtocol(Status),
    InvalidFramebuffer,
    UnsupportedPixelFormat,
    MemoryMap(Status),
    InvalidMemoryMap,
    AllocatePages(Status),
    ExitBootServices(Status),
    SizeOverflow,
    ImageProtocol(Status),
    FileSystem(Status),
    FileOpen(Status),
    FileRead(Status),
    FileTooLarge,
    Kernel(next_abi::elf::ElfError),
    KernelAllocation(Status),
    InvalidLoadedImage,
    InitOpen(Status),
    InitRead(Status),
    InitTooLarge,
    InitImage(next_abi::initrd::InitError),
    InitMissingSupervisor,
    DeploymentOpen(Status),
    DeploymentRead(Status),
    DeploymentTooLarge,
    Deployment(next_package::Error),
    AbState(next_package::ab::Error),
    DeploymentArtifacts,
    DeploymentVersion,
    ArtifactDigest,
}

/// Stable machine-readable failure identifiers written to the serial boot log.
/// Keep existing values fixed so external boot-log collectors can classify a
/// failed boot across loader releases. The loader deliberately halts after a
/// failure; it never retries by rebooting into an unvalidated image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum FailureCode {
    Panic = 0x4e58_0001,
    Deployment = 0x4e58_0101,
    Kernel = 0x4e58_0201,
    Initrd = 0x4e58_0301,
    FirmwareHandoff = 0x4e58_0401,
    BootRecord = 0x4e58_0501,
}

impl FailureCode {
    pub const fn value(self) -> u32 {
        self as u32
    }

    pub const fn stage(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Deployment => "deployment",
            Self::Kernel => "kernel",
            Self::Initrd => "initrd",
            Self::FirmwareHandoff => "firmware-handoff",
            Self::BootRecord => "boot-record",
        }
    }

    /// Format the stable, machine-readable recovery record before free-form
    /// diagnostic text. Callers should write this to the best available log
    /// sink before halting.
    pub fn write_record(self, output: &mut impl core::fmt::Write) -> core::fmt::Result {
        core::fmt::write(
            output,
            format_args!(
                "NEXT BOOT FAILURE: code=0x{:08x} stage={} action=HALT\n",
                self.value(),
                self.stage()
            ),
        )
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSystemTable => f.write_str("invalid UEFI system table"),
            Self::RuntimeServices => f.write_str("invalid UEFI runtime services or variable support"),
            Self::Watchdog(status) => write!(f, "disable watchdog: {status:#x}"),
            Self::GraphicsProtocol(status) => write!(f, "locate GOP: {status:#x}"),
            Self::InvalidFramebuffer => f.write_str("invalid GOP framebuffer"),
            Self::UnsupportedPixelFormat => f.write_str("unsupported GOP pixel format"),
            Self::MemoryMap(status) => write!(f, "get memory map: {status:#x}"),
            Self::InvalidMemoryMap => f.write_str("invalid memory map"),
            Self::AllocatePages(status) => write!(f, "allocate map pages: {status:#x}"),
            Self::ExitBootServices(status) => write!(f, "exit boot services: {status:#x}"),
            Self::SizeOverflow => f.write_str("firmware buffer size overflow"),
            Self::ImageProtocol(s) => write!(f, "loaded-image protocol: {s:#x}"),
            Self::FileSystem(s) => write!(f, "boot-device filesystem: {s:#x}"),
            Self::FileOpen(s) => write!(
                f,
                "open \\EFI\\NEXT\\HELIX.ELF: {s:#x}; rebuild or restore boot media"
            ),
            Self::FileRead(s) => write!(f, "read Helix ELF: {s:#x}"),
            Self::FileTooLarge => f.write_str("Helix ELF exceeds bounded 8 MiB file profile"),
            Self::Kernel(e) => write!(
                f,
                "Helix ELF rejected: {e:?}; rebuild or restore boot media"
            ),
            Self::KernelAllocation(s) => write!(f, "reserve Helix load address: {s:#x}"),
            Self::InvalidLoadedImage => f.write_str("invalid loaded-image address or size"),
            Self::InitOpen(s) => write!(
                f,
                "open \\EFI\\NEXT\\INITRD.NXI: {s:#x}; rebuild or restore boot media"
            ),
            Self::InitRead(s) => write!(f, "read init image: {s:#x}"),
            Self::InitTooLarge => f.write_str("init image exceeds bounded 16 MiB profile"),
            Self::InitImage(e) => write!(
                f,
                "init image rejected: {e:?}; rebuild or restore boot media"
            ),
            Self::InitMissingSupervisor => {
                f.write_str("init image missing /system/nextd.elf; rebuild or restore boot media")
            }
            Self::DeploymentOpen(s) => write!(f, "open \\EFI\\NEXT\\SYSTEM.NXP: {s:#x}; rebuild or restore boot media"),
            Self::DeploymentRead(s) => write!(f, "read signed deployment: {s:#x}"),
            Self::DeploymentTooLarge => f.write_str("signed deployment exceeds bounded 16 MiB profile"),
            Self::Deployment(e) => write!(f, "signed deployment rejected: {e:?}; rebuild or restore boot media"),
            Self::AbState(e) => write!(f, "A/B boot metadata rejected: {e:?}; restore trusted recovery media"),
            Self::DeploymentArtifacts => f.write_str("signed deployment has wrong kind or boot artifact set"),
            Self::DeploymentVersion => f.write_str("deployment version cannot be ordered as a bounded generation"),
            Self::ArtifactDigest => f.write_str("boot artifact SHA-256 differs from signed deployment; rebuild or restore boot media"),
        }
    }
}

const LOADED_IMAGE_GUID: Guid = Guid {
    data1: 0x5b1b31a1,
    data2: 0x9562,
    data3: 0x11d2,
    data4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};
const SIMPLE_FS_GUID: Guid = Guid {
    data1: 0x964e5b22,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};
#[repr(C)]
struct LoadedImage {
    revision: u32,
    parent: usize,
    system_table: *mut SystemTable,
    device: usize,
    file_path: usize,
    reserved: usize,
    load_options_size: u32,
    load_options: usize,
    image_base: *const u8,
    image_size: u64,
    code_type: u32,
    data_type: u32,
    unload: usize,
}
#[repr(C)]
struct SimpleFs {
    revision: u64,
    open_volume: unsafe extern "efiapi" fn(*mut SimpleFs, *mut *mut File) -> Status,
}
#[repr(C)]
struct File {
    revision: u64,
    open: unsafe extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    close: unsafe extern "efiapi" fn(*mut File) -> Status,
    delete: usize,
    read: unsafe extern "efiapi" fn(*mut File, *mut usize, *mut u8) -> Status,
    write: usize,
    get_position: usize,
    set_position: usize,
    get_info: usize,
    set_info: usize,
    flush: usize,
}
const _: () = {
    assert!(mem::size_of::<LoadedImage>() == 96);
    assert!(mem::offset_of!(LoadedImage, device) == 24);
    assert!(mem::offset_of!(LoadedImage, image_base) == 64);
    assert!(mem::offset_of!(File, read) == 32);
};
struct OpenFile(*mut File);
impl Drop for OpenFile {
    fn drop(&mut self) {
        unsafe {
            ((*self.0).close)(self.0);
        }
    }
}
struct Pages<'a> {
    services: &'a BootServices,
    address: u64,
    count: usize,
}
impl Drop for Pages<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.services.free_pages)(self.address, self.count);
        }
    }
}
/// Reserve exactly one SIPI-addressable page. Failure retains BSP-only mode.
/// The returned owner must stay live until the final memory map is validated.
unsafe fn reserve_ap_trampoline(services: &BootServices) -> Option<Pages<'_>> {
    let mut address = next_abi::boot::AP_TRAMPOLINE_BASE;
    // AllocateAddress = 2, EfiLoaderData = 2; never write unallocated low RAM.
    if (services.allocate_pages)(2, 2, 1, &mut address) != SUCCESS {
        return None;
    }
    let owner = Pages {
        services,
        address,
        count: 1,
    };
    if address != next_abi::boot::AP_TRAMPOLINE_BASE {
        return None; // Drop frees only the firmware-returned owned allocation.
    }
    Some(owner)
}
/// Two owned pages: private real-mode emergency stacks, then low metadata.
unsafe fn reserve_ap_scratch(services: &BootServices) -> Option<Pages<'_>> {
    let mut address = next_abi::boot::AP_BOOTSTRAP_SCRATCH_BASE;
    if (services.allocate_pages)(2, 2, 2, &mut address) != SUCCESS {
        return None;
    }
    let owner = Pages {
        services,
        address,
        count: 2,
    };
    if address != next_abi::boot::AP_BOOTSTRAP_SCRATCH_BASE {
        return None;
    }
    Some(owner)
}
pub struct LoadedKernel {
    pub entry: u64,
    pub range: next_abi::boot::PhysicalRange,
    pub loader: next_abi::boot::PhysicalRange,
}
/// Owner-configured development trust anchor. This does not authenticate the
/// EFI loader itself and does not claim UEFI Secure Boot or TPM measured boot.
static DEVELOPMENT_KEY: &[u8; 32] = include_bytes!(env!("NEXT_DEV_PUBLIC_KEY"));
pub struct DeploymentDigests {
    pub kernel: [u8; 32],
    pub initrd: [u8; 32],
    pub slot: next_package::ab::Slot,
    pub image: next_package::ab::Image,
    pub sequence: u64,
    pub attempts_left: u8,
    pub mode: next_package::ab::BootMode,
    pub initialized: bool,
}

#[derive(Clone, Copy)]
struct VerifiedSlot {
    image: next_package::ab::Image,
    kernel: [u8; 32],
    initrd: [u8; 32],
}

fn version_generation(version: [u32; 3]) -> Result<u64, Error> {
    if version[0] > u16::MAX as u32 || version[1] > u16::MAX as u32 {
        return Err(Error::DeploymentVersion);
    }
    let generation =
        (u64::from(version[0]) << 48) | (u64::from(version[1]) << 32) | u64::from(version[2]);
    if generation == 0 {
        return Err(Error::DeploymentVersion);
    }
    Ok(generation)
}

fn slot_path(slot: next_package::ab::Slot, leaf: &[u8]) -> [u16; 64] {
    let prefix: &[u8] = match slot {
        next_package::ab::Slot::A => b"\\EFI\\NEXT\\A\\",
        next_package::ab::Slot::B => b"\\EFI\\NEXT\\B\\",
    };
    let mut path = [0u16; 64];
    for (index, byte) in prefix.iter().chain(leaf).enumerate() {
        path[index] = u16::from(*byte);
    }
    path
}

/// Authenticate one slot bundle and bind its exact package version and digest.
/// # Safety
/// `root` and `services` must be live firmware objects during boot services.
unsafe fn load_slot_deployment(
    root: *mut File,
    services: &BootServices,
    slot: next_package::ab::Slot,
) -> Result<VerifiedSlot, Error> {
    use next_package::{PackageKind, TrustAnchor, VerifiedPackage, MAX_BUNDLE_BYTES};
    let name = slot_path(slot, b"SYSTEM.NXP");
    let mut handle = ptr::null_mut();
    let status = ((*root).open)(root, &mut handle, name.as_ptr(), 1, 0);
    if status != SUCCESS || handle.is_null() {
        return Err(Error::DeploymentOpen(status));
    }
    let file = OpenFile(handle);
    let count = MAX_BUNDLE_BYTES / PAGE_SIZE + 1;
    let mut address = 0xffff_ffff;
    let status = (services.allocate_pages)(1, 2, count, &mut address);
    if status != SUCCESS {
        return Err(Error::AllocatePages(status));
    }
    let allocation = Pages {
        services,
        address,
        count,
    };
    if address == 0
        || !address.is_multiple_of(PAGE_SIZE as u64)
        || address
            .checked_add((count * PAGE_SIZE) as u64)
            .is_none_or(|end| end > 1 << 32)
    {
        return Err(Error::InvalidLoadedImage);
    }
    let bytes =
        core::slice::from_raw_parts_mut(allocation.address as *mut u8, MAX_BUNDLE_BYTES + 1);
    let mut total = 0;
    loop {
        let mut size = bytes.len() - total;
        let wanted = size;
        let status = ((*file.0).read)(file.0, &mut size, bytes.as_mut_ptr().add(total));
        if status != SUCCESS {
            return Err(Error::DeploymentRead(status));
        }
        if size > wanted {
            return Err(Error::DeploymentRead(INVALID_PARAMETER));
        }
        total += size;
        if total > MAX_BUNDLE_BYTES {
            return Err(Error::DeploymentTooLarge);
        }
        if size == 0 {
            break;
        }
    }
    let key = *DEVELOPMENT_KEY;
    let anchor = TrustAnchor {
        id: next_package::signer_id(&key),
        key,
        revoked: false,
        allowed_permissions: 0,
        allowed_kinds: 2,
    };
    let package = VerifiedPackage::verify(&bytes[..total], &[anchor]).map_err(Error::Deployment)?;
    if package.kind != PackageKind::Deployment
        || package.len() != 2
        || package.executable().path != "boot/helix.elf"
    {
        return Err(Error::DeploymentArtifacts);
    }
    let kernel = package
        .find("boot/helix.elf")
        .ok_or(Error::DeploymentArtifacts)?
        .digest;
    let initrd = package
        .find("boot/initrd.nxi")
        .ok_or(Error::DeploymentArtifacts)?
        .digest;
    let generation = version_generation(package.version)?;
    Ok(VerifiedSlot {
        image: next_package::ab::Image {
            generation,
            sha256: next_package::sha256(&bytes[..total]),
        },
        kernel,
        initrd,
    })
}
/// Verify the signed package and complete content tree before boot allocations.
/// # Safety
/// Firmware handles and tables must remain valid and boot services live.
pub unsafe fn load_deployment(
    image: usize,
    table: *mut SystemTable,
) -> Result<DeploymentDigests, Error> {
    use next_package::ab::{self, Slot};
    let table = table.as_ref().ok_or(Error::InvalidSystemTable)?;
    if table.header.signature != SYSTEM_TABLE_SIGNATURE || table.boot_services.is_null() {
        return Err(Error::InvalidSystemTable);
    }
    let runtime = (table.runtime_services as *const RuntimeServices)
        .as_ref()
        .ok_or(Error::RuntimeServices)?;
    if runtime.header.signature != 0x5652_4553_544e_5552
        || runtime.header.header_size < mem::size_of::<RuntimeServices>() as u32
    {
        return Err(Error::RuntimeServices);
    }
    let services = &*table.boot_services;
    let status = (services.set_watchdog_timer)(0, 0, 0, ptr::null());
    if status != SUCCESS {
        return Err(Error::Watchdog(status));
    }
    let mut protocol = ptr::null_mut();
    let status = (services.handle_protocol)(image, &LOADED_IMAGE_GUID, &mut protocol);
    if status != SUCCESS {
        return Err(Error::ImageProtocol(status));
    }
    let loaded = protocol
        .cast::<LoadedImage>()
        .as_ref()
        .ok_or(Error::InvalidLoadedImage)?;
    protocol = ptr::null_mut();
    let status = (services.handle_protocol)(loaded.device, &SIMPLE_FS_GUID, &mut protocol);
    if status != SUCCESS {
        return Err(Error::FileSystem(status));
    }
    let fs = protocol.cast::<SimpleFs>();
    if fs.is_null() {
        return Err(Error::FileSystem(INVALID_PARAMETER));
    }
    let mut root = ptr::null_mut();
    let status = ((*fs).open_volume)(fs, &mut root);
    if status != SUCCESS || root.is_null() {
        return Err(Error::FileSystem(status));
    }
    let root = OpenFile(root);
    let slot_a = load_slot_deployment(root.0, services, Slot::A);
    let slot_b = load_slot_deployment(root.0, services, Slot::B);
    let first_error = slot_a
        .as_ref()
        .err()
        .copied()
        .unwrap_or(Error::DeploymentArtifacts);
    let slots = [slot_a.ok(), slot_b.ok()];
    let inventory = [
        slots[0].map(|slot| slot.image),
        slots[1].map(|slot| slot.image),
    ];
    if inventory.iter().all(Option::is_none) {
        return Err(first_error);
    }

    let mut state = UefiVariableStore { services: runtime };
    let (_initial_state, initialized) =
        ab::initialize_from_inventory(&mut state, inventory).map_err(Error::AbState)?;

    #[cfg(feature = "ab-smoke")]
    if initialized {
        let current = ab::read_latest(&state).map_err(Error::AbState)?;
        if current.sequence == 1
            && current.pending.is_none()
            && current.confirmed_slot == Slot::A
            && inventory[1]
                .is_some_and(|candidate| candidate.generation > current.confirmed.generation)
        {
            ab::stage_update(
                &mut state,
                Slot::B,
                inventory[1].ok_or(Error::DeploymentArtifacts)?,
            )
            .map_err(Error::AbState)?;
        }
    }

    let decision = ab::select_boot(&mut state, inventory).map_err(Error::AbState)?;
    let selected = match decision.slot {
        Slot::A => slots[0],
        Slot::B => slots[1],
    }
    .ok_or(Error::DeploymentArtifacts)?;
    if selected.image != decision.image {
        return Err(Error::DeploymentArtifacts);
    }
    Ok(DeploymentDigests {
        kernel: selected.kernel,
        initrd: selected.initrd,
        slot: decision.slot,
        image: decision.image,
        sequence: decision.next_metadata.sequence,
        attempts_left: decision.next_metadata.attempts_left,
        mode: decision.mode,
        initialized,
    })
}

/// Read a bounded init image from the loader's own boot device. The successful
/// allocation remains LoaderData; neither firmware nor the kernel allocator
/// may reuse it while the immutable archive is referenced.
/// # Safety
/// Firmware handles and tables must remain valid and boot services live.
pub unsafe fn load_initrd(
    image: usize,
    table: *mut SystemTable,
    slot: next_package::ab::Slot,
    kernel: &LoadedKernel,
    expected_digest: &[u8; 32],
) -> Result<next_abi::boot::PhysicalRange, Error> {
    use next_abi::{
        boot::PhysicalRange,
        initrd::{InitImage, MAX_IMAGE_SIZE},
    };
    let table = table.as_ref().ok_or(Error::InvalidSystemTable)?;
    if table.header.signature != SYSTEM_TABLE_SIGNATURE || table.boot_services.is_null() {
        return Err(Error::InvalidSystemTable);
    }
    let services = &*table.boot_services;
    let mut protocol = ptr::null_mut();
    let status = (services.handle_protocol)(image, &LOADED_IMAGE_GUID, &mut protocol);
    if status != SUCCESS {
        return Err(Error::ImageProtocol(status));
    }
    let loaded = protocol
        .cast::<LoadedImage>()
        .as_ref()
        .ok_or(Error::InvalidLoadedImage)?;
    protocol = ptr::null_mut();
    let status = (services.handle_protocol)(loaded.device, &SIMPLE_FS_GUID, &mut protocol);
    if status != SUCCESS {
        return Err(Error::FileSystem(status));
    }
    let fs = protocol.cast::<SimpleFs>();
    if fs.is_null() {
        return Err(Error::FileSystem(INVALID_PARAMETER));
    }
    let mut root = ptr::null_mut();
    let status = ((*fs).open_volume)(fs, &mut root);
    if status != SUCCESS || root.is_null() {
        return Err(Error::FileSystem(status));
    }
    let root = OpenFile(root);
    let name = slot_path(slot, b"INITRD.NXI");
    let mut file = ptr::null_mut();
    let status = ((*root.0).open)(root.0, &mut file, name.as_ptr(), 1, 0);
    if status != SUCCESS || file.is_null() {
        return Err(Error::InitOpen(status));
    }
    let file = OpenFile(file);
    let count = MAX_IMAGE_SIZE / PAGE_SIZE + 1;
    let mut address = 0xffff_ffff;
    let status = (services.allocate_pages)(1, 2, count, &mut address);
    if status != SUCCESS {
        return Err(Error::AllocatePages(status));
    }
    let mut allocation = Pages {
        services,
        address,
        count,
    };
    if address == 0
        || !address.is_multiple_of(PAGE_SIZE as u64)
        || address
            .checked_add((count * PAGE_SIZE) as u64)
            .is_none_or(|end| end > 1 << 32)
    {
        return Err(Error::InvalidLoadedImage);
    }
    let bytes = core::slice::from_raw_parts_mut(address as *mut u8, MAX_IMAGE_SIZE + 1);
    let mut total = 0;
    loop {
        let mut size = bytes.len() - total;
        let wanted = size;
        let status = ((*file.0).read)(file.0, &mut size, bytes.as_mut_ptr().add(total));
        if status != SUCCESS {
            return Err(Error::InitRead(status));
        }
        if size > wanted {
            return Err(Error::InitRead(INVALID_PARAMETER));
        }
        total += size;
        if total > MAX_IMAGE_SIZE {
            return Err(Error::InitTooLarge);
        }
        if size == 0 {
            break;
        }
    }
    let archive = InitImage::parse(&bytes[..total]).map_err(Error::InitImage)?;
    if archive.find("/system/nextd.elf").is_none() {
        return Err(Error::InitMissingSupervisor);
    }
    if next_package::sha256(&bytes[..total]) != *expected_digest {
        return Err(Error::ArtifactDigest);
    }
    let retained_pages = total.div_ceil(PAGE_SIZE);
    let retained_end = address + (retained_pages * PAGE_SIZE) as u64;
    for occupied in [kernel.range, kernel.loader] {
        if address < occupied.base + occupied.byte_len && occupied.base < retained_end {
            return Err(Error::InvalidLoadedImage);
        }
    }
    let status = (services.free_pages)(retained_end, count - retained_pages);
    if status != SUCCESS {
        return Err(Error::AllocatePages(status));
    }
    allocation.count = retained_pages;
    // Never expose uninitialized padding to later readers.
    ptr::write_bytes(
        (address as *mut u8).add(total),
        0,
        retained_pages * PAGE_SIZE - total,
    );
    mem::forget(allocation);
    Ok(PhysicalRange {
        base: address,
        byte_len: total as u64,
    })
}
/// Load only from the filesystem of the running EFI image, never a global
/// filesystem search. All temporary allocations and file handles are released
/// while boot services are live; the loaded kernel stays reserved permanently.
/// # Safety
/// Same firmware ownership and validity preconditions as `prepare_and_exit`.
pub unsafe fn load_kernel(
    image: usize,
    table: *mut SystemTable,
    slot: next_package::ab::Slot,
    expected_digest: &[u8; 32],
) -> Result<LoadedKernel, Error> {
    use next_abi::{boot::PhysicalRange, kernel::MAX_KERNEL_FILE};
    let table = table.as_ref().ok_or(Error::InvalidSystemTable)?;
    if table.header.signature != SYSTEM_TABLE_SIGNATURE || table.boot_services.is_null() {
        return Err(Error::InvalidSystemTable);
    }
    let services = &*table.boot_services;
    let status = (services.set_watchdog_timer)(0, 0, 0, ptr::null());
    if status != SUCCESS {
        return Err(Error::Watchdog(status));
    }
    let mut protocol = ptr::null_mut();
    let status = (services.handle_protocol)(image, &LOADED_IMAGE_GUID, &mut protocol);
    if status != SUCCESS {
        return Err(Error::ImageProtocol(status));
    }
    let loaded = protocol
        .cast::<LoadedImage>()
        .as_ref()
        .ok_or(Error::InvalidLoadedImage)?;
    let loader = PhysicalRange {
        base: loaded.image_base as u64,
        byte_len: loaded.image_size,
    };
    if loader.base == 0
        || !loader.base.is_multiple_of(4096)
        || loader.byte_len == 0
        || loader
            .base
            .checked_add(loader.byte_len)
            .is_none_or(|end| end > 1 << 32)
    {
        return Err(Error::InvalidLoadedImage);
    }
    protocol = ptr::null_mut();
    let status = (services.handle_protocol)(loaded.device, &SIMPLE_FS_GUID, &mut protocol);
    if status != SUCCESS {
        return Err(Error::FileSystem(status));
    }
    let fs = protocol.cast::<SimpleFs>();
    if fs.is_null() {
        return Err(Error::FileSystem(INVALID_PARAMETER));
    }
    let mut root = ptr::null_mut();
    let status = ((*fs).open_volume)(fs, &mut root);
    if status != SUCCESS || root.is_null() {
        return Err(Error::FileSystem(status));
    }
    let root = OpenFile(root);
    let name = slot_path(slot, b"HELIX.ELF");
    let mut file = ptr::null_mut();
    let status = ((*root.0).open)(root.0, &mut file, name.as_ptr(), 1, 0);
    if status != SUCCESS || file.is_null() {
        return Err(Error::FileOpen(status));
    }
    let file = OpenFile(file);
    let count = MAX_KERNEL_FILE / PAGE_SIZE + 1;
    let mut address = 0xffff_ffff;
    let status = (services.allocate_pages)(1, 2, count, &mut address);
    if status != SUCCESS {
        return Err(Error::AllocatePages(status));
    }
    let buffer = Pages {
        services,
        address,
        count,
    };
    let bytes = core::slice::from_raw_parts_mut(buffer.address as *mut u8, MAX_KERNEL_FILE + 1);
    let mut total = 0;
    loop {
        let mut size = bytes.len() - total;
        let wanted = size;
        let status = ((*file.0).read)(file.0, &mut size, bytes.as_mut_ptr().add(total));
        if status != SUCCESS {
            return Err(Error::FileRead(status));
        }
        if size > wanted {
            return Err(Error::FileRead(INVALID_PARAMETER));
        }
        total += size;
        if total > MAX_KERNEL_FILE {
            return Err(Error::FileTooLarge);
        }
        if size == 0 {
            break;
        }
    }
    let kernel = parse_kernel_image(&bytes[..total])?;
    if next_package::sha256(&bytes[..total]) != *expected_digest {
        return Err(Error::ArtifactDigest);
    }
    let range = kernel.range();
    if range.base < loader.base + loader.byte_len && loader.base < range.base + range.byte_len {
        return Err(Error::InvalidLoadedImage);
    }
    let mut address = range.base;
    let count = range.byte_len as usize / PAGE_SIZE;
    let status = (services.allocate_pages)(2, 1, count, &mut address);
    if status != SUCCESS {
        return Err(Error::KernelAllocation(status));
    }
    let allocation = Pages {
        services,
        address,
        count,
    };
    if address != range.base {
        return Err(Error::KernelAllocation(INVALID_PARAMETER));
    }
    kernel
        .copy_into(core::slice::from_raw_parts_mut(
            address as *mut u8,
            range.byte_len as usize,
        ))
        .map_err(Error::Kernel)?;
    let entry = kernel.entry();
    mem::forget(allocation);
    Ok(LoadedKernel {
        entry,
        range,
        loader,
    })
}

/// Apply the fixed Helix executable profile at the loader boundary. The shared
/// parser validates ELF structure, segment bounds/alignment/overlap, entry
/// mapping and W^X; this profile additionally rejects every non-PT_LOAD record
/// because this kernel image has no interpreter, dynamic loader or extensions.
fn parse_kernel_image(bytes: &[u8]) -> Result<next_abi::kernel::KernelImage<'_>, Error> {
    next_abi::kernel::KernelImage::parse(bytes).map_err(Error::Kernel)
}

/// Acquire firmware data and exit boot services. After success no firmware
/// console or boot-service function may be called. The map allocation must not
/// be reclaimed until the kernel has copied or finished using the descriptors.
///
/// # Safety
/// `table` and `image` must be the live handles passed to the UEFI entry point.
/// Firmware must provide valid ABI-conforming protocol and configuration data.
pub unsafe fn prepare_and_exit(image: usize, table: *mut SystemTable) -> Result<Handoff, Error> {
    let table = table.as_ref().ok_or(Error::InvalidSystemTable)?;
    if table.header.signature != SYSTEM_TABLE_SIGNATURE || table.boot_services.is_null() {
        return Err(Error::InvalidSystemTable);
    }
    let services = &*table.boot_services;
    let status = (services.set_watchdog_timer)(0, 0, 0, ptr::null());
    if status != SUCCESS {
        return Err(Error::Watchdog(status));
    }
    let framebuffer = framebuffer(services)?;
    let rsdp = acpi_rsdp(table);
    let trampoline = reserve_ap_trampoline(services);
    let scratch = if trampoline.is_some() {
        reserve_ap_scratch(services)
    } else {
        None
    };
    let mut size = 0;
    let mut key = 0;
    let mut descriptor_size = 0;
    let mut descriptor_version = 0;
    let status = (services.get_memory_map)(
        &mut size,
        ptr::null_mut(),
        &mut key,
        &mut descriptor_size,
        &mut descriptor_version,
    );
    if status != BUFFER_TOO_SMALL {
        return Err(Error::MemoryMap(status));
    }
    if descriptor_size < 40 {
        return Err(Error::InvalidMemoryMap);
    }
    let (mut address, pages, mut capacity) = allocate_map(services, size, descriptor_size)?;
    let mut map_owner = Pages {
        services,
        address,
        count: pages,
    };
    for _ in 0..8 {
        size = capacity;
        let status = (services.get_memory_map)(
            &mut size,
            address as *mut u8,
            &mut key,
            &mut descriptor_size,
            &mut descriptor_version,
        );
        if status == BUFFER_TOO_SMALL {
            let (new_address, new_pages, new_capacity) =
                allocate_map(services, size, descriptor_size)?;
            // Free only the previous private allocation, then acquire a fresh map.
            map_owner = Pages {
                services,
                address: new_address,
                count: new_pages,
            };
            address = new_address;
            capacity = new_capacity;
            continue;
        }
        if status != SUCCESS {
            return Err(Error::MemoryMap(status));
        }
        if descriptor_version != 1 || size > capacity {
            return Err(Error::InvalidMemoryMap);
        }
        let regions = next_abi::boot::MemoryRegions::parse(
            core::slice::from_raw_parts(address as *const u8, size),
            descriptor_size,
        )
        .map_err(|_| Error::InvalidMemoryMap)?;
        if (trampoline.is_some() && !regions.ap_trampoline_owned())
            || (scratch.is_some() && !regions.ap_bootstrap_scratch_owned())
        {
            return Err(Error::InvalidMemoryMap);
        }
        // Nothing allocates, prints, or calls another firmware service between
        // this map and ExitBootServices. An invalid key gets a fresh map.
        let status = (services.exit_boot_services)(image, key);
        if status == SUCCESS {
            let ap_trampoline_reserved = trampoline.is_some();
            let ap_bootstrap_scratch_reserved = scratch.is_some();
            // Boot services are gone; keep the page, never invoke its RAII free.
            core::mem::forget(trampoline);
            core::mem::forget(scratch);
            core::mem::forget(map_owner);
            return Ok(Handoff {
                framebuffer,
                memory_map: address as *const u8,
                memory_map_len: size,
                descriptor_size,
                descriptor_version,
                rsdp,
                ap_trampoline_reserved,
                ap_bootstrap_scratch_reserved,
            });
        }
        if status != INVALID_PARAMETER {
            return Err(Error::ExitBootServices(status));
        }
    }
    Err(Error::ExitBootServices(INVALID_PARAMETER))
}

unsafe fn allocate_map(
    services: &BootServices,
    size: usize,
    descriptor_size: usize,
) -> Result<(u64, usize, usize), Error> {
    let slack = descriptor_size.checked_mul(64).ok_or(Error::SizeOverflow)?;
    let wanted = size.checked_add(slack).ok_or(Error::SizeOverflow)?;
    let pages = wanted
        .checked_add(PAGE_SIZE - 1)
        .ok_or(Error::SizeOverflow)?
        / PAGE_SIZE;
    let capacity = pages.checked_mul(PAGE_SIZE).ok_or(Error::SizeOverflow)?;
    let mut address = 0;
    // AllocateAnyPages = 0, EfiLoaderData = 2.
    let status = (services.allocate_pages)(0, 2, pages, &mut address);
    if status != SUCCESS {
        return Err(Error::AllocatePages(status));
    }
    if address == 0 {
        return Err(Error::InvalidMemoryMap);
    }
    Ok((address, pages, capacity))
}

unsafe fn framebuffer(services: &BootServices) -> Result<Framebuffer, Error> {
    let mut protocol = ptr::null_mut();
    let status = (services.locate_protocol)(&GOP_GUID, ptr::null_mut(), &mut protocol);
    if status != SUCCESS {
        return Err(Error::GraphicsProtocol(status));
    }
    let gop = (protocol as *const GraphicsOutput)
        .as_ref()
        .ok_or(Error::InvalidFramebuffer)?;
    let mode = gop.mode.as_ref().ok_or(Error::InvalidFramebuffer)?;
    if mode.size_of_info < mem::size_of::<GraphicsInfo>() {
        return Err(Error::InvalidFramebuffer);
    }
    let info = mode.info.as_ref().ok_or(Error::InvalidFramebuffer)?;
    if info.pixel_format > 1 {
        return Err(Error::UnsupportedPixelFormat);
    }
    let required = (info.pixels_per_scan_line as usize)
        .checked_mul(info.vertical_resolution as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(Error::SizeOverflow)?;
    if mode.frame_buffer_base == 0
        || info.horizontal_resolution == 0
        || info.vertical_resolution == 0
        || info.pixels_per_scan_line < info.horizontal_resolution
        || required > mode.frame_buffer_size
        || mode.frame_buffer_size > isize::MAX as usize
        || (mode.frame_buffer_base as usize)
            .checked_add(mode.frame_buffer_size)
            .is_none()
    {
        return Err(Error::InvalidFramebuffer);
    }
    Ok(Framebuffer {
        base: mode.frame_buffer_base as usize,
        byte_len: mode.frame_buffer_size,
        width: info.horizontal_resolution,
        height: info.vertical_resolution,
        stride: info.pixels_per_scan_line,
        is_bgr: info.pixel_format == 1,
    })
}

unsafe fn acpi_rsdp(table: &SystemTable) -> usize {
    if table.configuration_table.is_null() {
        return 0;
    }
    let mut legacy = 0;
    for index in 0..table.number_of_table_entries {
        let entry = &*table.configuration_table.add(index);
        if entry.vendor_guid == ACPI_20_GUID {
            return entry.vendor_table as usize;
        }
        if entry.vendor_guid == ACPI_10_GUID {
            legacy = entry.vendor_table as usize;
        }
    }
    legacy
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::alloc::{alloc_zeroed, dealloc, Layout};

    static MAP_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn recovery_failure_identifiers_are_stable_and_stage_specific() {
        let failures = [
            (FailureCode::Panic, 0x4e58_0001, "panic"),
            (FailureCode::Deployment, 0x4e58_0101, "deployment"),
            (FailureCode::Kernel, 0x4e58_0201, "kernel"),
            (FailureCode::Initrd, 0x4e58_0301, "initrd"),
            (
                FailureCode::FirmwareHandoff,
                0x4e58_0401,
                "firmware-handoff",
            ),
            (FailureCode::BootRecord, 0x4e58_0501, "boot-record"),
        ];
        for (index, (failure, expected_code, expected_stage)) in failures.iter().enumerate() {
            assert_eq!(failure.value(), *expected_code);
            assert_eq!(failure.stage(), *expected_stage);
            assert!(failures[..index]
                .iter()
                .all(|(previous, _, _)| previous.value() != failure.value()));
            let mut record = std::string::String::new();
            failure.write_record(&mut record).unwrap();
            assert_eq!(
                record,
                std::format!(
                    "NEXT BOOT FAILURE: code=0x{:08x} stage={} action=HALT\n",
                    expected_code,
                    expected_stage
                )
            );
        }
    }

    fn kernel_fixture(program_headers: u16) -> std::vec::Vec<u8> {
        let mut bytes = std::vec![0; 4112];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        for (at, value) in [
            (16, 2u16),
            (18, 62),
            (52, 64),
            (54, 56),
            (56, program_headers),
        ] {
            bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        for (at, value) in [
            (24, 0x0400_0000u64),
            (32, 64u64),
            (72, 4096u64),
            (80, 0x0400_0000u64),
            (88, 0x0400_0000u64),
            (96, 16u64),
            (104, 4096u64),
            (112, 4096u64),
        ] {
            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[64..68].copy_from_slice(&next_abi::elf::PT_LOAD.to_le_bytes());
        bytes[68..72].copy_from_slice(&(next_abi::elf::PF_R | next_abi::elf::PF_X).to_le_bytes());
        bytes[4096..].fill(0xa5);
        bytes
    }

    #[test]
    fn loader_kernel_profile_rejects_unknown_records_and_wx_segments() {
        let mut unknown_record = kernel_fixture(2);
        unknown_record[120..124].copy_from_slice(&3u32.to_le_bytes()); // PT_INTERP
        assert!(matches!(
            parse_kernel_image(&unknown_record),
            Err(Error::Kernel(next_abi::elf::ElfError::ProgramHeaders))
        ));

        let mut writable_executable = kernel_fixture(1);
        writable_executable[68..72].copy_from_slice(
            &(next_abi::elf::PF_R | next_abi::elf::PF_W | next_abi::elf::PF_X).to_le_bytes(),
        );
        assert!(matches!(
            parse_kernel_image(&writable_executable),
            Err(Error::Kernel(next_abi::elf::ElfError::WritableExecutable))
        ));
    }

    unsafe extern "efiapi" fn allocate(
        mode: u32,
        kind: u32,
        pages: usize,
        output: *mut u64,
    ) -> Status {
        assert_eq!(kind, 2);
        if mode == 2 {
            assert_eq!(*output, next_abi::boot::AP_TRAMPOLINE_BASE);
            assert_eq!(pages, 1);
            return INVALID_PARAMETER;
        }
        assert_eq!(mode, 0);
        ALLOC_CALLS.fetch_add(1, Ordering::SeqCst);
        let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
        *output = alloc_zeroed(layout) as u64;
        SUCCESS
    }
    unsafe extern "efiapi" fn free(address: u64, pages: usize) -> Status {
        dealloc(
            address as *mut u8,
            Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap(),
        );
        SUCCESS
    }
    unsafe extern "efiapi" fn map(
        size: *mut usize,
        buffer: *mut u8,
        key: *mut usize,
        stride: *mut usize,
        version: *mut u32,
    ) -> Status {
        let call = MAP_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        *stride = 48;
        *version = 1;
        if buffer.is_null() || *size < 48 {
            *size = 48;
            return BUFFER_TOO_SMALL;
        }
        *size = 48;
        *key = call;
        ptr::write_unaligned(buffer.cast::<u32>(), 7);
        ptr::write_unaligned(buffer.add(24).cast::<u64>(), 1024);
        SUCCESS
    }
    unsafe extern "efiapi" fn exit(image: usize, key: usize) -> Status {
        assert_eq!(image, 17);
        assert_eq!(key, MAP_CALLS.load(Ordering::SeqCst));
        if EXIT_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            INVALID_PARAMETER
        } else {
            SUCCESS
        }
    }
    unsafe extern "efiapi" fn watchdog(timeout: usize, _: u64, _: usize, _: *const u16) -> Status {
        assert_eq!(timeout, 0);
        SUCCESS
    }
    static INFO: GraphicsInfo = GraphicsInfo {
        version: 0,
        horizontal_resolution: 640,
        vertical_resolution: 480,
        pixel_format: 1,
        pixel_information: [0; 4],
        pixels_per_scan_line: 640,
    };
    static mut MODE: GraphicsMode = GraphicsMode {
        max_mode: 1,
        mode: 0,
        info: &INFO,
        size_of_info: mem::size_of::<GraphicsInfo>(),
        frame_buffer_base: 0x100000,
        frame_buffer_size: 640 * 480 * 4,
    };
    static mut GOP: GraphicsOutput = GraphicsOutput {
        query_mode: 0,
        set_mode: 0,
        blt: 0,
        mode: ptr::addr_of!(MODE),
    };
    unsafe extern "efiapi" fn locate(
        guid: *const Guid,
        _: *mut u8,
        output: *mut *mut u8,
    ) -> Status {
        assert!(*guid == GOP_GUID);
        *output = ptr::addr_of_mut!(GOP).cast();
        SUCCESS
    }
    fn header(signature: u64) -> TableHeader {
        TableHeader {
            signature,
            revision: 0x20000,
            header_size: 0,
            crc32: 0,
            reserved: 0,
        }
    }
    fn services() -> BootServices {
        BootServices {
            header: header(0),
            raise_tpl: 0,
            restore_tpl: 0,
            allocate_pages: allocate,
            free_pages: free,
            get_memory_map: map,
            allocate_pool: 0,
            free_pool: 0,
            create_event: 0,
            set_timer: 0,
            wait_for_event: 0,
            signal_event: 0,
            close_event: 0,
            check_event: 0,
            install_protocol_interface: 0,
            reinstall_protocol_interface: 0,
            uninstall_protocol_interface: 0,
            handle_protocol: unsupported_protocol,
            reserved: 0,
            register_protocol_notify: 0,
            locate_handle: 0,
            locate_device_path: 0,
            install_configuration_table: 0,
            load_image: 0,
            start_image: 0,
            exit: 0,
            unload_image: 0,
            exit_boot_services: exit,
            get_next_monotonic_count: 0,
            stall: 0,
            set_watchdog_timer: watchdog,
            connect_controller: 0,
            disconnect_controller: 0,
            open_protocol: 0,
            close_protocol: 0,
            open_protocol_information: 0,
            protocols_per_handle: 0,
            locate_handle_buffer: 0,
            locate_protocol: locate,
            install_multiple_protocol_interfaces: 0,
            uninstall_multiple_protocol_interfaces: 0,
            calculate_crc32: 0,
            copy_mem: 0,
            set_mem: 0,
            create_event_ex: 0,
        }
    }
    unsafe extern "efiapi" fn unsupported_protocol(
        _: usize,
        _: *const Guid,
        _: *mut *mut u8,
    ) -> Status {
        INVALID_PARAMETER
    }

    #[test]
    fn handoff_reacquires_map_after_rejected_exit_key() {
        let mut services = services();
        let entries = [
            ConfigurationTable {
                vendor_guid: ACPI_10_GUID,
                vendor_table: 0x1000 as *const u8,
            },
            ConfigurationTable {
                vendor_guid: ACPI_20_GUID,
                vendor_table: 0x2000 as *const u8,
            },
        ];
        let mut table = SystemTable {
            header: header(SYSTEM_TABLE_SIGNATURE),
            firmware_vendor: ptr::null(),
            firmware_revision: 0,
            console_in_handle: 0,
            con_in: 0,
            console_out_handle: 0,
            con_out: 0,
            standard_error_handle: 0,
            std_err: 0,
            runtime_services: 0,
            boot_services: &mut services,
            number_of_table_entries: entries.len(),
            configuration_table: entries.as_ptr(),
        };
        let handoff = unsafe { prepare_and_exit(17, &mut table) }.unwrap();
        assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(MAP_CALLS.load(Ordering::SeqCst), 3);
        assert_eq!(ALLOC_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(handoff.rsdp, 0x2000);
        assert!(!handoff.ap_trampoline_reserved);
        assert!(handoff.framebuffer.is_bgr);
        assert_eq!(handoff.memory_map_count(), 1);
        assert_eq!(unsafe { handoff.ram_bytes() }, 4 * 1024 * 1024);
        unsafe {
            free(handoff.memory_map as u64, 1);
        }
    }

    #[test]
    fn trampoline_allocation_is_exact_owned_and_failure_is_optional() {
        static RETURN_MODE: AtomicUsize = AtomicUsize::new(0);
        static FREED: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "efiapi" fn fixed(
            mode: u32,
            kind: u32,
            pages: usize,
            out: *mut u64,
        ) -> Status {
            assert_eq!((mode, kind, pages), (2, 2, 1));
            assert_eq!(*out, next_abi::boot::AP_TRAMPOLINE_BASE);
            match RETURN_MODE.load(Ordering::SeqCst) {
                0 => SUCCESS,
                1 => INVALID_PARAMETER,
                _ => {
                    *out += 4096;
                    SUCCESS
                }
            }
        }
        unsafe extern "efiapi" fn release(address: u64, pages: usize) -> Status {
            assert_eq!(pages, 1);
            FREED.store(address as usize, Ordering::SeqCst);
            SUCCESS
        }
        let mut mock = services();
        mock.allocate_pages = fixed;
        mock.free_pages = release;
        let owner = unsafe { reserve_ap_trampoline(&mock) }.unwrap();
        assert_eq!(owner.address, next_abi::boot::AP_TRAMPOLINE_BASE);
        assert_eq!(FREED.load(Ordering::SeqCst), 0);
        drop(owner);
        assert_eq!(FREED.load(Ordering::SeqCst), 0x80000);
        FREED.store(0, Ordering::SeqCst);
        RETURN_MODE.store(1, Ordering::SeqCst);
        assert!(unsafe { reserve_ap_trampoline(&mock) }.is_none());
        assert_eq!(FREED.load(Ordering::SeqCst), 0);
        RETURN_MODE.store(2, Ordering::SeqCst);
        assert!(unsafe { reserve_ap_trampoline(&mock) }.is_none());
        assert_eq!(FREED.load(Ordering::SeqCst), 0x81000);
    }

    #[test]
    fn failed_handoffs_release_reserved_page_and_private_map() {
        static STAGE: AtomicUsize = AtomicUsize::new(0);
        static FREES: AtomicUsize = AtomicUsize::new(0);
        static EXITS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "efiapi" fn own(mode: u32, kind: u32, pages: usize, out: *mut u64) -> Status {
            assert_eq!(kind, 2);
            if mode == 2 {
                if *out == next_abi::boot::AP_BOOTSTRAP_SCRATCH_BASE {
                    assert_eq!(pages, 2);
                    return INVALID_PARAMETER;
                }
                assert_eq!((pages, *out), (1, next_abi::boot::AP_TRAMPOLINE_BASE));
            } else {
                assert_eq!(mode, 0);
                if STAGE.load(Ordering::SeqCst) == 5 {
                    return INVALID_PARAMETER;
                }
                *out = alloc_zeroed(Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap())
                    as u64;
            }
            SUCCESS
        }
        unsafe extern "efiapi" fn release(address: u64, pages: usize) -> Status {
            if address == next_abi::boot::AP_TRAMPOLINE_BASE {
                assert_eq!(pages, 1);
                FREES.fetch_or(1, Ordering::SeqCst);
            } else {
                dealloc(
                    address as *mut u8,
                    Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap(),
                );
                FREES.fetch_or(2, Ordering::SeqCst);
            }
            SUCCESS
        }
        unsafe extern "efiapi" fn final_map(
            size: *mut usize,
            buffer: *mut u8,
            key: *mut usize,
            stride: *mut usize,
            version: *mut u32,
        ) -> Status {
            *stride = 40;
            *version = 1;
            if buffer.is_null() {
                if STAGE.load(Ordering::SeqCst) == 4 {
                    return INVALID_PARAMETER;
                }
                *size = 40;
                return BUFFER_TOO_SMALL;
            }
            let stage = STAGE.load(Ordering::SeqCst);
            if stage == 0 {
                return INVALID_PARAMETER;
            }
            *size = 40;
            *key = 1;
            ptr::write_bytes(buffer, 0, 40);
            ptr::write_unaligned(buffer.cast::<u32>(), if stage == 1 { 7 } else { 2 });
            ptr::write_unaligned(
                buffer.add(8).cast::<u64>(),
                next_abi::boot::AP_TRAMPOLINE_BASE,
            );
            ptr::write_unaligned(buffer.add(24).cast::<u64>(), 1);
            SUCCESS
        }
        unsafe extern "efiapi" fn failed_exit(_: usize, _: usize) -> Status {
            EXITS.fetch_add(1, Ordering::SeqCst);
            if STAGE.load(Ordering::SeqCst) == 3 {
                INVALID_PARAMETER
            } else {
                BUFFER_TOO_SMALL
            }
        }
        for stage in 0..6 {
            STAGE.store(stage, Ordering::SeqCst);
            FREES.store(0, Ordering::SeqCst);
            EXITS.store(0, Ordering::SeqCst);
            let mut mock = services();
            mock.allocate_pages = own;
            mock.free_pages = release;
            mock.get_memory_map = final_map;
            mock.exit_boot_services = failed_exit;
            let mut table = SystemTable {
                header: header(SYSTEM_TABLE_SIGNATURE),
                firmware_vendor: ptr::null(),
                firmware_revision: 0,
                console_in_handle: 0,
                con_in: 0,
                console_out_handle: 0,
                con_out: 0,
                standard_error_handle: 0,
                std_err: 0,
                runtime_services: 0,
                boot_services: &mut mock,
                number_of_table_entries: 0,
                configuration_table: ptr::null(),
            };
            let result = unsafe { prepare_and_exit(17, &mut table) };
            assert!(result.is_err());
            assert_eq!(
                FREES.load(Ordering::SeqCst),
                if stage >= 4 { 1 } else { 3 },
                "stage {stage} must release every acquired allocation"
            );
            assert_eq!(
                EXITS.load(Ordering::SeqCst),
                match stage {
                    0 | 1 => 0,
                    2 => 1,
                    3 => 8,
                    _ => 0,
                }
            );
        }
    }

    #[test]
    fn paired_low_reservations_retain_only_actual_owned_allocations() {
        static CASE: AtomicUsize = AtomicUsize::new(0);
        static FREES: AtomicUsize = AtomicUsize::new(0);
        static SCRATCH_CALLS: AtomicUsize = AtomicUsize::new(0);
        static EXITS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "efiapi" fn own(mode: u32, kind: u32, pages: usize, out: *mut u64) -> Status {
            assert_eq!(kind, 2);
            if mode == 2 {
                let case = CASE.load(Ordering::SeqCst);
                if *out == 0x80000 {
                    assert_eq!(pages, 1);
                    if case == 2 {
                        return INVALID_PARAMETER;
                    }
                } else {
                    assert_eq!((*out, pages), (0x81000, 2));
                    SCRATCH_CALLS.fetch_add(1, Ordering::SeqCst);
                    if case == 1 {
                        return INVALID_PARAMETER;
                    }
                    if case == 3 {
                        *out = 0x83000;
                    }
                }
            } else {
                assert_eq!(mode, 0);
                *out = alloc_zeroed(Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap())
                    as u64;
            }
            SUCCESS
        }
        unsafe extern "efiapi" fn release(address: u64, pages: usize) -> Status {
            let bit = match address {
                0x80000 => {
                    assert_eq!(pages, 1);
                    1
                }
                0x81000 | 0x83000 => {
                    assert_eq!(pages, 2);
                    4
                }
                _ => {
                    dealloc(
                        address as *mut u8,
                        Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap(),
                    );
                    2
                }
            };
            FREES.fetch_or(bit, Ordering::SeqCst);
            SUCCESS
        }
        unsafe extern "efiapi" fn final_map(
            size: *mut usize,
            buffer: *mut u8,
            key: *mut usize,
            stride: *mut usize,
            version: *mut u32,
        ) -> Status {
            *stride = 40;
            *version = 1;
            if buffer.is_null() {
                *size = 40;
                return BUFFER_TOO_SMALL;
            }
            *size = 40;
            *key = 1;
            ptr::write_bytes(buffer, 0, 40);
            ptr::write_unaligned(buffer.cast::<u32>(), 2);
            ptr::write_unaligned(buffer.add(8).cast::<u64>(), 0x80000);
            let pages = if CASE.load(Ordering::SeqCst) == 0 {
                3
            } else {
                1
            };
            ptr::write_unaligned(buffer.add(24).cast::<u64>(), pages);
            SUCCESS
        }
        unsafe extern "efiapi" fn done(_: usize, _: usize) -> Status {
            EXITS.fetch_add(1, Ordering::SeqCst);
            SUCCESS
        }
        for case in 0..5 {
            CASE.store(case, Ordering::SeqCst);
            FREES.store(0, Ordering::SeqCst);
            SCRATCH_CALLS.store(0, Ordering::SeqCst);
            EXITS.store(0, Ordering::SeqCst);
            let mut mock = services();
            mock.allocate_pages = own;
            mock.free_pages = release;
            mock.get_memory_map = final_map;
            mock.exit_boot_services = done;
            let mut table = SystemTable {
                header: header(SYSTEM_TABLE_SIGNATURE),
                firmware_vendor: ptr::null(),
                firmware_revision: 0,
                console_in_handle: 0,
                con_in: 0,
                console_out_handle: 0,
                con_out: 0,
                standard_error_handle: 0,
                std_err: 0,
                runtime_services: 0,
                boot_services: &mut mock,
                number_of_table_entries: 0,
                configuration_table: ptr::null(),
            };
            let result = unsafe { prepare_and_exit(17, &mut table) };
            if case == 4 {
                assert!(matches!(result, Err(Error::InvalidMemoryMap)));
                assert_eq!(FREES.load(Ordering::SeqCst), 7);
                assert_eq!(EXITS.load(Ordering::SeqCst), 0);
            } else {
                let handoff = result.unwrap();
                assert_eq!(handoff.ap_trampoline_reserved, case != 2);
                assert_eq!(handoff.ap_bootstrap_scratch_reserved, case == 0);
                assert_eq!(EXITS.load(Ordering::SeqCst), 1);
                assert_eq!(FREES.load(Ordering::SeqCst), if case == 3 { 4 } else { 0 });
                unsafe {
                    release(handoff.memory_map as u64, 1);
                }
            }
            assert_eq!(SCRATCH_CALLS.load(Ordering::SeqCst), usize::from(case != 2));
        }
    }

    #[test]
    fn malformed_system_table_and_overflow_are_rejected() {
        assert!(matches!(
            unsafe { prepare_and_exit(0, ptr::null_mut()) },
            Err(Error::InvalidSystemTable)
        ));
        assert!(matches!(
            unsafe { allocate_map(&services(), usize::MAX, 40) },
            Err(Error::SizeOverflow)
        ));
    }

    #[test]
    fn descriptor_ram_accounting_ignores_mmio_and_honors_stride() {
        let mut descriptors = [0u8; 96];
        descriptors[..4].copy_from_slice(&7u32.to_le_bytes());
        descriptors[24..32].copy_from_slice(&5u64.to_le_bytes());
        descriptors[48..52].copy_from_slice(&11u32.to_le_bytes());
        descriptors[72..80].copy_from_slice(&999u64.to_le_bytes());
        let handoff = Handoff {
            framebuffer: Framebuffer {
                base: 0,
                byte_len: 0,
                width: 0,
                height: 0,
                stride: 0,
                is_bgr: false,
            },
            memory_map: descriptors.as_ptr(),
            memory_map_len: descriptors.len(),
            descriptor_size: 48,
            descriptor_version: 1,
            rsdp: 0,
            ap_trampoline_reserved: false,
            ap_bootstrap_scratch_reserved: false,
        };
        assert_eq!(handoff.memory_map_count(), 2);
        assert_eq!(unsafe { handoff.ram_bytes() }, 5 * 4096);
    }
}
