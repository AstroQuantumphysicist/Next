//! Host tests for the bounded Linux file adapter's synthetic stdin contract.
extern crate helix_core;
extern crate next_abi;

use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

#[allow(dead_code)]
mod input {
    pub unsafe fn port_in(_: u16) -> u8 {
        0x20
    }
    pub unsafe fn port_out(_: u16, _: u8) {}
}

#[allow(dead_code)]
mod file {
    use std::string::String;
    use std::sync::Mutex;
    use std::vec::Vec;

    use next_abi::Status;

    #[derive(Clone, Copy)]
    pub struct FileHandle(u64);

    struct MockFile {
        path: String,
        bytes: Vec<u8>,
    }
    struct MockHandle {
        inode: usize,
        offset: usize,
        refs: u32,
    }
    #[derive(Default)]
    struct Mock {
        files: Vec<MockFile>,
        handles: Vec<Option<MockHandle>>,
    }
    static MOCK: Mutex<Mock> = Mutex::new(Mock {
        files: Vec::new(),
        handles: Vec::new(),
    });

    pub fn open_for_linux(path: &str, _: u64) -> Result<FileHandle, helix_core::file::Error> {
        let mut mock = MOCK.lock().unwrap();
        let inode = mock
            .files
            .iter()
            .position(|file| file.path == path)
            .unwrap_or_else(|| {
                let bytes = match path {
                    "/native/positional" => b"abcdef".to_vec(),
                    "/native/append" => b"abc".to_vec(),
                    _ => b"readonly".to_vec(),
                };
                mock.files.push(MockFile {
                    path: path.into(),
                    bytes,
                });
                mock.files.len() - 1
            });
        let slot = mock.handles.len();
        mock.handles.push(Some(MockHandle {
            inode,
            offset: 0,
            refs: 1,
        }));
        Ok(FileHandle(slot as u64))
    }
    pub fn close(handle: FileHandle) -> Result<(), Status> {
        let mut mock = MOCK.lock().unwrap();
        let entry = mock
            .handles
            .get_mut(handle.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(Status::InvalidHandle)?;
        entry.refs -= 1;
        if entry.refs == 0 {
            mock.handles[handle.0 as usize] = None;
        }
        Ok(())
    }
    pub fn read(handle: FileHandle, out: &mut [u8]) -> Result<usize, Status> {
        let mut mock = MOCK.lock().unwrap();
        let Mock { files, handles } = &mut *mock;
        let entry = handles
            .get_mut(handle.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(Status::InvalidHandle)?;
        let file = &files[entry.inode];
        let start = entry.offset.min(file.bytes.len());
        let length = out.len().min(file.bytes.len() - start);
        out[..length].copy_from_slice(&file.bytes[start..start + length]);
        entry.offset += length;
        Ok(length)
    }
    pub fn write(handle: FileHandle, bytes: &[u8]) -> Result<usize, Status> {
        let mut mock = MOCK.lock().unwrap();
        let Mock { files, handles } = &mut *mock;
        let entry = handles
            .get_mut(handle.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(Status::InvalidHandle)?;
        let end = entry
            .offset
            .checked_add(bytes.len())
            .ok_or(Status::InvalidArgument)?;
        if end > helix_core::file::FILE_BYTES {
            return Err(Status::InvalidArgument);
        }
        let file = &mut files[entry.inode];
        if end > file.bytes.len() {
            file.bytes.resize(end, 0);
        }
        file.bytes[entry.offset..end].copy_from_slice(bytes);
        entry.offset = end;
        Ok(bytes.len())
    }
    pub fn seek(handle: FileHandle, offset: i64, origin: u64) -> Result<u64, Status> {
        let mut mock = MOCK.lock().unwrap();
        let Mock { files, handles } = &mut *mock;
        let entry = handles
            .get_mut(handle.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(Status::InvalidHandle)?;
        let base = match origin {
            0 => 0i64,
            1 => entry.offset as i64,
            2 => files[entry.inode].bytes.len() as i64,
            _ => return Err(Status::InvalidArgument),
        };
        let offset = base.checked_add(offset).ok_or(Status::InvalidArgument)?;
        if !(0..=helix_core::file::FILE_BYTES as i64).contains(&offset) {
            return Err(Status::InvalidArgument);
        }
        entry.offset = offset as usize;
        Ok(offset as u64)
    }
    pub fn info(handle: FileHandle) -> Result<(usize, usize), Status> {
        let mock = MOCK.lock().unwrap();
        let entry = mock
            .handles
            .get(handle.0 as usize)
            .and_then(Option::as_ref)
            .ok_or(Status::InvalidHandle)?;
        Ok((mock.files[entry.inode].bytes.len(), entry.offset))
    }
    pub fn inode(handle: FileHandle) -> Result<u64, Status> {
        let mock = MOCK.lock().unwrap();
        let entry = mock
            .handles
            .get(handle.0 as usize)
            .and_then(Option::as_ref)
            .ok_or(Status::InvalidHandle)?;
        Ok(entry.inode as u64 + 1)
    }
    pub fn retain(handle: FileHandle) -> Result<(), Status> {
        let mut mock = MOCK.lock().unwrap();
        let entry = mock
            .handles
            .get_mut(handle.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(Status::InvalidHandle)?;
        entry.refs += 1;
        Ok(())
    }
    pub fn rename(_: &str, _: &str) -> Result<(), Status> {
        Ok(())
    }
    pub fn remove(_: &str) -> Result<(), Status> {
        Ok(())
    }
}

#[path = "../crates/helix-kernel/src/linux.rs"]
#[allow(dead_code)]
mod linux;
#[path = "../crates/helix-kernel/src/linux_fs.rs"]
mod linux_fs;

#[test]
fn getpid_returns_kernel_pid_only_for_linux_profile() {
    const KERNEL_PID: u64 = 0x1_0000_0042;

    assert_eq!(
        linux::identity_syscall_result(39, linux::Profile::Linux, KERNEL_PID),
        KERNEL_PID
    );
    assert_eq!(
        linux::identity_syscall_result(39, linux::Profile::Native, KERNEL_PID),
        next_abi::Status::NotSupported as i64 as u64
    );
    assert_eq!(
        linux::identity_syscall_result(39, linux::Profile::Nt, KERNEL_PID),
        next_abi::Status::NotSupported as i64 as u64
    );
    assert_eq!(
        linux::identity_syscall_result(0xfeed, linux::Profile::Linux, KERNEL_PID),
        linux::ENOSYS
    );
}

#[test]
fn uname_uses_linux_x86_64_six_field_layout() {
    let bytes = linux::utsname_bytes();
    assert_eq!(bytes.len(), 390);
    let fields: [&[u8]; 6] = [
        b"Linux",
        b"next",
        b"1.0-next-linux-abi",
        b"#1 Helix Linux compatibility personality",
        b"x86_64",
        b"",
    ];
    for (index, field) in fields.iter().enumerate() {
        let value = &bytes[index * 65..(index + 1) * 65];
        assert_eq!(&value[..field.len()], *field);
        assert!(value[field.len()..].iter().all(|byte| *byte == 0));
    }
}

#[test]
fn stdin_is_eof_device_with_independent_duplicate_lifetime() {
    let _serial = SERIAL.lock().unwrap();
    const SLOT: usize = 0;
    const GENERATION: u64 = 1;

    let mut byte = [0xa5];
    assert_eq!(linux_fs::read(SLOT, GENERATION, 0, &mut byte), Ok(0));
    assert_eq!(byte, [0xa5]);
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, 0, 17, 0), Ok(0));
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, 0, 0, 3),
        Err((-22i64) as u64)
    );

    let mut stat = [0; linux_fs::STAT_BYTES];
    linux_fs::stat(SLOT, GENERATION, 0, &mut stat).unwrap();
    assert_eq!(
        u32::from_le_bytes(stat[24..28].try_into().unwrap()) & 0xf000,
        0x2000
    );
    assert_eq!(i64::from_le_bytes(stat[48..56].try_into().unwrap()), 0);

    let duplicate = linux_fs::duplicate(SLOT, GENERATION, 0).unwrap();
    assert_eq!(duplicate, 3);
    assert_eq!(linux_fs::close(SLOT, GENERATION, 0), Ok(()));
    assert_eq!(
        linux_fs::read(SLOT, GENERATION, 0, &mut byte),
        Err((-9i64) as u64)
    );
    assert_eq!(
        linux_fs::read(SLOT, GENERATION, duplicate, &mut byte),
        Ok(0)
    );
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, duplicate, 31, 2), Ok(0));
    assert_eq!(
        linux_fs::write(SLOT, GENERATION, duplicate, b"x"),
        Err((-9i64) as u64)
    );

    let second_duplicate = linux_fs::duplicate(SLOT, GENERATION, duplicate).unwrap();
    assert_eq!(second_duplicate, 4);
    assert_eq!(linux_fs::close(SLOT, GENERATION, duplicate), Ok(()));
    assert_eq!(
        linux_fs::read(SLOT, GENERATION, second_duplicate, &mut byte),
        Ok(0)
    );
    assert_eq!(linux_fs::close(SLOT, GENERATION, second_duplicate), Ok(()));
    assert_eq!(
        linux_fs::duplicate(SLOT, GENERATION, 0),
        Err((-9i64) as u64)
    );

    assert_eq!(linux_fs::read(SLOT, 2, 0, &mut byte), Ok(0));
}

