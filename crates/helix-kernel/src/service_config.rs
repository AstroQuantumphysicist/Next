//! Authenticated PID1 configuration delivery and independent worker identity
//! checks. Configuration is a signed member of the exact running nextd bundle;
//! a CRC or a mutable filesystem path never establishes service authority.
use next_abi::service::{Config, Entry};
use next_package::{identity::Principal, PackageKind, TrustAnchor, VerifiedPackage};
pub const MEMBER: &str = "config/services.bin";
pub const SUPERVISOR_ID: &str = "org.next.development.native.nextd";
static KEY: &[u8; 32] = include_bytes!(env!("NEXT_DEV_PUBLIC_KEY"));
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Package(next_package::Error),
    Principal,
    Missing,
    Configuration(next_abi::service::Error),
    Profile,
    Service,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }
    fn nextd() -> std::vec::Vec<u8> {
        fs::read(root().join("build/apps/system/nextd.elf")).unwrap()
    }
    fn principal(bundle: &[u8]) -> Principal {
        crate::application::load(
            bundle,
            crate::application::NATIVE_BASE_RIGHTS | next_abi::Rights::EXECUTE.0,
        )
        .unwrap()
        .principal
    }
    #[test]
    fn actual_signed_supervisor_member_binds_exact_worker_and_attenuation() {
        let bundle = nextd();
        let principal = principal(&bundle);
        let config = AuthenticatedConfig::load(&bundle, &principal).unwrap();
        assert!(config.recipient_matches(&principal));
        assert_eq!(Config::parse(config.bytes()).unwrap().active_mask(), 1);
        let entry = config.entry(0).unwrap();
        assert_eq!(entry.name, "worker-primary");
        assert_eq!(entry.capability_ceiling, 0x1b);
        assert!(config.entry(1).is_err());
        let worker = fs::read(root().join("build/apps/system/service-worker.elf")).unwrap();
        let worker = crate::application::load_with_grant(
            &worker,
            entry.permissions,
            entry.capability_ceiling,
        )
        .unwrap();
        assert!(AuthenticatedConfig::worker_matches(
            entry,
            &worker.principal
        ));
        assert_eq!(worker.principal.permissions(), 0);
        assert_eq!(worker.principal.capability_rights(), 0x1b);
        assert!(
            crate::application::authorize(&worker.principal, next_abi::Rights::EXECUTE.0).is_err()
        );
        let probe = fs::read(root().join("build/apps/system/probe.elf")).unwrap();
        let probe = crate::application::load(&probe, 0x1b).unwrap();
        assert!(!AuthenticatedConfig::worker_matches(
            entry,
            &probe.principal
        ));
        assert!(matches!(
            AuthenticatedConfig::load(&bundle, &probe.principal),
            Err(Error::Principal)
        ));
    }
    #[test]
    fn unsigned_and_tampered_configuration_never_establish_authority() {
        let mut bundle = nextd();
        let principal = principal(&bundle);
        let raw = fs::read(root().join("build/user/nextd.elf")).unwrap();
        assert!(matches!(
            AuthenticatedConfig::load(&raw, &principal),
            Err(Error::Package(_))
        ));
        let package = VerifiedPackage::verify(
            &bundle,
            &[TrustAnchor {
                id: next_package::signer_id(KEY),
                key: *KEY,
                revoked: false,
                allowed_permissions: 0x3f,
                allowed_kinds: 1,
            }],
        )
        .unwrap();
        let member = package.find(MEMBER).unwrap();
        let offset = member.data.as_ptr() as usize - bundle.as_ptr() as usize;
        bundle[offset + 24] ^= 2;
        assert!(matches!(
            AuthenticatedConfig::load(&bundle, &principal),
            Err(Error::Package(_))
        ));
    }
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn signed_config(config: &[u8]) -> std::vec::Vec<u8> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = Scratch(root().join("work").join(std::format!(
            "service-fixture-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(&dir.0).unwrap();
        let member = dir.0.join("services.bin");
        fs::write(&member, config).unwrap();
        let out = dir.0.join("nextd.nxp");
        let result = Command::new(root().join("target/debug/next-package"))
            .arg("pack")
            .arg(root().join("work/package-dev.seed"))
            .arg(SUPERVISOR_ID)
            .args(["0.1.0", "3f", "app", "bin/nextd.elf"])
            .arg(&out)
            .arg(std::format!(
                "bin/nextd.elf={}",
                root().join("build/user/nextd.elf").display()
            ))
            .arg(std::format!("config/services.bin={}", member.display()))
            .output()
            .unwrap();
        assert!(result.status.success());
        fs::read(out).unwrap()
    }
    #[test]
    fn valid_signature_cannot_hide_malformed_config_and_alternate_is_explicit() {
        let original = nextd();
        let initial = principal(&original);
        let mut config = AuthenticatedConfig::load(&original, &initial)
            .unwrap()
            .bytes()
            .to_vec();
        config[24] = 3;
        config[20..24].fill(0);
        let crc = next_abi::service::checksum(&config);
        config[20..24].copy_from_slice(&crc.to_le_bytes());
        let alternate = signed_config(&config);
        let alternate_principal = principal(&alternate);
        let authenticated = AuthenticatedConfig::load(&alternate, &alternate_principal).unwrap();
        assert_eq!(authenticated.entry(1).unwrap().dependencies, 1);
        assert!(!authenticated.recipient_matches(&initial));
        assert!(matches!(
            AuthenticatedConfig::load(&alternate, &initial),
            Err(Error::Principal)
        ));
        config[0] ^= 1;
        config[20..24].fill(0);
        let crc = next_abi::service::checksum(&config);
        config[20..24].copy_from_slice(&crc.to_le_bytes());
        let hostile = signed_config(&config);
        let hostile_principal = principal(&hostile);
        assert!(matches!(
            AuthenticatedConfig::load(&hostile, &hostile_principal),
            Err(Error::Configuration(_))
        ));
    }
}
#[derive(Clone, Copy)]
pub struct AuthenticatedConfig<'a> {
    bytes: &'a [u8],
    recipient: [u8; 32],
}
impl<'a> AuthenticatedConfig<'a> {
    pub fn load(bundle: &'a [u8], principal: &Principal) -> Result<Self, Error> {
        let anchor = TrustAnchor {
            id: next_package::signer_id(KEY),
            key: *KEY,
            revoked: false,
            allowed_permissions: next_package::KNOWN_PERMISSIONS,
            allowed_kinds: 1,
        };
        let package = VerifiedPackage::verify(bundle, &[anchor]).map_err(Error::Package)?;
        if package.kind != PackageKind::Application
            || package.id() != SUPERVISOR_ID
            || principal.package_id() != SUPERVISOR_ID
            || principal.package_digest() != next_package::sha256(bundle)
            || principal.executable_digest() != package.executable().digest
        {
            return Err(Error::Principal);
        }
        let bytes = package.find(MEMBER).ok_or(Error::Missing)?.data;
        let config = Config::parse(bytes).map_err(Error::Configuration)?;
        // Four process slots permit PID1, two audited workers and one console.
        // The shipped mask1 instead leaves two console slots. Larger declared
        // graphs require a separately implemented resource profile.
        if config.len() > 2 || config.active_mask().count_ones() > 2 {
            return Err(Error::Profile);
        }
        Ok(Self {
            bytes,
            recipient: principal.identity(),
        })
    }
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn recipient_matches(self, principal: &Principal) -> bool {
        self.recipient == principal.identity()
    }
    pub fn entry(self, index: usize) -> Result<Entry<'a>, Error> {
        let config = Config::parse(self.bytes).map_err(Error::Configuration)?;
        if !config.entry_enabled(index) {
            return Err(Error::Service);
        }
        config.entry(index).ok_or(Error::Service)
    }
    pub fn worker_matches(entry: Entry<'_>, principal: &Principal) -> bool {
        entry.matches_identity(
            principal.package_id(),
            &principal.executable_digest(),
            principal.permissions(),
            principal.capability_rights(),
        )
    }
}
