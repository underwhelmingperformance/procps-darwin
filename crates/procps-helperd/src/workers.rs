// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    time::Instant,
};

use darwin_proc::Uid;

/// Limits the snapshots that the helper takes at once.
///
/// A request that finds every worker busy waits for one. When a worker
/// becomes free, the request whose user has the fewest snapshots running goes
/// next, and requests from the same user go in the order that they arrived.
/// One user who sends many requests therefore delays mostly their own.
#[derive(Debug)]
pub(crate) struct Workers {
    state: Mutex<State>,
    freed: Condvar,
    limit: NonZeroUsize,
}

/// The snapshots that are running and the requests that are waiting.
#[derive(Debug, Default)]
struct State {
    /// The number of snapshots running for each user.
    running: HashMap<Uid, usize>,
    /// The number of snapshots running for all users.
    total: usize,
    /// The waiting requests, by ticket, with their users.
    waiting: Vec<(u64, Uid)>,
    /// The ticket for the next request.
    next_ticket: u64,
}

impl State {
    /// The ticket of the request that goes next.
    fn next(&self) -> Option<u64> {
        self.waiting
            .iter()
            .min_by_key(|(ticket, uid)| (self.running.get(uid).copied().unwrap_or(0), *ticket))
            .map(|(ticket, _)| *ticket)
    }
}

impl Workers {
    /// Workers for at most `limit` snapshots at once.
    pub(crate) fn new(limit: NonZeroUsize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            freed: Condvar::new(),
            limit,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until a worker is free and it is the turn of `uid`'s request, and
    /// returns the worker, or returns `None` when `until` passes first.
    pub(crate) fn wait(&self, uid: Uid, until: Instant) -> Option<Worker<'_>> {
        let mut state = self.state();
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push((ticket, uid));

        loop {
            if state.total < self.limit.get() && state.next() == Some(ticket) {
                state.waiting.retain(|(waiting, _)| *waiting != ticket);
                state.total += 1;
                *state.running.entry(uid).or_default() += 1;

                // A waiter that woke before this request took its worker may
                // have found that this request was next, and gone back to
                // sleep while another worker was free.
                if state.total < self.limit.get() && !state.waiting.is_empty() {
                    self.freed.notify_all();
                }

                return Some(Worker { workers: self, uid });
            }

            let remaining = until.saturating_duration_since(Instant::now());

            if remaining.is_zero() {
                state.waiting.retain(|(waiting, _)| *waiting != ticket);
                // Another request may now be next.
                self.freed.notify_all();

                return None;
            }

            state = self
                .freed
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A worker taken for one snapshot. Dropping it frees the worker.
#[derive(Debug)]
pub(crate) struct Worker<'a> {
    workers: &'a Workers,
    uid: Uid,
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        let mut state = self.workers.state();
        state.total -= 1;

        if let Some(running) = state.running.get_mut(&self.uid) {
            *running -= 1;

            if *running == 0 {
                state.running.remove(&self.uid);
            }
        }

        self.workers.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroUsize,
        time::{Duration, Instant},
    };

    use darwin_proc::Uid;
    use pretty_assertions::assert_eq;

    use super::{State, Workers};

    #[test]
    fn a_user_with_fewer_snapshots_running_goes_first() {
        let state = State {
            running: [(Uid::from(501), 1)].into_iter().collect(),
            total: 1,
            waiting: vec![
                (1, Uid::from(501)),
                (2, Uid::from(502)),
                (3, Uid::from(502)),
            ],
            next_ticket: 4,
        };

        assert_eq!(state.next(), Some(2));
    }

    #[test]
    fn a_request_waits_no_longer_than_its_deadline() {
        let workers = Workers::new(NonZeroUsize::MIN);
        let first = workers.wait(Uid::from(501), Instant::now());
        let second = workers.wait(Uid::from(502), Instant::now() + Duration::from_millis(10));

        assert_eq!((first.is_some(), second.is_some()), (true, false));
    }

    #[test]
    fn a_free_worker_goes_to_a_waiting_request_at_once() {
        // Repeat, because a lost wakeup shows up only in some interleavings.
        for _ in 0..10 {
            let workers = Workers::new(NonZeroUsize::MIN.saturating_add(2));
            let far = Instant::now() + Duration::from_secs(5);
            let freed = [
                workers.wait(Uid::from(9), far),
                workers.wait(Uid::from(9), far),
            ];
            let running = workers.wait(Uid::from(1), far);

            let prompt = std::thread::scope(|scope| {
                // User 1's request waits first. If it wakes first, it finds
                // that user 2's request is next and waits again. Each waiter
                // keeps the worker that it gets, so that freeing it does not
                // wake the other waiter. If a waiter's wakeup is lost, it
                // sleeps until its deadline, two seconds later, so the test
                // requires each waiter to get a worker within one second.
                let waiters = [Uid::from(1), Uid::from(2)].map(|uid| {
                    let workers = &workers;
                    let waiter = scope.spawn(move || {
                        let started = Instant::now();
                        let worker = workers.wait(uid, started + Duration::from_secs(2));

                        (worker, started.elapsed())
                    });
                    std::thread::sleep(Duration::from_millis(5));
                    waiter
                });
                drop(freed);

                waiters.map(|waiter| {
                    waiter.join().is_ok_and(|(worker, waited)| {
                        worker.is_some() && waited < Duration::from_secs(1)
                    })
                })
            });
            drop(running);

            assert_eq!(prompt, [true, true]);
        }
    }

    #[test]
    fn a_freed_worker_goes_to_the_next_request() {
        let workers = Workers::new(NonZeroUsize::MIN);
        let first = workers.wait(Uid::from(501), Instant::now());
        drop(first);
        let second = workers.wait(Uid::from(502), Instant::now());

        assert!(second.is_some());
    }
}
