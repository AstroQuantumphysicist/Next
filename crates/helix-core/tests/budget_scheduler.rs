//! Uses actual core eligibility and budget owners; no hardware clock is faked
//! as guest evidence. Units below are deterministic monotonic nanoseconds.
use helix_core::{
    budget::{Budget, Config},
    scheduler::{Scheduler, TaskState},
};
#[test]
fn bounded_picker_preserves_protected_progress_and_normal_quota_fairness() {
    let mut scheduler = Scheduler::<3>::new();
    let tasks = [
        scheduler.spawn().unwrap(),
        scheduler.spawn().unwrap(),
        scheduler.spawn().unwrap(),
    ];
    let config = Config::new(100, 30, 10).unwrap();
    let mut budgets = [
        Some(Budget::identified(config, 0, 1).unwrap()),
        Some(Budget::identified(config, 0, 2).unwrap()),
        None,
    ];
    let mut occupied = [0u64; 3];
    let mut now = 0;
    let mut protected_last = 0;
    let mut rejected = 0;
    while now < 1000 {
        let selected = scheduler
            .schedule_eligible(|task| {
                let slot = tasks
                    .iter()
                    .position(|candidate| *candidate == task)
                    .unwrap();
                let available = budgets[slot]
                    .as_mut()
                    .is_none_or(|budget| budget.eligible_at(now).unwrap() <= now);
                if !available {
                    rejected += 1;
                }
                available
            })
            .unwrap();
        let slot = tasks
            .iter()
            .position(|candidate| *candidate == selected)
            .unwrap();
        let elapsed = if let Some(budget) = &mut budgets[slot] {
            let ticket = budget.begin(now).unwrap();
            let elapsed = ticket.deadline() - now;
            budget.finish(ticket, now + elapsed).unwrap();
            elapsed
        } else {
            assert!(
                now - protected_last <= 30,
                "protected task lost bounded progress"
            );
            protected_last = now;
            10
        };
        assert!(elapsed > 0);
        occupied[slot] += elapsed;
        now += elapsed;
    }
    assert!(rejected > 0);
    assert!(occupied[0] <= 300 && occupied[1] <= 300);
    assert!(occupied[0].abs_diff(occupied[1]) <= 10);
    assert!(occupied[2] >= 400);
    assert_eq!(occupied.iter().sum::<u64>(), now);
    for slot in 0..2 {
        assert_eq!(
            budgets[slot].as_ref().unwrap().metrics().occupied_ns,
            occupied[slot]
        );
    }
}
#[test]
fn blocking_and_ipc_wake_do_not_recreate_exhausted_budget() {
    let mut scheduler = Scheduler::<2>::new();
    let ordinary = scheduler.spawn().unwrap();
    let protected = scheduler.spawn().unwrap();
    let mut budget = Budget::identified(Config::new(100, 10, 10).unwrap(), 0, 1).unwrap();
    assert_eq!(scheduler.schedule(), Some(ordinary));
    let ticket = budget.begin(0).unwrap();
    scheduler.block_current().unwrap();
    budget.finish(ticket, 10).unwrap();
    scheduler.wake(ordinary).unwrap();
    assert_eq!(scheduler.state(ordinary), Ok(TaskState::Ready));
    assert_eq!(
        scheduler
            .schedule_eligible(|task| task == protected || budget.eligible_at(10).unwrap() <= 10),
        Some(protected)
    );
    assert_eq!(budget.metrics().occupied_ns, 10);
    assert_eq!(budget.credit(), 0);
    assert_eq!(
        scheduler
            .schedule_eligible(|task| task == protected || budget.eligible_at(100).unwrap() <= 100),
        Some(ordinary)
    );
}
