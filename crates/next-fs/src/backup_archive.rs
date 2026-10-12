//! Bounded, no-allocation in-memory archive codec for [`super::backup`].
//!
//! The archive is a 16-byte versioned header, the encoded backup manifest, and
//! tightly packed file bytes in manifest order. The current journal profile
//! bounds it to 8 files, 4096 bytes per file, 32768 payload bytes, a 616-byte
//! manifest, and a 33400-byte archive (the exact bound is
//! [`MAX_ARCHIVE_BYTES`]).
//!
//! This host-side codec only packages bytes already supplied by its caller. It
//! does not authenticate or encrypt archives, read/write persistent storage,
//! select restore destinations, or provide rollback if a restore target fails
//! after earlier verified files have been applied. SHA-256 preflight does
//! ensure that malformed content never causes the first target mutation.

use super::backup::{self, FilePayload, Manifest, ManifestError, RestoreError, RestoreTarget};

const MAGIC: &[u8; 8] = b"NXTBKA01";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 16;
const MAX_PAYLOAD_BYTES: usize = backup::MAX_TOTAL_BYTES;

/// Maximum archive size under the current bounded journal profile.
pub const MAX_ARCHIVE_BYTES: usize = HEADER_BYTES + backup::MAX_MANIFEST_BYTES + MAX_PAYLOAD_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    Truncated,
    TrailingBytes,
    InvalidHeader,
    UnsupportedVersion,
    ManifestTooLarge,
    LengthOverflow,
    BufferTooSmall,
    Manifest(ManifestError),
}

/// Borrowed, structurally validated view into an encoded archive.
pub struct Archive<'a> {
    wire: &'a [u8],
    manifest: Manifest<'a>,
    manifest_end: usize,
}

/// One manifest entry paired with its tightly packed content bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveFile<'a> {
    pub path: &'a str,
    pub length: u64,
    pub digest: [u8; 32],
    pub bytes: &'a [u8],
}

/// Iterator over archive files in manifest order.
pub struct ArchiveFiles<'a> {
    entries: backup::Entries<'a>,
    payloads: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for ArchiveFiles<'a> {
    type Item = ArchiveFile<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.entries.next()?;
        let length = entry.length as usize;
        let end = self.offset + length;
        let bytes = self.payloads.get(self.offset..end)?;
        self.offset = end;
        Some(ArchiveFile {
            path: entry.path,
            length: entry.length,
            digest: entry.digest,
            bytes,
        })
    }
}

impl<'a> Archive<'a> {
    /// Validate the header, manifest, payload bounds, and exact archive length.
    /// Digest verification is deferred to [`Archive::restore`], which runs the
    /// manifest's complete SHA-256 preflight before calling the target.
    pub fn parse(wire: &'a [u8]) -> Result<Self, ArchiveError> {
        if wire.len() < HEADER_BYTES {
            return Err(ArchiveError::Truncated);
        }
        if &wire[..8] != MAGIC || wire[10..12] != [0; 2] {
            return Err(ArchiveError::InvalidHeader);
        }
        if u16_at(wire, 8) != VERSION {
            return Err(ArchiveError::UnsupportedVersion);
        }

        let manifest_len =
            usize::try_from(u32_at(wire, 12)).map_err(|_| ArchiveError::LengthOverflow)?;
        if manifest_len > backup::MAX_MANIFEST_BYTES {
            return Err(ArchiveError::ManifestTooLarge);
        }
        let manifest_start = HEADER_BYTES;
        let manifest_end = manifest_start
            .checked_add(manifest_len)
            .ok_or(ArchiveError::LengthOverflow)?;
        if manifest_end > wire.len() {
            return Err(ArchiveError::Truncated);
        }

        let manifest_bytes = &wire[manifest_start..manifest_end];
        let manifest = Manifest::parse(manifest_bytes).map_err(ArchiveError::Manifest)?;
        let mut payload_len = 0usize;
        for entry in manifest.entries() {
            payload_len = payload_len
                .checked_add(entry.length as usize)
                .ok_or(ArchiveError::LengthOverflow)?;
        }
        if payload_len > MAX_PAYLOAD_BYTES {
            return Err(ArchiveError::Manifest(ManifestError::TotalTooLarge));
        }
        let expected_len = manifest_end
            .checked_add(payload_len)
            .ok_or(ArchiveError::LengthOverflow)?;
        if wire.len() < expected_len {
            return Err(ArchiveError::Truncated);
        }
        if wire.len() > expected_len {
            return Err(ArchiveError::TrailingBytes);
        }

        Ok(Self {
            wire,
            manifest,
            manifest_end,
        })
    }

