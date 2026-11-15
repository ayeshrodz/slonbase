//! Automations that run when a record changes.
//!
//! Every change of a collection's table is written to the outbox realtime reads
//! (`_slonbase.events`) by a trigger, in the transaction of the change: whatever made it (the
//! API, a hook, the SQL console, another program), and never when it was rolled back. The
//! dispatcher here turns those changes into queued runs of the automations that watch for
//! them.
//!
//! - **Once.** The dispatcher keeps the snapshot up to which changes were dispatched
//!   (`_slonbase.event_cursor`). One pass reads the changes committed since, queues a run for
//!   each automation they start, and moves the snapshot, all in one transaction: a change
//!   starts its automations exactly once, whichever instance looks (one at a time, under a
//!   lock) and whatever stops in between. The run itself is a queued job like any other, with
//!   the automation's retries.
//! - **In order, none skipped.** Changes are read by transaction snapshot, as realtime reads
//!   them: what committed after the last pass, whenever it began.
//! - **Kept until dispatched.** The outbox is pruned after a few minutes; a change the
//!   dispatcher has not read yet stays (for a day at most), so runs are not lost while no
//!   instance is up.
//! - **Nothing for those who don't use it.** Without an automation of this kind there is no
//!   snapshot, no pass and no extra work in the trigger; with one, an update of a watched
//!   collection also records which fields it changed.
//! - **It ends.** A change made by an automation carries the chain of automations it came
//!   through ([`crate::origin`]): none is started twice in a chain, and a chain is at most
//!   three long. At most [`MAX_WAITING`] runs of an automation wait at once: changes beyond
//!   that are not run, and the history says so.
//!
//! Bulk loads (a restore, an import) write no events, so they start nothing. Changes made
//! while an automation is switched off, or while jobs are paused, do not start it later.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use crate::core::Core;
use crate::cron_jobs::JobDef;

/// The most runs of one automation that wait at once.
pub const MAX_WAITING: i64 = 1000;
/// The most changes one pass turns into runs; more at once are a bulk change, not run.
const MAX_PER_PASS: usize = 5000;
/// A change not dispatched stays in the outbox this long at most.
const KEEP_HOURS: i64 = 24;
const LOCK_KEY: i64 = crate::migrations::SCHEMA_LOCK_KEY + 3;
/// Passes are at least this far apart, so a burst of writes is one pass.
const SETTLE: Duration = Duration::from_millis(20);
const POLL_EVERY: Duration = Duration::from_millis(500);
const POLL_IDLE: Duration = Duration::from_secs(5);

/// The automations that changes can start: switched on, and watching a collection.
fn watching(jobs: &[JobDef]) -> Vec<&JobDef> {
    jobs.iter()
        .filter(|job| job.enabled && job.trigger.is_record())
        .collect()
}

fn watched_collections(jobs: &[JobDef]) -> Vec<String> {
    let mut ids: Vec<String> = watching(jobs)
        .iter()
        .map(|job| job.trigger.collection.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The database's time now, as events' times are compared (`YYYY-MM-DD HH:MM:SS.ffffff`, UTC).
pub(crate) async fn now(core: &Core) -> anyhow::Result<String> {
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    Ok(client
        .query_one(&format!("SELECT {}", time_sql("now()")), &[])
        .await?
        .get(0))
}

fn time_sql(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US')")
}

/// Writes which collections are watched (so the trigger records the changed fields of their
/// updates) and, when something watches, the place changes are dispatched from if there is
/// none yet. Called when automations are saved, before the save is answered, so a change made
/// after it is one the dispatcher will read; and by the dispatcher when it finds the settings
/// changed.
///
/// It goes by the settings as stored, read under the dispatcher's lock, not by this
/// instance's copy of them: two saves, or an instance that hasn't heard of the last one,
/// can't leave the table behind what is saved.
pub(crate) async fn sync_watch(core: &Core) -> anyhow::Result<Vec<String>> {
    let mut client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&LOCK_KEY])
        .await?;
    let stored = crate::settings::Settings::load(&tx, core.secrets_key()).await?;
    let ids = watched_collections(&stored.cron_jobs);
    tx.execute(
        "WITH gone AS (
             DELETE FROM _slonbase.event_watch WHERE collection_id <> ALL($1)
         )
         INSERT INTO _slonbase.event_watch (collection_id)
         SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
        &[&ids],
    )
    .await?;
    match ids.is_empty() {
        // nothing to keep a place for: the outbox is pruned as before
        true => {
            tx.execute("DELETE FROM _slonbase.event_cursor", &[])
                .await?;
        }
        false => {
            tx.execute(
                "INSERT INTO _slonbase.event_cursor (one, snapshot)
                 VALUES (true, pg_current_snapshot()::text) ON CONFLICT (one) DO NOTHING",
                &[],
            )
            .await?;
        }
    }
    tx.commit().await?;
    core.realtime.dispatch.notify_one();
    Ok(ids)
}