#[test]
fn positional_io_preserves_offset_and_obeys_bounded_file_errors() {
    let _serial = SERIAL.lock().unwrap();
    const SLOT: usize = 1;
    const GENERATION: u64 = 7;

    let fd = linux_fs::open(SLOT, GENERATION, true, "/native/positional", 2, 0).unwrap();
    assert_eq!(fd, 3);
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 2, 0), Ok(2));
    let mut bytes = [0; 3];
    assert_eq!(linux_fs::pread(SLOT, GENERATION, fd, 0, &mut bytes), Ok(3));
    assert_eq!(&bytes, b"abc");
    assert_eq!(linux_fs::read(SLOT, GENERATION, fd, &mut bytes), Ok(3));
    assert_eq!(&bytes, b"cde");

    assert_eq!(
        linux_fs::pread(SLOT, GENERATION, fd, -1, &mut bytes),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::pwrite(SLOT, GENERATION, fd, -1, b"!"),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::pread(SLOT, GENERATION, fd, i64::MAX, &mut bytes),
        Ok(0)
    );
    assert_eq!(linux_fs::pwrite(SLOT, GENERATION, fd, 1, b"XY"), Ok(2));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 0, 1), Ok(5));
    let mut byte = [0; 1];
    assert_eq!(linux_fs::read(SLOT, GENERATION, fd, &mut byte), Ok(1));
    assert_eq!(byte, *b"f");

    assert_eq!(linux_fs::pwrite(SLOT, GENERATION, fd, 8, b"Z"), Ok(1));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 0, 1), Ok(6));
    assert_eq!(
        linux_fs::pwrite(SLOT, GENERATION, fd, 4096, b"!"),
        Err(linux::EFBIG)
    );
    assert_eq!(
        linux_fs::pread(SLOT, GENERATION, fd, 4096, &mut byte),
        Ok(0)
    );
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 0, 1), Ok(6));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 0, 0), Ok(0));
    let mut full = [0; 9];
    assert_eq!(linux_fs::read(SLOT, GENERATION, fd, &mut full), Ok(9));
    assert_eq!(&full, b"aXYdef\0\0Z");
    assert_eq!(
        linux_fs::pwrite(SLOT, GENERATION, 99, 0, b"!"),
        Err(linux::EBADF)
    );

    let append_fd = linux_fs::open(SLOT, GENERATION, true, "/native/append", 2 | 1024, 0).unwrap();
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, append_fd, 1, 0), Ok(1));
    assert_eq!(
        linux_fs::pwrite(SLOT, GENERATION, append_fd, 0, b"!"),
        Ok(1)
    );
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, append_fd, 0, 1), Ok(1));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, append_fd, 0, 0), Ok(0));
    let mut appended = [0; 4];
    assert_eq!(
        linux_fs::read(SLOT, GENERATION, append_fd, &mut appended),
        Ok(4)
    );
    assert_eq!(&appended, b"abc!");

    let readonly = linux_fs::open(SLOT, GENERATION, true, "/native/readonly", 0, 0).unwrap();
    assert_eq!(
        linux_fs::pwrite(SLOT, GENERATION, readonly, 0, b"!"),
        Err(linux::EBADF)
    );
}

