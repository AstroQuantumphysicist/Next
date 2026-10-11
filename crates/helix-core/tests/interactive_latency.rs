//! Deterministic service-latency model for a focused interactive task under
//! non-yielding, quota-limited CPU work. This complements (and cannot replace)
//! the source-bound QEMU benchmark: all time here is synthetic nanoseconds.
use helix_core::{
    budget::{Budget, Config},
    scheduler::Scheduler,
};

const QUANTUM_NS: u64 = 1_000_000;
const INPUT_INTERVAL_NS: u64 = 37_000_000;
const DURATION_NS: u64 = 2_000_000_000;

#[test]
fn focused_task_gets_bounded_wakeup_latency_with_two_cpu_hogs() {
    let mut scheduler = Scheduler::<3>::new();
    let interactive = scheduler.spawn().unwrap();
    let hog_a = scheduler.spawn().unwrap();
    let hog_b = scheduler.spawn().unwrap();
    let tasks = [interactive, hog_a, hog_b];
    let config = Config::new(200_000_000, 100_000_000, 10_000_000).unwrap();
    let mut budgets = [
        None,
        Some(Budget::identified(config, 0, 101).unwrap()),
        Some(Budget::identified(config, 0, 102).unwrap()),
    ];

    let mut now = 0;
    let mut next_input = 0;
    let mut pending_since = None;
    let mut response_times = Vec::new();
    let mut interactive_service = 0;

    while now < DURATION_NS {
        if now >= next_input {
            if pending_since.is_none() {
                pending_since = Some(now);
            }
            next_input += INPUT_INTERVAL_NS;
        }

        let selected = scheduler
            .schedule_eligible(|task| {
                let slot = tasks
                    .iter()
                    .position(|candidate| *candidate == task)
                    .unwrap();
                budgets[slot]
                    .as_mut()
                    .is_none_or(|budget| budget.eligible_at(now).unwrap() <= now)
            })
            .expect("the protected interactive task remains runnable");
        let slot = tasks
            .iter()
            .position(|candidate| *candidate == selected)
            .unwrap();

        if selected == interactive {
            if let Some(arrival) = pending_since.take() {
                response_times.push(now - arrival);
            }
            interactive_service += QUANTUM_NS;
            now += QUANTUM_NS;
        } else {
            let budget = budgets[slot].as_mut().unwrap();
            let ticket = budget.begin(now).unwrap();
            let end = ticket.deadline().min(now + QUANTUM_NS);
            budget.finish(ticket, end).unwrap();
            now = end;
        }
    }

    assert!(
        response_times.len() >= 40,
        "interactive input did not make sustained progress"
    );
    assert!(
        response_times
            .iter()
            .all(|latency| *latency <= 3 * QUANTUM_NS),
        "worst modeled response was {} ms",
        response_times.iter().copied().max().unwrap() / 1_000_000
    );
    assert!(interactive_service >= 50_000_000);
    for budget in budgets[1..].iter().flatten() {
        assert!(budget.metrics().dispatches > 0);
        assert!(budget.metrics().occupied_ns >= 400_000_000);
    }
}
