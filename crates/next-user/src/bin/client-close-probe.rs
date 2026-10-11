#![no_std]
#![no_main]
//! Original cooperative-close client using an explicitly granted file namespace.
use next_abi::{
    client_close::{Event, Payload, Request, Response},
    surface::{PixelFormat, Rect, MAX_DAMAGE},
};
use next_user::{
    self as rt,
    sdk::{self, Error, Rights, Surface, SurfaceBuffer, SurfaceFrame, Wait},
};
rt::entry!(probe);
const PATH: &str = "/native/close-proof";
fn require(ok: bool, code: i64) {
    if !ok {
        rt::exit(code);
    }
}
fn clock() -> u64 {
    sdk::clock().unwrap_or_else(|_| rt::exit(400))
}
fn reply_args(cap: u64, request: Request, response: u64) -> [u64; 6] {
    [
        cap,
        request.surface_generation,
        request.configure_generation,
        request.ticket,
        request.sequence,
        response,
    ]
}
fn probe(info: &rt::StartInfo) -> ! {
    let held = rt::file_open(
        info.launcher,
        PATH,
        5,
        next_abi::Rights::READ.union(next_abi::Rights::WRITE),
    );
    require(held > 0, 401);
    let held = held as u64;
    require(rt::file_write(held, b"dirty-state") == 11, 402);
    let surface = Surface::create(
        64,
        48,
        Rights::READ
            .union(Rights::WRITE)
            .union(Rights::DUPLICATE)
            .union(Rights::WAIT),
    )
    .unwrap_or_else(|_| rt::exit(403));
    let configure = surface.configure().unwrap_or_else(|_| rt::exit(404));
    require(
        rt::call(
            rt::Syscall::SurfaceAcknowledge,
            [
                surface.handle().raw(),
                configure.surface_generation,
                configure.configure_generation,
                2,
                0,
                0,
            ],
        ) == rt::Status::InvalidArgument as i64,
        405,
    );
    require(surface.acknowledge_close_events(configure) == Ok(()), 406);
    let reader = surface
        .duplicate(Rights::READ)
        .unwrap_or_else(|_| rt::exit(407));
    let mut pixels = [0u8; 64 * 48 * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0xa0, 0x50, 0xa0, 0xff]);
    }
    let buffer = SurfaceBuffer::import(&pixels, Rights::READ).unwrap_or_else(|_| rt::exit(408));
    let mut rectangles = [Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    }; MAX_DAMAGE];
    rectangles[0] = Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 48,
    };
    let frame = SurfaceFrame {
        surface_generation: configure.surface_generation,
        configure_generation: configure.configure_generation,
        frame_generation: 1,
        buffer_handle: 1,
        logical_width: 64,
        logical_height: 48,
        pixel_width: 64,
        pixel_height: 48,
        scale_16_16: 65536,
        stride_bytes: 256,
        buffer_bytes: pixels.len() as u64,
        private_content: false,
        format: PixelFormat::Bgrx,
        damage_rectangles: rectangles,
        damage_count: 1,
    };
    require(
        surface.publish(buffer, &frame) == Ok(()) && buffer.close() == Ok(()),
        409,
    );
    rt::log(info.log, "CLIENT CLOSE PROBE: ready; negotiated v2; unknown Ack features preserved configuration; dirty file held\n");
    let mut close_count = 0;
    let mut previous_sequence = 0;
    loop {
        require(
            surface.wait_input(Wait::Until(clock().saturating_add(60_000_000_000))) == Ok(()),
            410,
        );
        let mut legacy_wire = [0u8; 64];
        let legacy = rt::call(
            rt::Syscall::SurfaceInputRead,
            [
                surface.handle().raw(),
                legacy_wire.as_mut_ptr() as u64,
                64,
                0,
                0,
                0,
            ],
        );
        if legacy == 64 {
            let event = next_abi::client_input::Event::decode(&legacy_wire)
                .unwrap_or_else(|_| rt::exit(426));
            require(
                event.surface_generation == configure.surface_generation
                    && event.configure_generation == configure.configure_generation
                    && event.sequence > previous_sequence,
                427,
            );
            previous_sequence = event.sequence;
            continue;
        }
        require(legacy == rt::Status::NotSupported as i64, 428);
        let mut untouched = [0x5au8; 64];
        require(
            rt::call(
                rt::Syscall::SurfaceInputRead,
                [
                    surface.handle().raw(),
                    untouched.as_mut_ptr() as u64,
                    63,
                    2,
                    0,
                    0,
                ],
            ) == rt::Status::BufferTooSmall as i64
                && untouched == [0x5a; 64],
            429,
        );
        static READ_ONLY: [u8; 64] = [0x5a; 64];
        for pointer in [
            0,
            READ_ONLY.as_ptr() as u64,
            next_abi::native::STACK_TOP - 32,
        ] {
            require(
                rt::call(
                    rt::Syscall::SurfaceInputRead,
                    [surface.handle().raw(), pointer, 64, 2, 0, 0],
                ) == rt::Status::InvalidArgument as i64,
                430,
            );
        }
        require(
            rt::call(
                rt::Syscall::SurfaceInputRead,
                [
                    surface.handle().raw(),
                    untouched.as_mut_ptr() as u64,
                    64,
                    3,
                    0,
                    0,
                ],
            ) == rt::Status::InvalidArgument as i64
                && untouched == [0x5a; 64],
            431,
        );
        require(surface.poll_input() == Ok(()), 432);
        let mut wire = [0u8; 64];
        let read = rt::call(
            rt::Syscall::SurfaceInputRead,
            [
                surface.handle().raw(),
                wire.as_mut_ptr() as u64,
                64,
                2,
                0,
                0,
            ],
        );
        require(read == 64, 411);
        let event = Event::decode(&wire).unwrap_or_else(|_| rt::exit(412));
        require(
            event.surface_generation == configure.surface_generation
                && event.configure_generation == configure.configure_generation
                && event.sequence > previous_sequence,
            413,
        );
        previous_sequence = event.sequence;
        let Payload::CloseRequested { ticket } = event.payload else {
            continue;
        };
        let request = Request {
            surface_generation: event.surface_generation,
            configure_generation: event.configure_generation,
            sequence: event.sequence,
            ticket,
        };
        require(
            reader.reply_close(request, Response::Refused)
                == Err(Error::Status(rt::Status::AccessDenied)),
            414,
        );
        require(
            rt::call(
                rt::Syscall::SurfaceCloseReply,
                reply_args(info.log, request, 1),
            ) == rt::Status::InvalidHandle as i64,
            415,
        );
        require(
            rt::call(
                rt::Syscall::SurfaceCloseReply,
                reply_args(surface.handle().raw(), request, 99),
            ) == rt::Status::InvalidArgument as i64,
            416,
        );
        let mut wrong = request;
        wrong.ticket = wrong.ticket.wrapping_add(1);
        require(
            rt::call(
                rt::Syscall::SurfaceCloseReply,
                reply_args(surface.handle().raw(), wrong, 1),
            ) < 0,
            417,
        );
        wrong = request;
        wrong.sequence = wrong.sequence.wrapping_add(1);
        require(
            rt::call(
                rt::Syscall::SurfaceCloseReply,
                reply_args(surface.handle().raw(), wrong, 1),
            ) < 0,
            418,
        );
        close_count += 1;
        if close_count == 1 {
            require(
                surface.reply_close(request, Response::Refused) == Ok(()),
                419,
            );
            require(
                rt::call(
                    rt::Syscall::SurfaceCloseReply,
                    reply_args(surface.handle().raw(), request, 1),
                ) < 0,
                420,
            );
            rt::log(info.log, "CLIENT CLOSE PROBE: refused first request; READ-only/foreign role/bad response/ticket/sequence denied; stale reply denied; process continues\n");
            continue;
        }
        require(
            close_count == 2 && surface.reply_close(request, Response::WillClose) == Ok(()),
            421,
        );
        rt::log(
            info.log,
            "CLIENT CLOSE PROBE: WillClose acknowledged; process remains alive while saving\n",
        );
        require(
            sdk::sleep_until(clock().saturating_add(500_000_000)) == Ok(()),
            422,
        );
        require(
            rt::file_seek(held, 0, 0) == 0
                && rt::file_write(held, b"saved-close") == 11
                && rt::file_seek(held, 0, 0) == 0,
            423,
        );
        let mut saved = [0u8; 11];
        require(
            rt::file_read(held, &mut saved) == 11 && saved == *b"saved-close",
            424,
        );
        require(rt::close(held) == 0 && reader.close() == Ok(()), 425);
        rt::log(info.log, "CLIENT CLOSE PROBE: saved file readback verified; cooperative exit; no automatic kernel termination\n");
        rt::exit(51);
    }
}
