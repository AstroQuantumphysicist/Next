#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(worker);
fn frame(kind: u64, sequence: u64) -> [u8; 56] {
    let mut bytes = [0; 56];
    let mut payload = [0; 16];
    payload[..8].copy_from_slice(&kind.to_le_bytes());
    payload[8..].copy_from_slice(&sequence.to_le_bytes());
    let _ = helix_core::ipc::encode(
        &mut bytes,
        helix_core::ipc::Header {
            opcode: 0x53,
            flags: 0,
            correlation_id: sequence,
            deadline: 0,
        },
        &payload,
        &[],
    );
    bytes
}
fn worker(info: &rt::StartInfo) -> ! {
    // The signed manifest may request NETWORK, but this configured worker's
    // authenticated execution grant is zero. Requests alone grant no access.
    if rt::call(rt::Syscall::DatagramCreate, [0, 3, 0, 0, 0, 0]) != rt::Status::AccessDenied as i64
    {
        rt::exit(117);
    }
    rt::log(
        info.log,
        "SERVICE WORKER: NETWORK declaration without execution grant denied UDP socket\n",
    );
    let name = b"next.test";
    if rt::call(
        rt::Syscall::LookupCreate,
        [name.as_ptr() as u64, name.len() as u64, 1, 1, 0, 0],
    ) != rt::Status::AccessDenied as i64
    {
        rt::exit(118);
    }
    rt::log(
        info.log,
        "SERVICE WORKER: NETWORK declaration without execution grant denied DNS lookup\n",
    );
    if info.endpoint == 0 || rt::send(info.endpoint, &frame(1, 1)) != 0 {
        rt::exit(110);
    }
    rt::log(
        info.log,
        "SERVICE WORKER: isolated worker ready with scoped channel/log capabilities\n",
    );
    let mut sequence = 2u64;
    let mut socket_import_denied = false;
    loop {
        let mut bytes = [0; 64];
        let status = rt::receive(info.endpoint, &mut bytes);
        if status == 56 {
            let packet = match helix_core::ipc::decode(&bytes[..56], 16, 0, rt::clock() as u64) {
                Ok(p) => p,
                Err(_) => rt::exit(111),
            };
            if packet.header.opcode != 0x53
                || packet.header.flags != 0
                || packet.payload.len() != 16
                || packet.payload[..8] != 3u64.to_le_bytes()
            {
                rt::exit(112);
            }
            if rt::send(info.endpoint, &frame(4, sequence)) != 0 {
                rt::exit(113);
            }
            rt::log(info.log, "SERVICE WORKER: intentional heartbeat stall\n");
            loop {
                rt::sleep_until((rt::clock() as u64).saturating_add(1_000_000_000));
            }
        } else if status == rt::Status::AccessDenied as i64 {
            // The socket remains in the queued frame when installation is
            // denied. Acknowledge once and keep normal health reporting; the
            // supervisor's explicit teardown releases the queued reference.
            if !socket_import_denied {
                rt::log(info.log,
                    "SERVICE WORKER: NETWORK socket capability import denied without execution grant\n");
                if rt::send(info.endpoint, &frame(5, sequence)) != 0 {
                    rt::exit(118);
                }
                sequence = sequence.checked_add(1).unwrap_or_else(|| rt::exit(116));
                socket_import_denied = true;
            }
        } else if status != rt::Status::WouldBlock as i64 {
            rt::exit(114);
        }
        if rt::send(info.endpoint, &frame(2, sequence)) != 0 {
            rt::exit(115);
        }
        sequence = match sequence.checked_add(1) {
            Some(next) => next,
            None => rt::exit(116),
        };
        rt::sleep_until((rt::clock() as u64).saturating_add(30_000_000));
    }
}
