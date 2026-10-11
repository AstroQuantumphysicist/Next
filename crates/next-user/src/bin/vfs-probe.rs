#![no_std]
#![no_main]
//! Original native integration fixture for the bounded common VFS.
use next_user as rt;
rt::entry!(probe);
const A: &str = "/native/vfs-a";
const B: &str = "/native/vfs-b";
fn check(info: &rt::StartInfo, condition: bool, code: i64) {
    if !condition {
        rt::log(info.log, "VFS-PROBE: FAIL (exit code identifies check)\n");
        rt::exit(code);
    }
}
fn open(info: &rt::StartInfo, path: &str, flags: u64) -> u64 {
    let result = rt::file_open(
        info.launcher,
        path,
        flags,
        next_abi::Rights::READ
            .union(next_abi::Rights::WRITE)
            .union(next_abi::Rights::DUPLICATE),
    );
    if result <= 0 {
        use core::fmt::Write;
        struct Line {
            bytes: [u8; 128],
            len: usize,
        }
        impl Write for Line {
            fn write_str(&mut self, text: &str) -> core::fmt::Result {
                let end = self
                    .len
                    .checked_add(text.len())
                    .filter(|n| *n <= self.bytes.len())
                    .ok_or(core::fmt::Error)?;
                self.bytes[self.len..end].copy_from_slice(text.as_bytes());
                self.len = end;
                Ok(())
            }
        }
        let mut line = Line {
            bytes: [0; 128],
            len: 0,
        };
        let _ = writeln!(
            line,
            "VFS-PROBE: open path={} flags={} result={}",
            path, flags, result
        );
        rt::log(
            info.log,
            core::str::from_utf8(&line.bytes[..line.len]).unwrap_or("VFS-PROBE: open failed\n"),
        );
        rt::exit(101);
    }
    result as u64
}
fn rename(authority: u64, old: &str, new: &str) -> i64 {
    rt::call(
        rt::Syscall::FileRename,
        [
            authority,
            old.as_ptr() as u64,
            old.len() as u64,
            new.as_ptr() as u64,
            new.len() as u64,
            0,
        ],
    )
}
fn unlink(authority: u64, path: &str) -> i64 {
    rt::call(
        rt::Syscall::FileUnlink,
        [authority, path.as_ptr() as u64, path.len() as u64, 0, 0, 0],
    )
}
fn stat(info: &rt::StartInfo, handle: u64, expected: [u64; 2], code: i64) {
    let mut value = [0; 2];
    check(
        info,
        rt::file_stat(handle, &mut value) == 0 && value == expected,
        code,
    );
}
fn read(info: &rt::StartInfo, handle: u64, expected: &[u8], code: i64) {
    let mut value = [0; 32];
    let n = rt::file_read(handle, &mut value[..expected.len()]);
    check(
        info,
        n == expected.len() as i64 && value[..expected.len()] == *expected,
        code,
    );
}
fn probe(info: &rt::StartInfo) -> ! {
    rt::log(info.log, "VFS-PROBE: shared inode regression starting\n");
    check(info, info.launcher != 0, 100);
    let a = open(info, A, 3);
    check(info, rt::file_write(a, b"abcde") == 5, 102);
    let second = open(info, A, 0);
    stat(info, a, [5, 5], 103);
    stat(info, second, [5, 0], 104);
    read(info, second, b"ab", 105);
    stat(info, a, [5, 5], 106);
    check(
        info,
        rt::file_seek(a, 0, 0) == 0 && rt::file_write(a, b"XY") == 2,
        107,
    );
    check(info, rt::file_seek(second, 0, 0) == 0, 108);
    read(info, second, b"XY", 109);
    let duplicate = rt::call(
        rt::Syscall::HandleDuplicate,
        [second, next_abi::Rights::READ.0, 0, 0, 0, 0],
    );
    check(info, duplicate > 0, 110);
    read(info, duplicate as u64, b"c", 111);
    stat(info, second, [5, 3], 112);
    check(info, rt::close(duplicate as u64) == 0, 113);
    let target = open(info, B, 3);
    check(info, rt::file_write(target, b"target") == 6, 114);
    check(
        info,
        rename(info.log, A, B) == rt::Status::AccessDenied as i64,
        115,
    );
    check(
        info,
        unlink(info.log, A) == rt::Status::AccessDenied as i64,
        116,
    );
    check(info, rename(info.launcher, A, B) == 0, 117);
    check(
        info,
        rt::file_open(info.launcher, A, 0, next_abi::Rights::READ) == rt::Status::NotFound as i64,
        118,
    );
    let renamed = open(info, B, 0);
    read(info, renamed, b"XYcde", 119);
    check(
        info,
        rt::file_seek(target, 0, 0) == 0 && rt::file_write(target, b"private") == 7,
        120,
    );
    check(info, rt::file_seek(renamed, 0, 0) == 0, 121);
    read(info, renamed, b"XYcde", 122);
    check(
        info,
        rt::file_seek(a, 5, 0) == 5 && rt::file_write(a, b"!") == 1,
        123,
    );
    stat(info, second, [6, 3], 124);
    check(info, unlink(info.launcher, B) == 0, 125);
    check(info, rt::file_write(a, b"anon") == 4, 126);
    check(
        info,
        rt::file_open(info.launcher, B, 0, next_abi::Rights::READ) == rt::Status::NotFound as i64,
        127,
    );
    let recreated = open(info, B, 1);
    check(info, rt::file_write(recreated, b"new") == 3, 128);
    check(info, rt::file_seek(renamed, 0, 0) == 0, 129);
    read(info, renamed, b"XYcde!anon", 130);
    check(info, rt::file_seek(recreated, 0, 0) == 0, 131);
    read(info, recreated, b"new", 132);
    check(info, rt::file_seek(target, 0, 0) == 0, 133);
    read(info, target, b"private", 134);
    for handle in [a, second, target, renamed, recreated] {
        check(info, rt::close(handle) == 0, 135);
    }
    check(info, unlink(info.launcher, B) == 0, 136);
    check(
        info,
        rt::file_open(info.launcher, B, 0, next_abi::Rights::READ) == rt::Status::NotFound as i64,
        137,
    );
    rt::log(info.log,"VFS-PROBE: independent/shared offsets, rename replacement, unlink lifetime and recreate isolation passed\n");
    rt::exit(0)
}
