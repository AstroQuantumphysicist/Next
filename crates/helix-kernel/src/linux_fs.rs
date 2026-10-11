//! Development Linux descriptor adapter over the same native stream objects.
//! Path opens require an explicit console-launch grant and stay in /native/.
//! Eight file descriptors per process; open paths remain namespace-exclusive.
use crate::file::{self, FileHandle};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use helix_core::{
    descriptor::{Descriptors, Error as FdError},
    file::Error as FileError,
    handle::Rights,
};
use next_abi::Status;
pub const MAX_IO: usize = 4096;
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
    errno(match e {
        Status::InvalidHandle => 9,
        Status::InvalidArgument => 22,
        Status::AccessDenied => 13,
        Status::NotFound => 2,
        Status::NoMemory => 23,
        Status::WouldBlock => 16,
        _ => 5,
    })
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
struct Stream {
    handle: FileHandle,
    append: bool,
}
struct Owner {
    generation: u64,
    fds: Descriptors<Stream, 8>,
    stdout: bool,
    stderr: bool,
}
impl Owner {
    const EMPTY: Self = Self {
        generation: 0,
        fds: Descriptors::new(),
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
            let _ = file::close(stream.handle);
        }
        owner.generation = generation;
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
    if !granted || !path.starts_with("/native/") {
        return Err(errno(13));
    }
    const ALLOWED: u64 = 3 | 64 | 128 | 512 | 1024 | 524288;
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
        let native_flags = u64::from(flags & 64 != 0)
            | (u64::from(flags & 512 != 0) * 2)
            | (u64::from(flags & 128 != 0) * 4);
        match file::open_for_linux(path, native_flags) {
            Ok(handle) => {
                let stream = Stream {
                    handle,
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
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::READ) {
            return Err(errno(9));
        }
        file::read(stream.handle, out).map_err(status)
    })
}
pub fn write(slot: usize, generation: u64, fd: u64, bytes: &[u8]) -> Result<usize, u64> {
    with_owner(slot, generation, |owner| {
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        if !rights.contains(Rights::WRITE) {
            return Err(errno(9));
        }
        let old_offset = if stream.append {
            let (_, offset) = file::info(stream.handle).map_err(status)?;
            file::seek(stream.handle, 0, 2).map_err(status)?;
            Some(offset)
        } else {
            None
        };
        match file::write(stream.handle, bytes) {
            Ok(n) => Ok(n),
            Err(e) => {
                if let Some(offset) = old_offset {
                    let _ = file::seek(stream.handle, offset as i64, 0);
                }
                Err(status(e))
            }
        }
    })
}
pub fn close(slot: usize, generation: u64, fd: u64) -> Result<(), u64> {
    with_owner(slot, generation, |owner| {
        if fd == 1 && owner.stdout {
            owner.stdout = false;
            return Ok(());
        }
        if fd == 2 && owner.stderr {
            owner.stderr = false;
            return Ok(());
        }
        let stream = owner.fds.remove(fd).map_err(fd_error)?.ok_or(errno(9))?;
        file::close(stream.handle).map_err(status)
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
pub fn seek(slot: usize, generation: u64, fd: u64, offset: i64, origin: u64) -> Result<u64, u64> {
    with_owner(slot, generation, |owner| {
        let (stream, _) = owner.fds.get(fd).map_err(fd_error)?;
        file::seek(stream.handle, offset, origin).map_err(status)
    })
}
pub fn duplicate(slot: usize, generation: u64, fd: u64) -> Result<u64, u64> {
    with_owner(slot, generation, |owner| {
        let (stream, rights) = owner.fds.get(fd).map_err(fd_error)?;
        let new = owner.fds.reserve().map_err(fd_error)?;
        if let Err(e) = file::retain(stream.handle) {
            let _ = owner.fds.remove(new);
            return Err(status(e));
        }
        if let Err(e) = owner.fds.install(new, stream, rights) {
            let _ = file::close(stream.handle);
            let _ = owner.fds.remove(new);
            return Err(fd_error(e));
        }
        Ok(new)
    })
}
pub fn stat(slot: usize, generation: u64, fd: u64, out: &mut [u8; STAT_BYTES]) -> Result<(), u64> {
    with_owner(slot, generation, |owner| {
        let (stream, _) = owner.fds.get(fd).map_err(fd_error)?;
        let (len, _) = file::info(stream.handle).map_err(status)?;
        let inode = file::inode(stream.handle).map_err(status)?;
        out.fill(0);
        out[0..8].copy_from_slice(&1u64.to_le_bytes());
        out[8..16].copy_from_slice(&inode.to_le_bytes());
        out[16..24].copy_from_slice(&1u64.to_le_bytes());
        out[24..28].copy_from_slice(&0o100600u32.to_le_bytes());
        out[28..32].copy_from_slice(&1000u32.to_le_bytes());
        out[32..36].copy_from_slice(&1000u32.to_le_bytes());
        out[48..56].copy_from_slice(&(len as i64).to_le_bytes());
        out[56..64].copy_from_slice(&512i64.to_le_bytes());
        out[64..72].copy_from_slice(&(len.div_ceil(512) as i64).to_le_bytes());
        Ok(())
    })
}
pub fn close_all(slot: usize, generation: u64) {
    let _ = with_owner(slot, generation, |owner| {
        while let Some(stream) = owner.fds.take_one() {
            let _ = file::close(stream.handle);
        }
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
