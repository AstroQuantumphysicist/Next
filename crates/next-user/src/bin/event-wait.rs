#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(waiter);
fn check(info: &rt::StartInfo, ok: bool, message: &str) {
    if !ok {
        rt::log(info.log, message);
        rt::exit(101);
    }
}
fn block(handle: u64, result: &mut i64, deadline: u64) -> i64 {
    rt::call(
        rt::Syscall::ObjectWait,
        [handle, result as *mut i64 as u64, 1, deadline, 0, 0],
    )
}
fn waiter(info: &rt::StartInfo) -> ! {
    let mut frame = [0; 60];
    loop {
        let status = rt::receive(info.endpoint, &mut frame);
        if status == rt::Status::WouldBlock as i64 {
            rt::yield_now();
            continue;
        }
        check(
            info,
            status == 52 || status == 60,
            "EVENT WAITER: receive failed\n",
        );
        break;
    }
    let mode = frame[40];
    if mode == b'W' {
        check(
            info,
            frame[44..60].len() == 16,
            "EVENT WAITER: handles missing\n",
        );
        let first = u64::from_le_bytes(frame[44..52].try_into().unwrap_or([0; 8]));
        let second = u64::from_le_bytes(frame[52..60].try_into().unwrap_or([0; 8]));
        let handles = [
            rt::sdk::Handle::from_raw(first).unwrap_or_else(|_| rt::exit(101)),
            rt::sdk::Handle::from_raw(second).unwrap_or_else(|_| rt::exit(101)),
        ];
        let deadline = (rt::clock() as u64).saturating_add(30_000_000);
        check(
            info,
            rt::sdk::Handle::wait_many(&handles, rt::sdk::Wait::Until(deadline))
                == Err(rt::sdk::Error::Status(rt::Status::TimedOut)),
            "EVENT WAITER: wait-many timeout failed\n",
        );
        check(
            info,
            rt::send(info.endpoint, &rt::ipc_frame(true)) == 0,
            "EVENT WAITER: wait-many ready acknowledgement failed\n",
        );
        rt::log(info.log, "EVENT WAITER: parking in wait-many\n");
        check(
            info,
            rt::sdk::Handle::wait_many(&handles, rt::sdk::Wait::Block) == Ok(1),
            "EVENT WAITER: wait-many selected index or reset completion failed\n",
        );
        check(
            info,
            rt::sdk::Handle::wait_many(&handles, rt::sdk::Wait::Poll)
                == Err(rt::sdk::Error::Status(rt::Status::WouldBlock)),
            "EVENT WAITER: wait-many poll parked or consumed an extra signal\n",
        );
        check(
            info,
            handles[0].close().is_ok()
                && handles[1].close().is_ok()
                && rt::close(info.endpoint) == 0,
            "EVENT WAITER: wait-many cleanup failed\n",
        );
        rt::log(
            info.log,
            "EVENT WAITER: atomic wait-many timeout, poll, signal/reset and selected index passed\n",
        );
        rt::exit(0)
    }
    let event = u64::from_le_bytes(frame[44..52].try_into().unwrap_or([0; 8]));
    let mut result = 99;
    if mode != b'K' {
        let deadline = (rt::clock() as u64).saturating_add(30_000_000);
        check(
            info,
            block(event, &mut result, deadline) == rt::Status::TimedOut as i64 && result == 99,
            "EVENT WAITER: blocking timeout failed\n",
        );
    }
    check(
        info,
        rt::send(info.endpoint, &rt::ipc_frame(true)) == 0,
        "EVENT WAITER: ready acknowledgement failed\n",
    );
    rt::log(
        info.log,
        "EVENT WAITER: parking with owned WAIT capability\n",
    );
    check(
        info,
        block(event, &mut result, 0) == 0 && result == 0,
        "EVENT WAITER: signal wake failed\n",
    );
    check(
        info,
        mode != b'K',
        "EVENT WAITER: cancelled waiter unexpectedly resumed\n",
    );
    let repeated = rt::wait_result(event, &mut result);
    check(
        info,
        repeated
            == if mode == b'M' {
                0
            } else {
                rt::Status::WouldBlock as i64
            },
        "EVENT WAITER: reset mode wake semantics failed\n",
    );
    check(
        info,
        rt::close(event) == 0 && rt::close(info.endpoint) == 0,
        "EVENT WAITER: cleanup failed\n",
    );
    rt::log(
        info.log,
        "EVENT WAITER: blocking timeout, wake and reset checks passed\n",
    );
    rt::exit(0)
}
