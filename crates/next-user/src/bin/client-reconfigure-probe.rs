#![no_std]
#![no_main]
use core::{
    cell::UnsafeCell,
    fmt::{self, Write},
};
use next_abi::{
    client_input::{Key, Payload, RESET_RECONFIGURE},
    surface::{PixelFormat, Rect, MAX_DAMAGE},
};
use next_user::{
    self as rt,
    sdk::{
        self, Error, Rights, Surface, SurfaceBuffer, SurfaceFrame, SurfaceLayout, SurfaceSession,
        Wait,
    },
};
struct PixelStorage(UnsafeCell<[u8; next_abi::surface::NATIVE_MAX_BUFFER_BYTES]>);
unsafe impl Sync for PixelStorage {}
// This original app has one thread. Static storage leaves its guarded 64KiB
// stack available for actual SDK/syscall frames at the maximum pixel budget.
static PIXELS: PixelStorage = PixelStorage(UnsafeCell::new(
    [0; next_abi::surface::NATIVE_MAX_BUFFER_BYTES],
));
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
fn frame_log(info: &rt::StartInfo, layout: SurfaceLayout, session: &SurfaceSession) {
    let mut line = Line {
        bytes: [0; 192],
        len: 0,
    };
    let _ = writeln!(
        line,
        "CLIENT RECONFIGURE FRAME: configure={} frame={} width={} height={} stride={} bytes={}",
        layout.configure.configure_generation,
        session.last_accepted_frame(),
        layout.configure.pixel_width,
        layout.configure.pixel_height,
        layout.stride_bytes,
        layout.buffer_bytes
    );
    rt::log(
        info.log,
        core::str::from_utf8(&line.bytes[..line.len]).unwrap_or(""),
    );
}
rt::entry!(probe);
fn require(condition: bool, code: i64) {
    if !condition {
        rt::exit(code);
    }
}
fn now() -> u64 {
    sdk::clock().unwrap_or_else(|_| rt::exit(380))
}
fn pattern(bytes: &mut [u8], layout: SurfaceLayout) {
    let c = layout.configure;
    require(
        c.scale_16_16 == 65536
            && c.logical_width == c.pixel_width
            && c.logical_height == c.pixel_height
            && layout.buffer_bytes <= next_abi::surface::NATIVE_MAX_BUFFER_BYTES,
        381,
    );
    for y in 0..c.pixel_height as usize {
        for x in 0..c.pixel_width as usize {
            let colors = if c.configure_generation == 1 {
                [[0xb0, 0x60, 0x30, 0xff], [0x20, 0xc0, 0xf0, 0xff]]
            } else {
                [[0x30, 0xd0, 0x30, 0xff], [0xa0, 0x50, 0xa0, 0xff]]
            };
            let begin = y * layout.stride_bytes as usize + x * 4;
            bytes[begin..begin + 4].copy_from_slice(&colors[(x / 8 + y / 8) % 2]);
        }
    }
}
fn descriptor(layout: SurfaceLayout, frame_generation: u64) -> SurfaceFrame {
    let c = layout.configure;
    let mut damage = [Rect::EMPTY; MAX_DAMAGE];
    damage[0] = Rect {
        x: 0,
        y: 0,
        width: c.pixel_width,
        height: c.pixel_height,
    };
    SurfaceFrame {
        surface_generation: c.surface_generation,
        configure_generation: c.configure_generation,
        frame_generation,
        buffer_handle: 1,
        logical_width: c.logical_width,
        logical_height: c.logical_height,
        pixel_width: c.pixel_width,
        pixel_height: c.pixel_height,
        scale_16_16: c.scale_16_16,
        stride_bytes: layout.stride_bytes,
        buffer_bytes: layout.buffer_bytes as u64,
        private_content: false,
        format: PixelFormat::Bgrx,
        damage_rectangles: damage,
        damage_count: 1,
    }
}
fn raw_publish(surface: Surface, buffer: SurfaceBuffer, mut frame: SurfaceFrame) -> i64 {
    frame.buffer_handle = buffer.handle().raw();
    let mut wire = [0u8; next_abi::surface::MAX_BYTES];
    let len = frame.encode(&mut wire).unwrap_or_else(|_| rt::exit(403));
    rt::call(
        rt::Syscall::SurfacePublish,
        [
            surface.handle().raw(),
            wire.as_ptr() as u64,
            len as u64,
            0,
            0,
            0,
        ],
    )
}
fn publish(session: &mut SurfaceSession, buffer: SurfaceBuffer) {
    let deadline = now().saturating_add(5_000_000_000);
    loop {
        match session.publish_full(buffer, PixelFormat::Bgrx, false) {
            Ok(()) => break,
            Err(Error::Status(rt::Status::WouldBlock)) if now() < deadline => rt::yield_now(),
            _ => rt::exit(382),
        }
    }
}
fn probe(info: &rt::StartInfo) -> ! {
    let surface = Surface::create(
        64,
        48,
        Rights::READ.union(Rights::WRITE).union(Rights::WAIT),
    )
    .unwrap_or_else(|_| rt::exit(383));
    let mut session = SurfaceSession::new(surface, false);
    let mut layout = session.refresh().unwrap_or_else(|_| rt::exit(384));
    let pixels = unsafe { &mut *PIXELS.0.get() };
    pattern(pixels, layout);
    let mut buffer = SurfaceBuffer::import(&pixels[..layout.buffer_bytes], Rights::READ)
        .unwrap_or_else(|_| rt::exit(385));
    publish(&mut session, buffer);
    frame_log(info, layout, &session);
    rt::log(info.log,"CLIENT RECONFIGURE PROBE: 64x48 full checkerboard accepted; waiting for trusted resize, no focus requested\n");
    let mut sequence = 0;
    let mut timestamp = 0;
    let mut reconfigured = false;
    loop {
        require(
            surface.wait_input(Wait::Until(now().saturating_add(120_000_000_000))) == Ok(()),
            386,
        );
        let event = surface.read_input().unwrap_or_else(|_| rt::exit(387));
        require(
            event.surface_generation == layout.configure.surface_generation
                && event.sequence > sequence
                && event.timestamp_ns >= timestamp,
            388,
        );
        sequence = event.sequence;
        timestamp = event.timestamp_ns;
        if matches!(event.payload,Payload::Reset{reason} if reason & RESET_RECONFIGURE != 0) {
            // A future trusted caller may supersede an earlier pending
            // configure. A Reset already covered by our accepted layout needs
            // no duplicate Ack or publication.
            if event.configure_generation <= layout.configure.configure_generation {
                continue;
            }
            let old = layout;
            require(
                event.configure_generation > old.configure.configure_generation,
                389,
            );
            let current = surface.configure().unwrap_or_else(|_| rt::exit(390));
            require(
                current.configure_generation >= event.configure_generation
                    && current.surface_generation == event.surface_generation,
                391,
            );
            require(
                surface.acknowledge(old.configure)
                    == Err(Error::Status(rt::Status::InvalidArgument)),
                392,
            );
            let old_frame = descriptor(old, session.last_accepted_frame() + 1);
            require(
                surface.publish(buffer, &old_frame) == Err(Error::Status(rt::Status::WouldBlock)),
                393,
            );
            let refresh_deadline = now().saturating_add(2_000_000_000);
            layout = loop {
                match session.refresh() {
                    Ok(layout) => break layout,
                    Err(
                        Error::InvalidResult
                        | Error::Status(rt::Status::WouldBlock | rt::Status::InvalidArgument),
                    ) if now() < refresh_deadline => rt::yield_now(),
                    _ => rt::exit(394),
                }
            };
            require(
                surface.publish(buffer, &old_frame)
                    == Err(Error::Status(rt::Status::InvalidArgument)),
                401,
            );
            pattern(pixels, layout);
            if old.buffer_bytes != layout.buffer_bytes {
                let wrong = descriptor(layout, session.last_accepted_frame() + 1);
                require(
                    raw_publish(surface, buffer, wrong) == rt::Status::InvalidArgument as i64,
                    395,
                );
            }
            require(buffer.close() == Ok(()), 396);
            let stale_buffer_frame = descriptor(layout, session.last_accepted_frame() + 1);
            require(
                raw_publish(surface, buffer, stale_buffer_frame)
                    == rt::Status::InvalidHandle as i64,
                402,
            );
            buffer = SurfaceBuffer::import(&pixels[..layout.buffer_bytes], Rights::READ)
                .unwrap_or_else(|_| rt::exit(397));
            publish(&mut session, buffer);
            reconfigured = true;
            frame_log(info, layout, &session);
            rt::log(info.log,"CLIENT RECONFIGURE PROBE: dynamic full checkerboard accepted; raw stale config/frame/buffer checks passed; fresh focus required\n");
        } else {
            require(
                event.configure_generation == layout.configure.configure_generation,
                398,
            );
            if matches!(
                event.payload,
                Payload::Key {
                    key: Key::Character('q'),
                    ..
                }
            ) {
                require(reconfigured, 399);
                require(buffer.close() == Ok(()) && surface.close() == Ok(()), 400);
                rt::log(info.log,"CLIENT RECONFIGURE PROBE: dynamic bounded geometry and exact generation handshake passed\n");
                rt::exit(52);
            }
        }
    }
}
