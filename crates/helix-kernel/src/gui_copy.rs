//! Conditional bounded file copy. Caller must hold the existing filesystem guard
//! across validation, source read, durable commit and coherent cache publication.
//! This module is not wired to a GUI action yet; it grants no application rights.
use helix_core::file::{self, Backend, Files, FILE_BYTES};
use next_fs::journal::{self, Store, WritableBlockDevice};
use prism::files_actions::{BackendError, Path, Snapshot};
// Matches the sole development-volume mount policy identity in filesystem.rs.
const DIRECTORY_OWNER: u64 = 0x4e45_5854_4445_5631;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CopyRequest {
    pub source: Snapshot,
    pub destination: Path,
    pub replace: Option<Snapshot>,
}
fn map_error<E>(error: journal::Error<E>) -> BackendError {
    match error {
        journal::Error::ReadOnly => BackendError::ReadOnly,
        journal::Error::NotFound => BackendError::NotFound,
        journal::Error::Exists => BackendError::Exists,
        journal::Error::Full => BackendError::Full,
        journal::Error::InvalidPath | journal::Error::TooLarge | journal::Error::BufferTooSmall => {
            BackendError::InvalidPath
        }
        _ => BackendError::Io,
    }
}
fn checked_native_leaf<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    path: Path,
) -> Result<Option<next_fs::journal::directories::Entry>, BackendError> {
    use next_fs::journal::directories::{self, Kind};
    if !path.as_str().starts_with("/native/") {
        return Err(BackendError::ReadOnly);
    }
    let parent = path
        .as_str()
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    let listing = directories::list(store, DIRECTORY_OWNER, parent).map_err(directory_error)?;
    let leaf = listing
        .entries
        .iter()
        .take(listing.count)
        .find(|entry| entry.path() == path.as_str())
        .copied();
    if leaf.is_some_and(|entry| entry.kind != Kind::File) {
        return Err(BackendError::InvalidPath);
    }
    Ok(leaf)
}
fn directory_error<E>(error: next_fs::journal::directories::Error<E>) -> BackendError {
    use next_fs::journal::directories::Error as D;
    match error {
        D::Store(error) => map_error(error),
        D::Full => BackendError::Full,
        D::NotFound => BackendError::NotFound,
        D::Exists => BackendError::Exists,
        D::Conflict => BackendError::Conflict,
        D::Path | D::Owner | D::Corrupt | D::MissingParent | D::NotDirectory | D::NotEmpty => {
            BackendError::InvalidPath
        }
    }
}
fn validate<D: WritableBlockDevice>(
    store: &mut Store<'_, D>,
    request: CopyRequest,
) -> Result<(), BackendError> {
    if !request.source.path.as_str().starts_with("/native/")
        || !request.destination.as_str().starts_with("/native/")
    {
        return Err(BackendError::ReadOnly);
    }
    if request.source.path == request.destination {
        return Err(BackendError::InvalidPath);
    }
    if request.source.revision != store.generation() {
        return Err(BackendError::Conflict);
    }
    if checked_native_leaf(store, request.source.path)?.is_none() {
        return Err(BackendError::NotFound);
    }
    let present = checked_native_leaf(store, request.destination)?.is_some();
    match request.replace {
        Some(target)
            if target.path != request.destination
                || target.revision != store.generation()
                || !present =>
        {
            return Err(BackendError::Conflict)
        }
        None if present => return Err(BackendError::Exists),
        _ => {}
    }
    if store.is_read_only() {
        return Err(BackendError::ReadOnly);
    }
    Ok(())
}
struct CopyBackend<'s, 'd, D: WritableBlockDevice> {
    store: &'s mut Store<'d, D>,
    request: CopyRequest,
    error: Option<BackendError>,
}
impl<D: WritableBlockDevice> Backend for CopyBackend<'_, '_, D> {
    fn read(&mut self, _: &str, _: &mut [u8; FILE_BYTES]) -> Result<usize, file::Error> {
        Err(file::Error::Io)
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), file::Error> {
        let result = (|| {
            if path != self.request.destination.as_str() {
                return Err(BackendError::InvalidPath);
            }
            validate(self.store, self.request)?;
            if checked_native_leaf(self.store, self.request.destination)?.is_some() {
                self.store.put(path, bytes).map_err(map_error)
            } else {
                let generation = self.store.generation();
                next_fs::journal::directories::create_file(
                    self.store,
                    DIRECTORY_OWNER,
                    generation,
                    path,
                    bytes,
                )
                .map(|_| ())
                .map_err(directory_error)
            }
        })();
        result.map_err(|error| {
            self.error = Some(error);
            file::Error::Io
        })
    }
}
pub(crate) fn copy<D: WritableBlockDevice, const N: usize>(
    files: &mut Files<N>,
    store: &mut Store<'_, D>,
    request: CopyRequest,
) -> Result<(), BackendError> {
    validate(store, request)?;
    let mut bytes = [0; FILE_BYTES];
    let len = store
        .read_file(request.source.path.as_str(), &mut bytes)
        .map_err(map_error)?;
    let mut backend = CopyBackend {
        store,
        request,
        error: None,
    };
    files
        .replace(&mut backend, request.destination.as_str(), &bytes[..len])
        .map_err(|error| {
            backend.error.unwrap_or(match error {
                file::Error::Full => BackendError::Full,
                file::Error::Busy => BackendError::Busy,
                file::Error::ReadOnly => BackendError::ReadOnly,
                file::Error::InvalidPath | file::Error::TooLarge => BackendError::InvalidPath,
                _ => BackendError::Io,
            })
        })
}
