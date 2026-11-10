// Settings → Backups and Cron jobs: what's there, what runs when, and what ran (the jobs'
// history, /api/jobs/runs).
import { html, useState, useEffect, useStore, cx } from "../../lib/core.js";
import { api, qs, enc, getFileToken, BASE } from "../../lib/api.js";
import { Icon } from "../../lib/icons.js";
import { Button, IconButton, Field, Input, Select, Switch, Skeleton, Empty, Segmented, Modal, SheetHead, Callout, toast, confirmDialog } from "../../lib/ui.js";
import { app } from "../../lib/state.js";
import { bytes, formatDate, formatDateIn, relativeTime, timeZones, browserTimeZone } from "../../lib/format.js";
import { Card } from "./shared.js";
import { S3Fields, TestS3 } from "./storage.js";
import { useMyRole } from "./team.js";

const ms = (n) => (n == null ? "—" : n < 1000 ? `${n} ms` : n < 60000 ? `${(n / 1000).toFixed(1)} s` : `${Math.round(n / 60000)} min`);

export function StatusChip({ status }) {
  const kind = status === "ok" ? "ok" : status === "error" ? "danger" : "warn";
  const label = status === "ok" ? "Succeeded" : status === "error" ? "Failed" : "Running";
  return html`<span class=${"chip " + kind}>${label}</span>`;
}

// the latest runs of a kind of job (or of one job)
// What a run did, in a few words, from the result a job type recorded
function runSummary(meta) {
  if (!meta) return "";
  const n = (v, one, many) => `${v} ${v === 1 ? one : many}`;
  if (meta.deleted != null) return `Deleted ${n(meta.deleted, "record", "records")}${meta.collection ? ` from ${meta.collection}` : ""}`;
  if (meta.sent != null) return `Sent ${n(meta.sent, "email", "emails")}`;
  if (meta.status != null) return `HTTP ${meta.status}`;
  if (meta.rows != null) return `${n(meta.rows, "row", "rows")}${meta.affectedRows ? `, ${meta.affectedRows} changed` : ""}`;
  return "";
}

export function JobHistory({ kind, job, refreshKey, names = {}, empty = "Nothing has run yet." }) {
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
                <td class="mono small">${r.kind === "backup" ? (r.job === "auto" ? "automatic backup" : "backup") : names[r.job] || r.job}</td>
                <td><span class="chip outline">${r.trigger === "schedule" ? "scheduled" : r.trigger}</span></td>
                <td><${StatusChip} status=${r.status} /></td>
                <td class="num mono small">${ms(r.durationMs)}</td>
                <td class="small">
                  ${runSummary(r.meta) && html`<span>${runSummary(r.meta)}</span>`}
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
  const at = job.timezone || "UTC"; // the server's answer
  return html`<span class="small muted row wrap tight"><${Icon} name="clock" size=${13} /> Next: <b>${formatDateIn(next, at)}</b> (${relativeTime(next)})${at !== "UTC" && html` <span class="tiny" title="UTC, which every date in the API is in">= ${formatDateIn(next, "UTC")}</span>`}</span>`;
}

