//! Totals of list requests (`totalItems`), kept between requests. Counting every matching row
//! is most of a list's cost (all of the rows, against a page of them), and most lists are read
//! far more often than their collections change.
//!
//! An entry is keyed by the count's exact SQL and parameter values, so it's only ever served
//! for the same filter, rule and caller (rules put the caller's id, and anything else they
//! read, in the parameters). It holds while the data generation it was counted at is current:
//! the generation moves on every change of any collection's data, from any instance or
//! program (the realtime triggers' notifications), with this instance's own writes counted at
//! commit, and on truncates, restores and imports. The generation is read before counting, so
//! a write committed during the count leaves the entry stale rather than wrong. As a safety net
//! for changes nothing reports, entries also expire after [`TTL`]. Never cached: counts over
//! view collections (their queries may read any table) and SQL that depends on the request's
//! time (`@now`, `strftime()`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long an entry is trusted at most, whatever the generation says.
pub const TTL: Duration = Duration::from_secs(5);
/// Entries kept at most; past it, the expired and stale ones go, then all.
const MAX_ENTRIES: usize = 4096;
/// Longer keys (SQL and parameters) aren't cached.
const MAX_KEY: usize = 8 << 10;

struct Entry {
    generation: u64,
    at: Instant,
    count: i64,
}

#[derive(Default)]
pub struct CountCache {
    generation: AtomicU64,
    entries: Mutex<HashMap<String, Entry>>,
}

impl CountCache {
    /// The current data generation, read before counting (see the module docs).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Records that data changed: every entry becomes stale.
    pub fn changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The key of a count: its SQL and its parameters, unambiguously (a parameter can't
    /// imitate the separator, since lengths come first).
    pub fn key(sql: &str, params: &[Option<String>]) -> Option<String> {
        let mut key = String::with_capacity(sql.len() + 16 * params.len() + 8);
        key.push_str(&sql.len().to_string());
        key.push(':');
        key.push_str(sql);
        for p in params {
            match p {
                Some(v) => {
                    key.push_str(&format!("|{}:", v.len()));
                    key.push_str(v);
                }
                None => key.push_str("|-"),
            }
        }
        (key.len() <= MAX_KEY).then_some(key)
    }

    pub fn get(&self, key: &str) -> Option<i64> {
        let generation = self.generation();
        let entries = self.entries.lock().expect("count cache lock");
        entries
            .get(key)
            .filter(|e| e.generation == generation && e.at.elapsed() < TTL)
            .map(|e| e.count)
    }

    /// Keeps a count made at `generation` (read before the count ran).
    pub fn put(&self, key: String, generation: u64, count: i64) {
        if generation != self.generation() {
            return;
        }
        let mut entries = self.entries.lock().expect("count cache lock");
        if entries.len() >= MAX_ENTRIES {
            let current = self.generation();
            entries.retain(|_, e| e.generation == current && e.at.elapsed() < TTL);
            if entries.len() >= MAX_ENTRIES {
                entries.clear();
            }
        }
        entries.insert(
            key,
            Entry {
                generation,
                at: Instant::now(),
                count,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn served_until_data_changes() {
        let cache = CountCache::default();
        let key = CountCache::key("SELECT count(*)", &[Some("a".into())]).unwrap();
        assert_eq!(cache.get(&key), None);
        let generation = cache.generation();
        cache.put(key.clone(), generation, 42);
        assert_eq!(cache.get(&key), Some(42));
        cache.changed();
        assert_eq!(cache.get(&key), None);
    }

    #[test]
    fn a_count_overtaken_by_a_write_isnt_kept() {
        let cache = CountCache::default();
        let key = CountCache::key("q", &[]).unwrap();
        let generation = cache.generation();
        // a write commits while the count runs
        cache.changed();
        cache.put(key.clone(), generation, 1);
        assert_eq!(cache.get(&key), None);
    }

    #[test]
    fn keys_are_unambiguous() {
        let key = |p: &[Option<String>]| CountCache::key("q", p).unwrap();
        assert_ne!(
            key(&[Some("a|1:b".into())]),
            key(&[Some("a".into()), Some("b".into())])
        );
        assert_ne!(key(&[None]), key(&[Some("-".into())]));
        assert_ne!(key(&[Some(String::new())]), key(&[]));
        assert!(CountCache::key(&"x".repeat(MAX_KEY), &[]).is_none());
    }

    #[test]
    fn bounded() {
        let cache = CountCache::default();
        for i in 0..MAX_ENTRIES + 10 {
            cache.put(format!("k{i}"), cache.generation(), 1);
        }
        assert!(cache.entries.lock().unwrap().len() <= MAX_ENTRIES);
    }
}
