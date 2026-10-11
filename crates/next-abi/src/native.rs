//! Native development ELF profile and process startup contract (ABI 0.1).
use crate::elf::{Elf64, ElfError, PF_R, PF_W, PF_X, PT_LOAD};
pub const IMAGE_BASE: u64 = 0x40_0000_0000;
pub const IMAGE_PAGES: usize = 64;
pub const IMAGE_BYTES: usize = IMAGE_PAGES * 4096;
pub const STACK_BASE: u64 = IMAGE_BASE + 0x10_0000;
pub const STACK_PAGES: usize = 16;
pub const STACK_TOP: u64 = STACK_BASE + STACK_PAGES as u64 * 4096;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const NOTE: [u8; 36] = [
    5, 0, 0, 0, 16, 0, 0, 0, 1, 84, 88, 78, 78, 101, 120, 116, 0, 0, 0, 0, 78, 88, 84, 65, 66, 73,
    48, 49, 0, 0, 1, 0, 0, 0, 0, 0,
];
pub const START_MAGIC: u64 = u64::from_le_bytes(*b"NXTSTART");
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StartInfo {
    pub magic: u64,
    pub abi_major: u16,
    pub abi_minor: u16,
    pub size: u32,
    pub pid: u64,
    pub log: u64,
    pub launcher: u64,
    pub endpoint: u64,
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    fn w16(b: &mut [u8], at: usize, n: u16) {
        b[at..at + 2].copy_from_slice(&n.to_le_bytes());
    }
    fn w32(b: &mut [u8], at: usize, n: u32) {
        b[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fn w64(b: &mut [u8], at: usize, n: u64) {
        b[at..at + 8].copy_from_slice(&n.to_le_bytes());
    }
    fn ph(b: &mut [u8], i: usize, kind: u32, flags: u32, layout: [u64; 5]) {
        let [offset, address, file, memory, alignment] = layout;
        let at = 64 + i * 56;
        w32(b, at, kind);
        w32(b, at + 4, flags);
        w64(b, at + 8, offset);
        w64(b, at + 16, address);
        w64(b, at + 24, address);
        w64(b, at + 32, file);
        w64(b, at + 40, memory);
        w64(b, at + 48, alignment);
    }
    fn fixture() -> [u8; 12292] {
        let mut b = [0; 12292];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        w16(&mut b, 16, 2);
        w16(&mut b, 18, 62);
        w32(&mut b, 20, 1);
        w64(&mut b, 24, IMAGE_BASE);
        w64(&mut b, 32, 64);
        w16(&mut b, 52, 64);
        w16(&mut b, 54, 56);
        w16(&mut b, 56, 4);
        ph(
            &mut b,
            0,
            PT_LOAD,
            PF_R | PF_X,
            [4096, IMAGE_BASE, 4, 4096, 4096],
        );
        ph(
            &mut b,
            1,
            PT_LOAD,
            PF_R,
            [8192, IMAGE_BASE + 4096, 40, 4096, 4096],
        );
        ph(
            &mut b,
            2,
            PT_LOAD,
            PF_R | PF_W,
            [12288, IMAGE_BASE + 8192, 4, 8192, 4096],
        );
        ph(&mut b, 3, 4, PF_R, [8192, IMAGE_BASE + 4096, 36, 36, 4]);
        b[4096..4100].copy_from_slice(&[0x31, 0xc0, 0xcd, 0x80]);
        b[8192..8228].copy_from_slice(&NOTE);
        b[12288..].copy_from_slice(b"data");
        b
    }
    fn error(b: &[u8]) -> ElfError {
        NativeImage::parse(b).unwrap_err()
    }
    #[test]
    fn maps_permissions_and_zero_fills_bss_and_unmapped_gaps() {
        let b = fixture();
        let image = NativeImage::parse(&b).unwrap();
        assert_eq!(image.entry(), IMAGE_BASE);
        assert_eq!(image.page_flags(0), PF_R | PF_X);
        assert_eq!(image.page_flags(1), PF_R);
        assert_eq!(image.page_flags(2), PF_R | PF_W);
        assert_eq!(image.page_flags(3), PF_R | PF_W);
        for page in [4, IMAGE_PAGES, usize::MAX] {
            assert_eq!(image.page_flags(page), 0);
        }
        let mut output = [0xa5; IMAGE_BYTES];
        image.copy_into(&mut output).unwrap();
        assert_eq!(&output[..4], &b[4096..4100]);
        assert_eq!(&output[4096..4132], &NOTE);
        assert_eq!(&output[8192..8196], b"data");
        assert!(output[8196..].iter().all(|b| *b == 0));
        assert!(output[4..4096].iter().all(|b| *b == 0));
        assert_eq!(
            image.copy_into(&mut output[..IMAGE_BYTES - 1]),
            Err(ElfError::SegmentSize)
        );
    }
    #[test]
    fn requires_unique_exact_abi_metadata() {
        let mut b = fixture();
        w16(&mut b, 56, 3);
        assert_eq!(error(&b), ElfError::Version);
        let mut b = fixture();
        b[8226] = 1;
        assert_eq!(error(&b), ElfError::Version);
        let mut b = fixture();
        ph(&mut b, 4, 4, PF_R, [8192, IMAGE_BASE + 4096, 36, 36, 4]);
        w16(&mut b, 56, 5);
        assert_eq!(error(&b), ElfError::Version);
        let mut b = fixture();
        w64(&mut b, 64 + 3 * 56 + 32, 35);
        assert_eq!(error(&b), ElfError::Version);
    }
    #[test]
    fn metadata_must_be_file_backed_read_only_and_exactly_mapped() {
        let mut b = fixture();
        w32(&mut b, 64 + 56 + 4, PF_R | PF_W);
        assert_eq!(error(&b), ElfError::Address);
        let mut b = fixture();
        w64(&mut b, 64 + 3 * 56 + 16, IMAGE_BASE + 12288);
        assert_eq!(error(&b), ElfError::Address);
        let mut b = fixture();
        w64(&mut b, 64 + 3 * 56 + 16, IMAGE_BASE + 4100);
        assert_eq!(error(&b), ElfError::Address);
        let mut b = fixture();
        w32(&mut b, 64 + 3 * 56 + 4, PF_R | PF_W);
        assert_eq!(error(&b), ElfError::Flags);
        let mut b = fixture();
        w64(&mut b, 64 + 3 * 56 + 40, 0);
        assert_eq!(error(&b), ElfError::Flags);
        let mut b = fixture();
        w64(&mut b, 64 + 3 * 56 + 48, 1);
        assert_eq!(error(&b), ElfError::Alignment);
        let mut b = fixture();
        w64(&mut b, 64 + 56 + 32, 35);
        assert_eq!(error(&b), ElfError::Address);
    }
    #[test]
    fn rejects_foreign_dynamic_unknown_and_unsafe_load_profiles() {
        let mut b = fixture();
        w16(&mut b, 16, 3);
        assert_eq!(error(&b), ElfError::Type);
        let mut b = fixture();
        w16(&mut b, 18, 183);
        assert_eq!(error(&b), ElfError::Architecture);
        let mut b = fixture();
        w32(&mut b, 64 + 3 * 56, 3);
        assert_eq!(error(&b), ElfError::ProgramHeaders);
        let mut b = fixture();
        w32(&mut b, 68, PF_R | PF_W | PF_X);
        assert_eq!(error(&b), ElfError::WritableExecutable);
        let mut b = fixture();
        w32(&mut b, 68, PF_R | PF_X | 8);
        assert_eq!(error(&b), ElfError::Flags);
        let mut b = fixture();
        w32(&mut b, 68, PF_X);
        assert_eq!(error(&b), ElfError::SegmentSize);
        let mut b = fixture();
        w64(&mut b, 64 + 2 * 56 + 40, 0);
        assert_eq!(error(&b), ElfError::SegmentSize);
    }
    #[test]
    fn rejects_page_overlap_misalignment_window_escape_and_overflow() {
        let mut b = fixture();
        w64(&mut b, 64 + 40, 4097);
        assert_eq!(error(&b), ElfError::Overlap);
        let mut b = fixture();
        w64(&mut b, 64 + 2 * 56 + 16, IMAGE_BASE + 8193);
        assert_eq!(error(&b), ElfError::Alignment);
        let mut b = fixture();
        w64(&mut b, 64 + 2 * 56 + 48, 1);
        assert_eq!(error(&b), ElfError::Alignment);
        let mut b = fixture();
        w64(&mut b, 64 + 2 * 56 + 16, IMAGE_BASE - 4096);
        w64(&mut b, 64 + 2 * 56 + 40, 4096);
        assert_eq!(error(&b), ElfError::Address);
        let mut b = fixture();
        w64(
            &mut b,
            64 + 2 * 56 + 16,
            IMAGE_BASE + IMAGE_BYTES as u64 - 4096,
        );
        assert_eq!(error(&b), ElfError::Address);
        let mut b = fixture();
        w64(&mut b, 64 + 2 * 56 + 40, u64::MAX);
        assert_eq!(error(&b), ElfError::Overflow);
        let mut b = fixture();
        w64(&mut b, 64 + 2 * 56 + 8, u64::MAX);
        assert_eq!(error(&b), ElfError::Overflow);
        let mut b = fixture();
        w64(&mut b, 24, IMAGE_BASE + 8);
        assert_eq!(error(&b), ElfError::Entry);
    }
    #[test]
    fn program_header_order_does_not_change_mapping() {
        let mut b = fixture();
        let mut text = [0; 56];
        text.copy_from_slice(&b[64..120]);
        b.copy_within(232..288, 64);
        b[232..288].copy_from_slice(&text);
        let image = NativeImage::parse(&b).unwrap();
        assert_eq!(image.page_flags(0), PF_R | PF_X);
        assert_eq!(image.page_flags(1), PF_R);
    }
    #[test]
    fn truncated_images_and_bounded_header_mutations_never_panic() {
        let valid = fixture();
        for n in 0..valid.len() {
            assert!(NativeImage::parse(&valid[..n]).is_err());
        }
        for at in (0..344).chain(8192..8228) {
            for value in [0, 1, 0x7f, 0xff] {
                let mut b = valid;
                b[at] = value;
                let _ = NativeImage::parse(&b);
            }
        }
        let b = std::vec![0;MAX_FILE_BYTES+1];
        assert_eq!(error(&b), ElfError::SegmentSize);
    }
    #[test]
    fn startup_layout_and_version_are_explicit() {
        assert_eq!(core::mem::align_of::<StartInfo>(), 8);
        assert_eq!(core::mem::offset_of!(StartInfo, pid), 16);
        assert_eq!(core::mem::offset_of!(StartInfo, log), 24);
        assert_eq!(core::mem::offset_of!(StartInfo, endpoint), 40);
        let valid = StartInfo::new(1, 2, 3, 4);
        assert!(valid.validate());
        assert_eq!(valid.magic.to_le_bytes(), *b"NXTSTART");
        assert!(!StartInfo::default().validate());
        for bad in [
            StartInfo { magic: 0, ..valid },
            StartInfo {
                abi_major: 1,
                ..valid
            },
            StartInfo {
                abi_minor: 2,
                ..valid
            },
            StartInfo { size: 47, ..valid },
            StartInfo { pid: 0, ..valid },
        ] {
            assert!(!bad.validate());
        }
    }
}
const _: () = assert!(core::mem::size_of::<StartInfo>() == 48);
impl StartInfo {
    pub fn new(pid: u64, log: u64, launcher: u64, endpoint: u64) -> Self {
        Self {
            magic: START_MAGIC,
            abi_major: 0,
            abi_minor: 1,
            size: 48,
            pid,
            log,
            launcher,
            endpoint,
        }
    }
    pub fn validate(&self) -> bool {
        self.magic == START_MAGIC
            && self.abi_major == 0
            && self.abi_minor == 1
            && self.size == 48
            && self.pid != 0
    }
}
#[derive(Debug)]
pub struct NativeImage<'a> {
    elf: Elf64<'a>,
}
impl<'a> NativeImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ElfError::SegmentSize);
        }
        let elf = Elf64::parse(bytes)?;
        if elf.is_position_independent() {
            return Err(ElfError::Type);
        }
        let mut metadata = None;
        for i in 0..elf.segment_count() {
            let s = elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            if s.kind == 4 {
                if metadata.is_some() || elf.segment_data(i) != Some(&NOTE[..]) {
                    return Err(ElfError::Version);
                }
                if s.flags != PF_R || s.memory_size != NOTE.len() as u64 {
                    return Err(ElfError::Flags);
                }
                if !matches!(s.alignment, 4 | 8) || !s.virtual_address.is_multiple_of(4) {
                    return Err(ElfError::Alignment);
                }
                metadata = Some(s);
                continue;
            }
            if s.kind != PT_LOAD {
                return Err(ElfError::ProgramHeaders);
            }
            if s.memory_size == 0 || s.flags & PF_R == 0 {
                return Err(ElfError::SegmentSize);
            }
            if !s.virtual_address.is_multiple_of(4096) || s.alignment != 4096 {
                return Err(ElfError::Alignment);
            }
            if s.virtual_address < IMAGE_BASE || s.virtual_end()? > IMAGE_BASE + IMAGE_BYTES as u64
            {
                return Err(ElfError::Address);
            }
        }
        let note = metadata.ok_or(ElfError::Version)?;
        // Metadata must describe bytes that are actually installed in the
        // read-only image, rather than an untrusted, unmapped file appendage.
        let note_end = note.virtual_end()?;
        let loaded_read_only = (0..elf.segment_count())
            .filter_map(|i| elf.segment(i))
            .any(|s| {
                s.is_load()
                    && s.flags == PF_R
                    && note.virtual_address >= s.virtual_address
                    && s.virtual_address
                        .checked_add(s.file_size)
                        .is_some_and(|end| note_end <= end)
                    && s.offset
                        .checked_add(note.virtual_address - s.virtual_address)
                        == Some(note.offset)
            });
        if !loaded_read_only {
            return Err(ElfError::Address);
        }
        Ok(Self { elf })
    }
    pub fn entry(&self) -> u64 {
        self.elf.entry()
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
            let s = self.elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            if !s.is_load() {
                continue;
            }
            let start = (s.virtual_address - IMAGE_BASE) as usize;
            let data = self.elf.segment_data(i).ok_or(ElfError::SegmentBounds)?;
            output
                .get_mut(start..start + data.len())
                .ok_or(ElfError::SegmentBounds)?
                .copy_from_slice(data);
        }
        Ok(())
    }
}
