#![no_std]
#![no_main]
use next_user as rt;
rt::entry!(supervise);
fn require(info: &rt::StartInfo, ok: bool, reason: &str) {
    if !ok {
        rt::log(info.log, reason);
        rt::exit(100);
    }
}
fn await_exit(handle: u64) -> Result<i64, i64> {
    loop {
        let mut exit_code = 0;
        let status = rt::wait_result(handle, &mut exit_code);
        if status == rt::Status::Success as i64 {
            return Ok(exit_code);
        }
        if status != rt::Status::WouldBlock as i64 {
            return Err(status);
        }
        rt::yield_now();
    }
}

fn blocking_wait(handle: u64, result: &mut i64) -> i64 {
    rt::call(
        rt::Syscall::ObjectWait,
        [
            handle,
            result as *mut i64 as u64,
            1,
            (rt::clock() as u64).saturating_add(2_000_000_000),
            0,
            0,
        ],
    )
}
fn start_event_waiter(info: &rt::StartInfo, event: u64, mode: u8) -> (u64, u64) {
    let mut pair = [0; 2];
    require(
        info,
        rt::channel_pair(&mut pair) == 0,
        "NEXTD: event waiter channel creation failed\n",
    );
    let child = rt::spawn_with_endpoint(info.launcher, "/system/event-wait.elf", pair[1]);
    require(
        info,
        child > 0 && rt::close(pair[1]) == 0,
        "NEXTD: event waiter spawn failed\n",
    );
    let duplicate = rt::call(rt::Syscall::HandleDuplicate, [event, 48, 0, 0, 0, 0]);
    require(
        info,
        duplicate > 0,
        "NEXTD: event waiter capability duplication failed\n",
    );
    let mut frame = rt::ipc_frame_with_handle(false, duplicate as u64);
    frame[40] = mode;
    require(
        info,
        rt::send_with_rights(pair[0], &frame, &[32]) == 0,
        "NEXTD: event waiter capability transfer failed\n",
    );
    (child as u64, pair[0])
}
fn start_event_waiter_many(info: &rt::StartInfo, events: [u64; 2]) -> (u64, u64) {
    let mut pair = [0; 2];
    require(
        info,
        rt::channel_pair(&mut pair) == 0,
        "NEXTD: wait-many channel creation failed\n",
    );
    let child = rt::spawn_with_endpoint(info.launcher, "/system/event-wait.elf", pair[1]);
    require(
        info,
        child > 0 && rt::close(pair[1]) == 0,
        "NEXTD: wait-many child spawn failed\n",
    );
    let first = rt::call(rt::Syscall::HandleDuplicate, [events[0], 48, 0, 0, 0, 0]);
    let second = rt::call(rt::Syscall::HandleDuplicate, [events[1], 48, 0, 0, 0, 0]);
    require(
        info,
        first > 0 && second > 0,
        "NEXTD: wait-many capability duplication failed\n",
    );
    let mut frame = rt::ipc_frame_with_handles(false, [first as u64, second as u64]);
    frame[40] = b'W';
    require(
        info,
        rt::send_with_rights(pair[0], &frame, &[32, 32]) == 0,
        "NEXTD: wait-many capability transfer failed\n",
    );
    (child as u64, pair[0])
}
fn waiter_ready(info: &rt::StartInfo, endpoint: u64) {
    let mut frame = [0; 44];
    let deadline = (rt::clock() as u64).saturating_add(2_000_000_000);
    loop {
        let status = rt::receive(endpoint, &mut frame);
        if status == rt::Status::WouldBlock as i64 && (rt::clock() as u64) < deadline {
            rt::yield_now();
            continue;
        }
        require(
            info,
            status == 44 && frame == rt::ipc_frame(true),
            "NEXTD: event waiter readiness failed\n",
        );
        break;
    }
}
fn blocking_event_checks(info: &rt::StartInfo, rights: next_abi::Rights) {
    let wait_many_events = [rt::event_create(0, rights), rt::event_create(0, rights)];
    require(
        info,
        wait_many_events.iter().all(|event| *event > 0),
        "NEXTD: wait-many event creation failed\n",
    );
    let (wait_many_child, wait_many_channel) = start_event_waiter_many(
        info,
        [wait_many_events[0] as u64, wait_many_events[1] as u64],
    );
    waiter_ready(info, wait_many_channel);
    rt::sleep_until((rt::clock() as u64).saturating_add(20_000_000));
    require(
        info,
        rt::event_signal(wait_many_events[1] as u64) == 0
            && rt::event_reset(wait_many_events[1] as u64) == 0,
        "NEXTD: wait-many signal/reset failed\n",
    );
    let mut wait_many_code = 99;
    require(
        info,
        blocking_wait(wait_many_child, &mut wait_many_code) == 0 && wait_many_code == 0,
        "NEXTD: atomic wait-many signal/reset completion failed\n",
    );
    require(
        info,
        rt::close(wait_many_child) == 0
            && rt::close(wait_many_channel) == 0
            && rt::close(wait_many_events[0] as u64) == 0
            && rt::close(wait_many_events[1] as u64) == 0,
        "NEXTD: wait-many cleanup failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: atomic wait-many timeout, signal/reset winner and cleanup passed\n",
    );

    let auto = rt::event_create(0, rights);
    require(
        info,
        auto > 0,
        "NEXTD: blocking auto-event creation failed\n",
    );
    let (child, channel) = start_event_waiter(info, auto as u64, b'A');
    waiter_ready(info, channel);
    require(
        info,
        rt::event_signal(auto as u64) == 0,
        "NEXTD: blocking auto-event signal failed\n",
    );
    require(
        info,
        rt::event_reset(auto as u64) == 0,
        "NEXTD: auto-event reset after blocked signal failed\n",
    );
    let mut code = 99;
    require(
        info,
        blocking_wait(child, &mut code) == 0 && code == 0,
        "NEXTD: blocking process wait failed\n",
    );
    require(
        info,
        rt::close(child) == 0 && rt::close(channel) == 0 && rt::close(auto as u64) == 0,
        "NEXTD: blocking auto-event cleanup failed\n",
    );
    let manual = rt::event_create(1, rights);
    require(
        info,
        manual > 0,
        "NEXTD: blocking manual-event creation failed\n",
    );
    let (first, first_channel) = start_event_waiter(info, manual as u64, b'M');
    let (second, second_channel) = start_event_waiter(info, manual as u64, b'M');
    waiter_ready(info, first_channel);
    waiter_ready(info, second_channel);
    require(
        info,
        rt::event_signal(manual as u64) == 0,
        "NEXTD: blocking manual-event signal failed\n",
    );
    require(
        info,
        blocking_wait(first, &mut code) == 0 && code == 0,
        "NEXTD: manual-event broadcast failed\n",
    );
    require(
        info,
        blocking_wait(second, &mut code) == 0 && code == 0,
        "NEXTD: second manual-event waiter failed\n",
    );
    require(
        info,
        rt::close(first) == 0
            && rt::close(second) == 0
            && rt::close(first_channel) == 0
            && rt::close(second_channel) == 0
            && rt::close(manual as u64) == 0,
        "NEXTD: manual-event broadcast cleanup failed\n",
    );
    let cancelled = rt::event_create(0, rights);
    require(
        info,
        cancelled > 0,
        "NEXTD: cancellation event creation failed\n",
    );
    let (child, channel) = start_event_waiter(info, cancelled as u64, b'K');
    waiter_ready(info, channel);
    rt::sleep_until((rt::clock() as u64).saturating_add(30_000_000));
    require(
        info,
        rt::kill(child, 77) == 0 && blocking_wait(child, &mut code) == 0 && code == 77,
        "NEXTD: blocked wait cancellation failed\n",
    );
    require(
        info,
        rt::close(child) == 0 && rt::close(channel) == 0 && rt::close(cancelled as u64) == 0,
        "NEXTD: blocked wait cancellation cleanup failed\n",
    );
    for _ in 0..32 {
        let event = rt::event_create(0, rights);
        require(
            info,
            event > 0 && rt::close(event as u64) == 0,
            "NEXTD: event reference leaked after wait cancellation\n",
        );
    }
    rt::log(info.log,"NEXTD: blocking event timeout, signal-reset auto wake, manual broadcast, process wait and kill cancellation passed\n");
}

