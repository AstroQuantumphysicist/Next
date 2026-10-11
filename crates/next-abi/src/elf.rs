//! Bounded ELF64 little-endian x86-64 inspection. This is a parser, not an
//! executor: callers still need relocation, mapping, signature and ABI policy.
pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;
pub const MAX_PROGRAM_HEADERS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    Truncated,
    Magic,
    Class,
    Endian,
    Version,
    Architecture,
    Type,
    HeaderSize,
    ProgramHeaders,
    SectionHeaders,
    Overflow,
    SegmentBounds,
    SegmentSize,
    Alignment,
    Address,
    Overlap,
    WritableExecutable,
    Flags,
    Entry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub virtual_address: u64,
    pub physical_address: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub alignment: u64,
}
impl Segment {
    pub fn virtual_end(self) -> Result<u64, ElfError> {
        self.virtual_address
            .checked_add(self.memory_size)
            .ok_or(ElfError::Overflow)
    }
    pub const fn is_load(self) -> bool {
        self.kind == PT_LOAD
    }
}

#[derive(Debug)]
pub struct Elf64<'a> {
    bytes: &'a [u8],
    ph_offset: usize,
    ph_count: usize,
    entry: u64,
    kind: u16,
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, ElfError> {
    Ok(u16::from_le_bytes(
        b.get(i..i.checked_add(2).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::Truncated)?
            .try_into()
            .map_err(|_| ElfError::Truncated)?,
    ))
}
fn u32_at(b: &[u8], i: usize) -> Result<u32, ElfError> {
    Ok(u32::from_le_bytes(
        b.get(i..i.checked_add(4).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::Truncated)?
            .try_into()
            .map_err(|_| ElfError::Truncated)?,
    ))
}
fn u64_at(b: &[u8], i: usize) -> Result<u64, ElfError> {
    Ok(u64::from_le_bytes(
        b.get(i..i.checked_add(8).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::Truncated)?
            .try_into()
            .map_err(|_| ElfError::Truncated)?,
    ))
}
fn canonical(address: u64) -> bool {
    address <= 0x0000_7fff_ffff_ffff || address >= 0xffff_8000_0000_0000
}
fn checked_file_range(offset: u64, len: u64, total: usize) -> Result<(), ElfError> {
    if offset.checked_add(len).ok_or(ElfError::Overflow)? > total as u64 {
        return Err(ElfError::SegmentBounds);
    }
    Ok(())
}

