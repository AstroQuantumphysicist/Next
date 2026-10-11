//! Configure/ack ordering foundation, not a running compositor.
//! Owner values must come from the authenticated channel/buffer broker, not wire
//! client claims. No capability, mapping, fence or display isolation is implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub logical_width: u32,
    pub logical_height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_16_16: u32,
}
impl Geometry {
    pub fn validate(self) -> Result<Self, Error> {
        for extent in [
            self.logical_width,
            self.logical_height,
            self.pixel_width,
            self.pixel_height,
        ] {
            if extent == 0 || extent > 8192 {
                return Err(Error::Geometry);
            }
        }
        if !(65536..=262144).contains(&self.scale_16_16) {
            return Err(Error::Geometry);
        }
        for (logical, pixel) in [
            (self.logical_width, self.pixel_width),
            (self.logical_height, self.pixel_height),
        ] {
            if (u64::from(logical) * u64::from(self.scale_16_16)).div_ceil(65536)
                != u64::from(pixel)
            {
                return Err(Error::Geometry);
            }
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentity,
    Owner,
    StaleSurface,
    StaleConfigure,
    Unacknowledged,
    StaleFrame,
    Geometry,
    Exhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configure {
    pub generation: u64,
    pub geometry: Geometry,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceState {
    owner: u64,
    surface_generation: u64,
    next_configure: u64,
    pending: Option<Configure>,
    active: Option<Configure>,
    last_frame: u64,
}
impl SurfaceState {
    pub fn new(authenticated_owner: u64, surface_generation: u64) -> Result<Self, Error> {
        if authenticated_owner == 0 || surface_generation == 0 {
            return Err(Error::InvalidIdentity);
        }
        Ok(Self {
            owner: authenticated_owner,
            surface_generation,
            next_configure: 1,
            pending: None,
            active: None,
            last_frame: 0,
        })
    }
    fn authority(&self, owner: u64, surface_generation: u64) -> Result<(), Error> {
        if owner != self.owner {
            return Err(Error::Owner);
        }
        if surface_generation != self.surface_generation {
            return Err(Error::StaleSurface);
        }
        Ok(())
    }
    /// Server-issued configuration supersedes older pending requests. Invalid
    /// geometry/sequence exhaustion leaves the complete previous state intact.
    pub fn configure(&mut self, geometry: Geometry) -> Result<Configure, Error> {
        let geometry = geometry.validate()?;
        let next = self.next_configure.checked_add(1).ok_or(Error::Exhausted)?;
        let configuration = Configure {
            generation: self.next_configure,
            geometry,
        };
        self.next_configure = next;
        self.pending = Some(configuration);
        Ok(configuration)
    }
    pub fn acknowledge(
        &mut self,
        authenticated_owner: u64,
        surface_generation: u64,
        configure_generation: u64,
    ) -> Result<(), Error> {
        self.authority(authenticated_owner, surface_generation)?;
        let pending = self.pending.ok_or(Error::StaleConfigure)?;
        if configure_generation != pending.generation {
            return Err(Error::StaleConfigure);
        }
        self.active = Some(pending);
        self.pending = None;
        Ok(())
    }
    pub fn active(&self) -> Option<Configure> {
        self.active
    }
    pub fn pending(&self) -> Option<Configure> {
        self.pending
    }
    /// Called only after structural frame decode, capability/mapped-range checks
    /// and actual buffer-broker owner validation. authenticated_buffer_owner is
    /// broker metadata, never a client-controlled field or success boolean.
    pub fn present(
        &mut self,
        authenticated_owner: u64,
        authenticated_buffer_owner: u64,
        surface_generation: u64,
        configure_generation: u64,
        frame_generation: u64,
        geometry: Geometry,
    ) -> Result<(), Error> {
        self.authority(authenticated_owner, surface_generation)?;
        if authenticated_buffer_owner != self.owner {
            return Err(Error::Owner);
        }
        if self.pending.is_some() {
            return Err(Error::Unacknowledged);
        }
        let active = self.active.ok_or(Error::Unacknowledged)?;
        if configure_generation != active.generation {
            return Err(Error::StaleConfigure);
        }
        if frame_generation == 0 || frame_generation <= self.last_frame {
            return Err(Error::StaleFrame);
        }
        if geometry != active.geometry {
            return Err(Error::Geometry);
        }
        self.last_frame = frame_generation;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn geometry() -> Geometry {
        Geometry {
            logical_width: 640,
            logical_height: 400,
            pixel_width: 800,
            pixel_height: 500,
            scale_16_16: 81920,
        }
    }
    #[test]
    fn out_of_order_acks_cannot_activate_superseded_configure_or_stale_surface() {
        let mut state = SurfaceState::new(7, 3).unwrap();
        let first = state.configure(geometry()).unwrap();
        let latest = state.configure(geometry()).unwrap();
        let before = state;
        for (owner, surface, generation, error) in [
            (8, 3, latest.generation, Error::Owner),
            (7, 2, latest.generation, Error::StaleSurface),
            (7, 3, first.generation, Error::StaleConfigure),
        ] {
            assert_eq!(state.acknowledge(owner, surface, generation), Err(error));
            assert_eq!(state, before);
        }
        state.acknowledge(7, 3, latest.generation).unwrap();
        assert_eq!(state.active(), Some(latest));
        assert_eq!(state.pending(), None);
        assert_eq!(
            state.acknowledge(7, 3, latest.generation),
            Err(Error::StaleConfigure)
        );
    }
    #[test]
    fn unauthorized_buffers_unacked_geometry_and_replayed_frames_preserve_state() {
        let mut state = SurfaceState::new(7, 3).unwrap();
        let config = state.configure(geometry()).unwrap();
        assert_eq!(
            state.present(7, 7, 3, config.generation, 1, geometry()),
            Err(Error::Unacknowledged)
        );
        state.acknowledge(7, 3, config.generation).unwrap();
        let before = state;
        assert_eq!(
            state.present(7, 8, 3, config.generation, 1, geometry()),
            Err(Error::Owner)
        );
        assert_eq!(state, before);
        let mut changed = geometry();
        changed.pixel_width += 1;
        assert_eq!(
            state.present(7, 7, 3, config.generation, 1, changed),
            Err(Error::Geometry)
        );
        assert_eq!(state, before);
        state
            .present(7, 7, 3, config.generation, 1, geometry())
            .unwrap();
        assert_eq!(
            state.present(7, 7, 3, config.generation, 1, geometry()),
            Err(Error::StaleFrame)
        );
        let next = state.configure(geometry()).unwrap();
        assert_eq!(
            state.present(7, 7, 3, config.generation, 2, geometry()),
            Err(Error::Unacknowledged)
        );
        state.acknowledge(7, 3, next.generation).unwrap();
        assert_eq!(
            state.present(7, 7, 3, config.generation, 2, geometry()),
            Err(Error::StaleConfigure)
        );
        state
            .present(7, 7, 3, next.generation, 2, geometry())
            .unwrap();
    }
    #[test]
    fn invalid_geometry_and_generation_wrap_never_mutate_or_reuse_identity() {
        let mut state = SurfaceState::new(7, 3).unwrap();
        let mut invalid = geometry();
        invalid.logical_width = u32::MAX;
        let before = state;
        assert_eq!(state.configure(invalid), Err(Error::Geometry));
        assert_eq!(state, before);
        state.next_configure = u64::MAX;
        let before = state;
        assert_eq!(state.configure(geometry()), Err(Error::Exhausted));
        assert_eq!(state, before);
        assert_eq!(SurfaceState::new(0, 1), Err(Error::InvalidIdentity));
        assert_eq!(SurfaceState::new(1, 0), Err(Error::InvalidIdentity));
    }
}