static SERVICE_READONLY: [u8; next_abi::service::MAX_BYTES] = [0x5a; next_abi::service::MAX_BYTES];

struct ServiceDemo {
    manager: helix_core::service::Supervisor,
    workers: [u64; 2],
    channels: [u64; 2],
    config_bytes: [u8; next_abi::service::MAX_BYTES],
    config_len: usize,
    active_mask: u8,
    stage: u8,
    import_proof: u8,
    sequences: [u64; 2],
}
fn service_frame(kind: u64, sequence: u64) -> [u8; 56] {
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
impl ServiceDemo {
    fn new(info: &rt::StartInfo) -> Self {
        let mut small = [0xa5; 16];
        require(
            info,
            rt::service_configuration(&mut small) == rt::Status::BufferTooSmall as i64
                && small == [0xa5; 16],
            "NEXTD: service configuration partial copyout\n",
        );
        require(
            info,
            rt::call(
                rt::Syscall::ServiceConfiguration,
                [0, next_abi::service::MAX_BYTES as u64, 0, 0, 0, 0],
            ) == rt::Status::InvalidArgument as i64,
            "NEXTD: service configuration invalid copyout\n",
        );
        require(
            info,
            rt::call(
                rt::Syscall::ServiceConfiguration,
                [next_abi::native::STACK_TOP - 8, 16, 0, 0, 0, 0],
            ) == rt::Status::InvalidArgument as i64,
            "NEXTD: service configuration crossguard copyout\n",
        );
        require(
            info,
            rt::call(
                rt::Syscall::ServiceConfiguration,
                [
                    SERVICE_READONLY.as_ptr() as u64,
                    SERVICE_READONLY.len() as u64,
                    0,
                    0,
                    0,
                    0,
                ],
            ) == rt::Status::InvalidArgument as i64
                && (0..SERVICE_READONLY.len()).all(|index| unsafe {
                    core::ptr::read_volatile(SERVICE_READONLY.as_ptr().add(index)) == 0x5a
                }),
            "NEXTD: service configuration readonly copyout\n",
        );
        let mut config_bytes = [0; next_abi::service::MAX_BYTES];
        let config_len = rt::sdk::service_configuration(&mut config_bytes).unwrap_or_else(|_| {
            rt::log(
                info.log,
                "NEXTD: authenticated service configuration unavailable\n",
            );
            rt::exit(117)
        });
        let config = next_abi::service::Config::parse(&config_bytes[..config_len])
            .unwrap_or_else(|_| rt::exit(117));
        require(
            info,
            config.len() <= 2,
            "NEXTD: service configuration exceeds process profile\n",
        );
        let mut specs = [None; 8];
        for (index, spec) in specs.iter_mut().enumerate().take(config.len()) {
            if config.entry_enabled(index) {
                let entry = config.entry(index).unwrap_or_else(|| rt::exit(117));
                *spec = Some(helix_core::service::Spec {
                    dependencies: entry.dependencies,
                    max_starts: entry.max_starts,
                    window_ns: entry.window_ns,
                    backoff_ns: entry.backoff_ns,
                    max_backoff_ns: entry.max_backoff_ns,
                    health_ns: entry.health_ns,
                });
            }
        }
        let active_mask = config.active_mask();
        require(
            info,
            active_mask & 1 != 0,
            "NEXTD: primary audited service disabled\n",
        );
        rt::log(
            info.log,
            "NEXTD: authenticated service configuration and copyout checks passed\n",
        );
        Self {
            manager: helix_core::service::Supervisor::new(specs).unwrap_or_else(|_| rt::exit(117)),
            workers: [0; 2],
            channels: [0; 2],
            config_bytes,
            config_len,
            active_mask,
            stage: 0,
            import_proof: 0,
            sequences: [0; 2],
        }
    }
    fn trace(&self, info: &rt::StartInfo, index: usize, phase: &str) {
        use core::fmt::Write;
        struct Text {
            bytes: [u8; 256],
            len: usize,
        }
        impl Write for Text {
            fn write_str(&mut self, text: &str) -> core::fmt::Result {
                if text.len() > self.bytes.len() - self.len {
                    return Err(core::fmt::Error);
                }
                self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
                self.len += text.len();
                Ok(())
            }
        }
        let config = next_abi::service::Config::parse(&self.config_bytes[..self.config_len])
            .unwrap_or_else(|_| rt::exit(117));
        let entry = config.entry(index).unwrap_or_else(|| rt::exit(117));
        let mut text = Text {
            bytes: [0; 256],
            len: 0,
        };
        let _ = writeln!(
            text,
            "NEXTD SERVICE: index={} name={} phase={} instance={} dependencies={} ceiling={:#x}",
            index,
            entry.name,
            phase,
            self.workers[index],
            entry.dependencies,
            entry.capability_ceiling
        );
        rt::log(
            info.log,
            core::str::from_utf8(&text.bytes[..text.len])
                .unwrap_or("NEXTD: service trace failed\n"),
        );
    }
    fn request_import_denial(&mut self, info: &rt::StartInfo) {
        let socket = rt::call(rt::Syscall::DatagramCreate, [0, 0x1b, 0, 0, 0, 0]);
        if socket <= 0 {
            self.import_proof = 3;
            rt::log(
                info.log,
                "NEXTD: NETWORK capability import denial fixture unavailable\n",
            );
            return;
        }
        let mut bytes = [0; 64];
        let mut payload = [0; 16];
        payload[..8].copy_from_slice(&5u64.to_le_bytes());
        payload[8..].copy_from_slice(&1u64.to_le_bytes());
        let encoded = helix_core::ipc::encode(
            &mut bytes,
            helix_core::ipc::Header {
                opcode: 0x53,
                flags: 0,
                correlation_id: 1,
                deadline: 0,
            },
            &payload,
            &[helix_core::handle::Handle::from_raw(socket as u64)],
        );
        if !encoded.is_ok_and(|frame| frame.len() == 64)
            || rt::send_with_rights(self.channels[0], &bytes, &[1]) != 0
        {
            let _ = rt::close(socket as u64);
            self.import_proof = 3;
            rt::log(
                info.log,
                "NEXTD: NETWORK capability import denial fixture unavailable\n",
            );
            return;
        }
        require(
            info,
            rt::close(socket as u64) == rt::Status::InvalidHandle as i64,
            "NEXTD: moved NETWORK capability sender remained valid\n",
        );
        self.import_proof = 1;
        rt::log(
            info.log,
            "NEXTD: moved NETWORK capability queued for unprivileged recipient\n",
        );
    }
    fn retire(&mut self, info: &rt::StartInfo, index: usize, kill: bool) {
        let worker = self.workers[index];
        if worker == 0 {
            return;
        }
        if kill {
            let _ = rt::kill(worker, 81);
        }
        let _ = await_exit(worker);
        let _ = rt::close(worker);
        let _ = rt::close(self.channels[index]);
        require(
            info,
            self.manager
                .exited(index, worker, rt::clock() as u64)
                .is_ok(),
            "NEXTD: service instance retirement failed\n",
        );
        self.workers[index] = 0;
        self.channels[index] = 0;
        self.sequences[index] = 0;
    }
    fn launch(&mut self, info: &rt::StartInfo, index: usize) {
        let mut pair = [0; 2];
        if rt::channel_pair(&mut pair) != 0 {
            let _ = self.manager.start_failed(index, rt::clock() as u64);
            return;
        }
        let config = next_abi::service::Config::parse(&self.config_bytes[..self.config_len])
            .unwrap_or_else(|_| rt::exit(117));
        let entry = config.entry(index).unwrap_or_else(|| rt::exit(117));
        let worker = rt::call(
            rt::Syscall::ProcessSpawn,
            [
                info.launcher,
                entry.path.as_ptr() as u64,
                entry.path.len() as u64,
                pair[1],
                2,
                index as u64,
            ],
        );
        let _ = rt::close(pair[1]);
        if worker <= 0 {
            let _ = rt::close(pair[0]);
            let _ = self.manager.start_failed(index, rt::clock() as u64);
            return;
        }
        self.workers[index] = worker as u64;
        self.channels[index] = pair[0];
        self.sequences[index] = 0;
        require(
            info,
            self.manager
                .started(index, self.workers[index], rt::clock() as u64)
                .is_ok(),
            "NEXTD: service start registration failed\n",
        );
        self.trace(info, index, "started");
        rt::log(
            info.log,
            "NEXTD: service start charged against bounded restart budget\n",
        );
    }
    fn step(&mut self, info: &rt::StartInfo) {
        for index in 0..2 {
            if self.workers[index] != 0 {
                let mut code = 0;
                if rt::wait_result(self.workers[index], &mut code) == 0 {
                    self.retire(info, index, false);
                }
            }
            if self.workers[index] != 0 {
                for _ in 0..4 {
                    let mut bytes = [0; 56];
                    let received = rt::receive(self.channels[index], &mut bytes);
                    if received == rt::Status::WouldBlock as i64 {
                        break;
                    }
                    let packet = if received == 56 {
                        helix_core::ipc::decode(&bytes, 16, 0, rt::clock() as u64).ok()
                    } else {
                        None
                    };
                    let Some(packet) = packet else {
                        self.retire(info, index, true);
                        break;
                    };
                    if packet.header.opcode != 0x53
                        || packet.payload.len() != 16
                        || packet.header.flags != 0
                    {
                        self.retire(info, index, true);
                        break;
                    }
                    let kind = u64::from_le_bytes(packet.payload[..8].try_into().unwrap_or([0; 8]));
                    let sequence =
                        u64::from_le_bytes(packet.payload[8..].try_into().unwrap_or([0; 8]));
                    if sequence != packet.header.correlation_id || sequence <= self.sequences[index]
                    {
                        self.retire(info, index, true);
                        break;
                    }
                    self.sequences[index] = sequence;
                    if kind == 1 {
                        if self
                            .manager
                            .ready(index, self.workers[index], rt::clock() as u64)
                            .is_err()
                        {
                            break;
                        }
                        self.trace(info, index, "ready");
                        if index == 0 && self.stage == 0 && self.import_proof == 0 {
                            self.request_import_denial(info);
                        } else if index == 0 && self.stage == 1 {
                            require(
                                info,
                                rt::send(self.channels[index], &service_frame(3, 1)) == 0,
                                "NEXTD: service stall request failed\n",
                            );
                            self.stage = 2;
                        } else if index == 0 && self.stage == 3 {
                            self.stage = 4;
                            rt::log(info.log,"NEXTD: service crash restart, heartbeat watchdog recovery and PID1 survival passed\n");
                        }
                    } else if kind == 2 {
                        let _ =
                            self.manager
                                .heartbeat(index, self.workers[index], rt::clock() as u64);
                    } else if kind == 5 && index == 0 && self.import_proof == 1 {
                        self.import_proof = 2;
                        rt::log(
                            info.log,
                            "NEXTD: recipient NETWORK capability import denial acknowledged\n",
                        );
                    } else if kind != 4 {
                        self.retire(info, index, true);
                        break;
                    }
                }
            }
        }
        if self.stage == 0
            && matches!(self.import_proof, 2 | 3)
            && self.manager.phase(0) == Some(helix_core::service::Phase::Running)
            && (self.active_mask == 1
                || self.manager.phase(1) == Some(helix_core::service::Phase::Running))
        {
            if self.active_mask == 3 {
                rt::log(
                    info.log,
                    "NEXTD: signed dependency workers ready before isolated primary crash\n",
                );
            }
            rt::log(
                info.log,
                "NEXTD: intentionally killing ready service to exercise restart backoff\n",
            );
            self.stage = 1;
            self.retire(info, 0, true);
        }
        match self.manager.poll(rt::clock() as u64) {
            Ok(Some(helix_core::service::Action::Start(index))) => self.launch(info, index),
            Ok(Some(helix_core::service::Action::Stop { service: index, .. })) => {
                rt::log(
                    info.log,
                    "NEXTD: service heartbeat deadline expired; stopping isolated instance\n",
                );
                self.trace(info, index, "watchdog-or-dependency-stop");
                if index == 0 && self.stage == 2 {
                    self.stage = 3;
                }
                self.retire(info, index, true);
            }
            Ok(_) => {}
            Err(_) => {
                let _ = rt::log(info.log, "NEXTD: service policy clock/state error\n");
            }
        }
    }
    fn bootstrap(&mut self, info: &rt::StartInfo) {
        let deadline = (rt::clock() as u64).saturating_add(5_000_000_000);
        while self.stage < 4
            || (0..2).any(|index| {
                self.active_mask & (1 << index) != 0
                    && self.manager.phase(index) != Some(helix_core::service::Phase::Running)
            })
        {
            self.step(info);
            require(
                info,
                (rt::clock() as u64) < deadline,
                "NEXTD: service supervision selftest deadline failed\n",
            );
            rt::sleep_until((rt::clock() as u64).saturating_add(10_000_000));
        }
        if self.active_mask == 3 {
            rt::log(
                info.log,
                "NEXTD: signed two-service dependency readiness and isolated recovery passed\n",
            );
        }
    }
}

fn supervise(info: &rt::StartInfo) -> ! {
    rt::log(info.log, "NEXTD: native supervisor started\n");
    require(
        info,
        info.launcher != 0,
        "NEXTD: missing launcher capability\n",
    );
    let event_rights = next_abi::Rights::WAIT
        .union(next_abi::Rights::SIGNAL)
        .union(next_abi::Rights::DUPLICATE)
        .union(next_abi::Rights::TRANSFER);
    let auto = rt::event_create(2, event_rights);
    require(info, auto > 0, "NEXTD: auto-event creation failed\n");
    let mut event_status = 99;
    require(
        info,
        rt::wait_result(auto as u64, &mut event_status) == 0 && event_status == 0,
        "NEXTD: auto-event initial wait failed\n",
    );
    require(
        info,
        rt::wait_result(auto as u64, &mut event_status) == rt::Status::WouldBlock as i64,
        "NEXTD: auto-event did not consume signal\n",
    );
    require(
        info,
        rt::event_signal(auto as u64) == 0,
        "NEXTD: event signal failed\n",
    );
    require(
        info,
        rt::call(rt::Syscall::ObjectWait, [auto as u64, 0, 0, 0, 0, 0])
            == rt::Status::InvalidArgument as i64,
        "NEXTD: event invalid status pointer accepted\n",
    );
    require(
        info,
        rt::wait_result(auto as u64, &mut event_status) == 0,
        "NEXTD: invalid event copyout consumed signal\n",
    );
    let manual = rt::event_create(3, event_rights);
    require(info, manual > 0, "NEXTD: manual-event creation failed\n");
    require(
        info,
        rt::wait_result(manual as u64, &mut event_status) == 0
            && rt::wait_result(manual as u64, &mut event_status) == 0,
        "NEXTD: manual-event consumed signal\n",
    );
    require(
        info,
        rt::event_reset(manual as u64) == 0
            && rt::wait_result(manual as u64, &mut event_status) == rt::Status::WouldBlock as i64,
        "NEXTD: manual-event reset failed\n",
    );
    let wait_only = rt::call(
        rt::Syscall::HandleDuplicate,
        [manual as u64, next_abi::Rights::WAIT.0, 0, 0, 0, 0],
    );
    require(
        info,
        wait_only > 0 && rt::event_signal(wait_only as u64) == rt::Status::AccessDenied as i64,
        "NEXTD: event rights attenuation failed\n",
    );
    require(
        info,
        rt::close(wait_only as u64) == 0
            && rt::close(manual as u64) == 0
            && rt::close(auto as u64) == 0,
        "NEXTD: event close failed\n",
    );
    require(
        info,
        rt::wait_result(auto as u64, &mut event_status) == rt::Status::InvalidHandle as i64,
        "NEXTD: stale event handle accepted\n",
    );
    rt::log(
        info.log,
        "NEXTD: manual/auto event lifecycle and rights checks passed\n",
    );
    blocking_event_checks(info, event_rights);
    let file = rt::file_open(
        info.launcher,
        "/native/probe",
        3,
        next_abi::Rights::READ
            .union(next_abi::Rights::WRITE)
            .union(next_abi::Rights::DUPLICATE)
            .union(next_abi::Rights::TRANSFER),
    );
    require(info, file > 0, "NEXTD: native file open failed\n");
    require(
        info,
        rt::file_write(file as u64, rt::FILE_PROBE_CONTENT) == rt::FILE_PROBE_CONTENT.len() as i64,
        "NEXTD: native file write failed\n",
    );
    let mut stat = [0; 2];
    require(
        info,
        rt::file_stat(file as u64, &mut stat) == 0
            && stat == [rt::FILE_PROBE_CONTENT.len() as u64; 2],
        "NEXTD: native file stat failed\n",
    );
    require(
        info,
        rt::file_seek(file as u64, 0, 0) == 0,
        "NEXTD: native file seek failed\n",
    );
    require(
        info,
        rt::call(
            rt::Syscall::FileRead,
            [file as u64, next_abi::native::STACK_TOP - 4, 8, 0, 0, 0],
        ) == rt::Status::InvalidArgument as i64,
        "NEXTD: native file copyout accepted invalid range\n",
    );
    require(
        info,
        rt::file_stat(file as u64, &mut stat) == 0 && stat[1] == 0,
        "NEXTD: invalid copyout advanced file offset\n",
    );
    let mut file_bytes = [0; 64];
    let length = rt::file_read(file as u64, &mut file_bytes);
    require(
        info,
        length == rt::FILE_PROBE_CONTENT.len() as i64
            && file_bytes[..rt::FILE_PROBE_CONTENT.len()] == *rt::FILE_PROBE_CONTENT,
        "NEXTD: native file read failed\n",
    );
    let readonly = rt::call(
        rt::Syscall::HandleDuplicate,
        [file as u64, next_abi::Rights::READ.0, 0, 0, 0, 0],
    );
    require(info, readonly > 0, "NEXTD: file attenuation failed\n");
    require(
        info,
        rt::file_write(readonly as u64, b"forbidden") == rt::Status::AccessDenied as i64,
        "NEXTD: readonly file write accepted\n",
    );
    require(
        info,
        rt::close(readonly as u64) == 0,
        "NEXTD: readonly file close failed\n",
    );
    require(
        info,
        rt::file_read(readonly as u64, &mut file_bytes) == rt::Status::InvalidHandle as i64,
        "NEXTD: stale file handle accepted\n",
    );
    require(
        info,
        rt::file_seek(file as u64, 0, 0) == 0,
        "NEXTD: file rewind failed\n",
    );
    let moved_file = rt::call(
        rt::Syscall::HandleDuplicate,
        [
            file as u64,
            next_abi::Rights::READ.union(next_abi::Rights::TRANSFER).0,
            0,
            0,
            0,
            0,
        ],
    );
    require(
        info,
        moved_file > 0,
        "NEXTD: transferable file duplication failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: native file read/write/seek/stat and copyout checks passed\n",
    );
    let mut pair = [0; 2];
    require(
        info,
        rt::channel_pair(&mut pair) == 0,
        "NEXTD: channel creation failed\n",
    );
    let probe = rt::spawn_with_endpoint(info.launcher, "/system/probe.elf", pair[1]);
    require(info, probe > 0, "NEXTD: probe spawn failed\n");
    let duplicate = rt::call(
        rt::Syscall::HandleDuplicate,
        [probe as u64, next_abi::Rights::WAIT.0, 0, 0, 0, 0],
    );
    require(info, duplicate > 0, "NEXTD: process duplication failed\n");

    require(
        info,
        rt::close(pair[1]) == 0,
        "NEXTD: inherited endpoint close failed\n",
    );
    let moved = rt::call(
        rt::Syscall::HandleDuplicate,
        [
            info.log,
            next_abi::Rights::WRITE.union(next_abi::Rights::TRANSFER).0,
            0,
            0,
            0,
            0,
        ],
    );
    require(
        info,
        moved > 0,
        "NEXTD: transferable log duplication failed\n",
    );
    require(
        info,
        rt::send_with_rights(
            pair[0],
            &rt::ipc_frame_with_handles(false, [moved as u64, moved_file as u64]),
            &[0, next_abi::Rights::READ.0],
        ) == 0,
        "NEXTD: attenuated IPC request failed\n",
    );
    require(
        info,
        rt::log(moved as u64, "moved stale handle") == rt::Status::InvalidHandle as i64,
        "NEXTD: IPC move retained sender authority\n",
    );
    let mut response = [0; 52];
    loop {
        let count = rt::receive(pair[0], &mut response);
        if count == rt::Status::WouldBlock as i64 {
            rt::yield_now();
            continue;
        }
        require(
            info,
            count == 52
                && response[..28] == rt::ipc_frame(true)[..28]
                && response[28..32] == [1, 0, 0, 0]
                && response[32..44] == rt::ipc_frame(true)[32..44],
            "NEXTD: IPC reply failed\n",
        );
        break;
    }
    let received = u64::from_le_bytes(response[44..52].try_into().unwrap_or([0; 8]));
    require(
        info,
        rt::log(received, "NEXTD: transferred log authority written\n") >= 0,
        "NEXTD: transferred WRITE capability failed\n",
    );
    require(
        info,
        rt::close(received) == 0,
        "NEXTD: received capability close failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: move-only attenuated IPC handle transfer passed\n",
    );
    require(
        info,
        rt::close(file as u64) == 0,
        "NEXTD: native file close failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: read-only file capability transfer passed\n",
    );

    require(
        info,
        rt::close(pair[0]) == 0,
        "NEXTD: channel close failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: isolated-process IPC request/reply passed\n",
    );
    require(
        info,
        await_exit(probe as u64) == Ok(42),
        "NEXTD: probe result failed\n",
    );
    require(
        info,
        await_exit(probe as u64) == Ok(42),
        "NEXTD: repeated original wait failed\n",
    );
    require(
        info,
        await_exit(duplicate as u64) == Ok(42),
        "NEXTD: duplicate wait failed\n",
    );
    let bad_copyout = rt::call(
        rt::Syscall::ObjectWait,
        [probe as u64, next_abi::native::STACK_TOP - 4, 0, 0, 0, 0],
    );
    require(
        info,
        bad_copyout == rt::Status::InvalidArgument as i64,
        "NEXTD: wait copyout bounds accepted\n",
    );
    require(
        info,
        await_exit(probe as u64) == Ok(42),
        "NEXTD: bad copyout consumed exit result\n",
    );
    require(
        info,
        rt::close(probe as u64) == 0,
        "NEXTD: process close failed\n",
    );
    require(
        info,
        rt::wait_result(probe as u64, &mut 0) == rt::Status::InvalidHandle as i64,
        "NEXTD: stale process handle accepted\n",
    );
    require(
        info,
        await_exit(duplicate as u64) == Ok(42),
        "NEXTD: closing original invalidated duplicate\n",
    );
    require(
        info,
        rt::close(duplicate as u64) == 0,
        "NEXTD: duplicate process close failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: native exit and stale-handle checks passed\n",
    );
    let negative = rt::spawn(info.launcher, "/system/wait-negative.elf");
    require(
        info,
        negative > 0,
        "NEXTD: negative-exit child spawn failed\n",
    );
    require(
        info,
        await_exit(negative as u64) == Ok(-7),
        "NEXTD: negative exit confused with wait status\n",
    );
    require(
        info,
        rt::close(negative as u64) == 0,
        "NEXTD: negative-exit close failed\n",
    );
    rt::log(
        info.log,
        "NEXTD: retained waits, duplicate ownership, negative exit and copyout checks passed\n",
    );

    let fault = rt::spawn(info.launcher, "/system/fault.elf");
    require(info, fault > 0, "NEXTD: fault spawn failed\n");
    require(
        info,
        await_exit(fault as u64) == Ok(rt::Status::Faulted as i64),
        "NEXTD: fault containment failed\n",
    );
    rt::close(fault as u64);
    rt::log(
        info.log,
        "NEXTD: child fault contained; supervisor survived\n",
    );
    for path in [
        "/system/fault-supervisor.elf",
        "/system/fault-rx.elf",
        "/system/fault-nx.elf",
        "/system/fault-guard.elf",
        "/system/fault-ud.elf",
        "/system/fault-avx.elf",
        "/system/fault-fsgsbase.elf",
    ] {
        let child = rt::spawn(info.launcher, path);
        require(info, child > 0, "NEXTD: adversarial child spawn failed\n");
        require(
            info,
            await_exit(child as u64) == Ok(rt::Status::Faulted as i64),
            "NEXTD: adversarial fault escaped containment\n",
        );
        require(
            info,
            rt::close(child as u64) == 0,
            "NEXTD: adversarial child close failed\n",
        );
    }
    rt::log(
        info.log,
        "NEXTD: supervisor/RX/NX/guard/UD faults contained\n",
    );
    let context = rt::spawn(info.launcher, "/system/context.elf");
    require(info, context > 0, "NEXTD: context child spawn failed\n");
    require(
        info,
        await_exit(context as u64) == Ok(0),
        "NEXTD: IRQ integer/XMM/x87 state corrupted\n",
    );
    require(
        info,
        rt::close(context as u64) == 0,
        "NEXTD: context child close failed\n",
    );
    rt::log(info.log, "NEXTD: IRQ integer/XMM/x87 preservation passed\n");
    let spin = rt::spawn(info.launcher, "/system/spin.elf");
    require(info, spin > 0, "NEXTD: spin spawn failed\n");
    // A non-yielding child must be interrupted by the timer for this parent
    // to run again. Elapsed time also avoids killing it before it gets a turn.
    let until = rt::clock() as u64 + 80_000_000;
    while (rt::clock() as u64) < until {
        rt::yield_now();
    }
    require(
        info,
        rt::kill(spin as u64, 73) == 0,
        "NEXTD: child kill failed\n",
    );
    require(
        info,
        await_exit(spin as u64) == Ok(73),
        "NEXTD: killed child result failed\n",
    );
    rt::close(spin as u64);
    rt::log(info.log, "NEXTD: non-yielding child preempted and killed\n");
    let again = rt::spawn(info.launcher, "/system/probe.elf");
    require(
        info,
        again > 0 && again != probe,
        "NEXTD: process reuse failed\n",
    );
    require(
        info,
        await_exit(again as u64) == Ok(42),
        "NEXTD: reused process failed\n",
    );
    rt::close(again as u64);
    rt::log(info.log, "NEXTD: native lifecycle selftest passed\n");
    // Explicit supervisor-authority inheritance is used only by this original
    // namespace selftest; normal console and service launches use flags zero.
    let vfs_path = "/system/vfs-probe.elf";
    let vfs = rt::call(
        rt::Syscall::ProcessSpawn,
        [
            info.launcher,
            vfs_path.as_ptr() as u64,
            vfs_path.len() as u64,
            0,
            1,
            0,
        ],
    );
    require(
        info,
        vfs > 0,
        "NEXTD: privileged VFS fixture spawn failed\n",
    );
    require(
        info,
        await_exit(vfs as u64) == Ok(0),
        "NEXTD: VFS lifetime fixture failed\n",
    );
    require(
        info,
        rt::close(vfs as u64) == 0,
        "NEXTD: VFS fixture process close failed\n",
    );
    let mut service = ServiceDemo::new(info);
    service.bootstrap(info);
    require(
        info,
        rt::ready(info.launcher) == 0,
        "NEXTD: readiness rejected\n",
    );
    loop {
        service.step(info);
        rt::sleep_until((rt::clock() as u64).saturating_add(10_000_000));
    }
}
