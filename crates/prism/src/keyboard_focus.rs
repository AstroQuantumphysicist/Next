//! Bounded keyboard focus foundation. This model does not implement an assistive
//! technology transport: callers expose these labels, roles and states to one.
//! Controls are traversed in registration order; trusted modal scopes trap focus.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlId {
    pub scope: u32,
    pub local: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Button,
    TextBox,
    ListItem,
    CheckBox,
    Window,
    Dialog,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Control<'a> {
    pub id: ControlId,
    pub label: &'a str,
    pub role: Role,
    pub enabled: bool,
    pub visible: bool,
    pub tab_stop: bool,
    pub checked: Option<bool>,
}
impl Control<'_> {
    fn eligible(self) -> bool {
        self.enabled && self.visible && self.tab_stop
    }
    fn valid(self) -> bool {
        self.id.scope != 0
            && self.id.local != 0
            && !self.label.is_empty()
            && self.label.len() <= 128
            && !self.label.chars().any(char::is_control)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidControl,
    DuplicateControl,
    Capacity,
    WrongScope,
    StaleModal,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Forward,
    Reverse,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input {
    Keyboard,
    Pointer,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State<'a> {
    pub control: Control<'a>,
    pub focused: bool,
    pub focus_visible: bool,
}
#[derive(Clone, Copy)]
struct Modal {
    scope: u32,
    restore: Option<ControlId>,
    restore_visible: bool,
}

#[derive(Clone, Copy)]
pub struct Focus<'a, const N: usize = 64, const M: usize = 4> {
    controls: [Option<Control<'a>>; N],
    base_scope: u32,
    focused: Option<ControlId>,
    focus_visible: bool,
    modals: [Option<Modal>; M],
    depth: usize,
}
impl<'a, const N: usize, const M: usize> Focus<'a, N, M> {
    pub const fn new(base_scope: u32) -> Self {
        Self {
            controls: [None; N],
            base_scope,
            focused: None,
            focus_visible: false,
            modals: [None; M],
            depth: 0,
        }
    }
    pub fn active_scope(&self) -> u32 {
        if self.depth == 0 {
            self.base_scope
        } else {
            self.modals[self.depth - 1].map_or(self.base_scope, |modal| modal.scope)
        }
    }
    pub fn focused(&self) -> Option<ControlId> {
        self.focused
    }
    pub fn focus_visible(&self) -> bool {
        self.focused.is_some() && self.focus_visible
    }
    pub fn modal_depth(&self) -> usize {
        self.depth
    }
    pub fn state(&self, id: ControlId) -> Option<State<'a>> {
        self.controls
            .iter()
            .flatten()
            .find(|c| c.id == id)
            .map(|c| State {
                control: *c,
                focused: self.focused == Some(id),
                focus_visible: self.focused == Some(id) && self.focus_visible,
            })
    }
    fn eligible(&self, id: ControlId) -> bool {
        id.scope == self.active_scope()
            && self.state(id).is_some_and(|state| state.control.eligible())
    }
    fn first(&self) -> Option<ControlId> {
        self.controls
            .iter()
            .flatten()
            .find(|c| c.id.scope == self.active_scope() && c.eligible())
            .map(|c| c.id)
    }
    /// Replace one scope atomically. Registration order is its tab order.
    /// Invalid labels, duplicate IDs and exhausted capacity leave focus unchanged.
    pub fn replace_scope(&mut self, scope: u32, controls: &[Control<'a>]) -> Result<(), Error> {
        if scope == 0 || controls.iter().any(|c| c.id.scope != scope || !c.valid()) {
            return Err(Error::InvalidControl);
        }
        let mut next = *self;
        for slot in &mut next.controls {
            if slot.is_some_and(|c| c.id.scope == scope) {
                *slot = None;
            }
        }
        for control in controls {
            if next.controls.iter().flatten().any(|c| c.id == control.id) {
                return Err(Error::DuplicateControl);
            }
            let Some(slot) = next.controls.iter_mut().find(|slot| slot.is_none()) else {
                return Err(Error::Capacity);
            };
            *slot = Some(*control);
        }
        if !next.focused.is_some_and(|id| next.eligible(id)) {
            next.focused = next.first();
        }
        *self = next;
        Ok(())
    }
    /// Modal priority cannot be bypassed by selecting a background window.
    pub fn set_base_scope(&mut self, scope: u32) -> Result<(), Error> {
        if scope == 0 || self.depth != 0 {
            return Err(Error::WrongScope);
        }
        self.base_scope = scope;
        self.focused = self.first();
        self.focus_visible = true;
        Ok(())
    }
    pub fn select(&mut self, id: ControlId, input: Input) -> Result<(), Error> {
        if !self.eligible(id) {
            return Err(Error::WrongScope);
        }
        self.focused = Some(id);
        self.focus_visible = input == Input::Keyboard;
        Ok(())
    }
    pub fn traverse(&mut self, direction: Direction) -> Option<ControlId> {
        let mut order = [None; N];
        let mut count = 0;
        for c in self.controls.iter().flatten() {
            if c.id.scope == self.active_scope() && c.eligible() {
                order[count] = Some(c.id);
                count += 1;
            }
        }
        self.focus_visible = true;
        self.focused = if count == 0 {
            None
        } else {
            let index = order[..count].iter().position(|id| *id == self.focused);
            let target = match (index, direction) {
                (Some(i), Direction::Forward) => (i + 1) % count,
                (Some(i), Direction::Reverse) => (i + count - 1) % count,
                (None, Direction::Forward) => 0,
                (None, Direction::Reverse) => count - 1,
            };
            order[target]
        };
        self.focused
    }
    /// Callers push higher-priority trusted scopes, with Cancel registered first.
    /// Empty dialogs trap focus too; they cannot tab into background controls.
    pub fn push_modal(&mut self, scope: u32) -> Result<(), Error> {
        if scope == 0
            || scope == self.base_scope
            || self.modals.iter().flatten().any(|m| m.scope == scope)
        {
            return Err(Error::WrongScope);
        }
        if self.depth == M {
            return Err(Error::Capacity);
        }
        self.modals[self.depth] = Some(Modal {
            scope,
            restore: self.focused,
            restore_visible: self.focus_visible,
        });
        self.depth += 1;
        self.focused = self.first();
        self.focus_visible = true;
        Ok(())
    }
    /// Only the current modal may dismiss itself. Stale expiry IDs cannot close
    /// a newer dialog or erase its focus restoration state.
    pub fn pop_modal(&mut self, scope: u32) -> Result<(), Error> {
        if self.depth == 0 || self.active_scope() != scope {
            return Err(Error::StaleModal);
        }
        self.depth -= 1;
        let modal = self.modals[self.depth].take().ok_or(Error::StaleModal)?;
        self.focused = modal
            .restore
            .filter(|id| self.eligible(*id))
            .or_else(|| self.first());
        self.focus_visible = modal.restore_visible;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(scope: u32, local: u32) -> Control<'static> {
        Control {
            id: ControlId { scope, local },
            label: "Actual action",
            role: Role::Button,
            enabled: true,
            visible: true,
            tab_stop: true,
            checked: None,
        }
    }
    #[test]
    fn forward_reverse_wrap_skip_disabled_hidden_and_non_tab_controls() {
        let mut focus = Focus::<8, 2>::new(1);
        let mut disabled = c(1, 2);
        disabled.enabled = false;
        let mut hidden = c(1, 3);
        hidden.visible = false;
        let mut label = c(1, 4);
        label.tab_stop = false;
        focus
            .replace_scope(1, &[c(1, 1), disabled, hidden, label, c(1, 5)])
            .unwrap();
        assert_eq!(focus.focused(), Some(c(1, 1).id));
        assert_eq!(focus.traverse(Direction::Forward), Some(c(1, 5).id));
        assert_eq!(focus.traverse(Direction::Forward), Some(c(1, 1).id));
        assert_eq!(focus.traverse(Direction::Reverse), Some(c(1, 5).id));
        assert!(focus.focus_visible());
    }
    #[test]
    fn trusted_nested_modals_trap_focus_and_restore_exact_control() {
        let mut focus = Focus::<8, 2>::new(1);
        for scope in 1..=3 {
            focus
                .replace_scope(scope, &[c(scope, 1), c(scope, 2)])
                .unwrap();
        }
        focus.select(c(1, 2).id, Input::Keyboard).unwrap();
        focus.push_modal(2).unwrap();
        assert_eq!(
            focus.select(c(1, 1).id, Input::Pointer),
            Err(Error::WrongScope)
        );
        assert_eq!(focus.set_base_scope(3), Err(Error::WrongScope));
        focus.select(c(2, 2).id, Input::Keyboard).unwrap();
        focus.push_modal(3).unwrap();
        assert_eq!(focus.pop_modal(2), Err(Error::StaleModal));
        assert_eq!(focus.active_scope(), 3);
        focus.pop_modal(3).unwrap();
        assert_eq!(focus.focused(), Some(c(2, 2).id));
        focus.pop_modal(2).unwrap();
        assert_eq!(focus.focused(), Some(c(1, 2).id));
        assert!(focus.focus_visible());
    }
    #[test]
    fn stale_removed_restore_target_falls_back_and_empty_modal_stays_trapped() {
        let mut focus = Focus::<4, 2>::new(1);
        focus.replace_scope(1, &[c(1, 1), c(1, 2)]).unwrap();
        focus.select(c(1, 2).id, Input::Keyboard).unwrap();
        focus.push_modal(2).unwrap();
        assert_eq!(focus.traverse(Direction::Forward), None);
        focus.replace_scope(1, &[c(1, 1)]).unwrap();
        assert_eq!(focus.focused(), None);
        focus.pop_modal(2).unwrap();
        assert_eq!(focus.focused(), Some(c(1, 1).id));
    }
    #[test]
    fn replacement_is_atomic_for_duplicate_capacity_and_invalid_labels() {
        let mut focus = Focus::<2, 1>::new(1);
        focus.replace_scope(1, &[c(1, 1), c(1, 2)]).unwrap();
        focus.select(c(1, 2).id, Input::Keyboard).unwrap();
        assert_eq!(
            focus.replace_scope(1, &[c(1, 1), c(1, 1)]),
            Err(Error::DuplicateControl)
        );
        assert_eq!(focus.replace_scope(2, &[c(2, 1)]), Err(Error::Capacity));
        let mut invalid = c(1, 3);
        invalid.label = "Misleading\nlabel";
        assert_eq!(
            focus.replace_scope(1, &[invalid]),
            Err(Error::InvalidControl)
        );
        assert_eq!(focus.focused(), Some(c(1, 2).id));
        assert!(focus.state(c(1, 1).id).is_some());
    }
    #[test]
    fn pointer_and_keyboard_visibility_preserve_accessible_role_label_and_check_state() {
        let mut focus = Focus::<4, 1>::new(1);
        let mut check = c(1, 1);
        check.role = Role::CheckBox;
        check.label = "Do not disturb";
        check.checked = Some(true);
        focus.replace_scope(1, &[check, c(1, 2)]).unwrap();
        focus.select(check.id, Input::Pointer).unwrap();
        let state = focus.state(check.id).unwrap();
        assert_eq!(state.control.checked, Some(true));
        assert!(state.focused && !state.focus_visible);
        focus.traverse(Direction::Reverse);
        assert!(focus.focus_visible());
    }
    #[test]
    fn zero_capacity_and_modal_overflow_do_not_panic_or_escape() {
        let mut empty = Focus::<0, 0>::new(1);
        assert_eq!(empty.traverse(Direction::Forward), None);
        assert_eq!(empty.replace_scope(1, &[c(1, 1)]), Err(Error::Capacity));
        assert_eq!(empty.push_modal(2), Err(Error::Capacity));
        let mut focus = Focus::<2, 1>::new(1);
        focus.push_modal(2).unwrap();
        assert_eq!(focus.push_modal(3), Err(Error::Capacity));
        assert_eq!(focus.active_scope(), 2);
        assert_eq!(focus.modal_depth(), 1);
    }
}
