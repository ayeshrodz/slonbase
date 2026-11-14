//! The job queue (`_slonbase.jobs`): work done in the background, outside any request.
//!
//! - **Durable.** A job is a row. It stays until a worker has run it, so it outlives the
//!   instance that queued it.
//! - **Shared.** Every instance runs workers; a job is claimed by one of them
//!   (`FOR UPDATE SKIP LOCKED`) and held with a lease that the worker renews while it runs. A
//!   lease that runs out (the instance stopped) frees the job: it is run again if it has
//!   attempts left, and marked failed otherwise.
//! - **Once.** A job queued with a `dedupe` key exists once, whichever instance queues it
//!   first: that is how a scheduled job runs once a minute across instances.
//! - **Apart.** Workers take their connections from a pool of their own
//!   ([`Core::job_pool`]), so background work never uses a connection an API request is
//!   waiting for. What a job does through [`Core::db`] runs on that pool too.
//! - **Quiet.** A worker with nothing to do holds no connection and asks nothing of the
//!   database until a job is queued here or the next one is due; it looks once in a while for
//!   jobs left by instances that stopped.
//!
//! What a job *does* is registered by kind ([`Handlers`]); the queue knows nothing about the
//! features that use it.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::core::Core;
use crate::services::Failure;

/// How long a claim holds without being renewed.
const LEASE_SECONDS: i64 = 90;
/// How often a running job's lease is renewed.
const RENEW_EVERY: Duration = Duration::from_secs(20);
/// How often a worker with nothing to do looks for jobs left by instances that stopped.
const IDLE_LOOK: Duration = Duration::from_secs(600);
/// Finished jobs are kept this long.
const KEEP_DAYS: i64 = 7;

tokio::task_local! {
    /// Set while a worker runs a job: [`Core::db`] then answers with the workers' pool.
    static ON_WORKER: ();
}

/// Whether the current task is a queue worker running a job.
pub(crate) fn on_worker() -> bool {
    ON_WORKER.try_with(|_| ()).is_ok()
}

/// A claimed job, as its handler gets it.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    /// What it belongs to (a job definition's id).
    pub reference: String,
    pub payload: Value,
    /// Which attempt this is, from 1.
    pub attempt: i32,
    pub max_attempts: i32,
}

type Run = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send>>;
/// What runs jobs of one kind.
pub type Handler = Arc<dyn Fn(Arc<Core>, Job) -> Run + Send + Sync>;

/// The handlers by kind, filled in when the core is loaded.
#[derive(Default, Clone)]
pub struct Handlers(HashMap<&'static str, Handler>);

impl Handlers {
    /// Registers what runs jobs of `kind`.
    pub fn register<F, Fut>(&mut self, kind: &'static str, handler: F)
    where
        F: Fn(Arc<Core>, Job) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        self.0.insert(
            kind,
            Arc::new(move |core, job| Box::pin(handler(core, job))),
        );
    }
}

/// The queue's state on one core: its handlers and what wakes its workers.
#[derive(Default)]
pub struct Queue {
    pub(crate) handlers: Handlers,
    wake: Arc<tokio::sync::Notify>,
}

impl Queue {
    pub(crate) fn new(handlers: Handlers) -> Self {
        Self {
            handlers,
            wake: Default::default(),
        }
    }
}

/// A job to queue.
#[derive(Debug, Clone, Default)]
pub struct NewJob<'a> {
    pub kind: &'a str,
    pub reference: &'a str,
    pub payload: Value,
    /// When set, the job is only queued if none with this key exists.
    pub dedupe: Option<&'a str>,
    /// How many times it may be tried (at least 1).
    pub max_attempts: i32,
}

/// Queues a job and wakes the workers of this instance. Returns whether it was queued (false:
/// one with the same `dedupe` key exists already).
pub async fn enqueue(core: &Core, job: NewJob<'_>) -> Result<bool> {
    let client = core
        .db()
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let rows = client
        .execute(
            "INSERT INTO _slonbase.jobs (kind, ref, payload, dedupe, max_attempts)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (dedupe) DO NOTHING",
            &[
                &job.kind,
                &job.reference,
                &job.payload,
                &job.dedupe,
                &job.max_attempts.max(1),
            ],
        )
        .await?;
    drop(client);
    if rows == 1 {
        core.queue.wake.notify_waiters();
    }
    Ok(rows == 1)
}

/// Starts `count` workers for `core`. They stop when the core is dropped.
pub(crate) fn spawn_workers(core: &Arc<Core>, count: usize) {
    let instance = crate::ids::random_id(12);
    for _ in 0..count {
        tokio::spawn(worker(
            Arc::downgrade(core),
            core.queue.wake.clone(),
            instance.clone(),
        ));
    }
}

