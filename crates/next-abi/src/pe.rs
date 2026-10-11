//! Bounded PE32+ AMD64 development profile. This is not complete Windows/NT.
//! Fixed preferred base, no relocations/TLS/CLR/unwind/delay imports, console
//! executables only. Import resolution is a closed original shim allowlist.
use crate::{
    elf::{PF_R, PF_W, PF_X},
    native::{IMAGE_BASE, IMAGE_BYTES, IMAGE_PAGES, STACK_PAGES},
};
pub const SHIM_BYTES: usize = 16384;
pub const MAX_IMAGE_BYTES: usize = IMAGE_BYTES - SHIM_BYTES;
pub const MAX_SECTIONS: usize = 16;
pub const MAX_IMPORTS: usize = 3;
const FILE_ALIGN: usize = 512;
const PAGE: usize = 4096;
const SECTION_READ: u32 = 0x4000_0000;
const SECTION_WRITE: u32 = 0x8000_0000;
const SECTION_EXECUTE: u32 = 0x2000_0000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeError {
    Truncated,
    Magic,
    Architecture,
    Header,
    Sections,
    Alignment,
    Size,
    Overflow,
    Address,
    Overlap,
    WritableExecutable,
    Entry,
    Flags,
    Subsystem,
    UnsupportedDirectory(u8),
    Import,
    UnsupportedImport,
    OrdinalImport,
    Iat,
    Stack,
}
#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub rva: u32,
    pub virtual_size: u32,
    pub file_offset: u32,
    pub file_size: u32,
    pub flags: u32,
}
impl Section {
    fn span(self) -> usize {
        (self.virtual_size as usize).max(self.file_size as usize)
    }
    fn page_end(self) -> usize {
        (self.rva as usize + self.span() + PAGE - 1) & !(PAGE - 1)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportSymbol {
    GetStdHandle,
    WriteFile,
    ExitProcess,
}
#[derive(Clone, Copy, Debug)]
pub struct Import<'a> {
    pub iat_rva: u32,
    pub dll: &'a str,
    pub name: &'a str,
    pub symbol: ImportSymbol,
}
#[derive(Debug)]
pub struct PeImage<'a> {
    bytes: &'a [u8],
    sections: [Section; MAX_SECTIONS],
    count: usize,
    headers: usize,
    image_size: usize,
    entry: u64,
    imports: [Option<Import<'a>>; MAX_IMPORTS],
    import_count: usize,
}
fn read<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], PeError> {
    bytes
        .get(offset..offset.checked_add(N).ok_or(PeError::Overflow)?)
        .ok_or(PeError::Truncated)?
        .try_into()
        .map_err(|_| PeError::Truncated)
}
fn u16_at(b: &[u8], i: usize) -> Result<u16, PeError> {
    Ok(u16::from_le_bytes(read(b, i)?))
}
fn u32_at(b: &[u8], i: usize) -> Result<u32, PeError> {
    Ok(u32::from_le_bytes(read(b, i)?))
}
fn u64_at(b: &[u8], i: usize) -> Result<u64, PeError> {
    Ok(u64::from_le_bytes(read(b, i)?))
}
fn ranges_overlap(a: usize, size_a: usize, b: usize, size_b: usize) -> bool {
    size_a != 0 && size_b != 0 && a < b + size_b && b < a + size_a
}
impl<'a> PeImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, PeError> {
        if bytes.len() > crate::native::MAX_FILE_BYTES {
            return Err(PeError::Size);
        }
        if bytes.get(..2) != Some(b"MZ") {
            return Err(PeError::Magic);
        }
        let nt = u32_at(bytes, 0x3c)? as usize;
        if nt < 64 || !nt.is_multiple_of(4) || nt > PAGE - 264 {
            return Err(PeError::Header);
        }
        if bytes.get(nt..nt + 4) != Some(b"PE\0\0") {
            return Err(PeError::Magic);
        }
        let coff = nt + 4;
        if u16_at(bytes, coff)? != 0x8664 {
            return Err(PeError::Architecture);
        }
        let count = u16_at(bytes, coff + 2)? as usize;
        if count == 0 || count > MAX_SECTIONS {
            return Err(PeError::Sections);
        }
        if u32_at(bytes, coff + 8)? != 0 || u32_at(bytes, coff + 12)? != 0 {
            return Err(PeError::Header);
        }
        if u16_at(bytes, coff + 16)? != 240 {
            return Err(PeError::Header);
        }
        let characteristics = u16_at(bytes, coff + 18)?;
        if characteristics & 0x22 != 0x22 || characteristics & !0x23 != 0 {
            return Err(PeError::Flags);
        }
        let optional = coff + 20;
        if u16_at(bytes, optional)? != 0x20b {
            return Err(PeError::Architecture);
        }
        if u64_at(bytes, optional + 24)? != IMAGE_BASE {
            return Err(PeError::Address);
        }
        if u32_at(bytes, optional + 32)? != PAGE as u32
            || u32_at(bytes, optional + 36)? != FILE_ALIGN as u32
        {
            return Err(PeError::Alignment);
        }
        if u32_at(bytes, optional + 52)? != 0
            || u32_at(bytes, optional + 104)? != 0
            || u32_at(bytes, optional + 108)? != 16
        {
            return Err(PeError::Header);
        }
        if u16_at(bytes, optional + 68)? != 3 {
            return Err(PeError::Subsystem);
        }
        let dll_flags = u16_at(bytes, optional + 70)?;
        if dll_flags & 0x100 == 0 || dll_flags & !0x8100 != 0 {
            return Err(PeError::Flags);
        }
        let stack_reserve = u64_at(bytes, optional + 72)?;
        let stack_commit = u64_at(bytes, optional + 80)?;
        if stack_reserve == 0
            || stack_reserve > (STACK_PAGES * PAGE) as u64
            || stack_commit == 0
            || stack_commit > stack_reserve
            || !stack_reserve.is_multiple_of(PAGE as u64)
            || !stack_commit.is_multiple_of(PAGE as u64)
        {
            return Err(PeError::Stack);
        }
        let heap_reserve = u64_at(bytes, optional + 88)?;
        let heap_commit = u64_at(bytes, optional + 96)?;
        if heap_commit > heap_reserve || heap_reserve > 1024 * 1024 {
            return Err(PeError::Size);
        }
        let image_size = u32_at(bytes, optional + 56)? as usize;
        let headers = u32_at(bytes, optional + 60)? as usize;
        let section_table = optional + 240;
        let headers_end = section_table
            .checked_add(count * 40)
            .ok_or(PeError::Overflow)?;
        if headers == 0
            || headers > PAGE
            || !headers.is_multiple_of(FILE_ALIGN)
            || headers < headers_end
            || headers > bytes.len()
            || image_size > MAX_IMAGE_BYTES
            || !image_size.is_multiple_of(PAGE)
            || image_size < PAGE
        {
            return Err(PeError::Size);
        }
        const EMPTY: Section = Section {
            rva: 0,
            virtual_size: 0,
            file_offset: 0,
            file_size: 0,
            flags: 0,
        };
        let mut image = Self {
            bytes,
            sections: [EMPTY; MAX_SECTIONS],
            count,
            headers,
            image_size,
            entry: 0,
            imports: [None; MAX_IMPORTS],
            import_count: 0,
        };
        let mut maximum = PAGE;
        for i in 0..count {
            let at = section_table + i * 40;
            let name = read::<8>(bytes, at)?;
            if name[0] == 0
                || !name
                    .iter()
                    .all(|b| *b == 0 || b.is_ascii_alphanumeric() || matches!(*b, b'.' | b'_'))
            {
                return Err(PeError::Sections);
            }
            if let Some(zero) = name.iter().position(|b| *b == 0) {
                if name[zero..].iter().any(|b| *b != 0) {
                    return Err(PeError::Sections);
                }
            }
            if u32_at(bytes, at + 24)? != 0
                || u32_at(bytes, at + 28)? != 0
                || u16_at(bytes, at + 32)? != 0
                || u16_at(bytes, at + 34)? != 0
            {
                return Err(PeError::Sections);
            }
            let characteristics = u32_at(bytes, at + 36)?;
            if characteristics & SECTION_READ == 0 || characteristics & !0xe000_00e0 != 0 {
                return Err(PeError::Flags);
            }
            let flags =
                PF_R | if characteristics & SECTION_WRITE != 0 {
                    PF_W
                } else {
                    0
                } | if characteristics & SECTION_EXECUTE != 0 {
                    PF_X
                } else {
                    0
                };
            if flags & (PF_W | PF_X) == PF_W | PF_X {
                return Err(PeError::WritableExecutable);
            }
            let section = Section {
                rva: u32_at(bytes, at + 12)?,
                virtual_size: u32_at(bytes, at + 8)?,
                file_offset: u32_at(bytes, at + 20)?,
                file_size: u32_at(bytes, at + 16)?,
                flags,
            };
            let rva = section.rva as usize;
            let file = section.file_offset as usize;
            let size = section.file_size as usize;
            if section.span() == 0 || rva < PAGE || !rva.is_multiple_of(PAGE) {
                return Err(PeError::Alignment);
            }
            if size == 0 {
                if file != 0 {
                    return Err(PeError::Size);
                }
            } else if file < headers
                || !file.is_multiple_of(FILE_ALIGN)
                || !size.is_multiple_of(FILE_ALIGN)
                || file.checked_add(size).is_none_or(|end| end > bytes.len())
            {
                return Err(PeError::Size);
            }
            let memory_end = rva.checked_add(section.span()).ok_or(PeError::Overflow)?;
            if memory_end > image_size || section.page_end() > image_size {
                return Err(PeError::Address);
            }
            for prior in &image.sections[..i] {
                if ranges_overlap(
                    rva,
                    section.page_end() - rva,
                    prior.rva as usize,
                    prior.page_end() - prior.rva as usize,
                ) || ranges_overlap(
                    file,
                    size,
                    prior.file_offset as usize,
                    prior.file_size as usize,
                ) {
                    return Err(PeError::Overlap);
                }
            }
            maximum = maximum.max(section.page_end());
            image.sections[i] = section;
        }
        if maximum != image_size {
            return Err(PeError::Size);
        }
        let entry_rva = u32_at(bytes, optional + 16)?;
        let executable = image.sections[..count]
            .iter()
            .find(|section| {
                section.flags & PF_X != 0
                    && entry_rva >= section.rva
                    && (entry_rva as usize - section.rva as usize) < section.file_size as usize
            })
            .ok_or(PeError::Entry)?;
        if entry_rva as usize - executable.rva as usize >= executable.virtual_size as usize {
            return Err(PeError::Entry);
        }
        image.entry = IMAGE_BASE + entry_rva as u64;
        let mut import = (0u32, 0u32);
        let mut iat = (0u32, 0u32);
        for directory in 0..16 {
            let at = optional + 112 + directory * 8;
            let rva = u32_at(bytes, at)?;
            let size = u32_at(bytes, at + 4)?;
            if (rva == 0) != (size == 0) {
                return Err(PeError::Header);
            }
            if rva == 0 {
                continue;
            }
            match directory {
                1 => import = (rva, size),
                12 => iat = (rva, size),
                _ => return Err(PeError::UnsupportedDirectory(directory as u8)),
            }
        }
        image.parse_imports(import, iat)?;
        Ok(image)
    }
    fn rva_bytes(&self, rva: u32, size: usize) -> Result<&'a [u8], PeError> {
        let rva = rva as usize;
        let end = rva.checked_add(size).ok_or(PeError::Overflow)?;
        if end > self.image_size {
            return Err(PeError::Import);
        }
        if end <= self.headers {
            return self.bytes.get(rva..end).ok_or(PeError::Truncated);
        }
        let section = self.sections[..self.count]
            .iter()
            .find(|s| rva >= s.rva as usize && end <= s.rva as usize + s.file_size as usize)
            .ok_or(PeError::Import)?;
        let offset = section.file_offset as usize + rva - section.rva as usize;
        self.bytes
            .get(offset..offset + size)
            .ok_or(PeError::Truncated)
    }
    fn name(&self, rva: u32) -> Result<&'a str, PeError> {
        for length in 0..64 {
            let byte_rva = rva.checked_add(length).ok_or(PeError::Overflow)?;
            let b = self.rva_bytes(byte_rva, 1)?[0];
            if b == 0 {
                return core::str::from_utf8(self.rva_bytes(rva, length as usize)?)
                    .map_err(|_| PeError::Import);
            }
            if !b.is_ascii_graphic() {
                return Err(PeError::Import);
            }
        }
        Err(PeError::Import)
    }
    fn parse_imports(&mut self, import: (u32, u32), iat: (u32, u32)) -> Result<(), PeError> {
        if import.0 == 0 {
            if iat.0 != 0 {
                return Err(PeError::Iat);
            }
            return Ok(());
        }
        if import.1 != 40 || iat.0 == 0 {
            return Err(PeError::Import);
        }
        let descriptors = self.rva_bytes(import.0, 40)?;
        if descriptors[20..].iter().any(|b| *b != 0)
            || u32_at(descriptors, 4)? != 0
            || u32_at(descriptors, 8)? != 0
        {
            return Err(PeError::Import);
        }
        let lookup = u32_at(descriptors, 0)?;
        let names = u32_at(descriptors, 12)?;
        let first = u32_at(descriptors, 16)?;
        if lookup == 0
            || first == 0
            || lookup == first
            || !lookup.is_multiple_of(8)
            || !first.is_multiple_of(8)
            || iat.0 != first
        {
            return Err(PeError::Iat);
        }
        let dll = self.name(names)?;
        if !dll.eq_ignore_ascii_case("kernel32.dll") {
            return Err(PeError::UnsupportedImport);
        }
        let mut count = 0;
        for index in 0..=MAX_IMPORTS {
            let delta = (index * 8) as u32;
            let lookup_rva = lookup.checked_add(delta).ok_or(PeError::Overflow)?;
            let iat_rva = first.checked_add(delta).ok_or(PeError::Overflow)?;
            let thunk = u64_at(self.rva_bytes(lookup_rva, 8)?, 0)?;
            let destination = u64_at(self.rva_bytes(iat_rva, 8)?, 0)?;
            if thunk != destination {
                return Err(PeError::Iat);
            }
            if thunk == 0 {
                count = index;
                break;
            }
            if index == MAX_IMPORTS {
                return Err(PeError::Import);
            }
            if thunk & (1 << 63) != 0 {
                return Err(PeError::OrdinalImport);
            }
            let name_rva = u32::try_from(thunk).map_err(|_| PeError::Import)?;
            self.rva_bytes(name_rva, 2)?;
            let name = self.name(name_rva.checked_add(2).ok_or(PeError::Overflow)?)?;
            let symbol = match name {
                "GetStdHandle" => ImportSymbol::GetStdHandle,
                "WriteFile" => ImportSymbol::WriteFile,
                "ExitProcess" => ImportSymbol::ExitProcess,
                _ => return Err(PeError::UnsupportedImport),
            };
            if self.imports[..index]
                .iter()
                .flatten()
                .any(|i| i.symbol == symbol)
            {
                return Err(PeError::Import);
            }
            self.imports[index] = Some(Import {
                iat_rva,
                dll,
                name,
                symbol,
            });
        }
        let bytes = (count + 1) * 8;
        if count == 0
            || iat.1 as usize != bytes
            || ranges_overlap(lookup as usize, bytes, first as usize, bytes)
        {
            return Err(PeError::Iat);
        }
        let writable = self.sections[..self.count].iter().any(|s| {
            s.flags == PF_R | PF_W
                && first >= s.rva
                && first as usize + bytes <= s.rva as usize + s.file_size as usize
        });
        if !writable {
            return Err(PeError::Iat);
        }
        self.import_count = count;
        Ok(())
    }
    pub fn entry(&self) -> u64 {
        self.entry
    }
    pub fn page_flags(&self, page: usize) -> u32 {
        if page >= IMAGE_PAGES || page * PAGE >= self.image_size {
            return 0;
        }
        if page * PAGE < PAGE {
            return PF_R;
        }
        self.sections[..self.count]
            .iter()
            .find(|s| page * PAGE >= s.rva as usize && page * PAGE < s.page_end())
            .map(|s| s.flags)
            .unwrap_or(0)
    }
    pub fn copy_into(&self, out: &mut [u8]) -> Result<(), PeError> {
        if out.len() != IMAGE_BYTES {
            return Err(PeError::Size);
        }
        out.fill(0);
        out[..self.headers].copy_from_slice(&self.bytes[..self.headers]);
        for section in &self.sections[..self.count] {
            let from = section.file_offset as usize;
            let size = section.file_size as usize;
            let to = section.rva as usize;
            out[to..to + size].copy_from_slice(&self.bytes[from..from + size]);
        }
        Ok(())
    }
    pub fn imports(&self) -> Imports<'_, 'a> {
        Imports {
            image: self,
            index: 0,
        }
    }
    pub fn sections(&self) -> &[Section] {
        &self.sections[..self.count]
    }
}
pub struct Imports<'i, 'a> {
    image: &'i PeImage<'a>,
    index: usize,
}
impl<'a> Iterator for Imports<'_, 'a> {
    type Item = Import<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.image.import_count {
            return None;
        }
        let item = self.image.imports[self.index];
        self.index += 1;
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w16(b: &mut [u8], at: usize, v: u16) {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn w32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn w64(b: &mut [u8], at: usize, v: u64) {
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
    const NT: usize = 128;
    const OPT: usize = NT + 24;
    const SECTION: usize = OPT + 240;
    fn fixture() -> [u8; 3072] {
        let mut b = [0; 3072];
        b[..2].copy_from_slice(b"MZ");
        w32(&mut b, 0x3c, NT as u32);
        b[NT..NT + 4].copy_from_slice(b"PE\0\0");
        w16(&mut b, NT + 4, 0x8664);
        w16(&mut b, NT + 6, 4);
        w16(&mut b, NT + 20, 240);
        w16(&mut b, NT + 22, 0x23);
        w16(&mut b, OPT, 0x20b);
        w32(&mut b, OPT + 16, 0x1000);
        w64(&mut b, OPT + 24, IMAGE_BASE);
        w32(&mut b, OPT + 32, 4096);
        w32(&mut b, OPT + 36, 512);
        w32(&mut b, OPT + 56, 0x5000);
        w32(&mut b, OPT + 60, 1024);
        w16(&mut b, OPT + 68, 3);
        w16(&mut b, OPT + 70, 0x100);
        w64(&mut b, OPT + 72, 65536);
        w64(&mut b, OPT + 80, 4096);
        w64(&mut b, OPT + 88, 1048576);
        w64(&mut b, OPT + 96, 4096);
        w32(&mut b, OPT + 108, 16);
        w32(&mut b, OPT + 112 + 8, 0x3000);
        w32(&mut b, OPT + 116 + 8, 40);
        w32(&mut b, OPT + 112 + 12 * 8, 0x3040);
        w32(&mut b, OPT + 116 + 12 * 8, 32);
        for (i, (name, rva, size, flags)) in [
            (b".text".as_slice(), 0x1000, 4, 0x60000020),
            (b".rdata".as_slice(), 0x2000, 8, 0x40000040),
            (b".idata".as_slice(), 0x3000, 512, 0xc0000040),
            (b".data".as_slice(), 0x4000, 1024, 0xc0000040),
        ]
        .iter()
        .enumerate()
        {
            let at = SECTION + i * 40;
            b[at..at + name.len()].copy_from_slice(name);
            w32(&mut b, at + 8, *size);
            w32(&mut b, at + 12, *rva);
            w32(&mut b, at + 16, 512);
            w32(&mut b, at + 20, (1024 + i * 512) as u32);
            w32(&mut b, at + 36, *flags);
        }
        b[1024..1028].copy_from_slice(&[0x90, 0x90, 0x90, 0xc3]);
        b[1536..1544].copy_from_slice(b"readable");
        b[2560..2564].copy_from_slice(b"data");
        let base = 2048;
        w32(&mut b, base, 0x3080);
        w32(&mut b, base + 12, 0x3140);
        w32(&mut b, base + 16, 0x3040);
        b[base + 0x140..base + 0x14c].copy_from_slice(b"kernel32.dll");
        for (i, (rva, name)) in [
            (0x30c0, "GetStdHandle"),
            (0x30e0, "WriteFile"),
            (0x3100, "ExitProcess"),
        ]
        .iter()
        .enumerate()
        {
            w64(&mut b, base + 0x40 + i * 8, *rva);
            w64(&mut b, base + 0x80 + i * 8, *rva);
            let at = base + *rva as usize - 0x3000 + 2;
            b[at..at + name.len()].copy_from_slice(name.as_bytes());
        }
        b
    }
    fn error(bytes: &[u8]) -> PeError {
        PeImage::parse(bytes).unwrap_err()
    }
    #[test]
    fn copies_headers_sections_zero_bss_gaps_and_reserved_shims() {
        let b = fixture();
        let image = PeImage::parse(&b).unwrap();
        assert_eq!(image.entry(), IMAGE_BASE + 4096);
        assert_eq!(image.sections().len(), 4);
        assert_eq!(image.page_flags(0), PF_R);
        assert_eq!(image.page_flags(1), PF_R | PF_X);
        assert_eq!(image.page_flags(2), PF_R);
        assert_eq!(image.page_flags(3), PF_R | PF_W);
        assert_eq!(image.page_flags(4), PF_R | PF_W);
        for page in [5, IMAGE_PAGES - 1, IMAGE_PAGES, usize::MAX] {
            assert_eq!(image.page_flags(page), 0);
        }
        let mut out = [0xa5; IMAGE_BYTES];
        image.copy_into(&mut out).unwrap();
        assert_eq!(&out[..1024], &b[..1024]);
        assert_eq!(&out[4096..4100], &b[1024..1028]);
        assert_eq!(&out[16384..16388], b"data");
        assert!(out[16388..].iter().all(|v| *v == 0));
        assert!(out[1024..4096].iter().all(|v| *v == 0));
        assert_eq!(
            image.copy_into(&mut out[..IMAGE_BYTES - 1]),
            Err(PeError::Size)
        );
    }
    #[test]
    fn resolves_only_closed_builtin_imports() {
        let b = fixture();
        let image = PeImage::parse(&b).unwrap();
        let mut iter = image.imports();
        for (i, symbol) in [
            ImportSymbol::GetStdHandle,
            ImportSymbol::WriteFile,
            ImportSymbol::ExitProcess,
        ]
        .iter()
        .enumerate()
        {
            let import = iter.next().unwrap();
            assert_eq!(import.dll, "kernel32.dll");
            assert_eq!(import.iat_rva, 0x3040 + i as u32 * 8);
            assert_eq!(import.symbol, *symbol);
        }
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
        let mut b = fixture();
        b[2048 + 0x140] = b'K';
        assert!(PeImage::parse(&b).is_ok());
        let mut b = fixture();
        b[2048 + 0x140] = b'z';
        assert_eq!(error(&b), PeError::UnsupportedImport);
        let mut b = fixture();
        b[2048 + 0xc2] = b'X';
        assert_eq!(error(&b), PeError::UnsupportedImport);
    }
    #[test]
    fn rejects_dll_pe32_wrong_arch_foreign_dirs_and_dynamic_base() {
        let mut b = fixture();
        w16(&mut b, NT + 22, 0x2023);
        assert_eq!(error(&b), PeError::Flags);
        let mut b = fixture();
        w16(&mut b, OPT, 0x10b);
        assert_eq!(error(&b), PeError::Architecture);
        let mut b = fixture();
        w16(&mut b, NT + 4, 0x14c);
        assert_eq!(error(&b), PeError::Architecture);
        let mut b = fixture();
        w16(&mut b, OPT + 70, 0x140);
        assert_eq!(error(&b), PeError::Flags);
        for dir in [0usize, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15] {
            let mut b = fixture();
            w32(&mut b, OPT + 112 + dir * 8, 0x2000);
            w32(&mut b, OPT + 116 + dir * 8, 8);
            assert_eq!(error(&b), PeError::UnsupportedDirectory(dir as u8));
        }
    }
    #[test]
    fn enforces_wx_disjoint_pages_file_ranges_entry_and_capacity() {
        let mut b = fixture();
        w32(&mut b, SECTION + 36, 0xe0000020);
        assert_eq!(error(&b), PeError::WritableExecutable);
        let mut b = fixture();
        w32(&mut b, SECTION + 40 + 12, 0x1000);
        assert_eq!(error(&b), PeError::Overlap);
        let mut b = fixture();
        w32(&mut b, SECTION + 40 + 20, 1024);
        assert_eq!(error(&b), PeError::Overlap);
        let mut b = fixture();
        w32(&mut b, OPT + 16, 0x2000);
        assert_eq!(error(&b), PeError::Entry);
        let mut b = fixture();
        w32(&mut b, OPT + 16, 0x1004);
        assert_eq!(error(&b), PeError::Entry);
        let mut b = fixture();
        w64(&mut b, OPT + 24, IMAGE_BASE + 4096);
        assert_eq!(error(&b), PeError::Address);
        let mut b = fixture();
        w32(&mut b, OPT + 56, (MAX_IMAGE_BYTES + 4096) as u32);
        assert_eq!(error(&b), PeError::Size);
        let mut b = fixture();
        w32(&mut b, SECTION + 8, u32::MAX);
        assert_eq!(error(&b), PeError::Address);
    }
    #[test]
    fn refuses_readonly_iat_prebound_ordinal_duplicate_and_hostile_rvas() {
        let mut b = fixture();
        w32(&mut b, SECTION + 80 + 36, 0x40000040);
        assert_eq!(error(&b), PeError::Iat);
        let mut b = fixture();
        w64(&mut b, 2048 + 0x40, 0x12345678);
        assert_eq!(error(&b), PeError::Iat);
        let mut b = fixture();
        w64(&mut b, 2048 + 0x40, 1 << 63);
        w64(&mut b, 2048 + 0x80, 1 << 63);
        assert_eq!(error(&b), PeError::OrdinalImport);
        let mut b = fixture();
        w64(&mut b, 2048 + 0x48, 0x30c0);
        w64(&mut b, 2048 + 0x88, 0x30c0);
        assert_eq!(error(&b), PeError::Import);
        let mut b = fixture();
        w32(&mut b, 2048, 0xfffffff8);
        assert!(PeImage::parse(&b).is_err());
        let mut b = fixture();
        w64(&mut b, 2048 + 0x40, u64::MAX >> 1);
        w64(&mut b, 2048 + 0x80, u64::MAX >> 1);
        assert_eq!(error(&b), PeError::Import);
        let mut b = fixture();
        w32(&mut b, OPT + 116 + 12 * 8, 24);
        assert_eq!(error(&b), PeError::Iat);
    }
    #[test]
    fn bounded_mutations_and_all_truncated_prefixes_never_panic() {
        let b = fixture();
        for length in 0..b.len() {
            assert!(PeImage::parse(&b[..length]).is_err());
        }
        for offset in 0..b.len() {
            let mut mutated = b;
            mutated[offset] ^= 0x80;
            if let Ok(image) = PeImage::parse(&mutated) {
                let mut out = [0; IMAGE_BYTES];
                image.copy_into(&mut out).unwrap();
                assert!(
                    image.entry() >= IMAGE_BASE
                        && image.entry() < IMAGE_BASE + MAX_IMAGE_BYTES as u64
                );
                assert!(image.imports().count() <= MAX_IMPORTS);
            }
        }
    }
}