#[test]
fn lseek_uses_linux_whence_and_signed_offset_errors() {
    let _serial = SERIAL.lock().unwrap();
    const SLOT: usize = 2;
    const GENERATION: u64 = 11;

    let fd = linux_fs::open(SLOT, GENERATION, true, "/native/lseek", 2, 0).unwrap();
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 4, 0), Ok(4));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 1, 1), Ok(5));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, -2, 1), Ok(3));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, -1, 2), Ok(7));

    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, fd, -1, 0),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, fd, -8, 1),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, fd, -9, 2),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, fd, 0, 3),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, fd, i64::MAX, 1),
        Err(linux::EOVERFLOW)
    );
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, fd, i64::MAX, 0),
        Err(linux::EINVAL)
    );
    assert_eq!(
        linux_fs::lseek(SLOT, GENERATION, 99, 0, 3),
        Err(linux::EBADF)
    );

    // Failed offsets and whence values leave the shared open-description
    // offset unchanged, including for a descriptor duplicated by dup.
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 0, 1), Ok(7));
    let duplicate = linux_fs::duplicate(SLOT, GENERATION, fd).unwrap();
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, duplicate, -1, 1), Ok(6));
    assert_eq!(linux_fs::lseek(SLOT, GENERATION, fd, 0, 1), Ok(6));
}
