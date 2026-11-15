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
use crate::settings::Settings;

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

/// What the watchers are, in a few bytes: who watches what, for which changes, under which
/// condition. A dispatcher goes by its instance's copy of the settings; it only dispatches when
/// this is what the stored settings came to (see [`reconcile`]).
fn fingerprint(jobs: &[JobDef]) -> String {
    use sha2::Digest;
    let watching: Vec<Value> = watching(jobs)
        .iter()
        .map(|job| json!([job.id, job.trigger]))
        .collect();
    match watching.is_empty() {
        true => String::new(),
        false => hex::encode(sha2::Sha256::digest(json!(watching).to_string().as_bytes())),
    }
}

/// What of the settings the dispatcher goes by: when it differs between two versions of them,
/// the tables here have to follow.
pub(crate) fn state(settings: &Settings) -> (String, bool) {
    (
        fingerprint(&settings.cron_jobs),
        settings.schedule.pause_jobs,
    )
}

fn time_sql(column: &str) -> String {
    format!("to_char(({column}) AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US')")
}

/// Makes the dispatcher's tables say what `settings` say, in `tx`. It is called in the
/// transaction that saves the settings ([`crate::settings::modify`]), so the two change
/// together or not at all, and under the dispatcher's lock, so no pass runs across it:
///
/// - who watches, and since when: an automation that begins to watch (made, switched on,
///   pointed at another collection) watches from this moment, and a change older than that
///   never starts it;
/// - the place changes are dispatched from, made when the first automation watches and
///   removed with the last (the outbox is then pruned as before);
/// - what the watchers are, for a dispatcher to compare its own settings with;
/// - when automations were paused: changes made from then until they go on are not run.
pub(crate) async fn reconcile(
    tx: &deadpool_postgres::Transaction<'_>,
    settings: &Settings,
) -> anyhow::Result<()> {
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&LOCK_KEY])
        .await?;
    let watching = watching(&settings.cron_jobs);
    let jobs: Vec<&str> = watching.iter().map(|job| job.id.as_str()).collect();
    let collections: Vec<&str> = watching
        .iter()
        .map(|job| job.trigger.collection.as_str())
        .collect();
    // (one that watches another collection than it did begins again)
    tx.execute(
        "DELETE FROM _slonbase.event_watchers w
         WHERE NOT EXISTS (
             SELECT 1 FROM unnest($1::text[], $2::text[]) AS now(job, collection_id)
             WHERE now.job = w.job AND now.collection_id = w.collection_id)",
        &[&jobs, &collections],
    )
    .await?;
    tx.execute(
        "INSERT INTO _slonbase.event_watchers (job, collection_id, since)
         SELECT job, collection_id, clock_timestamp()
         FROM unnest($1::text[], $2::text[]) AS now(job, collection_id)
         ON CONFLICT (job) DO NOTHING",
        &[&jobs, &collections],
    )
    .await?;
    if watching.is_empty() {
        tx.execute("DELETE FROM _slonbase.event_cursor", &[])
            .await?;
        return Ok(());
    }
    tx.execute(
        "INSERT INTO _slonbase.event_cursor (one, snapshot, watching)
         VALUES (true, pg_current_snapshot()::text, $1)
         ON CONFLICT (one) DO UPDATE SET watching = excluded.watching",
        &[&fingerprint(&settings.cron_jobs)],
    )
    .await?;
    // the pauses: every one of the last day is kept, since a change that began in one can be
    // read after several more (a transaction left open meanwhile)
    let row = tx
        .query_one(
            &format!(
                "SELECT pauses, {}, {} FROM _slonbase.event_cursor",
                time_sql("clock_timestamp()"),
                time_sql(&format!(
                    "clock_timestamp() - interval '{KEEP_HOURS} hours'"
                ))
            ),
            &[],
        )
        .await?;
    let (stored, now, kept_from): (Value, String, String) = (row.get(0), row.get(1), row.get(2));
    let pauses = noted_pause(
        serde_json::from_value(stored).unwrap_or_default(),
        settings.schedule.pause_jobs,
        &now,
        &kept_from,
    );
    tx.execute(
        "UPDATE _slonbase.event_cursor SET pauses = $1",
        &[&serde_json::to_value(&pauses)?],
    )
    .await?;
    Ok(())
}

/// A pause of automations: from when, and until when (none while it lasts). Times as
/// [`time_sql`] writes them, so they compare as text.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Pause {
    from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<String>,
}

/// The pauses with the current state noted at `now`: one begins when automations are paused
/// and none is open, the open one ends when they go on. Those that ended before `kept_from`
/// are let go (a change older than that is no longer in the outbox).
fn noted_pause(mut pauses: Vec<Pause>, paused: bool, now: &str, kept_from: &str) -> Vec<Pause> {
    let open = pauses.last().is_some_and(|p| p.to.is_none());
    match (paused, open) {
        (true, false) => pauses.push(Pause {
            from: now.to_string(),
            to: None,
        }),
        (false, true) => {
            if let Some(last) = pauses.last_mut() {
                last.to = Some(now.to_string());
            }
        }
        _ => {}
    }
    pauses.retain(|p| p.to.as_deref().is_none_or(|to| to >= kept_from));
    pauses
}

/// Whether a change made at `at` was made while automations were paused.
fn during_pause(pauses: &[Pause], at: &str) -> bool {
    pauses
        .iter()
        .any(|p| at >= p.from.as_str() && p.to.as_deref().is_none_or(|to| at < to))
}

