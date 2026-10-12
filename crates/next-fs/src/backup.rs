//! Bounded, host-side backup manifest preflight for the current journal store.
//!
//! This module encodes source paths, byte lengths, and SHA-256 digests, and
//! verifies the complete manifest/payload set before calling a restore target.
//! It is not a backup service: it does not read a disk, write an archive, pick
//! destinations, encrypt data, retain snapshots, or provide rollback if a
//! target itself fails partway through applying already-verified files.
//!
//! Exact limits follow the current writable journal profile: at most eight
//! files, 4096 bytes per file, 31-byte canonical ASCII paths, and 32768 total
//! payload bytes. The format is an unsigned integrity manifest; it does not
//! authenticate who created the manifest or provide encryption.
use sha2::{Digest, Sha256};

pub const MAX_ENTRIES: usize = super::journal::MAX_FILES;
pub const MAX_ENTRY_BYTES: usize = super::journal::MAX_FILE_BYTES;
pub const MAX_TOTAL_BYTES: usize = MAX_ENTRIES * MAX_ENTRY_BYTES;
pub const MAX_PATH_BYTES: usize = 31;

const MAGIC: &[u8; 8] = b"NXTBKP01";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 16;
const ENTRY_HEADER_BYTES: usize = 44;

/// Maximum serialized manifest size for the current bounded journal profile.
pub const MAX_MANIFEST_BYTES: usize =
    HEADER_BYTES + MAX_ENTRIES * (ENTRY_HEADER_BYTES + MAX_PATH_BYTES);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    Truncated,
    TrailingBytes,
    InvalidHeader,
    UnsupportedVersion,
    TooManyEntries,
    ManifestTooLarge,
    InvalidPath,
    PathTooLong,
    DuplicatePath,
    EntryTooLarge,
    TotalTooLarge,
    BufferTooSmall,
    PayloadCount,
    PayloadSet,
    LengthMismatch,
    DigestMismatch,
}

/// A path and its content, supplied by the host-side backup/restore adapter.
#[derive(Clone, Copy, Debug)]
pub struct FilePayload<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestEntry<'a> {
    pub path: &'a str,
    pub length: u64,
    pub digest: [u8; 32],
}

/// Borrowed view of a fully validated manifest. The wire bytes remain owned by
/// the caller and are never modified or retained by this value.
pub struct Manifest<'a> {
    bytes: &'a [u8],
    count: usize,
}

pub struct Entries<'a> {
    bytes: &'a [u8],
    offset: usize,
    remaining: usize,
}

impl<'a> Iterator for Entries<'a> {
    type Item = ManifestEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let (entry, next) = read_entry(self.bytes, self.offset).ok()?;
        self.offset = next;
        self.remaining -= 1;
        Some(entry)
    }
}

impl<'a> Manifest<'a> {
    /// Parse and validate every entry before exposing any of them to restore.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ManifestError> {
        if bytes.len() < HEADER_BYTES {
            return Err(ManifestError::Truncated);
        }
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::ManifestTooLarge);
        }
        if &bytes[..8] != MAGIC || bytes[12..16] != [0; 4] {
            return Err(ManifestError::InvalidHeader);
        }
        if u16_at(bytes, 8) != VERSION {
            return Err(ManifestError::UnsupportedVersion);
        }
        let count = u16_at(bytes, 10) as usize;
        if count > MAX_ENTRIES {
            return Err(ManifestError::TooManyEntries);
        }

        let mut offset = HEADER_BYTES;
        let mut total_bytes = 0usize;
        let mut paths = [(0usize, 0usize); MAX_ENTRIES];
        for index in 0..count {
            let (entry, next) = read_entry(bytes, offset)?;
            total_bytes = total_bytes
                .checked_add(entry.length as usize)
                .ok_or(ManifestError::TotalTooLarge)?;
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(ManifestError::TotalTooLarge);
            }
            let path_start = offset + ENTRY_HEADER_BYTES;
            let path_len = entry.path.len();
            if paths[..index].iter().any(|(start, len)| {
                *len == path_len
                    && bytes[*start..*start + *len] == bytes[path_start..path_start + path_len]
            }) {
                return Err(ManifestError::DuplicatePath);
            }
            paths[index] = (path_start, path_len);
            offset = next;
        }
        if offset != bytes.len() {
            return Err(ManifestError::TrailingBytes);
        }
        Ok(Self { bytes, count })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn entries(&self) -> Entries<'a> {
        Entries {
            bytes: self.bytes,
            offset: HEADER_BYTES,
            remaining: self.count,
        }
    }
}

