//! Host development installer. The caller owns a private, trusted directory.
//! No scripts execute, permissions are not granted, and retained deployments
//! support rollback. A single atomic registry rename commits each transaction.
use crate::{Error as VerifyError, PackageKind, TrustAnchor, VerifiedPackage, MAX_BUNDLE_BYTES};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    prelude::v1::*,
};
const MAX_INSTALLED: usize = 16;
const MAX_REGISTRY_BYTES: usize = 4096;
#[derive(Debug)]
pub enum Error {
    Verify(VerifyError),
    Io(std::io::Error),
    Indeterminate(std::io::Error),
    UnsafeRoot,
    InvalidRegistry,
    AlreadyInstalled,
    NotInstalled,
    WrongKind,
    Full,
    Conflict,
    CorruptDeployment,
    NoRollback,
    InjectedFailure,
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitPoint {
    Staged,
    BeforeRegistryCommit,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub current: [u8; 32],
    pub previous: Option<[u8; 32]>,
}
struct Registry {
    generation: u64,
    packages: BTreeMap<String, Installed>,
}
pub struct Installer {
    root: PathBuf,
}
struct Lock {
    path: PathBuf,
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| std::format!("{b:02x}")).collect()
}
fn digest(text: &str) -> Result<[u8; 32], Error> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(Error::InvalidRegistry);
    }
    let mut out = [0; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| Error::InvalidRegistry)?;
    }
    Ok(out)
}
fn regular(path: &Path) -> Result<(), Error> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err(Error::UnsafeRoot);
    }
    Ok(())
}
fn directory(path: &Path) -> Result<(), Error> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_type().is_symlink() || m.permissions().mode() & 0o022 != 0 {
        return Err(Error::UnsafeRoot);
    }
    Ok(())
}
fn read(path: &Path, max: usize) -> Result<Vec<u8>, Error> {
    regular(path)?;
    let mut out = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() > max {
        return Err(Error::CorruptDeployment);
    }
    Ok(out)
}
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}
impl Installer {
    /// Authenticate before consulting the destination. This never creates a
    /// root implicitly, even when the caller supplied an invalid signing key.
    pub fn install_at(
        root: &Path,
        bytes: &[u8],
        anchors: &[TrustAnchor],
        update: bool,
    ) -> Result<(), Error> {
        let package = VerifiedPackage::verify(bytes, anchors).map_err(Error::Verify)?;
        if package.kind != PackageKind::Application {
            return Err(Error::WrongKind);
        }
        let installer = Self::open(root)?;
        if update {
            installer.update(bytes, anchors)
        } else {
            installer.install(bytes, anchors)
        }
    }
    /// Open an existing private root; never create or choose it implicitly.
    /// Caller must own it and protect all ancestors from hostile rename races.
    pub fn open(root: &Path) -> Result<Self, Error> {
        directory(root)?;
        let root = root.canonicalize()?;
        directory(&root)?;
        Ok(Self { root })
    }
    fn lock(&self) -> Result<Lock, Error> {
        directory(&self.root)?;
        let path = self.root.join(".install.lock");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok(Lock { path })
    }
    fn load(&self) -> Result<Registry, Error> {
        let path = self.root.join("registry");
        if !path.try_exists()? {
            return Ok(Registry {
                generation: 0,
                packages: BTreeMap::new(),
            });
        }
        let bytes = read(&path, MAX_REGISTRY_BYTES)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::InvalidRegistry)?;
        if !text.ends_with('\n') {
            return Err(Error::InvalidRegistry);
        }
        let mut lines = text.lines();
        if lines.next() != Some("NXTREG01") {
            return Err(Error::InvalidRegistry);
        }
        let generation = lines
            .next()
            .ok_or(Error::InvalidRegistry)?
            .parse::<u64>()
            .map_err(|_| Error::InvalidRegistry)?;
        let mut packages = BTreeMap::new();
        let mut previous = "";
        for line in lines {
            let mut fields = line.split(' ');
            let id = fields.next().ok_or(Error::InvalidRegistry)?;
            if !crate::valid_id(id) || id <= previous {
                return Err(Error::InvalidRegistry);
            }
            previous = id;
            let current = digest(fields.next().ok_or(Error::InvalidRegistry)?)?;
            let old = fields.next().ok_or(Error::InvalidRegistry)?;
            let prior = if old == "-" { None } else { Some(digest(old)?) };
            if fields.next().is_some() || packages.len() == MAX_INSTALLED {
                return Err(Error::InvalidRegistry);
            }
            packages.insert(
                id.to_string(),
                Installed {
                    id: id.to_string(),
                    current,
                    previous: prior,
                },
            );
        }
        Ok(Registry {
            generation,
            packages,
        })
    }
    fn commit(&self, registry: Registry) -> Result<(), Error> {
        self.commit_with_hooks(registry, || Ok(()), sync_directory)
    }
    fn commit_with_hooks<BeforeRename, SyncRoot>(
        &self,
        mut registry: Registry,
        before_rename: BeforeRename,
        sync_root: SyncRoot,
    ) -> Result<(), Error>
    where
        BeforeRename: FnOnce() -> Result<(), Error>,
        SyncRoot: FnOnce(&Path) -> std::io::Result<()>,
    {
        registry.generation = registry
            .generation
            .checked_add(1)
            .ok_or(Error::InvalidRegistry)?;
        let mut bytes = std::format!("NXTREG01\n{}\n", registry.generation);
        for entry in registry.packages.values() {
            bytes.push_str(&std::format!(
                "{} {} {}\n",
                entry.id,
                hex(&entry.current),
                entry
                    .previous
                    .map(|b| hex(&b))
                    .unwrap_or_else(|| "-".into())
            ));
        }
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err(Error::Full);
        }
        let temporary = self.root.join(".registry.next");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(bytes.as_bytes())?;
            file.sync_all()?;
            before_rename()?;
            fs::rename(&temporary, self.root.join("registry"))?;
            sync_root(&self.root).map_err(Error::Indeterminate)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    fn deployment(&self, hash: &[u8; 32]) -> PathBuf {
        self.root.join("deployments").join(hex(hash))
    }
    fn verify_deployment(
        &self,
        expected: &[u8; 32],
        id: &str,
        anchors: &[TrustAnchor],
    ) -> Result<(), Error> {
        directory(&self.root.join("deployments"))?;
        let root = self.deployment(expected);
        directory(&root)?;
        let bytes = read(&root.join("bundle.nextapp"), MAX_BUNDLE_BYTES)?;
        if crate::sha256(&bytes) != *expected {
            return Err(Error::CorruptDeployment);
        }
        let package = VerifiedPackage::verify(&bytes, anchors).map_err(Error::Verify)?;
        if package.kind != PackageKind::Application || package.id() != id {
            return Err(Error::CorruptDeployment);
        }
        directory(&root.join("files"))?;
        for i in 0..package.len() {
            let file = package.file(i).ok_or(Error::CorruptDeployment)?;
            let mut parent = root.join("files");
            let parts: Vec<_> = file.path.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                parent.push(part);
                directory(&parent)?;
            }
            let copied = read(&root.join("files").join(file.path), file.data.len())?;
            if copied != file.data {
                return Err(Error::CorruptDeployment);
            }
        }
        Ok(())
    }
    fn stage(
        &self,
        bytes: &[u8],
        package: &VerifiedPackage<'_>,
        hash: &[u8; 32],
        anchors: &[TrustAnchor],
    ) -> Result<(), Error> {
        let deployments = self.root.join("deployments");
        if !deployments.try_exists()? {
            fs::create_dir(&deployments)?;
        }
        directory(&deployments)?;
        let final_root = self.deployment(hash);
        if final_root.try_exists()? {
            return self.verify_deployment(hash, package.id(), anchors);
        }
        // Directory names are authenticated content digests, never archive paths.
        let temporary = deployments.join(std::format!(".{}.staging", hex(hash)));
        fs::create_dir(&temporary)?;
        let _cleanup = StageCleanup(temporary.clone());
        let mut bundle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(temporary.join("bundle.nextapp"))?;
        bundle.write_all(bytes)?;
        bundle.sync_all()?;
        let files = temporary.join("files");
        fs::create_dir(&files)?;
        let mut created_directories = Vec::new();
        for i in 0..package.len() {
            let input = package.file(i).ok_or(Error::CorruptDeployment)?;
            let path = files.join(input.path);
            let parent = path.parent().ok_or(Error::Conflict)?;
            let mut current = files.clone();
            for part in input
                .path
                .split('/')
                .take(input.path.split('/').count() - 1)
            {
                current.push(part);
                if !current.try_exists()? {
                    fs::create_dir(&current)?;
                    created_directories.push(current.clone());
                }
                directory(&current)?;
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|_| Error::Conflict)?;
            output.write_all(input.data)?;
            output.sync_all()?;
            sync_directory(parent)?;
        }
        for directory in created_directories.iter().rev() {
            sync_directory(directory)?;
        }
        sync_directory(&files)?;
        sync_directory(&temporary)?;
        fs::rename(&temporary, &final_root)?;
        sync_directory(&deployments)?;
        self.verify_deployment(hash, package.id(), anchors)
    }
    /// Signature/content verification precedes even acquiring the mutation lock.
    pub fn install(&self, bytes: &[u8], anchors: &[TrustAnchor]) -> Result<(), Error> {
        self.install_with_hook(bytes, anchors, false, |_| Ok(()))
    }
    pub fn update(&self, bytes: &[u8], anchors: &[TrustAnchor]) -> Result<(), Error> {
        self.install_with_hook(bytes, anchors, true, |_| Ok(()))
    }
    pub fn install_with_hook<F>(
        &self,
        bytes: &[u8],
        anchors: &[TrustAnchor],
        update: bool,
        mut hook: F,
    ) -> Result<(), Error>
    where
        F: FnMut(CommitPoint) -> Result<(), Error>,
    {
        let package = VerifiedPackage::verify(bytes, anchors).map_err(Error::Verify)?;
        if package.kind != PackageKind::Application {
            return Err(Error::WrongKind);
        }
        let _lock = self.lock()?;
        let mut registry = self.load()?;
        let prior = registry.packages.get(package.id()).map(|p| p.current);
        if update && prior.is_none() {
            return Err(Error::NotInstalled);
        }
        if !update && prior.is_some() {
            return Err(Error::AlreadyInstalled);
        }
        if prior.is_none() && registry.packages.len() == MAX_INSTALLED {
            return Err(Error::Full);
        }
        let current = crate::sha256(bytes);
        self.stage(bytes, &package, &current, anchors)?;
        hook(CommitPoint::Staged)?;
        registry.packages.insert(
            package.id().to_string(),
            Installed {
                id: package.id().into(),
                current,
                previous: prior,
            },
        );
        hook(CommitPoint::BeforeRegistryCommit)?;
        self.commit(registry)
    }
    pub fn uninstall(&self, id: &str) -> Result<(), Error> {
        if !crate::valid_id(id) {
            return Err(Error::InvalidRegistry);
        }
        let _lock = self.lock()?;
        let mut registry = self.load()?;
        if registry.packages.remove(id).is_none() {
            return Err(Error::NotInstalled);
        }
        self.commit(registry)
    }
    pub fn rollback(&self, id: &str, anchors: &[TrustAnchor]) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut registry = self.load()?;
        let entry = registry.packages.get_mut(id).ok_or(Error::NotInstalled)?;
        let previous = entry.previous.ok_or(Error::NoRollback)?;
        self.verify_deployment(&previous, id, anchors)?;
        entry.previous = Some(entry.current);
        entry.current = previous;
        self.commit(registry)
    }
    pub fn list(&self) -> Result<Vec<Installed>, Error> {
        let _lock = self.lock()?;
        Ok(self.load()?.packages.into_values().collect())
    }
    /// Resolve a verified installed payload without executing it or granting caps.
    pub fn file(&self, id: &str, path: &str, anchors: &[TrustAnchor]) -> Result<Vec<u8>, Error> {
        if !crate::valid_path(path) {
            return Err(Error::Conflict);
        }
        let _lock = self.lock()?;
        let registry = self.load()?;
        let entry = registry.packages.get(id).ok_or(Error::NotInstalled)?;
        self.verify_deployment(&entry.current, id, anchors)?;
        let bytes = read(
            &self.deployment(&entry.current).join("bundle.nextapp"),
            MAX_BUNDLE_BYTES,
        )?;
        let package = VerifiedPackage::verify(&bytes, anchors).map_err(Error::Verify)?;
        let file = package.find(path).ok_or(Error::Conflict)?;
        Ok(file.data.to_vec())
    }
}
struct StageCleanup(PathBuf);
impl Drop for StageCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{symlink, MetadataExt},
        sync::atomic::{AtomicU64, Ordering},
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let path = std::env::temp_dir().join(std::format!(
                "next-installer-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn installer(&self) -> Installer {
            Installer::open(&self.0).unwrap()
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn bundle(id: &str, payload: &[u8; 4], version: u32) -> Vec<u8> {
        let mut bytes = crate::tests::fixture();
        bytes[96..160].fill(0);
        bytes[96..96 + id.len()].copy_from_slice(id.as_bytes());
        bytes[56..60].copy_from_slice(&version.to_le_bytes());
        let at = crate::HEADER_BYTES;
        bytes[at + 80..at + 112].copy_from_slice(&crate::sha256(payload));
        let data = crate::HEADER_BYTES + 2 * crate::ENTRY_BYTES;
        bytes[data..data + 4].copy_from_slice(payload);
        crate::tests::seal(&mut bytes);
        bytes
    }
    fn staged_update(installer: &Installer, bytes: &[u8], anchors: &[TrustAnchor]) -> Registry {
        let package = VerifiedPackage::verify(bytes, anchors).unwrap();
        let current = crate::sha256(bytes);
        installer.stage(bytes, &package, &current, anchors).unwrap();
        let mut registry = installer.load().unwrap();
        let previous = registry.packages.get(package.id()).unwrap().current;
        registry.packages.insert(
            package.id().to_string(),
            Installed {
                id: package.id().into(),
                current,
                previous: Some(previous),
            },
        );
        registry
    }
    #[test]
    fn installation_update_rollback_and_uninstall_persist_registry() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let old = bundle("org.next.test", b"old!", 0);
        let new = bundle("org.next.test", b"new!", 1);
        installer.install(&old, &anchors).unwrap();
        assert_eq!(
            installer
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"old!"
        );
        assert!(matches!(
            installer.install(&old, &anchors),
            Err(Error::AlreadyInstalled)
        ));
        installer.update(&new, &anchors).unwrap();
        assert_eq!(
            root.installer()
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"new!"
        );
        installer.rollback("org.next.test", &anchors).unwrap();
        assert_eq!(
            installer
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"old!"
        );
        installer.uninstall("org.next.test").unwrap();
        assert!(root.installer().list().unwrap().is_empty());
        assert!(matches!(
            installer.file("org.next.test", "assets/help.txt", &anchors),
            Err(Error::NotInstalled)
        ));
        assert!(installer.deployment(&crate::sha256(&old)).is_dir());
        assert!(installer.deployment(&crate::sha256(&new)).is_dir());
    }
    #[test]
    fn invalid_signatures_cannot_create_locks_staging_or_registry() {
        let root = Temporary::new();
        let installer = root.installer();
        let mut bad = bundle("org.next.test", b"old!", 0);
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(matches!(
            installer.install(&bad, &[crate::tests::anchor()]),
            Err(Error::Verify(_))
        ));
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
        let good = bundle("org.next.test", b"old!", 0);
        assert!(matches!(
            installer.install(&good, &[]),
            Err(Error::Verify(VerifyError::UnknownSigner))
        ));
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    }
    #[test]
    fn interrupted_staging_or_commit_keeps_previous_active_package() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let old = bundle("org.next.test", b"old!", 0);
        let new = bundle("org.next.test", b"new!", 1);
        installer.install(&old, &anchors).unwrap();
        let registry = fs::read(root.0.join("registry")).unwrap();
        for point in [CommitPoint::Staged, CommitPoint::BeforeRegistryCommit] {
            let result = installer.install_with_hook(&new, &anchors, true, |at| {
                if at == point {
                    Err(Error::InjectedFailure)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err(Error::InjectedFailure)));
            assert_eq!(fs::read(root.0.join("registry")).unwrap(), registry);
            assert_eq!(
                installer
                    .file("org.next.test", "assets/help.txt", &anchors)
                    .unwrap(),
                b"old!"
            );
        }
        installer.update(&new, &anchors).unwrap();
        assert_eq!(
            installer
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"new!"
        );
    }
    #[test]
    fn failed_synced_registry_temporary_preserves_active_version_and_cleans_up() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let old = bundle("org.next.test", b"old!", 0);
        let new = bundle("org.next.test", b"new!", 1);
        installer.install(&old, &anchors).unwrap();
        let registry = fs::read(root.0.join("registry")).unwrap();

        let result = installer.commit_with_hooks(
            staged_update(&installer, &new, &anchors),
            || Err(Error::InjectedFailure),
            sync_directory,
        );

        assert!(matches!(result, Err(Error::InjectedFailure)));
        assert_eq!(fs::read(root.0.join("registry")).unwrap(), registry);
        assert!(!root.0.join(".registry.next").exists());
        assert_eq!(
            root.installer()
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"old!"
        );

        installer.update(&new, &anchors).unwrap();
        assert_eq!(
            root.installer()
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"new!"
        );
    }
    #[test]
    fn directory_sync_failure_after_registry_rename_is_indeterminate_but_consistent() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let old = bundle("org.next.test", b"old!", 0);
        let new = bundle("org.next.test", b"new!", 1);
        installer.install(&old, &anchors).unwrap();

        let result = installer.commit_with_hooks(
            staged_update(&installer, &new, &anchors),
            || Ok(()),
            |_| Err(std::io::Error::other("injected directory sync failure")),
        );

        assert!(matches!(result, Err(Error::Indeterminate(_))));
        let reopened = root.installer();
        assert_eq!(
            reopened
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"new!"
        );
        assert!(!root.0.join(".registry.next").exists());
    }
    #[test]
    fn app_path_ownership_isolated_and_conflicts_cannot_replace_existing_data() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let first = bundle("org.next.test", b"one!", 0);
        let second = bundle("org.next.demo", b"two!", 0);
        installer.install(&first, &anchors).unwrap();
        installer.install(&second, &anchors).unwrap();
        assert_eq!(
            installer
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"one!"
        );
        assert_eq!(
            installer
                .file("org.next.demo", "assets/help.txt", &anchors)
                .unwrap(),
            b"two!"
        );
        assert!(matches!(
            installer.file("org.next.test", "../registry", &anchors),
            Err(Error::Conflict)
        ));
        installer.uninstall("org.next.demo").unwrap();
        assert_eq!(
            installer
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"one!"
        );
        assert!(matches!(
            installer.update(&second, &anchors),
            Err(Error::NotInstalled)
        ));
    }
    #[test]
    fn path_prefix_conflicts_are_rejected_before_staging_and_outputs_are_unlinked_regular_files() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];

        let mut conflicting = crate::tests::fixture();
        let first = crate::HEADER_BYTES;
        let second = crate::HEADER_BYTES + crate::ENTRY_BYTES;
        conflicting[first..first + 64].fill(0);
        conflicting[first..first + b"bin/test.elf".len()].copy_from_slice(b"bin/test.elf");
        conflicting[second..second + 64].fill(0);
        conflicting[second..second + b"bin/test.elf/child".len()]
            .copy_from_slice(b"bin/test.elf/child");
        conflicting[36..40].copy_from_slice(&0_u32.to_le_bytes());
        crate::tests::seal(&mut conflicting);

        assert!(matches!(
            installer.install(&conflicting, &anchors),
            Err(Error::Verify(VerifyError::Duplicate))
        ));
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);

        let valid = bundle("org.next.test", b"safe", 0);
        installer.install(&valid, &anchors).unwrap();
        let files = installer.deployment(&crate::sha256(&valid)).join("files");
        let help = fs::symlink_metadata(files.join("assets/help.txt")).unwrap();
        let executable = fs::symlink_metadata(files.join("bin/test.elf")).unwrap();
        assert!(help.file_type().is_file());
        assert!(executable.file_type().is_file());
        assert_eq!(help.nlink(), 1);
        assert_eq!(executable.nlink(), 1);
        assert_ne!(help.ino(), executable.ino());
    }
    #[test]
    fn private_root_symlinks_and_corrupted_extracted_files_fail_closed() {
        let root = Temporary::new();
        let link = root.0.join("alias");
        symlink(&root.0, &link).unwrap();
        assert!(matches!(Installer::open(&link), Err(Error::UnsafeRoot)));
        fs::remove_file(link).unwrap();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let bytes = bundle("org.next.test", b"old!", 0);
        installer.install(&bytes, &anchors).unwrap();
        let deployment = installer.deployment(&crate::sha256(&bytes));
        let file = deployment.join("files/assets/help.txt");
        fs::write(&file, b"evil").unwrap();
        assert!(matches!(
            installer.file("org.next.test", "assets/help.txt", &anchors),
            Err(Error::CorruptDeployment)
        ));
        fs::remove_file(&file).unwrap();
        symlink(root.0.join("registry"), &file).unwrap();
        assert!(matches!(
            installer.file("org.next.test", "assets/help.txt", &anchors),
            Err(Error::UnsafeRoot)
        ));
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(Installer::open(&root.0), Err(Error::UnsafeRoot)));
    }
    #[test]
    fn revoked_rollback_and_corrupt_registry_never_mutate_active_state() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        installer
            .install(&bundle("org.next.test", b"old!", 0), &anchors)
            .unwrap();
        installer
            .update(&bundle("org.next.test", b"new!", 1), &anchors)
            .unwrap();
        let registry = fs::read(root.0.join("registry")).unwrap();
        let mut revoked = crate::tests::anchor();
        revoked.revoked = true;
        assert!(matches!(
            installer.rollback("org.next.test", &[revoked]),
            Err(Error::Verify(VerifyError::Revoked))
        ));
        assert_eq!(fs::read(root.0.join("registry")).unwrap(), registry);
        fs::write(root.0.join("registry"), b"NXTREG01\n1\n../escape 00 -\n").unwrap();
        assert!(matches!(
            installer.uninstall("org.next.test"),
            Err(Error::InvalidRegistry)
        ));
        assert_eq!(
            fs::read(root.0.join("registry")).unwrap(),
            b"NXTREG01\n1\n../escape 00 -\n"
        );
    }
    #[test]
    fn wrong_key_is_rejected_before_consulting_or_creating_destination() {
        let root = Temporary::new();
        let absent = root.0.join("not-created");
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32])
            .verifying_key()
            .to_bytes();
        let anchor = TrustAnchor {
            id: crate::signer_id(&key),
            key,
            revoked: false,
            allowed_permissions: crate::KNOWN_PERMISSIONS,
            allowed_kinds: 3,
        };
        assert!(matches!(
            Installer::install_at(
                &absent,
                &bundle("org.next.test", b"old!", 0),
                &[anchor],
                false
            ),
            Err(Error::Verify(VerifyError::UnknownSigner))
        ));
        assert!(!absent.exists());
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    }
    #[test]
    fn corruption_of_previous_deployment_blocks_rollback_without_registry_change() {
        let root = Temporary::new();
        let installer = root.installer();
        let anchors = [crate::tests::anchor()];
        let old = bundle("org.next.test", b"old!", 0);
        let new = bundle("org.next.test", b"new!", 1);
        installer.install(&old, &anchors).unwrap();
        installer.update(&new, &anchors).unwrap();
        fs::write(
            installer
                .deployment(&crate::sha256(&old))
                .join("files/assets/help.txt"),
            b"evil",
        )
        .unwrap();
        let registry = fs::read(root.0.join("registry")).unwrap();
        assert!(matches!(
            installer.rollback("org.next.test", &anchors),
            Err(Error::CorruptDeployment)
        ));
        assert_eq!(fs::read(root.0.join("registry")).unwrap(), registry);
        assert_eq!(
            installer
                .file("org.next.test", "assets/help.txt", &anchors)
                .unwrap(),
            b"new!"
        );
    }
}
