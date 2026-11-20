//! The lock that serializes collection changes within an instance (the database advisory lock
//! serializes them across instances).
//!
//! A collection change holds the lock from reading the collections to saving its own, and the
//! collection hooks (`onCollectionCreate`, `onCollectionValidate`, `onCollectionCreateExecute`
//! and the update, delete and import ones) run inside it. A hook may change another collection
//! (`$app.save(collection)`): that nested change runs inside the outer one instead of waiting
//! for it, which would never end. What tells them apart is a token: each change that holds the
//! lock runs with its token as a value of its task ([`SchemaGuard::scope`]), handed to the
//! JavaScript thread that runs a hook for it ([`set_thread`]), as [`crate::origin`]'s chain is.
//! A change made with the token of the change holding the lock is nested in it; any other one
//! waits its turn.
//!
//! Nested changes run one at a time, too: one made while another nested change of the same
//! outer change runs (two Rust handlers running theirs together) waits for it to end.
//!
//! The outer change reads the collections before its hooks run. A nested change makes that
//! reading stale, so [`SchemaLock::writes`] counts nested changes: the outer change reads the
//! collections again when it went up, and refuses to save over a collection a hook changed.

use std::cell::Cell;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{MutexGuard, Notify};

tokio::task_local! {
    static HELD: u64;
}

thread_local! {
    static THREAD_HELD: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The token of the collection change running here, if any.
pub fn current() -> Option<u64> {
    HELD.try_with(|token| *token)
        .ok()
        .or_else(|| THREAD_HELD.with(Cell::get))
}

/// Sets the token of what this thread runs (a JavaScript runtime's thread, for the hook it
/// was given), or clears it.
pub fn set_thread(token: Option<u64>) {
    THREAD_HELD.with(|slot| slot.set(token));
}

#[derive(Default)]
pub struct SchemaLock {
    /// Held by the outer change.
    outer: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    /// A change ended (a nested one waiting for its turn checks again).
    released: Notify,
    tokens: AtomicU64,
}

#[derive(Default)]
struct State {
    /// The tokens of the outer change and of the nested changes running in it, outer first.
    held: Vec<u64>,
    /// Nested changes ended so far.
    writes: u64,
}

/// The lock is refused: the caller runs inside a collection change that can't let it in.
#[derive(Debug)]
pub struct Refused;

impl SchemaLock {
    /// Takes the lock for a collection change: at once when it is nested in the change holding
    /// the lock, else when that change ends.
    pub async fn acquire(&self) -> SchemaGuard<'_> {
        if let Some(token) = current() {
            loop {
                let released = self.released.notified();
                tokio::pin!(released);
                // registered before the state is read, so no release is missed
                released.as_mut().enable();
                {
                    let mut state = self.state.lock().expect("schema lock state");
                    match state.held.iter().position(|held| *held == token) {
                        // not this change's lock: wait for it like anyone else
                        None => break,
                        // nested in the innermost change
                        Some(at) if at + 1 == state.held.len() => {
                            let nested = self.token();
                            state.held.push(nested);
                            return SchemaGuard {
                                lock: self,
                                token: nested,
                                outer: None,
                            };
                        }
                        // another nested change of this one runs: after it
                        Some(_) => {}
                    }
                }
                released.await;
            }
        }
        let outer = self.outer.lock().await;
        let token = self.token();
        self.state
            .lock()
            .expect("schema lock state")
            .held
            .push(token);
        SchemaGuard {
            lock: self,
            token,
            outer: Some(outer),
        }
    }

    /// Takes the lock for something that replaces every collection (restoring a backup): never
    /// nested, and refused inside a collection change rather than waiting for it forever.
    pub async fn exclusive(&self) -> Result<SchemaGuard<'_>, Refused> {
        if let Some(token) = current()
            && self
                .state
                .lock()
                .expect("schema lock state")
                .held
                .contains(&token)
        {
            return Err(Refused);
        }
        let outer = self.outer.lock().await;
        let token = self.token();
        self.state
            .lock()
            .expect("schema lock state")
            .held
            .push(token);
        Ok(SchemaGuard {
            lock: self,
            token,
            outer: Some(outer),
        })
    }

    /// How many nested changes have ended. Changes are serialized, so while a change holds the
    /// lock, a different count than it saw means a hook changed collections in between.
    pub fn writes(&self) -> u64 {
        self.state.lock().expect("schema lock state").writes
    }

    fn token(&self) -> u64 {
        self.tokens.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// A held [`SchemaLock`]: let go when dropped.
pub struct SchemaGuard<'a> {
    lock: &'a SchemaLock,
    token: u64,
    outer: Option<MutexGuard<'a, ()>>,
}

