//! Development Linux descriptor adapter over the same native stream objects.
//! Path opens require an explicit console-launch grant and stay in /native/.
//! Eight file descriptors per process; directory streams are bounded open snapshots.
#[path = "linux_dirent.rs"]
pub(crate) mod dirent;
use crate::file::{self, FileHandle};
use crate::linux;
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use helix_core::{
    descriptor::{Descriptors, Error as FdError},
    file::{Error as FileError, FILE_BYTES},
    handle::Rights,
};
use next_abi::Status;
pub const MAX_IO: usize = 4096;
/// Linux x86-64 `struct stat` size for syscall 5 (144-byte `newfstat` ABI).
pub const STAT_BYTES: usize = 144;
fn errno(n: u64) -> u64 {
    (0u64).wrapping_sub(n)
}
fn fd_error(e: FdError) -> u64 {
    errno(match e {
        FdError::Full => 24,
        _ => 9,
    })
}
fn status(e: Status) -> u64 {
    crate::linux::status_errno(e)
}
fn open_error(e: FileError) -> u64 {
    errno(match e {
        FileError::Exists => 17,
        FileError::Busy => 16,
        FileError::Full => 23,
        FileError::ReadOnly => 13,
        FileError::NotFound => 2,
        FileError::InvalidPath => 22,
        FileError::TooLarge => 27,
        _ => 5,
    })
}
#[derive(Clone, Copy)]
enum StreamObject {
    File(FileHandle),
    NullInput,
    Directory(usize),
}
#[derive(Clone, Copy)]
struct Stream {
    object: StreamObject,
    append: bool,
}
#[derive(Clone, Copy)]
struct Directory {
    generation: u64,
    snapshot: dirent::Snapshot,
    cursor: usize,
    refs: u8,
}
impl Directory {
    const EMPTY: Self = Self {
        generation: 0,
        snapshot: dirent::Snapshot::EMPTY,
        cursor: 0,
        refs: 0,
    };
}
struct Owner {
    generation: u64,
    directories: [Directory; 8],
    next_directory_generation: u64,
    fds: Descriptors<Stream, 8>,
    stdin: bool,
    stdout: bool,
    stderr: bool,
}
impl Owner {
    const EMPTY: Self = Self {
        generation: 0,
        directories: [Directory::EMPTY; 8],
        next_directory_generation: 1,
        fds: Descriptors::new(),
        stdin: true,
        stdout: true,
        stderr: true,
    };
}
struct Global(UnsafeCell<[Owner; 4]>);
unsafe impl Sync for Global {}
static OWNERS: Global = Global(UnsafeCell::new([const { Owner::EMPTY }; 4]));
static LOCK: AtomicBool = AtomicBool::new(false);
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}
fn with_owner<T>(
    slot: usize,
    generation: u64,
    operation: impl FnOnce(&mut Owner) -> Result<T, u64>,
) -> Result<T, u64> {
    LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| errno(16))?;
    let _guard = Guard;
    let owner = unsafe { &mut *OWNERS.0.get() }
        .get_mut(slot)
        .ok_or(errno(9))?;
    if generation == 0 {
        return Err(errno(9));
    }
    if owner.generation != generation {
        while let Some(stream) = owner.fds.take_one() {
            if let StreamObject::File(handle) = stream.object {
                let _ = file::close(handle);
            }
        }
        owner.directories = [Directory::EMPTY; 8];
        owner.generation = generation;
        owner.stdin = true;
        owner.stdout = true;
        owner.stderr = true;
    }
    operation(owner)
}
/// No new namespace authority is derived from the Linux personality itself.
pub fn open(
    slot: usize,
    generation: u64,
    granted: bool,
    path: &str,
    flags: u64,
    mode: u64,
) -> Result<u64, u64> {
    if !granted || !(path == "/native" || path.starts_with("/native/")) {
        return Err(errno(13));
    }
    const ALLOWED: u64 = 3 | 64 | 128 | 512 | 1024 | 65536 | 524288;
    if flags & !ALLOWED != 0 {
        return Err(errno(38));
    }
    let access = flags & 3;
    if access == 3
        || flags & 128 != 0 && flags & 64 == 0
        || flags & 512 != 0 && access == 0
        || flags & 64 != 0 && mode & 0o7777 != 0o600
    {
        return Err(errno(22));
    }
    let rights = match access {
        0 => Rights::READ,
        1 => Rights::WRITE,
        _ => Rights::READ.union(Rights::WRITE),
    };
    with_owner(slot, generation, |owner| {
        let fd = owner.fds.reserve().map_err(fd_error)?;
        // Directory lookup cannot create or mutate namespace entries.
        match crate::filesystem::linux_directory(path) {
            Ok(snapshot) => {
                if !snapshot.valid_directory() {
                    let _ = owner.fds.remove(fd);
                    return Err(errno(5));
                }
                if access != 0 || flags & (64 | 128 | 512 | 1024) != 0 {
                    let _ = owner.fds.remove(fd);
                    return Err(errno(21));
                }
                let Some(index) = owner.directories.iter().position(|d| d.refs == 0) else {
                    let _ = owner.fds.remove(fd);
                    return Err(errno(24));
                };
                let Some(next_generation) = owner.next_directory_generation.checked_add(1) else {
                    let _ = owner.fds.remove(fd);
                    return Err(errno(24));
                };
                owner.directories[index] = Directory {
                    generation: owner.next_directory_generation,
                    snapshot,
                    cursor: 0,
                    refs: 1,
                };
                if let Err(error) = owner.fds.install(
                    fd,
                    Stream {
                        object: StreamObject::Directory(index),
                        append: false,
                    },
                    Rights::READ,
                ) {
                    owner.directories[index] = Directory::EMPTY;
                    let _ = owner.fds.remove(fd);
                    return Err(fd_error(error));
                }
                owner.next_directory_generation = next_generation;
                return Ok(fd);
            }
            Err(error) if error == errno(20) || error == errno(2) => {
                if flags & 65536 != 0 || path == "/native" {
                    let _ = owner.fds.remove(fd);
                    return Err(error);
                }
            }
            Err(error) => {
                let _ = owner.fds.remove(fd);
                return Err(error);
            }
        }
        let native_flags = u64::from(flags & 64 != 0)
            | (u64::from(flags & 512 != 0) * 2)
            | (u64::from(flags & 128 != 0) * 4);
        match file::open_for_linux(path, native_flags) {
            Ok(handle) => {
                let stream = Stream {
                    object: StreamObject::File(handle),
                    append: flags & 1024 != 0,
                };
                if let Err(error) = owner.fds.install(fd, stream, rights) {
                    let _ = file::close(handle);
                    let _ = owner.fds.remove(fd);
                    return Err(fd_error(error));
                }
                Ok(fd)
            }
            Err(error) => {
                let _ = owner.fds.remove(fd);
                Err(open_error(error))
            }
        }
    })
}
pub fn read(slot: usize, generation: u64, fd: u64, out: &mut [u8]) -> Result<usize, u64> {
    with_owner(slot, generation, |owner| {
        if fd == 0 {
            return if owner.stdin { Ok(0) } else { Err(errno(9)) };
        }
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::READ) {
            return Err(errno(9));
        }
        match stream.object {
            StreamObject::NullInput => Ok(0),
            StreamObject::Directory(_) => Err(errno(21)),
            StreamObject::File(handle) => file::read(handle, out).map_err(status),
        }
    })
}
/// Linux positional reads use a temporary per-open offset and always restore
/// the descriptor's shared current offset. Offsets beyond this bounded regular
/// file's maximum capacity are ordinary EOF reads.
pub fn pread(
    slot: usize,
    generation: u64,
    fd: u64,
    offset: i64,
    out: &mut [u8],
) -> Result<usize, u64> {
    with_owner(slot, generation, |owner| {
        if fd == 0 {
            if !owner.stdin {
                return Err(errno(9));
            }
            return if offset < 0 { Err(errno(22)) } else { Ok(0) };
        }
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::READ) {
            return Err(errno(9));
        }
        if offset < 0 {
            return Err(errno(22));
        }
        let StreamObject::File(handle) = stream.object else {
            return if matches!(stream.object, StreamObject::Directory(_)) {
                Err(errno(21))
            } else {
                Ok(0)
            };
        };
        if offset as u64 > FILE_BYTES as u64 {
            return Ok(0);
        }
        let (_, original_offset) = file::info(handle).map_err(status)?;
        file::seek(handle, offset, 0).map_err(status)?;
        let result = file::read(handle, out).map_err(status);
        let restore = file::seek(handle, original_offset as i64, 0).map_err(status);
        match result {
            Err(error) => Err(error),
            Ok(bytes_read) => {
                restore?;
                Ok(bytes_read)
            }
        }
    })
}
pub fn write(slot: usize, generation: u64, fd: u64, bytes: &[u8]) -> Result<usize, u64> {
    with_owner(slot, generation, |owner| {
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::WRITE) {
            return Err(errno(9));
        }
        let StreamObject::File(handle) = stream.object else {
            return Err(errno(9));
        };
        let old_offset = if stream.append {
            let (_, offset) = file::info(handle).map_err(status)?;
            file::seek(handle, 0, 2).map_err(status)?;
            Some(offset)
        } else {
            None
        };
        match file::write(handle, bytes) {
            Ok(n) => Ok(n),
            Err(e) => {
                if let Some(offset) = old_offset {
                    let _ = file::seek(handle, offset as i64, 0);
                }
                Err(status(e))
            }
        }
    })
}
/// Linux positional writes leave the descriptor offset unchanged. Linux's
/// O_APPEND behavior also applies to pwrite64: the supplied offset is ignored
/// for placement, while the descriptor's current offset is still preserved.
pub fn pwrite(
    slot: usize,
    generation: u64,
    fd: u64,
    offset: i64,
    bytes: &[u8],
) -> Result<usize, u64> {
    with_owner(slot, generation, |owner| {
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::WRITE) {
            return Err(errno(9));
        }
        let StreamObject::File(handle) = stream.object else {
            return Err(errno(9));
        };
        if offset < 0 {
            return Err(errno(22));
        }
        let (length, original_offset) = file::info(handle).map_err(status)?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let write_offset = if stream.append {
            length
        } else {
            usize::try_from(offset).map_err(|_| linux::EFBIG)?
        };
        if write_offset
            .checked_add(bytes.len())
            .filter(|end| *end <= FILE_BYTES)
            .is_none()
        {
            return Err(linux::EFBIG);
        }
        file::seek(handle, write_offset as i64, 0).map_err(status)?;
        let result = file::write(handle, bytes).map_err(status);
        let restore = file::seek(handle, original_offset as i64, 0).map_err(status);
        match result {
            Err(error) => Err(error),
            Ok(bytes_written) => {
                restore?;
                Ok(bytes_written)
            }
        }
    })
}
pub fn close(slot: usize, generation: u64, fd: u64) -> Result<(), u64> {
    with_owner(slot, generation, |owner| {
        if fd == 0 && owner.stdin {
            owner.stdin = false;
            return Ok(());
        }
        if fd == 1 && owner.stdout {
            owner.stdout = false;
            return Ok(());
        }
        if fd == 2 && owner.stderr {
            owner.stderr = false;
            return Ok(());
        }
        let stream = owner.fds.remove(fd).map_err(fd_error)?.ok_or(errno(9))?;
        match stream.object {
            StreamObject::File(handle) => file::close(handle).map_err(status),
            StreamObject::NullInput => Ok(()),
            StreamObject::Directory(index) => {
                owner.directories[index].refs -= 1;
                if owner.directories[index].refs == 0 {
                    owner.directories[index] = Directory::EMPTY;
                }
                Ok(())
            }
        }
    })
}
pub fn stdout_open(slot: usize, generation: u64, fd: u64) -> bool {
    with_owner(slot, generation, |owner| {
        Ok(match fd {
            1 => owner.stdout,
            2 => owner.stderr,
            _ => false,
        })
    })
    .unwrap_or(false)
}
/// Linux x86-64 syscall 8 for the bounded regular files in this adapter.
///
/// Keep the arithmetic at the Linux ABI boundary so signed overflow gets the
/// Linux `EOVERFLOW` result, while a negative resulting position and an
/// unsupported whence retain `EINVAL`. The underlying file cache still owns
/// the final bounded-offset check and descriptor offset update.
pub fn lseek(slot: usize, generation: u64, fd: u64, offset: i64, whence: u64) -> Result<u64, u64> {
    with_owner(slot, generation, |owner| {
        if fd == 0 {
            return if owner.stdin && whence <= 2 {
                Ok(0)
            } else if owner.stdin {
                Err(errno(22))
            } else {
                Err(errno(9))
            };
        }
        let (stream, _) = owner.fds.get(fd).map_err(fd_error)?;
        match stream.object {
            StreamObject::NullInput => {
                if whence <= 2 {
                    Ok(0)
                } else {
                    Err(errno(22))
                }
            }
            StreamObject::Directory(index) => {
                let directory = &mut owner.directories[index];
                if whence == 1 && offset == 0 {
                    return Ok(directory.cursor as u64);
                }
                if whence != 0 || offset < 0 || offset as usize > directory.snapshot.count {
                    return Err(errno(22));
                }
                directory.cursor = offset as usize;
                Ok(offset as u64)
            }
            StreamObject::File(handle) => {
                let (length, current) = file::info(handle).map_err(status)?;
                let base = match whence {
                    0 => 0,
                    1 => current,
                    2 => length,
                    _ => return Err(errno(22)),
                };
                let base = i64::try_from(base).map_err(|_| linux::EOVERFLOW)?;
                let next = base.checked_add(offset).ok_or(linux::EOVERFLOW)?;
                if next < 0 {
                    return Err(errno(22));
                }
                // `file::seek` enforces FILE_BYTES and atomically publishes
                // the new offset only after all checks have passed.
                file::seek(handle, next, 0).map_err(status)
            }
        }
    })
}
pub fn duplicate(slot: usize, generation: u64, fd: u64) -> Result<u64, u64> {
    with_owner(slot, generation, |owner| {
        if fd == 0 {
            if !owner.stdin {
                return Err(errno(9));
            }
            let new = owner.fds.reserve().map_err(fd_error)?;
            if let Err(error) = owner.fds.install(
                new,
                Stream {
                    object: StreamObject::NullInput,
                    append: false,
                },
                Rights::READ,
            ) {
                let _ = owner.fds.remove(new);
                return Err(fd_error(error));
            }
            return Ok(new);
        }
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        let new = owner.fds.reserve().map_err(fd_error)?;
        if let StreamObject::File(handle) = stream.object {
            if let Err(error) = file::retain(handle) {
                let _ = owner.fds.remove(new);
                return Err(status(error));
            }
        }
        if let StreamObject::Directory(index) = stream.object {
            owner.directories[index].refs += 1;
        }
        if let Err(e) = owner.fds.install(new, stream, rights) {
            if let StreamObject::Directory(index) = stream.object {
                owner.directories[index].refs -= 1;
            }
            if let StreamObject::File(handle) = stream.object {
                let _ = file::close(handle);
            }
            let _ = owner.fds.remove(new);
            return Err(fd_error(e));
        }
        Ok(new)
    })
}
pub fn stat(slot: usize, generation: u64, fd: u64, out: &mut [u8; STAT_BYTES]) -> Result<(), u64> {
    with_owner(slot, generation, |owner| {
        if fd == 0 {
            if !owner.stdin {
                return Err(errno(9));
            }
            fill_null_stat(out);
            return Ok(());
        }
        if fd == 1 || fd == 2 {
            let open = if fd == 1 { owner.stdout } else { owner.stderr };
            if !open {
                return Err(errno(9));
            }
            fill_console_stat(out, fd);
            return Ok(());
        }
        let (stream, _) = owner.fds.get(fd).map_err(fd_error)?;
        match stream.object {
            StreamObject::File(handle) => {
                let (len, _) = file::info(handle).map_err(status)?;
                let inode = file::inode(handle).map_err(status)?;
                let links = file::link_count(handle).map_err(status)?;
                fill_stat(
                    out,
                    StatRecord {
                        dev: 1,
                        ino: inode,
                        nlink: links,
                        mode: 0o100600,
                        rdev: 0,
                        size: len as i64,
                        block_size: 512,
                        blocks: len.div_ceil(512) as i64,
                    },
                );
                Ok(())
            }
            StreamObject::Directory(index) => {
                fill_stat(
                    out,
                    StatRecord {
                        dev: 3,
                        ino: owner.directories[index].snapshot.entries[0].inode,
                        nlink: 2 + owner.directories[index].snapshot.entries
                            [2..owner.directories[index].snapshot.count]
                            .iter()
                            .filter(|entry| entry.kind == 4)
                            .count() as u64,
                        mode: 0o040700,
                        rdev: 0,
                        size: 0,
                        block_size: 4096,
                        blocks: 0,
                    },
                );
                Ok(())
            }
            StreamObject::NullInput => {
                fill_null_stat(out);
                Ok(())
            }
        }
    })
}
/// Serialize only the documented synthetic metadata subset of the Linux
/// x86-64 144-byte `struct stat`; timestamps, explicit padding and reserved
/// words remain zero.
struct StatRecord {
    dev: u64,
    ino: u64,
    nlink: u64,
    mode: u32,
    rdev: u64,
    size: i64,
    block_size: i64,
    blocks: i64,
}
fn fill_stat(out: &mut [u8; STAT_BYTES], record: StatRecord) {
    out.fill(0);
    out[0..8].copy_from_slice(&record.dev.to_le_bytes());
    out[8..16].copy_from_slice(&record.ino.to_le_bytes());
    out[16..24].copy_from_slice(&record.nlink.to_le_bytes());
    out[24..28].copy_from_slice(&record.mode.to_le_bytes());
    out[28..32].copy_from_slice(&1000u32.to_le_bytes());
    out[32..36].copy_from_slice(&1000u32.to_le_bytes());
    out[40..48].copy_from_slice(&record.rdev.to_le_bytes());
    out[48..56].copy_from_slice(&record.size.to_le_bytes());
    out[56..64].copy_from_slice(&record.block_size.to_le_bytes());
    out[64..72].copy_from_slice(&record.blocks.to_le_bytes());
}
fn fill_null_stat(out: &mut [u8; STAT_BYTES]) {
    // Character streams occupy a separate device namespace from file inodes.
    fill_stat(
        out,
        StatRecord {
            dev: 2,
            ino: 1,
            nlink: 1,
            mode: 0o020666,
            rdev: 259,
            size: 0,
            block_size: 4096,
            blocks: 0,
        },
    );
}
fn fill_console_stat(out: &mut [u8; STAT_BYTES], fd: u64) {
    // Fds 1 and 2 are bounded synthetic character streams, not host TTYs.
    fill_stat(
        out,
        StatRecord {
            dev: 2,
            ino: fd + 1,
            nlink: 1,
            mode: 0o020600,
            rdev: 0,
            size: 0,
            block_size: 4096,
            blocks: 0,
        },
    );
}
pub fn close_all(slot: usize, generation: u64) {
    let _ = with_owner(slot, generation, |owner| {
        while let Some(stream) = owner.fds.take_one() {
            if let StreamObject::File(handle) = stream.object {
                let _ = file::close(handle);
            }
        }
        owner.directories = [Directory::EMPTY; 8];
        owner.stdin = false;
        owner.stdout = false;
        owner.stderr = false;
        Ok(())
    });
}
/// Namespace mutation uses the launcher's explicit grant, never an fd's
/// inherited data rights. Open inode descriptions remain alive across removal.
pub fn rename(granted: bool, old: &str, new: &str) -> Result<(), u64> {
    if !granted || !old.starts_with("/native/") || !new.starts_with("/native/") {
        return Err(errno(13));
    }
    file::rename(old, new).map_err(status)
}
pub fn remove(granted: bool, path: &str) -> Result<(), u64> {
    if !granted || !path.starts_with("/native/") {
        return Err(errno(13));
    }
    file::remove(path).map_err(status)
}

