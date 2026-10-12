#![no_std]
#![no_main]
use next_user::{
    self as rt,
    sdk::{self, Error, Handle, Rights, Timer, Wait},
};
rt::entry!(probe);
fn check(ok: bool, code: i64) {
    if !ok {
        rt::exit(code);
    }
}
fn now() -> u64 {
    sdk::clock().unwrap_or_else(|_| rt::exit(401))
}
fn probe(info: &rt::StartInfo) -> ! {
    let all = Rights(next_abi::TIMER_RIGHTS);
    check(
        rt::call(rt::Syscall::TimerCreate, [0, Rights::WRITE.0, 0, 0, 0, 0])
            == rt::Status::InvalidArgument as i64,
        402,
    );
    let expired = Timer::create(0, all).unwrap_or_else(|_| rt::exit(403));
    check(expired.wait(Wait::Until(1)) == Ok(()), 404); // terminal readiness wins over expired wait deadline
    let weak = expired
        .duplicate(Rights::WAIT)
        .unwrap_or_else(|_| rt::exit(405));
    check(
        weak.cancel() == Err(Error::Status(rt::Status::AccessDenied)),
        406,
    );
    check(
        weak.duplicate(Rights::WAIT) == Err(Error::Status(rt::Status::AccessDenied)),
        407,
    );
    check(
        rt::call(
            rt::Syscall::ObjectWait,
            [
                expired.handle().raw(),
                next_abi::native::STACK_TOP - 4,
                0,
                0,
                0,
                0,
            ],
        ) == rt::Status::InvalidArgument as i64,
        408,
    );
    check(
        expired.cancel() == Ok(())
            && expired.wait(Wait::Poll) == Ok(())
            && expired.wait(Wait::Poll) == Ok(()),
        409,
    );
    let old = expired.handle().raw();
    check(
        expired.close() == Ok(()) && weak.wait(Wait::Poll) == Ok(()),
        410,
    );
    check(
        rt::close(old) == rt::Status::InvalidHandle as i64 && weak.close() == Ok(()),
        411,
    );
    let future = Timer::create(now() + 1_000_000_000, all).unwrap_or_else(|_| rt::exit(412));
    check(
        future.wait(Wait::Poll) == Err(Error::Status(rt::Status::WouldBlock)),
        413,
    );
    check(
        future.wait(Wait::Until(0)) == Err(Error::Status(rt::Status::TimedOut))
            && future.wait(Wait::Poll) == Err(Error::Status(rt::Status::WouldBlock)),
        433,
    );
    check(
        future.wait(Wait::Until(now() + 30_000_000)) == Err(Error::Status(rt::Status::TimedOut)),
        414,
    );
    check(future.cancel() == Ok(()) && future.cancel() == Ok(()), 415);
    check(
        future.wait(Wait::Block) == Err(Error::Status(rt::Status::Cancelled))
            && future.close() == Ok(()),
        416,
    );
    let deadline = now() + 50_000_000;
    let timed = Timer::create(deadline, all).unwrap_or_else(|_| rt::exit(417));
    check(
        timed.wait(Wait::Until(deadline)) == Ok(()) && now() >= deadline,
        418,
    );
    check(
        timed.cancel() == Ok(()) && timed.wait(Wait::Poll) == Ok(()),
        419,
    );
    // Real queued MOVE keeps the timer alive and attenuates the imported token.
    let mut pair = [0; 2];
    check(rt::channel_pair(&mut pair) == 0, 420);
    let bytes = rt::ipc_frame_with_handle(false, timed.handle().raw());
    check(
        rt::send_with_rights(pair[0], &bytes, &[Rights::WAIT.0]) == 0,
        421,
    );
    check(
        timed.wait(Wait::Poll) == Err(Error::Status(rt::Status::InvalidHandle)),
        422,
    );
    let mut received = [0; 52];
    check(rt::receive(pair[1], &mut received) == 52, 423);
    let raw = u64::from_le_bytes(received[44..52].try_into().unwrap_or([0; 8]));
    let moved = Timer::from_handle(Handle::from_raw(raw).unwrap_or_else(|_| rt::exit(424)));
    check(
        moved.wait(Wait::Poll) == Ok(())
            && moved.cancel() == Err(Error::Status(rt::Status::AccessDenied)),
        425,
    );
    check(
        moved.close() == Ok(()) && rt::close(pair[0]) == 0 && rt::close(pair[1]) == 0,
        426,
    );
    let a = Timer::create(u64::MAX, all).unwrap_or_else(|_| rt::exit(427));
    let b = Timer::create(u64::MAX, all).unwrap_or_else(|_| rt::exit(428));
    check(
        Timer::create(u64::MAX, all) == Err(Error::Status(rt::Status::NoMemory)),
        429,
    );
    check(a.close() == Ok(()), 430);
    // Deliberately leave the two live objects to normal process-exit cleanup.
    let _ = Timer::create(u64::MAX, all).unwrap_or_else(|_| rt::exit(431));
    check(
        b.wait(Wait::Poll) == Err(Error::Status(rt::Status::WouldBlock)),
        432,
    );
    rt::log(info.log,"TIMER PROBE: monotonic expiry, zero/absolute deadlines, cancellation, rights, queued transfer, stale handles and quotas passed\n");
    rt::exit(58)
}
