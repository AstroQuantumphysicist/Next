//! Bounded shell semantics for trusted observation, not a screen-reader transport.
use crate::Bounds;
pub const CAPACITY: usize = 48;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Window,
    Dialog,
    Button,
    TextBox,
    List,
    ListItem,
    CheckBox,
    Status,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Id {
    pub context: u32,
    pub local: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    pub enabled: bool,
    pub visible: bool,
    pub focused: bool,
    pub selected: bool,
    pub checked: Option<bool>,
    pub busy: bool,
}
impl State {
    pub const ENABLED: Self = Self {
        enabled: true,
        visible: true,
        focused: false,
        selected: false,
        checked: None,
        busy: false,
    };
}
#[derive(Clone, Copy)]
/// Bounds describe available render geometry; zero extents do not grant a hit/action.
pub struct Node {
    pub id: Id,
    pub parent: Option<Id>,
    pub role: Role,
    pub bounds: Option<Bounds>,
    pub state: State,
    name: [u8; 128],
    name_len: usize,
    value: [u8; 192],
    value_len: usize,
}
impl Node {
    const EMPTY: Self = Self {
        id: Id {
            context: 0,
            local: 0,
        },
        parent: None,
        role: Role::Status,
        bounds: None,
        state: State::ENABLED,
        name: [0; 128],
        name_len: 0,
        value: [0; 192],
        value_len: 0,
    };
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
    pub fn value(&self) -> &str {
        core::str::from_utf8(&self.value[..self.value_len]).unwrap_or("")
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Capacity,
    InvalidNode,
    Duplicate,
    GenerationExhausted,
}
pub struct Snapshot {
    nodes: [Node; CAPACITY],
    len: usize,
    pub generation: u64,
    pub context: u32,
    pub token: u64,
    pub complete: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self::empty()
    }
}
impl Snapshot {
    pub const fn empty() -> Self {
        Self {
            nodes: [Node::EMPTY; CAPACITY],
            len: 0,
            generation: 0,
            context: 0,
            token: 0,
            complete: false,
        }
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes[..self.len]
    }
    pub fn focused(&self) -> Option<Id> {
        self.nodes().iter().find(|n| n.state.focused).map(|n| n.id)
    }
    pub fn begin(&mut self, context: u32, token: u64) {
        self.len = 0;
        self.context = context;
        self.token = token;
        self.complete = false;
    }
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        local: u32,
        parent: Option<Id>,
        role: Role,
        name: &str,
        value: &str,
        bounds: Option<Bounds>,
        state: State,
    ) -> Result<(), Error> {
        let id = Id {
            context: self.context,
            local,
        };
        let parent = parent.or_else(|| {
            (local != 1).then_some(Id {
                context: id.context,
                local: 1,
            })
        });
        if id.context == 0
            || local == 0
            || name.is_empty()
            || name.len() > 128
            || value.len() > 192
            || name.chars().any(char::is_control)
            || parent.is_some_and(|p| !self.nodes().iter().any(|n| n.id == p))
            || (state.focused && (!state.enabled || !state.visible || self.focused().is_some()))
        {
            return Err(Error::InvalidNode);
        }
        if self.nodes().iter().any(|n| n.id == id) {
            return Err(Error::Duplicate);
        }
        let node = self.nodes.get_mut(self.len).ok_or(Error::Capacity)?;
        node.id = id;
        node.parent = parent;
        node.role = role;
        node.bounds = bounds;
        node.state = state;
        node.name[..name.len()].copy_from_slice(name.as_bytes());
        node.name_len = name.len();
        node.value[..value.len()].copy_from_slice(value.as_bytes());
        node.value_len = value.len();
        self.len += 1;
        Ok(())
    }
    fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        let mut byte = |b: u8| {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
        };
        for b in self
            .context
            .to_le_bytes()
            .into_iter()
            .chain(self.token.to_le_bytes())
        {
            byte(b);
        }
        for n in self.nodes() {
            for b in n.id.local.to_le_bytes() {
                byte(b);
            }
            byte(n.role as u8);
            match n.parent {
                None => byte(0),
                Some(parent) => {
                    byte(1);
                    for b in parent
                        .context
                        .to_le_bytes()
                        .into_iter()
                        .chain(parent.local.to_le_bytes())
                    {
                        byte(b);
                    }
                }
            }
            for b in n.name().bytes() {
                byte(b);
            }
            byte(0);
            for b in n.value().bytes() {
                byte(b);
            }
            byte(0);
            for b in [
                n.state.enabled,
                n.state.visible,
                n.state.focused,
                n.state.selected,
                n.state.busy,
            ] {
                byte(u8::from(b));
            }
            byte(n.state.checked.map_or(0, |v| if v { 2 } else { 1 }));
            if let Some(b) = n.bounds {
                for v in [b.x, b.y, b.width, b.height] {
                    for b in v.to_le_bytes() {
                        byte(b);
                    }
                }
            }
        }
        hash
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    SnapshotChanged,
    ContextChanged,
    FocusChanged,
    Reset,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    pub generation: u64,
    pub kind: EventKind,
    pub focused: Option<Id>,
}
pub struct Observer {
    fingerprint: Option<u64>,
    context: (u32, u64),
    focused: Option<Id>,
    generation: u64,
    events: [Option<Event>; 8],
    len: usize,
}
impl Observer {
    pub const fn new() -> Self {
        Self {
            fingerprint: None,
            context: (0, 0),
            focused: None,
            generation: 0,
            events: [None; 8],
            len: 0,
        }
    }
    fn push(&mut self, kind: EventKind) {
        let event = Event {
            generation: self.generation,
            kind,
            focused: self.focused,
        };
        if self.len == 8 {
            self.events = [None; 8];
            self.events[0] = Some(Event {
                kind: EventKind::Reset,
                ..event
            });
            self.len = 1;
        } else {
            self.events[self.len] = Some(event);
            self.len += 1;
        }
    }
    pub fn observe(&mut self, snapshot: &mut Snapshot) -> Result<(), Error> {
        snapshot.complete = false;
        if snapshot.context == 0
            || snapshot.nodes().first().is_none_or(|root| {
                root.id.local != 1 || root.id.context != snapshot.context || root.parent.is_some()
            })
            || snapshot
                .nodes()
                .iter()
                .any(|node| node.id.context != snapshot.context)
        {
            return Err(Error::InvalidNode);
        }
        let hash = snapshot.fingerprint();
        if self.fingerprint != Some(hash) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or(Error::GenerationExhausted)?;
            let context_changed = self.context != (snapshot.context, snapshot.token);
            let focus_changed = self.focused != snapshot.focused() || context_changed;
            self.fingerprint = Some(hash);
            self.context = (snapshot.context, snapshot.token);
            self.focused = snapshot.focused();
            if context_changed {
                self.push(EventKind::ContextChanged);
            }
            if focus_changed {
                self.push(EventKind::FocusChanged);
            }
            self.push(EventKind::SnapshotChanged);
        }
        snapshot.generation = self.generation;
        snapshot.complete = true;
        Ok(())
    }
    pub fn take(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let result = self.events[0];
        self.events.copy_within(1..self.len, 0);
        self.len -= 1;
        self.events[self.len] = None;
        result
    }
}
impl Default for Observer {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_nodes_reject_invalid_focus_duplicates_and_partial_state_is_explicit() {
        let mut s = Snapshot::empty();
        s.begin(1, 0);
        assert_eq!(
            s.add(1, None, Role::Button, "", "", None, State::ENABLED),
            Err(Error::InvalidNode)
        );
        s.add(
            1,
            None,
            Role::Button,
            "Cancel",
            "",
            None,
            State {
                focused: true,
                ..State::ENABLED
            },
        )
        .unwrap();
        assert_eq!(
            s.add(1, None, Role::Button, "Cancel", "", None, State::ENABLED),
            Err(Error::Duplicate)
        );
        assert_eq!(
            s.add(
                2,
                None,
                Role::Button,
                "Approve",
                "",
                None,
                State {
                    focused: true,
                    ..State::ENABLED
                }
            ),
            Err(Error::InvalidNode)
        );
        for id in 2..=CAPACITY as u32 {
            s.add(id, None, Role::Button, "Action", "", None, State::ENABLED)
                .unwrap();
        }
        assert_eq!(
            s.add(99, None, Role::Button, "Extra", "", None, State::ENABLED),
            Err(Error::Capacity)
        );
        assert!(!s.complete);
    }
    #[test]
    fn rootless_or_retagged_observation_cannot_publish_complete_or_advance_observer() {
        let mut snapshot = Snapshot::empty();
        let mut observer = Observer::new();
        assert_eq!(observer.observe(&mut snapshot), Err(Error::InvalidNode));
        assert!(!snapshot.complete);
        assert_eq!(observer.take(), None);
        snapshot.begin(1, 0);
        assert_eq!(observer.observe(&mut snapshot), Err(Error::InvalidNode));
        snapshot
            .add(1, None, Role::Window, "Window", "", None, State::ENABLED)
            .unwrap();
        observer.observe(&mut snapshot).unwrap();
        assert_eq!(snapshot.generation, 1);
        while observer.take().is_some() {}
        snapshot.context = 2;
        assert_eq!(observer.observe(&mut snapshot), Err(Error::InvalidNode));
        assert!(!snapshot.complete);
        assert_eq!(snapshot.generation, 1);
        assert_eq!(observer.take(), None);
        snapshot.context = 1;
        observer.observe(&mut snapshot).unwrap();
        assert!(snapshot.complete);
        assert_eq!(snapshot.generation, 1);
    }
    #[test]
    fn implicit_parent_requires_root_and_parent_only_changes_advance_generation() {
        let mut s = Snapshot::empty();
        s.begin(1, 0);
        assert_eq!(
            s.add(2, None, Role::Button, "Orphan", "", None, State::ENABLED),
            Err(Error::InvalidNode)
        );
        assert!(s.nodes().is_empty());
        s.add(1, None, Role::Window, "Window", "", None, State::ENABLED)
            .unwrap();
        s.add(2, None, Role::List, "Group", "", None, State::ENABLED)
            .unwrap();
        s.add(3, None, Role::Button, "Action", "", None, State::ENABLED)
            .unwrap();
        assert_eq!(
            s.nodes()[2].parent,
            Some(Id {
                context: 1,
                local: 1
            })
        );
        let mut o = Observer::new();
        o.observe(&mut s).unwrap();
        while o.take().is_some() {}
        let generation = s.generation;
        s.nodes[2].parent = Some(Id {
            context: 1,
            local: 2,
        });
        o.observe(&mut s).unwrap();
        assert_eq!(s.generation, generation + 1);
        assert_eq!(o.take().unwrap().kind, EventKind::SnapshotChanged);
        assert_eq!(o.take(), None);
    }
    #[test]
    fn redraws_do_not_announce_and_overflow_requires_snapshot_reset() {
        let mut s = Snapshot::empty();
        let mut o = Observer::new();
        s.begin(1, 0);
        s.add(
            1,
            None,
            Role::Button,
            "Cancel",
            "",
            None,
            State {
                focused: true,
                ..State::ENABLED
            },
        )
        .unwrap();
        o.observe(&mut s).unwrap();
        let generation = s.generation;
        while o.take().is_some() {}
        o.observe(&mut s).unwrap();
        assert_eq!(s.generation, generation);
        assert_eq!(o.take(), None);
        for token in 1..10 {
            s.token = token;
            o.observe(&mut s).unwrap();
        }
        let mut reset = false;
        while let Some(e) = o.take() {
            reset |= e.kind == EventKind::Reset;
        }
        assert!(reset);
    }
}
