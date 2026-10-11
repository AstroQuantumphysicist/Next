//! Single-CPU bounded round-robin state machine, without context switching.
//! Synchronization, timers, priority policy and SMP run queues belong to callers.
use crate::handle::{Handle, HandleError, HandleTable, Rights};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Exited,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    Handle(HandleError),
    InvalidTransition,
}
impl From<HandleError> for ScheduleError {
    fn from(error: HandleError) -> Self {
        Self::Handle(error)
    }
}
pub struct Scheduler<const N: usize> {
    tasks: HandleTable<TaskState, N>,
    order: [Option<Handle>; N],
    cursor: usize,
    current: Option<Handle>,
}
impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Scheduler<N> {
    pub fn new() -> Self {
        Self {
            tasks: HandleTable::new(),
            order: [None; N],
            cursor: 0,
            current: None,
        }
    }
    pub fn spawn(&mut self) -> Result<Handle, ScheduleError> {
        let slot = self
            .order
            .iter()
            .position(Option::is_none)
            .ok_or(HandleError::Full)?;
        let handle = self.tasks.insert(TaskState::Ready, Rights(0))?;
        self.order[slot] = Some(handle);
        Ok(handle)
    }
    pub fn state(&self, handle: Handle) -> Result<TaskState, ScheduleError> {
        Ok(*self.tasks.get(handle, Rights(0))?)
    }
    pub fn current(&self) -> Option<Handle> {
        self.current
    }
    pub fn wake(&mut self, handle: Handle) -> Result<(), ScheduleError> {
        let state = self.tasks.get_mut(handle, Rights(0))?;
        if *state != TaskState::Blocked {
            return Err(ScheduleError::InvalidTransition);
        }
        *state = TaskState::Ready;
        Ok(())
    }
    pub fn block_current(&mut self) -> Result<Handle, ScheduleError> {
        let handle = self.current.ok_or(ScheduleError::InvalidTransition)?;
        *self.tasks.get_mut(handle, Rights(0))? = TaskState::Blocked;
        self.current = None;
        Ok(handle)
    }
    pub fn exit(&mut self, handle: Handle) -> Result<(), ScheduleError> {
        let state = self.tasks.get_mut(handle, Rights(0))?;
        if *state == TaskState::Exited {
            return Err(ScheduleError::InvalidTransition);
        }
        *state = TaskState::Exited;
        if self.current == Some(handle) {
            self.current = None;
        }
        Ok(())
    }
    pub fn reap(&mut self, handle: Handle) -> Result<(), ScheduleError> {
        if self.state(handle)? != TaskState::Exited {
            return Err(ScheduleError::InvalidTransition);
        }
        self.tasks.close(handle)?;
        for slot in &mut self.order {
            if *slot == Some(handle) {
                *slot = None;
                break;
            }
        }
        Ok(())
    }
    /// Timer/yield decision. Returns None when all tasks are blocked or exited.
    pub fn schedule(&mut self) -> Option<Handle> {
        self.schedule_eligible(|_| true)
    }
    /// Bounded one-pass selection. Ineligible Ready tasks retain their state;
    /// blocking/wake, quotas and authorization remain caller-owned policies.
    pub fn schedule_eligible(
        &mut self,
        mut eligible: impl FnMut(Handle) -> bool,
    ) -> Option<Handle> {
        if let Some(handle) = self.current.take() {
            if let Ok(state) = self.tasks.get_mut(handle, Rights(0)) {
                *state = TaskState::Ready;
            }
        }
        for _ in 0..N {
            let slot = self.cursor;
            self.cursor = if slot + 1 == N { 0 } else { slot + 1 };
            if let Some(handle) = self.order[slot] {
                if let Ok(state) = self.tasks.get_mut(handle, Rights(0)) {
                    if *state == TaskState::Ready && eligible(handle) {
                        *state = TaskState::Running;
                        self.current = Some(handle);
                        return Some(handle);
                    }
                }
            }
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtered_selection_is_bounded_and_keeps_throttled_tasks_ready() {
        let mut scheduler = Scheduler::<3>::new();
        let a = scheduler.spawn().unwrap();
        let b = scheduler.spawn().unwrap();
        let c = scheduler.spawn().unwrap();
        assert_eq!(scheduler.schedule_eligible(|task| task == b), Some(b));
        assert_eq!(scheduler.state(a), Ok(TaskState::Ready));
        assert_eq!(scheduler.state(c), Ok(TaskState::Ready));
        let mut visited = 0;
        assert_eq!(
            scheduler.schedule_eligible(|_| {
                visited += 1;
                false
            }),
            None
        );
        assert_eq!(visited, 3);
        assert_eq!(scheduler.current(), None);
        for expected in [c, a, b, c, a, b] {
            assert_eq!(scheduler.schedule_eligible(|_| true), Some(expected));
        }
        scheduler.block_current().unwrap();
        assert_eq!(scheduler.schedule_eligible(|task| task == b), None);
        scheduler.wake(b).unwrap();
        assert_eq!(scheduler.schedule_eligible(|task| task == b), Some(b));
    }
    #[test]
    fn fair_rotation_and_lifecycle() {
        let mut scheduler = Scheduler::<3>::new();
        let a = scheduler.spawn().unwrap();
        let b = scheduler.spawn().unwrap();
        let c = scheduler.spawn().unwrap();
        for _ in 0..100 {
            for expected in [a, b, c] {
                assert_eq!(scheduler.schedule(), Some(expected));
            }
        }
        scheduler.block_current().unwrap();
        assert_eq!(scheduler.state(c), Ok(TaskState::Blocked));
        assert_eq!(scheduler.schedule(), Some(a));
        scheduler.exit(a).unwrap();
        scheduler.reap(a).unwrap();
        assert_eq!(scheduler.schedule(), Some(b));
        scheduler.exit(b).unwrap();
        assert_eq!(scheduler.schedule(), None);
        scheduler.wake(c).unwrap();
        assert_eq!(scheduler.schedule(), Some(c));
        assert_eq!(scheduler.wake(c), Err(ScheduleError::InvalidTransition));
        let replacement = scheduler.spawn().unwrap();
        assert_ne!(replacement, a);
        assert_eq!(
            scheduler.wake(a),
            Err(ScheduleError::Handle(HandleError::Invalid))
        );
    }
    #[test]
    fn empty_and_invalid_operations() {
        let mut scheduler = Scheduler::<0>::new();
        assert_eq!(scheduler.schedule(), None);
        assert_eq!(
            scheduler.spawn(),
            Err(ScheduleError::Handle(HandleError::Full))
        );
        assert_eq!(
            scheduler.block_current(),
            Err(ScheduleError::InvalidTransition)
        );
        let mut scheduler = Scheduler::<1>::new();
        let a = scheduler.spawn().unwrap();
        assert_eq!(scheduler.reap(a), Err(ScheduleError::InvalidTransition));
    }
}
