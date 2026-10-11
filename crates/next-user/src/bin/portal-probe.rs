#![no_std]
#![no_main]
use next_user::{self as rt, sdk};
use sdk::{Error, PortalAccess};
rt::entry!(portal_probe);

fn require(info: &rt::StartInfo, condition: bool, code: i64) {
    if !condition {
        rt::log(info.log, "PORTAL-PROBE: fixture failed\n");
        rt::exit(code);
    }
}
fn now() -> u64 {
    sdk::clock().unwrap_or_else(|_| rt::exit(80))
}
fn poll(request: sdk::FileRequest, deadline: u64) -> sdk::Result<sdk::Handle> {
    loop {
        match request.poll() {
            Err(Error::Status(rt::Status::WouldBlock)) if now() < deadline => {
                rt::sleep_until(now().saturating_add(5_000_000));
            }
            result => return result,
        }
    }
}
fn portal_probe(info: &rt::StartInfo) -> ! {
    require(info, info.launcher == 0, 81);
    // Invalid access and deadlines fail before creating any UI request.
    require(
        info,
        rt::portal_request_open(2, now() + 1_000_000_000) == rt::Status::InvalidArgument as i64,
        82,
    );
    require(
        info,
        rt::portal_request_open(1, now()) == rt::Status::InvalidArgument as i64,
        83,
    );
    require(
        info,
        rt::portal_request_open(1, now() + 31_000_000_000) == rt::Status::InvalidArgument as i64,
        84,
    );
    require(info, rt::portal_poll(0) < 0 && rt::portal_cancel(0) < 0, 85);

    let deadline = now() + 25_000_000_000;
    let readonly =
        sdk::request_file(PortalAccess::ReadOnly, deadline).unwrap_or_else(|_| rt::exit(86));
    rt::log(
        info.log,
        "PORTAL-PROBE: waiting for readonly file consent\n",
    );
    let file = poll(readonly, deadline).unwrap_or_else(|_| rt::exit(87));
    let mut data = [0; 32];
    require(
        info,
        file.read(&mut data) == Ok(11) && &data[..11] == b"portal-data",
        88,
    );
    require(
        info,
        file.write(b"denied") == Err(Error::Status(rt::Status::AccessDenied)),
        89,
    );
    require(info, file.close().is_ok(), 90);
    // A grant is move-only: polling again cannot mint another capability.
    require(
        info,
        readonly.poll() == Err(Error::Status(rt::Status::InvalidHandle)),
        91,
    );

    let deadline = now() + 25_000_000_000;
    let denied =
        sdk::request_file(PortalAccess::ReadWrite, deadline).unwrap_or_else(|_| rt::exit(92));
    rt::log(
        info.log,
        "PORTAL-PROBE: waiting for cancelled readwrite request\n",
    );
    require(
        info,
        poll(denied, deadline) == Err(Error::Status(rt::Status::AccessDenied)),
        93,
    );

    let deadline = now() + 100_000_000;
    let expires =
        sdk::request_file(PortalAccess::ReadOnly, deadline).unwrap_or_else(|_| rt::exit(94));
    require(
        info,
        poll(expires, deadline + 1_000_000_000) == Err(Error::Status(rt::Status::TimedOut)),
        95,
    );

    let cancelled = sdk::request_file(PortalAccess::ReadOnly, now() + 1_000_000_000)
        .unwrap_or_else(|_| rt::exit(96));
    require(info, cancelled.cancel().is_ok(), 97);
    require(
        info,
        cancelled.poll() == Err(Error::Status(rt::Status::Cancelled)),
        98,
    );
    require(
        info,
        cancelled.poll() == Err(Error::Status(rt::Status::InvalidHandle)),
        99,
    );
    rt::log(
        info.log,
        "PORTAL-PROBE: approved readonly file, denied write, cancel and expiry passed\n",
    );
    rt::exit(0)
}
