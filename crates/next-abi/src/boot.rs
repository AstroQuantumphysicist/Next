//! Pointer fields are physical addresses. Validation never dereferences them.
//! The caller must establish readable mappings and lifetimes before using data.
pub const NEXT_BOOT_MAGIC: u64 = u64::from_le_bytes(*b"NEXTBOOT");
pub const ABI_MAJOR: u16 = 0;
pub const ABI_MINOR: u16 = 2;
pub const PIXEL_RGB: u32 = 0;
pub const PIXEL_BGR: u32 = 1;
/// Bound quadratic overlap checks on untrusted firmware data.
pub const MAX_MEMORY_REGIONS: usize = 4096;
/// Explicit loader-owned SIPI reservation; no AP execution is implied.
pub const AP_TRAMPOLINE_RESERVED: u64 = 1;
pub const AP_TRAMPOLINE_BASE: u64 = 0x80000;
pub const AP_TRAMPOLINE_BYTES: u64 = 4096;
/// Additional owned low stacks/metadata; depends on the code reservation.
pub const AP_BOOTSTRAP_SCRATCH_RESERVED: u64 = 2;
pub const AP_BOOTSTRAP_SCRATCH_BASE: u64 = 0x81000;
pub const AP_BOOTSTRAP_SCRATCH_BYTES: u64 = 8192;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BootInfoHeader {
    pub magic: u64,
    pub abi_major: u16,
    pub abi_minor: u16,
    pub struct_len: u32,
    pub flags: u64,
    pub checksum: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Framebuffer {
    pub base: u64,
    pub byte_len: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: u32,
    pub reserved: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryMap {
    pub base: u64,
    pub byte_len: u64,
    pub descriptor_size: u32,
    pub descriptor_version: u32,
    pub key: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PhysicalRange {
    pub base: u64,
    pub byte_len: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BootInfo {
    pub header: BootInfoHeader,
    pub framebuffer: Framebuffer,
    pub memory_map: MemoryMap,
    pub acpi_rsdp: u64,
    pub kernel: PhysicalRange,
    pub initrd: PhysicalRange,
    /// EFI image including the retained BootInfo record. Never reclaimed yet.
    pub loader: PhysicalRange,
    pub serial_port: u16,
    pub reserved: [u8; 6],
}
const _: () = assert!(core::mem::size_of::<BootInfo>() == 168);
const _: () = assert!(core::mem::size_of::<BootInfoHeader>() == 32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootError {
    Magic,
    Version,
    Length,
    UnsupportedFlags,
    Checksum,
    Reserved,
    Framebuffer,
    MemoryMap,
    Initrd,
    Overflow,
    Alignment,
    Overlap,
}

impl BootInfo {
    pub fn new() -> Self {
        Self {
            header: BootInfoHeader {
                magic: NEXT_BOOT_MAGIC,
                abi_major: ABI_MAJOR,
                abi_minor: ABI_MINOR,
                struct_len: core::mem::size_of::<Self>() as u32,
                flags: 0,
                checksum: 0,
            },
            ..Self::default()
        }
    }
    /// Additive little-endian u64 checksum over the entire fixed 168-byte ABI
    /// record. Its wrapping sum must be zero. This detects corruption, not malice.
    /// This explicit field encoding never reads padding or relies on host endian.
    pub fn seal(&mut self) {
        self.header.checksum = 0u64.wrapping_sub(self.word_sum(false));
    }
    fn word_sum(&self, checksum: bool) -> u64 {
        let f = self.framebuffer;
        let m = self.memory_map;
        let words = [
            self.header.magic,
            self.header.abi_major as u64
                | ((self.header.abi_minor as u64) << 16)
                | ((self.header.struct_len as u64) << 32),
            self.header.flags,
            if checksum { self.header.checksum } else { 0 },
            f.base,
            f.byte_len,
            f.width as u64 | ((f.height as u64) << 32),
            f.stride as u64 | ((f.pixel_format as u64) << 32),
            f.reserved,
            m.base,
            m.byte_len,
            m.descriptor_size as u64 | ((m.descriptor_version as u64) << 32),
            m.key,
            self.acpi_rsdp,
            self.kernel.base,
            self.kernel.byte_len,
            self.initrd.base,
            self.initrd.byte_len,
            self.loader.base,
            self.loader.byte_len,
            self.serial_port as u64
                | ((self.reserved[0] as u64) << 16)
                | ((self.reserved[1] as u64) << 24)
                | ((self.reserved[2] as u64) << 32)
                | ((self.reserved[3] as u64) << 40)
                | ((self.reserved[4] as u64) << 48)
                | ((self.reserved[5] as u64) << 56),
        ];
        words.into_iter().fold(0u64, u64::wrapping_add)
    }
    pub fn validate(&self) -> Result<(), BootError> {
        if self.header.magic != NEXT_BOOT_MAGIC {
            return Err(BootError::Magic);
        }
        if self.header.abi_major != ABI_MAJOR || self.header.abi_minor != ABI_MINOR {
            return Err(BootError::Version);
        }
        if self.header.struct_len as usize != core::mem::size_of::<Self>() {
            return Err(BootError::Length);
        }
        if self.header.flags & !(AP_TRAMPOLINE_RESERVED | AP_BOOTSTRAP_SCRATCH_RESERVED) != 0
            || (self.header.flags & AP_BOOTSTRAP_SCRATCH_RESERVED != 0
                && self.header.flags & AP_TRAMPOLINE_RESERVED == 0)
        {
            return Err(BootError::UnsupportedFlags);
        }
        if self.reserved != [0; 6] || self.framebuffer.reserved != 0 {
            return Err(BootError::Reserved);
        }
        if self.word_sum(true) != 0 {
            return Err(BootError::Checksum);
        }
        self.framebuffer.validate()?;
        let m = self.memory_map;
        if m.base == 0
            || !m.base.is_multiple_of(8)
            || m.descriptor_size < 40
            || !m.descriptor_size.is_multiple_of(8)
            || m.descriptor_version != 1
            || m.byte_len == 0
            || !m.byte_len.is_multiple_of(u64::from(m.descriptor_size))
            || m.byte_len / u64::from(m.descriptor_size) > MAX_MEMORY_REGIONS as u64
        {
            return Err(BootError::MemoryMap);
        }
        m.base.checked_add(m.byte_len).ok_or(BootError::Overflow)?;
        if self.initrd.byte_len == 0 || self.initrd.byte_len > crate::initrd::MAX_IMAGE_SIZE as u64
        {
            return Err(BootError::Initrd);
        }
        for range in [self.kernel, self.loader, self.initrd] {
            if range.base == 0 || range.byte_len == 0 || !range.base.is_multiple_of(4096) {
                return Err(BootError::Alignment);
            }
            range
                .base
                .checked_add(range.byte_len)
                .ok_or(BootError::Overflow)?;
        }
        // Boot allocations own entire pages even when the ELF, archive or map
        // ends partway through one. A byte-disjoint descriptor hidden in that
        // padding must not alias a retained allocation.
        let occupied = [
            page_span(self.kernel)?,
            page_span(self.loader)?,
            page_span(self.initrd)?,
            page_span(PhysicalRange {
                base: m.base,
                byte_len: m.byte_len,
            })?,
        ];
        for (index, (base, end)) in occupied.iter().copied().enumerate() {
            for (flag, reserved_base, reserved_len) in [
                (
                    AP_TRAMPOLINE_RESERVED,
                    AP_TRAMPOLINE_BASE,
                    AP_TRAMPOLINE_BYTES,
                ),
                (
                    AP_BOOTSTRAP_SCRATCH_RESERVED,
                    AP_BOOTSTRAP_SCRATCH_BASE,
                    AP_BOOTSTRAP_SCRATCH_BYTES,
                ),
            ] {
                if self.header.flags & flag != 0
                    && base < reserved_base + reserved_len
                    && reserved_base < end
                {
                    return Err(BootError::Overlap);
                }
            }
            for (other_base, other_end) in occupied[..index].iter().copied() {
                if base < other_end && other_base < end {
                    return Err(BootError::Overlap);
                }
            }
        }
        for (flag, base, len) in [
            (
                AP_TRAMPOLINE_RESERVED,
                AP_TRAMPOLINE_BASE,
                AP_TRAMPOLINE_BYTES,
            ),
            (
                AP_BOOTSTRAP_SCRATCH_RESERVED,
                AP_BOOTSTRAP_SCRATCH_BASE,
                AP_BOOTSTRAP_SCRATCH_BYTES,
            ),
        ] {
            if self.header.flags & flag != 0
                && self.framebuffer.base < base + len
                && base < self.framebuffer.base + self.framebuffer.byte_len
            {
                return Err(BootError::Overlap);
            }
        }
        Ok(())
    }
    /// No pointer access. A known checksummed flag and retained LoaderData map
    /// ownership are both required before a kernel may touch this fixed page.
    pub fn ap_trampoline(
        &self,
        regions: &MemoryRegions<'_>,
    ) -> Result<Option<PhysicalRange>, BootError> {
        self.validate()?;
        if self.header.flags & AP_TRAMPOLINE_RESERVED == 0 {
            return Ok(None);
        }
        if !regions.ap_trampoline_owned() {
            return Err(BootError::MemoryMap);
        }
        Ok(Some(PhysicalRange {
            base: AP_TRAMPOLINE_BASE,
            byte_len: AP_TRAMPOLINE_BYTES,
        }))
    }
    /// Geometry and retained map ownership only. Does not authorize AP startup.
    pub fn ap_bootstrap_scratch(
        &self,
        regions: &MemoryRegions<'_>,
    ) -> Result<Option<PhysicalRange>, BootError> {
        self.validate()?;
        if self.header.flags & AP_BOOTSTRAP_SCRATCH_RESERVED == 0 {
            return Ok(None);
        }
        if !regions.ap_trampoline_owned() || !regions.ap_bootstrap_scratch_owned() {
            return Err(BootError::MemoryMap);
        }
        Ok(Some(PhysicalRange {
            base: AP_BOOTSTRAP_SCRATCH_BASE,
            byte_len: AP_BOOTSTRAP_SCRATCH_BYTES,
        }))
    }
}
fn page_span(range: PhysicalRange) -> Result<(u64, u64), BootError> {
    let end = range
        .base
        .checked_add(range.byte_len)
        .ok_or(BootError::Overflow)?;
    let rounded_end = end.checked_add(4095).ok_or(BootError::Overflow)? & !4095;
    Ok((range.base & !4095, rounded_end))
}
impl Framebuffer {
    pub fn validate(&self) -> Result<(), BootError> {
        if self.base == 0
            || !self.base.is_multiple_of(4)
            || self.width == 0
            || self.height == 0
            || self.stride < self.width
            || self.pixel_format > PIXEL_BGR
        {
            return Err(BootError::Framebuffer);
        }
        let required = u64::from(self.stride)
            .checked_mul(u64::from(self.height))
            .and_then(|v| v.checked_mul(4))
            .ok_or(BootError::Overflow)?;
        if required > self.byte_len {
            return Err(BootError::Framebuffer);
        }
        self.base
            .checked_add(self.byte_len)
            .ok_or(BootError::Overflow)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub kind: u32,
    pub base: u64,
    pub pages: u64,
    pub attributes: u64,
}
impl MemoryRegion {
    pub fn end(self) -> Result<u64, BootError> {
        self.pages
            .checked_mul(4096)
            .and_then(|n| self.base.checked_add(n))
            .ok_or(BootError::Overflow)
    }
    pub const fn is_conventional(self) -> bool {
        self.kind == 7
    }
}
/// Reads UEFI descriptors without casting untrusted bytes to aligned structs.
pub struct MemoryRegions<'a> {
    bytes: &'a [u8],
    stride: usize,
}
impl<'a> MemoryRegions<'a> {
    pub fn ap_trampoline_owned(&self) -> bool {
        self.nonruntime_loader_data(PhysicalRange {
            base: AP_TRAMPOLINE_BASE,
            byte_len: AP_TRAMPOLINE_BYTES,
        })
    }
    pub fn ap_bootstrap_scratch_owned(&self) -> bool {
        self.nonruntime_loader_data(PhysicalRange {
            base: AP_BOOTSTRAP_SCRATCH_BASE,
            byte_len: AP_BOOTSTRAP_SCRATCH_BYTES,
        })
    }
    fn nonruntime_loader_data(&self, range: PhysicalRange) -> bool {
        let Some(end) = range.base.checked_add(range.byte_len) else {
            return false;
        };
        if range.byte_len == 0 {
            return false;
        }
        let mut cursor = range.base;
        while cursor < end {
            let Some(region) = (0..self.len()).filter_map(|i| self.get(i)).find(|r| {
                r.kind == 2
                    && r.attributes & (1u64 << 63) == 0
                    && r.base <= cursor
                    && r.end().is_ok_and(|e| cursor < e)
            }) else {
                return false;
            };
            cursor = region.end().unwrap_or(cursor).min(end);
        }
        true
    }
    pub fn parse(bytes: &'a [u8], stride: usize) -> Result<Self, BootError> {
        if stride < 40
            || !stride.is_multiple_of(8)
            || bytes.is_empty()
            || !bytes.len().is_multiple_of(stride)
            || bytes.len() / stride > MAX_MEMORY_REGIONS
        {
            return Err(BootError::MemoryMap);
        }
        let map = Self { bytes, stride };
        for i in 0..map.len() {
            let a = map.get(i).ok_or(BootError::MemoryMap)?;
            if a.base % 4096 != 0 {
                return Err(BootError::Alignment);
            }
            let end = a.end()?;
            for j in 0..i {
                let b = map.get(j).ok_or(BootError::MemoryMap)?;
                if a.pages != 0 && b.pages != 0 && a.base < b.end()? && b.base < end {
                    return Err(BootError::Overlap);
                }
            }
        }
        Ok(map)
    }
    pub fn len(&self) -> usize {
        self.bytes.len() / self.stride
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    pub fn get(&self, index: usize) -> Option<MemoryRegion> {
        let start = index.checked_mul(self.stride)?;
        let b = self.bytes.get(start..start.checked_add(40)?)?;
        Some(MemoryRegion {
            kind: u32::from_le_bytes(b[0..4].try_into().ok()?),
            base: u64::from_le_bytes(b[8..16].try_into().ok()?),
            pages: u64::from_le_bytes(b[24..32].try_into().ok()?),
            attributes: u64::from_le_bytes(b[32..40].try_into().ok()?),
        })
    }
    /// A retained range must be completely covered by LoaderCode/LoaderData.
    /// Handles unsorted and adjacent descriptors without treating gaps as RAM.
    pub fn is_loader_owned(&self, range: PhysicalRange) -> bool {
        let Some(end) = range.base.checked_add(range.byte_len) else {
            return false;
        };
        if range.byte_len == 0 {
            return false;
        }
        let mut cursor = range.base;
        while cursor < end {
            let Some(region) = (0..self.len()).filter_map(|i| self.get(i)).find(|r| {
                matches!(r.kind, 1 | 2) && r.base <= cursor && r.end().is_ok_and(|e| cursor < e)
            }) else {
                return false;
            };
            cursor = region.end().unwrap_or(cursor).min(end);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> BootInfo {
        let mut b = BootInfo::new();
        b.framebuffer = Framebuffer {
            base: 0x100000,
            byte_len: 640 * 480 * 4,
            width: 640,
            height: 480,
            stride: 640,
            pixel_format: PIXEL_BGR,
            reserved: 0,
        };
        b.memory_map = MemoryMap {
            base: 0x200000,
            byte_len: 48,
            descriptor_size: 48,
            descriptor_version: 1,
            key: 42,
        };
        b.kernel = PhysicalRange {
            base: 0x300000,
            byte_len: 4096,
        };
        b.loader = PhysicalRange {
            base: 0x400000,
            byte_len: 4096,
        };
        b.initrd = PhysicalRange {
            base: 0x500000,
            byte_len: 113,
        };
        b.seal();
        b
    }
    fn descriptor(bytes: &mut [u8], base: u64, pages: u64) {
        bytes[0..4].copy_from_slice(&7u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&base.to_le_bytes());
        bytes[24..32].copy_from_slice(&pages.to_le_bytes());
    }
    #[test]
    fn boot_layout_and_checksum() {
        let mut b = valid();
        assert_eq!(b.validate(), Ok(()));
        b.memory_map.key += 1;
        assert_eq!(b.validate(), Err(BootError::Checksum));
        b.seal();
        assert_eq!(b.validate(), Ok(()));
        b.header.flags = AP_BOOTSTRAP_SCRATCH_RESERVED << 1;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::UnsupportedFlags));
    }
    #[test]
    fn trampoline_requires_known_sealed_flag_and_exact_nonruntime_loader_data() {
        let mut boot = valid();
        let mut bytes = [0; 40];
        descriptor(&mut bytes, AP_TRAMPOLINE_BASE, 1);
        bytes[..4].copy_from_slice(&2u32.to_le_bytes());
        let map = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(boot.ap_trampoline(&map).unwrap().is_none());
        boot.header.flags = AP_TRAMPOLINE_RESERVED;
        assert!(matches!(boot.ap_trampoline(&map), Err(BootError::Checksum)));
        boot.seal();
        let range = boot.ap_trampoline(&map).unwrap().unwrap();
        assert_eq!((range.base, range.byte_len), (0x80000, 4096));
        assert_eq!(core::mem::size_of::<BootInfo>(), 168);
        for kind in [0u32, 1, 3, 4, 5, 6, 7, 9, 10, 11] {
            bytes[..4].copy_from_slice(&kind.to_le_bytes());
            let map = MemoryRegions::parse(&bytes, 40).unwrap();
            assert!(matches!(
                boot.ap_trampoline(&map),
                Err(BootError::MemoryMap)
            ));
        }
        bytes[..4].copy_from_slice(&2u32.to_le_bytes());
        bytes[32..40].copy_from_slice(&(1u64 << 63).to_le_bytes());
        let map = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(matches!(
            boot.ap_trampoline(&map),
            Err(BootError::MemoryMap)
        ));
        bytes[32..40].fill(0);
        bytes[8..16].copy_from_slice(&(AP_TRAMPOLINE_BASE + 4096).to_le_bytes());
        let map = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(matches!(
            boot.ap_trampoline(&map),
            Err(BootError::MemoryMap)
        ));
        boot.loader.base = AP_TRAMPOLINE_BASE;
        boot.seal();
        assert_eq!(boot.validate(), Err(BootError::Overlap));
    }
    #[test]
    fn trampoline_map_boundaries_and_retained_aliases_fail_closed() {
        let mut boot = valid();
        boot.header.flags = AP_TRAMPOLINE_RESERVED;
        boot.seal();
        let mut bytes = [0u8; 40];
        // Firmware may coalesce adjacent LoaderData allocations. Coverage,
        // rather than exact descriptor equality, is required.
        descriptor(&mut bytes, AP_TRAMPOLINE_BASE - 4096, 3);
        bytes[..4].copy_from_slice(&2u32.to_le_bytes());
        assert!(boot
            .ap_trampoline(&MemoryRegions::parse(&bytes, 40).unwrap())
            .unwrap()
            .is_some());
        for (base, pages) in [
            (AP_TRAMPOLINE_BASE, 0),
            (AP_TRAMPOLINE_BASE - 4096, 1),
            (AP_TRAMPOLINE_BASE + 4096, 1),
        ] {
            descriptor(&mut bytes, base, pages);
            bytes[..4].copy_from_slice(&2u32.to_le_bytes());
            assert!(matches!(
                boot.ap_trampoline(&MemoryRegions::parse(&bytes, 40).unwrap()),
                Err(BootError::MemoryMap)
            ));
        }
        for which in 0..5 {
            let mut alias = boot;
            match which {
                0 => alias.kernel.base = AP_TRAMPOLINE_BASE,
                1 => alias.loader.base = AP_TRAMPOLINE_BASE,
                2 => alias.initrd.base = AP_TRAMPOLINE_BASE,
                3 => alias.memory_map.base = AP_TRAMPOLINE_BASE + 4088,
                _ => alias.framebuffer.base = AP_TRAMPOLINE_BASE,
            }
            alias.seal();
            assert_eq!(alias.validate(), Err(BootError::Overlap));
        }
        // Without the ownership flag unrelated low LoaderData grants nothing.
        boot.header.flags = 0;
        boot.seal();
        assert!(boot
            .ap_trampoline(&MemoryRegions::parse(&bytes, 40).unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn scratch_flag_requires_code_and_complete_nonruntime_loader_data() {
        let mut boot = valid();
        let mut bytes = [0u8; 120];
        for (index, base) in [0x80000, 0x81000, 0x82000].into_iter().enumerate() {
            let d = &mut bytes[index * 40..index * 40 + 40];
            descriptor(d, base, 1);
            d[..4].copy_from_slice(&2u32.to_le_bytes());
        }
        let map = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(boot.ap_bootstrap_scratch(&map).unwrap().is_none());
        boot.header.flags = AP_BOOTSTRAP_SCRATCH_RESERVED;
        boot.seal();
        assert_eq!(boot.validate(), Err(BootError::UnsupportedFlags));
        boot.header.flags |= AP_TRAMPOLINE_RESERVED;
        assert!(matches!(
            boot.ap_bootstrap_scratch(&map),
            Err(BootError::Checksum)
        ));
        boot.seal();
        let range = boot.ap_bootstrap_scratch(&map).unwrap().unwrap();
        assert_eq!((range.base, range.byte_len), (0x81000, 8192));
        assert_eq!(core::mem::size_of::<BootInfo>(), 168);
        for index in 0..3 {
            let mut bad = bytes;
            bad[index * 40..index * 40 + 4].copy_from_slice(&7u32.to_le_bytes());
            assert!(matches!(
                boot.ap_bootstrap_scratch(&MemoryRegions::parse(&bad, 40).unwrap()),
                Err(BootError::MemoryMap)
            ));
            let mut bad = bytes;
            bad[index * 40 + 32..index * 40 + 40].copy_from_slice(&(1u64 << 63).to_le_bytes());
            assert!(matches!(
                boot.ap_bootstrap_scratch(&MemoryRegions::parse(&bad, 40).unwrap()),
                Err(BootError::MemoryMap)
            ));
        }
        let mut gap = bytes;
        gap[88..96].copy_from_slice(&0x84000u64.to_le_bytes());
        assert!(matches!(
            boot.ap_bootstrap_scratch(&MemoryRegions::parse(&gap, 40).unwrap()),
            Err(BootError::MemoryMap)
        ));
        let mut coalesced = [0u8; 40];
        descriptor(&mut coalesced, 0x80000, 3);
        coalesced[..4].copy_from_slice(&2u32.to_le_bytes());
        assert!(boot
            .ap_bootstrap_scratch(&MemoryRegions::parse(&coalesced, 40).unwrap())
            .unwrap()
            .is_some());
    }
    #[test]
    fn scratch_cannot_alias_any_retained_span_or_framebuffer() {
        let mut boot = valid();
        boot.header.flags = AP_TRAMPOLINE_RESERVED | AP_BOOTSTRAP_SCRATCH_RESERVED;
        for which in 0..5 {
            let mut alias = boot;
            match which {
                0 => alias.kernel.base = 0x82000,
                1 => alias.loader.base = 0x82000,
                2 => alias.initrd.base = 0x82000,
                3 => alias.memory_map.base = 0x82ff8,
                _ => alias.framebuffer.base = 0x82000,
            }
            alias.seal();
            assert_eq!(alias.validate(), Err(BootError::Overlap));
        }
        boot.header.flags = 0;
        boot.seal();
        let mut d = [0; 40];
        descriptor(&mut d, 0x80000, 3);
        d[..4].copy_from_slice(&2u32.to_le_bytes());
        assert!(boot
            .ap_bootstrap_scratch(&MemoryRegions::parse(&d, 40).unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn boot_versions_and_lengths_fail_closed() {
        let mut b = valid();
        b.header.abi_minor += 1;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Version));
        let mut b = valid();
        b.header.struct_len -= 1;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Length));
        let mut b = valid();
        b.reserved[0] = 1;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Reserved));
    }
    #[test]
    fn framebuffer_bounds_and_overflow() {
        let mut f = valid().framebuffer;
        f.byte_len -= 1;
        assert_eq!(f.validate(), Err(BootError::Framebuffer));
        let mut f = valid().framebuffer;
        f.stride = 1;
        assert_eq!(f.validate(), Err(BootError::Framebuffer));
        let mut f = valid().framebuffer;
        f.base = u64::MAX - 3;
        assert_eq!(f.validate(), Err(BootError::Overflow));
        let mut f = valid().framebuffer;
        f.pixel_format = 2;
        assert_eq!(f.validate(), Err(BootError::Framebuffer));
    }
    #[test]
    fn memory_map_validation() {
        let mut bytes = [0; 96];
        descriptor(&mut bytes[..48], 0x1000, 2);
        descriptor(&mut bytes[48..], 0x3000, 2);
        let map = MemoryRegions::parse(&bytes, 48).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.get(0).unwrap().is_conventional());
        assert!(map.get(usize::MAX).is_none());
        descriptor(&mut bytes[48..], 0x2000, 2);
        assert!(matches!(
            MemoryRegions::parse(&bytes, 48),
            Err(BootError::Overlap)
        ));
        descriptor(&mut bytes[48..], 0x3001, 2);
        assert!(matches!(
            MemoryRegions::parse(&bytes, 48),
            Err(BootError::Alignment)
        ));
        descriptor(&mut bytes[48..], 0xffff_ffff_ffff_f000, 2);
        assert!(matches!(
            MemoryRegions::parse(&bytes, 48),
            Err(BootError::Overflow)
        ));
    }
    #[test]
    fn descriptor_lengths_rejected_without_panicking() {
        let bytes = [0; 96];
        for stride in [0, 1, 39, 41, usize::MAX] {
            assert!(MemoryRegions::parse(&bytes, stride).is_err());
        }
        for len in 0..48 {
            assert!(MemoryRegions::parse(&bytes[..len], 48).is_err());
        }
    }

    #[test]
    fn excessive_descriptor_counts_fail_before_overlap_scan() {
        let bytes = [0; (MAX_MEMORY_REGIONS + 1) * 40];
        assert!(matches!(
            MemoryRegions::parse(&bytes, 40),
            Err(BootError::MemoryMap)
        ));
        let mut b = valid();
        b.memory_map.byte_len =
            (MAX_MEMORY_REGIONS as u64 + 1) * u64::from(b.memory_map.descriptor_size);
        b.seal();
        assert_eq!(b.validate(), Err(BootError::MemoryMap));
    }
    #[test]
    fn occupied_ranges_require_contiguous_loader_descriptors() {
        let mut bytes = [0; 96];
        descriptor(&mut bytes[..48], 0x2000, 2);
        descriptor(&mut bytes[48..], 0x1000, 1);
        bytes[..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[48..52].copy_from_slice(&2u32.to_le_bytes());
        let range = PhysicalRange {
            base: 0x1800,
            byte_len: 0x2800,
        };
        assert!(MemoryRegions::parse(&bytes, 48)
            .unwrap()
            .is_loader_owned(range));
        bytes[..4].copy_from_slice(&7u32.to_le_bytes());
        assert!(!MemoryRegions::parse(&bytes, 48)
            .unwrap()
            .is_loader_owned(range));
        assert!(!MemoryRegions::parse(&bytes, 48)
            .unwrap()
            .is_loader_owned(PhysicalRange {
                base: u64::MAX,
                byte_len: 2
            }));
        assert!(!MemoryRegions::parse(&bytes, 48)
            .unwrap()
            .is_loader_owned(PhysicalRange {
                base: 0x1000,
                byte_len: 0
            }));
    }
    #[test]
    fn kernel_and_loader_cannot_overlap_or_wrap() {
        let mut b = valid();
        b.loader.base = b.kernel.base;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overlap));
        b.loader.base = u64::MAX & !4095;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overflow));
    }
    #[test]
    fn initrd_presence_alignment_size_and_overflow_are_required() {
        for range in [
            PhysicalRange {
                base: 0x500000,
                byte_len: 0,
            },
            PhysicalRange {
                base: 0x500000,
                byte_len: crate::initrd::MAX_IMAGE_SIZE as u64 + 1,
            },
        ] {
            let mut b = valid();
            b.initrd = range;
            b.seal();
            assert_eq!(b.validate(), Err(BootError::Initrd));
        }
        for base in [0, 0x500001] {
            let mut b = valid();
            b.initrd.base = base;
            b.seal();
            assert_eq!(b.validate(), Err(BootError::Alignment));
        }
        let mut b = valid();
        b.initrd = PhysicalRange {
            base: u64::MAX & !4095,
            byte_len: 4096,
        };
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overflow));
        let mut b = valid();
        b.initrd = PhysicalRange {
            base: u64::MAX & !4095,
            byte_len: 1,
        };
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overflow));
        let mut b = valid();
        b.initrd.byte_len = crate::initrd::MAX_IMAGE_SIZE as u64;
        b.seal();
        assert_eq!(b.validate(), Ok(()));
    }
    #[test]
    fn retained_page_spans_cannot_alias_even_in_unused_padding() {
        for base in [
            valid().kernel.base,
            valid().loader.base,
            valid().memory_map.base,
        ] {
            let mut b = valid();
            b.initrd.base = base;
            b.seal();
            assert_eq!(b.validate(), Err(BootError::Overlap));
        }
        // The map bytes don't touch the archive bytes, but share its last page.
        let mut b = valid();
        b.memory_map.base = b.initrd.base + 128;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overlap));
        let mut b = valid();
        b.initrd.byte_len = 4097;
        b.memory_map.base = b.initrd.base + 4096 + 128;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overlap));
        // A map on the following page is safe; touching half-open ends is fine.
        b.memory_map.base = b.initrd.base + 8192;
        b.seal();
        assert_eq!(b.validate(), Ok(()));
        let mut b = valid();
        b.loader.byte_len = 4097;
        b.initrd.base = b.loader.base + 4096;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overlap));
        let mut b = valid();
        b.memory_map.base = b.kernel.base;
        b.seal();
        assert_eq!(b.validate(), Err(BootError::Overlap));
    }
}
