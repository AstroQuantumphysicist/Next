#![no_std]
#![no_main]
use next_user::{
    self as rt,
    sdk::{self, Error, Lookup, LookupFamily, Rights, Wait},
};
rt::entry!(probe);
fn require(ok: bool, code: i64) {
    if !ok {
        rt::exit(code);
    }
}
fn now() -> u64 {
    sdk::clock().unwrap_or_else(|_| rt::exit(250))
}
fn begin(name: &str, family: LookupFamily, deadline: u64) -> Lookup {
    Lookup::begin(
        name,
        family,
        Rights::READ.union(Rights::WAIT).union(Rights::DUPLICATE),
        Some(deadline),
    )
    .unwrap_or_else(|_| rt::exit(251))
}
fn probe(info: &rt::StartInfo) -> ! {
    let invalid = rt::Status::InvalidArgument as i64;
    let name = b"next.test";
    for arguments in [
        [0, 9, 1, 1, 0, 0],
        [name.as_ptr() as u64, 0, 1, 1, 0, 0],
        [name.as_ptr() as u64, 254, 1, 1, 0, 0],
        [name.as_ptr() as u64, 9, 2, 1, 0, 0],
        [name.as_ptr() as u64, 9, 1, 2, 0, 0],
        [name.as_ptr() as u64, 9, 1, 1, 0, 1],
    ] {
        require(
            rt::call(rt::Syscall::LookupCreate, arguments) == invalid,
            252,
        );
    }
    let first = begin(
        "next.test",
        LookupFamily::A,
        now().saturating_add(5_000_000_000),
    );
    let token = first.handle().raw();
    let read = first
        .duplicate(Rights::READ)
        .unwrap_or_else(|_| rt::exit(253));
    let wait = first
        .duplicate(Rights::WAIT)
        .unwrap_or_else(|_| rt::exit(254));
    require(
        read.wait(Wait::Poll) == Err(Error::Status(rt::Status::AccessDenied)),
        255,
    );
    require(
        wait.result() == Err(Error::Status(rt::Status::AccessDenied)),
        256,
    );
    require(
        first.wait(Wait::Until(now().saturating_add(5_000_000_000))) == Ok(()),
        257,
    );
    let mut untouched = [0x5a; 144];
    require(
        rt::call(
            rt::Syscall::LookupResult,
            [token, untouched.as_mut_ptr() as u64, 143, 0, 0, 0],
        ) == rt::Status::BufferTooSmall as i64,
        258,
    );
    require(untouched == [0x5a; 144], 259);
    static READ_ONLY: [u8; 144] = [0x5a; 144];
    for pointer in [
        0,
        READ_ONLY.as_ptr() as u64,
        next_abi::native::STACK_TOP - 64,
    ] {
        require(
            rt::call(rt::Syscall::LookupResult, [token, pointer, 144, 0, 0, 0]) == invalid,
            260,
        );
    }
    let answer = first.result().unwrap_or_else(|_| rt::exit(261));
    require(
        answer.family == 1
            && answer.count == 1
            && answer.addresses[0][..4] == [10, 0, 2, 2]
            && answer.ttl_seconds <= 2,
        262,
    );
    require(
        first.result() == Ok(answer)
            && read.result() == Ok(answer)
            && wait.wait(Wait::Poll) == Ok(()),
        263,
    );
    require(
        rt::call(
            rt::Syscall::ObjectWait,
            [token, untouched.as_mut_ptr() as u64, 2, 0, 0, 0],
        ) == invalid,
        264,
    );
    require(first.close() == Ok(()), 265);
    require(
        first.result() == Err(Error::Status(rt::Status::InvalidHandle))
            && read.result() == Ok(answer),
        266,
    );
    require(read.close() == Ok(()) && wait.close() == Ok(()), 267);
    let cached = begin(
        "next.test",
        LookupFamily::A,
        now().saturating_add(5_000_000_000),
    );
    let answer = cached.result().unwrap_or_else(|_| rt::exit(268));
    require(
        answer.flags & next_abi::dns_lookup::CACHE_HIT != 0
            && answer.addresses[0][..4] == [10, 0, 2, 2],
        269,
    );
    require(cached.close() == Ok(()), 270);
    let ipv6 = begin(
        "next.test",
        LookupFamily::Aaaa,
        now().saturating_add(5_000_000_000),
    );
    require(
        ipv6.wait(Wait::Until(now().saturating_add(5_000_000_000))) == Ok(()),
        271,
    );
    let answer = ipv6.result().unwrap_or_else(|_| rt::exit(272));
    let mut loopback = [0; 16];
    loopback[15] = 1;
    require(
        answer.family == 28 && answer.count == 1 && answer.addresses[0] == loopback,
        273,
    );
    require(ipv6.close() == Ok(()), 274);
    for (name, status) in [
        ("unknown.next.test", rt::Status::NotFound),
        ("dns-truncated.next.test", rt::Status::NotSupported),
        ("dns-alias.next.test", rt::Status::NotSupported),
        ("dns-referral.next.test", rt::Status::NotSupported),
    ] {
        let lookup = begin(name, LookupFamily::A, now().saturating_add(5_000_000_000));
        require(
            lookup.wait(Wait::Until(now().saturating_add(5_000_000_000))) == Ok(()),
            275,
        );
        require(
            lookup.result() == Err(Error::Status(status))
                && lookup.result() == Err(Error::Status(status)),
            276,
        );
        require(lookup.close() == Ok(()), 277);
    }
    for name in ["dns-timeout.next.test", "dns-malformed.next.test"] {
        let lookup = begin(name, LookupFamily::A, now().saturating_add(300_000_000));
        require(
            lookup.wait(Wait::Until(now().saturating_add(2_000_000_000)))
                == Err(Error::Status(rt::Status::TimedOut)),
            278,
        );
        require(
            lookup.result() == Err(Error::Status(rt::Status::TimedOut)),
            279,
        );
        require(lookup.close() == Ok(()), 280);
        require(
            lookup.result() == Err(Error::Status(rt::Status::InvalidHandle)),
            281,
        );
    }
    let mut held = [None; 4];
    for request in &mut held {
        *request = Some(begin(
            "dns-timeout.next.test",
            LookupFamily::A,
            now().saturating_add(5_000_000_000),
        ));
    }
    require(
        Lookup::begin("next.test", LookupFamily::A, Rights::READ, None)
            == Err(Error::Status(rt::Status::NoMemory)),
        282,
    );
    for request in held.into_iter().flatten() {
        require(request.close() == Ok(()), 283);
    }
    let reopened = begin(
        "next.test",
        LookupFamily::A,
        now().saturating_add(5_000_000_000),
    );
    require(
        reopened.wait(Wait::Until(now().saturating_add(5_000_000_000))) == Ok(()),
        284,
    );
    require(reopened.result().is_ok() && reopened.close() == Ok(()), 285);
    rt::log(info.log,"DNS PROBE: actual A/AAAA peer answers, cache, readonly/short copyout, retained duplicate results, stale handles, negative/unsupported and malformed/timeout passed\n");
    rt::exit(48)
}
