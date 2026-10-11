//! Owner-authenticated GPT storage policy. Raw initrd CRC never authorizes writes.
use next_fs::gpt;
use next_fs::storage_policy as schema;
use next_package::{PackageKind, TrustAnchor, VerifiedPackage};
const KEY: &[u8; 32] = include_bytes!(env!("NEXT_DEV_PUBLIC_KEY"));
pub const MEMBER: &str = "config/storage.bin";
const SUPERVISOR: &str = "org.next.development.native.nextd";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Archive(next_abi::initrd::InitError),
    Package(next_package::Error),
    Supervisor,
    Missing,
    Policy(schema::Error),
}
/// Opaque authenticated declaration; no constructor from raw schema bytes.
pub struct AuthenticatedPolicy {
    policy: schema::Policy,
}
impl AuthenticatedPolicy {
    pub fn load(initrd: &[u8]) -> Result<Self, Error> {
        let image = next_abi::initrd::InitImage::parse(initrd).map_err(Error::Archive)?;
        let supervisor = image.find("/system/nextd.elf").ok_or(Error::Missing)?;
        let anchor = TrustAnchor {
            id: next_package::signer_id(KEY),
            key: *KEY,
            revoked: false,
            allowed_permissions: next_package::KNOWN_PERMISSIONS,
            allowed_kinds: 1,
        };
        let package =
            VerifiedPackage::verify(supervisor.data, &[anchor]).map_err(Error::Package)?;
        if package.kind != PackageKind::Application
            || package.id() != SUPERVISOR
            || package.executable().path != "bin/nextd.elf"
        {
            return Err(Error::Supervisor);
        }
        let member = package.find(MEMBER).ok_or(Error::Missing)?;
        Ok(Self {
            policy: schema::Policy::parse(member.data).map_err(Error::Policy)?,
        })
    }
    pub fn bind(
        self,
        disk: gpt::Guid,
        data: gpt::Partition,
        capacity: u64,
    ) -> Result<AuthenticatedExtent, Error> {
        let extent = self
            .policy
            .bind(disk, data, capacity)
            .map_err(Error::Policy)?;
        Ok(AuthenticatedExtent { extent, capacity })
    }
}
/// Only signature verification and exact GPT binding can construct this grant.
/// It permits mounting a preformatted journal, never formatting/repairing one.
pub struct AuthenticatedExtent {
    extent: schema::Extent,
    capacity: u64,
}
impl AuthenticatedExtent {
    pub fn start(&self) -> u64 {
        self.extent.start
    }
    pub fn sectors(&self) -> u64 {
        self.extent.sectors
    }
    pub fn writable(&self) -> bool {
        self.extent.writable
    }
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
}

/// The shared, testable mount transaction. Authority covers only the fixed
/// prefix; mount performs reads only, and failure never invokes format/repair.
pub fn mount_existing<'a, D: next_fs::journal::WritableBlockDevice>(
    device: &'a mut D,
    extent: &AuthenticatedExtent,
) -> Result<next_fs::journal::Store<'a, D>, next_fs::journal::Error<D::Error>> {
    use next_fs::journal::{Error as JournalError, Partition, Store, REQUIRED_SECTORS};
    let start = extent.start();
    let count = extent.sectors();
    let capacity = device.sector_count();
    let end = start.checked_add(count).ok_or(JournalError::Geometry)?;
    if capacity != extent.capacity() || count != REQUIRED_SECTORS || end > capacity {
        return Err(JournalError::Geometry);
    }
    let partition = Partition::new(device, start, count, &[(0, start), (end, capacity - end)])?;
    let mut store = Store::mount(partition)?;
    if !extent.writable() {
        store.restrict_read_only();
    }
    Ok(store)
}
