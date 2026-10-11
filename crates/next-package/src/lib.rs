//! Bounded signed native application bundles. No extraction or permission grants.
//! Signature authenticates canonical metadata and SHA-256 hashes of every file.
//! Trust anchors and revocations belong to the caller, never embedded here.
#![no_std]
#![forbid(unsafe_code)]
#[cfg(feature = "host-tools")]
extern crate std;
pub mod identity;
#[cfg(feature = "host-tools")]
pub mod installer;
pub mod trust;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"NXTAPP01";
pub const HEADER_BYTES: usize = 256;
pub const ENTRY_BYTES: usize = 128;
pub const SIGNATURE_BYTES: usize = 64;
pub const MAX_FILES: usize = 32;
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const KNOWN_PERMISSIONS: u64 = 0x3f;
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    Application = 1,
    Deployment = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Header,
    Version,
    Architecture,
    Size,
    Reserved,
    Path,
    Duplicate,
    EntryPoint,
    Permissions,
    Trust,
    UnknownSigner,
    Revoked,
    Signature,
    Digest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustAnchor {
    pub id: [u8; 32],
    pub key: [u8; 32],
    pub revoked: bool,
    pub allowed_permissions: u64,
    pub allowed_kinds: u8,
}
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub fn signer_id(key: &[u8; 32]) -> [u8; 32] {
    sha256(key)
}
pub fn verify_signature(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> Result<(), Error> {
    let key = validated_key(key)?;
    key.verify_strict(message, &Signature::from_bytes(signature))
        .map_err(|_| Error::Signature)
}
fn validated_key(bytes: &[u8; 32]) -> Result<VerifyingKey, Error> {
    let key = VerifyingKey::from_bytes(bytes).map_err(|_| Error::Signature)?;
    if key.is_weak() || key.to_edwards().compress().to_bytes() != key.to_bytes() {
        return Err(Error::Signature);
    }
    Ok(key)
}
fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(b[i..i + 2].try_into().unwrap_or([0; 2]))
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or([0; 4]))
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]))
}
fn string(bytes: &[u8]) -> Result<&str, Error> {
    let end = bytes.iter().position(|b| *b == 0).ok_or(Error::Path)?;
    if bytes[end..].iter().any(|b| *b != 0) {
        return Err(Error::Path);
    }
    core::str::from_utf8(&bytes[..end]).map_err(|_| Error::Path)
}
pub fn valid_path(path: &str) -> bool {
    if path.is_empty()
        || path.len() >= 64
        || !path.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'/' | b'.' | b'_' | b'-')
        })
    {
        return false;
    }
    path.split('/').all(|part| {
        if part.is_empty() || part == "." || part == ".." || part.ends_with('.') {
            return false;
        }
        let stem = part.split('.').next().unwrap_or("");
        !matches!(stem, "con" | "prn" | "aux" | "nul")
            && !(stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    })
}
fn valid_id(id: &str) -> bool {
    id.len() >= 3
        && id.len() < 64
        && id.contains('.')
        && id.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-')
                })
        })
}
#[derive(Clone, Copy, Debug)]
pub struct File<'a> {
    pub path: &'a str,
    pub data: &'a [u8],
    pub digest: [u8; 32],
}
/// Possessing this value proves authentication/integrity, not install authority.
#[derive(Debug)]
pub struct VerifiedPackage<'a> {
    bytes: &'a [u8],
    count: usize,
    entry: usize,
    pub permissions: u64,
    pub version: [u32; 3],
    pub signer: [u8; 32],
    pub kind: PackageKind,
}
impl<'a> VerifiedPackage<'a> {
    pub fn verify(bytes: &'a [u8], anchors: &[TrustAnchor]) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES + ENTRY_BYTES + 1 + SIGNATURE_BYTES
            || bytes.get(..8) != Some(MAGIC)
        {
            return Err(Error::Header);
        }
        if bytes.len() > MAX_BUNDLE_BYTES
            || u64_at(bytes, 24) != bytes.len() as u64
            || u32_at(bytes, 12) != HEADER_BYTES as u32
        {
            return Err(Error::Size);
        }
        if u16_at(bytes, 8) != 1 || u16_at(bytes, 160) != 0 || u16_at(bytes, 162) != 1 {
            return Err(Error::Version);
        }
        if u16_at(bytes, 10) != 62 {
            return Err(Error::Architecture);
        }
        if u64_at(bytes, 16) != 0
            || bytes[60..64].iter().any(|b| *b != 0)
            || bytes[166..HEADER_BYTES].iter().any(|b| *b != 0)
        {
            return Err(Error::Reserved);
        }
        let kind = match u16_at(bytes, 164) {
            1 => PackageKind::Application,
            2 => PackageKind::Deployment,
            _ => return Err(Error::Version),
        };
        let count = u32_at(bytes, 32) as usize;
        let entry = u32_at(bytes, 36) as usize;
        if count == 0 || count > MAX_FILES || entry >= count {
            return Err(Error::Size);
        }
        let table_end = HEADER_BYTES + count * ENTRY_BYTES;
        let content_end = bytes.len() - SIGNATURE_BYTES;
        if table_end >= content_end {
            return Err(Error::Size);
        }
        let permissions = u64_at(bytes, 40);
        if permissions & !KNOWN_PERMISSIONS != 0 {
            return Err(Error::Permissions);
        }
        let version = [u32_at(bytes, 48), u32_at(bytes, 52), u32_at(bytes, 56)];
        if version == [0; 3] || !valid_id(string(&bytes[96..160])?) {
            return Err(Error::Version);
        }
        let mut signer = [0; 32];
        signer.copy_from_slice(&bytes[64..96]);
        if anchors.len() > 32 {
            return Err(Error::Trust);
        }
        for (i, anchor) in anchors.iter().enumerate() {
            if signer_id(&anchor.key) != anchor.id
                || anchor.allowed_permissions & !KNOWN_PERMISSIONS != 0
                || anchor.allowed_kinds & !3 != 0
                || anchors[..i].iter().any(|a| a.id == anchor.id)
            {
                return Err(Error::Trust);
            }
        }
        let anchor = anchors
            .iter()
            .find(|a| a.id == signer)
            .ok_or(Error::UnknownSigner)?;
        if anchor.revoked {
            return Err(Error::Revoked);
        }
        if anchor.allowed_kinds & (1 << (kind as u16 - 1)) == 0
            || permissions & !anchor.allowed_permissions != 0
        {
            return Err(Error::Permissions);
        }
        let signature: &[u8; 64] = bytes[content_end..].try_into().map_err(|_| Error::Size)?;
        // Authenticate metadata before returning or acting on its declarations.
        verify_signature(&anchor.key, &bytes[..table_end], signature)?;
        let package = Self {
            bytes,
            count,
            entry,
            permissions,
            version,
            signer,
            kind,
        };
        let mut expected = table_end;
        let mut previous = "";
        for i in 0..count {
            let at = HEADER_BYTES + i * ENTRY_BYTES;
            if bytes[at + 112..at + 128].iter().any(|b| *b != 0) {
                return Err(Error::Reserved);
            }
            let path = string(&bytes[at..at + 64])?;
            if !valid_path(path) {
                return Err(Error::Path);
            }
            // Canonical lexical ordering also rejects duplicate/case aliases.
            if i != 0 && path <= previous {
                return Err(Error::Duplicate);
            }
            previous = path;
            let offset = usize::try_from(u64_at(bytes, at + 64)).map_err(|_| Error::Size)?;
            let size = usize::try_from(u64_at(bytes, at + 72)).map_err(|_| Error::Size)?;
            if offset != expected || size == 0 || size > MAX_FILE_BYTES {
                return Err(Error::Size);
            }
            expected = offset
                .checked_add(size)
                .filter(|end| *end <= content_end)
                .ok_or(Error::Size)?;
            if sha256(&bytes[offset..expected]) != bytes[at + 80..at + 112] {
                return Err(Error::Digest);
            }
        }
        if expected != content_end {
            return Err(Error::Size);
        }
        let executable = package.file(entry).ok_or(Error::EntryPoint)?;
        if !executable
            .path
            .starts_with(if kind == PackageKind::Application {
                "bin/"
            } else {
                "boot/"
            })
            || !(executable.path.ends_with(".elf")
                || kind == PackageKind::Application && executable.path.ends_with(".exe"))
        {
            return Err(Error::EntryPoint);
        }
        Ok(package)
    }
    pub fn id(&self) -> &'a str {
        string(&self.bytes[96..160]).unwrap_or("")
    }
    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn executable(&self) -> File<'a> {
        self.file(self.entry).expect("verified entry index")
    }
    pub fn file(&self, index: usize) -> Option<File<'a>> {
        if index >= self.count {
            return None;
        }
        let at = HEADER_BYTES + index * ENTRY_BYTES;
        let offset = usize::try_from(u64_at(self.bytes, at + 64)).ok()?;
        let size = usize::try_from(u64_at(self.bytes, at + 72)).ok()?;
        let mut digest = [0; 32];
        digest.copy_from_slice(&self.bytes[at + 80..at + 112]);
        Some(File {
            path: string(&self.bytes[at..at + 64]).ok()?,
            data: self.bytes.get(offset..offset.checked_add(size)?)?,
            digest,
        })
    }
    pub fn find(&self, path: &str) -> Option<File<'a>> {
        (0..self.count)
            .filter_map(|i| self.file(i))
            .find(|f| f.path == path)
    }
}