/// Encode a manifest for the provided source files and return its exact size.
/// Validation and size checks complete before the output buffer is modified.
pub fn encode(files: &[FilePayload<'_>], out: &mut [u8]) -> Result<usize, ManifestError> {
    if files.len() > MAX_ENTRIES {
        return Err(ManifestError::TooManyEntries);
    }
    let mut total_bytes = 0usize;
    let mut manifest_bytes = HEADER_BYTES;
    for (index, file) in files.iter().enumerate() {
        validate_path(file.path)?;
        if file.bytes.len() > MAX_ENTRY_BYTES {
            return Err(ManifestError::EntryTooLarge);
        }
        total_bytes = total_bytes
            .checked_add(file.bytes.len())
            .ok_or(ManifestError::TotalTooLarge)?;
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(ManifestError::TotalTooLarge);
        }
        if files[..index]
            .iter()
            .any(|previous| previous.path == file.path)
        {
            return Err(ManifestError::DuplicatePath);
        }
        manifest_bytes = manifest_bytes
            .checked_add(ENTRY_HEADER_BYTES + file.path.len())
            .ok_or(ManifestError::TotalTooLarge)?;
    }
    if manifest_bytes > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooManyEntries);
    }
    if out.len() < manifest_bytes {
        return Err(ManifestError::BufferTooSmall);
    }

    out[..HEADER_BYTES].fill(0);
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&VERSION.to_le_bytes());
    out[10..12].copy_from_slice(&(files.len() as u16).to_le_bytes());
    let mut offset = HEADER_BYTES;
    for file in files {
        let path = file.path.as_bytes();
        out[offset..offset + ENTRY_HEADER_BYTES].fill(0);
        out[offset..offset + 2].copy_from_slice(&(path.len() as u16).to_le_bytes());
        out[offset + 4..offset + 12].copy_from_slice(&(file.bytes.len() as u64).to_le_bytes());
        let digest: [u8; 32] = Sha256::digest(file.bytes).into();
        out[offset + 12..offset + ENTRY_HEADER_BYTES].copy_from_slice(&digest);
        let path_start = offset + ENTRY_HEADER_BYTES;
        out[path_start..path_start + path.len()].copy_from_slice(path);
        offset = path_start + path.len();
    }
    Ok(manifest_bytes)
}

pub trait RestoreTarget {
    type Error;