impl SchemaGuard<'_> {
    /// Runs `work` as part of this change: changes it makes (through hooks) are nested in it.
    pub async fn scope<F: Future>(&self, work: F) -> F::Output {
        HELD.scope(self.token, work).await
    }
}

impl Drop for SchemaGuard<'_> {
    fn drop(&mut self) {
        {
            let mut state = self.lock.state.lock().expect("schema lock state");
            state.held.retain(|held| *held != self.token);
            if self.outer.is_none() {
                state.writes += 1;
            }
        }
        // the outer lock goes after the state is cleared, so the next change starts clean
        self.outer.take();
        self.lock.released.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn a_change_made_inside_a_change_is_nested_in_it() {
        let lock = SchemaLock::default();
        let outer = lock.acquire().await;
        assert_eq!(lock.writes(), 0);
        outer
            .scope(async {
                let nested = tokio::time::timeout(Duration::from_secs(1), lock.acquire())
                    .await
                    .expect("not blocked by the change it is part of");
                // and nested again, inside the nested one
                nested
                    .scope(async {
                        tokio::time::timeout(Duration::from_secs(1), lock.acquire())
                            .await
                            .expect("nested twice");
                    })
                    .await;
            })
            .await;
        assert_eq!(lock.writes(), 2);
        drop(outer);
        assert_eq!(lock.state.lock().unwrap().held, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn other_changes_wait_for_the_one_holding_the_lock() {
        let lock = Arc::new(SchemaLock::default());
        let outer = lock.acquire().await;
        let other = {
            let lock = lock.clone();
            tokio::spawn(async move {
                // a task of its own: not part of the change, whatever it was spawned from
                let _guard = lock.acquire().await;
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!other.is_finished());
        // a token left from an earlier change lets nothing in
        let stale = outer.token;
        drop(outer);
        other.await.unwrap();
        let next = lock.acquire().await;
        HELD.scope(stale, async {
            assert!(
                tokio::time::timeout(Duration::from_millis(50), lock.acquire())
                    .await
                    .is_err()
            );
        })
        .await;
        drop(next);
    }

    #[tokio::test]
    async fn nested_changes_run_one_at_a_time() {
        let lock = SchemaLock::default();
        let outer = lock.acquire().await;
        let running = std::sync::atomic::AtomicUsize::new(0);
        let most = std::sync::atomic::AtomicUsize::new(0);
        let nested = || async {
            let _guard = lock.acquire().await;
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            running.fetch_sub(1, Ordering::SeqCst);
        };
        outer
            .scope(async { tokio::join!(nested(), nested(), nested()) })
            .await;
        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert_eq!(lock.writes(), 3);
    }

    #[tokio::test]
    async fn a_restore_is_refused_inside_a_change_and_waits_outside_it() {
        let lock = SchemaLock::default();
        let outer = lock.acquire().await;
        outer
            .scope(async { assert!(lock.exclusive().await.is_err()) })
            .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lock.exclusive())
                .await
                .is_err(),
            "waits for the change"
        );
        drop(outer);
        drop(lock.exclusive().await.expect("free once the change ends"));
    }

    #[test]
    fn a_thread_carries_the_token_it_is_given() {
        assert_eq!(current(), None);
        set_thread(Some(7));
        assert_eq!(current(), Some(7));
        set_thread(None);
        assert_eq!(current(), None);
    }
}
