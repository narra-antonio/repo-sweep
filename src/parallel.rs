//! Bounded parallel processing of independent items.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

/// Applies `work` to every item with at most `workers` threads, preserving the input order.
pub fn map<T, R, F>(items: &[T], workers: usize, work: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let workers = workers.clamp(1, items.len().max(1));
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    break;
                };
                let result = work(item);
                *results[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .expect("every item is processed")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order() {
        let items: Vec<u64> = (0..50).collect();
        let results = map(&items, 8, |n| n * 2);
        assert_eq!(results, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn handles_empty_input_and_zero_workers() {
        let empty: Vec<u8> = Vec::new();
        assert!(map(&empty, 4, |n| *n).is_empty());
        assert_eq!(map(&[1, 2, 3], 0, |n| n + 1), vec![2, 3, 4]);
    }
}