/// The statement that prunes the outbox: events older than `seconds`, except those the
/// dispatcher has yet to read (not visible in its snapshot), which stay up to a day.
pub(crate) fn prune_sql(seconds: i64) -> String {
    format!(
        "DELETE FROM _slonbase.events e
         WHERE e.created < now() - interval '{seconds} seconds'
           AND (e.created < now() - interval '{KEEP_HOURS} hours' OR NOT EXISTS (
               SELECT 1 FROM _slonbase.event_cursor c
               WHERE e.xid >= pg_snapshot_xmax(c.snapshot::pg_snapshot)
                  OR e.xid = ANY(ARRAY(SELECT pg_snapshot_xip(c.snapshot::pg_snapshot)))));"
    )
}

/// How many changes wait to be dispatched for longer than a minute (none without a
/// snapshot): the dispatcher is behind when this is not zero.
pub(crate) async fn behind(core: &Core) -> anyhow::Result<i64> {
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let row = client
        .query_one(
            "SELECT count(*) FROM _slonbase.events e, _slonbase.event_cursor c
             WHERE e.created < now() - interval '1 minute'
               AND e.collection_id IN (SELECT collection_id FROM _slonbase.event_watch)
               AND (e.xid >= pg_snapshot_xmax(c.snapshot::pg_snapshot)
                    OR e.xid = ANY(ARRAY(SELECT pg_snapshot_xip(c.snapshot::pg_snapshot))))",
            &[],
        )
        .await?;
    Ok(row.get(0))
}

/// What a pass came to.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Pass {
    /// Runs queued.
    pub queued: usize,
    /// Changes not run because too many runs waited, by automation.
    pub dropped: HashMap<String, usize>,
    /// Nothing watches: there is nothing to look for.
    pub idle: bool,
}

/// What a step sees of the change that started its run.
fn trigger_json(event: &Value) -> Value {
    json!({
        "type": "record",
        "action": event["action"],
        "collection": event["collection_id"],
        "record": event["record"],
        "changed": match &event["changed"] {
            Value::Null => json!([]),
            changed => changed.clone(),
        },
    })
}

/// One pass: the changes committed since the last one become queued runs.
pub(crate) async fn pass(core: &Arc<Core>) -> anyhow::Result<Pass> {
    let settings = core.settings();
    let watching = watching(&settings.cron_jobs);
    let mut client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    if watching.is_empty() {
        // (the place kept goes when the stored settings say nothing watches: `sync_watch`,
        // which the dispatcher calls whenever its settings change)
        return Ok(Pass {
            idle: true,
            ..Default::default()
        });
    }
    let tx = client.transaction().await?;
    let locked: bool = tx
        .query_one("SELECT pg_try_advisory_xact_lock($1)", &[&LOCK_KEY])
        .await?
        .get(0);
    if !locked {
        // another instance is at it
        return Ok(Pass::default());
    }
    // the place kept; none yet (or one from another database, after a restore elsewhere):
    // from now on
    let since: Option<String> = tx
        .query_opt(
            "SELECT snapshot FROM _slonbase.event_cursor
             WHERE pg_snapshot_xmax(snapshot::pg_snapshot) <= pg_snapshot_xmax(pg_current_snapshot())",
            &[],
        )
        .await?
        .map(|row| row.get(0));
    let Some(since) = since else {
        tx.execute(
            "INSERT INTO _slonbase.event_cursor (one, snapshot)
             VALUES (true, pg_current_snapshot()::text)
             ON CONFLICT (one) DO UPDATE SET snapshot = excluded.snapshot, updated = now()",
            &[],
        )
        .await?;
        tx.commit().await?;
        return Ok(Pass::default());
    };
    let collections = watched_collections(&settings.cron_jobs);
    let row = tx
        .query_one(
            &format!(
                "SELECT pg_current_snapshot()::text, coalesce((
                 SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM (
                     SELECT e.id, e.collection_id, e.action, e.record, e.changed, e.origin,
                            {} AS at
                     FROM _slonbase.events e
                     WHERE e.collection_id = ANY($2)
                       AND (e.xid >= pg_snapshot_xmax($1::text::pg_snapshot)
                            OR e.xid = ANY(ARRAY(SELECT pg_snapshot_xip($1::text::pg_snapshot))))
                     ORDER BY e.id LIMIT $3
                 ) n
             ), '[]'::jsonb)",
                time_sql("e.created")
            ),
            &[&since, &collections, &(MAX_PER_PASS as i64 + 1)],
        )
        .await?;
    let next: String = row.get(0);
    let events: Value = row.get(1);
    let events = events.as_array().cloned().unwrap_or_default();
    if events.len() > MAX_PER_PASS {
        tracing::warn!(
            limit = MAX_PER_PASS,
            "more record changes at once than automations are started for: the rest were not run"
        );
    }
    let mut out = Pass::default();
    let mut waiting: HashMap<&str, i64> = HashMap::new();
    // while automations are paused nothing is queued, and the place still moves: what changed
    // meanwhile is not run when they go on
    let paused = settings.schedule.pause_jobs;
    for event in events.iter().take(MAX_PER_PASS).filter(|_| !paused) {
        let (collection, action) = (
            event["collection_id"].as_str().unwrap_or_default(),
            event["action"].as_str().unwrap_or_default(),
        );
        let origin = event["origin"].as_str();
        let trigger = trigger_json(event);
        for job in watching.iter().filter(|job| {
            job.trigger.collection == collection && job.trigger.actions.iter().any(|a| a == action)
        }) {
            if !crate::origin::may_start(origin, &job.id) {
                continue;
            }
            // (a change from before the automation was made or switched on is not its own)
            if event["at"].as_str().unwrap_or_default() < job.trigger.since.as_str() {
                continue;
            }
            let condition = job.trigger.condition.trim();
            if !condition.is_empty() {
                match crate::templating::holds(condition, &json!({ "trigger": trigger })) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(err) => {
                        tracing::warn!(job = %job.id, error = %err, "an automation's trigger condition failed: not run");
                        continue;
                    }
                }
            }
            let count = match waiting.get(job.id.as_str()) {
                Some(count) => *count,
                None => tx
                    .query_one(
                        "SELECT count(*) FROM _slonbase.jobs
                         WHERE kind = $1 AND ref = $2 AND state = 'queued'
                           AND payload->>'trigger' = 'record'",
                        &[&crate::cron_jobs::QUEUE_KIND, &job.id],
                    )
                    .await?
                    .get(0),
            };
            if count >= MAX_WAITING {
                waiting.insert(&job.id, count);
                *out.dropped.entry(job.id.clone()).or_default() += 1;
                continue;
            }
            let added = tx
                .execute(
                    "INSERT INTO _slonbase.jobs (kind, ref, payload, dedupe, max_attempts)
                     VALUES ($1, $2, $3, $4, $5) ON CONFLICT (dedupe) DO NOTHING",
                    &[
                        &crate::cron_jobs::QUEUE_KIND,
                        &job.id,
                        &json!({ "trigger": "record", "event": trigger, "origin": origin }),
                        &format!("event:{}:{}", event["id"], job.id),
                        &crate::cron_jobs::attempts(job),
                    ],
                )
                .await?;
            waiting.insert(&job.id, count + added as i64);
            out.queued += added as usize;
        }
    }
    tx.execute(
        "UPDATE _slonbase.event_cursor SET snapshot = $1, updated = now()",
        &[&next],
    )
    .await?;
    tx.commit().await?;
    // (given back before the history is written: the pool may have this one connection)
    drop(client);
    if out.queued > 0 {
        core.queue.wake();
    }
    for (id, count) in &out.dropped {
        tracing::warn!(job = %id, changes = count, "an automation has too many runs waiting: changes were not run");
        let run = crate::jobs::start(core, crate::jobs::CRON, id, "record").await;
        run.finish(Err(format!(
            "{count} changes were not run: {MAX_WAITING} runs of this automation were already waiting."
        )))
        .await;
    }
    Ok(out)
}

