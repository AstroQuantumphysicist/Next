//! Bounded two-operation namespace/data transaction, no on-disk format change.
//! The caller supplies authority and an exact global generation from its trusted
//! snapshot. All namespace validation and checksum preflight precede writes.
//! This is a storage primitive, not Trash, a permission grant or VFS publication.
use super::{
    crc32, path_name, read_payload, Entry, Error, FileInfo, Store, WritableBlockDevice, MAX_FILES,
    MAX_FILE_BYTES,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation<'a> {
    Create {
        path: &'a str,
        bytes: &'a [u8],
    },
    Replace {
        path: &'a str,
        bytes: &'a [u8],
    },
    Rename {
        old: &'a str,
        new: &'a str,
        replace: bool,
    },
    Remove {
        path: &'a str,
    },
}
#[derive(Debug, PartialEq, Eq)]
pub enum TransactionError<E> {
    Count,
    Conflict,
    Journal(Error<E>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub previous_generation: u64,
    pub generation: u64,
}
impl<'disk, D: WritableBlockDevice> Store<'disk, D> {
    /// Ordered operations are staged against the preceding operation's result.
    /// No externally visible intermediate namespace or generation is published.
    pub fn transaction(
        &mut self,
        expected_generation: u64,
        operations: &[Mutation<'_>],
    ) -> Result<Receipt, TransactionError<D::Error>> {
        self.transaction_inner(expected_generation, operations, false)
    }

    /// Directory namespace's checked metadata transaction. The crate-private
    /// entry point is the only way to mutate reserved `/dirs` marker objects.
    pub(crate) fn directory_transaction(
        &mut self,
        expected_generation: u64,
        operations: &[Mutation<'_>],
    ) -> Result<Receipt, TransactionError<D::Error>> {
        self.transaction_inner(expected_generation, operations, true)
    }

    fn transaction_inner(
        &mut self,
        expected_generation: u64,
        operations: &[Mutation<'_>],
        allow_directory_metadata: bool,
    ) -> Result<Receipt, TransactionError<D::Error>> {
        use TransactionError::{Conflict, Count, Journal};
        if operations.is_empty() || operations.len() > 2 {
            return Err(Count);
        }
        if expected_generation != self.generation {
            return Err(Conflict);
        }
        if !allow_directory_metadata
            && operations.iter().any(|operation| match operation {
                Mutation::Create { path, .. }
                | Mutation::Replace { path, .. }
                | Mutation::Remove { path } => reserved_directory_path(path),
                Mutation::Rename { old, new, .. } => {
                    reserved_directory_path(old) || reserved_directory_path(new)
                }
            })
        {
            return Err(Journal(Error::InvalidPath));
        }
        self.writable().map_err(Journal)?;
        self.generation
            .checked_add(1)
            .ok_or(Journal(Error::GenerationExhausted))?;
        let mut entries = self.entries;
        let mut replacements = [None; MAX_FILES];
        for operation in operations {
            match *operation {
                Mutation::Create { path, bytes } | Mutation::Replace { path, bytes } => {
                    let name = path_name(path).ok_or(Journal(Error::InvalidPath))?;
                    if bytes.len() > MAX_FILE_BYTES {
                        return Err(Journal(Error::TooLarge));
                    }
                    let present = entries
                        .iter()
                        .position(|entry| entry.used() && entry.info.name == name);
                    let index = match (*operation, present) {
                        (Mutation::Create { .. }, Some(_)) => return Err(Journal(Error::Exists)),
                        (Mutation::Replace { .. }, None) => return Err(Journal(Error::NotFound)),
                        (_, Some(index)) => index,
                        (_, None) => entries
                            .iter()
                            .position(|entry| !entry.used())
                            .ok_or(Journal(Error::Full))?,
                    };
                    entries[index] = Entry {
                        info: FileInfo {
                            name,
                            size: bytes.len() as u32,
                        },
                        crc: crc32(bytes),
                    };
                    replacements[index] = Some(bytes);
                }
                Mutation::Remove { path } => {
                    let name = path_name(path).ok_or(Journal(Error::InvalidPath))?;
                    let index = entries
                        .iter()
                        .position(|entry| entry.used() && entry.info.name == name)
                        .ok_or(Journal(Error::NotFound))?;
                    entries[index] = Entry::EMPTY;
                    replacements[index] = None;
                }
                Mutation::Rename { old, new, replace } => {
                    let old = path_name(old).ok_or(Journal(Error::InvalidPath))?;
                    let new = path_name(new).ok_or(Journal(Error::InvalidPath))?;
                    let index = entries
                        .iter()
                        .position(|entry| entry.used() && entry.info.name == old)
                        .ok_or(Journal(Error::NotFound))?;
                    if old != new {
                        if let Some(destination) = entries
                            .iter()
                            .position(|entry| entry.used() && entry.info.name == new)
                        {
                            if !replace {
                                return Err(Journal(Error::Exists));
                            }
                            entries[destination] = Entry::EMPTY;
                            replacements[destination] = None;
                        }
                        entries[index].info.name = new;
                    }
                }
            }
        }
        // Validate every current file before issuing even an inactive-bank write.
        // Failure cannot hide a corrupt object by removing/replacing its entry.
        let mut scratch = [0; MAX_FILE_BYTES];
        for (index, entry) in self.entries.iter().copied().enumerate() {
            if entry.used() {
                if let Err(error) =
                    read_payload(&mut self.partition, self.bank, index, entry, &mut scratch)
                {
                    self.read_only = true;
                    return Err(Journal(error));
                }
            }
        }
        self.commit_payloads(entries, replacements)
            .map_err(Journal)?;
        Ok(Receipt {
            previous_generation: expected_generation,
            generation: self.generation,
        })
    }
}

fn reserved_directory_path(path: &str) -> bool {
    path == "/dirs" || path.starts_with("/dirs/")
}
