//! Capability-checked CPU-copy surface foundation, currently unexported.
//! Callers synchronize this broker and authenticate owners outside the wire.
//! User pointers never enter this model: import receives a fully validated
//! immutable slice; publication contains owned pixels, not process mappings.
//! No GPU fence, scanout, compositor process or capture policy is implemented.
#[cfg(test)]
use helix_core::handle::{Handle, HandleError, HandleTable, Rights};
use next_abi::surface::{self, Frame, Rect};
use prism::surface_state::{Configure, Geometry, SurfaceState};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidHandle,
    #[cfg(test)]
    Denied,
    Full,
    Bytes,
    References,
    WouldBlock,
    Damage,
    Protocol(surface::Error),
    State(prism::surface_state::Error),
}
#[cfg(test)]
impl From<HandleError> for Error {
    fn from(error: HandleError) -> Self {
        match error {
            HandleError::Full => Self::Full,
            HandleError::Denied => Self::Denied,
            HandleError::Invalid => Self::InvalidHandle,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceId {
    slot: usize,
    generation: u64,
}
impl SurfaceId {
    pub fn generation(self) -> u64 {
        self.generation
    }
    pub fn slot(self) -> usize {
        self.slot
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferId {
    slot: usize,
    generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    Surface(SurfaceId),
    Buffer(BufferId),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    surface: SurfaceId,
    frame: u64,
    configure: u64,
}
struct Buffer<const B: usize> {
    generation: u64,
    refs: u32,
    retired: bool,
    owner: u64,
    len: usize,
    pixels: [u8; B],
}
impl<const B: usize> Buffer<B> {
    const EMPTY: Self = Self {
        generation: 1,
        refs: 0,
        retired: false,
        owner: 0,
        len: 0,
        pixels: [0; B],
    };
}
struct Surface<const B: usize> {
    generation: u64,
    refs: u32,
    retired: bool,
    state: Option<SurfaceState>,
    frame: Option<Frame>,
    pending: bool,
    pixels: [u8; B],
}
impl<const B: usize> Surface<B> {
    const EMPTY: Self = Self {
        generation: 1,
        refs: 0,
        retired: false,
        state: None,
        frame: None,
        pending: false,
        pixels: [0; B],
    };
}
/// The deployment chooses small fixed quotas independent of structural wire
/// maxima. Large instances must be initialized in static storage, never on a
/// kernel/process stack. No allocation or borrowed process pointer is retained.
pub struct Broker<const S: usize, const N: usize, const B: usize> {
    surfaces: [Surface<B>; S],
    buffers: [Buffer<B>; N],
}
impl<const S: usize, const N: usize, const B: usize> Default for Broker<S, N, B> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const S: usize, const N: usize, const B: usize> Broker<S, N, B> {
    pub const fn new() -> Self {
        Self {
            surfaces: [const { Surface::EMPTY }; S],
            buffers: [const { Buffer::EMPTY }; N],
        }
    }
    fn surface(&self, id: SurfaceId) -> Result<&Surface<B>, Error> {
        self.surfaces
            .get(id.slot)
            .filter(|entry| entry.refs != 0 && entry.generation == id.generation)
            .ok_or(Error::InvalidHandle)
    }
    fn surface_mut(&mut self, id: SurfaceId) -> Result<&mut Surface<B>, Error> {
        self.surfaces
            .get_mut(id.slot)
            .filter(|entry| entry.refs != 0 && entry.generation == id.generation)
            .ok_or(Error::InvalidHandle)
    }
    fn buffer(&self, id: BufferId) -> Result<&Buffer<B>, Error> {
        self.buffers
            .get(id.slot)
            .filter(|entry| entry.refs != 0 && entry.generation == id.generation)
            .ok_or(Error::InvalidHandle)
    }
    #[cfg(test)]
    fn surface_cap<const C: usize>(
        table: &HandleTable<Object, C>,
        handle: Handle,
        rights: Rights,
    ) -> Result<SurfaceId, Error> {
        match *table.get(handle, rights)? {
            Object::Surface(id) => Ok(id),
            _ => Err(Error::InvalidHandle),
        }
    }
    #[cfg(test)]
    fn rights(rights: Rights, buffer: bool) -> Result<(), Error> {
        let allowed = Rights::READ
            .union(Rights::DUPLICATE)
            .union(Rights::TRANSFER)
            .union(if buffer { Rights(0) } else { Rights::WRITE });
        if rights.0 & !allowed.0 != 0 {
            return Err(Error::Denied);
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn create<const C: usize>(
        &mut self,
        table: &mut HandleTable<Object, C>,
        owner: u64,
        rights: Rights,
    ) -> Result<Handle, Error> {
        Self::rights(rights, false)?;
        let id = self.create_object(owner)?;
        match table.insert(Object::Surface(id), rights) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.release(Object::Surface(id))?;
                Err(error.into())
            }
        }
    }
    /// Runtime reserves its owner capability slot before allocating. Owner is
    /// immutable authenticated process identity, never a wire field.
    pub fn create_object(&mut self, owner: u64) -> Result<SurfaceId, Error> {
        let slot = self
            .surfaces
            .iter()
            .position(|entry| entry.refs == 0 && !entry.retired)
            .ok_or(Error::Full)?;
        let id = SurfaceId {
            slot,
            generation: self.surfaces[slot].generation,
        };
        let state = SurfaceState::new(owner, id.generation).map_err(Error::State)?;
        let entry = &mut self.surfaces[slot];
        entry.refs = 1;
        entry.state = Some(state);
        entry.frame = None;
        entry.pending = false;
        entry.pixels.fill(0);
        Ok(id)
    }
    #[cfg(test)]
    pub fn import<const C: usize>(
        &mut self,
        table: &mut HandleTable<Object, C>,
        owner: u64,
        rights: Rights,
        bytes: &[u8],
    ) -> Result<Handle, Error> {
        Self::rights(rights, true)?;
        let id = self.import_object(owner, bytes)?;
        match table.insert(Object::Buffer(id), rights) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.release(Object::Buffer(id))?;
                Err(error.into())
            }
        }
    }
    /// Copy a fully range-validated source into immutable broker storage.
    pub fn import_object(&mut self, owner: u64, bytes: &[u8]) -> Result<BufferId, Error> {
        if owner == 0
            || bytes.is_empty()
            || bytes.len() > B
            || bytes.len() as u64 > surface::MAX_BUFFER_BYTES
        {
            return Err(Error::Bytes);
        }
        let slot = self
            .buffers
            .iter()
            .position(|entry| entry.refs == 0 && !entry.retired)
            .ok_or(Error::Full)?;
        let id = BufferId {
            slot,
            generation: self.buffers[slot].generation,
        };
        let entry = &mut self.buffers[slot];
        entry.refs = 1;
        entry.owner = owner;
        entry.len = bytes.len();
        entry.pixels.fill(0);
        entry.pixels[..bytes.len()].copy_from_slice(bytes);
        Ok(id)
    }
    pub fn retain(&mut self, object: Object) -> Result<(), Error> {
        let refs = match object {
            Object::Surface(id) => &mut self.surface_mut(id)?.refs,
            Object::Buffer(id) => {
                self.buffer(id)?;
                &mut self.buffers[id.slot].refs
            }
        };
        *refs = refs.checked_add(1).ok_or(Error::References)?;
        Ok(())
    }
    #[cfg(test)]
    pub fn duplicate<const C: usize>(
        &mut self,
        table: &mut HandleTable<Object, C>,
        handle: Handle,
        rights: Rights,
    ) -> Result<Handle, Error> {
        let object = *table.get(handle, Rights::DUPLICATE)?;
        if !table.rights(handle)?.contains(rights) {
            return Err(Error::Denied);
        }
        self.retain(object)?;
        match table.insert(object, rights) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.release(object)?;
                Err(error.into())
            }
        }
    }
    /// Destination principal/capture/role policy must be authorized before
    /// transfer. Moving the cap preserves exactly one broker reference.
    #[cfg(test)]
    pub fn transfer<const C: usize, const D: usize>(
        &mut self,
        source: &mut HandleTable<Object, C>,
        handle: Handle,
        destination: &mut HandleTable<Object, D>,
        rights: Rights,
    ) -> Result<Handle, Error> {
        let object = *source.get(handle, Rights::TRANSFER)?;
        match object {
            Object::Surface(id) => {
                self.surface(id)?;
            }
            Object::Buffer(id) => {
                self.buffer(id)?;
            }
        }
        source
            .transfer(handle, destination, rights)
            .map_err(Into::into)
    }
    pub fn release(&mut self, object: Object) -> Result<(), Error> {
        match object {
            Object::Surface(id) => {
                let entry = self.surface_mut(id)?;
                entry.refs -= 1;
                if entry.refs == 0 {
                    entry.state = None;
                    entry.frame = None;
                    entry.pending = false;
                    entry.pixels.fill(0);
                    match entry.generation.checked_add(1) {
                        Some(next) => entry.generation = next,
                        None => entry.retired = true,
                    }
                }
            }
            Object::Buffer(id) => {
                self.buffer(id)?;
                let entry = &mut self.buffers[id.slot];
                entry.refs -= 1;
                if entry.refs == 0 {
                    entry.owner = 0;
                    entry.len = 0;
                    entry.pixels.fill(0);
                    match entry.generation.checked_add(1) {
                        Some(next) => entry.generation = next,
                        None => entry.retired = true,
                    }
                }
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn close<const C: usize>(
        &mut self,
        table: &mut HandleTable<Object, C>,
        handle: Handle,
    ) -> Result<(), Error> {
        let object = *table.get(handle, Rights(0))?;
        match object {
            Object::Surface(id) => {
                self.surface(id)?;
            }
            Object::Buffer(id) => {
                self.buffer(id)?;
            }
        }
        table.close(handle)?;
        self.release(object)
    }
    pub fn references(&self, object: Object) -> Result<u32, Error> {
        match object {
            Object::Surface(id) => Ok(self.surface(id)?.refs),
            Object::Buffer(id) => Ok(self.buffer(id)?.refs),
        }
    }
    pub fn configuration(&self, id: SurfaceId) -> Result<Configure, Error> {
        let state = self.surface(id)?.state.ok_or(Error::InvalidHandle)?;
        state
            .pending()
            .or(state.active())
            .ok_or(Error::State(prism::surface_state::Error::Unacknowledged))
    }
    #[cfg(test)]
    pub fn configure<const C: usize>(
        &mut self,
        table: &HandleTable<Object, C>,
        handle: Handle,
        geometry: Geometry,
    ) -> Result<Configure, Error> {
        let id = Self::surface_cap(table, handle, Rights::WRITE)?;
        self.configure_object(id, geometry)
    }
    /// Preflight without changing publication lifetime or generation counters.
    pub fn next_configuration(
        &self,
        id: SurfaceId,
        geometry: Geometry,
    ) -> Result<Configure, Error> {
        let mut state = self.surface(id)?.state.ok_or(Error::InvalidHandle)?;
        let next = state.configure(geometry).map_err(Error::State)?;
        if u64::from(geometry.pixel_width) * 4 * u64::from(geometry.pixel_height) > B as u64 {
            return Err(Error::Bytes);
        }
        Ok(next)
    }
    pub fn configure_object(
        &mut self,
        id: SurfaceId,
        geometry: Geometry,
    ) -> Result<Configure, Error> {
        // Validate a copied state first: an invalid geometry or exhausted
        // generation must not retire a still-readable publication.
        let entry = self.surface_mut(id)?;
        let mut state = entry.state.ok_or(Error::InvalidHandle)?;
        let configuration = state.configure(geometry).map_err(Error::State)?;
        if u64::from(geometry.pixel_width) * 4 * u64::from(geometry.pixel_height) > B as u64 {
            return Err(Error::Bytes);
        }
        entry.state = Some(state);
        entry.pending = false;
        entry.frame = None; // First publication of the new configuration is full damage.
        Ok(configuration)
    }
    #[cfg(test)]
    pub fn acknowledge<const C: usize>(
        &mut self,
        table: &HandleTable<Object, C>,
        handle: Handle,
        owner: u64,
        generation: u64,
    ) -> Result<(), Error> {
        let id = Self::surface_cap(table, handle, Rights::WRITE)?;
        self.acknowledge_object(id, owner, generation)
    }
    pub fn acknowledge_object(
        &mut self,
        id: SurfaceId,
        owner: u64,
        generation: u64,
    ) -> Result<(), Error> {
        self.surface_mut(id)?
            .state
            .as_mut()
            .ok_or(Error::InvalidHandle)?
            .acknowledge(owner, id.generation, generation)
            .map_err(Error::State)
    }
    #[cfg(test)]
    pub fn publish<const C: usize>(
        &mut self,
        table: &HandleTable<Object, C>,
        handle: Handle,
        owner: u64,
        descriptor: &[u8],
    ) -> Result<Ticket, Error> {
        let frame = Frame::decode(descriptor).map_err(Error::Protocol)?;
        let id = Self::surface_cap(table, handle, Rights::WRITE)?;
        let buffer_id = match *table.get(Handle::from_raw(frame.buffer_handle), Rights::READ)? {
            Object::Buffer(id) => id,
            _ => return Err(Error::InvalidHandle),
        };
        self.publish_objects(id, owner, frame, buffer_id)
    }
    /// Surface WRITE and buffer READ must have been resolved independently by
    /// the runtime before this entry. Typed metadata is revalidated here, so
    /// internal callers cannot bypass wire arithmetic checks.
    pub fn publish_objects(
        &mut self,
        id: SurfaceId,
        owner: u64,
        frame: Frame,
        buffer_id: BufferId,
    ) -> Result<Ticket, Error> {
        let mut wire = [0; surface::MAX_BYTES];
        frame.encode(&mut wire).map_err(Error::Protocol)?;
        let buffer = self.buffer(buffer_id)?;
        if frame.buffer_bytes != buffer.len as u64 || buffer.len > B {
            return Err(Error::Bytes);
        }
        let surface = self.surface(id)?;
        if surface.pending {
            return Err(Error::WouldBlock);
        }
        let geometry = Geometry {
            logical_width: frame.logical_width,
            logical_height: frame.logical_height,
            pixel_width: frame.pixel_width,
            pixel_height: frame.pixel_height,
            scale_16_16: frame.scale_16_16,
        };
        let mut state = surface.state.ok_or(Error::InvalidHandle)?;
        state
            .present(
                owner,
                buffer.owner,
                frame.surface_generation,
                frame.configure_generation,
                frame.frame_generation,
                geometry,
            )
            .map_err(Error::State)?;
        let changed = surface.frame.is_none_or(|prior| {
            prior.pixel_width != frame.pixel_width
                || prior.pixel_height != frame.pixel_height
                || prior.stride_bytes != frame.stride_bytes
                || prior.format != frame.format
        });
        if changed
            && !frame.damage().iter().any(|rect| {
                *rect
                    == Rect {
                        x: 0,
                        y: 0,
                        width: frame.pixel_width,
                        height: frame.pixel_height,
                    }
            })
        {
            return Err(Error::Damage);
        }
        // Every error is resolved before either pixels or state are committed.
        // Owned, immutable source storage cannot race the copy. Row arithmetic
        // is bounded by the validated stride, rect and exact byte length.
        let buffer = &self.buffers[buffer_id.slot];
        let surface = &mut self.surfaces[id.slot];
        if changed {
            surface.pixels.fill(0);
        }
        for rect in frame.damage() {
            for y in rect.y..rect.y + rect.height {
                let start = y as usize * frame.stride_bytes as usize + rect.x as usize * 4;
                let end = start + rect.width as usize * 4;
                surface.pixels[start..end].copy_from_slice(&buffer.pixels[start..end]);
            }
        }
        surface.state = Some(state);
        surface.frame = Some(frame);
        surface.pending = true;
        Ok(Ticket {
            surface: id,
            frame: frame.frame_generation,
            configure: frame.configure_generation,
        })
    }
    /// Only a READ surface capability permits copying the publication. Wire
    /// buffer_handle is informational here and is never dereferenced again.
    #[cfg(test)]
    pub fn peek<const C: usize>(
        &self,
        table: &HandleTable<Object, C>,
        handle: Handle,
        output: &mut [u8],
    ) -> Result<(Frame, Ticket), Error> {
        let id = Self::surface_cap(table, handle, Rights::READ)?;
        self.peek_object(id, output)
    }
    /// Trusted compositor-only consumer. Client reads require a surface READ
    /// capability before this entry; capture policy is not supplied here.
    pub fn peek_object(&self, id: SurfaceId, output: &mut [u8]) -> Result<(Frame, Ticket), Error> {
        let entry = self.surface(id)?;
        if !entry.pending {
            return Err(Error::WouldBlock);
        }
        let frame = entry.frame.ok_or(Error::WouldBlock)?;
        let length = frame.buffer_bytes as usize;
        if output.len() < length {
            return Err(Error::Bytes);
        }
        output[..length].copy_from_slice(&entry.pixels[..length]);
        Ok((
            frame,
            Ticket {
                surface: id,
                frame: frame.frame_generation,
                configure: frame.configure_generation,
            },
        ))
    }
    #[cfg(test)]
    pub fn consume<const C: usize>(
        &mut self,
        table: &HandleTable<Object, C>,
        handle: Handle,
        ticket: Ticket,
    ) -> Result<(), Error> {
        let id = Self::surface_cap(table, handle, Rights::READ)?;
        self.consume_object(id, ticket)
    }
    pub fn pending_at(&self, slot: usize) -> Option<SurfaceId> {
        let entry = self.surfaces.get(slot)?;
        (entry.refs != 0 && entry.pending).then_some(SurfaceId {
            slot,
            generation: entry.generation,
        })
    }
    /// Trusted consumer fence, valid after consumption until the next frame or
    /// configuration. Lock failure is handled by the synchronized adapter.
    pub fn publication_is_current(&self, id: SurfaceId, ticket: Ticket) -> Result<bool, Error> {
        let entry = self.surface(id)?;
        let configuration = self.configuration(id)?;
        Ok(ticket.surface == id
            && ticket.configure == configuration.generation
            && entry.frame.is_some_and(|frame| {
                frame.frame_generation == ticket.frame
                    && frame.configure_generation == ticket.configure
            }))
    }
    pub fn consume_object(&mut self, id: SurfaceId, ticket: Ticket) -> Result<(), Error> {
        if !self.publication_is_current(id, ticket)? {
            return Err(Error::InvalidHandle);
        }
        let entry = self.surface_mut(id)?;
        if ticket.surface != id
            || !entry.pending
            || entry.frame.is_none_or(|frame| {
                frame.frame_generation != ticket.frame
                    || frame.configure_generation != ticket.configure
            })
        {
            return Err(Error::InvalidHandle);
        }
        entry.pending = false;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn geometry() -> Geometry {
        Geometry {
            logical_width: 2,
            logical_height: 2,
            pixel_width: 2,
            pixel_height: 2,
            scale_16_16: 65536,
        }
    }
    fn frame(surface: u64, config: u64, sequence: u64, buffer: Handle) -> Frame {
        Frame {
            surface_generation: surface,
            configure_generation: config,
            frame_generation: sequence,
            buffer_handle: buffer.raw(),
            logical_width: 2,
            logical_height: 2,
            pixel_width: 2,
            pixel_height: 2,
            scale_16_16: 65536,
            stride_bytes: 12,
            buffer_bytes: 24,
            private_content: true,
            format: surface::PixelFormat::Rgbx,
            damage_rectangles: [Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }; surface::MAX_DAMAGE],
            damage_count: 1,
        }
    }
    fn wire(frame: Frame) -> ([u8; surface::MAX_BYTES], usize) {
        let mut bytes = [0; surface::MAX_BYTES];
        let length = frame.encode(&mut bytes).unwrap();
        (bytes, length)
    }
    fn generation(table: &HandleTable<Object, 8>, surface: Handle) -> u64 {
        let Object::Surface(id) = *table.get(surface, Rights(0)).unwrap() else {
            panic!()
        };
        id.generation()
    }
    #[test]
    fn owned_copy_padding_and_pending_publication_survive_source_close() {
        let mut broker = Broker::<1, 2, 64>::new();
        let mut caps = HandleTable::<Object, 8>::new();
        let surface = broker
            .create(&mut caps, 7, Rights::READ.union(Rights::WRITE))
            .unwrap();
        let mut pixels = [3; 24];
        let buffer = broker.import(&mut caps, 7, Rights::READ, &pixels).unwrap();
        pixels.fill(9);
        let config = broker.configure(&caps, surface, geometry()).unwrap();
        broker
            .acknowledge(&caps, surface, 7, config.generation)
            .unwrap();
        let (bytes, length) = wire(frame(
            generation(&caps, surface),
            config.generation,
            1,
            buffer,
        ));
        let ticket = broker.publish(&caps, surface, 7, &bytes[..length]).unwrap();
        broker.close(&mut caps, buffer).unwrap();
        let mut output = [0xa5; 64];
        let (metadata, _) = broker.peek(&caps, surface, &mut output).unwrap();
        assert!(metadata.private_content);
        assert_eq!(&output[..8], &[3; 8]);
        assert_eq!(&output[8..12], &[0; 4]);
        assert_eq!(&output[12..20], &[3; 8]);
        assert_eq!(&output[20..24], &[0; 4]);
        assert!(output[24..].iter().all(|b| *b == 0xa5));
        assert_eq!(broker.peek(&caps, surface, &mut [0; 23]), Err(Error::Bytes));
        broker.consume(&caps, surface, ticket).unwrap();
        assert_eq!(
            broker.consume(&caps, surface, ticket),
            Err(Error::InvalidHandle)
        );
        broker.close(&mut caps, surface).unwrap();
        assert!(broker.surfaces[0].pixels.iter().all(|b| *b == 0));
        assert!(broker.buffers[0].pixels.iter().all(|b| *b == 0));
    }
    #[test]
    fn attenuated_caps_wrong_owner_and_bad_wire_never_change_publication() {
        let mut broker = Broker::<1, 2, 64>::new();
        let mut caps = HandleTable::<Object, 8>::new();
        let surface = broker
            .create(
                &mut caps,
                7,
                Rights::READ.union(Rights::WRITE).union(Rights::DUPLICATE),
            )
            .unwrap();
        let reader = broker.duplicate(&mut caps, surface, Rights::READ).unwrap();
        let buffer = broker.import(&mut caps, 8, Rights::READ, &[1; 24]).unwrap();
        let config = broker.configure(&caps, surface, geometry()).unwrap();
        broker
            .acknowledge(&caps, surface, 7, config.generation)
            .unwrap();
        let (bytes, length) = wire(frame(
            generation(&caps, surface),
            config.generation,
            1,
            buffer,
        ));
        assert_eq!(
            broker.publish(&caps, reader, 7, &bytes[..length]),
            Err(Error::Denied)
        );
        assert_eq!(
            broker.publish(&caps, surface, 7, &bytes[..length]),
            Err(Error::State(prism::surface_state::Error::Owner))
        );
        for cut in 0..length {
            assert!(broker.publish(&caps, surface, 7, &bytes[..cut]).is_err());
        }
        assert_eq!(
            broker.peek(&caps, reader, &mut [0; 64]),
            Err(Error::WouldBlock)
        );
        assert_eq!(broker.surfaces[0].frame, None);
        broker.close(&mut caps, buffer).unwrap();
        let buffer = broker.import(&mut caps, 7, Rights::READ, &[2; 24]).unwrap();
        let (bytes, length) = wire(frame(
            generation(&caps, surface),
            config.generation,
            1,
            buffer,
        ));
        broker.publish(&caps, surface, 7, &bytes[..length]).unwrap();
        assert_eq!(
            broker.publish(&caps, surface, 7, &bytes[..length]),
            Err(Error::WouldBlock)
        );
        let (_, ticket) = broker.peek(&caps, reader, &mut [0; 64]).unwrap();
        broker.consume(&caps, reader, ticket).unwrap();
        assert_eq!(
            broker.publish(&caps, surface, 7, &bytes[..length]),
            Err(Error::State(prism::surface_state::Error::StaleFrame))
        );
    }
    #[test]
    fn partial_damage_preserves_other_pixels_and_stale_configuration_rejects() {
        let mut broker = Broker::<1, 2, 64>::new();
        let mut caps = HandleTable::<Object, 8>::new();
        let surface = broker
            .create(&mut caps, 7, Rights::READ.union(Rights::WRITE))
            .unwrap();
        let buffer = broker.import(&mut caps, 7, Rights::READ, &[1; 24]).unwrap();
        let config = broker.configure(&caps, surface, geometry()).unwrap();
        broker
            .acknowledge(&caps, surface, 7, config.generation)
            .unwrap();
        let mut f = frame(generation(&caps, surface), config.generation, 1, buffer);
        f.damage_count = 0;
        let (bytes, length) = wire(f);
        assert_eq!(
            broker.publish(&caps, surface, 7, &bytes[..length]),
            Err(Error::Damage)
        );
        f.damage_count = 1;
        let (bytes, length) = wire(f);
        let ticket = broker.publish(&caps, surface, 7, &bytes[..length]).unwrap();
        broker.consume(&caps, surface, ticket).unwrap();
        broker.close(&mut caps, buffer).unwrap();
        let buffer = broker.import(&mut caps, 7, Rights::READ, &[9; 24]).unwrap();
        f.buffer_handle = buffer.raw();
        f.frame_generation = 2;
        f.damage_rectangles[0] = Rect {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        };
        let (bytes, length) = wire(f);
        let ticket = broker.publish(&caps, surface, 7, &bytes[..length]).unwrap();
        let mut out = [0; 64];
        broker.peek(&caps, surface, &mut out).unwrap();
        assert_eq!(&out[..8], &[1; 8]);
        assert_eq!(&out[12..16], &[1; 4]);
        assert_eq!(&out[16..20], &[9; 4]);
        broker.consume(&caps, surface, ticket).unwrap();
        let next = broker.configure(&caps, surface, geometry()).unwrap();
        f.frame_generation = 3;
        let (bytes, length) = wire(f);
        assert_eq!(
            broker.publish(&caps, surface, 7, &bytes[..length]),
            Err(Error::State(prism::surface_state::Error::Unacknowledged))
        );
        broker
            .acknowledge(&caps, surface, 7, next.generation)
            .unwrap();
        assert_eq!(
            broker.publish(&caps, surface, 7, &bytes[..length]),
            Err(Error::State(prism::surface_state::Error::StaleConfigure))
        );
    }
    #[test]
    fn reconfigure_retires_copied_ticket_without_consuming_new_frame() {
        let mut broker = Broker::<1, 1, 64>::new();
        let mut caps = HandleTable::<Object, 8>::new();
        let cap = broker
            .create(&mut caps, 7, Rights::READ.union(Rights::WRITE))
            .unwrap();
        let Object::Surface(id) = *caps.get(cap, Rights(0)).unwrap() else {
            panic!()
        };
        let buffer = broker.import(&mut caps, 7, Rights::READ, &[9; 24]).unwrap();
        let config = broker.configure(&caps, cap, geometry()).unwrap();
        broker
            .acknowledge(&caps, cap, 7, config.generation)
            .unwrap();
        let mut f = frame(id.generation(), config.generation, 1, buffer);
        let (bytes, len) = wire(f);
        let old = broker.publish(&caps, cap, 7, &bytes[..len]).unwrap();
        let mut staging = [0; 64];
        assert_eq!(broker.peek(&caps, cap, &mut staging).unwrap().1, old);
        assert_eq!(broker.publication_is_current(id, old), Ok(true));
        assert_eq!(
            broker.configure_object(
                id,
                Geometry {
                    pixel_width: 0,
                    ..geometry()
                }
            ),
            Err(Error::State(prism::surface_state::Error::Geometry))
        );
        assert_eq!(broker.peek(&caps, cap, &mut staging).unwrap().1, old);
        let new_config = broker.configure(&caps, cap, geometry()).unwrap();
        assert_eq!(broker.publication_is_current(id, old), Ok(false));
        assert_eq!(
            broker.peek(&caps, cap, &mut staging),
            Err(Error::WouldBlock)
        );
        assert_eq!(broker.consume(&caps, cap, old), Err(Error::InvalidHandle));
        broker
            .acknowledge(&caps, cap, 7, new_config.generation)
            .unwrap();
        f.configure_generation = new_config.generation;
        f.frame_generation = 2;
        f.damage_rectangles[0] = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let (bytes, len) = wire(f);
        assert_eq!(
            broker.publish(&caps, cap, 7, &bytes[..len]),
            Err(Error::Damage)
        );
        f.damage_rectangles[0] = Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        let (bytes, len) = wire(f);
        let new = broker.publish(&caps, cap, 7, &bytes[..len]).unwrap();
        assert_eq!(broker.consume(&caps, cap, old), Err(Error::InvalidHandle));
        assert_eq!(broker.peek(&caps, cap, &mut staging).unwrap().1, new);
        assert_eq!(broker.publication_is_current(id, new), Ok(true));
        broker.consume(&caps, cap, new).unwrap();
        assert_eq!(broker.publication_is_current(id, new), Ok(true));
        assert_eq!(broker.publication_is_current(id, old), Ok(false));
    }
    #[test]
    fn failed_import_and_duplicate_leave_quota_and_refs_unchanged_and_move_keeps_owner() {
        let mut broker = Broker::<1, 1, 32>::new();
        let mut caps = HandleTable::<Object, 1>::new();
        let surface = broker
            .create(&mut caps, 7, Rights::READ.union(Rights::DUPLICATE))
            .unwrap();
        assert_eq!(
            broker.import(&mut caps, 7, Rights::READ, &[1; 24]),
            Err(Error::Full)
        );
        assert_eq!(broker.buffers[0].refs, 0);
        assert_eq!(
            broker.duplicate(&mut caps, surface, Rights::READ),
            Err(Error::Full)
        );
        assert_eq!(broker.surfaces[0].refs, 1);
        broker.close(&mut caps, surface).unwrap();
        assert_eq!(
            broker.import(&mut caps, 7, Rights::WRITE, &[1; 24]),
            Err(Error::Denied)
        );
        assert_eq!(
            broker.import(&mut caps, 7, Rights::READ, &[1; 33]),
            Err(Error::Bytes)
        );
        let buffer = broker
            .import(&mut caps, 7, Rights::READ.union(Rights::TRANSFER), &[1; 24])
            .unwrap();
        let mut target = HandleTable::<Object, 1>::new();
        let moved = broker
            .transfer(&mut caps, buffer, &mut target, Rights::READ)
            .unwrap();
        assert_eq!(caps.get(buffer, Rights::READ), Err(HandleError::Invalid));
        assert_eq!(broker.buffers[0].owner, 7);
        assert_eq!(broker.buffers[0].refs, 1);
        broker.close(&mut target, moved).unwrap();
        assert_eq!(broker.buffers[0].refs, 0);
    }
}
