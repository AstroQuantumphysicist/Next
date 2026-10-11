//! Immutable authenticated executable identities for a future launch boundary.
//!
//! The caller supplies independent permission/capability grants. A signed
//! manifest is a ceiling, not authorization. This module neither launches a
//! process nor installs capabilities, persists trust, or revokes live objects.
//! Runtime users must retain this record with the process and call `authorize`
//! at their documented revocation boundary. Display names/paths do not define
//! identity; `bind_file` resolves only a path already authenticated in metadata.
use crate::{trust::TrustState, PackageKind, VerifiedPackage, KNOWN_PERMISSIONS};
use sha2::{Digest, Sha256};

/// Next wire rights, not helix-core's differently numbered internal mask.
pub const KNOWN_CAPABILITY_RIGHTS: u64 = 0xff;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub permissions: u64,
    pub capability_rights: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidTrust,
    UnknownSigner,
    Revoked,
    ExpiredGeneration,
    TrustRollback,
    Permissions,
    CapabilityRights,
    ExecutableDigest,
    FileNotFound,
    Package(crate::Error),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Principal {
    identity: [u8; 32],
    package_id: [u8; 64],
    id_len: u8,
    version: [u32; 3],
    kind: PackageKind,
    package_digest: [u8; 32],
    executable_digest: [u8; 32],
    signer: [u8; 32],
    trust_generation: u64,
    permissions: u64,
    capability_rights: u64,
}
fn validate_trust(state: TrustState<'_>) -> Result<(), Error> {
    if state.anchors.is_empty() || state.anchors.len() > 32 || state.revoked.len() > 64 {
        return Err(Error::InvalidTrust);
    }
    for (index, anchor) in state.anchors.iter().enumerate() {
        if crate::signer_id(&anchor.key) != anchor.id
            || anchor.allowed_permissions & !KNOWN_PERMISSIONS != 0
            || anchor.allowed_kinds == 0
            || anchor.allowed_kinds & !3 != 0
            || state.anchors[..index].iter().any(|old| old.id == anchor.id)
            || crate::validated_key(&anchor.key).is_err()
        {
            return Err(Error::InvalidTrust);
        }
    }
    for (index, revoked) in state.revoked.iter().enumerate() {
        if state.revoked[..index].contains(revoked) {
            return Err(Error::InvalidTrust);
        }
    }
    Ok(())
}
impl Principal {
    /// Bind the package's authenticated entrypoint to the exact loaded bytes.
    /// Reverification reads private signed bytes, not publicly mutable summary
    /// fields on VerifiedPackage. Capability grants remain caller policy.
    pub fn bind(
        package: &VerifiedPackage<'_>,
        loaded: &[u8],
        state: TrustState<'_>,
        grant: Grant,
    ) -> Result<Self, Error> {
        Self::bind_selected(package, None, loaded, state, grant)
    }
    /// Select another authenticated package file, independently of its current
    /// filesystem/display name. The caller still validates executable format.
    pub fn bind_file(
        package: &VerifiedPackage<'_>,
        manifest_path: &str,
        loaded: &[u8],
        state: TrustState<'_>,
        grant: Grant,
    ) -> Result<Self, Error> {
        Self::bind_selected(package, Some(manifest_path), loaded, state, grant)
    }
    fn bind_selected(
        package: &VerifiedPackage<'_>,
        manifest_path: Option<&str>,
        loaded: &[u8],
        state: TrustState<'_>,
        grant: Grant,
    ) -> Result<Self, Error> {
        validate_trust(state)?;
        let checked =
            VerifiedPackage::verify(package.bytes, state.anchors).map_err(|error| match error {
                crate::Error::Revoked => Error::Revoked,
                crate::Error::UnknownSigner => Error::UnknownSigner,
                other => Error::Package(other),
            })?;
        if state.revoked.contains(&checked.signer) {
            return Err(Error::Revoked);
        }
        if grant.permissions & !checked.permissions != 0 {
            return Err(Error::Permissions);
        }
        if grant.capability_rights & !KNOWN_CAPABILITY_RIGHTS != 0 {
            return Err(Error::CapabilityRights);
        }
        let executable = match manifest_path {
            Some(path) => checked.find(path).ok_or(Error::FileNotFound)?,
            None => checked.executable(),
        };
        if loaded.len() != executable.data.len() || crate::sha256(loaded) != executable.digest {
            return Err(Error::ExecutableDigest);
        }
        let mut package_id = [0; 64];
        let id = checked.id().as_bytes();
        package_id[..id.len()].copy_from_slice(id);
        let package_digest = crate::sha256(checked.bytes);
        let mut hash = Sha256::new();
        hash.update(b"NEXT-EXECUTABLE-IDENTITY-1\0");
        hash.update(checked.signer);
        hash.update(package_digest);
        hash.update(executable.digest);
        let principal = Self {
            identity: hash.finalize().into(),
            package_id,
            id_len: id.len() as u8,
            version: checked.version,
            kind: checked.kind,
            package_digest,
            executable_digest: executable.digest,
            signer: checked.signer,
            trust_generation: state.generation,
            permissions: grant.permissions,
            capability_rights: grant.capability_rights,
        };
        principal.authorize(state, grant.permissions, grant.capability_rights)?;
        Ok(principal)
    }
    /// Check current policy before granting/using rights. Any trust-generation
    /// change invalidates the old binding; the loader must rebind under the new
    /// authenticated policy. No implicit renewal or revocation grace period.
    pub fn authorize(
        &self,
        state: TrustState<'_>,
        permissions: u64,
        capability_rights: u64,
    ) -> Result<(), Error> {
        validate_trust(state)?;
        if state.revoked.contains(&self.signer) {
            return Err(Error::Revoked);
        }
        let anchor = state
            .anchors
            .iter()
            .find(|anchor| anchor.id == self.signer)
            .ok_or(Error::UnknownSigner)?;
        if anchor.revoked {
            return Err(Error::Revoked);
        }
        if state.generation < self.trust_generation {
            return Err(Error::TrustRollback);
        }
        if state.generation != self.trust_generation {
            return Err(Error::ExpiredGeneration);
        }
        if anchor.allowed_kinds & (1 << (self.kind as u16 - 1)) == 0
            || self.permissions & !anchor.allowed_permissions != 0
            || permissions & !self.permissions != 0
        {
            return Err(Error::Permissions);
        }
        if capability_rights & !self.capability_rights != 0 {
            return Err(Error::CapabilityRights);
        }
        Ok(())
    }
    pub fn validate_image(&self, loaded: &[u8]) -> Result<(), Error> {
        if loaded.len() > crate::MAX_FILE_BYTES || crate::sha256(loaded) != self.executable_digest {
            Err(Error::ExecutableDigest)
        } else {
            Ok(())
        }
    }
    pub const fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn package_id(&self) -> &str {
        core::str::from_utf8(&self.package_id[..self.id_len as usize]).unwrap_or("")
    }
    pub const fn version(&self) -> [u32; 3] {
        self.version
    }
    pub const fn kind(&self) -> PackageKind {
        self.kind
    }
    pub const fn package_digest(&self) -> [u8; 32] {
        self.package_digest
    }
    pub const fn executable_digest(&self) -> [u8; 32] {
        self.executable_digest
    }
    pub const fn signer(&self) -> [u8; 32] {
        self.signer
    }
    pub const fn trust_generation(&self) -> u64 {
        self.trust_generation
    }
    pub const fn permissions(&self) -> u64 {
        self.permissions
    }
    pub const fn capability_rights(&self) -> u64 {
        self.capability_rights
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(anchors: &[crate::TrustAnchor]) -> TrustState<'_> {
        TrustState {
            generation: 7,
            anchors,
            revoked: &[],
        }
    }
    fn grant() -> Grant {
        Grant {
            permissions: 1,
            capability_rights: 3,
        }
    }
    #[test]
    fn immutable_binding_checks_real_signed_entry_bytes_and_rename_is_irrelevant() {
        let bytes = crate::tests::fixture();
        let anchors = [crate::tests::anchor()];
        let package = VerifiedPackage::verify(&bytes, &anchors).unwrap();
        let executable = package.executable().data;
        let principal = Principal::bind(&package, executable, state(&anchors), grant()).unwrap();
        let copied_to_another_path = std::vec::Vec::from(executable);
        let renamed = Principal::bind(
            &package,
            &copied_to_another_path,
            state(&anchors),
            Grant {
                permissions: 0,
                capability_rights: 1,
            },
        )
        .unwrap();
        assert_eq!(principal.identity(), renamed.identity());
        assert_eq!(principal.package_id(), "org.next.test");
        assert_eq!(principal.executable_digest(), crate::sha256(executable));
        assert_eq!(principal.authorize(state(&anchors), 1, 1), Ok(()));
        assert_eq!(
            principal.authorize(state(&anchors), 2, 1),
            Err(Error::Permissions)
        );
        assert_eq!(
            principal.authorize(state(&anchors), 1, 4),
            Err(Error::CapabilityRights)
        );
        assert_eq!(
            renamed.authorize(state(&anchors), 1, 1),
            Err(Error::Permissions)
        );
        let mut changed = copied_to_another_path;
        changed[0] ^= 1;
        assert_eq!(
            principal.validate_image(&changed),
            Err(Error::ExecutableDigest)
        );
        assert_eq!(
            Principal::bind(&package, &changed, state(&anchors), grant()),
            Err(Error::ExecutableDigest)
        );
        assert_eq!(
            Principal::bind(
                &package,
                package.file(0).unwrap().data,
                state(&anchors),
                grant()
            ),
            Err(Error::ExecutableDigest)
        );
    }
    #[test]
    fn manifest_requests_and_mutable_summary_fields_cannot_grant_authority() {
        let bytes = crate::tests::fixture();
        let anchors = [crate::tests::anchor()];
        let mut package = VerifiedPackage::verify(&bytes, &anchors).unwrap();
        let executable = package.executable().data;
        assert_eq!(
            Principal::bind(
                &package,
                executable,
                state(&anchors),
                Grant {
                    permissions: 2,
                    capability_rights: 1
                }
            ),
            Err(Error::Permissions)
        );
        assert_eq!(
            Principal::bind(
                &package,
                executable,
                state(&anchors),
                Grant {
                    permissions: 1,
                    capability_rights: 256
                }
            ),
            Err(Error::CapabilityRights)
        );
        package.permissions = KNOWN_PERMISSIONS;
        package.signer = [0xff; 32];
        package.version = [999, 999, 999];
        package.kind = PackageKind::Deployment;
        let principal = Principal::bind(&package, executable, state(&anchors), grant()).unwrap();
        assert_eq!(principal.signer(), anchors[0].id);
        assert_eq!(principal.version(), [0, 1, 0]);
        assert_eq!(principal.kind(), PackageKind::Application);
        assert_eq!(
            Principal::bind(
                &package,
                executable,
                state(&anchors),
                Grant {
                    permissions: 2,
                    capability_rights: 1
                }
            ),
            Err(Error::Permissions)
        );
    }
    #[test]
    fn revoked_removed_rotated_and_rolled_back_trust_fail_closed() {
        let bytes = crate::tests::fixture();
        let anchors = [crate::tests::anchor()];
        let package = VerifiedPackage::verify(&bytes, &anchors).unwrap();
        let principal = Principal::bind(
            &package,
            package.executable().data,
            state(&anchors),
            grant(),
        )
        .unwrap();
        let mut changed = state(&anchors);
        changed.generation = 8;
        assert_eq!(
            principal.authorize(changed, 0, 0),
            Err(Error::ExpiredGeneration)
        );
        changed.generation = 6;
        assert_eq!(
            principal.authorize(changed, 0, 0),
            Err(Error::TrustRollback)
        );
        let revoked = [anchors[0].id];
        changed = state(&anchors);
        changed.revoked = &revoked;
        assert_eq!(principal.authorize(changed, 0, 0), Err(Error::Revoked));
        assert_eq!(
            Principal::bind(&package, package.executable().data, changed, grant()),
            Err(Error::Revoked)
        );
        let mut revoked_anchor = anchors;
        revoked_anchor[0].revoked = true;
        assert_eq!(
            principal.authorize(state(&revoked_anchor), 0, 0),
            Err(Error::Revoked)
        );
        let mut narrowed = anchors;
        narrowed[0].allowed_permissions = 0;
        assert_eq!(
            principal.authorize(state(&narrowed), 0, 0),
            Err(Error::Permissions)
        );
        let mut different = anchors;
        different[0].id[0] ^= 1;
        assert_eq!(
            principal.authorize(state(&different), 0, 0),
            Err(Error::InvalidTrust)
        );
    }
    #[test]
    fn changed_signed_package_identity_does_not_alias_same_executable() {
        let bytes = crate::tests::fixture();
        let anchors = [crate::tests::anchor()];
        let package = VerifiedPackage::verify(&bytes, &anchors).unwrap();
        let first = Principal::bind(
            &package,
            package.executable().data,
            state(&anchors),
            grant(),
        )
        .unwrap();
        let mut replacement = bytes;
        replacement[96..109].copy_from_slice(b"org.next.fake");
        crate::tests::seal(&mut replacement);
        let package = VerifiedPackage::verify(&replacement, &anchors).unwrap();
        let second = Principal::bind(
            &package,
            package.executable().data,
            state(&anchors),
            grant(),
        )
        .unwrap();
        assert_eq!(first.executable_digest(), second.executable_digest());
        assert_ne!(first.identity(), second.identity());
    }
    #[test]
    fn selected_file_must_belong_to_authenticated_manifest() {
        let bytes = crate::tests::fixture();
        let anchors = [crate::tests::anchor()];
        let package = VerifiedPackage::verify(&bytes, &anchors).unwrap();
        let data = package.file(0).unwrap().data;
        let selected =
            Principal::bind_file(&package, "assets/help.txt", data, state(&anchors), grant())
                .unwrap();
        assert_eq!(selected.executable_digest(), crate::sha256(data));
        assert_eq!(
            Principal::bind_file(&package, "bin/renamed.elf", data, state(&anchors), grant()),
            Err(Error::FileNotFound)
        );
        assert_eq!(
            Principal::bind_file(&package, "bin/test.elf", data, state(&anchors), grant()),
            Err(Error::ExecutableDigest)
        );
    }
}
