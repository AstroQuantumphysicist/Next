//! Bounded firmware discovery. Validated table bytes never grant permission to
//! read arbitrary physical pointers; all reads require retained ACPI memory.
//! MADT topology discovery does not start application processors or enable SMP.
use next_abi::boot::MemoryRegions;
const MAX_TABLE: usize = 65536;
const MAX_ROOT: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    PhysicalRange,
    Signature,
    Length,
    Checksum,
    Revision,
    Duplicate,
    Entry,
    Capacity,
    MissingMadt,
    MissingCpu,
    MissingBsp,
    Address,
    Flags,
    Hpet,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cpu {
    pub apic_id: u32,
    pub uid: u32,
    pub enabled: bool,
    pub online_capable: bool,
}
const EMPTY_CPU: Cpu = Cpu {
    apic_id: 0,
    uid: 0,
    enabled: false,
    online_capable: false,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApic {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}
const EMPTY_IO: IoApic = IoApic {
    id: 0,
    address: 0,
    gsi_base: 0,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptOverride {
    pub source: u8,
    pub gsi: u32,
    pub flags: u16,
}
const EMPTY_OVERRIDE: InterruptOverride = InterruptOverride {
    source: 0,
    gsi: 0,
    flags: 0,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hpet {
    pub address: u64,
    pub block_id: u32,
    pub number: u8,
    pub minimum_tick: u16,
}
#[derive(Clone, Copy, Debug)]
pub struct Topology {
    pub cpus: [Cpu; 16],
    pub cpu_count: usize,
    pub io_apics: [IoApic; 4],
    pub io_apic_count: usize,
    pub overrides: [InterruptOverride; 16],
    pub override_count: usize,
    pub lapic_address: u64,
    pub bootstrap_apic_id: u32,
    pub pcat_compatible: bool,
    pub unknown_madt_entries: u16,
    pub validated_tables: usize,
    pub uses_xsdt: bool,
    pub hpet: Option<Hpet>,
}
impl Topology {
    fn empty() -> Self {
        Self {
            cpus: [EMPTY_CPU; 16],
            cpu_count: 0,
            io_apics: [EMPTY_IO; 4],
            io_apic_count: 0,
            overrides: [EMPTY_OVERRIDE; 16],
            override_count: 0,
            lapic_address: 0,
            bootstrap_apic_id: 0,
            pcat_compatible: false,
            unknown_madt_entries: 0,
            validated_tables: 0,
            uses_xsdt: false,
            hpet: None,
        }
    }
}
fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn le64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}
fn valid_checksum(b: &[u8]) -> bool {
    b.iter().fold(0u8, |s, b| s.wrapping_add(*b)) == 0
}
/// Entire ranges, including adjacent descriptors, must be classified as ACPI
/// reclaim or NVS. They remain reserved by the kernel's frame allocator.
pub fn acpi_range(regions: &MemoryRegions<'_>, address: u64, len: usize) -> bool {
    let Some(end) = address.checked_add(len as u64) else {
        return false;
    };
    if len == 0 || address == 0 || end > 1u64 << 32 {
        return false;
    }
    let mut cursor = address;
    while cursor < end {
        let Some(r) = (0..regions.len()).filter_map(|i| regions.get(i)).find(|r| {
            matches!(r.kind, 9 | 10) && r.base <= cursor && r.end().is_ok_and(|e| cursor < e)
        }) else {
            return false;
        };
        cursor = r.end().unwrap_or(cursor).min(end);
    }
    true
}
trait Reader {
    fn bytes(&self, address: u64, len: usize) -> Result<&[u8], Error>;
}
struct Physical<'a> {
    regions: &'a MemoryRegions<'a>,
}
impl Reader for Physical<'_> {
    fn bytes(&self, address: u64, len: usize) -> Result<&[u8], Error> {
        if !acpi_range(self.regions, address, len) {
            return Err(Error::PhysicalRange);
        }
        Ok(unsafe { core::slice::from_raw_parts(address as *const u8, len) })
    }
}
fn table(reader: &impl Reader, address: u64) -> Result<&[u8], Error> {
    let header = reader.bytes(address, 36)?;
    let size = le32(header, 4) as usize;
    if !(36..=MAX_TABLE).contains(&size) {
        return Err(Error::Length);
    }
    let bytes = reader.bytes(address, size)?;
    if !valid_checksum(bytes) {
        return Err(Error::Checksum);
    }
    Ok(bytes)
}
fn rsdp(reader: &impl Reader, address: u64) -> Result<(u64, bool), Error> {
    let b = reader.bytes(address, 20)?;
    if &b[..8] != b"RSD PTR " {
        return Err(Error::Signature);
    }
    if !valid_checksum(b) {
        return Err(Error::Checksum);
    }
    match b[15] {
        0 => {
            let root = le32(b, 16) as u64;
            if root == 0 {
                return Err(Error::Address);
            }
            Ok((root, false))
        }
        1 => Err(Error::Revision),
        _ => {
            let extended = reader.bytes(address, 36)?;
            let size = le32(extended, 20) as usize;
            if !(36..=4096).contains(&size) {
                return Err(Error::Length);
            }
            let extended = reader.bytes(address, size)?;
            if !valid_checksum(extended) || extended[33..36] != [0; 3] {
                return Err(Error::Checksum);
            }
            let xsdt = le64(extended, 24);
            if xsdt != 0 {
                Ok((xsdt, true))
            } else {
                let root = le32(extended, 16) as u64;
                if root == 0 {
                    return Err(Error::Address);
                }
                Ok((root, false))
            }
        }
    }
}
fn discover_with(reader: &impl Reader, address: u64) -> Result<Topology, Error> {
    let (root, xsdt) = rsdp(reader, address)?;
    let root = table(reader, root)?;
    if &root[..4] != if xsdt { b"XSDT" } else { b"RSDT" } {
        return Err(Error::Signature);
    }
    let stride = if xsdt { 8 } else { 4 };
    if !(root.len() - 36).is_multiple_of(stride) || (root.len() - 36) / stride > MAX_ROOT {
        return Err(Error::Length);
    }
    let mut seen = [0u64; MAX_ROOT];
    let mut topology = Topology::empty();
    topology.uses_xsdt = xsdt;
    topology.validated_tables = 1;
    let mut madt = false;
    for (index, entry) in root[36..].chunks(stride).enumerate() {
        let pointer = if xsdt {
            le64(entry, 0)
        } else {
            le32(entry, 0) as u64
        };
        if pointer == 0 || seen[..index].contains(&pointer) {
            return Err(Error::Duplicate);
        }
        seen[index] = pointer;
        let bytes = table(reader, pointer)?;
        topology.validated_tables += 1;
        match &bytes[..4] {
            b"APIC" => {
                if madt {
                    return Err(Error::Duplicate);
                }
                parse_madt(bytes, &mut topology)?;
                madt = true;
            }
            b"HPET" => {
                if topology.hpet.is_some() {
                    return Err(Error::Duplicate);
                }
                topology.hpet = Some(parse_hpet(bytes)?);
            }
            _ => {}
        }
    }
    if !madt {
        return Err(Error::MissingMadt);
    }
    if !topology.cpus[..topology.cpu_count]
        .iter()
        .any(|c| c.enabled)
    {
        return Err(Error::MissingCpu);
    }
    Ok(topology)
}
fn mmio_page(address: u64) -> bool {
    address != 0
        && address.is_multiple_of(4096)
        && address
            .checked_add(4096)
            .is_some_and(|end| end <= 1u64 << 32)
}
fn interrupt_flags(flags: u16) -> bool {
    flags & !15 == 0 && flags & 3 != 2 && (flags >> 2) & 3 != 2
}
fn cpu(topology: &mut Topology, id: u32, uid: u32, flags: u32) -> Result<(), Error> {
    if flags & !3 != 0 {
        return Err(Error::Flags);
    }
    if topology.cpus[..topology.cpu_count]
        .iter()
        .any(|c| c.apic_id == id || c.uid == uid)
    {
        return Err(Error::Duplicate);
    }
    if topology.cpu_count == topology.cpus.len() {
        return Err(Error::Capacity);
    }
    topology.cpus[topology.cpu_count] = Cpu {
        apic_id: id,
        uid,
        enabled: flags & 1 != 0,
        online_capable: flags & 2 != 0,
    };
    topology.cpu_count += 1;
    Ok(())
}
fn parse_madt(bytes: &[u8], topology: &mut Topology) -> Result<(), Error> {
    if bytes.len() < 44 {
        return Err(Error::Length);
    }
    topology.lapic_address = le32(bytes, 36) as u64;
    let flags = le32(bytes, 40);
    if flags & !1 != 0 {
        return Err(Error::Flags);
    }
    topology.pcat_compatible = flags & 1 != 0;
    let mut offset = 44;
    let mut override_address = false;
    while offset < bytes.len() {
        if bytes.len() - offset < 2 {
            return Err(Error::Entry);
        }
        let kind = bytes[offset];
        let size = bytes[offset + 1] as usize;
        if size < 2 || offset + size > bytes.len() {
            return Err(Error::Entry);
        }
        let e = &bytes[offset..offset + size];
        offset += size;
        match kind {
            0 => {
                if size != 8 {
                    return Err(Error::Entry);
                }
                cpu(topology, e[3] as u32, e[2] as u32, le32(e, 4))?
            }
            1 => {
                if size != 12 || e[3] != 0 {
                    return Err(Error::Entry);
                }
                let address = le32(e, 4);
                if !mmio_page(address as u64) {
                    return Err(Error::Address);
                }
                let gsi_base = le32(e, 8);
                if topology.io_apics[..topology.io_apic_count]
                    .iter()
                    .any(|a| a.id == e[2] || a.address == address || a.gsi_base == gsi_base)
                {
                    return Err(Error::Duplicate);
                }
                if topology.io_apic_count == topology.io_apics.len() {
                    return Err(Error::Capacity);
                }
                topology.io_apics[topology.io_apic_count] = IoApic {
                    id: e[2],
                    address,
                    gsi_base,
                };
                topology.io_apic_count += 1;
            }
            2 => {
                if size != 10 || e[2] != 0 || e[3] >= 16 {
                    return Err(Error::Entry);
                }
                let gsi = le32(e, 4);
                let flags = le16(e, 8);
                if !interrupt_flags(flags) {
                    return Err(Error::Flags);
                }
                if topology.overrides[..topology.override_count]
                    .iter()
                    .any(|v| v.source == e[3] || v.gsi == gsi)
                {
                    return Err(Error::Duplicate);
                }
                if topology.override_count == topology.overrides.len() {
                    return Err(Error::Capacity);
                }
                topology.overrides[topology.override_count] = InterruptOverride {
                    source: e[3],
                    gsi,
                    flags,
                };
                topology.override_count += 1;
            }
            3 => {
                if size != 8 || !interrupt_flags(le16(e, 2)) {
                    return Err(Error::Entry);
                }
            }
            4 => {
                if size != 6 || !interrupt_flags(le16(e, 3)) || e[5] > 1 {
                    return Err(Error::Entry);
                }
            }
            5 => {
                if size != 12 || le16(e, 2) != 0 {
                    return Err(Error::Entry);
                }
                if override_address {
                    return Err(Error::Duplicate);
                }
                override_address = true;
                topology.lapic_address = le64(e, 4);
            }
            9 => {
                if size != 16 || le16(e, 2) != 0 {
                    return Err(Error::Entry);
                }
                cpu(topology, le32(e, 4), le32(e, 12), le32(e, 8))?
            }
            10 => {
                if size != 12 || !interrupt_flags(le16(e, 2)) || e[8] > 1 || e[9..12] != [0; 3] {
                    return Err(Error::Entry);
                }
            }
            _ => topology.unknown_madt_entries += 1,
        }
    }
    if !mmio_page(topology.lapic_address) {
        return Err(Error::Address);
    }
    Ok(())
}
fn parse_hpet(bytes: &[u8]) -> Result<Hpet, Error> {
    if bytes.len() != 56 {
        return Err(Error::Length);
    }
    if bytes[40] != 0
        || !matches!(bytes[41], 0 | 64)
        || bytes[42] != 0
        || !matches!(bytes[43], 0 | 4)
        || bytes[55] > 2
    {
        return Err(Error::Hpet);
    }
    let address = le64(bytes, 44);
    if !mmio_page(address) {
        return Err(Error::Address);
    }
    Ok(Hpet {
        address,
        block_id: le32(bytes, 36),
        number: bytes[52],
        minimum_tick: le16(bytes, 53),
    })
}
fn bootstrap_id() -> u32 {
    unsafe {
        let max = core::arch::x86_64::__cpuid(0).eax;
        for leaf in [0x1f, 0xb] {
            if max >= leaf {
                let topology = core::arch::x86_64::__cpuid_count(leaf, 0);
                if topology.ebx != 0 {
                    return topology.edx;
                }
            }
        }
        core::arch::x86_64::__cpuid(1).ebx >> 24
    }
}
/// # Safety
/// Sole bootstrap CPU, firmware exited, ACPI reclaim/NVS memory retained and
/// identity mapped below 4 GiB. No firmware or DMA writer mutates these tables.
pub unsafe fn discover(address: u64, regions: &MemoryRegions<'_>) -> Result<Topology, Error> {
    let mut topology = discover_with(&Physical { regions }, address)?;
    topology.bootstrap_apic_id = bootstrap_id();
    if !topology.cpus[..topology.cpu_count]
        .iter()
        .any(|c| c.enabled && c.apic_id == topology.bootstrap_apic_id)
    {
        return Err(Error::MissingBsp);
    }
    Ok(topology)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Image([u8; 4096]);
    impl Reader for Image {
        fn bytes(&self, address: u64, len: usize) -> Result<&[u8], Error> {
            let start = usize::try_from(address).map_err(|_| Error::PhysicalRange)?;
            self.0
                .get(start..start.checked_add(len).ok_or(Error::PhysicalRange)?)
                .ok_or(Error::PhysicalRange)
        }
    }
    fn checksum(b: &mut [u8], at: usize) {
        b[at] = 0;
        let sum = b.iter().fold(0u8, |s, b| s.wrapping_add(*b));
        b[at] = 0u8.wrapping_sub(sum);
    }
    fn sdt(image: &mut Image, at: usize, signature: &[u8; 4], size: usize) {
        let b = &mut image.0[at..at + size];
        b.fill(0);
        b[..4].copy_from_slice(signature);
        b[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        b[8] = 1;
    }
    fn fixture() -> Image {
        let mut image = Image([0; 4096]);
        image.0[16..24].copy_from_slice(b"RSD PTR ");
        image.0[31] = 2;
        image.0[36..40].copy_from_slice(&36u32.to_le_bytes());
        image.0[40..48].copy_from_slice(&128u64.to_le_bytes());
        checksum(&mut image.0[16..36], 8);
        checksum(&mut image.0[16..52], 32);
        sdt(&mut image, 128, b"XSDT", 52);
        image.0[164..172].copy_from_slice(&256u64.to_le_bytes());
        image.0[172..180].copy_from_slice(&512u64.to_le_bytes());
        checksum(&mut image.0[128..180], 9);
        sdt(&mut image, 256, b"APIC", 74);
        image.0[292..296].copy_from_slice(&0xfee00000u32.to_le_bytes());
        image.0[296] = 1;
        image.0[300..308].copy_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]);
        image.0[308..320].copy_from_slice(&[1, 12, 1, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0]);
        image.0[320..330].copy_from_slice(&[2, 10, 0, 0, 2, 0, 0, 0, 0, 0]);
        checksum(&mut image.0[256..330], 9);
        sdt(&mut image, 512, b"HPET", 56);
        image.0[548..552].copy_from_slice(&0x8086a201u32.to_le_bytes());
        image.0[553] = 64;
        image.0[556..564].copy_from_slice(&0xfed00000u64.to_le_bytes());
        image.0[565..567].copy_from_slice(&128u16.to_le_bytes());
        checksum(&mut image.0[512..568], 9);
        image
    }
    #[test]
    fn valid_xsdt_topology_and_hpet_are_discovered() {
        let i = fixture();
        let t = discover_with(&i, 16).unwrap();
        assert!(t.uses_xsdt);
        assert_eq!(t.validated_tables, 3);
        assert_eq!(t.cpu_count, 1);
        assert!(t.cpus[0].enabled);
        assert_eq!(t.io_apics[0].address, 0xfec00000);
        assert_eq!(t.overrides[0].gsi, 2);
        assert_eq!(t.hpet.unwrap().address, 0xfed00000);
    }
    #[test]
    fn wrong_checksums_lengths_and_root_pointers_fail_closed() {
        let base = fixture();
        for at in [16, 24, 48, 137, 265, 521] {
            let mut i = Image(base.0);
            i.0[at] ^= 1;
            assert!(discover_with(&i, 16).is_err(), "offset {at}");
        }
        let mut i = fixture();
        i.0[164..172].copy_from_slice(&u64::MAX.to_le_bytes());
        checksum(&mut i.0[128..180], 9);
        assert_eq!(discover_with(&i, 16).err(), Some(Error::PhysicalRange));
    }
    #[test]
    fn duplicate_roots_cpus_and_bad_madt_sizes_are_rejected() {
        let mut i = fixture();
        i.0[172..180].copy_from_slice(&256u64.to_le_bytes());
        checksum(&mut i.0[128..180], 9);
        assert_eq!(discover_with(&i, 16).err(), Some(Error::Duplicate));
        let mut t = Topology::empty();
        cpu(&mut t, 0, 0, 1).unwrap();
        assert_eq!(cpu(&mut t, 0, 1, 1), Err(Error::Duplicate));
        assert_eq!(cpu(&mut t, 1, 0, 1), Err(Error::Duplicate));
        for length in [0, 1, 7, 255] {
            let mut i = fixture();
            i.0[301] = length;
            checksum(&mut i.0[256..330], 9);
            assert!(discover_with(&i, 16).is_err());
        }
    }
    #[test]
    fn rsdt_v1_and_bounded_unknown_madt_entries_are_supported() {
        let mut i = fixture();
        i.0[31] = 0;
        i.0[32..36].copy_from_slice(&128u32.to_le_bytes());
        checksum(&mut i.0[16..36], 8);
        sdt(&mut i, 128, b"RSDT", 40);
        i.0[164..168].copy_from_slice(&256u32.to_le_bytes());
        checksum(&mut i.0[128..168], 9);
        i.0[320] = 200;
        checksum(&mut i.0[256..330], 9);
        let t = discover_with(&i, 16).unwrap();
        assert!(!t.uses_xsdt);
        assert_eq!(t.unknown_madt_entries, 1);
        assert!(t.hpet.is_none());
    }
    #[test]
    fn interrupt_reserved_encodings_and_hpet_gas_are_checked() {
        for flags in [2, 8, 16, 0xffff] {
            assert!(!interrupt_flags(flags));
        }
        for flags in [0, 1, 3, 4, 12, 15] {
            assert!(interrupt_flags(flags));
        }
        for (offset, value) in [(552, 1), (553, 32), (554, 1), (555, 3), (567, 255)] {
            let mut i = fixture();
            i.0[offset] = value;
            checksum(&mut i.0[512..568], 9);
            assert!(discover_with(&i, 16).is_err());
        }
    }
    #[test]
    fn physical_ranges_require_retained_acpi_coverage_without_gaps() {
        let mut bytes = [0u8; 80];
        bytes[..4].copy_from_slice(&9u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&0x1000u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&1u64.to_le_bytes());
        bytes[40..44].copy_from_slice(&10u32.to_le_bytes());
        bytes[48..56].copy_from_slice(&0x2000u64.to_le_bytes());
        bytes[64..72].copy_from_slice(&1u64.to_le_bytes());
        let r = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(acpi_range(&r, 0x1800, 4096));
        assert!(!acpi_range(&r, 0x2800, 4096));
        assert!(!acpi_range(&r, u64::MAX, 1));
        assert!(!acpi_range(&r, 0x1000, 0));
        bytes[40..44].copy_from_slice(&7u32.to_le_bytes());
        let r = MemoryRegions::parse(&bytes, 40).unwrap();
        assert!(!acpi_range(&r, 0x1800, 4096));
    }
}
