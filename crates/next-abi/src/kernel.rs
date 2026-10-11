//! Policy and copying for the fixed-address development Helix ELF profile.
//! This is intentionally separate from general ELF inspection: dynamic images,
//! relocation, arbitrary physical addresses and unsupported records fail closed.
use crate::{
    boot::PhysicalRange,
    elf::{Elf64, ElfError, PF_R},
};
pub const KERNEL_BASE: u64 = 0x0400_0000;
pub const MAX_KERNEL_MEMORY: u64 = 16 * 1024 * 1024;
pub const MAX_KERNEL_FILE: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct KernelImage<'a> {
    elf: Elf64<'a>,
    range: PhysicalRange,
}
impl<'a> KernelImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() > MAX_KERNEL_FILE {
            return Err(ElfError::SegmentSize);
        }
        let elf = Elf64::parse(bytes)?;
        if elf.is_position_independent() {
            return Err(ElfError::Type);
        }
        let mut highest = KERNEL_BASE;
        let mut lowest = u64::MAX;
        for i in 0..elf.segment_count() {
            let s = elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            if !s.is_load() {
                return Err(ElfError::ProgramHeaders);
            }
            if s.memory_size == 0 || s.flags & PF_R == 0 {
                return Err(ElfError::SegmentSize);
            }
            if s.physical_address != s.virtual_address || s.virtual_address < KERNEL_BASE {
                return Err(ElfError::Address);
            }
            if !s.virtual_address.is_multiple_of(4096) || s.alignment != 4096 {
                return Err(ElfError::Alignment);
            }
            let end = s
                .virtual_end()?
                .checked_add(4095)
                .ok_or(ElfError::Overflow)?
                & !4095;
            if end > KERNEL_BASE + MAX_KERNEL_MEMORY {
                return Err(ElfError::SegmentSize);
            }
            highest = highest.max(end);
            lowest = lowest.min(s.virtual_address);
        }
        if lowest != KERNEL_BASE {
            return Err(ElfError::Address);
        }
        Ok(Self {
            elf,
            range: PhysicalRange {
                base: KERNEL_BASE,
                byte_len: highest - KERNEL_BASE,
            },
        })
    }
    pub const fn range(&self) -> PhysicalRange {
        self.range
    }
    pub fn entry(&self) -> u64 {
        self.elf.entry()
    }
    /// Zero the entire reserved image span, including BSS and inter-segment gaps.
    /// Destination must describe precisely the loader's exclusive allocation.
    pub fn copy_into(&self, destination: &mut [u8]) -> Result<(), ElfError> {
        if destination.len() as u64 != self.range.byte_len {
            return Err(ElfError::SegmentSize);
        }
        destination.fill(0);
        for i in 0..self.elf.segment_count() {
            let s = self.elf.segment(i).ok_or(ElfError::ProgramHeaders)?;
            let start = (s.virtual_address - self.range.base) as usize;
            let data = self.elf.segment_data(i).ok_or(ElfError::SegmentBounds)?;
            let end = start.checked_add(data.len()).ok_or(ElfError::Overflow)?;
            destination
                .get_mut(start..end)
                .ok_or(ElfError::SegmentBounds)?
                .copy_from_slice(data);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::{PF_X, PT_LOAD};
    fn fixture() -> [u8; 4112] {
        let mut b = [0; 4112];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        for (at, value) in [(16, 2u16), (18, 62), (52, 64), (54, 56), (56, 1)] {
            b[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        for (at, value) in [
            (24, KERNEL_BASE),
            (32, 64),
            (72, 4096),
            (80, KERNEL_BASE),
            (88, KERNEL_BASE),
            (96, 16),
            (104, 32),
            (112, 4096),
        ] {
            b[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        b[64..68].copy_from_slice(&PT_LOAD.to_le_bytes());
        b[68..72].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        b[4096..].fill(0xa5);
        b
    }
    #[test]
    fn copies_only_file_bytes_and_zeroes_bss_and_gaps() {
        let b = fixture();
        let image = KernelImage::parse(&b).unwrap();
        assert_eq!(image.entry(), KERNEL_BASE);
        assert_eq!(image.range().byte_len, 4096);
        let mut destination = [0xff; 4096];
        image.copy_into(&mut destination).unwrap();
        assert_eq!(&destination[..16], &[0xa5; 16]);
        assert!(destination[16..].iter().all(|v| *v == 0));
        let mut short = [0xff; 32];
        assert_eq!(image.copy_into(&mut short), Err(ElfError::SegmentSize));
        assert_eq!(short, [0xff; 32]);
    }
    #[test]
    fn unsupported_load_profiles_are_rejected_before_copy() {
        let b = fixture();
        for (at, value, expected) in [
            (88, KERNEL_BASE + 4096, ElfError::Address),
            (104, MAX_KERNEL_MEMORY + 1, ElfError::SegmentSize),
            (112, 8192, ElfError::Alignment),
        ] {
            let mut changed = b;
            changed[at..at + 8].copy_from_slice(&value.to_le_bytes());
            assert_eq!(KernelImage::parse(&changed).unwrap_err(), expected);
        }
        let mut b = fixture();
        b[16..18].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(KernelImage::parse(&b).unwrap_err(), ElfError::Type);
    }
}
