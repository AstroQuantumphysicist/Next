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
    let mut frame = [0; 52];
    loop {
        let status = rt::receive(info.endpoint, &mut frame);
        if status == rt::Status::WouldBlock as i64 {
            rt::yield_now();
            continue;
        }
        check(info, status == 52, "EVENT WAITER: receive failed\n");
        break;
    }
    let event = u64::from_le_bytes(frame[44..52].try_into().unwrap_or([0; 8]));
    let mode = frame[40];
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