/// [`reconcile`] with the settings as stored, outside a save: what the dispatcher does when it
/// starts, and when it finds the tables are not what its settings say (a restore emptied them,
/// or the settings were changed by something that doesn't go through `modify`).
pub(crate) async fn sync_watch(core: &Core) -> anyhow::Result<()> {
    let mut client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let tx = client.transaction().await?;
    // (the lock before the settings are read: a save that commits in between would otherwise
    // be undone by what was read before it)
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&LOCK_KEY])
        .await?;
    let stored = Settings::load(&tx, core.secrets_key()).await?;
    reconcile(&tx, &stored).await?;
    tx.commit().await?;
    Ok(())
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
               AND e.collection_id IN (SELECT collection_id FROM _slonbase.event_watchers)
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
    /// The tables are not what this instance's settings say: nothing was dispatched.
    pub stale: bool,
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
    // the place kept, with what the watchers were when it was written and the pauses
    let cursor = tx
        .query_opt(
            "SELECT snapshot, watching, pauses,
                    pg_snapshot_xmax(snapshot::pg_snapshot) <= pg_snapshot_xmax(pg_current_snapshot())
             FROM _slonbase.event_cursor",
            &[],
        )
        .await?;
    // no place, or the watchers there are not this instance's: the settings were changed and
    // this instance has yet to hear of it (or the tables were emptied). Nothing is dispatched
    // by settings that may be behind: a change would be passed over for an automation this
    // instance doesn't know of
    let Some(cursor) =
        cursor.filter(|row| row.get::<_, String>(1) == fingerprint(&settings.cron_jobs))
    else {
        return Ok(Pass {
            stale: true,
            ..Default::default()
        });
    };
    let pauses: Vec<Pause> = serde_json::from_value(cursor.get(2)).unwrap_or_default();
    // (a place from another database, after a restore elsewhere: from now on)
    if !cursor.get::<_, bool>(3) {
        tx.execute(
            "UPDATE _slonbase.event_cursor SET snapshot = pg_current_snapshot()::text, updated = now()",
            &[],
        )
        .await?;
        tx.commit().await?;
        return Ok(Pass::default());
    }
    let since: String = cursor.get(0);
    // since when each automation watches
    let watchers: HashMap<String, String> = tx
        .query(
            &format!(
                "SELECT job, {} FROM _slonbase.event_watchers",
                time_sql("since")
            ),
            &[],
        )
        .await?
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
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
    // (and what changed during a pause is not run however late it is read: after they went on,
    // or after more pauses)
    for event in events.iter().take(MAX_PER_PASS).filter(|_| !paused) {
        let at = event["at"].as_str().unwrap_or_default();
        if during_pause(&pauses, at) {
            continue;
        }
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
            // (a change from before the automation began to watch is not its own)
            if watchers
                .get(&job.id)
                .is_none_or(|since| at < since.as_str())
            {
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
        // what the tables were last made to say, as this instance's settings have it
        let mut synced: Option<(String, bool)> = None;
        // nothing watches and the place kept is gone: nothing is asked of the database
        let mut at_rest = false;
        loop {
            let Some(core) = core.upgrade() else { return };
            let wake = core.realtime.dispatch.clone();
            // (the tables follow the settings in the transaction that saves them; this is for
            // a start, a restore, and settings changed in some other way)
            let settings = core.settings();
            let wanted = state(&settings);
            let watched = watched_collections(&settings.cron_jobs);
            drop(settings);
            if synced.as_ref() != Some(&wanted) {
                match sync_watch(&core).await {
                    Ok(()) => synced = Some(wanted),
                    Err(err) => {
                        tracing::debug!(error = %format!("{err:#}"), "failed to write who watches for record changes")
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
                // (the tables and these settings disagree: made to agree with what is stored
                // on the next round, and tried again once this instance has the settings)
                Ok(pass) if pass.stale => {
                    synced = None;
                    Some(POLL_EVERY)
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_AGO: &str = "2026-01-01 00:00:00.000000";

    #[test]
    fn every_pause_is_kept() {
        let at = |s: &str| format!("2026-01-02 {s}.000000");
        // paused, going on, paused again and going on again: both are kept
        let mut pauses = noted_pause(Vec::new(), true, &at("10:00:00"), DAY_AGO);
        pauses = noted_pause(pauses, true, &at("10:00:30"), DAY_AGO);
        pauses = noted_pause(pauses, false, &at("10:01:00"), DAY_AGO);
        pauses = noted_pause(pauses, true, &at("11:00:00"), DAY_AGO);
        pauses = noted_pause(pauses, false, &at("11:01:00"), DAY_AGO);
        pauses = noted_pause(pauses, false, &at("11:02:00"), DAY_AGO);
        assert_eq!(pauses.len(), 2);
        // a change that began in the first pause is still one of a pause after the second
        assert!(during_pause(&pauses, &at("10:00:10")));
        assert!(during_pause(&pauses, &at("11:00:59")));
        assert!(!during_pause(&pauses, &at("10:30:00")));
        assert!(!during_pause(&pauses, &at("11:01:00")));
        // one that is going on covers what comes after it
        let open = noted_pause(pauses.clone(), true, &at("12:00:00"), DAY_AGO);
        assert!(during_pause(&open, &at("23:59:59")));
        // those that ended before the day kept are let go
        let later = noted_pause(pauses, false, &at("12:00:00"), &at("10:30:00"));
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].from, at("11:00:00"));
    }
}