// The time zone the schedule is read in (backups and the cron jobs of hooks): 03:00 means 03:00
// on its clocks. UTC until changed, as in PocketBase.
function TimeZoneField({ s }) {
  const zone = s.draft.schedule.timezone;
  const mine = browserTimeZone();
  const names = timeZones();
  if (!names.includes(zone)) names.push(zone);
  return html`<div class="span-all">
    <${Field} label="Time zone" error=${s.errors["schedule.timezone"]} help="The schedule above, and the cron jobs of your hooks, are read on this zone's clocks. UTC is the default; pick a zone, or use your device's, if the schedule should follow local clocks. Daylight saving is handled for you.">
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
    <${Card} title="Schedule" subtitle=${`Automatic backups (cron syntax, on ${d.schedule.timezone} time)`} icon="clock" actions=${html`<${NextRun} cron=${s.saved.backups.cron} zone=${s.saved.schedule.timezone} />`}>
      <div class="grid-2">
        <${Field} label="Schedule" error=${s.errors["backups.cron"]} help=${`e.g. 0 3 * * * for every day at 03:00 ${d.schedule.timezone === "UTC" ? "UTC" : "(" + d.schedule.timezone + ")"} — empty turns it off`}>
          <${Input} class="mono" value=${d.backups.cron} onInput=${(v) => s.set("backups.cron", v)} placeholder="0 3 * * *" />
        </${Field}>
        <${Field} label="Keep" help="How many automatic backups to keep: older ones are deleted (also when you lower this). Manual backups are never deleted."><${Input} type="number" min="1" value=${d.backups.cronMaxKeep} onInput=${(v) => s.set("backups.cronMaxKeep", Number(v))} /></${Field}>
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


// ---------------------------------------------------------------------------------------------
// Cron jobs made here (not in a hook file): a schedule and a type of job with its settings.

const SCHEDULES = [
  ["Every 5 minutes", "*/5 * * * *"],
  ["Hourly", "0 * * * *"],
  ["Daily at 03:00", "0 3 * * *"],
  ["Weekly (Sunday 03:00)", "0 3 * * 0"],
];

const JOB_TYPES = [
  { value: "cleanup", label: "Clean up records" },
  { value: "http", label: "Call a URL" },
  { value: "mail", label: "Send an email" },
  { value: "sql", label: "Run SQL" },
];
const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];
// Ready-made starting points for a new job: they fill the form, nothing is saved until "Make job"
const TEMPLATES = [
  { id: "cleanup", label: "Delete expired records", fill: { name: "Delete old records", type: "cleanup", expression: "0 3 * * *", days: "30", field: "created", filter: "", maxRows: "10000" } },
  { id: "ping", label: "Ping a URL every 5 minutes", fill: { name: "Ping a URL", type: "http", expression: "*/5 * * * *", method: "GET", url: "https://example.com/health", retries: "2", retryDelay: "30" } },
  { id: "mail", label: "Weekly email reminder", fill: { name: "Weekly reminder", type: "mail", expression: "0 8 * * 1", subject: "Weekly reminder", mailBody: "A reminder for this week." } },
  { id: "sql", label: "Refresh planner statistics", owner: true, fill: { name: "Refresh statistics", type: "sql", expression: "0 3 * * 0", statement: "ANALYZE" } },
];
// headers are edited as lines of "Name: value"; saved values come back masked and stay as they are
const headerLines = (h) => Object.entries(h || {}).map(([k, v]) => `${k}: ${v}`).join("\n");
const headerObject = (text) => {
  const out = {};
  for (const line of text.split("\n")) {
    const i = line.indexOf(":");
    if (i > 0) out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return out;
};

function JobEditor({ job, copyOf, onClose, onDone }) {
  const editing = !!job && !copyOf; // a copy starts from a job but makes a new one
  const collections = useStore(app, (st) => st.collections) || [];
  const choices = collections.filter((c) => !c.system && !String(c.name).startsWith("_") && c.type !== "view");
  const c = (job && job.config) || {};
  const [name, setName] = useState(job ? (copyOf ? `Copy of ${job.name}`.slice(0, 60) : job.name) : "");
  const [expression, setExpression] = useState(job ? job.expression : "0 3 * * *");
  const [type, setType] = useState(job ? job.type : "cleanup");
  const [collection, setCollection] = useState(c.collection || (choices[0] && choices[0].name) || "");
  const [filter, setFilter] = useState(c.filter || "");
  const [days, setDays] = useState(c.olderThanDays ? String(c.olderThanDays) : "");
  const [field, setField] = useState(c.field || "created");
  const [maxRows, setMaxRows] = useState(c.maxRows ? String(c.maxRows) : "10000");
  const [url, setUrl] = useState(c.url || "");
  const [method, setMethod] = useState(c.method || "GET");
  const [headers, setHeaders] = useState(headerLines(c.headers));
  const [payload, setPayload] = useState(c.body || "");
  const [timeout, setTimeoutSeconds] = useState(c.timeoutSeconds ? String(c.timeoutSeconds) : "30");
  const [expect, setExpect] = useState(c.expectStatus ? String(c.expectStatus) : "");
  const [mailTo, setMailTo] = useState(Array.isArray(c.to) ? c.to.join(", ") : c.to || "");
  const [subject, setSubject] = useState(c.subject || "");
  const [mailBody, setMailBody] = useState(c.body || "");
  const [statement, setStatement] = useState(c.statement || "");
  const [sqlOn, setSqlOn] = useState(null);
  const role = useMyRole();
  // SQL jobs are for owners, and only when the server runs the SQL console
  useEffect(() => {
    if (role === "owner") api.get("/api/sql").then((i) => setSqlOn(!!i.enabled)).catch(() => setSqlOn(false));
  }, [role]);
  const types = JOB_TYPES.filter((t) => t.value !== "sql" || role === "owner");
  const templates = TEMPLATES.filter((t) => !t.owner || (role === "owner" && sqlOn));
  const start = (t) => {
    const f = t.fill;
    setName(f.name);
    setType(f.type);
    setExpression(f.expression);
    if (f.days != null) {
      setDays(f.days);
      setField(f.field);
      setFilter(f.filter);
      setMaxRows(f.maxRows);
    }
    if (f.url != null) {
      setMethod(f.method);
      setUrl(f.url);
      setHeaders("");
      setPayload("");
    }
    // every example starts from the same defaults, so one doesn't leave its settings in the next
    setRetries(f.retries || "0");
    setRetryDelay(f.retryDelay || "30");
    if (f.subject != null) {
      setSubject(f.subject);
      setMailBody(f.mailBody);
      setMailTo("");
    }
    if (f.statement != null) setStatement(f.statement);
  };
  const [retries, setRetries] = useState(job ? String(job.retries || 0) : "0");
  const [retryDelay, setRetryDelay] = useState(job && job.retries ? String(job.retryDelaySeconds) : "30");
  const [notifyEmails, setNotifyEmails] = useState(job && job.notify ? job.notify.emails.join(", ") : "");
  const [notifyHook, setNotifyHook] = useState(job && job.notify ? job.notify.webhook : "");
  const [enabled, setEnabled] = useState(job ? job.enabled : true);
  const [errors, setErrors] = useState({});
  const [preview, setPreview] = useState(null);
  const [busy, setBusy] = useState(false);
  const body = () => ({
    id: editing ? job.id : undefined,
    copyOf: copyOf || undefined,
    name,
    expression,
    type,
    enabled,
    notify: { emails: notifyEmails, webhook: notifyHook.trim() },
    retries: Number(retries),
    retryDelaySeconds: retryDelay.trim() ? Number(retryDelay) : null,
    config:
      type === "mail"
        ? { to: mailTo, subject, body: mailBody }
        : type === "sql"
        ? { statement }
        : type === "http"
        ? { url, method, headers: headerObject(headers), body: payload, timeoutSeconds: timeout.trim() ? Number(timeout) : null, expectStatus: expect.trim() ? Number(expect) : null }
        : { collection, filter, field, olderThanDays: days.trim() ? Number(days) : null, maxRows: maxRows.trim() ? Number(maxRows) : null },
  });
  // what it would do, as it is being made (nothing is saved or run)
  useEffect(() => {
    const t = setTimeout(async () => {
      try {
        setPreview(await api.post("/api/cron-preview", body()));
        setErrors({});
      } catch (err) {
        setPreview(null);
        setErrors(err.fieldErrors ? err.fieldErrors() : {});
      }
    }, 450);
    return () => clearTimeout(t);
  }, [name, expression, type, collection, filter, days, field, maxRows, url, method, headers, payload, timeout, expect, mailTo, subject, mailBody, statement, retries, retryDelay, notifyEmails, notifyHook]);
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      if (editing) await api.patch(`/api/crons/${enc(job.id)}`, body());
      else await api.post("/api/crons", body());
      toast(editing ? "Saved." : "Job made.");
      onDone();
      onClose();
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<${Modal} onClose=${onClose} size="mid">
    <${SheetHead} title=${editing ? "Edit cron job" : copyOf ? "Copy cron job" : "New cron job"} subtitle="Runs on a schedule, on one instance, and shows in the history below" onClose=${onClose} />
    <form class="sheet-body stack" onSubmit=${submit}>
      ${!job && html`<div class="stack tight"><span class="small muted">Start from an example</span><div class="chips">${templates.map((t) => html`<button type="button" class="chip selectable outline" onClick=${() => start(t)}>${t.label}</button>`)}</div></div>`}
      <${Field} label="Name" error=${errors.name} help="What it's for, e.g. “Delete old sessions”"><${Input} value=${name} onInput=${setName} required autofocus maxlength="60" /></${Field}>
      <${Field} label="Type"><${Segmented} value=${type} onChange=${setType} options=${types} /></${Field}>
      <${Field} label="Schedule" error=${errors.expression} help=${preview ? `${preview.description || "Custom schedule"}, on ${preview.timezone} time` : "minute hour day month weekday"}>
        <${Input} class="mono" value=${expression} onInput=${setExpression} required />
      </${Field}>
      <div class="chips">
        ${SCHEDULES.map(([label, value]) => html`<button type="button" class=${"chip selectable " + (expression === value ? "accent" : "outline")} onClick=${() => setExpression(value)}>${label}</button>`)}
      </div>
      ${type === "cleanup" &&
      html`<div class="stack">
        <${Field} label="Collection" error=${errors["config.collection"]} help="Its records are deleted, with their files and anything that cascades, as when you delete them yourself">
          <${Select} value=${collection} onChange=${setCollection} options=${choices.map((x) => ({ value: x.name, label: x.name }))} />
        </${Field}>
        <div class="grid-2">
          <${Field} label="Older than (days)" error=${errors["config.olderThanDays"]} help="Leave empty to go by the filter only"><${Input} type="number" min="1" value=${days} onInput=${setDays} /></${Field}>
          <${Field} label="Age is read from" error=${errors["config.field"]} help="The collection needs that date field"><${Select} value=${field} onChange=${setField} options=${["created", "updated"]} /></${Field}>
        </div>
        <${Field} label="Filter" error=${errors["config.filter"]} help="Same syntax as API rules, e.g. status = 'expired'. Both are applied when you give both."><${Input} class="mono" value=${filter} onInput=${setFilter} placeholder="status = 'expired'" /></${Field}>
        <${Field} label="At most this many records per run" error=${errors["config.maxRows"]}><${Input} type="number" min="1" max="100000" value=${maxRows} onInput=${setMaxRows} /></${Field}>
        ${preview && preview.matching != null && html`<${Callout} kind=${preview.matching ? "warn" : "info"}>${preview.matching === 0 ? "No record matches now." : `${preview.matching} record${preview.matching === 1 ? "" : "s"} match now and would be deleted at the next run (at most ${maxRows || "10000"} per run).`}</${Callout}>`}
      </div>`}
      ${type === "http" &&
      html`<div class="stack">
        <div class="grid-2">
          <${Field} label="Method"><${Select} value=${method} onChange=${setMethod} options=${METHODS} /></${Field}>
          <${Field} label="URL" error=${errors["config.url"]} help="Public addresses only: this server and private networks can't be called"><${Input} class="mono" value=${url} onInput=${setUrl} placeholder="https://example.com/hook" /></${Field}>
        </div>
        <${Field} label="Headers" error=${errors["config.headers"]} help="One per line, Name: value. Saved values are hidden; leave a line as it is to keep it."><textarea class="input mono" rows="3" value=${headers} onInput=${(e) => setHeaders(e.target.value)} placeholder="Authorization: Bearer …"></textarea></${Field}>
        ${method !== "GET" && html`<${Field} label="Body" error=${errors["config.body"]}><textarea class="input mono" rows="3" value=${payload} onInput=${(e) => setPayload(e.target.value)} placeholder='{"ping": true}'></textarea></${Field}>`}
        <div class="grid-2">
          <${Field} label="Time limit (seconds)" error=${errors["config.timeoutSeconds"]}><${Input} type="number" min="1" max="60" value=${timeout} onInput=${setTimeoutSeconds} /></${Field}>
          <${Field} label="Counts as success" error=${errors["config.expectStatus"]} help="A status code; empty means any 2xx"><${Input} type="number" min="100" max="599" value=${expect} onInput=${setExpect} placeholder="2xx" /></${Field}>
        </div>
      </div>`}
      ${type === "mail" &&
      html`<div class="stack">
        <${Field} label="To" error=${errors["config.to"]} help="Up to 10 addresses, separated by commas. Sent through Settings → Mail."><${Input} value=${mailTo} onInput=${setMailTo} placeholder="ops@example.com, dev@example.com" /></${Field}>
        <${Field} label="Subject" error=${errors["config.subject"]}><${Input} value=${subject} onInput=${setSubject} maxlength="200" /></${Field}>
        <${Field} label="Message" error=${errors["config.body"]} help="Plain text"><textarea class="input" rows="5" value=${mailBody} onInput=${(e) => setMailBody(e.target.value)}></textarea></${Field}>
      </div>`}
      ${type === "sql" &&
      html`<div class="stack">
        ${sqlOn === false && html`<${Callout} kind="warn">The SQL console is off on this server. Start slonbase with <code>--sql-console</code> to save SQL jobs.</${Callout}>`}
        <${Field} label="Statement" error=${errors["config.statement"]} help="Runs as the SQL console does: with its role and time limits, and it can only change data when the console allows writes. Only owners can make these."><textarea class="input mono" rows="6" value=${statement} onInput=${(e) => setStatement(e.target.value)} placeholder="DELETE FROM slonbase.sessions WHERE expires < now()"></textarea></${Field}>
      </div>`}
      <div class="grid-2">
        <${Field} label="If it fails, try again" error=${errors.retries} help="Waits longer each time"><${Select} value=${retries} onChange=${setRetries} options=${[0, 1, 2, 3, 4, 5].map((n) => ({ value: String(n), label: n === 0 ? "Don't retry" : `${n} time${n === 1 ? "" : "s"}` }))} /></${Field}>
        ${Number(retries) > 0 && html`<${Field} label="First retry after (seconds)" error=${errors.retryDelaySeconds} help="Then twice as long, at most 5 minutes"><${Input} type="number" min="1" max="300" value=${retryDelay} onInput=${setRetryDelay} /></${Field}>`}
      </div>
      <${Field} label="Tell me when it fails (email)" error=${errors["notify.emails"]} help="Up to 10 addresses, through Settings → Mail. One message when it starts failing, one when it works again."><${Input} value=${notifyEmails} onInput=${setNotifyEmails} placeholder="ops@example.com" /></${Field}>
      <${Field} label="Tell me when it fails (webhook)" error=${errors["notify.webhook"]} help="A public URL that gets a JSON POST (event, job, error). Saved URLs are hidden; leave it as it is to keep it."><${Input} class="mono" value=${notifyHook} onInput=${setNotifyHook} placeholder="https://hooks.example.com/…" /></${Field}>
      ${preview && preview.next && html`<div class="small muted">Next runs: ${preview.next.slice(0, 3).map((t) => formatDateIn(t, preview.timezone)).join(" · ")}</div>`}
      <div class="row"><${Switch} checked=${enabled} onChange=${setEnabled} label=${enabled ? "On" : "Off: it won't run"} /><span class="spacer"></span>
        <${Button} variant="ghost" onClick=${onClose}>Cancel</${Button}><${Button} variant="primary" type="submit" loading=${busy}>${editing ? "Save" : "Make job"}</${Button}></div>
    </form>
  </${Modal}>`;
}