/// Syscall217: shared directory cookie advances only after successful copyout.
/// Copyout callback validates only returned bytes before writing; no guard
/// spans callback execution. Failed copyout preserves the shared cookie.
pub fn getdents64(
    slot: usize,
    generation: u64,
    fd: u64,
    capacity: usize,
    copyout: impl FnOnce(&[u8]) -> Result<(), u64>,
) -> Result<usize, u64> {
    let mut bytes = [0; MAX_IO];
    let (index, object_generation, cursor, length, next) = with_owner(slot, generation, |owner| {
        if fd < 3 {
            let live = match fd {
                0 => owner.stdin,
                1 => owner.stdout,
                2 => owner.stderr,
                _ => false,
            };
            return Err(errno(if live { 20 } else { 9 }));
        }
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::READ) {
            return Err(errno(9));
        }
        let StreamObject::Directory(index) = stream.object else {
            return Err(errno(20));
        };
        let directory = &owner.directories[index];
        // Linux's directory actor uses signed-int remaining capacity. At EOF
        // no actor runs, so even an overflowing count remains a zero result.
        if capacity > i32::MAX as usize && directory.cursor < directory.snapshot.count {
            return Err(errno(22));
        }
        let (length, next) = directory
            .snapshot
            .encode(directory.cursor, &mut bytes[..capacity.min(MAX_IO)])
            .map_err(|()| errno(22))?;
        Ok((index, directory.generation, directory.cursor, length, next))
    })?;
    // EOF never accesses user memory, regardless of the supplied pointer.
    if length == 0 {
        return Ok(0);
    }
    // No adapter guard or Owner reference spans user validation/copyout.
    // The kernel syscall executes with IF clear on the sole boot CPU.
    copyout(&bytes[..length])?;
    with_owner(slot, generation, |owner| {
        let (stream, _) = owner.fds.get(fd).map_err(fd_error)?;
        if !matches!(stream.object, StreamObject::Directory(i) if i == index)
            || owner.directories[index].generation != object_generation
            || owner.directories[index].cursor != cursor
        {
            return Err(errno(16));
        }
        owner.directories[index].cursor = next;
        Ok(length)
    })
}
