//! Original, deliberately small Linux x86-64 syscall compatibility profile.
//! Only explicit /compat/linux ELF paths select it. No Linux kernel code is used.
//! AVX/XSAVE are outside the contract: OSXSAVE/FSGSBASE remain disabled.
//! arch_prctl sets owned FS/GS bases; x87/MMX/SSE and TLS state are isolated.
use next_abi::{
    elf::{Elf64, ElfError, PF_R, PF_W, PF_X, PT_LOAD},
    native::{IMAGE_BASE, IMAGE_BYTES, IMAGE_PAGES, MAX_FILE_BYTES},
};
pub const ENOSYS: u64 = (-38i64) as u64;
pub const EFAULT: u64 = (-14i64) as u64;
pub const EBADF: u64 = (-9i64) as u64;
pub const EINVAL: u64 = (-22i64) as u64;
pub const ENOMEM: u64 = (-12i64) as u64;
pub const EPERM: u64 = (-1i64) as u64;
pub const ENODEV: u64 = (-19i64) as u64;
pub const EOVERFLOW: u64 = (-75i64) as u64;
pub const ANON_BASE: u64 = IMAGE_BASE + 0x8_0000;
pub const ANON_PAGES: usize = 64;
pub const ANON_END: u64 = ANON_BASE + ANON_PAGES as u64 * 4096;
pub const ALLOCATED: u8 = 0x80;
pub fn mapping_pages(length: u64) -> Option<usize> {
    let pages = length.checked_add(4095)? / 4096;
    if pages == 0 || pages > ANON_PAGES as u64 {
        None
    } else {
        Some(pages as usize)
    }
}
pub fn first_fit(flags: &[u8; ANON_PAGES], pages: usize, hint: usize) -> Option<usize> {
    if pages == 0 || pages > ANON_PAGES {
        return None;
    }
    if hint <= ANON_PAGES - pages && flags[hint..hint + pages].iter().all(|&f| f == 0) {
        return Some(hint);
    }
    (0..=ANON_PAGES - pages).find(|&start| flags[start..start + pages].iter().all(|&f| f == 0))
}
pub fn mapping_flags(protection: u64) -> Result<u32, u64> {
    if protection & !7 != 0 {
        return Err(EINVAL);
    }
    if protection & 6 == 6 {
        return Err(EPERM);
    }
    if protection == 0 {
        return Ok(0);
    }
    Ok(PF_R
        | if protection & 2 != 0 { PF_W } else { 0 }
        | if protection & 4 != 0 { PF_X } else { 0 })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    Native,
    Linux,
    Nt,
}
// The bounded PE descriptor is kept inline: this early no-allocator loader
// deliberately spends under 512 bytes instead of allocating a box.
#[allow(clippy::large_enum_variant)]
pub enum Executable<'a> {
    Native(next_abi::native::NativeImage<'a>),
    Linux(LinuxImage<'a>),
    Nt(next_abi::pe::PeImage<'a>),
}
impl Executable<'_> {
    pub fn entry(&self) -> u64 {
        match self {
            Self::Native(i) => i.entry(),
            Self::Linux(i) => i.entry(),
            Self::Nt(i) => i.entry(),
        }
    }
    pub fn page_flags(&self, page: usize) -> u32 {
        match self {
            Self::Native(i) => i.page_flags(page),
            Self::Linux(i) => i.page_flags(page),
            Self::Nt(i) => {
                if (IMAGE_PAGES - 4..IMAGE_PAGES).contains(&page) {
                    PF_R | PF_X
                } else {
                    i.page_flags(page)
                }
            }
        }
    }
    pub fn copy_into(&self, output: &mut [u8]) -> Result<(), ElfError> {
        match self {
            Self::Native(i) => i.copy_into(output),
            Self::Linux(i) => i.copy_into(output),
            Self::Nt(i) => {
                use next_abi::pe::ImportSymbol;
                i.copy_into(output).map_err(|_| ElfError::SegmentSize)?;
                let shim_offset = IMAGE_BYTES - 16384;
                for service in 0..3 {
                    let offset = shim_offset + service * 16;
                    // MOV EAX, private_service; INT 81h; RET. The gateway saves
                    // all GPRs and the complete declared x87/SSE state.
                    output[offset..offset + 8].copy_from_slice(&[
                        0xb8,
                        (service + 1) as u8,
                        0,
                        0,
                        0,
                        0xcd,
                        0x81,
                        0xc3,
                    ]);
                }
                for import in i.imports() {
                    let offset = match import.symbol {
                        ImportSymbol::GetStdHandle => 0,
                        ImportSymbol::WriteFile => 16,
                        ImportSymbol::ExitProcess => 32,
                    };
                    let address = IMAGE_BASE + shim_offset as u64 + offset;
                    let iat = import.iat_rva as usize;
                    output[iat..iat + 8].copy_from_slice(&address.to_le_bytes());
                }
                Ok(())
            }
        }
    }
    pub fn memory_end(&self) -> u64 {
        match self {
            Self::Linux(image) => image.memory_end(),
            Self::Native(_) | Self::Nt(_) => IMAGE_BASE + IMAGE_BYTES as u64,
        }
    }
    pub fn profile(&self) -> Profile {
        match self {
            Self::Native(_) => Profile::Native,
            Self::Linux(_) => Profile::Linux,
            Self::Nt(_) => Profile::Nt,
        }
    }
}
pub struct LinuxImage<'a> {
    elf: Elf64<'a>,
}
impl<'a> LinuxImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ElfError::SegmentSize);
        }
        let elf = Elf64::parse(bytes)?;
        if elf.is_position_independent() {
            return Err(ElfError::Type);
        }
        for i in 0..elf.segment_count() {
            let segment = elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            // Reject PT_INTERP, PT_DYNAMIC and every unsupported runtime requirement.
            if segment.kind != PT_LOAD {
                return Err(ElfError::ProgramHeaders);
            }
            if segment.memory_size == 0 || segment.flags & PF_R == 0 {
                return Err(ElfError::SegmentSize);
            }
            if segment.alignment != 4096 || !segment.virtual_address.is_multiple_of(4096) {
                return Err(ElfError::Alignment);
            }
            if segment.virtual_address < IMAGE_BASE
                || segment.virtual_end()? > IMAGE_BASE + IMAGE_BYTES as u64
            {
                return Err(ElfError::Address);
            }
        }
        Ok(Self { elf })
    }
    pub fn entry(&self) -> u64 {
        self.elf.entry()
    }
    pub fn memory_end(&self) -> u64 {
        (0..self.elf.segment_count())
            .filter_map(|index| self.elf.segment(index))
            .filter_map(|segment| segment.virtual_end().ok())
            .max()
            .unwrap_or(IMAGE_BASE)
    }
    pub fn page_flags(&self, page: usize) -> u32 {
        if page >= IMAGE_PAGES {
            return 0;
        }
        let address = IMAGE_BASE + page as u64 * 4096;
        (0..self.elf.segment_count())
            .filter_map(|i| self.elf.segment(i))
            .find(|s| {
                s.is_load()
                    && address >= s.virtual_address
                    && address < s.virtual_end().unwrap_or(0)
            })
            .map_or(0, |s| s.flags & (PF_R | PF_W | PF_X))
    }
    pub fn copy_into(&self, output: &mut [u8]) -> Result<(), ElfError> {
        if output.len() != IMAGE_BYTES {
            return Err(ElfError::SegmentSize);
        }
        output.fill(0);
        for i in 0..self.elf.segment_count() {
            let segment = self.elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            let start = (segment.virtual_address - IMAGE_BASE) as usize;
            let bytes = self.elf.segment_data(i).ok_or(ElfError::SegmentBounds)?;
            output
                .get_mut(start..start + bytes.len())
                .ok_or(ElfError::SegmentBounds)?
                .copy_from_slice(bytes);
        }
        Ok(())
    }
}
/// Bounded raw-byte stdout/stderr sink, including non-UTF8 bytes.
pub fn write_stdout(bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            serial_byte(b'\r')
        }
        serial_byte(byte)
    }
}
fn serial_byte(byte: u8) {
    for _ in 0..10_000 {
        let status = unsafe { crate::input::port_in(0x3fd) };
        if status == 0xff {
            return;
        }
        if status & 0x20 != 0 {
            unsafe { crate::input::port_out(0x3f8, byte) }
            return;
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    #[test]
    fn bounds_overflow_and_write_execute_requests_fail_closed() {
        assert_eq!(mapping_pages(0), None);
        assert_eq!(mapping_pages(u64::MAX), None);
        assert_eq!(mapping_pages(ANON_PAGES as u64 * 4096 + 1), None);
        assert_eq!(mapping_pages(1), Some(1));
        assert_eq!(mapping_pages(4097), Some(2));
        assert_eq!(mapping_flags(7), Err(EPERM));
        assert_eq!(mapping_flags(8), Err(EINVAL));
        assert_eq!(mapping_flags(0), Ok(0));
        assert_eq!(mapping_flags(3), Ok(PF_R | PF_W));
        assert_eq!(mapping_flags(4), Ok(PF_R | PF_X));
    }
    #[test]
    fn occupied_and_fragmented_pages_are_never_reused() {
        let mut flags = [ALLOCATED; ANON_PAGES];
        assert_eq!(first_fit(&flags, 1, 0), None);
        flags[7..10].fill(0);
        flags[25..27].fill(0);
        assert_eq!(first_fit(&flags, 3, 25), Some(7));
        assert_eq!(first_fit(&flags, 2, 25), Some(25));
        assert_eq!(first_fit(&flags, 4, 0), None);
        assert_eq!(first_fit(&flags, 0, 0), None);
        assert_eq!(first_fit(&flags, ANON_PAGES + 1, 0), None);
    }
}