impl<'a> Elf64<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() < 64 {
            return Err(ElfError::Truncated);
        }
        if &bytes[..4] != b"\x7fELF" {
            return Err(ElfError::Magic);
        }
        if bytes[4] != 2 {
            return Err(ElfError::Class);
        }
        if bytes[5] != 1 {
            return Err(ElfError::Endian);
        }
        if bytes[6] != 1 || u32_at(bytes, 20)? != 1 {
            return Err(ElfError::Version);
        }
        if u16_at(bytes, 18)? != 62 {
            return Err(ElfError::Architecture);
        }
        let kind = u16_at(bytes, 16)?;
        if kind != 2 && kind != 3 {
            return Err(ElfError::Type);
        }
        if u16_at(bytes, 52)? != 64 {
            return Err(ElfError::HeaderSize);
        }
        let ph_count = u16_at(bytes, 56)? as usize;
        if ph_count == 0 || ph_count > MAX_PROGRAM_HEADERS || u16_at(bytes, 54)? != 56 {
            return Err(ElfError::ProgramHeaders);
        }
        let ph_offset = usize::try_from(u64_at(bytes, 32)?).map_err(|_| ElfError::Overflow)?;
        if ph_offset < 64
            || ph_offset
                .checked_add(ph_count.checked_mul(56).ok_or(ElfError::Overflow)?)
                .ok_or(ElfError::Overflow)?
                > bytes.len()
        {
            return Err(ElfError::ProgramHeaders);
        }
        // Extended numbering requires section zero semantics and is deliberately
        // unsupported rather than silently interpreted as zero headers.
        let sh_offset = u64_at(bytes, 40)?;
        let sh_count = u16_at(bytes, 60)? as u64;
        let sh_string = u16_at(bytes, 62)? as u64;
        if sh_count == 0 {
            if sh_offset != 0 || sh_string != 0 {
                return Err(ElfError::SectionHeaders);
            }
        } else if sh_offset < 64
            || u16_at(bytes, 58)? != 64
            || sh_string >= sh_count
            || checked_file_range(
                sh_offset,
                sh_count.checked_mul(64).ok_or(ElfError::Overflow)?,
                bytes.len(),
            )
            .is_err()
        {
            return Err(ElfError::SectionHeaders);
        }
        let image = Self {
            bytes,
            ph_offset,
            ph_count,
            entry: u64_at(bytes, 24)?,
            kind,
        };
        let mut entry_mapped = false;
        for i in 0..image.ph_count {
            let segment = image.segment(i).ok_or(ElfError::ProgramHeaders)?;
            checked_file_range(segment.offset, segment.file_size, bytes.len())?;
            if !segment.is_load() {
                continue;
            }
            if segment.file_size > segment.memory_size {
                return Err(ElfError::SegmentSize);
            }
            if segment.flags & !(PF_R | PF_W | PF_X) != 0 {
                return Err(ElfError::Flags);
            }
            if segment.flags & (PF_W | PF_X) == PF_W | PF_X {
                return Err(ElfError::WritableExecutable);
            }
            if segment.alignment > 1
                && (!segment.alignment.is_power_of_two()
                    || segment.virtual_address % segment.alignment
                        != segment.offset % segment.alignment)
            {
                return Err(ElfError::Alignment);
            }
            let end = segment.virtual_end()?;
            if !canonical(segment.virtual_address)
                || (segment.memory_size != 0
                    && (!canonical(end - 1)
                        || (segment.virtual_address >> 47) != ((end - 1) >> 47)))
            {
                return Err(ElfError::Address);
            }
            if segment.memory_size != 0 {
                let page_start = segment.virtual_address & !4095;
                let page_end = end.checked_add(4095).ok_or(ElfError::Overflow)? & !4095;
                for j in 0..i {
                    let other = image.segment(j).ok_or(ElfError::ProgramHeaders)?;
                    if other.is_load() && other.memory_size != 0 {
                        let other_start = other.virtual_address & !4095;
                        let other_end = other
                            .virtual_end()?
                            .checked_add(4095)
                            .ok_or(ElfError::Overflow)?
                            & !4095;
                        // Reject even disjoint segments sharing a page: a loader
                        // cannot accidentally merge executable and writable rights.
                        if page_start < other_end && other_start < page_end {
                            return Err(ElfError::Overlap);
                        }
                    }
                }
                if segment.flags & PF_X != 0
                    && image.entry >= segment.virtual_address
                    && image.entry
                        < segment
                            .virtual_address
                            .checked_add(segment.file_size)
                            .ok_or(ElfError::Overflow)?
                {
                    entry_mapped = true;
                }
            }
        }
        if !entry_mapped {
            return Err(ElfError::Entry);
        }
        Ok(image)
    }
    pub const fn entry(&self) -> u64 {
        self.entry
    }
    pub const fn is_position_independent(&self) -> bool {
        self.kind == 3
    }
    pub const fn segment_count(&self) -> usize {
        self.ph_count
    }
    pub fn segment(&self, index: usize) -> Option<Segment> {
        if index >= self.ph_count {
            return None;
        }
        let at = self.ph_offset.checked_add(index.checked_mul(56)?)?;
        Some(Segment {
            kind: u32_at(self.bytes, at).ok()?,
            flags: u32_at(self.bytes, at + 4).ok()?,
            offset: u64_at(self.bytes, at + 8).ok()?,
            virtual_address: u64_at(self.bytes, at + 16).ok()?,
            physical_address: u64_at(self.bytes, at + 24).ok()?,
            file_size: u64_at(self.bytes, at + 32).ok()?,
            memory_size: u64_at(self.bytes, at + 40).ok()?,
            alignment: u64_at(self.bytes, at + 48).ok()?,
        })
    }
    pub fn segment_data(&self, index: usize) -> Option<&'a [u8]> {
        let segment = self.segment(index)?;
        let start = usize::try_from(segment.offset).ok()?;
        let len = usize::try_from(segment.file_size).ok()?;
        self.bytes.get(start..start.checked_add(len)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write16(b: &mut [u8], at: usize, value: u16) {
        b[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn write32(b: &mut [u8], at: usize, value: u32) {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn write64(b: &mut [u8], at: usize, value: u64) {
        b[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture() -> [u8; 512] {
        let mut b = [0; 512];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        write16(&mut b, 16, 2);
        write16(&mut b, 18, 62);
        write32(&mut b, 20, 1);
        write64(&mut b, 24, 0x400100);
        write64(&mut b, 32, 64);
        write16(&mut b, 52, 64);
        write16(&mut b, 54, 56);
        write16(&mut b, 56, 1);
        write32(&mut b, 64, PT_LOAD);
        write32(&mut b, 68, PF_R | PF_X);
        write64(&mut b, 72, 256);
        write64(&mut b, 80, 0x400100);
        write64(&mut b, 96, 16);
        write64(&mut b, 104, 32);
        write64(&mut b, 112, 4096);
        b
    }
    fn error(b: &[u8]) -> ElfError {
        Elf64::parse(b).unwrap_err()
    }
    #[test]
    fn accepts_valid_image() {
        let b = fixture();
        let elf = Elf64::parse(&b).unwrap();
        assert_eq!(elf.entry(), 0x400100);
        assert_eq!(elf.segment_data(0).unwrap().len(), 16);
        assert!(elf.segment(usize::MAX).is_none());
    }
    #[test]
    fn every_truncated_prefix_is_rejected() {
        let b = fixture();
        for len in 0..272 {
            assert!(Elf64::parse(&b[..len]).is_err());
        }
    }
    #[test]
    fn rejects_wrong_machine_and_headers() {
        let mut b = fixture();
        write16(&mut b, 18, 183);
        assert_eq!(error(&b), ElfError::Architecture);
        let mut b = fixture();
        write64(&mut b, 32, u64::MAX);
        assert!(Elf64::parse(&b).is_err());
        let mut b = fixture();
        write16(&mut b, 56, u16::MAX);
        assert_eq!(error(&b), ElfError::ProgramHeaders);
        let mut b = fixture();
        write64(&mut b, 40, 128);
        assert_eq!(error(&b), ElfError::SectionHeaders);
    }
    #[test]
    fn rejects_segment_overflow_and_invalid_alignment() {
        let mut b = fixture();
        write64(&mut b, 72, u64::MAX);
        assert_eq!(error(&b), ElfError::Overflow);
        let mut b = fixture();
        write64(&mut b, 112, 3);
        assert_eq!(error(&b), ElfError::Alignment);
        let mut b = fixture();
        write64(&mut b, 80, 0x400101);
        assert_eq!(error(&b), ElfError::Alignment);
        let mut b = fixture();
        write64(&mut b, 104, 1);
        assert_eq!(error(&b), ElfError::SegmentSize);
    }
    #[test]
    fn rejects_wx_and_non_executable_or_bss_entry() {
        let mut b = fixture();
        write32(&mut b, 68, PF_R | PF_W | PF_X);
        assert_eq!(error(&b), ElfError::WritableExecutable);
        let mut b = fixture();
        write32(&mut b, 68, PF_R);
        assert_eq!(error(&b), ElfError::Entry);
        let mut b = fixture();
        write64(&mut b, 24, 0x400110);
        assert_eq!(error(&b), ElfError::Entry);
    }
    #[test]
    fn rejects_shared_page_and_noncanonical_addresses() {
        let mut b = fixture();
        write16(&mut b, 56, 2);
        let mut ph = [0; 56];
        ph.copy_from_slice(&b[64..120]);
        b[120..176].copy_from_slice(&ph);
        write64(&mut b, 128, 288);
        write64(&mut b, 136, 0x400120);
        assert_eq!(error(&b), ElfError::Overlap);
        let mut b = fixture();
        write64(&mut b, 80, 0x0000_8000_0000_0100);
        assert_eq!(error(&b), ElfError::Address);
    }
    #[test]
    fn bounded_mutations_never_panic() {
        let valid = fixture();
        for at in 0..valid.len() {
            for value in [0, 1, 0x7f, 0xff] {
                let mut b = valid;
                b[at] = value;
                let _ = Elf64::parse(&b);
            }
        }
    }
}
