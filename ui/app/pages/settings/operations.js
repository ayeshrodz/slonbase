// Settings → Backups and Cron jobs: what's there, what runs when, and what ran (the jobs'
// history, /api/jobs/runs).
import { html, useState, useEffect, cx } from "../../lib/core.js";
import { api, qs, enc, getFileToken, BASE } from "../../lib/api.js";
import { Icon } from "../../lib/icons.js";
import { Button, IconButton, Field, Input, Select, Switch, Skeleton, Empty, Segmented, toast, confirmDialog } from "../../lib/ui.js";
import { bytes, formatDate, formatDateIn, relativeTime, timeZones, browserTimeZone } from "../../lib/format.js";
import { Card } from "./shared.js";
import { S3Fields, TestS3 } from "./storage.js";

const ms = (n) => (n == null ? "—" : n < 1000 ? `${n} ms` : n < 60000 ? `${(n / 1000).toFixed(1)} s` : `${Math.round(n / 60000)} min`);

export function StatusChip({ status }) {
  const kind = status === "ok" ? "ok" : status === "error" ? "danger" : "warn";
  const label = status === "ok" ? "Succeeded" : status === "error" ? "Failed" : "Running";
  return html`<span class=${"chip " + kind}>${label}</span>`;
}

// the latest runs of a kind of job (or of one job)
export function JobHistory({ kind, job, refreshKey, empty = "Nothing has run yet." }) {
  const [data, setData] = useState(null);
  const [status, setStatus] = useState("");
  const [open, setOpen] = useState(null);
  useEffect(() => {
    api
      .get("/api/jobs/runs" + qs({ kind, job, status: status || undefined, limit: 100 }))
      .then(setData)
      .catch((err) => {
        toast.error(err);
        setData({ items: [], total: 0, ok: 0, errors: 0 });
      });
  }, [kind, job, status, refreshKey]);
  if (!data) return html`<${Skeleton} height=${120} />`;
  return html`<div class="stack">
    <div class="row wrap">
      <span class="small muted">${data.total} run${data.total === 1 ? "" : "s"} · ${data.ok} succeeded · ${data.errors} failed</span>
      <span class="spacer"></span>
      <${Segmented} value=${status} onChange=${setStatus} options=${[{ value: "", label: "All" }, { value: "ok", label: "Succeeded" }, { value: "error", label: "Failed" }]} />
    </div>
    ${data.items.length === 0
      ? html`<${Empty} icon="clock" title="No runs">${empty}</${Empty}>`
      : html`<div class="table-wrap"><table class="table runs-table">
          <thead><tr><th>Started</th><th>Job</th><th>Trigger</th><th>Result</th><th class="num">Took</th><th>Details</th></tr></thead>
          <tbody>
            ${data.items.map(
              (r) => html`<tr key=${r.id} class=${cx({ "row-error": r.status === "error" })} onClick=${() => setOpen(open === r.id ? null : r.id)}>
                <td title=${formatDate(r.started)}><span class="mono small">${formatDate(r.started)}</span><div class="tiny muted">${relativeTime(r.started)}</div></td>
                <td class="mono small">${r.kind === "backup" ? (r.job === "auto" ? "automatic backup" : "backup") : r.job}</td>
                <td><span class="chip outline">${r.trigger === "schedule" ? "scheduled" : r.trigger}</span></td>
                <td><${StatusChip} status=${r.status} /></td>
                <td class="num mono small">${ms(r.durationMs)}</td>
                <td class="small">
                  ${r.meta && r.meta.key && html`<span class="mono">${r.meta.key}</span> · ${bytes(r.meta.size || 0)} ${r.meta.encrypted ? html`· <${Icon} name="lock" size=${12} /> encrypted` : r.meta.encrypted === false ? "· not encrypted" : ""}`}
                  ${r.status === "error" && html`<div class=${cx("run-detail", { open: open === r.id })}>${r.detail}</div>`}
                </td>
              </tr>`
            )}
          </tbody>
        </table></div>`}
  </div>`;
}

// When the automatic backup runs next, on the clocks of the saved time zone (and in UTC, which
// every date in the API is in)
function NextRun({ cron, zone }) {
  const [job, setJob] = useState(null);
  useEffect(() => {
    api
      .get("/api/crons")
      .then((jobs) => setJob(jobs.find((j) => j.id === "__pbAutoBackup__") || null))
      .catch(() => setJob(null));
  }, [cron, zone]);
  if (!cron || !job || !job.next) return null;
  const next = job.next;
  const at = job.timezone || "UTC"; // the server's answer: "system" is already the OS's zone
  return html`<span class="small muted row wrap tight"><${Icon} name="clock" size=${13} /> Next: <b>${formatDateIn(next, at)}</b> (${relativeTime(next)})${at !== "UTC" && html` <span class="tiny" title="UTC, which every date in the API is in">= ${formatDateIn(next, "UTC")}</span>`}</span>`;
}

