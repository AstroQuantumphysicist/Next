#![no_std]
#![no_main]
//! Original development fixture: a retained open description survives a GUI Trash move.
use next_user as rt;
rt::entry!(probe);
const PATH: &str = "/native/trash-proof";
const ORIGINAL: &[u8] = b"original-data";
const MODIFIED: &[u8] = b"held-modified";
fn check(info: &rt::StartInfo, phase: &str, result: i64, expected: i64) {
    if result != expected {
        use core::fmt::Write;
        struct Line {
            bytes: [u8; 128],
            len: usize,
        }
        impl Write for Line {
            fn write_str(&mut self, value: &str) -> core::fmt::Result {
                let end = self
                    .len
                    .checked_add(value.len())
                    .filter(|n| *n <= self.bytes.len())
                    .ok_or(core::fmt::Error)?;
                self.bytes[self.len..end].copy_from_slice(value.as_bytes());
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
            "TRASH-PROBE: FAIL phase={} result={} expected={}",
            phase, result, expected
        );
        rt::log(
            info.log,
            core::str::from_utf8(&line.bytes[..line.len]).unwrap_or("TRASH-PROBE: FAIL\n"),
        );
        rt::exit(101);
    }
}
fn probe(info: &rt::StartInfo) -> ! {
    let denied = rt::Status::AccessDenied as i64;
    check(
        info,
        "namespace-spawn-denied",
        rt::spawn(info.launcher, "/system/probe.elf"),
        denied,
    );
    check(
        info,
        "namespace-ready-denied",
        rt::ready(info.launcher),
        denied,
    );
    check(
        info,
        "namespace-duplicate-denied",
        rt::call(
            rt::Syscall::HandleDuplicate,
            [info.launcher, next_abi::Rights::READ.0, 0, 0, 0, 0],
        ),
        denied,
    );
    check(
        info,
        "log-open-denied",
        rt::file_open(info.log, PATH, 0, next_abi::Rights::READ),
        denied,
    );
    rt::log(
        info.log,
        "TRASH-PROBE: scoped namespace rejects Spawn/Ready/Duplicate and log rejects FileOpen\n",
    );
    // The trusted development runfiles command supplies a scoped FileNamespace,
    // not a Launcher. Exclusive creation preserves any pre-existing user file.
    let result = rt::file_open(
        info.launcher,
        PATH,
        5,
        next_abi::Rights::READ.union(next_abi::Rights::WRITE),
    );
    if result <= 0 {
        check(info, "exclusive-open", result, 1);
    }
    let held = result as u64;
    check(
        info,
        "initial-write",
        rt::file_write(held, ORIGINAL),
        ORIGINAL.len() as i64,
    );
    rt::log(
        info.log,
        "TRASH-PROBE: READY retained descriptor; waiting for trusted GUI move\n",
    );
    let start = rt::clock();
    if start < 0 {
        check(info, "clock", start, 0);
    }
    let deadline = (start as u64).saturating_add(120_000_000_000);
    loop {
        let opened = rt::file_open(info.launcher, PATH, 0, next_abi::Rights::READ);
        if opened == rt::Status::NotFound as i64 {
            break;
        }
        if opened <= 0 {
            check(info, "poll-open", opened, rt::Status::NotFound as i64);
        }
        check(info, "poll-close", rt::close(opened as u64), 0);
        let now = rt::clock();
        if now < 0 {
            check(info, "poll-clock", now, 0);
        }
        if now as u64 >= deadline {
            check(info, "move-timeout", now, 0);
        }
        rt::sleep_until((now as u64).saturating_add(50_000_000));
    }
    check(info, "held-seek", rt::file_seek(held, 0, 0), 0);
    check(
        info,
        "held-write",
        rt::file_write(held, MODIFIED),
        MODIFIED.len() as i64,
    );
    check(info, "readback-seek", rt::file_seek(held, 0, 0), 0);
    let mut bytes = [0; 13];
    check(
        info,
        "held-read",
        rt::file_read(held, &mut bytes),
        MODIFIED.len() as i64,
    );
    check(info, "held-content", i64::from(bytes != MODIFIED), 0);
    check(info, "held-close", rt::close(held), 0);
    rt::log(
        info.log,
        "TRASH-PROBE: PASS held inode write/readback after GUI move\n",
    );
    rt::exit(0)
}