async fn worker(core: Weak<Core>, wake: Arc<tokio::sync::Notify>, instance: String) {
    loop {
        // (registered before looking, so a job queued meanwhile isn't missed)
        let woken = wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        let Some(strong) = core.upgrade() else { return };
        let wait = match claim(&strong, &instance).await {
            Ok(Some(job)) => {
                run(&strong, job, &instance).await;
                continue;
            }
            Ok(None) => next_wake(&strong).await,
            Err(err) => {
                tracing::debug!(error = %format!("{err:#}"), "the job queue couldn't be read");
                Duration::from_secs(30)
            }
        };
        drop(strong);
        tokio::select! {
            _ = &mut woken => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Takes the next job that is due, or one whose lease ran out and that has attempts left.
async fn claim(core: &Core, instance: &str) -> Result<Option<Job>> {
    let client = core
        .job_pool()
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    // left by an instance that stopped, with no attempt left
    client
        .execute(
            "UPDATE _slonbase.jobs SET state = 'failed', finished = now(),
                    error = 'Interrupted: the instance running it stopped.'
             WHERE state = 'running' AND locked_until < now() AND attempt >= max_attempts",
            &[],
        )
        .await?;
    let row = client
        .query_opt(
            &format!(
                "UPDATE _slonbase.jobs
                 SET state = 'running', locked_by = $1, attempt = attempt + 1,
                     locked_until = now() + interval '{LEASE_SECONDS} seconds',
                     started = COALESCE(started, now())
                 WHERE id = (
                     SELECT id FROM _slonbase.jobs
                     WHERE (state = 'queued' AND run_after <= now())
                        OR (state = 'running' AND locked_until < now())
                     ORDER BY run_after, id
                     FOR UPDATE SKIP LOCKED LIMIT 1
                 )
                 RETURNING id, kind, ref, payload, attempt, max_attempts"
            ),
            &[&instance],
        )
        .await?;
    Ok(row.map(|r| Job {
        id: r.get(0),
        kind: r.get(1),
        reference: r.get(2),
        payload: r.get(3),
        attempt: r.get(4),
        max_attempts: r.get(5),
    }))
}

/// How long a worker that found nothing waits: until the next job is due or the next lease
/// runs out, or a long while when the queue is empty. With an empty queue the workers' pool is
/// emptied, so an instance with nothing to do holds no connection for it.
async fn next_wake(core: &Core) -> Duration {
    let next = async {
        let client = core
            .job_pool()
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        let row = client
            .query_one(
                "SELECT extract(epoch FROM min(
                     CASE WHEN state = 'queued' THEN run_after ELSE locked_until END
                 ) - now())::float8
                 FROM _slonbase.jobs WHERE state IN ('queued', 'running')",
                &[],
            )
            .await?;
        anyhow::Ok(row.get::<_, Option<f64>>(0))
    };
    match next.await {
        Ok(Some(seconds)) => Duration::from_secs_f64(seconds.clamp(0.5, IDLE_LOOK.as_secs_f64())),
        Ok(None) => {
            core.job_pool().retain(|_, _| false);
            IDLE_LOOK
        }
        Err(_) => Duration::from_secs(30),
    }
}

/// Runs a claimed job with its lease renewed, and records how it ended.
async fn run(core: &Arc<Core>, job: Job, instance: &str) {
    let Some(handler) = core.queue.handlers.0.get(job.kind.as_str()).cloned() else {
        finish(
            core,
            &job,
            instance,
            Err(Failure::permanent(format!(
                "Nothing runs jobs of kind {}.",
                job.kind
            ))),
        )
        .await;
        return;
    };
    let renew = {
        let (core, id, instance) = (Arc::downgrade(core), job.id, instance.to_string());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(RENEW_EVERY).await;
                let Some(core) = core.upgrade() else { return };
                let Ok(client) = core.job_pool().get().await else {
                    continue;
                };
                let _ = client
                    .execute(
                        &format!(
                            "UPDATE _slonbase.jobs
                             SET locked_until = now() + interval '{LEASE_SECONDS} seconds'
                             WHERE id = $1 AND locked_by = $2 AND state = 'running'"
                        ),
                        &[&id, &instance],
                    )
                    .await;
            }
        })
    };
    // in a task of its own, so a handler that panics fails its job and not the worker
    let work = tokio::spawn(ON_WORKER.scope((), handler(core.clone(), job.clone())));
    let result = match work.await {
        Ok(result) => result,
        Err(_) => Err(Failure::permanent("The job stopped unexpectedly.")),
    };
    renew.abort();
    finish(core, &job, instance, result).await;
}

async fn finish(core: &Core, job: &Job, instance: &str, result: Result<(), Failure>) {
    let done = async {
        let client = core
            .job_pool()
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        match &result {
            Ok(()) => {
                client
                    .execute(
                        "UPDATE _slonbase.jobs SET state = 'done', finished = now(), error = ''
                         WHERE id = $1 AND locked_by = $2",
                        &[&job.id, &instance],
                    )
                    .await?;
            }
            // tried again later, waiting longer each time
            Err(failure) if failure.retry && job.attempt < job.max_attempts => {
                let wait = 30i64
                    .saturating_mul(1 << (job.attempt - 1).clamp(0, 10))
                    .min(300);
                client
                    .execute(
                        &format!(
                            "UPDATE _slonbase.jobs
                             SET state = 'queued', locked_by = NULL, locked_until = NULL,
                                 error = $3, run_after = now() + interval '{wait} seconds'
                             WHERE id = $1 AND locked_by = $2"
                        ),
                        &[&job.id, &instance, &short(&failure.message)],
                    )
                    .await?;
            }
            Err(failure) => {
                client
                    .execute(
                        "UPDATE _slonbase.jobs SET state = 'failed', finished = now(), error = $3
                         WHERE id = $1 AND locked_by = $2",
                        &[&job.id, &instance, &short(&failure.message)],
                    )
                    .await?;
            }
        }
        anyhow::Ok(())
    };
    if let Err(err) = done.await {
        tracing::warn!(job = job.id, error = %format!("{err:#}"), "failed to record a job's end");
    }
}

fn short(message: &str) -> String {
    message.chars().take(4000).collect()
}

/// Deletes the jobs that finished more than a week ago.
pub(crate) async fn prune(core: &Core) -> Result<()> {
    let client = core
        .db()
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    client
        .execute(
            &format!(
                "DELETE FROM _slonbase.jobs
                 WHERE state IN ('done', 'failed') AND finished < now() - interval '{KEEP_DAYS} days'"
            ),
            &[],
        )
        .await?;
    Ok(())
}
