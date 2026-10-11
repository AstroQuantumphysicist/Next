//! Actual development-profile application load authentication and authority.
//! The immutable boot image explicitly preauthorizes these fixture classes;
//! this is not production user consent, portals, or persistent trust rotation.
use next_abi::Rights;
use next_package::{
    identity::{Grant, Principal},
    trust::TrustState,
    TrustAnchor, VerifiedPackage,
};
static DEVELOPMENT_KEY: &[u8; 32] = include_bytes!(env!("NEXT_DEV_PUBLIC_KEY"));
pub const DEVELOPMENT_MANIFEST_CEILING: u64 = next_package::KNOWN_PERMISSIONS;
pub const DEVELOPMENT_EXECUTION_GRANT: u64 = next_package::KNOWN_PERMISSIONS;
pub const NATIVE_BASE_RIGHTS: u64 = Rights::READ.0
    | Rights::WRITE.0
    | Rights::DUPLICATE.0
    | Rights::TRANSFER.0
    | Rights::WAIT.0
    | Rights::SIGNAL.0;
pub const COMPATIBILITY_BRIDGE_RIGHTS: u64 = Rights::READ.0 | Rights::WRITE.0;
fn anchor() -> TrustAnchor {
    let key = *DEVELOPMENT_KEY;
    TrustAnchor {
        id: next_package::signer_id(&key),
        key,
        revoked: false,
        allowed_permissions: DEVELOPMENT_MANIFEST_CEILING,
        allowed_kinds: 1,
    }
}
pub struct Loaded<'a> {
    pub payload: &'a [u8],
    pub principal: Principal,
    pub declared_permissions: u64,
}
/// Authenticating precedes PID allocation, task creation, page mutation, or
/// namespace access. There is deliberately no unsigned fallback.
pub fn load(
    bytes: &[u8],
    capability_ceiling: u64,
) -> Result<Loaded<'_>, next_package::identity::Error> {
    load_with_grant(bytes, DEVELOPMENT_EXECUTION_GRANT, capability_ceiling)
}
/// Service launch grants come from authenticated PID1 configuration, and can
/// only attenuate the development policy and signed package declarations.
pub fn load_with_grant(
    bytes: &[u8],
    permissions: u64,
    capability_ceiling: u64,
) -> Result<Loaded<'_>, next_package::identity::Error> {
    let anchors = [anchor()];
    let package =
        VerifiedPackage::verify(bytes, &anchors).map_err(next_package::identity::Error::Package)?;
    let payload = package.executable().data;
    let principal = Principal::bind(
        &package,
        payload,
        TrustState {
            generation: 0,
            anchors: &anchors,
            revoked: &[],
        },
        Grant {
            permissions: package.permissions & DEVELOPMENT_EXECUTION_GRANT & permissions,
            capability_rights: capability_ceiling,
        },
    )?;
    Ok(Loaded {
        payload,
        principal,
        declared_permissions: package.permissions,
    })
}
/// This boot's development trust state is immutable. The principal primitive
/// also supports revocations/generation expiry, but no mutable trust service is
/// installed here and no persisted runtime-revocation claim is made.
pub fn authorize(principal: &Principal, rights: u64) -> Result<(), next_package::identity::Error> {
    authorize_permissions(principal, 0, rights)
}
pub fn authorize_permissions(
    principal: &Principal,
    permissions: u64,
    rights: u64,
) -> Result<(), next_package::identity::Error> {
    let anchors = [anchor()];
    principal.authorize(
        TrustState {
            generation: 0,
            anchors: &anchors,
            revoked: &[],
        },
        permissions,
        rights,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> std::vec::Vec<u8> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/apps/system");
        std::fs::read(root.join(name))
            .expect("build signed application fixtures before host checks")
    }
    #[test]
    fn network_declaration_cannot_bypass_attenuated_execution_or_capability_grants() {
        let bundle = fixture("service-worker.elf");
        let declared_only = load_with_grant(&bundle, 0, 0x1b).unwrap();
        assert_eq!(declared_only.declared_permissions & 4, 4);
        assert_eq!(declared_only.principal.permissions(), 0);
        assert!(authorize_permissions(&declared_only.principal, 4, 1).is_err());
        assert!(authorize_permissions(&declared_only.principal, 4, 0).is_err());
        let granted = load_with_grant(&bundle, 4, 0x3b).unwrap();
        assert_eq!(authorize_permissions(&granted.principal, 4, 0x3b), Ok(()));
        assert!(authorize_permissions(&granted.principal, 4, next_abi::Rights::EXECUTE.0).is_err());
    }
    #[test]
    fn filesystem_declaration_requires_actual_execution_and_rights_grants() {
        let bundle = fixture("probe.elf");
        let declared_only = load_with_grant(&bundle, 0, 0x7b).unwrap();
        assert_eq!(declared_only.declared_permissions & 3, 3);
        assert!(authorize_permissions(&declared_only.principal, 1, 1).is_err());
        assert!(authorize_permissions(&declared_only.principal, 3, 3).is_err());
        let read_only = load_with_grant(&bundle, 1, 0x7b).unwrap();
        assert_eq!(authorize_permissions(&read_only.principal, 1, 1), Ok(()));
        assert!(authorize_permissions(&read_only.principal, 3, 3).is_err());
        let read_write = load_with_grant(&bundle, 3, 0x7b).unwrap();
        assert_eq!(authorize_permissions(&read_write.principal, 3, 3), Ok(()));
        assert!(authorize_permissions(&read_write.principal, 3, Rights::EXECUTE.0).is_err());
        let attenuated = load_with_grant(&bundle, 3, 1).unwrap();
        assert!(authorize_permissions(&attenuated.principal, 3, 3).is_err());
    }
    #[test]
    fn exact_guest_load_boundary_accepts_signed_payload_and_explicit_attenuation() {
        let bytes = fixture("probe.elf");
        let app = load(&bytes, NATIVE_BASE_RIGHTS).unwrap();
        assert_eq!(
            app.principal.package_id(),
            "org.next.development.native.probe"
        );
        assert!(next_abi::native::NativeImage::parse(app.payload).is_ok());
        assert_eq!(
            app.principal.executable_digest(),
            next_package::sha256(app.payload)
        );
        assert_eq!(authorize(&app.principal, Rights::WRITE.0), Ok(()));
        assert_eq!(
            authorize(&app.principal, Rights::EXECUTE.0),
            Err(next_package::identity::Error::CapabilityRights)
        );
        assert_eq!(
            authorize(&app.principal, Rights::ADMIN.0),
            Err(next_package::identity::Error::CapabilityRights)
        );
        let privileged = load(&bytes, NATIVE_BASE_RIGHTS | Rights::EXECUTE.0).unwrap();
        assert_eq!(authorize(&privileged.principal, Rights::EXECUTE.0), Ok(()));
        assert_eq!(privileged.principal.identity(), app.principal.identity());
    }
    #[test]
    fn actual_archived_tampered_envelope_and_unsigned_payload_are_rejected() {
        let bytes = fixture("probe.elf");
        let app = load(&bytes, NATIVE_BASE_RIGHTS).unwrap();
        assert!(load(app.payload, NATIVE_BASE_RIGHTS).is_err());
        let tampered = fixture("untrusted.elf");
        assert!(matches!(
            load(&tampered, NATIVE_BASE_RIGHTS),
            Err(next_package::identity::Error::Package(
                next_package::Error::Signature
            ))
        ));
        let mut altered = bytes;
        let last_content = altered.len() - next_package::SIGNATURE_BYTES - 1;
        altered[last_content] ^= 1;
        assert!(load(&altered, NATIVE_BASE_RIGHTS).is_err());
    }
}
