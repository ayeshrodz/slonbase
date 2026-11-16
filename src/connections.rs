//! The database connections of one installation (the main one, or a project), split between
//! API requests and background jobs without opening more than `database.max_connections`.
//!
//! - **The API's pool** ([`Core::pool`](crate::core::Core::pool)) has `max_connections`
//!   less the reserve.
//! - **The reserve** (`database.job_connections`: 2 by default, 1 when `max_connections` is
//!   below 8) belongs to the job queue's workers while they work: a long job never takes a
//!   connection an API request needs, and a busy API never makes a job wait for one, beyond
//!   the requests that hold one of the reserve's when the workers start.
//! - **While no worker works**, API requests use the reserve too, once their own pool is in
//!   use, so the API has every connection when there is nothing in the background.
//!
//! Both are ordinary pools of fixed size, so the two together can never open more than
//! `max_connections`, whatever the order of events. A job that finds the reserve held for
//! [`JOB_WAIT`] (another job's long statement, with fewer reserved connections than workers)
//! then takes whichever connection frees first, the reserve's or the API's, rather than fail.
//! A request whose own pool is in use waits likewise for either, while no worker works.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use deadpool_postgres::{Object, Pool, PoolError, Status, Timeouts};

/// How long a job waits for a connection of the reserve before it takes one of the API's.
pub const JOB_WAIT: Duration = Duration::from_secs(2);

pub struct Connections {
    /// The API's own connections (the same pool as `Core::pool`).
    api: Pool,
    /// The reserve, when there is one (set when the core is started).
    reserve: OnceLock<Pool>,
    /// How many workers (and lease renewals) are working: while any is, the reserve is theirs.
    working: AtomicUsize,
}

/// Held while a worker works: the reserve is the workers' until the last one is dropped.
pub struct Working<'a>(&'a Connections);

impl Drop for Working<'_> {
    fn drop(&mut self) {
        self.0.working.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Connections {
    pub(crate) fn new(api: Pool) -> Self {
        Self {
            api,
            reserve: OnceLock::new(),
            working: AtomicUsize::new(0),
        }
    }

    /// Gives the job queue a reserve of its own (once).
    pub(crate) fn set_reserve(&self, reserve: Pool) {
        let _ = self.reserve.set(reserve);
    }

    /// How many connections are reserved for background jobs (0: they use the API's).
    pub fn reserved(&self) -> usize {
        self.reserve.get().map_or(0, |r| r.status().max_size)
    }

    /// A connection: the job queue's way (see [`Connections::for_jobs`]) on a worker running
    /// a job, the API's otherwise.
    pub async fn get(&self) -> Result<Object, PoolError> {
        if crate::queue::on_worker() {
            return self.for_jobs().await;
        }
        let Some(reserve) = self.reserve.get() else {
            return self.api.get().await;
        };
        if self.workers_busy() {
            return self.api.get().await;
        }
        // the API's own first, then the reserve, without waiting for either
        match self.api.timeout_get(&now(&self.api)).await {
            Err(PoolError::Timeout(deadpool_postgres::TimeoutType::Wait)) => {}
            answer => return answer,
        }
        if let Ok(object) = reserve.timeout_get(&now(reserve)).await {
            // (a worker that began meanwhile has it: it goes back to the reserve)
            if !self.workers_busy() {
                return Ok(object);
            }
        }
        // both in use: whichever frees one first (the reserve's only while no worker works)
        let api = self.api.get();
        tokio::pin!(api);
        if !self.workers_busy() {
            tokio::select! {
                biased;
                answer = &mut api => return answer,
                Ok(object) = reserve.get() => {
                    if !self.workers_busy() {
                        return Ok(object);
                    }
                }
            }
        }
        api.await
    }

    /// A connection for the job queue: one of the reserve, or after [`JOB_WAIT`] without one,
    /// one of the API's. Public for tests.
    #[doc(hidden)]
    pub async fn for_jobs(&self) -> Result<Object, PoolError> {
        let Some(reserve) = self.reserve.get() else {
            return self.api.get().await;
        };
        let reserved = reserve.get();
        tokio::pin!(reserved);
        tokio::select! {
            answer = &mut reserved => return answer,
            _ = tokio::time::sleep(JOB_WAIT) => {}
        }
        // still none: whichever frees one first, the reserve or the API's pool
        tracing::debug!("every reserved connection is in use: a job may take one of the API's");
        tokio::select! {
            biased;
            answer = &mut reserved => answer,
            answer = self.api.get() => answer,
        }
    }

    /// Marks a worker as working until the returned guard is dropped. Public for tests.
    #[doc(hidden)]
    pub fn working(&self) -> Working<'_> {
        self.working.fetch_add(1, Ordering::SeqCst);
        Working(self)
    }

    fn workers_busy(&self) -> bool {
        self.working.load(Ordering::SeqCst) > 0
    }

    /// The API's pool and the reserve together, as one pool's status.
    pub fn status(&self) -> Status {
        let mut status = self.api.status();
        if let Some(reserve) = self.reserve.get() {
            let more = reserve.status();
            status.max_size += more.max_size;
            status.size += more.size;
            status.available += more.available;
            status.waiting += more.waiting;
        }
        status
    }

    /// Closes the connections that have been idle for `idle` or longer.
    pub(crate) fn release_idle(&self, idle: Duration) {
        self.api.retain(|_, metrics| metrics.last_used() < idle);
        if let Some(reserve) = self.reserve.get() {
            reserve.retain(|_, metrics| metrics.last_used() < idle);
        }
    }

    /// Closes the reserved connections nobody is using (the queue is empty).
    pub(crate) fn release_idle_reserve(&self) {
        if let Some(reserve) = self.reserve.get() {
            reserve.retain(|_, _| false);
        }
    }

    /// Closes both pools (a project that is stopped).
    pub(crate) fn close(&self) {
        self.api.close();
        if let Some(reserve) = self.reserve.get() {
            reserve.close();
        }
    }
}

/// The pool's own timeouts, without waiting for a connection to be free.
fn now(pool: &Pool) -> Timeouts {
    let mut timeouts = pool.timeouts();
    timeouts.wait = Some(Duration::ZERO);
    timeouts
}
