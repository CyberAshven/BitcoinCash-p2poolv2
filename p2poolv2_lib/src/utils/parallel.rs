// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checks of many independent items run on several cores at once.
//!
//! A task that checks many items, such as the weak shares one share carries,
//! checks them on its own thread and on helper threads, as many as the
//! node's helper budget has free. The budget is one thread fewer than the
//! node's CPUs and is shared by every task, so tasks checking at the same
//! time never start more threads than the node has cores to spare. A task
//! that finds the budget spent checks its items alone, as before.
//!
//! The result is the one checking the items one after another gives: every
//! item's result in order, or the error of the first item in order that
//! fails. Which threads checked which items, and in what order they
//! finished, changes nothing.

use crate::utils::cpu::available_cpus;
use std::panic;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Fewest items a thread checks on average: a thread is started only for
/// this many more items, so a few items are checked on the calling thread
/// without the cost of starting one.
const MIN_ITEMS_PER_THREAD: usize = 8;

/// Helper threads checking items now, across the node.
static HELPERS_IN_USE: AtomicUsize = AtomicUsize::new(0);

/// The most helper threads the node runs at once: one fewer than its CPUs,
/// since every calling thread checks items too.
fn helper_budget() -> usize {
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| available_cpus().saturating_sub(1))
}

/// Helper threads taken from a budget, given back when dropped.
struct HelperThreads<'a> {
    in_use: &'a AtomicUsize,
    count: usize,
}

impl<'a> HelperThreads<'a> {
    /// Take up to `wanted` of the `budget` helper threads `in_use` counts,
    /// as many as are free: none when the budget is spent. Never waits.
    ///
    /// All `wanted` are counted first and the ones over the budget given
    /// back, so the threads taken never exceed the budget: one taking at the
    /// same moment may see fewer free than there are, never more.
    fn take(in_use: &'a AtomicUsize, budget: usize, wanted: usize) -> Self {
        let used = in_use.fetch_add(wanted, Ordering::AcqRel);
        let count = budget.saturating_sub(used).min(wanted);
        in_use.fetch_sub(wanted - count, Ordering::AcqRel);
        Self { in_use, count }
    }
}

impl Drop for HelperThreads<'_> {
    fn drop(&mut self) {
        self.in_use.fetch_sub(self.count, Ordering::AcqRel);
    }
}

/// Run `check` on every item and give the results in order, or the error of
/// the first item in order whose check fails: exactly what
/// `items.iter().map(check).collect()` gives, provided no item's check
/// depends on another's.
///
/// The calling thread checks items, joined by helper threads from the
/// node's budget when there are enough items and the budget has some free.
/// It returns once every thread has finished, so `check` may borrow from
/// the caller. A panic in `check` reaches the caller, as it would on one
/// thread.
pub fn try_map_in_parallel<T, R, E>(
    items: &[T],
    check: impl Fn(&T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
{
    let wanted = (items.len() / MIN_ITEMS_PER_THREAD).saturating_sub(1);
    let helpers = HelperThreads::take(&HELPERS_IN_USE, helper_budget(), wanted);
    try_map_on_threads(items, helpers.count, check)
}

/// [`try_map_in_parallel`] on the calling thread and up to `helpers` helper
/// threads; a helper the system cannot start leaves its items to the others.
///
/// Every thread takes the next item in order and stops once it would take
/// an item after one that failed. So every item before the first failure is
/// checked, whichever thread finished first, and its error is the one
/// returned; an item after it may have been checked too, and its result is
/// dropped.
fn try_map_on_threads<T, R, E>(
    items: &[T],
    helpers: usize,
    check: impl Fn(&T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
{
    if helpers == 0 {
        return items.iter().map(check).collect();
    }
    let next = AtomicUsize::new(0);
    let first_failure = AtomicUsize::new(usize::MAX);
    let share = items.len().div_ceil(helpers + 1);
    let check_items = || check_claimed_items(items, &next, &first_failure, &check, share);
    let mut checked = thread::scope(|scope| {
        let started: Vec<_> = (0..helpers)
            .filter_map(|_| {
                thread::Builder::new()
                    .name("parallel-check".to_string())
                    .spawn_scoped(scope, check_items)
                    .ok()
            })
            .collect();
        let mut checked = check_items();
        for helper in started {
            match helper.join() {
                Ok(results) => checked.extend(results),
                Err(payload) => panic::resume_unwind(payload),
            }
        }
        checked
    });
    checked.sort_unstable_by_key(|(index, _)| *index);
    checked.into_iter().map(|(_, result)| result).collect()
}

/// Check the next unclaimed item, in order, until none is left or the next
/// comes after one that failed, giving each checked item's index and result.
/// `share` is about how many items one thread checks.
fn check_claimed_items<T, R, E>(
    items: &[T],
    next: &AtomicUsize,
    first_failure: &AtomicUsize,
    check: &impl Fn(&T) -> Result<R, E>,
    share: usize,
) -> Vec<(usize, Result<R, E>)> {
    let claim = || {
        let index = next.fetch_add(1, Ordering::Relaxed);
        (index < items.len() && index < first_failure.load(Ordering::Relaxed)).then_some(index)
    };
    let mut checked = Vec::with_capacity(share);
    while let Some(index) = claim() {
        let result = check(&items[index]);
        if result.is_err() {
            first_failure.fetch_min(index, Ordering::Relaxed);
        }
        checked.push((index, result));
    }
    checked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Item 10 fails late and item 40 at once: checked one after another,
    /// item 10's error is the one returned, and so it is on four threads,
    /// though item 40's failure is found first.
    #[test]
    fn the_first_failure_in_order_wins_over_a_later_one_found_first() {
        let items: Vec<u32> = (0..64).collect();
        let check = |item: &u32| match *item {
            10 => {
                thread::sleep(Duration::from_millis(200));
                Err(format!("item {item}"))
            }
            40 => Err(format!("item {item}")),
            _ => Ok(item * 2),
        };

        let one_thread = try_map_on_threads(&items, 0, check);
        let four_threads = try_map_on_threads(&items, 3, check);

        assert_eq!(one_thread, Err("item 10".to_string()));
        assert_eq!(four_threads, one_thread);
    }

    #[test]
    fn every_result_comes_back_in_item_order() {
        let items: Vec<u32> = (0..64).collect();

        let doubled = try_map_on_threads(&items, 3, |item| Ok::<u32, String>(item * 2));

        assert_eq!(doubled, Ok((0..64).map(|item| item * 2).collect()));
    }

    /// Helper threads are given back when a check ends, or the node would
    /// soon check everything on one thread again.
    #[test]
    fn helper_threads_are_given_back() {
        let in_use = AtomicUsize::new(0);

        let first = HelperThreads::take(&in_use, 3, 5);
        assert_eq!(first.count, 3, "at most the budget");
        let second = HelperThreads::take(&in_use, 3, 1);
        assert_eq!(second.count, 0, "none while the budget is spent");
        drop(first);
        let third = HelperThreads::take(&in_use, 3, 1);
        assert_eq!(third.count, 1, "the first's helpers are free again");
        drop(second);
        drop(third);
        assert_eq!(in_use.load(Ordering::Acquire), 0);
    }
}