#[cfg(all(test, not(feature = "host-tools")))]
extern crate std;
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::vec::Vec;
    fn hex<const N: usize>(text: &str) -> [u8; N] {
        let mut out = [0; N];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }
    fn key() -> SigningKey {
        SigningKey::from_bytes(&hex(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        ))
    }
    pub(crate) fn anchor() -> TrustAnchor {
        let key = key().verifying_key().to_bytes();
        TrustAnchor {
            id: signer_id(&key),
            key,
            revoked: false,
            allowed_permissions: KNOWN_PERMISSIONS,
            allowed_kinds: 3,
        }
    }
    pub(crate) fn seal(bytes: &mut [u8]) {
        let end = HEADER_BYTES + u32_at(bytes, 32) as usize * ENTRY_BYTES;
        let signature = key().sign(&bytes[..end]);
        let len = bytes.len();
        bytes[len - 64..].copy_from_slice(&signature.to_bytes());
    }
    pub(crate) fn fixture() -> Vec<u8> {
        let files = [
            ("assets/help.txt", &b"help"[..]),
            ("bin/test.elf", &b"executable fixture"[..]),
        ];
        let mut bytes = std::vec![0;HEADER_BYTES+ENTRY_BYTES*2+4+18+64];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8] = 1;
        bytes[10] = 62;
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        let len = bytes.len() as u64;
        bytes[24..32].copy_from_slice(&len.to_le_bytes());
        bytes[32] = 2;
        bytes[36] = 1;
        bytes[40] = 1;
        bytes[52] = 1;
        bytes[64..96].copy_from_slice(&anchor().id);
        bytes[96..109].copy_from_slice(b"org.next.test");
        bytes[162] = 1;
        bytes[164] = 1;
        let mut offset = HEADER_BYTES + 2 * ENTRY_BYTES;
        for (i, (path, data)) in files.iter().enumerate() {
            let at = HEADER_BYTES + i * ENTRY_BYTES;
            bytes[at..at + path.len()].copy_from_slice(path.as_bytes());
            bytes[at + 64..at + 72].copy_from_slice(&(offset as u64).to_le_bytes());
            bytes[at + 72..at + 80].copy_from_slice(&(data.len() as u64).to_le_bytes());
            bytes[at + 80..at + 112].copy_from_slice(&sha256(data));
            bytes[offset..offset + data.len()].copy_from_slice(data);
            offset += data.len();
        }
        seal(&mut bytes);
        bytes
    }
    #[test]
    fn signed_application_pe_entry_allowed_but_deployment_pe_and_other_suffixes_rejected() {
        let mut bytes = fixture();
        let at = HEADER_BYTES + ENTRY_BYTES;
        bytes[at..at + 64].fill(0);
        bytes[at..at + 12].copy_from_slice(b"bin/test.exe");
        seal(&mut bytes);
        let package = VerifiedPackage::verify(&bytes, &[anchor()]).unwrap();
        assert_eq!(package.kind, PackageKind::Application);
        assert_eq!(package.executable().path, "bin/test.exe");
        bytes[at..at + 64].fill(0);
        bytes[at..at + 12].copy_from_slice(b"bin/test.bin");
        seal(&mut bytes);
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[anchor()]).unwrap_err(),
            Error::EntryPoint
        );
        bytes[164] = 2;
        bytes[at..at + 64].fill(0);
        bytes[at..at + 13].copy_from_slice(b"boot/test.exe");
        seal(&mut bytes);
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[anchor()]).unwrap_err(),
            Error::EntryPoint
        );
        bytes[at..at + 13].copy_from_slice(b"boot/test.elf");
        seal(&mut bytes);
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[anchor()]).unwrap().kind,
            PackageKind::Deployment
        );
    }
    #[test]
    fn published_sha256_and_rfc8032_vectors() {
        assert_eq!(
            sha256(b""),
            hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(
            sha256(b"abc"),
            hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        let public = hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let signature=hex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b");
        assert_eq!(verify_signature(&public, b"", &signature), Ok(()));
        assert_eq!(
            verify_signature(&public, b"tampered", &signature),
            Err(Error::Signature)
        );
        let public = hex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c");
        let signature=hex("92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00");
        assert_eq!(verify_signature(&public, &[0x72], &signature), Ok(()));
    }
    #[test]
    fn strict_verification_rejects_weak_keys_r_and_noncanonical_s() {
        let signature = key().sign(b"test").to_bytes();
        let mut weak = [0; 32];
        weak[0] = 1;
        assert_eq!(
            verify_signature(&weak, b"test", &signature),
            Err(Error::Signature)
        );
        let mut noncanonical = [0xff; 32];
        noncanonical[0] = 0xed;
        noncanonical[31] = 0x7f;
        assert_eq!(
            verify_signature(&noncanonical, b"test", &signature),
            Err(Error::Signature)
        );
        let mut bad = signature;
        bad[32..].fill(0xff);
        assert_eq!(
            verify_signature(&anchor().key, b"test", &bad),
            Err(Error::Signature)
        );
        let mut bad = signature;
        bad[..32].fill(0);
        bad[0] = 1;
        assert_eq!(
            verify_signature(&anchor().key, b"test", &bad),
            Err(Error::Signature)
        );
    }
    #[test]
    fn authenticates_all_content_and_metadata() {
        let bytes = fixture();
        let package = VerifiedPackage::verify(&bytes, &[anchor()]).unwrap();
        assert_eq!(package.id(), "org.next.test");
        assert_eq!(package.len(), 2);
        assert_eq!(package.executable().data, b"executable fixture");
        assert_eq!(package.find("assets/help.txt").unwrap().data, b"help");
        assert!(package.file(usize::MAX).is_none());
        let mut bad = bytes.clone();
        bad[HEADER_BYTES + 2 * ENTRY_BYTES] ^= 1;
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Digest
        );
        let mut bad = bytes.clone();
        bad[52] ^= 2;
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Signature
        );
        let mut bad = bytes;
        let end = bad.len();
        bad[end - 1] ^= 1;
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Signature
        );
    }
    #[test]
    fn explicit_trust_revocation_and_permission_ceilings() {
        let bytes = fixture();
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[]).unwrap_err(),
            Error::UnknownSigner
        );
        let mut trusted = anchor();
        trusted.revoked = true;
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[trusted]).unwrap_err(),
            Error::Revoked
        );
        let mut trusted = anchor();
        trusted.allowed_permissions = 0;
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[trusted]).unwrap_err(),
            Error::Permissions
        );
        let mut trusted = anchor();
        trusted.allowed_kinds = 2;
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[trusted]).unwrap_err(),
            Error::Permissions
        );
        let mut trusted = anchor();
        trusted.id[0] ^= 1;
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[trusted]).unwrap_err(),
            Error::Trust
        );
        assert_eq!(
            VerifiedPackage::verify(&bytes, &[anchor(), anchor()]).unwrap_err(),
            Error::Trust
        );
    }
    #[test]
    fn resigned_hostile_paths_aliases_reserved_bits_and_ranges_fail_closed() {
        for path in [
            "../escape",
            "/absolute",
            "bin/../escape",
            "bin//escape",
            "bin/./x",
            "bin/CON",
            "bin/con.txt",
            "bin/lpt1",
            "bin/x.",
            "bin/x:stream",
            "bin/x\\y",
        ] {
            assert!(!valid_path(path));
            let mut bad = fixture();
            bad[HEADER_BYTES..HEADER_BYTES + 64].fill(0);
            bad[HEADER_BYTES..HEADER_BYTES + path.len()].copy_from_slice(path.as_bytes());
            seal(&mut bad);
            assert!(VerifiedPackage::verify(&bad, &[anchor()]).is_err());
        }
        let mut bad = fixture();
        let first = bad[HEADER_BYTES..HEADER_BYTES + 64].to_vec();
        bad[HEADER_BYTES + ENTRY_BYTES..HEADER_BYTES + ENTRY_BYTES + 64].copy_from_slice(&first);
        seal(&mut bad);
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Duplicate
        );
        let mut bad = fixture();
        bad[HEADER_BYTES + 64..HEADER_BYTES + 72].copy_from_slice(&u64::MAX.to_le_bytes());
        seal(&mut bad);
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Size
        );
        let mut bad = fixture();
        bad[HEADER_BYTES + 112] = 1;
        seal(&mut bad);
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Reserved
        );
        let mut bad = fixture();
        bad[47] = 0x80;
        seal(&mut bad);
        assert_eq!(
            VerifiedPackage::verify(&bad, &[anchor()]).unwrap_err(),
            Error::Permissions
        );
    }
    #[test]
    fn every_truncated_prefix_and_bounded_mutations_are_rejected() {
        let bytes = fixture();
        for size in 0..bytes.len() {
            assert!(VerifiedPackage::verify(&bytes[..size], &[anchor()]).is_err());
        }
        for offset in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[offset] ^= 0x80;
            assert!(VerifiedPackage::verify(&bad, &[anchor()]).is_err());
        }
    }
}