export function Crons() {
  const [jobs, setJobs] = useState(null);
  const [running, setRunning] = useState("");
  const [selected, setSelected] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [zone, setZone] = useState("UTC");
  const [paused, setPaused] = useState(false);
  const [editing, setEditing] = useState(null);
  const role = useMyRole();
  const canMake = role === "admin" || role === "owner";
  const load = () =>
    api
      .get("/api/crons")
      .then(setJobs)
      .catch(() => setJobs([]));
  useEffect(() => {
    load();
    api
      .get("/api/settings")
      .then((st) => {
        setZone((st.schedule && st.schedule.timezone) || "UTC");
        setPaused(!!(st.schedule && st.schedule.pauseJobs));
      })
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
  // stops every job made here from running on its schedule (a maintenance window)
  const pause = async (on) => {
    try {
      await api.patch("/api/settings", { schedule: { pauseJobs: on } });
      setPaused(on);
      toast(on ? "Jobs paused." : "Jobs resumed.");
    } catch (err) {
      toast.error(err);
    }
  };
  const toggle = async (j, on) => {
    try {
      await api.patch(`/api/crons/${enc(j.id)}`, { enabled: on });
      setRefresh((n) => n + 1);
    } catch (err) {
      toast.error(err);
      setRefresh((n) => n + 1);
    }
  };
  const remove = async (j) => {
    if (!(await confirmDialog({ title: `Delete “${j.name}”?`, message: "It stops running. Its history stays until it ages out.", confirm: "Delete", danger: true }))) return;
    try {
      await api.delete(`/api/crons/${enc(j.id)}`);
      toast("Deleted.");
      if (selected === j.id) setSelected("");
      setRefresh((n) => n + 1);
    } catch (err) {
      toast.error(err);
    }
  };
  const label = (j) => (j.id === "__pbAutoBackup__" ? "Automatic backup" : j.kind === "user" ? j.name : j.id);
  const historyOf = jobs && jobs.find((j) => j.id === selected);
  return html`<div class="stack loose">
    <${Card} title="Cron jobs" subtitle=${`Jobs made here, in hooks (cronAdd) and the backup, on ${(jobs && jobs[0] && jobs[0].timezone) || zone} time`} icon="clock" actions=${html`${canMake && html`<${Switch} checked=${paused} onChange=${pause} label="Pause all" />`}${canMake && html`<${Button} size="sm" variant="primary" icon="plus" onClick=${() => setEditing({})}>New job</${Button}>`}<a class="small" href="#/settings/backups">Time zone</a><${IconButton} icon="refresh" label="Refresh" onClick=${() => setRefresh((n) => n + 1)} />`}>
      ${paused && html`<${Callout} kind="warn">Jobs made here are paused: none runs on its schedule until you turn this off. Run now still works. Jobs from hook files and the automatic backup are not affected.</${Callout}>`}
      ${jobs === null
        ? html`<${Skeleton} height=${80} />`
        : jobs.length === 0
        ? html`<${Empty} icon="clock" title="No cron jobs">${canMake ? "Make one with New job, " : ""}add them with <code>cronAdd()</code> in a hook file, or schedule backups.</${Empty}>`
        : html`<div class="table-wrap"><table class="table crons-table">
            <thead><tr><th>Job</th><th>Schedule</th><th>Next run</th><th>Last run</th><th></th></tr></thead>
            <tbody>
              ${jobs.map(
                (j) => html`<tr key=${j.id} class=${cx({ on: selected === j.id })}>
                  <td><b class=${j.kind === "user" ? "small" : "mono small"}>${label(j)}</b><div class="tiny muted">${j.kind === "backup" ? "Settings → Backups" : j.kind === "user" ? (j.enabled ? "made here" : "made here · off") : "hook"}</div></td>
                  <td><code>${j.expression}</code>${j.description && html`<div class="tiny muted">${j.description}</div>`}</td>
                  <td>${j.next ? html`<span class="small">${relativeTime(j.next)}</span><div class="tiny muted mono" title=${`${formatDateIn(j.next, "UTC")}`}>${formatDateIn(j.next, j.timezone || zone)}</div>` : html`<span class="muted">never</span>`}</td>
                  <td>${j.lastRun ? html`<${StatusChip} status=${j.lastRun.status} /><div class="tiny muted">${relativeTime(j.lastRun.started)} · ${ms(j.lastRun.durationMs)}</div>` : html`<span class="muted small">not yet</span>`}</td>
                  <td class="row tight end">
                    <${IconButton} icon="list" label="History" size="sm" onClick=${() => setSelected(selected === j.id ? "" : j.id)} />
                    ${j.kind === "user" && canMake && (j.type !== "sql" || role === "owner") && html`<${Switch} checked=${j.enabled} onChange=${(on) => toggle(j, on)} ariaLabel=${`${j.name} is ${j.enabled ? "on" : "off"}`} /><${IconButton} icon="copy" label="Duplicate" size="sm" onClick=${() => setEditing({ ...j, copyOf: j.id })} /><${IconButton} icon="edit" label="Edit" size="sm" onClick=${() => setEditing(j)} /><${IconButton} icon="trash" label="Delete" size="sm" onClick=${() => remove(j)} />`}
                    <${Button} size="sm" icon="play" loading=${running === j.id} onClick=${() => run(j)}>Run now</${Button}>
                  </td>
                </tr>`
              )}
            </tbody>
          </table></div>`}
      <div class="small muted row"><${Icon} name="info" size=${14} /> Each job runs once per minute it's due, on one instance. Old logs are deleted every minute, too (Settings → Logs).</div>
    </${Card}>
    ${editing && html`<${JobEditor} job=${editing.id ? editing : null} copyOf=${editing.copyOf || ""} onClose=${() => setEditing(null)} onDone=${() => setRefresh((n) => n + 1)} />`}
    <${Card} title=${historyOf ? `History: ${label(historyOf)}` : "History"} subtitle=${historyOf ? "Its last runs" : "Every job's last runs; choose one above to see only its own"} icon="list">
      <${JobHistory}
        kind=${historyOf ? (historyOf.kind === "backup" ? "backup" : "cron") : undefined}
        job=${historyOf ? (historyOf.kind === "backup" ? "auto" : historyOf.id) : undefined}
        refreshKey=${refresh}
        names=${Object.fromEntries((jobs || []).filter((j) => j.kind === "user").map((j) => [j.id, j.name]))}
      />
    </${Card}>
  </div>`;
}
