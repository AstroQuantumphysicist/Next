//! Trusted gestures and semantic presses pending the kernel broker's validation.
use crate::Key;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceRef {
    pub slot: usize,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    FocusChrome {
        surface: SurfaceRef,
    },
    RequestClose {
        surface: SurfaceRef,
    },
    Focus {
        surface: SurfaceRef,
    },
    Pointer {
        surface: SurfaceRef,
        x: i32,
        y: i32,
        buttons: u32,
    },
    Key {
        surface: SurfaceRef,
        key: Key,
    },
    Suspend,
}
const CAPACITY: usize = 8;
#[derive(Clone, Copy)]
struct Packet {
    x: i32,
    y: i32,
    buttons: u32,
}
#[derive(Clone, Copy)]
struct Pending {
    surface: SurfaceRef,
    packets: [Option<Packet>; CAPACITY],
    len: usize,
}
pub struct Interaction {
    queue: [Option<Action>; CAPACITY],
    head: usize,
    len: usize,
    focused: Option<SurfaceRef>,
    context: Option<SurfaceRef>,
    pending: Option<Pending>,
    buttons: u32,
}
impl Default for Interaction {
    fn default() -> Self {
        Self::new()
    }
}
impl Interaction {
    pub const fn new() -> Self {
        Self {
            queue: [None; CAPACITY],
            head: 0,
            len: 0,
            focused: None,
            context: None,
            pending: None,
            buttons: 0,
        }
    }
    pub fn context(&self) -> Option<SurfaceRef> {
        self.context
    }
    pub fn context_active(&self) -> bool {
        self.context.is_some()
    }
    pub fn buttons(&self) -> u32 {
        self.buttons
    }
    pub fn focused(&self) -> Option<SurfaceRef> {
        self.focused
    }
    pub(crate) fn accepts_pointer(&self, surface: SurfaceRef) -> bool {
        self.focused == Some(surface)
            || self
                .pending
                .is_some_and(|pending| pending.surface == surface)
    }
    fn push(&mut self, action: Action) -> bool {
        if self.len == CAPACITY {
            self.revoke();
            return false;
        }
        self.queue[(self.head + self.len) % CAPACITY] = Some(action);
        self.len += 1;
        true
    }
    pub fn request_close(&mut self, surface: SurfaceRef) {
        if self.context != Some(surface) {
            self.revoke();
            self.context = Some(surface);
        }
        if self.queue.contains(&Some(Action::RequestClose { surface })) {
            return;
        }
        self.push(Action::RequestClose { surface });
    }
    /// Revoke delivery while keeping the user's native context selected.
    pub fn block_context(&mut self, surface: SurfaceRef) {
        self.context = Some(surface);
        self.revoke();
    }
    pub fn chrome_gesture(&mut self, surface: SurfaceRef) {
        self.suspend();
        self.context = Some(surface);
        self.pending = Some(Pending {
            surface,
            packets: [None; CAPACITY],
            len: 0,
        });
        self.buttons = 0;
        self.push(Action::FocusChrome { surface });
    }
    pub fn gesture(&mut self, surface: SurfaceRef, x: i32, y: i32) {
        self.gesture_packet(surface, x, y, 1);
    }
    /// Only a real left-button press acquires client focus. Initial and later
    /// discrete masks remain ordered across the asynchronous focus response.
    pub fn gesture_packet(&mut self, surface: SurfaceRef, x: i32, y: i32, buttons: u32) {
        if buttons & !7 != 0 {
            self.revoke();
            return;
        }
        if buttons & 1 == 0 {
            return;
        }
        self.suspend();
        self.context = Some(surface);
        let mut packets = [None; CAPACITY];
        packets[0] = Some(Packet { x, y, buttons });
        self.pending = Some(Pending {
            surface,
            packets,
            len: 1,
        });
        self.buttons = buttons;
        self.push(Action::Focus { surface });
    }
    pub fn focus_result(&mut self, surface: SurfaceRef, accepted: bool) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        if pending.surface != surface {
            return false;
        }
        self.pending = None;
        if accepted {
            self.focused = Some(surface);
            for packet in pending.packets[..pending.len].iter().flatten() {
                self.buttons = packet.buttons;
                if !self.push(Action::Pointer {
                    surface,
                    x: packet.x,
                    y: packet.y,
                    buttons: packet.buttons,
                }) {
                    break;
                }
            }
        } else {
            self.revoke();
        }
        true
    }
    pub fn pointer(&mut self, surface: SurfaceRef, x: i32, y: i32, buttons: u32) {
        if buttons & !7 != 0 {
            self.revoke();
            return;
        }
        if let Some(pending) = self
            .pending
            .as_mut()
            .filter(|pending| pending.surface == surface)
        {
            // Preserve the first actual press location and every mask change.
            // Only a subsequent motion with the same mask may replace motion.
            let coalesce = pending.len >= 2
                && pending.packets[pending.len - 1].is_some_and(|p| p.buttons == buttons)
                && pending.packets[pending.len - 2].is_some_and(|p| p.buttons == buttons);
            if coalesce {
                pending.packets[pending.len - 1] = Some(Packet { x, y, buttons });
            } else if pending.len < CAPACITY {
                pending.packets[pending.len] = Some(Packet { x, y, buttons });
                pending.len += 1;
            } else {
                self.revoke();
                return;
            }
            self.buttons = buttons;
            return;
        }
        if self.focused == Some(surface) {
            self.buttons = buttons;
            self.push(Action::Pointer {
                surface,
                x,
                y,
                buttons,
            });
        }
    }
    pub fn leave(&mut self) {
        if self.buttons != 0 {
            self.revoke();
        }
    }
    pub fn key(&mut self, key: Key) -> bool {
        let Some(surface) = self.focused else {
            return self.context.is_some();
        };
        if matches!(
            key,
            Key::Character(_)
                | Key::Enter
                | Key::Backspace
                | Key::Tab
                | Key::Escape
                | Key::Up
                | Key::Down
                | Key::Left
                | Key::Right
        ) {
            self.push(Action::Key { surface, key });
        }
        true // Unsupported client keys never fall through into trusted terminal input.
    }
    pub fn suspend(&mut self) {
        self.revoke();
        self.context = None;
    }
    pub fn revoke(&mut self) {
        let active = self.focused.is_some()
            || self.pending.is_some()
            || self.len != 0
            || self.context.is_some();
        self.focused = None;
        self.pending = None;
        self.buttons = 0;
        self.queue = [None; CAPACITY];
        self.head = 0;
        self.len = 0;
        if active {
            self.queue[0] = Some(Action::Suspend);
            self.len = 1;
        }
    }
    pub fn take(&mut self) -> Option<Action> {
        if self.len == 0 {
            return None;
        }
        let action = self.queue[self.head].take();
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        action
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> SurfaceRef {
        SurfaceRef {
            slot: 1,
            generation: 7,
        }
    }
    #[test]
    fn chrome_focus_has_no_pointer_and_preserves_suspend_order() {
        let mut i = Interaction::new();
        i.gesture(id(), 1, 2);
        i.take();
        i.focus_result(id(), true);
        i.take();
        let next = SurfaceRef {
            slot: 1,
            generation: 2,
        };
        i.chrome_gesture(next);
        assert_eq!(i.take(), Some(Action::Suspend));
        assert_eq!(i.take(), Some(Action::FocusChrome { surface: next }));
        assert!(!i.focus_result(id(), true));
        assert!(i.focus_result(next, true));
        assert!(i.take().is_none());
        assert_eq!(i.buttons(), 0);
        i.chrome_gesture(next);
        i.take();
        i.take();
        assert!(i.focus_result(next, false));
        assert_eq!(i.take(), Some(Action::Suspend));
        assert!(i.key(Key::Enter));
        assert!(i.take().is_none());
    }
    #[test]
    fn gesture_requires_exact_ack_and_does_not_route_pending_keys_to_terminal() {
        let mut i = Interaction::new();
        i.gesture(id(), 2, 3);
        assert!(i.key(Key::Character('a')));
        assert_eq!(i.take(), Some(Action::Focus { surface: id() }));
        assert!(!i.focus_result(
            SurfaceRef {
                generation: 6,
                ..id()
            },
            true
        ));
        assert_eq!(i.focused(), None);
        assert!(i.focus_result(id(), true));
        assert_eq!(
            i.take(),
            Some(Action::Pointer {
                surface: id(),
                x: 2,
                y: 3,
                buttons: 1
            })
        );
        assert!(i.key(Key::Character('a')));
        assert!(matches!(
            i.take(),
            Some(Action::Key {
                key: Key::Character('a'),
                ..
            })
        ));
    }
    #[test]
    fn overflow_and_pointer_leave_revoke_focus_without_lost_release() {
        let mut i = Interaction::new();
        i.gesture(id(), 2, 3);
        i.take();
        i.focus_result(id(), true);
        i.take();
        for _ in 0..9 {
            i.key(Key::Character('x'));
        }
        assert_eq!(i.focused(), None);
        assert_eq!(i.take(), Some(Action::Suspend));
        assert_eq!(i.take(), None);
        assert!(i.key(Key::Enter));
        assert!(i.key(Key::Character('z')));
        assert_eq!(i.take(), None);
        i.suspend();
        assert!(!i.key(Key::Character('z')));
        i.gesture(id(), 2, 3);
        i.take();
        i.focus_result(id(), true);
        i.take();
        i.leave();
        assert_eq!(i.focused(), None);
        assert_eq!(i.take(), Some(Action::Suspend));
    }
    #[test]
    fn delayed_focus_ack_preserves_actual_press_then_release() {
        let mut i = Interaction::new();
        i.gesture(id(), 2, 3);
        i.take();
        i.pointer(id(), 4, 5, 0);
        assert!(i.focus_result(id(), true));
        assert_eq!(
            i.take(),
            Some(Action::Pointer {
                surface: id(),
                x: 2,
                y: 3,
                buttons: 1
            })
        );
        assert_eq!(
            i.take(),
            Some(Action::Pointer {
                surface: id(),
                x: 4,
                y: 5,
                buttons: 0
            })
        );
        assert_eq!(i.buttons(), 0);
    }
    #[test]
    fn pending_chords_preserve_each_discrete_mask_and_initial_location() {
        let mut i = Interaction::new();
        i.gesture_packet(id(), 1, 2, 3);
        assert_eq!(i.take(), Some(Action::Focus { surface: id() }));
        for (x, mask) in [(3, 3), (4, 3), (5, 7), (6, 6), (7, 2), (8, 0)] {
            i.pointer(id(), x, 2, mask);
        }
        assert!(i.focus_result(id(), true));
        for (x, mask) in [(1, 3), (4, 3), (5, 7), (6, 6), (7, 2), (8, 0)] {
            assert_eq!(
                i.take(),
                Some(Action::Pointer {
                    surface: id(),
                    x,
                    y: 2,
                    buttons: mask
                })
            );
        }
        assert_eq!(i.take(), None);
        assert_eq!(i.buttons(), 0);
    }
    #[test]
    fn pending_transition_overflow_revokes_and_swallow_keys_until_trusted_reset() {
        let mut i = Interaction::new();
        i.gesture_packet(id(), 1, 2, 1);
        i.take();
        for mask in [3, 2, 6, 4, 5, 7, 1, 0] {
            i.pointer(id(), 1, 2, mask);
        }
        assert!(!i.focus_result(id(), true));
        assert_eq!(i.take(), Some(Action::Suspend));
        assert_eq!(i.take(), None);
        assert!(i.key(Key::Enter));
        assert_eq!(i.focused(), None);
        assert_eq!(i.buttons(), 0);
    }
    #[test]
    fn right_middle_cannot_acquire_focus_and_unknown_mask_revokes() {
        let mut i = Interaction::new();
        for mask in [0, 2, 4, 6] {
            i.gesture_packet(id(), 1, 2, mask);
        }
        assert!(!i.context_active());
        assert_eq!(i.take(), None);
        i.gesture_packet(id(), 1, 2, 7);
        i.take();
        i.focus_result(id(), true);
        i.take();
        i.pointer(id(), 1, 2, 8);
        assert_eq!(i.take(), Some(Action::Suspend));
        assert!(i.context_active());
        assert_eq!(i.focused(), None);
    }
}
