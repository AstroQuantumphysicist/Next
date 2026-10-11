//! Original shared native input fixture; optional bounded startup delay changes no authority.
use core::fmt::{self, Write};
use next_abi::{
    client_input::{Event, Key, Payload},
    surface::{PixelFormat, Rect, MAX_DAMAGE},
};
use next_user::{
    self as rt,
    sdk::{self, Error, Rights, Surface, SurfaceBuffer, SurfaceFrame, Wait},
};
struct Line {
    bytes: [u8; 192],
    len: usize,
}
impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.len() > self.bytes.len() - self.len {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
fn require(ok: bool, code: i64) {
    if !ok {
        rt::exit(code);
    }
}
fn clock() -> u64 {
    sdk::clock().unwrap_or_else(|_| rt::exit(350))
}
fn event_log(info: &rt::StartInfo, event: Event) {
    let (kind, x, y, buttons, scalar) = match event.payload {
        Payload::FocusGain => (1, 0, 0, 0, 0),
        Payload::FocusLoss => (2, 0, 0, 0, 0),
        Payload::Key { key, .. } => (
            3,
            0,
            0,
            0,
            if let Key::Character(c) = key {
                c as u32
            } else {
                0
            },
        ),
        Payload::Pointer { x, y, buttons } => (4, x, y, buttons, 0),
        Payload::Reset { reason } => (5, 0, 0, reason, 0),
    };
    let mut line = Line {
        bytes: [0; 192],
        len: 0,
    };
    let _ = writeln!(line, "CLIENT INPUT EVENT: seq={} configure={} kind={} x={} y={} buttons={} scalar={} timestamp={}",
        event.sequence, event.configure_generation, kind, x, y, buttons, scalar, event.timestamp_ns);
    rt::log(
        info.log,
        core::str::from_utf8(&line.bytes[..line.len]).unwrap_or(""),
    );
}
pub fn probe(info: &rt::StartInfo, publication_delay_ns: u64) -> ! {
    let surface = Surface::create(
        64,
        48,
        Rights::READ
            .union(Rights::WRITE)
            .union(Rights::WAIT)
            .union(Rights::DUPLICATE),
    )
    .unwrap_or_else(|_| rt::exit(351));
    let configure = surface.configure().unwrap_or_else(|_| rt::exit(352));
    require(surface.acknowledge(configure) == Ok(()), 353);
    let reader = surface
        .duplicate(Rights::READ)
        .unwrap_or_else(|_| rt::exit(354));
    let waiter = surface
        .duplicate(Rights::WAIT)
        .unwrap_or_else(|_| rt::exit(355));
    require(
        reader.poll_input() == Err(Error::Status(rt::Status::AccessDenied)),
        356,
    );
    require(
        waiter.read_input() == Err(Error::Status(rt::Status::AccessDenied)),
        357,
    );
    require(
        surface.poll_input() == Err(Error::Status(rt::Status::WouldBlock)),
        358,
    );
    let mut pixels = [0u8; 64 * 48 * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0xb0, 0x60, 0x30, 0xff]);
    }
    let mut damage = [Rect::EMPTY; MAX_DAMAGE];
    damage[0] = Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 48,
    };
    let mut frame = SurfaceFrame {
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
        damage_rectangles: damage,
        damage_count: 1,
    };
    let buffer = SurfaceBuffer::import(&pixels, Rights::READ).unwrap_or_else(|_| rt::exit(359));
    if publication_delay_ns != 0 {
        rt::log(
            info.log,
            "CLIENT INPUT PROBE: bounded publication delay started; no focus requested\n",
        );
        require(
            sdk::sleep_until(clock().saturating_add(publication_delay_ns)) == Ok(()),
            375,
        );
    }
    require(
        surface.publish(buffer, &frame) == Ok(()) && buffer.close() == Ok(()),
        360,
    );
    rt::log(info.log, "CLIENT INPUT PROBE: ready; creation/publication did not focus; READ/WAIT attenuation passed\n");
    let mut sequence = 0;
    let mut timestamp = 0;
    let mut checked_copyout = false;
    let mut portal = None;
    loop {
        require(
            surface.wait_input(Wait::Until(clock().saturating_add(60_000_000_000))) == Ok(()),
            361,
        );
        if !checked_copyout {
            let mut unchanged = [0x5a; 64];
            require(
                rt::call(
                    rt::Syscall::SurfaceInputRead,
                    [
                        surface.handle().raw(),
                        unchanged.as_mut_ptr() as u64,
                        63,
                        0,
                        0,
                        0,
                    ],
                ) == rt::Status::BufferTooSmall as i64
                    && unchanged == [0x5a; 64],
                362,
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
                        [surface.handle().raw(), pointer, 64, 0, 0, 0],
                    ) == rt::Status::InvalidArgument as i64,
                    363,
                );
            }
            require(surface.poll_input() == Ok(()), 364);
            checked_copyout = true;
            rt::log(info.log, "CLIENT INPUT PROBE: short/null/readonly/guard copyout preserved head event and level readiness\n");
        }
        let event = surface.read_input().unwrap_or_else(|_| rt::exit(365));
        require(
            event.sequence > sequence
                && event.timestamp_ns >= timestamp
                && event.surface_generation == configure.surface_generation
                && event.configure_generation == configure.configure_generation,
            366,
        );
        sequence = event.sequence;
        timestamp = event.timestamp_ns;
        event_log(info, event);
        if let Some(request) = portal {
            match sdk::FileRequest::poll(request) {
                Err(Error::Status(rt::Status::WouldBlock)) => {}
                Err(Error::Status(rt::Status::AccessDenied)) => {
                    portal = None;
                    rt::log(info.log, "CLIENT INPUT PROBE: trusted portal denied; client focus needs a new gesture\n");
                }
                Ok(file) => {
                    require(file.close() == Ok(()), 371);
                    portal = None;
                }
                _ => rt::exit(372),
            }
        }
        if matches!(
            event.payload,
            Payload::Key {
                key: Key::Character('o'),
                ..
            }
        ) {
            require(portal.is_none(), 373);
            portal = Some(
                sdk::request_file(
                    sdk::PortalAccess::ReadOnly,
                    clock().saturating_add(30_000_000_000),
                )
                .unwrap_or_else(|_| rt::exit(374)),
            );
            rt::log(info.log, "CLIENT INPUT PROBE: requested trusted portal; secure overlay must suspend client input\n");
        }
        if matches!(
            event.payload,
            Payload::Key {
                key: Key::Character('q'),
                ..
            }
        ) {
            rt::log(info.log, "CLIENT INPUT PROBE: real gesture-routed input, owned response frames and monotonic queue passed\n");
            rt::exit(50);
        }
        let color = match event.payload {
            Payload::Key {
                key: Key::Character('a'),
                ..
            } => [0x30, 0xd0, 0x30, 0xff],
            Payload::Key {
                key: Key::Character('b'),
                ..
            } => [0x20, 0xc0, 0xf0, 0xff],
            Payload::Pointer { buttons, .. } if buttons != 0 => [0xe0, 0x70, 0x20, 0xff],
            Payload::FocusGain => [0xc0, 0x80, 0x40, 0xff],
            Payload::FocusLoss | Payload::Reset { .. } => [0x70, 0x70, 0x70, 0xff],
            _ => [0xa0, 0x50, 0xa0, 0xff],
        };
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }
        let buffer = SurfaceBuffer::import(&pixels, Rights::READ).unwrap_or_else(|_| rt::exit(367));
        frame.frame_generation = event
            .sequence
            .checked_add(1)
            .unwrap_or_else(|| rt::exit(368));
        loop {
            match surface.publish(buffer, &frame) {
                Ok(()) => break,
                Err(Error::Status(rt::Status::WouldBlock)) => rt::yield_now(),
                _ => rt::exit(369),
            }
        }
        require(buffer.close() == Ok(()), 370);
        if matches!(
            event.payload,
            Payload::Key {
                key: Key::Character('p'),
                ..
            }
        ) {
            rt::log(
                info.log,
                "CLIENT INPUT PROBE: pausing dequeue for bounded overflow test\n",
            );
            rt::sleep_until(clock().saturating_add(2_000_000_000));
        }
    }
}