// The time zone the schedule is read in (backups and the cron jobs of hooks): 03:00 means 03:00
// on its clocks. UTC until changed, as in PocketBase.
function TimeZoneField({ s }) {
  const zone = s.draft.schedule.timezone;
  const mine = browserTimeZone();
  const names = [{ value: "UTC", label: "UTC (default)" }, { value: "system", label: "System: the server's own time zone" }, ...timeZones().filter((n) => n !== "UTC")];
  if (!names.some((o) => o.value === zone)) names.push(zone);
  return html`<div class="span-all">
    <${Field} label="Time zone" error=${s.errors["schedule.timezone"]} help="The schedule above, and the cron jobs of your hooks, are read on this zone's clocks. UTC is the default; System follows the time zone the server's operating system is set to (a container is usually UTC), so daylight saving needs no handling here.">
      <div class="row wrap tight">
        <${Select} class="tz-select" aria-label="Time zone" value=${zone} onChange=${(v) => s.set("schedule.timezone", v)} options=${names} />
        ${zone !== mine && html`<button type="button" class="chip selectable outline" onClick=${() => s.set("schedule.timezone", mine)}>Use this device's: ${mine}</button>`}
        ${zone !== "UTC" && html`<button type="button" class="chip selectable outline" onClick=${() => s.set("schedule.timezone", "UTC")}>UTC</button>`}
      </div>
    </${Field}>
  </div>`;
}

export function Backups({ s }) {
  const [list, setList] = useState(null);
  const [busy, setBusy] = useState("");
  const [canBackup, setCanBackup] = useState(true);
  const [filter, setFilter] = useState("all");
  const [tab, setTab] = useState("files");
  const [refresh, setRefresh] = useState(0);
  const load = () =>
    api
      .get("/api/backups")
      .then((l) => setList(l.sort((a, b) => (a.modified < b.modified ? 1 : -1))))
      .catch((err) => {
        toast.error(err);
        setList([]);
      });
  useEffect(() => {
    load();
    api.get("/api/health").then((h) => setCanBackup(h.data.canBackup !== false)).catch(() => {});
  }, []);
  const after = () => {
    load();
    setRefresh((n) => n + 1);
  };
  const create = async () => {
    setBusy("create");
    try {
      await api.post("/api/backups", {});
      toast("Backup created.");
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy("");
      after();
    }
  };
  const upload = async (file) => {
    if (!file) return;
    const form = new FormData();
    form.append("file", file);
    setBusy("upload");
    try {
      await api.post("/api/backups/upload", form);
      toast("Backup uploaded.");
      load();
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy("");
    }
  };
  const download = async (key) => {
    try {
      const token = await getFileToken();
      window.open(`${BASE}/api/backups/${enc(key)}${qs({ token })}`, "_blank");
    } catch (err) {
      toast.error(err);
    }
  };
  const restore = async (key) => {
    const ok = await confirmDialog({
      title: "Restore this backup?",
      message: "Everything — collections, records, files, settings and images — is replaced with the backup. Everyone is signed out.",
      confirm: "Restore",
      danger: true,
      typed: "restore",
    });
    if (!ok) return;
    setBusy(key);
    try {
      await api.post(`/api/backups/${enc(key)}/restore`);
      toast("Backup restored. Sign in again.", "info");
      setTimeout(() => location.reload(), 1200);
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy("");
    }
  };
  const remove = async (key) => {
    if (!(await confirmDialog({ title: `Delete ${key}?`, confirm: "Delete", danger: true }))) return;
    try {
      await api.delete(`/api/backups/${enc(key)}`);
      toast("Backup deleted.");
      load();
    } catch (err) {
      toast.error(err);
    }
  };
  const d = s.draft;
  const shown = (list || []).filter((b) => filter === "all" || (filter === "auto" ? b.auto : !b.auto));
  const total = (list || []).reduce((n, b) => n + b.size, 0);
  const unencrypted = (list || []).filter((b) => b.encrypted === false).length;
  return html`<div class="stack loose">
    <${Card}
      title="Backups"
      subtitle=${list ? `${list.length} · ${bytes(total)} · the database, files and images in one zip` : "The database, files and images in one zip"}
      icon="archive"
      actions=${html`<div class="row">
        <label class="btn sm"><${Icon} name="upload" size=${15} /><span>${busy === "upload" ? "Uploading…" : "Upload"}</span><input type="file" accept=".zip" hidden onChange=${(e) => upload(e.target.files[0])} /></label>
        <${Button} size="sm" variant="primary" icon="plus" loading=${busy === "create"} disabled=${!canBackup} onClick=${create}>Create backup</${Button}>
      </div>`}
    >
      <div class="row wrap">
        <${Segmented} value=${tab} onChange=${setTab} options=${[{ value: "files", label: "Backups" }, { value: "history", label: "History" }]} />
        <span class="spacer"></span>
        ${tab === "files" && html`<${Segmented} value=${filter} onChange=${setFilter} options=${[{ value: "all", label: "All" }, { value: "auto", label: "Automatic" }, { value: "manual", label: "Manual" }]} />`}
      </div>
      ${tab === "history"
        ? html`<${JobHistory} kind="backup" refreshKey=${refresh} empty="Automatic and manual backups show up here once they run." />`
        : list === null
        ? html`<${Skeleton} height=${48} />`
        : shown.length === 0
        ? html`<${Empty} icon="archive" title="No backups here">Create one now, or schedule them below.</${Empty}>`
        : html`<div class="stack tight">
            ${unencrypted > 0 &&
            html`<div class="small muted row"><${Icon} name="unlock" size=${14} /> ${unencrypted} not encrypted: set an encryption key (<code>SLONBASE_BACKUPS_ENCRYPTION_KEY</code>) to encrypt new ones.</div>`}
            ${shown.map(
              (b, i) => html`<div class="backup-row" style=${{ "--i": i }}>
                <span class=${cx("backup-lock", { on: b.encrypted })} title=${b.encrypted ? "Encrypted" : b.encrypted === false ? "Not encrypted" : "Unknown"}>
                  <${Icon} name=${b.encrypted ? "lock" : "unlock"} size=${15} />
                </span>
                <div class="grow">
                  <div class="row tight"><span class="mono small ellipsis">${b.key}</span>${b.auto && html`<span class="chip outline">automatic</span>`}${b.encrypted ? html`<span class="chip ok">encrypted</span>` : b.encrypted === false ? html`<span class="chip warn">not encrypted</span>` : null}</div>
                  <div class="tiny muted">${bytes(b.size)} · ${formatDate(b.modified)} · ${relativeTime(b.modified)}</div>
                </div>
                <${IconButton} icon="download" label="Download" onClick=${() => download(b.key)} />
                <${Button} size="sm" icon="undo" loading=${busy === b.key} onClick=${() => restore(b.key)}>Restore</${Button}>
                <${IconButton} icon="trash" label="Delete" onClick=${() => remove(b.key)} />
              </div>`
            )}
          </div>`}
    </${Card}>
    <${Card} title="Schedule" subtitle=${`Automatic backups (cron syntax, on ${d.schedule.timezone === "system" ? "the server's" : d.schedule.timezone} time)`} icon="clock" actions=${html`<${NextRun} cron=${s.saved.backups.cron} zone=${s.saved.schedule.timezone} />`}>
      <div class="grid-2">
        <${Field} label="Schedule" error=${s.errors["backups.cron"]} help=${`e.g. 0 3 * * * for every day at 03:00 ${d.schedule.timezone === "UTC" ? "UTC" : "(" + d.schedule.timezone + ")"} — empty turns it off`}>
          <${Input} class="mono" value=${d.backups.cron} onInput=${(v) => s.set("backups.cron", v)} placeholder="0 3 * * *" />
        </${Field}>
        <${Field} label="Keep" help="Older automatic backups are deleted"><${Input} type="number" min="1" value=${d.backups.cronMaxKeep} onInput=${(v) => s.set("backups.cronMaxKeep", Number(v))} /></${Field}>
        <${TimeZoneField} s=${s} />
      </div>
      <div class="chips">
        ${[
          ["Daily 03:00", "0 3 * * *"],
          ["Every 6 hours", "0 */6 * * *"],
          ["Weekly", "0 3 * * 0"],
          ["Off", ""],
        ].map(([label, cron]) => html`<button type="button" class=${"chip selectable " + (d.backups.cron === cron ? "accent" : "outline")} onClick=${() => s.set("backups.cron", cron)}>${label}</button>`)}
      </div>
    </${Card}>
    <${Card} title="Store backups in S3" icon="cloud" actions=${html`<${Switch} checked=${d.backups.s3.enabled} onChange=${(v) => s.set("backups.s3.enabled", v)} label=${d.backups.s3.enabled ? "On" : "Off"} />`}>
      ${d.backups.s3.enabled && html`<${S3Fields} s=${s} path="backups.s3" errors=${s.errors} /><div class="row"><span class="spacer"></span><${TestS3} filesystem="backups" disabled=${s.dirty} /></div>`}
    </${Card}>
  </div>`;
}

export function Crons() {
  const [jobs, setJobs] = useState(null);
  const [running, setRunning] = useState("");
  const [selected, setSelected] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [zone, setZone] = useState("UTC");
  const load = () =>
    api
      .get("/api/crons")
      .then(setJobs)
      .catch(() => setJobs([]));
  useEffect(() => {
    load();
    api
      .get("/api/settings")
      .then((st) => setZone((st.schedule && st.schedule.timezone) || "UTC"))
      .catch(() => {});
  }, [refresh]);
  const run = async (j) => {
    setRunning(j.id);
    try {
      await api.post(`/api/crons/${enc(j.id)}`);
      toast(`Started ${j.id}.`);
      // the run is recorded in the background
      setTimeout(() => setRefresh((n) => n + 1), 1200);
    } catch (err) {
      toast.error(err);
    } finally {
      setRunning("");
    }
  };
  const label = (j) => (j.id === "__pbAutoBackup__" ? "Automatic backup" : j.id);
  const historyOf = jobs && jobs.find((j) => j.id === selected);
  return html`<div class="stack loose">
    <${Card} title="Cron jobs" subtitle=${`Scheduled jobs from hooks (cronAdd) and backups, on ${(jobs && jobs[0] && jobs[0].timezone) || zone} time`} icon="clock" actions=${html`<a class="small" href="#/settings/backups">Time zone</a><${IconButton} icon="refresh" label="Refresh" onClick=${() => setRefresh((n) => n + 1)} />`}>
      ${jobs === null
        ? html`<${Skeleton} height=${80} />`
        : jobs.length === 0
        ? html`<${Empty} icon="clock" title="No cron jobs">Add them with <code>cronAdd()</code> in a hook file, or schedule backups.</${Empty}>`
        : html`<div class="table-wrap"><table class="table crons-table">
            <thead><tr><th>Job</th><th>Schedule</th><th>Next run</th><th>Last run</th><th></th></tr></thead>
            <tbody>
              ${jobs.map(
                (j) => html`<tr key=${j.id} class=${cx({ on: selected === j.id })}>
                  <td><b class="mono small">${label(j)}</b><div class="tiny muted">${j.kind === "backup" ? "Settings → Backups" : "hook"}</div></td>
                  <td><code>${j.expression}</code>${j.description && html`<div class="tiny muted">${j.description}</div>`}</td>
                  <td>${j.next ? html`<span class="small">${relativeTime(j.next)}</span><div class="tiny muted mono" title=${`${formatDateIn(j.next, "UTC")}`}>${formatDateIn(j.next, j.timezone || zone)}</div>` : html`<span class="muted">never</span>`}</td>
                  <td>${j.lastRun ? html`<${StatusChip} status=${j.lastRun.status} /><div class="tiny muted">${relativeTime(j.lastRun.started)} · ${ms(j.lastRun.durationMs)}</div>` : html`<span class="muted small">not yet</span>`}</td>
                  <td class="row tight end">
                    <${Button} size="sm" variant="ghost" icon="list" onClick=${() => setSelected(selected === j.id ? "" : j.id)}>History</${Button}>
                    <${Button} size="sm" icon="play" loading=${running === j.id} onClick=${() => run(j)}>Run now</${Button}>
                  </td>
                </tr>`
              )}
            </tbody>
          </table></div>`}
      <div class="small muted row"><${Icon} name="info" size=${14} /> Each job runs once per minute it's due, on one instance. Old logs are deleted every minute, too (Settings → Logs).</div>
    </${Card}>
    <${Card} title=${historyOf ? `History: ${label(historyOf)}` : "History"} subtitle=${historyOf ? "Its last runs" : "Every job's last runs; choose one above to see only its own"} icon="list">
      <${JobHistory}
        kind=${historyOf ? (historyOf.kind === "backup" ? "backup" : "cron") : undefined}
        job=${historyOf ? (historyOf.kind === "backup" ? "auto" : historyOf.id) : undefined}
        refreshKey=${refresh}
      />
    </${Card}>
  </div>`;
}