    pub fn manifest(&self) -> &Manifest<'a> {
        &self.manifest
    }

    pub fn files(&self) -> ArchiveFiles<'a> {
        ArchiveFiles {
            entries: self.manifest.entries(),
            payloads: &self.wire[self.manifest_end..],
            offset: 0,
        }
    }

    /// Verify every payload digest before applying any file to `target`.
    /// Target errors can still leave earlier verified files applied.
    pub fn restore<T: RestoreTarget>(&self, target: &mut T) -> Result<(), RestoreError<T::Error>> {
        let mut payloads = [FilePayload {
            path: "",
            bytes: &[],
        }; backup::MAX_ENTRIES];
        let mut count = 0;
        for file in self.files() {
            payloads[count] = FilePayload {
                path: file.path,
                bytes: file.bytes,
            };
            count += 1;
        }
        let manifest_start = HEADER_BYTES;
        backup::restore_verified(
            &self.wire[manifest_start..self.manifest_end],
            &payloads[..count],
            target,
        )
    }
}

/// Encode a bounded archive without allocating. Validation and all capacity
/// checks complete before the caller's output is modified.
pub fn encode(files: &[FilePayload<'_>], out: &mut [u8]) -> Result<usize, ArchiveError> {
    let mut manifest_bytes = [0u8; backup::MAX_MANIFEST_BYTES];
    let manifest_len =
        backup::encode(files, &mut manifest_bytes).map_err(ArchiveError::Manifest)?;
    let payload_len = files.iter().try_fold(0usize, |total, file| {
        total
            .checked_add(file.bytes.len())
            .ok_or(ArchiveError::LengthOverflow)
    })?;
    let archive_len = HEADER_BYTES
        .checked_add(manifest_len)
        .and_then(|size| size.checked_add(payload_len))
        .ok_or(ArchiveError::LengthOverflow)?;
    if archive_len > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::LengthOverflow);
    }
    if out.len() < archive_len {
        return Err(ArchiveError::BufferTooSmall);
    }

    out[..HEADER_BYTES].fill(0);
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&VERSION.to_le_bytes());
    out[12..16].copy_from_slice(&(manifest_len as u32).to_le_bytes());
    let manifest_end = HEADER_BYTES + manifest_len;
    out[HEADER_BYTES..manifest_end].copy_from_slice(&manifest_bytes[..manifest_len]);
    let mut payload_offset = manifest_end;
    for file in files {
        let payload_end = payload_offset + file.bytes.len();
        out[payload_offset..payload_end].copy_from_slice(file.bytes);
        payload_offset = payload_end;
    }
    Ok(archive_len)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    #[derive(Default)]
    struct Applied {
        files: std::vec::Vec<(std::string::String, std::vec::Vec<u8>)>,
    }

    impl RestoreTarget for Applied {
        type Error = ();

        fn restore_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Self::Error> {
            self.files.push((path.into(), bytes.into()));
            Ok(())
        }
    }

    fn encoded(files: &[FilePayload<'_>]) -> std::vec::Vec<u8> {
        let mut wire = vec![0; MAX_ARCHIVE_BYTES];
        let used = encode(files, &mut wire).unwrap();
        wire.truncate(used);
        wire
    }

    #[test]
    fn archive_round_trips_packed_payloads_and_restores_in_manifest_order() {
        let source = [
            FilePayload {
                path: "/native/alpha",
                bytes: b"alpha data",
            },
            FilePayload {
                path: "/native/beta",
                bytes: b"beta",
            },
            FilePayload {
                path: "/native/empty",
                bytes: b"",
            },
        ];
        let wire = encoded(&source);
        let archive = Archive::parse(&wire).unwrap();
        assert_eq!(archive.manifest().len(), 3);
        let files: std::vec::Vec<_> = archive.files().collect();
        assert_eq!(
            files
                .iter()
                .map(|file| file.path)
                .collect::<std::vec::Vec<_>>(),
            ["/native/alpha", "/native/beta", "/native/empty",]
        );
        assert_eq!(files[0].bytes, b"alpha data");
        assert_eq!(files[1].bytes, b"beta");
        assert!(files[2].bytes.is_empty());

        let mut target = Applied::default();
        archive.restore(&mut target).unwrap();
        assert_eq!(
            target.files,
            vec![
                ("/native/alpha".into(), b"alpha data".to_vec()),
                ("/native/beta".into(), b"beta".to_vec()),
                ("/native/empty".into(), std::vec::Vec::new()),
            ]
        );
    }

    #[test]
    fn malformed_truncated_and_trailing_archives_are_rejected() {
        let wire = encoded(&[FilePayload {
            path: "/native/file",
            bytes: b"payload",
        }]);
        assert_eq!(
            Archive::parse(&wire[..8]).err(),
            Some(ArchiveError::Truncated)
        );
        assert_eq!(
            Archive::parse(&wire[..wire.len() - 1]).err(),
            Some(ArchiveError::Truncated)
        );

        let manifest_len = u32_at(&wire, 12);
        let mut short_manifest = wire.clone();
        short_manifest[12..16].copy_from_slice(&(manifest_len - 1).to_le_bytes());
        assert_eq!(
            Archive::parse(&short_manifest).err(),
            Some(ArchiveError::Manifest(ManifestError::Truncated))
        );
        let mut long_manifest = wire.clone();
        long_manifest[12..16].copy_from_slice(&(manifest_len + 1).to_le_bytes());
        assert_eq!(
            Archive::parse(&long_manifest).err(),
            Some(ArchiveError::Manifest(ManifestError::TrailingBytes))
        );

        let mut bad_magic = wire.clone();
        bad_magic[0] ^= 0xff;
        assert_eq!(
            Archive::parse(&bad_magic).err(),
            Some(ArchiveError::InvalidHeader)
        );

        let mut bad_version = wire.clone();
        bad_version[8..10].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            Archive::parse(&bad_version).err(),
            Some(ArchiveError::UnsupportedVersion)
        );

        let mut oversized_manifest = wire.clone();
        oversized_manifest[12..16]
            .copy_from_slice(&((backup::MAX_MANIFEST_BYTES as u32) + 1).to_le_bytes());
        assert_eq!(
            Archive::parse(&oversized_manifest).err(),
            Some(ArchiveError::ManifestTooLarge)
        );

        let mut trailing = wire;
        trailing.push(0);
        assert_eq!(
            Archive::parse(&trailing).err(),
            Some(ArchiveError::TrailingBytes)
        );
    }

    #[test]
    fn bad_payload_digest_is_found_before_any_restore_target_mutation() {
        let mut wire = encoded(&[
            FilePayload {
                path: "/native/first",
                bytes: b"verified first",
            },
            FilePayload {
                path: "/native/second",
                bytes: b"tamper me",
            },
        ]);
        let archive = Archive::parse(&wire).unwrap();
        let payload_offset = archive.manifest_end;
        let second_offset = payload_offset + b"verified first".len();
        wire[second_offset] ^= 0x80;

        let tampered = Archive::parse(&wire).unwrap();
        let mut target = Applied::default();
        assert_eq!(
            tampered.restore(&mut target),
            Err(RestoreError::Manifest(ManifestError::DigestMismatch))
        );
        assert!(target.files.is_empty());
    }

    #[test]
    fn encoding_checks_capacity_before_mutating_the_output() {
        let source = [FilePayload {
            path: "/native/file",
            bytes: b"payload",
        }];
        let mut short = [0xa5; HEADER_BYTES];
        assert_eq!(
            encode(&source, &mut short),
            Err(ArchiveError::BufferTooSmall)
        );
        assert!(short.iter().all(|byte| *byte == 0xa5));
    }
}
