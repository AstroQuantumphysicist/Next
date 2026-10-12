//! Host tests exercise the exact bounded parser linked into Helix.
extern crate self as next_abi;
#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Success = 0,
    InvalidArgument = -1,
    AccessDenied = -2,
    InvalidHandle = -3,
    NotFound = -4,
    NoMemory = -5,
    NotSupported = -6,
    WouldBlock = -7,
    TimedOut = -8,
    BufferTooSmall = -9,
    IoError = -10,
    Faulted = -11,
    PeerClosed = -12,
    Cancelled = -13,
}
#[path = "../crates/next-abi/src/elf.rs"]
pub mod elf;
#[allow(dead_code)]
#[path = "../crates/helix-kernel/src/linux.rs"]
mod linux;
#[path = "../crates/next-abi/src/native.rs"]
pub mod native;
#[path = "../crates/next-abi/src/pe.rs"]
pub mod pe;
#[allow(dead_code)]
mod input {
    pub unsafe fn port_in(_: u16) -> u8 {
        0x20
    }
    pub unsafe fn port_out(_: u16, _: u8) {}
}
const FIXTURE: &[u8] = include_bytes!("../build/compat/linux/hello.elf");
#[test]
fn independent_linux_elf_requires_explicit_profile() {
    let image = linux::LinuxImage::parse(FIXTURE).unwrap();
    assert_eq!(image.entry(), native::IMAGE_BASE);
    assert!(native::NativeImage::parse(FIXTURE).is_err());
    assert_eq!(image.page_flags(0), elf::PF_R | elf::PF_X);
    assert_eq!(image.page_flags(1), elf::PF_R);
    assert_eq!(image.page_flags(2), elf::PF_R | elf::PF_W);
    assert_eq!(image.page_flags(3), 0);
    let mut output = vec![0xcc; native::IMAGE_BYTES];
    image.copy_into(&mut output).unwrap();
    assert_eq!(&output[8192..8200], &0x9876543210abcdefu64.to_le_bytes());
    assert!(output[8200..].iter().all(|&b| b == 0));
    assert!(image.copy_into(&mut output[..4096]).is_err());
}
#[test]
fn unsupported_interpreter_dynamic_and_wrong_machine_reject() {
    for header in [2u32, 3, 4, 0x6474e551] {
        let mut bytes = FIXTURE.to_vec();
        bytes[120..124].copy_from_slice(&header.to_le_bytes());
        assert!(
            linux::LinuxImage::parse(&bytes).is_err(),
            "unsupported header {header}"
        );
    }
    for (offset, value) in [(16, 3u16), (18, 183)] {
        let mut bytes = FIXTURE.to_vec();
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        assert!(linux::LinuxImage::parse(&bytes).is_err());
    }
}
#[test]
fn hostile_mapping_alignment_flags_and_size_reject() {
    let mut bytes = FIXTURE.to_vec();
    bytes[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
    bytes[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
    assert!(linux::LinuxImage::parse(&bytes).is_err());
    let mut bytes = FIXTURE.to_vec();
    bytes[68..72].copy_from_slice(&7u32.to_le_bytes());
    assert!(linux::LinuxImage::parse(&bytes).is_err());
    let mut bytes = FIXTURE.to_vec();
    bytes[112..120].copy_from_slice(&0u64.to_le_bytes());
    assert!(linux::LinuxImage::parse(&bytes).is_err());
    let mut bytes = FIXTURE.to_vec();
    bytes.resize(native::MAX_FILE_BYTES + 1, 0);
    assert!(linux::LinuxImage::parse(&bytes).is_err());
}

#[test]
fn shared_status_errors_use_linux_errno_values() {
    use next_abi::Status;

    assert_eq!(linux::status_errno(Status::InvalidHandle), (-9i64) as u64);
    assert_eq!(
        linux::status_errno(Status::InvalidArgument),
        (-22i64) as u64
    );
    assert_eq!(linux::status_errno(Status::AccessDenied), (-13i64) as u64);
    assert_eq!(linux::status_errno(Status::NotFound), (-2i64) as u64);
    assert_eq!(linux::status_errno(Status::NoMemory), (-12i64) as u64);
    assert_eq!(linux::status_errno(Status::WouldBlock), linux::EAGAIN);
    assert_eq!(linux::EAGAIN, (-11i64) as u64);
    assert_eq!(linux::status_errno(Status::IoError), (-5i64) as u64);
    assert_eq!(linux::status_errno(Status::Cancelled), (-5i64) as u64);
}

#[test]
fn nt_original_shims_resolve_validated_iat_without_writable_code() {
    let bytes = include_bytes!("../build/compat/nt/hello.exe");
    let image = pe::PeImage::parse(bytes).unwrap();
    let imports: Vec<_> = image.imports().collect();
    let executable = linux::Executable::Nt(image);
    assert_eq!(executable.profile(), linux::Profile::Nt);
    let mut output = vec![0xcc; native::IMAGE_BYTES];
    executable.copy_into(&mut output).unwrap();
    let shim = native::IMAGE_BYTES - 16384;
    for import in imports {
        let service = match import.symbol {
            pe::ImportSymbol::GetStdHandle => 0,
            pe::ImportSymbol::WriteFile => 1,
            pe::ImportSymbol::ExitProcess => 2,
        };
        let at = import.iat_rva as usize;
        assert_eq!(
            u64::from_le_bytes(output[at..at + 8].try_into().unwrap()),
            native::IMAGE_BASE + shim as u64 + service * 16
        );
        assert_eq!(executable.page_flags(at / 4096), elf::PF_R | elf::PF_W);
    }
    for service in 0..3 {
        assert_eq!(
            &output[shim + service * 16..shim + service * 16 + 8],
            &[0xb8, (service + 1) as u8, 0, 0, 0, 0xcd, 0x81, 0xc3]
        );
    }
    for page in native::IMAGE_PAGES - 4..native::IMAGE_PAGES {
        assert_eq!(executable.page_flags(page), elf::PF_R | elf::PF_X);
    }
}

#[test]
fn original_pe32plus_binaries_use_only_the_explicit_nt_profile() {
    for bytes in [
        include_bytes!("../build/compat/nt/hello.exe").as_slice(),
        include_bytes!("../build/compat/nt/hello-gate.exe").as_slice(),
    ] {
        let image = pe::PeImage::parse(bytes).unwrap();
        assert_eq!(image.entry(), native::IMAGE_BASE + 4096);
        assert!(native::NativeImage::parse(bytes).is_err());
        assert!(linux::LinuxImage::parse(bytes).is_err());
        assert_eq!(image.page_flags(0), elf::PF_R);
        assert_eq!(image.page_flags(1), elf::PF_R | elf::PF_X);
        assert_eq!(image.page_flags(3), elf::PF_R | elf::PF_W);
        let imports: Vec<_> = image.imports().collect();
        assert_eq!(imports.len(), 3);
        assert_eq!(imports[0].symbol, pe::ImportSymbol::GetStdHandle);
        assert_eq!(imports[1].symbol, pe::ImportSymbol::WriteFile);
        assert_eq!(imports[2].symbol, pe::ImportSymbol::ExitProcess);
        assert!(imports
            .iter()
            .all(|i| i.dll == "kernel32.dll" && i.iat_rva >= 0x3000 && i.iat_rva < 0x4000));
        let mut copied = vec![0xcc; native::IMAGE_BYTES];
        image.copy_into(&mut copied).unwrap();
        assert!(copied[pe::MAX_IMAGE_BYTES..].iter().all(|b| *b == 0));
    }
}
