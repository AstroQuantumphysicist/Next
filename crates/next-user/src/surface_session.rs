//! Borrowed client-side configuration state; no resize or focus authority.
use super::{Error, Result, Status, Surface, SurfaceBuffer, SurfaceConfigure, SurfaceFrame};
use next_abi::surface::{PixelFormat, Rect, MAX_DAMAGE, NATIVE_MAX_BUFFER_BYTES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceLayout {
    pub configure: SurfaceConfigure,
    pub stride_bytes: u32,
    pub buffer_bytes: usize,
}
impl SurfaceLayout {
    fn checked(configure: SurfaceConfigure) -> Result<Self> {
        configure.encode().map_err(|_| Error::InvalidResult)?;
        let stride_bytes = configure
            .pixel_width
            .checked_mul(4)
            .ok_or(Error::InvalidResult)?;
        let buffer_bytes = usize::try_from(stride_bytes)
            .ok()
            .and_then(|stride| stride.checked_mul(configure.pixel_height as usize))
            .filter(|&bytes| bytes != 0 && bytes <= NATIVE_MAX_BUFFER_BYTES)
            .ok_or(Error::InvalidResult)?;
        Ok(Self {
            configure,
            stride_bytes,
            buffer_bytes,
        })
    }
}

/// Local state for a borrowed surface. Copying or dropping this state neither
/// duplicates nor closes capability authority. A trusted reconfigure Reset
/// prompts `refresh`; this helper does not request server geometry or focus.
/// Close opt-in is explicit and reapplied only with a new successful Ack.
pub struct SurfaceSession {
    surface: Surface,
    close_events: bool,
    layout: Option<SurfaceLayout>,
    last_frame: u64,
}
impl SurfaceSession {
    /// Start on an unpublished surface and keep this session as its exclusive
    /// frame publisher. Manual publication or another session sharing the token
    /// requires independent frame-counter coordination, which this helper does
    /// not provide. Copying a borrowed token does not reserve publication rights.
    pub const fn new(surface: Surface, close_events: bool) -> Self {
        Self {
            surface,
            close_events,
            layout: None,
            last_frame: 0,
        }
    }
    pub fn surface(&self) -> Surface {
        self.surface
    }
    pub fn layout(&self) -> Option<SurfaceLayout> {
        self.layout
    }
    /// Requested policy, not a query of live server negotiation or focus.
    pub fn close_events_requested(&self) -> bool {
        self.close_events
    }
    pub fn last_accepted_frame(&self) -> u64 {
        self.last_frame
    }
    /// Read the server's latest immutable descriptor, then acknowledge it.
    pub fn refresh(&mut self) -> Result<SurfaceLayout> {
        let configure = self.surface.configure()?;
        self.acknowledge(configure)
    }
    /// Commit local geometry only after a successful native acknowledgment.
    /// An unchanged descriptor is repeatable without issuing a duplicate Ack.
    pub fn acknowledge(&mut self, configure: SurfaceConfigure) -> Result<SurfaceLayout> {
        let surface = self.surface;
        self.acknowledge_current_with(
            configure,
            || surface.configure(),
            |configuration, close| {
                if close {
                    surface.acknowledge_close_events(configuration)
                } else {
                    surface.acknowledge(configuration)
                }
            },
        )
    }
    fn acknowledge_current_with(
        &mut self,
        configure: SurfaceConfigure,
        current: impl FnOnce() -> Result<SurfaceConfigure>,
        acknowledge: impl FnOnce(SurfaceConfigure, bool) -> Result<()>,
    ) -> Result<SurfaceLayout> {
        // Native Ack checks identity, not the caller's geometry fields. Compare
        // the complete read-only server descriptor before adopting any layout.
        SurfaceLayout::checked(configure)?;
        if current()? != configure {
            return Err(Error::InvalidResult);
        }
        self.acknowledge_with(configure, acknowledge)
    }
    fn acknowledge_with(
        &mut self,
        configure: SurfaceConfigure,
        acknowledge: impl FnOnce(SurfaceConfigure, bool) -> Result<()>,
    ) -> Result<SurfaceLayout> {
        let next = SurfaceLayout::checked(configure)?;
        if let Some(old) = self.layout {
            if configure.surface_generation != old.configure.surface_generation
                || configure.configure_generation < old.configure.configure_generation
            {
                return Err(Error::InvalidResult);
            }
            if configure.configure_generation == old.configure.configure_generation {
                return if next == old {
                    Ok(old)
                } else {
                    Err(Error::InvalidResult)
                };
            }
        }
        acknowledge(configure, self.close_events)?;
        self.layout = Some(next);
        Ok(next)
    }
    /// Every frame supplies full damage, including the first after configure.
    /// Frame identity increases across reconfigure; a refused queue submission
    /// preserves both the counter and geometry for an explicit retry.
    pub fn publish_full(
        &mut self,
        buffer: SurfaceBuffer,
        format: PixelFormat,
        private_content: bool,
    ) -> Result<()> {
        let surface = self.surface;
        self.publish_with(
            buffer.byte_len(),
            buffer.handle().raw(),
            format,
            private_content,
            |frame| surface.publish(buffer, frame),
        )
    }
    fn publish_with(
        &mut self,
        buffer_bytes: usize,
        buffer_handle: u64,
        format: PixelFormat,
        private_content: bool,
        publish: impl FnOnce(&SurfaceFrame) -> Result<()>,
    ) -> Result<()> {
        let layout = self.layout.ok_or(Error::Status(Status::InvalidArgument))?;
        if buffer_bytes != layout.buffer_bytes {
            return Err(Error::Status(Status::InvalidArgument));
        }
        let generation = self.last_frame.checked_add(1).ok_or(Error::InvalidResult)?;
        let c = layout.configure;
        let mut damage = [Rect::EMPTY; MAX_DAMAGE];
        damage[0] = Rect {
            x: 0,
            y: 0,
            width: c.pixel_width,
            height: c.pixel_height,
        };
        let frame = SurfaceFrame {
            surface_generation: c.surface_generation,
            configure_generation: c.configure_generation,
            frame_generation: generation,
            buffer_handle,
            logical_width: c.logical_width,
            logical_height: c.logical_height,
            pixel_width: c.pixel_width,
            pixel_height: c.pixel_height,
            scale_16_16: c.scale_16_16,
            stride_bytes: layout.stride_bytes,
            buffer_bytes: layout.buffer_bytes as u64,
            private_content,
            format,
            damage_rectangles: damage,
            damage_count: 1,
        };
        let mut wire = [0; next_abi::surface::MAX_BYTES];
        frame
            .encode(&mut wire)
            .map_err(|_| Error::Status(Status::InvalidArgument))?;
        publish(&frame)?;
        self.last_frame = generation;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::Handle;
    fn configuration(generation: u64, width: u32, height: u32) -> SurfaceConfigure {
        SurfaceConfigure {
            surface_generation: 7,
            configure_generation: generation,
            logical_width: width,
            logical_height: height,
            pixel_width: width,
            pixel_height: height,
            scale_16_16: 65536,
        }
    }
    fn session(close: bool) -> SurfaceSession {
        SurfaceSession::new(Surface::from_handle(Handle::from_raw(1).unwrap()), close)
    }
    #[test]
    fn ack_failure_preserves_geometry_and_close_choice_and_stale_descriptors_never_call_gate() {
        let mut session = session(true);
        let first = configuration(1, 64, 48);
        assert_eq!(
            session.acknowledge_with(first, |_, _| Err(Error::Status(Status::WouldBlock))),
            Err(Error::Status(Status::WouldBlock))
        );
        assert_eq!(session.layout(), None);
        let old = session
            .acknowledge_with(first, |c, close| {
                assert_eq!(c, first);
                assert!(close);
                Ok(())
            })
            .unwrap();
        assert_eq!(old.buffer_bytes, 12288);
        assert_eq!(
            session.acknowledge_with(first, |_, _| panic!("duplicate native Ack")),
            Ok(old)
        );
        let next = configuration(2, 80, 40);
        assert_eq!(
            session.acknowledge_with(next, |_, close| {
                assert!(close);
                Err(Error::Status(Status::InvalidHandle))
            }),
            Err(Error::Status(Status::InvalidHandle))
        );
        assert_eq!(session.layout(), Some(old));
        for invalid in [
            configuration(0, 64, 48),
            configuration(1, 80, 40),
            SurfaceConfigure {
                surface_generation: 8,
                ..next
            },
            configuration(2, 8192, 8192),
            SurfaceConfigure {
                scale_16_16: 81920,
                ..next
            },
        ] {
            assert_eq!(
                session.acknowledge_with(invalid, |_, _| panic!("invalid configure reached gate")),
                Err(Error::InvalidResult)
            );
        }
        session
            .acknowledge_with(next, |_, close| {
                assert!(close);
                Ok(())
            })
            .unwrap();
        assert_eq!(session.layout().unwrap().buffer_bytes, 12800);
    }
    #[test]
    fn publication_retries_and_reconfigure_preserve_frame_monotonicity_and_full_damage() {
        let mut session = session(false);
        session
            .acknowledge_with(configuration(1, 64, 48), |_, close| {
                assert!(!close);
                Ok(())
            })
            .unwrap();
        let mut attempts = [0; 3];
        let mut index = 0;
        for result in [Err(Error::Status(Status::WouldBlock)), Ok(()), Ok(())] {
            let expected = result;
            assert_eq!(
                session.publish_with(12288, 2, PixelFormat::Bgrx, true, |frame| {
                    attempts[index] = frame.frame_generation;
                    index += 1;
                    assert!(frame.private_content);
                    assert_eq!(frame.damage_count, 1);
                    assert_eq!(
                        frame.damage_rectangles[0],
                        Rect {
                            x: 0,
                            y: 0,
                            width: 64,
                            height: 48
                        }
                    );
                    result
                }),
                expected
            );
        }
        assert_eq!(attempts, [1, 1, 2]);
        session
            .acknowledge_with(configuration(2, 80, 40), |_, _| Ok(()))
            .unwrap();
        assert_eq!(session.last_accepted_frame(), 2);
        assert!(session
            .publish_with(12288, 2, PixelFormat::Bgrx, false, |_| panic!(
                "old size reached gate"
            ))
            .is_err());
        session
            .publish_with(12800, 2, PixelFormat::Rgbx, false, |frame| {
                assert_eq!(frame.frame_generation, 3);
                assert_eq!(frame.configure_generation, 2);
                assert_eq!(
                    frame.damage_rectangles[0],
                    Rect {
                        x: 0,
                        y: 0,
                        width: 80,
                        height: 40
                    }
                );
                Ok(())
            })
            .unwrap();
        session.last_frame = u64::MAX;
        assert_eq!(
            session.publish_with(12800, 2, PixelFormat::Bgrx, false, |_| panic!(
                "wrapped frame reached gate"
            )),
            Err(Error::InvalidResult)
        );
        assert_eq!(session.last_accepted_frame(), u64::MAX);
    }
    #[test]
    fn supplied_first_descriptor_cannot_poison_layout_with_valid_but_forged_geometry() {
        let mut session = session(true);
        let actual = configuration(1, 64, 48);
        let forged = configuration(1, 80, 40);
        assert_eq!(
            session.acknowledge_current_with(
                forged,
                || Ok(actual),
                |_, _| panic!("forged geometry reached Ack")
            ),
            Err(Error::InvalidResult)
        );
        assert_eq!(session.layout(), None);
        assert_eq!(
            session.acknowledge_current_with(
                actual,
                || Err(Error::Status(Status::WouldBlock)),
                |_, _| panic!("failed read reached Ack")
            ),
            Err(Error::Status(Status::WouldBlock))
        );
        assert_eq!(session.layout(), None);
        let layout = session
            .acknowledge_current_with(actual, || Ok(actual), |_, _| Ok(()))
            .unwrap();
        assert_eq!(layout.buffer_bytes, 12288);
        let newer = configuration(2, 80, 40);
        assert_eq!(
            session.acknowledge_current_with(
                actual,
                || Ok(newer),
                |_, _| panic!("stale descriptor reached Ack")
            ),
            Err(Error::InvalidResult)
        );
        assert_eq!(session.layout(), Some(layout));
    }
    #[test]
    fn packed_fractional_scale_and_exact_budget_are_checked_before_ack() {
        let at_limit = configuration(1, 128, 128);
        assert_eq!(
            SurfaceLayout::checked(at_limit).unwrap().buffer_bytes,
            65536
        );
        assert!(SurfaceLayout::checked(configuration(1, 129, 128)).is_err());
        let fractional = SurfaceConfigure {
            logical_width: 3,
            logical_height: 5,
            pixel_width: 4,
            pixel_height: 7,
            scale_16_16: 81920,
            ..at_limit
        };
        assert_eq!(
            SurfaceLayout::checked(fractional).unwrap().buffer_bytes,
            112
        );
        let mut state = session(false);
        assert!(state
            .publish_with(4, 2, PixelFormat::Bgrx, false, |_| panic!(
                "unacked frame reached gate"
            ))
            .is_err());
    }
}