/// Starts the dispatcher of a core: it looks when something was written, and now and then
/// regardless (a notice can be lost); without anything watching it only waits to be told.
pub(crate) fn spawn(core: Weak<Core>) {
    tokio::spawn(async move {
        let mut every = POLL_EVERY;
        let mut synced: Option<Vec<String>> = None;
        // nothing watches and the place kept is gone: nothing is asked of the database
        let mut at_rest = false;
        loop {
            let Some(core) = core.upgrade() else { return };
            let wake = core.realtime.dispatch.clone();
            // (the trigger knows which collections are watched from a table: kept as the
            // settings say, also when another instance changed them)
            let watched = watched_collections(&core.settings().cron_jobs);
            if synced.as_ref() != Some(&watched) {
                match sync_watch(&core).await {
                    Ok(ids) => synced = Some(ids),
                    Err(err) => {
                        tracing::debug!(error = %format!("{err:#}"), "failed to write the watched collections")
                    }
                }
            }
            let result = match at_rest && watched.is_empty() {
                true => Ok(Pass {
                    idle: true,
                    ..Default::default()
                }),
                false => pass(&core).await,
            };
            at_rest = result.as_ref().is_ok_and(|pass| pass.idle);
            let wait = match result {
                Ok(pass) if pass.idle => None,
                Ok(pass) if pass.queued > 0 => {
                    every = POLL_EVERY;
                    Some(every)
                }
                Ok(_) => {
                    every = (every * 2).min(POLL_IDLE);
                    Some(every)
                }
                Err(err) => {
                    tracing::debug!(error = %format!("{err:#}"), "failed to dispatch record changes");
                    Some(POLL_IDLE)
                }
            };
            drop(core);
            match wait {
                Some(wait) => {
                    if tokio::time::timeout(wait, wake.notified()).await.is_ok() {
                        every = POLL_EVERY;
                    }
                }
                // nothing watches: only a change of the settings matters, and that wakes
                None => {
                    let _ = tokio::time::timeout(Duration::from_secs(60), wake.notified()).await;
                }
            }
            tokio::time::sleep(SETTLE).await;
        }
    });
}
