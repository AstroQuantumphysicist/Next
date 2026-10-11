#![no_std]
#![no_main]
use next_user::{
    self as rt,
    sdk::{self, Datagram, Error, Rights, Wait},
};
rt::entry!(probe);
fn require(ok: bool, code: i64) {
    if !ok {
        rt::exit(code);
    }
}
fn wait_packet(mode: u64, nonce: u64) -> [u8; 24] {
    let mut packet = [0; 24];
    packet[..8].copy_from_slice(b"NEXTWAIT");
    packet[8..16].copy_from_slice(&mode.to_le_bytes());
    packet[16..].copy_from_slice(&nonce.to_le_bytes());
    packet
}
fn probe(info: &rt::StartInfo) -> ! {
    let rights = Rights::READ
        .union(Rights::WRITE)
        .union(Rights::DUPLICATE)
        .union(Rights::WAIT);
    let socket = Datagram::create(0, rights).unwrap_or_else(|_| rt::exit(180));
    let token = socket.handle().raw();
    let denied = Err(Error::Status(rt::Status::AccessDenied));
    let invalid = rt::Status::InvalidArgument as i64;
    require(
        rt::call(rt::Syscall::DatagramCreate, [49199, rights.0, 0, 0, 0, 0]) == invalid,
        181,
    );
    require(
        rt::call(rt::Syscall::DatagramCreate, [0, 128, 0, 0, 0, 0]) == invalid,
        182,
    );
    let read_only = socket
        .duplicate(Rights::READ.union(Rights::WAIT))
        .unwrap_or_else(|_| rt::exit(183));
    require(read_only.send(b"denied") == denied, 184);
    let write_only = socket
        .duplicate(Rights::WRITE)
        .unwrap_or_else(|_| rt::exit(185));
    require(write_only.receive(&mut [0; 24]) == denied, 186);
    require(
        rt::call(rt::Syscall::DatagramSend, [token, 0, 1, 0, 0, 0]) == invalid,
        187,
    );
    require(
        rt::call(rt::Syscall::DatagramSend, [token, 0, 1473, 0, 0, 0]) == invalid,
        188,
    );
    require(
        rt::call(rt::Syscall::DatagramReceive, [token, 0, 1473, 0, 0, 0]) == invalid,
        189,
    );
    require(
        socket.poll_readable() == Err(Error::Status(rt::Status::WouldBlock)),
        190,
    );
    // A fresh process/clock nonce is echoed by the explicitly configured local
    // development peer. No app-controlled destination or DNS bypass exists.
    let mut nonce = [0; 24];
    nonce[..8].copy_from_slice(b"NEXTNONC");
    nonce[8..16].copy_from_slice(&info.pid.to_le_bytes());
    let now = sdk::clock().unwrap_or_else(|_| rt::exit(191));
    nonce[16..].copy_from_slice(&now.to_le_bytes());
    require(socket.send(&nonce) == Ok(nonce.len()), 192);
    let deadline = now.saturating_add(5_000_000_000);
    loop {
        match socket.receive(&mut [0; 1]) {
            Err(Error::Status(rt::Status::WouldBlock)) => {
                let now = sdk::clock().unwrap_or_else(|_| rt::exit(193));
                require(now < deadline, 194);
                if sdk::sleep_until(now.saturating_add(2_000_000)).is_err() {
                    rt::exit(195);
                }
            }
            Err(Error::Status(rt::Status::BufferTooSmall)) => break,
            _ => rt::exit(196),
        }
    }
    require(socket.poll_readable() == Ok(()), 197); // readiness cannot consume
    static READ_ONLY: [u8; 25] = *b"readonly-user-output-area";
    for bad in [
        0,
        READ_ONLY.as_ptr() as u64,
        next_abi::native::STACK_TOP - 8,
    ] {
        require(
            rt::call(rt::Syscall::DatagramReceive, [token, bad, 24, 0, 0, 0]) == invalid,
            198,
        );
    }
    let mut reply = [0; 24];
    require(
        read_only.receive(&mut reply) == Ok(24) && reply == nonce,
        199,
    );
    require(
        socket.poll_readable() == Err(Error::Status(rt::Status::WouldBlock)),
        200,
    );
    require(
        socket.receive(&mut reply) == Err(Error::Status(rt::Status::WouldBlock)),
        201,
    );
    // Empty datagrams are packets, distinct from an empty receive queue. Zero
    // lengths must not dereference otherwise invalid user buffer addresses.
    require(
        rt::call(rt::Syscall::DatagramSend, [token, u64::MAX, 0, 0, 0, 0]) == 0,
        210,
    );
    let empty_deadline = sdk::clock()
        .unwrap_or_else(|_| rt::exit(211))
        .saturating_add(5_000_000_000);
    loop {
        match socket.poll_readable() {
            Ok(()) => break,
            Err(Error::Status(rt::Status::WouldBlock)) => {
                let now = sdk::clock().unwrap_or_else(|_| rt::exit(211));
                require(now < empty_deadline, 212);
                if sdk::sleep_until(now.saturating_add(2_000_000)).is_err() {
                    rt::exit(213);
                }
            }
            _ => rt::exit(214),
        }
    }
    require(
        rt::call(rt::Syscall::DatagramReceive, [token, u64::MAX, 0, 0, 0, 0]) == 0,
        215,
    );
    require(
        socket.poll_readable() == Err(Error::Status(rt::Status::WouldBlock)),
        216,
    );
    // Readiness authority is WAIT, independently of READ/WRITE authority.
    require(
        write_only.poll_writable() == Err(Error::Status(rt::Status::AccessDenied)),
        217,
    );
    require(read_only.poll_writable() == Ok(()), 218);
    let dropped = wait_packet(2, info.pid);
    // Start pressure checks in a fresh quantum, keeping the full-queue poll
    // and registration together before an unrelated timer preemption.
    require(sdk::yield_now().is_ok(), 219);
    let mut pressure = false;
    for _ in 0..32 {
        match socket.send(&dropped) {
            Ok(24) => {}
            Err(Error::Status(rt::Status::WouldBlock)) => {
                pressure = true;
                break;
            }
            _ => rt::exit(219),
        }
    }
    require(pressure, 220);
    require(
        socket.poll_writable() == Err(Error::Status(rt::Status::WouldBlock)),
        221,
    );
    let deadline = sdk::clock()
        .unwrap_or_else(|_| rt::exit(222))
        .saturating_add(5_000_000_000);
    require(socket.wait_writable(Wait::Until(deadline)) == Ok(()), 223);
    require(socket.poll_writable() == Ok(()), 224);

    let delayed = wait_packet(1, info.pid);
    let before = sdk::clock().unwrap_or_else(|_| rt::exit(225));
    require(socket.send(&delayed) == Ok(24), 226);
    require(
        read_only.wait_readable(Wait::Until(before.saturating_add(5_000_000_000))) == Ok(()),
        227,
    );
    let after = sdk::clock().unwrap_or_else(|_| rt::exit(228));
    require(after.saturating_sub(before) >= 150_000_000, 229);
    require(socket.poll_readable() == Ok(()), 230);
    require(read_only.poll_readable() == Ok(()), 231);
    static WAIT_RO: [u8; 8] = *b"wait-RO!";
    for bad in [0, WAIT_RO.as_ptr() as u64, next_abi::native::STACK_TOP - 4] {
        require(
            rt::call(rt::Syscall::ObjectWait, [token, bad, 1, deadline, 0, 0]) == invalid,
            232,
        );
    }
    require(
        read_only.receive(&mut reply) == Ok(24) && reply == delayed,
        233,
    );
    require(socket.send(&dropped) == Ok(24), 234);
    let omitted_until = sdk::clock()
        .unwrap_or_else(|_| rt::exit(235))
        .saturating_add(150_000_000);
    require(
        socket.wait_readable(Wait::Until(omitted_until))
            == Err(Error::Status(rt::Status::TimedOut)),
        236,
    );
    require(
        socket.receive(&mut reply) == Err(Error::Status(rt::Status::WouldBlock)),
        237,
    );
    require(
        rt::call(
            rt::Syscall::ObjectWait,
            [token, reply.as_mut_ptr() as u64, 4, 0, 0, 0],
        ) == invalid,
        238,
    );
    rt::log(info.log, "UDP WAIT: delayed readable park, writable pressure wake, omitted deadline and duplicate/nonconsume authority passed\n");
    require(read_only.close().is_ok(), 202);
    require(
        read_only.receive(&mut reply) == Err(Error::Status(rt::Status::InvalidHandle)),
        203,
    );
    require(write_only.close().is_ok(), 204);
    require(socket.close().is_ok(), 205);
    require(
        socket.send(&nonce) == Err(Error::Status(rt::Status::InvalidHandle)),
        206,
    );
    // Closing the last reference frees the bounded connected socket resource.
    let next = Datagram::create(0, rights).unwrap_or_else(|_| rt::exit(207));
    require(next.handle().raw() != token, 208);
    require(next.close().is_ok(), 209);
    rt::log(info.log,"UDP PROBE: actual nonce echo fixed trusted peer 10.0.2.2:39097, authority/copyout/nonconsume/stale/reuse passed\n");
    rt::exit(47)
}
