//! Pre-SMP paging policy only. This module neither sends SIPIs nor starts APs.
//! The hardware caller supplies its linker-verified immutable kernel image span.
use next_abi::boot::{BootInfo, MemoryRegions, AP_TRAMPOLINE_BASE, AP_TRAMPOLINE_BYTES};
const NX: u64 = 1 << 63;
const PAGE: u64 = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Handoff,
    Unreserved,
    Image,
    Mapping,
    State,
}
/// Opaque authority for exactly the fixed retained page; there is no arbitrary
/// physical-address constructor or writable executable page policy.
pub struct SplitPlan {
    image_len: usize,
}
impl SplitPlan {
    pub fn new(
        boot: &BootInfo,
        regions: &MemoryRegions<'_>,
        old_leaf: u64,
        image_start: u64,
        image_len: usize,
        readonly_start: u64,
        readonly_end: u64,
    ) -> Result<Self, Error> {
        if boot
            .ap_trampoline(regions)
            .map_err(|_| Error::Handoff)?
            .is_none()
        {
            return Err(Error::Unreserved);
        }
        // Exact identity huge page, supervisor RW/NX, default cache profile.
        // Hardware may have set Accessed/Dirty; all other changes are refused.
        if old_leaf & !((1 << 5) | (1 << 6)) != NX | 0x83 {
            return Err(Error::Mapping);
        }
        let image_end = image_start
            .checked_add(image_len as u64)
            .ok_or(Error::Image)?;
        if image_len == 0
            || image_len as u64 > AP_TRAMPOLINE_BYTES
            || readonly_start >= readonly_end
            || image_start < readonly_start
            || image_end > readonly_end
            || readonly_start < 0x20_0000
        {
            return Err(Error::Image);
        }
        Ok(Self { image_len })
    }
    pub fn image_len(&self) -> usize {
        self.image_len
    }
    pub fn entry(&self, index: usize) -> Option<u64> {
        if index >= 512 {
            return None;
        }
        let address = index as u64 * PAGE;
        Some(
            address
                | if address == AP_TRAMPOLINE_BASE {
                    1
                } else {
                    3 | NX
                },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use next_abi::boot::{Framebuffer, MemoryMap, PhysicalRange, AP_TRAMPOLINE_RESERVED};
    fn boot() -> BootInfo {
        let mut b = BootInfo::new();
        b.framebuffer = Framebuffer {
            base: 0xe0000000,
            byte_len: 640 * 480 * 4,
            width: 640,
            height: 480,
            stride: 640,
            pixel_format: 0,
            reserved: 0,
        };
        b.memory_map = MemoryMap {
            base: 0x400000,
            byte_len: 40,
            descriptor_size: 40,
            descriptor_version: 1,
            key: 0,
        };
        b.kernel = PhysicalRange {
            base: 0x4000000,
            byte_len: 4096,
        };
        b.loader = PhysicalRange {
            base: 0x600000,
            byte_len: 4096,
        };
        b.initrd = PhysicalRange {
            base: 0x800000,
            byte_len: 4096,
        };
        b.header.flags = AP_TRAMPOLINE_RESERVED;
        b.seal();
        b
    }
    fn map_bytes() -> [u8; 40] {
        let mut d = [0; 40];
        d[..4].copy_from_slice(&2u32.to_le_bytes());
        d[8..16].copy_from_slice(&AP_TRAMPOLINE_BASE.to_le_bytes());
        d[24..32].copy_from_slice(&1u64.to_le_bytes());
        d
    }
    fn plan(
        b: &BootInfo,
        m: &MemoryRegions<'_>,
        leaf: u64,
        start: u64,
        len: usize,
    ) -> Result<SplitPlan, Error> {
        SplitPlan::new(b, m, leaf, start, len, 0x4000000, 0x4020000)
    }
    #[test]
    fn only_owned_page_is_supervisor_readonly_executable() {
        let d = map_bytes();
        let m = MemoryRegions::parse(&d, 40).unwrap();
        let p = plan(&boot(), &m, NX | 0xe3, 0x4010000, 4096).unwrap();
        assert_eq!(p.image_len(), 4096);
        for i in 0..512 {
            let e = p.entry(i).unwrap();
            assert_eq!(e & !0xfff & !NX, i as u64 * 4096);
            assert_eq!(e & 4, 0);
            if i == 128 {
                assert_eq!(e & (NX | 3), 1);
            } else {
                assert_eq!(e & (NX | 3), NX | 3);
            }
        }
        assert!(p.entry(512).is_none());
    }
    #[test]
    fn changed_huge_mapping_and_unowned_handoff_are_refused() {
        let d = map_bytes();
        let m = MemoryRegions::parse(&d, 40).unwrap();
        let b = boot();
        for leaf in [
            0,
            0x83,
            NX | 0x81,
            NX | 0x87,
            NX | 0x183,
            NX | 0x200083,
            NX | 0x8b,
            NX | 0x1083,
        ] {
            assert!(matches!(
                plan(&b, &m, leaf, 0x4010000, 4),
                Err(Error::Mapping)
            ));
        }
        let mut absent = b;
        absent.header.flags = 0;
        absent.seal();
        assert!(matches!(
            plan(&absent, &m, NX | 0x83, 0x4010000, 4),
            Err(Error::Unreserved)
        ));
        let mut corrupt = b;
        corrupt.header.flags = 0;
        assert!(matches!(
            plan(&corrupt, &m, NX | 0x83, 0x4010000, 4),
            Err(Error::Handoff)
        ));
        let mut wrong = d;
        wrong[..4].copy_from_slice(&7u32.to_le_bytes());
        let m = MemoryRegions::parse(&wrong, 40).unwrap();
        assert!(matches!(
            plan(&b, &m, NX | 0x83, 0x4010000, 4),
            Err(Error::Handoff)
        ));
    }
    #[test]
    fn startup_image_must_fit_immutable_kernel_span() {
        let d = map_bytes();
        let m = MemoryRegions::parse(&d, 40).unwrap();
        let b = boot();
        for (start, len) in [
            (0x4010000, 0),
            (0x4010000, 4097),
            (AP_TRAMPOLINE_BASE, 4),
            (0x3ffffff, 1),
            (0x401ffff, 2),
            (u64::MAX, 2),
        ] {
            assert!(matches!(
                plan(&b, &m, NX | 0x83, start, len),
                Err(Error::Image)
            ));
        }
        assert!(plan(&b, &m, NX | 0x83, 0x401ffff, 1).is_ok());
        assert!(matches!(
            SplitPlan::new(&b, &m, NX | 0x83, 0x1000, 4, 0x1000, 0x2000),
            Err(Error::Image)
        ));
    }
}
