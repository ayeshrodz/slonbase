//! What made a change, when an automation did: the automations a chain of changes went
//! through.
//!
//! An automation that runs when a record changes can change records itself, and so start
//! automations, itself included. So that this ends, what an automation's run writes is marked
//! with its chain (`slonbase.origin`, set for the transaction; the trigger that records the
//! change copies it into the event), and the dispatcher starts no automation a second time in
//! a chain, nor any chain longer than [`MAX_DEPTH`] (see [`crate::events`]).
//!
//! The chain travels with the work, not with the connection: it is a value of the task that
//! runs the automation ([`scope`]), handed to the JavaScript thread that runs a handler for it
//! ([`set_thread`]), and written into each transaction that writes records ([`mark`]). What a
//! run does through another server (a URL it calls that writes back through the API) is not
//! marked; the limit on waiting runs bounds that.

use std::cell::RefCell;
use std::future::Future;

/// The most automations a chain of changes goes through.
pub const MAX_DEPTH: usize = 3;

tokio::task_local! {
    static CHAIN: String;
}

thread_local! {
    static THREAD_CHAIN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The chain of what is running here, if an automation started it: automation ids, comma
/// separated, the first being the one whose trigger was a change nobody's automation made.
pub fn current() -> Option<String> {
    CHAIN
        .try_with(|chain| chain.clone())
        .ok()
        .or_else(|| THREAD_CHAIN.with(|chain| chain.borrow().clone()))
}

/// Runs `work` as part of `chain`.
pub async fn scope<F: Future>(chain: String, work: F) -> F::Output {
    CHAIN.scope(chain, work).await
}

/// Sets the chain of what this thread runs (a JavaScript runtime's thread, for the handler it
/// was given), or clears it.
pub fn set_thread(chain: Option<String>) {
    THREAD_CHAIN.with(|slot| *slot.borrow_mut() = chain);
}

/// `chain` with `automation` added.
pub fn extended(chain: Option<&str>, automation: &str) -> String {
    match chain.filter(|c| !c.is_empty()) {
        Some(chain) => format!("{chain},{automation}"),
        None => automation.to_string(),
    }
}

/// Whether a change made by `chain` may start `automation`: not one already in the chain, and
/// not past the depth.
pub fn may_start(chain: Option<&str>, automation: &str) -> bool {
    let Some(chain) = chain.filter(|c| !c.is_empty()) else {
        return true;
    };
    let ids: Vec<&str> = chain.split(',').collect();
    ids.len() < MAX_DEPTH && !ids.contains(&automation)
}

/// Marks the transaction with the chain of what is running, when there is one. Called where a
/// transaction that writes records begins.
pub async fn mark<C: deadpool_postgres::GenericClient>(
    tx: &C,
) -> Result<(), tokio_postgres::Error> {
    if let Some(chain) = current() {
        tx.execute("SELECT set_config('slonbase.origin', $1, true)", &[&chain])
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_ends() {
        assert!(may_start(None, "user:a"));
        assert!(may_start(Some(""), "user:a"));
        assert!(!may_start(Some("user:a"), "user:a"));
        assert!(may_start(Some("user:a"), "user:b"));
        assert!(!may_start(Some("user:a,user:b"), "user:a"));
        assert!(may_start(Some("user:a,user:b"), "user:c"));
        // three automations deep, nothing more starts
        assert!(!may_start(Some("user:a,user:b,user:c"), "user:d"));
        assert_eq!(extended(None, "user:a"), "user:a");
        assert_eq!(extended(Some("user:a"), "user:b"), "user:a,user:b");
    }

    #[tokio::test]
    async fn the_chain_goes_with_the_task() {
        assert_eq!(current(), None);
        let inside = scope("user:a".into(), async { current() }).await;
        assert_eq!(inside.as_deref(), Some("user:a"));
        assert_eq!(current(), None);
        set_thread(Some("user:b".into()));
        assert_eq!(current().as_deref(), Some("user:b"));
        set_thread(None);
        assert_eq!(current(), None);
    }
}
