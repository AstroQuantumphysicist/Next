//! Bounded Appearance preferences; not exported or integrated yet.
//!
//! The trusted kernel session adapter must own PATH and authorize changes before
//! calling save. Caller-supplied identities are deliberately absent here. Codec
//! validity/CRC is corruption detection, never authority or authenticity.
//! One Persistence::replace_atomic must map to one existing journal Store::put;
//! no truncate/write sequence or implicit repair is permitted.
pub const PATH: &str = "/prefs/appearance";
pub const RECORD_BYTES: usize = 32;
const MAGIC: &[u8; 8] = b"NXAPREF1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Theme {
    Light = 0,
    Dark = 1,
    HighContrast = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum TextScale {
    Percent100 = 100,
    Percent125 = 125,
    Percent150 = 150,
    Percent200 = 200,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Accent {
    Blue = 0,
    Violet = 1,
    Teal = 2,
    Orange = 3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub theme: Theme,
    pub text_scale: TextScale,
    pub accent: Accent,
}
impl Preferences {
    pub const fn defaults() -> Self {
        Self {
            theme: Theme::Light,
            text_scale: TextScale::Percent100,
            accent: Accent::Blue,
        }
    }
}
impl Default for Preferences {
    fn default() -> Self {
        Self::defaults()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Length,
    Magic,
    Version,
    Reserved,
    Checksum,
    Theme,
    TextScale,
    Accent,
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
impl Preferences {
    pub fn encode(self) -> [u8; RECORD_BYTES] {
        let mut bytes = [0u8; RECORD_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&(RECORD_BYTES as u16).to_le_bytes());
        bytes[12] = self.theme as u8;
        bytes[13] = self.accent as u8;
        bytes[14..16].copy_from_slice(&(self.text_scale as u16).to_le_bytes());
        let crc = crc32(&bytes[..28]);
        bytes[28..].copy_from_slice(&crc.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != RECORD_BYTES {
            return Err(DecodeError::Length);
        }
        if &bytes[..8] != MAGIC {
            return Err(DecodeError::Magic);
        }
        if bytes[8..10] != 1u16.to_le_bytes() {
            return Err(DecodeError::Version);
        }
        if bytes[10..12] != (RECORD_BYTES as u16).to_le_bytes() {
            return Err(DecodeError::Length);
        }
        if bytes[16..28].iter().any(|&byte| byte != 0) {
            return Err(DecodeError::Reserved);
        }
        if crc32(&bytes[..28]) != u32::from_le_bytes(bytes[28..32].try_into().unwrap()) {
            return Err(DecodeError::Checksum);
        }
        let theme = match bytes[12] {
            0 => Theme::Light,
            1 => Theme::Dark,
            2 => Theme::HighContrast,
            _ => return Err(DecodeError::Theme),
        };
        let accent = match bytes[13] {
            0 => Accent::Blue,
            1 => Accent::Violet,
            2 => Accent::Teal,
            3 => Accent::Orange,
            _ => return Err(DecodeError::Accent),
        };
        let text_scale = match u16::from_le_bytes([bytes[14], bytes[15]]) {
            100 => TextScale::Percent100,
            125 => TextScale::Percent125,
            150 => TextScale::Percent150,
            200 => TextScale::Percent200,
            _ => return Err(DecodeError::TextScale),
        };
        Ok(Self {
            theme,
            text_scale,
            accent,
        })
    }
}
/// Backends return None only for an absent object. Oversized records must return
/// their true length or an error, never a truncated successful 32-byte prefix.
pub trait Persistence {
    type Error;
    fn read(
        &mut self,
        path: &str,
        output: &mut [u8; RECORD_BYTES],
    ) -> Result<Option<usize>, Self::Error>;
    fn replace_atomic(&mut self, path: &str, bytes: &[u8; RECORD_BYTES])
        -> Result<(), Self::Error>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    Storage(E),
    Corrupt(DecodeError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    MissingDefault,
    Persisted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub preferences: Preferences,
    pub origin: Origin,
}
pub fn load<P: Persistence>(storage: &mut P) -> Result<Loaded, Error<P::Error>> {
    let mut bytes = [0; RECORD_BYTES];
    match storage.read(PATH, &mut bytes).map_err(Error::Storage)? {
        None => Ok(Loaded {
            preferences: Preferences::default(),
            origin: Origin::MissingDefault,
        }),
        Some(len) if len != RECORD_BYTES => Err(Error::Corrupt(DecodeError::Length)),
        Some(_) => Ok(Loaded {
            preferences: Preferences::decode(&bytes).map_err(Error::Corrupt)?,
            origin: Origin::Persisted,
        }),
    }
}
/// Commit precedes activation. On any uncertain storage error the active value
/// stays unchanged; the journal adapter must retain its normal poisoned/RO state.
/// Calling this after a corrupt load requires explicit trusted recovery consent;
/// load itself never creates or repairs the preferences object.
pub fn save<P: Persistence>(
    storage: &mut P,
    active: &mut Preferences,
    proposed: Preferences,
) -> Result<(), Error<P::Error>> {
    storage
        .replace_atomic(PATH, &proposed.encode())
        .map_err(Error::Storage)?;
    *active = proposed;
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{vec, vec::Vec};
    struct Disk {
        bytes: Option<Vec<u8>>,
        fail: bool,
        writes: usize,
    }
    impl Persistence for Disk {
        type Error = &'static str;
        fn read(
            &mut self,
            path: &str,
            out: &mut [u8; RECORD_BYTES],
        ) -> Result<Option<usize>, Self::Error> {
            assert_eq!(path, PATH);
            Ok(self.bytes.as_ref().map(|bytes| {
                let n = bytes.len().min(out.len());
                out[..n].copy_from_slice(&bytes[..n]);
                bytes.len()
            }))
        }
        fn replace_atomic(
            &mut self,
            path: &str,
            bytes: &[u8; RECORD_BYTES],
        ) -> Result<(), Self::Error> {
            assert_eq!(path, PATH);
            self.writes += 1;
            if self.fail {
                return Err("read only or I/O error");
            }
            self.bytes = Some(bytes.to_vec());
            Ok(())
        }
    }
    fn reseal(bytes: &mut [u8; RECORD_BYTES]) {
        let crc = crc32(&bytes[..28]);
        bytes[28..].copy_from_slice(&crc.to_le_bytes());
    }
    #[test]
    fn all_supported_combinations_are_canonical_and_bounded() {
        assert!(PATH.len() <= 31 && PATH.starts_with('/'));
        for theme in [Theme::Light, Theme::Dark, Theme::HighContrast] {
            for text_scale in [
                TextScale::Percent100,
                TextScale::Percent125,
                TextScale::Percent150,
                TextScale::Percent200,
            ] {
                for accent in [Accent::Blue, Accent::Violet, Accent::Teal, Accent::Orange] {
                    let prefs = Preferences {
                        theme,
                        text_scale,
                        accent,
                    };
                    let encoded = prefs.encode();
                    assert_eq!(Preferences::decode(&encoded), Ok(prefs));
                    assert_eq!(encoded[16..28], [0; 12]);
                }
            }
        }
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
    #[test]
    fn every_single_bit_corruption_and_noncanonical_length_is_rejected() {
        let original = Preferences::default().encode();
        for bit in 0..RECORD_BYTES * 8 {
            let mut bad = original;
            bad[bit / 8] ^= 1 << (bit % 8);
            assert!(Preferences::decode(&bad).is_err(), "bit {bit}");
        }
        for n in 0..RECORD_BYTES {
            assert_eq!(
                Preferences::decode(&original[..n]),
                Err(DecodeError::Length)
            );
        }
        let mut longer = original.to_vec();
        longer.push(0);
        assert_eq!(Preferences::decode(&longer), Err(DecodeError::Length));
    }
    #[test]
    fn valid_checksum_cannot_authorize_unknown_values_or_reserved_extensions() {
        for (offset, value, expected) in [
            (8, 2, DecodeError::Version),
            (12, 3, DecodeError::Theme),
            (13, 4, DecodeError::Accent),
            (14, 99, DecodeError::TextScale),
            (16, 1, DecodeError::Reserved),
        ] {
            let mut bytes = Preferences::default().encode();
            bytes[offset] = value;
            reseal(&mut bytes);
            assert_eq!(Preferences::decode(&bytes), Err(expected));
        }
    }
    #[test]
    fn missing_defaults_corruption_does_not_repair_and_failed_save_keeps_active_state() {
        let mut disk = Disk {
            bytes: None,
            fail: false,
            writes: 0,
        };
        assert_eq!(load(&mut disk).unwrap().origin, Origin::MissingDefault);
        assert_eq!(disk.writes, 0);
        let bad = vec![7; 32];
        disk.bytes = Some(bad.clone());
        assert!(matches!(load(&mut disk), Err(Error::Corrupt(_))));
        assert_eq!(disk.bytes, Some(bad));
        assert_eq!(disk.writes, 0);
        let mut active = Preferences::default();
        let proposed = Preferences {
            theme: Theme::Dark,
            text_scale: TextScale::Percent150,
            accent: Accent::Teal,
        };
        disk.bytes = Some(active.encode().to_vec());
        disk.fail = true;
        assert!(save(&mut disk, &mut active, proposed).is_err());
        assert_eq!(active, Preferences::default());
        assert_eq!(load(&mut disk).unwrap().preferences, active);
        disk.fail = false;
        save(&mut disk, &mut active, proposed).unwrap();
        assert_eq!(active, proposed);
        assert_eq!(
            load(&mut disk).unwrap(),
            Loaded {
                preferences: proposed,
                origin: Origin::Persisted
            }
        );
        disk.bytes.as_mut().unwrap().extend_from_slice(&[0; 4]);
        assert_eq!(load(&mut disk), Err(Error::Corrupt(DecodeError::Length)));
    }
}
