//! Native compositor surface proposal v1; structural validation only.
//! Buffer handles require independent capability/range validation by the receiver.
//! An authenticated channel supplies client identity; no wire identity is trusted.
pub const HEADER_BYTES: usize = 80;
pub const MAX_DAMAGE: usize = 8;
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_DAMAGE * 16;
pub const MAX_DIMENSION: u32 = 8192;
pub const MAX_BUFFER_BYTES: u64 = 64 * 1024 * 1024;
pub const PRIVATE_CONTENT: u32 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Magic,
    Version,
    Reserved,
    Role,
    Flags,
    Extent,
    Scale,
    Stride,
    Buffer,
    Generation,
    Handle,
    Damage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgbx,
    Bgrx,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Rect {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub surface_generation: u64,
    pub configure_generation: u64,
    pub frame_generation: u64,
    pub buffer_handle: u64,
    pub logical_width: u32,
    pub logical_height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_16_16: u32,
    pub stride_bytes: u32,
    pub buffer_bytes: u64,
    pub private_content: bool,
    pub format: PixelFormat,
    pub damage_rectangles: [Rect; MAX_DAMAGE],
    pub damage_count: usize,
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}
impl Frame {
    /// Layout: magic4/versionu16/headeru16; roleu8/formatu8/countu8/reservedu8;
    /// flagsu32; surface/configure/frame generationsu64; buffer handleu64;
    /// logical/pixel extentsu32x4; scaleu32/strideu32; bufferbytesu64;
    /// followed by count rectangles of four u32 fields in pixel coordinates.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES {
            return Err(Error::Length);
        }
        if &bytes[..4] != b"NXSF" {
            return Err(Error::Magic);
        }
        if bytes[4..6] != 1u16.to_le_bytes() || bytes[6..8] != (HEADER_BYTES as u16).to_le_bytes() {
            return Err(Error::Version);
        }
        // v1 exports only unprivileged toplevel surfaces. System/lock roles and
        // child/modal parenting require another explicitly authorized contract.
        if bytes[8] != 0 {
            return Err(Error::Role);
        }
        let format = match bytes[9] {
            0 => PixelFormat::Rgbx,
            1 => PixelFormat::Bgrx,
            _ => return Err(Error::Buffer),
        };
        let count = bytes[10] as usize;
        if count > MAX_DAMAGE || bytes.len() != HEADER_BYTES + count * 16 {
            return Err(Error::Length);
        }
        if bytes[11] != 0 {
            return Err(Error::Reserved);
        }
        let flags = u32_at(bytes, 12);
        if flags & !PRIVATE_CONTENT != 0 {
            return Err(Error::Flags);
        }
        let surface_generation = u64_at(bytes, 16);
        let configure_generation = u64_at(bytes, 24);
        let frame_generation = u64_at(bytes, 32);
        if surface_generation == 0 || configure_generation == 0 || frame_generation == 0 {
            return Err(Error::Generation);
        }
        let buffer_handle = u64_at(bytes, 40);
        if buffer_handle == 0 {
            return Err(Error::Handle);
        }
        let logical_width = u32_at(bytes, 48);
        let logical_height = u32_at(bytes, 52);
        let pixel_width = u32_at(bytes, 56);
        let pixel_height = u32_at(bytes, 60);
        for extent in [logical_width, logical_height, pixel_width, pixel_height] {
            if extent == 0 || extent > MAX_DIMENSION {
                return Err(Error::Extent);
            }
        }
        let scale_16_16 = u32_at(bytes, 64);
        if !(65536..=262144).contains(&scale_16_16) {
            return Err(Error::Scale);
        }
        for (logical, pixel) in [(logical_width, pixel_width), (logical_height, pixel_height)] {
            let expected = (u64::from(logical) * u64::from(scale_16_16)).div_ceil(65536);
            if expected != u64::from(pixel) {
                return Err(Error::Scale);
            }
        }
        let stride_bytes = u32_at(bytes, 68);
        if !stride_bytes.is_multiple_of(4) || u64::from(stride_bytes) < u64::from(pixel_width) * 4 {
            return Err(Error::Stride);
        }
        let buffer_bytes = u64_at(bytes, 72);
        let required = u64::from(stride_bytes)
            .checked_mul(u64::from(pixel_height))
            .ok_or(Error::Buffer)?;
        if buffer_bytes != required || required > MAX_BUFFER_BYTES {
            return Err(Error::Buffer);
        }
        let mut damage = [Rect::EMPTY; MAX_DAMAGE];
        for (i, rect) in damage.iter_mut().enumerate().take(count) {
            let offset = HEADER_BYTES + i * 16;
            *rect = Rect {
                x: u32_at(bytes, offset),
                y: u32_at(bytes, offset + 4),
                width: u32_at(bytes, offset + 8),
                height: u32_at(bytes, offset + 12),
            };
            if rect.width == 0
                || rect.height == 0
                || rect
                    .x
                    .checked_add(rect.width)
                    .is_none_or(|end| end > pixel_width)
                || rect
                    .y
                    .checked_add(rect.height)
                    .is_none_or(|end| end > pixel_height)
            {
                return Err(Error::Damage);
            }
        }
        Ok(Self {
            surface_generation,
            configure_generation,
            frame_generation,
            buffer_handle,
            logical_width,
            logical_height,
            pixel_width,
            pixel_height,
            scale_16_16,
            stride_bytes,
            buffer_bytes,
            private_content: flags != 0,
            format,
            damage_rectangles: damage,
            damage_count: count,
        })
    }
    /// Encode only a structurally valid descriptor; output remains untouched on
    /// invalid input or insufficient capacity. Capability validation is separate.
    pub fn encode(&self, output: &mut [u8]) -> Result<usize, Error> {
        if self.damage_count > MAX_DAMAGE {
            return Err(Error::Length);
        }
        let length = HEADER_BYTES + self.damage_count * 16;
        if output.len() < length {
            return Err(Error::Length);
        }
        let mut bytes = [0u8; MAX_BYTES];
        bytes[..4].copy_from_slice(b"NXSF");
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
        bytes[9] = match self.format {
            PixelFormat::Rgbx => 0,
            PixelFormat::Bgrx => 1,
        };
        bytes[10] = self.damage_count as u8;
        bytes[12..16].copy_from_slice(
            &(if self.private_content {
                PRIVATE_CONTENT
            } else {
                0
            })
            .to_le_bytes(),
        );
        for (offset, value) in [
            (16, self.surface_generation),
            (24, self.configure_generation),
            (32, self.frame_generation),
            (40, self.buffer_handle),
            (72, self.buffer_bytes),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [
            (48, self.logical_width),
            (52, self.logical_height),
            (56, self.pixel_width),
            (60, self.pixel_height),
            (64, self.scale_16_16),
            (68, self.stride_bytes),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, rect) in self
            .damage_rectangles
            .iter()
            .enumerate()
            .take(self.damage_count)
        {
            for (field, value) in [rect.x, rect.y, rect.width, rect.height]
                .into_iter()
                .enumerate()
            {
                let offset = HEADER_BYTES + index * 16 + field * 4;
                bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        Self::decode(&bytes[..length])?;
        output[..length].copy_from_slice(&bytes[..length]);
        Ok(length)
    }
    pub fn damage(&self) -> &[Rect] {
        &self.damage_rectangles[..self.damage_count.min(MAX_DAMAGE)]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    pub fn valid() -> [u8; 96] {
        let mut b = [0u8; 96];
        b[..4].copy_from_slice(b"NXSF");
        b[4..6].copy_from_slice(&1u16.to_le_bytes());
        b[6..8].copy_from_slice(&80u16.to_le_bytes());
        b[10] = 1;
        for offset in [16, 24, 32, 40] {
            b[offset..offset + 8].copy_from_slice(&1u64.to_le_bytes());
        }
        for (offset, v) in [
            (48, 320u32),
            (52, 200),
            (56, 400),
            (60, 250),
            (64, 81920),
            (68, 1600),
            (88, 400),
            (92, 250),
        ] {
            b[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[72..80].copy_from_slice(&400000u64.to_le_bytes());
        b
    }
    #[test]
    fn fractional_geometry_exact_bytes_and_damage_are_validated() {
        let b = valid();
        let f = Frame::decode(&b).unwrap();
        let mut output = [0xa5; MAX_BYTES];
        assert_eq!(f.encode(&mut output), Ok(96));
        assert_eq!(&output[..96], &b);
        assert!(output[96..].iter().all(|&b| b == 0xa5));
        let mut bad = f;
        bad.stride_bytes = 0;
        let before = output;
        assert_eq!(bad.encode(&mut output), Err(Error::Stride));
        assert_eq!(output, before);
        assert_eq!(f.encode(&mut output[..95]), Err(Error::Length));
        assert_eq!(output, before);
        assert_eq!(
            f.damage(),
            &[Rect {
                x: 0,
                y: 0,
                width: 400,
                height: 250
            }]
        );
        for len in 0..96 {
            assert!(Frame::decode(&b[..len]).is_err());
        }
        let mut bigger = [0u8; 97];
        bigger[..96].copy_from_slice(&b);
        assert_eq!(Frame::decode(&bigger), Err(Error::Length));
    }
    #[test]
    fn hostile_privileged_roles_handles_geometry_and_overflow_are_rejected() {
        for (offset, value, expected) in [
            (8, 1, Error::Role),
            (11, 1, Error::Reserved),
            (12, 2, Error::Flags),
            (10, 9, Error::Length),
        ] {
            let mut b = valid();
            b[offset] = value;
            assert_eq!(Frame::decode(&b), Err(expected));
        }
        for offset in [16, 24, 32, 40] {
            let mut b = valid();
            b[offset..offset + 8].fill(0);
            assert!(Frame::decode(&b).is_err());
        }
        for offset in [48, 52, 56, 60, 64, 68, 80, 84, 88, 92] {
            let mut b = valid();
            b[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(Frame::decode(&b).is_err(), "offset {offset}");
        }
        let mut b = valid();
        b[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(Frame::decode(&b), Err(Error::Buffer));
        let mut b = valid();
        b[56..60].copy_from_slice(&401u32.to_le_bytes());
        assert_eq!(Frame::decode(&b), Err(Error::Scale));
    }
}

/// Read-only latest server configure. Retrieval does not acknowledge it.
pub const CONFIGURE_BYTES: usize = 64;
pub const SURFACE_RIGHTS: u64 = 0x2b;
pub const BUFFER_RIGHTS: u64 = 0x09;
pub const NATIVE_MAX_BUFFER_BYTES: usize = 65536;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configure {
    pub surface_generation: u64,
    pub configure_generation: u64,
    pub logical_width: u32,
    pub logical_height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_16_16: u32,
}
impl Configure {
    fn validate(self) -> Result<Self, Error> {
        if self.surface_generation == 0 || self.configure_generation == 0 {
            return Err(Error::Generation);
        }
        if !(65536..=262144).contains(&self.scale_16_16) {
            return Err(Error::Scale);
        }
        for (logical, pixel) in [
            (self.logical_width, self.pixel_width),
            (self.logical_height, self.pixel_height),
        ] {
            if logical == 0 || pixel == 0 || logical > MAX_DIMENSION || pixel > MAX_DIMENSION {
                return Err(Error::Extent);
            }
            if (u64::from(logical) * u64::from(self.scale_16_16)).div_ceil(65536)
                != u64::from(pixel)
            {
                return Err(Error::Scale);
            }
        }
        Ok(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != CONFIGURE_BYTES {
            return Err(Error::Length);
        }
        if &bytes[..4] != b"NXSC" {
            return Err(Error::Magic);
        }
        if bytes[4..6] != 1u16.to_le_bytes()
            || bytes[6..8] != (CONFIGURE_BYTES as u16).to_le_bytes()
        {
            return Err(Error::Version);
        }
        if bytes[8..16]
            .iter()
            .chain(bytes[52..64].iter())
            .any(|&byte| byte != 0)
        {
            return Err(Error::Reserved);
        }
        Self {
            surface_generation: u64_at(bytes, 16),
            configure_generation: u64_at(bytes, 24),
            logical_width: u32_at(bytes, 32),
            logical_height: u32_at(bytes, 36),
            pixel_width: u32_at(bytes, 40),
            pixel_height: u32_at(bytes, 44),
            scale_16_16: u32_at(bytes, 48),
        }
        .validate()
    }
    pub fn encode(self) -> Result<[u8; CONFIGURE_BYTES], Error> {
        self.validate()?;
        let mut bytes = [0; CONFIGURE_BYTES];
        bytes[..4].copy_from_slice(b"NXSC");
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&(CONFIGURE_BYTES as u16).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.surface_generation.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.configure_generation.to_le_bytes());
        for (offset, value) in [
            (32, self.logical_width),
            (36, self.logical_height),
            (40, self.pixel_width),
            (44, self.pixel_height),
            (48, self.scale_16_16),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(bytes)
    }
}
#[cfg(test)]
mod configure_tests {
    use super::*;
    #[test]
    fn configure_canonical_wire_and_every_truncation_are_checked() {
        let c = Configure {
            surface_generation: 7,
            configure_generation: 9,
            logical_width: 64,
            logical_height: 32,
            pixel_width: 80,
            pixel_height: 40,
            scale_16_16: 81920,
        };
        let bytes = c.encode().unwrap();
        assert_eq!(Configure::decode(&bytes), Ok(c));
        assert_eq!(&bytes[..8], b"NXSC\x01\x00\x40\x00");
        for length in 0..64 {
            assert_eq!(Configure::decode(&bytes[..length]), Err(Error::Length));
        }
        for offset in [8, 15, 52, 63] {
            let mut bad = bytes;
            bad[offset] = 1;
            assert_eq!(Configure::decode(&bad), Err(Error::Reserved));
        }
        let mut bad = c;
        bad.pixel_width += 1;
        assert_eq!(bad.encode(), Err(Error::Scale));
        let mut bad = c;
        bad.configure_generation = 0;
        assert_eq!(bad.encode(), Err(Error::Generation));
    }
}
