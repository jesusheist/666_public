//! A bounded ring of recent items. The lock is held to push or to copy the
//! handles out; filtering and formatting happen outside it.

use std::collections::VecDeque;
use std::sync::Mutex;

pub struct Ring<T> {
    cap: usize,
    inner: Mutex<Inner<T>>,
}

struct Inner<T> {
    items: VecDeque<T>,
    /// Pushed since the start (kept ones and dropped ones).
    total: u64,
}

impl<T: Clone> Ring<T> {
    pub const fn new(cap: usize) -> Self {
        Ring { cap, inner: Mutex::new(Inner { items: VecDeque::new(), total: 0 }) }
    }

    pub fn push(&self, item: T) {
        let evicted = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.total += 1;
            inner.items.push_back(item);
            if inner.items.len() > self.cap { inner.items.pop_front() } else { None }
        };
        // Dropped outside the lock.
        drop(evicted);
    }

    /// Every item kept, oldest first.
    pub fn snapshot(&self) -> Vec<T> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.items.iter().cloned().collect()
    }

    /// Items pushed since the start.
    pub fn total(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).total
    }

    /// The last `n` items for which `keep` holds, oldest first.
    pub fn last_matching(&self, n: usize, mut keep: impl FnMut(&T) -> bool) -> Vec<T> {
        let all = self.snapshot();
        let mut out: Vec<T> = all.into_iter().rev().filter(|i| keep(i)).take(n).collect();
        out.reverse();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_and_counts_all() {
        let ring = Ring::new(3);
        for i in 0..10 {
            ring.push(i);
        }
        assert_eq!(ring.snapshot(), vec![7, 8, 9]);
        assert_eq!(ring.total(), 10);
    }

    #[test]
    fn last_matching_is_oldest_first_and_bounded() {
        let ring = Ring::new(100);
        for i in 0..50 {
            ring.push(i);
        }
        assert_eq!(ring.last_matching(3, |i| i % 2 == 0), vec![44, 46, 48]);
        assert_eq!(ring.last_matching(5, |i| *i > 47), vec![48, 49]);
        assert!(ring.last_matching(5, |_| false).is_empty());
        assert!(Ring::<u8>::new(4).last_matching(5, |_| true).is_empty());
    }
}
