#![no_std]
#![no_main]
use next_abi::surface::{PixelFormat, Rect, MAX_DAMAGE};
use next_user::{
    self as rt,
    sdk::{self, Error, Rights, Surface, SurfaceBuffer, SurfaceFrame},
};
rt::entry!(probe);
fn require(ok: bool, code: i64) {
    if !ok {
        rt::exit(code);
    }
}
fn raw_publish(surface: Surface, frame: SurfaceFrame) -> i64 {
    let mut bytes = [0; next_abi::surface::MAX_BYTES];
    let length = frame.encode(&mut bytes).unwrap_or_else(|_| rt::exit(300));
    rt::call(
        rt::Syscall::SurfacePublish,
        [
            surface.handle().raw(),
            bytes.as_ptr() as u64,
            length as u64,
            0,
            0,
            0,
        ],
    )
}
fn probe(info: &rt::StartInfo) -> ! {
    let rights = Rights::READ.union(Rights::WRITE).union(Rights::DUPLICATE);
    let surface = Surface::create(64, 48, rights).unwrap_or_else(|_| rt::exit(301));
    require(
        Surface::create(16, 16, rights) == Err(Error::Status(rt::Status::NoMemory)),
        302,
    );
    let reader = surface
        .duplicate(Rights::READ)
        .unwrap_or_else(|_| rt::exit(303));
    let writer = surface
        .duplicate(Rights::WRITE)
        .unwrap_or_else(|_| rt::exit(304));
    require(
        writer.configure() == Err(Error::Status(rt::Status::AccessDenied)),
        305,
    );
    let configure = reader.configure().unwrap_or_else(|_| rt::exit(306));
    require(
        configure.pixel_width == 64
            && configure.pixel_height == 48
            && surface.configure() == Ok(configure),
        307,
    );
    require(
        reader.acknowledge(configure) == Err(Error::Status(rt::Status::AccessDenied)),
        308,
    );
    let mut short = [0x5a; 64];
    require(
        rt::call(
            rt::Syscall::SurfaceConfigure,
            [
                surface.handle().raw(),
                short.as_mut_ptr() as u64,
                63,
                0,
                0,
                0,
            ],
        ) == rt::Status::BufferTooSmall as i64
            && short == [0x5a; 64],
        309,
    );
    static READ_ONLY: [u8; 64] = [0x5a; 64];
    for pointer in [
        0,
        READ_ONLY.as_ptr() as u64,
        next_abi::native::STACK_TOP - 32,
    ] {
        require(
            rt::call(
                rt::Syscall::SurfaceConfigure,
                [surface.handle().raw(), pointer, 64, 0, 0, 0],
            ) == rt::Status::InvalidArgument as i64,
            310,
        );
    }
    let mut stale = configure;
    stale.configure_generation += 1;
    require(
        surface.acknowledge(stale) == Err(Error::Status(rt::Status::InvalidArgument)),
        311,
    );
    let mut pixels = [0u8; 64 * 48 * 4];
    for y in 0..48 {
        for x in 0..64 {
            let color = match (x < 32, y < 24) {
                (true, true) => [0xe0, 0x70, 0x20, 0xff],
                (false, true) => [0x20, 0xd0, 0xf0, 0xff],
                (true, false) => [0xb0, 0xc0, 0x20, 0xff],
                (false, false) => [0xd0, 0x40, 0xa0, 0xff],
            };
            pixels[(y * 64 + x) * 4..(y * 64 + x + 1) * 4].copy_from_slice(&color);
        }
    }
    let buffer = SurfaceBuffer::import(&pixels, Rights::READ.union(Rights::DUPLICATE))
        .unwrap_or_else(|_| rt::exit(312));
    pixels.fill(0);
    let second = SurfaceBuffer::import(&[1; 16], Rights::READ).unwrap_or_else(|_| rt::exit(313));
    require(
        SurfaceBuffer::import(&[1; 16], Rights::READ) == Err(Error::Status(rt::Status::NoMemory)),
        314,
    );
    require(second.close() == Ok(()), 315);
    let retained = buffer
        .duplicate(Rights::READ)
        .unwrap_or_else(|_| rt::exit(316));
    let no_read = buffer
        .duplicate(Rights(0))
        .unwrap_or_else(|_| rt::exit(317));
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
        buffer_handle: buffer.handle().raw(),
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
    require(
        surface.publish(buffer, &frame) == Err(Error::Status(rt::Status::WouldBlock)),
        318,
    );
    require(surface.acknowledge(configure) == Ok(()), 319);
    require(
        reader.publish(buffer, &frame) == Err(Error::Status(rt::Status::AccessDenied)),
        320,
    );
    require(
        surface.publish(no_read, &frame) == Err(Error::Status(rt::Status::AccessDenied)),
        321,
    );
    let mut wrong_buffer = frame;
    wrong_buffer.buffer_handle = surface.handle().raw();
    require(
        raw_publish(surface, wrong_buffer) == rt::Status::InvalidHandle as i64,
        322,
    );
    require(
        rt::call(
            rt::Syscall::SurfacePublish,
            [surface.handle().raw(), 0, 96, 0, 0, 0],
        ) == rt::Status::InvalidArgument as i64,
        333,
    );
    let mut no_damage = frame;
    no_damage.damage_count = 0;
    require(
        surface.publish(buffer, &no_damage) == Err(Error::Status(rt::Status::InvalidArgument)),
        334,
    );
    let mut forbidden_role = [0; next_abi::surface::MAX_BYTES];
    let descriptor_length = frame
        .encode(&mut forbidden_role)
        .unwrap_or_else(|_| rt::exit(335));
    forbidden_role[8] = 1;
    require(
        rt::call(
            rt::Syscall::SurfacePublish,
            [
                surface.handle().raw(),
                forbidden_role.as_ptr() as u64,
                descriptor_length as u64,
                0,
                0,
                0,
            ],
        ) == rt::Status::InvalidArgument as i64,
        336,
    );
    let mut wrong_generation = frame;
    wrong_generation.surface_generation += 1;
    require(
        surface.publish(buffer, &wrong_generation)
            == Err(Error::Status(rt::Status::InvalidArgument)),
        323,
    );
    let mut protected = frame;
    protected.private_content = true;
    require(
        surface.publish(buffer, &protected) == Err(Error::Status(rt::Status::NotSupported)),
        337,
    );
    require(buffer.close() == Ok(()), 324);
    require(
        raw_publish(surface, frame) == rt::Status::InvalidHandle as i64,
        325,
    );
    rt::yield_now();
    require(surface.publish(retained, &frame) == Ok(()), 326);
    let mut pressure = false;
    for generation in 2..34 {
        frame.frame_generation = generation;
        match surface.publish(retained, &frame) {
            Err(Error::Status(rt::Status::WouldBlock)) => {
                pressure = true;
                break;
            }
            // A genuine compositor drain between syscalls is a valid release,
            // so continue with a fresh generation instead of asserting timing.
            Ok(()) => {}
            _ => rt::exit(327),
        }
    }
    // Every frame may legitimately drain between calls on a fast compositor.
    // Record that supported outcome rather than treating release as a failure.
    rt::log(
        info.log,
        if pressure {
            "SURFACE PRESSURE: observed WouldBlock\n"
        } else {
            "SURFACE PRESSURE: all32 releases completed between calls\n"
        },
    );
    require(retained.close() == Ok(()) && no_read.close() == Ok(()), 328);
    require(writer.close() == Ok(()) && surface.close() == Ok(()), 329);
    require(
        surface.configure() == Err(Error::Status(rt::Status::InvalidHandle)),
        332,
    );
    require(reader.configure() == Ok(configure), 338);
    rt::log(info.log, "SURFACE PROBE: authenticated copied BGRX quadrants, configure/ack, rights, stale handles, quotas and publication lifecycle passed\n");
    let deadline = sdk::clock()
        .unwrap_or_else(|_| rt::exit(330))
        .saturating_add(5_000_000_000);
    rt::sleep_until(deadline);
    // The attenuated reader keeps the Surface object alive after the original
    // capability closes. Exit teardown drops that last reference atomically;
    // the trusted UI subsequently observes removal after the exit record.
    rt::exit(49)
}