    /// Apply one file after the complete manifest/payload set has been checked.
    fn restore_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreError<E> {
    Manifest(ManifestError),
    Target(E),
}

/// Verify all paths, lengths, and SHA-256 digests before the first target call.
/// A target error can still leave earlier verified files applied; atomic
/// multi-file rollback belongs to a future storage service.
pub fn restore_verified<T: RestoreTarget>(
    manifest_bytes: &[u8],
    payloads: &[FilePayload<'_>],
    target: &mut T,
) -> Result<(), RestoreError<T::Error>> {
    let manifest = Manifest::parse(manifest_bytes).map_err(RestoreError::Manifest)?;
    if payloads.len() != manifest.len() {
        return Err(RestoreError::Manifest(ManifestError::PayloadCount));
    }

    for (index, payload) in payloads.iter().enumerate() {
        validate_path(payload.path).map_err(RestoreError::Manifest)?;
        if payloads[..index]
            .iter()
            .any(|previous| previous.path == payload.path)
        {
            return Err(RestoreError::Manifest(ManifestError::PayloadSet));
        }
    }

    for entry in manifest.entries() {
        let payload = payloads
            .iter()
            .find(|payload| payload.path == entry.path)
            .ok_or(RestoreError::Manifest(ManifestError::PayloadSet))?;
        if payload.bytes.len() > MAX_ENTRY_BYTES {
            return Err(RestoreError::Manifest(ManifestError::EntryTooLarge));
        }
        if payload.bytes.len() as u64 != entry.length {
            return Err(RestoreError::Manifest(ManifestError::LengthMismatch));
        }
        let digest: [u8; 32] = Sha256::digest(payload.bytes).into();
        if digest != entry.digest {
            return Err(RestoreError::Manifest(ManifestError::DigestMismatch));
        }
    }

    for entry in manifest.entries() {
        let payload = payloads
            .iter()
            .find(|payload| payload.path == entry.path)
            .ok_or(RestoreError::Manifest(ManifestError::PayloadSet))?;
        target
            .restore_file(entry.path, payload.bytes)
            .map_err(RestoreError::Target)?;
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), ManifestError> {
    let bytes = path.as_bytes();
    if bytes.len() > MAX_PATH_BYTES {
        return Err(ManifestError::PathTooLong);
    }
    if bytes.len() < 2
        || !path.starts_with('/')
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'/' | b'.' | b'_' | b'-'))
        || !path[1..]
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
    {
        return Err(ManifestError::InvalidPath);
    }
    Ok(())
}

fn read_entry(bytes: &[u8], offset: usize) -> Result<(ManifestEntry<'_>, usize), ManifestError> {
    let header_end = offset
        .checked_add(ENTRY_HEADER_BYTES)
        .ok_or(ManifestError::Truncated)?;
    if header_end > bytes.len() {
        return Err(ManifestError::Truncated);
    }
    if bytes[offset + 2..offset + 4] != [0; 2] {
        return Err(ManifestError::InvalidHeader);
    }
    let path_len = u16_at(bytes, offset) as usize;
    if path_len > MAX_PATH_BYTES {
        return Err(ManifestError::PathTooLong);
    }
    let path_end = header_end
        .checked_add(path_len)
        .ok_or(ManifestError::Truncated)?;
    if path_end > bytes.len() {
        return Err(ManifestError::Truncated);
    }
    let path = core::str::from_utf8(&bytes[header_end..path_end])
        .map_err(|_| ManifestError::InvalidPath)?;
    validate_path(path)?;
    let length = u64_at(bytes, offset + 4);
    if length > MAX_ENTRY_BYTES as u64 {
        return Err(ManifestError::EntryTooLarge);
    }
    let digest = bytes[offset + 12..header_end]
        .try_into()
        .map_err(|_| ManifestError::Truncated)?;
    Ok((
        ManifestEntry {
            path,
            length,
            digest,
        },
        path_end,
    ))
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    #[derive(Default)]
    struct Applied {
        entries: std::vec::Vec<(std::string::String, std::vec::Vec<u8>)>,
    }

    impl RestoreTarget for Applied {
        type Error = ();

        fn restore_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Self::Error> {
            self.entries.push((path.into(), bytes.into()));
            Ok(())
        }
    }

    fn manifest(files: &[FilePayload<'_>]) -> std::vec::Vec<u8> {
        let mut bytes = vec![0; MAX_MANIFEST_BYTES];
        let used = encode(files, &mut bytes).unwrap();
        bytes.truncate(used);
        bytes
    }

    #[test]
    fn sha256_manifest_round_trips_and_restores_only_after_full_verification() {
        let source = [
            FilePayload {
                path: "/native/alpha",
                bytes: b"version one",
            },
            FilePayload {
                path: "/native/beta",
                bytes: b"",
            },
        ];
        let wire = manifest(&source);
        let parsed = Manifest::parse(&wire).unwrap();
        assert_eq!(parsed.len(), 2);
        let entries: std::vec::Vec<_> = parsed.entries().collect();
        assert_eq!(entries[0].path, "/native/alpha");
        assert_eq!(entries[0].length, 11);
        assert_eq!(entries[1].length, 0);
        assert_eq!(
            entries[1].digest,
            [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55,
            ]
        );

        let reordered = [source[1], source[0]];
        let mut applied = Applied::default();
        restore_verified(&wire, &reordered, &mut applied).unwrap();
        assert_eq!(
            applied.entries,
            vec![
                ("/native/alpha".into(), b"version one".to_vec()),
                ("/native/beta".into(), std::vec::Vec::new()),
            ]
        );
    }

    #[test]
    fn truncated_oversized_and_trailing_manifests_are_rejected() {
        let source = [FilePayload {
            path: "/native/a",
            bytes: b"small",
        }];
        let wire = manifest(&source);
        assert_eq!(
            Manifest::parse(&wire[..8]).err(),
            Some(ManifestError::Truncated)
        );
        assert_eq!(
            Manifest::parse(&wire[..wire.len() - 1]).err(),
            Some(ManifestError::Truncated)
        );

        let mut too_many = wire.clone();
        too_many[10..12].copy_from_slice(&((MAX_ENTRIES + 1) as u16).to_le_bytes());
        assert_eq!(
            Manifest::parse(&too_many).err(),
            Some(ManifestError::TooManyEntries)
        );

        let mut too_large = wire.clone();
        too_large[HEADER_BYTES + 4..HEADER_BYTES + 12]
            .copy_from_slice(&((MAX_ENTRY_BYTES as u64) + 1).to_le_bytes());
        assert_eq!(
            Manifest::parse(&too_large).err(),
            Some(ManifestError::EntryTooLarge)
        );
        let mut applied = Applied::default();
        assert_eq!(
            restore_verified(&too_large, &source, &mut applied),
            Err(RestoreError::Manifest(ManifestError::EntryTooLarge))
        );
        assert!(applied.entries.is_empty());

        let mut trailing = wire;
        trailing.push(0);
        assert_eq!(
            Manifest::parse(&trailing).err(),
            Some(ManifestError::TrailingBytes)
        );
        let oversized_manifest = vec![0; MAX_MANIFEST_BYTES + 1];
        assert_eq!(
            Manifest::parse(&oversized_manifest).err(),
            Some(ManifestError::ManifestTooLarge)
        );
    }

    #[test]
    fn duplicate_and_invalid_paths_and_oversized_inputs_fail_before_encoding() {
        let duplicates = [
            FilePayload {
                path: "/native/a",
                bytes: b"first",
            },
            FilePayload {
                path: "/native/a",
                bytes: b"second",
            },
        ];
        let mut out = [0x5a; MAX_MANIFEST_BYTES];
        assert_eq!(
            encode(&duplicates, &mut out),
            Err(ManifestError::DuplicatePath)
        );
        assert!(out.iter().all(|byte| *byte == 0x5a));
        let too_many = [FilePayload {
            path: "/native/a",
            bytes: b"",
        }; MAX_ENTRIES + 1];
        assert_eq!(
            encode(&too_many, &mut out),
            Err(ManifestError::TooManyEntries)
        );

        let mut duplicate_wire = manifest(&[
            FilePayload {
                path: "/native/a",
                bytes: b"a",
            },
            FilePayload {
                path: "/native/b",
                bytes: b"b",
            },
        ]);
        let second_path =
            HEADER_BYTES + ENTRY_HEADER_BYTES + "/native/a".len() + ENTRY_HEADER_BYTES;
        duplicate_wire[second_path..second_path + "/native/b".len()].copy_from_slice(b"/native/a");
        assert_eq!(
            Manifest::parse(&duplicate_wire).err(),
            Some(ManifestError::DuplicatePath)
        );
        let duplicate_payloads = [
            FilePayload {
                path: "/native/a",
                bytes: b"a",
            },
            FilePayload {
                path: "/native/b",
                bytes: b"b",
            },
        ];
        let mut applied = Applied::default();
        assert_eq!(
            restore_verified(&duplicate_wire, &duplicate_payloads, &mut applied),
            Err(RestoreError::Manifest(ManifestError::DuplicatePath))
        );
        assert!(applied.entries.is_empty());

        let oversized_path = [b'a'; MAX_PATH_BYTES + 1];
        let oversized_path = core::str::from_utf8(&oversized_path).unwrap();
        assert_eq!(
            encode(
                &[FilePayload {
                    path: oversized_path,
                    bytes: b"x"
                }],
                &mut out
            ),
            Err(ManifestError::PathTooLong)
        );
        for invalid_path in ["/native/../escape", "/native//empty", "relative"] {
            assert_eq!(
                encode(
                    &[FilePayload {
                        path: invalid_path,
                        bytes: b"x"
                    }],
                    &mut out
                ),
                Err(ManifestError::InvalidPath)
            );
        }
        assert_eq!(
            encode(
                &[FilePayload {
                    path: "/native/a",
                    bytes: &[0; MAX_ENTRY_BYTES + 1]
                }],
                &mut out
            ),
            Err(ManifestError::EntryTooLarge)
        );
        assert_eq!(
            encode(&duplicates, &mut out[..HEADER_BYTES]),
            Err(ManifestError::DuplicatePath)
        );
        let valid = [FilePayload {
            path: "/native/a",
            bytes: b"a",
        }];
        assert_eq!(
            encode(&valid, &mut out[..HEADER_BYTES]),
            Err(ManifestError::BufferTooSmall)
        );
    }

    #[test]
    fn mismatched_restore_sets_are_rejected_before_any_target_mutation() {
        let source = [
            FilePayload {
                path: "/native/a",
                bytes: b"alpha",
            },
            FilePayload {
                path: "/native/b",
                bytes: b"beta",
            },
        ];
        let wire = manifest(&source);
        let wrong_digest = [
            source[0],
            FilePayload {
                path: "/native/b",
                bytes: b"BETA",
            },
        ];
        let wrong_length = [
            source[0],
            FilePayload {
                path: "/native/b",
                bytes: b"bet",
            },
        ];
        let wrong_path = [
            source[0],
            FilePayload {
                path: "/native/c",
                bytes: b"beta",
            },
        ];
        for (payloads, expected) in [
            (&wrong_digest[..], ManifestError::DigestMismatch),
            (&wrong_length[..], ManifestError::LengthMismatch),
            (&wrong_path[..], ManifestError::PayloadSet),
        ] {
            let mut applied = Applied::default();
            assert_eq!(
                restore_verified(&wire, payloads, &mut applied),
                Err(RestoreError::Manifest(expected))
            );
            assert!(applied.entries.is_empty());
        }

        let duplicate_payloads = [
            source[0],
            FilePayload {
                path: "/native/a",
                bytes: b"beta",
            },
        ];
        let mut applied = Applied::default();
        assert_eq!(
            restore_verified(&wire, &duplicate_payloads, &mut applied),
            Err(RestoreError::Manifest(ManifestError::PayloadSet))
        );
        assert!(applied.entries.is_empty());

        let mut applied = Applied::default();
        assert_eq!(
            restore_verified(&wire, &source[..1], &mut applied),
            Err(RestoreError::Manifest(ManifestError::PayloadCount))
        );
        assert!(applied.entries.is_empty());

        let mut truncated = wire.clone();
        truncated.pop();
        let mut applied = Applied::default();
        assert_eq!(
            restore_verified(&truncated, &source, &mut applied),
            Err(RestoreError::Manifest(ManifestError::Truncated))
        );
        assert!(applied.entries.is_empty());
    }
}
