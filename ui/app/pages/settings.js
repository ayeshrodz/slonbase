// Settings: application, mail, file storage, backups, security, logs, crons, import/export.
import { html, useState, useEffect, useMemo, useStore, navigate, cx } from "../lib/core.js";
import { api, qs, enc, getFileToken, BASE } from "../lib/api.js";
import { app, loadCollections } from "../lib/state.js";
import { Icon } from "../lib/icons.js";
import {
  Button,
  IconButton,
  Field,
  Input,
  TextArea,
  Switch,
  Select,
  Modal,
  SheetHead,
  Skeleton,
  Empty,
  Callout,
  Segmented,
  toast,
  confirmDialog,
} from "../lib/ui.js";
import { bytes, formatDate, relativeTime, plural } from "../lib/format.js";

const SECTIONS = [
  { id: "general", label: "Application", icon: "sparkle" },
  { id: "mail", label: "Mail", icon: "mail" },
  { id: "storage", label: "File storage", icon: "cloud" },
  { id: "backups", label: "Backups", icon: "archive" },
  { id: "security", label: "Security & limits", icon: "shield" },
  { id: "logs", label: "Logs", icon: "logs" },
  { id: "crons", label: "Cron jobs", icon: "clock" },
  { id: "transfer", label: "Import & export", icon: "layers" },
];

function clone(v) {
  return JSON.parse(JSON.stringify(v));
}

function Card({ title, subtitle, icon, children, actions }) {
  return html`<div class="card enter">
    <div class="card-head">
      ${icon && html`<span class="field-type-badge"><${Icon} name=${icon} size=${16} /></span>`}
      <div class="grow"><h3>${title}</h3>${subtitle && html`<div class="small muted">${subtitle}</div>`}</div>
      ${actions}
    </div>
    ${[].concat(children).some(Boolean) && html`<div class="card-body stack">${children}</div>`}
  </div>`;
}

// settings sections share one draft and a save bar
function useSettings() {
  const [saved, setSaved] = useState(null);
  const [draft, setDraft] = useState(null);
  const [errors, setErrors] = useState({});
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    api
      .get("/api/settings")
      .then((s) => {
        setSaved(s);
        setDraft(clone(s));
      })
      .catch(toast.error);
  }, []);
  const dirty = useMemo(() => saved && JSON.stringify(saved) !== JSON.stringify(draft), [saved, draft]);
  const save = async () => {
    setSaving(true);
    setErrors({});
    try {
      const res = await api.patch("/api/settings", draft);
      setSaved(res);
      setDraft(clone(res));
      applyAccent(res.meta.accentColor);
      app.set({ hideControls: !!res.meta.hideControls });
      toast("Settings saved.");
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setSaving(false);
    }
  };
  const set = (path, value) => {
    setDraft((d) => {
      const next = clone(d);
      const keys = path.split(".");
      let obj = next;
      for (const k of keys.slice(0, -1)) obj = obj[k];
      obj[keys[keys.length - 1]] = value;
      return next;
    });
  };
  return { saved, draft, set, setDraft, dirty, save, saving, errors, reset: () => setDraft(clone(saved)) };
}

export function applyAccent(color) {
  // the default (and PocketBase's default, #1055c9, and 0.2's #4a5a72) keeps the theme's own accent,
  // which has a light and a dark variant
  if (!color || !/^#[0-9a-f]{6}$/i.test(color) || ["#1055c9", "#4a5a72", "#5062a6"].includes(color.toLowerCase())) {
    document.documentElement.style.removeProperty("--accent");
    return;
  }
  document.documentElement.style.setProperty("--accent", color);
}

// ------------------------------------------------------------------ sections

function General({ s }) {
  const d = s.draft;
  // muted tones that sit well on the neutral surfaces
  const swatches = ["#5062a6", "#3b8481", "#5a8750", "#a57a30", "#b0644b", "#a5537a", "#7a5ea8", "#4b7ca3"];
  return html`<${Card} title="Application" subtitle="Name and address used in emails and links" icon="sparkle">
    <div class="grid-2">
      <${Field} label="Application name" error=${s.errors["meta.appName"]}><${Input} value=${d.meta.appName} onInput=${(v) => s.set("meta.appName", v)} /></${Field}>
      <${Field} label="Application URL" error=${s.errors["meta.appURL"]} help="Links in emails point here"><${Input} value=${d.meta.appURL} onInput=${(v) => s.set("meta.appURL", v)} /></${Field}>
    </div>
    <${Field} label="Dashboard accent" help="Applies to this dashboard for every superuser">
      <div class="row wrap">
        ${swatches.map(
          (c) => html`<button
            type="button"
            aria-label=${c}
            class=${"swatch" + (d.meta.accentColor === c ? " on" : "")}
            style=${{ background: c }}
            onClick=${() => {
              s.set("meta.accentColor", c);
              applyAccent(c);
            }}
          ></button>`
        )}
        <input type="color" value=${d.meta.accentColor} onInput=${(e) => {
          s.set("meta.accentColor", e.target.value);
          applyAccent(e.target.value);
        }} style="width:36px;height:30px;border:0;background:none;cursor:pointer" aria-label="Custom color" />
      </div>
    </${Field}>
    <${Switch} checked=${d.meta.hideControls} onChange=${(v) => s.set("meta.hideControls", v)} label="Hide schema controls (lock collection editing in the dashboard)" />
  </${Card}>`;
}

function Mail({ s }) {
  const d = s.draft;
  const [testTo, setTestTo] = useState("");
  const [template, setTemplate] = useState("verification");
  const [sending, setSending] = useState(false);
  const test = async () => {
    setSending(true);
    try {
      await api.post("/api/settings/test/email", { email: testTo, template });
      toast(`Test email sent to ${testTo}.`);
    } catch (err) {
      toast.error(err);
    } finally {
      setSending(false);
    }
  };
  return html`<div class="stack loose">
    <${Card} title="Sender" icon="mail">
      <div class="grid-2">
        <${Field} label="Sender name"><${Input} value=${d.meta.senderName} onInput=${(v) => s.set("meta.senderName", v)} /></${Field}>
        <${Field} label="Sender address" error=${s.errors["meta.senderAddress"]}><${Input} type="email" value=${d.meta.senderAddress} onInput=${(v) => s.set("meta.senderAddress", v)} /></${Field}>
      </div>
    </${Card}>
    <${Card} title="SMTP server" subtitle=${d.smtp.enabled ? "Emails go through your server" : "Off: emails are written to the server log"} icon="server" actions=${html`<${Switch} checked=${d.smtp.enabled} onChange=${(v) => s.set("smtp.enabled", v)} label=${d.smtp.enabled ? "On" : "Off"} />`}>
      ${d.smtp.enabled &&
      html`<div class="stack enter">
        <div class="grid-3" style="grid-template-columns:2fr 1fr 1fr">
          <${Field} label="Host" error=${s.errors["smtp.host"]}><${Input} value=${d.smtp.host} onInput=${(v) => s.set("smtp.host", v)} /></${Field}>
          <${Field} label="Port" error=${s.errors["smtp.port"]}><${Input} type="number" value=${d.smtp.port} onInput=${(v) => s.set("smtp.port", Number(v))} /></${Field}>
          <${Field} label="Encryption">
            <${Select} value=${d.smtp.tls ? "tls" : "starttls"} onChange=${(v) => s.set("smtp.tls", v === "tls")} options=${[{ value: "starttls", label: "STARTTLS" }, { value: "tls", label: "TLS" }]} />
          </${Field}>
        </div>
        <div class="grid-2">
          <${Field} label="Username"><${Input} value=${d.smtp.username} onInput=${(v) => s.set("smtp.username", v)} autocomplete="off" /></${Field}>
          <${Field} label="Password" help="Leave empty to keep the saved one"><${Input} type="password" value=${d.smtp.password} onInput=${(v) => s.set("smtp.password", v)} autocomplete="new-password" placeholder="••••••••" /></${Field}>
        </div>
        <div class="grid-2">
          <${Field} label="Auth method"><${Select} value=${d.smtp.authMethod} onChange=${(v) => s.set("smtp.authMethod", v)} options=${[{ value: "", label: "Automatic" }, "PLAIN", "LOGIN"]} /></${Field}>
          <${Field} label="HELO name" help="Optional"><${Input} value=${d.smtp.localName} onInput=${(v) => s.set("smtp.localName", v)} /></${Field}>
        </div>
      </div>`}
    </${Card}>
    <${Card} title="Send a test email" icon="arrowRight">
      <div class="row wrap top">
        <${Field} class="grow" label="To"><${Input} type="email" value=${testTo} onInput=${setTestTo} placeholder="you@example.com" /></${Field}>
        <${Field} label="Template">
          <${Select} value=${template} onChange=${setTemplate} options=${[{ value: "verification", label: "Verification" }, { value: "password-reset", label: "Password reset" }, { value: "email-change", label: "Email change" }, { value: "otp", label: "One-time password" }, { value: "login-alert", label: "New device alert" }]} />
        </${Field}>
      </div>
      <div class="row"><span class="small muted grow">Uses the saved settings — save first.</span><${Button} icon="mail" loading=${sending} disabled=${!testTo || s.dirty} onClick=${test}>Send test</${Button}></div>
    </${Card}>
  </div>`;
}

function S3Fields({ s, path, errors }) {
  const v = path.split(".").reduce((o, k) => o[k], s.draft);
  return html`<div class="stack enter">
    <div class="grid-2">
      <${Field} label="Endpoint" error=${errors[path + ".endpoint"]} help="e.g. https://s3.eu-central-1.amazonaws.com"><${Input} value=${v.endpoint} onInput=${(x) => s.set(path + ".endpoint", x)} /></${Field}>
      <${Field} label="Bucket" error=${errors[path + ".bucket"]}><${Input} value=${v.bucket} onInput=${(x) => s.set(path + ".bucket", x)} /></${Field}>
      <${Field} label="Region" error=${errors[path + ".region"]}><${Input} value=${v.region} onInput=${(x) => s.set(path + ".region", x)} /></${Field}>
      <${Field} label="Access key" error=${errors[path + ".accessKey"]}><${Input} value=${v.accessKey} onInput=${(x) => s.set(path + ".accessKey", x)} autocomplete="off" /></${Field}>
      <${Field} label="Secret" help="Leave empty to keep the saved one"><${Input} type="password" value=${v.secret} onInput=${(x) => s.set(path + ".secret", x)} autocomplete="new-password" placeholder="••••••••" /></${Field}>
    </div>
    <${Switch} checked=${v.forcePathStyle} onChange=${(x) => s.set(path + ".forcePathStyle", x)} label="Path-style URLs (MinIO and most self-hosted servers)" />
    ${path === "s3" &&
    html`<${Switch} checked=${v.presignedDownloads} onChange=${(x) => s.set(path + ".presignedDownloads", x)} label="Downloads straight from the bucket (redirects to short-lived signed URLs; clients must reach the endpoint)" />`}
  </div>`;
}

function TestS3({ filesystem, disabled }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      await api.post("/api/settings/test/s3", { filesystem });
      toast("Connected to the bucket.");
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<${Button} size="sm" icon="zap" loading=${busy} disabled=${disabled} onClick=${run}>Test connection</${Button}>`;
}

function Storage({ s }) {
  const on = s.draft.s3.enabled;
  return html`<${Card}
    title="S3-compatible storage"
    subtitle=${on ? "Uploads are stored in your bucket" : "Uploads are stored on this server's disk"}
    icon="cloud"
    actions=${html`<${Switch} checked=${on} onChange=${(v) => s.set("s3.enabled", v)} label=${on ? "On" : "Off"} />`}
  >
    ${on && html`<${S3Fields} s=${s} path="s3" errors=${s.errors} />`}
    ${on && html`<div class="row"><${Callout}>Existing files aren't moved when you switch storage.</${Callout}><span class="spacer"></span><${TestS3} filesystem="storage" disabled=${s.dirty} /></div>`}
  </${Card}>`;
}

function Backups({ s }) {
  const [list, setList] = useState(null);
  const [busy, setBusy] = useState("");
  const [canBackup, setCanBackup] = useState(true);
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
  const create = async () => {
    setBusy("create");
    try {
      await api.post("/api/backups", {});
      toast("Backup created.");
      load();
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy("");
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
      message: "Everything — collections, records, files and settings — is replaced with the backup. Everyone is signed out.",
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
  return html`<div class="stack loose">
    <${Card}
      title="Backups"
      subtitle="Database and uploaded files in one zip"
      icon="archive"
      actions=${html`<div class="row">
        <label class="btn sm"><${Icon} name="upload" size=${15} /><span>${busy === "upload" ? "Uploading…" : "Upload"}</span><input type="file" accept=".zip" hidden onChange=${(e) => upload(e.target.files[0])} /></label>
        <${Button} size="sm" variant="primary" icon="plus" loading=${busy === "create"} disabled=${!canBackup} onClick=${create}>Create backup</${Button}>
      </div>`}
    >
      ${list === null
        ? html`<${Skeleton} height=${48} />`
        : list.length === 0
        ? html`<${Empty} icon="archive" title="No backups yet">Create one now, or schedule them below.</${Empty}>`
        : html`<div class="stack tight stagger">
            ${list.map(
              (b, i) => html`<div class="row" style=${{ "--i": i, padding: "10px 12px", border: "1px solid var(--line)", borderRadius: "var(--r-md)" }}>
                <span class="field-type-badge"><${Icon} name="archive" size=${15} /></span>
                <div class="grow"><div class="mono small ellipsis">${b.key}</div><div class="tiny muted">${bytes(b.size)} · ${formatDate(b.modified)} · ${relativeTime(b.modified)}</div></div>
                <${IconButton} icon="download" label="Download" onClick=${() => download(b.key)} />
                <${Button} size="sm" icon="undo" loading=${busy === b.key} onClick=${() => restore(b.key)}>Restore</${Button}>
                <${IconButton} icon="trash" label="Delete" onClick=${() => remove(b.key)} />
              </div>`
            )}
          </div>`}
    </${Card}>
    <${Card} title="Schedule" subtitle="Automatic backups (cron syntax, UTC)" icon="clock">
      <div class="grid-2">
        <${Field} label="Schedule" error=${s.errors["backups.cron"]} help="e.g. 0 3 * * * for every day at 03:00 — empty turns it off">
          <${Input} class="mono" value=${d.backups.cron} onInput=${(v) => s.set("backups.cron", v)} placeholder="0 3 * * *" />
        </${Field}>
        <${Field} label="Keep" help="Older automatic backups are deleted"><${Input} type="number" min="1" value=${d.backups.cronMaxKeep} onInput=${(v) => s.set("backups.cronMaxKeep", Number(v))} /></${Field}>
      </div>
      <div class="chips">
        ${[
          ["Daily 03:00", "0 3 * * *"],
          ["Every 6 hours", "0 */6 * * *"],
          ["Weekly", "0 3 * * 0"],
          ["Off", ""],
        ].map(([label, cron]) => html`<span class=${"chip selectable " + (d.backups.cron === cron ? "accent" : "outline")} onClick=${() => s.set("backups.cron", cron)}>${label}</span>`)}
      </div>
    </${Card}>
    <${Card} title="Store backups in S3" icon="cloud" actions=${html`<${Switch} checked=${d.backups.s3.enabled} onChange=${(v) => s.set("backups.s3.enabled", v)} label=${d.backups.s3.enabled ? "On" : "Off"} />`}>
      ${d.backups.s3.enabled && html`<${S3Fields} s=${s} path="backups.s3" errors=${s.errors} /><div class="row"><span class="spacer"></span><${TestS3} filesystem="backups" disabled=${s.dirty} /></div>`}
    </${Card}>
  </div>`;
}

function Security({ s }) {
  const d = s.draft;
  const rules = d.rateLimits.rules;
  const setRule = (i, patch) => s.set("rateLimits.rules", rules.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return html`<div class="stack loose">
    <${Card} title="Rate limits" subtitle="Requests per IP, per rule" icon="zap" actions=${html`<${Switch} checked=${d.rateLimits.enabled} onChange=${(v) => s.set("rateLimits.enabled", v)} label=${d.rateLimits.enabled ? "On" : "Off"} />`}>
      <div class="stack tight">
        <div class="row tiny faint" style="padding:0 4px"><span style="flex:2">Label (path or collection:action)</span><span style="flex:1">Audience</span><span style="width:110px">Max requests</span><span style="width:110px">Per seconds</span><span style="width:32px"></span></div>
        ${rules.map(
          (r, i) => html`<div class="row" key=${i}>
            <${Input} style="flex:2" class="mono" value=${r.label} onInput=${(v) => setRule(i, { label: v })} placeholder="/api/ or *:auth" />
            <${Select} style="flex:1" value=${r.audience} onChange=${(v) => setRule(i, { audience: v })} options=${[{ value: "", label: "Everyone" }, { value: "@guest", label: "Guests" }, { value: "@auth", label: "Signed in" }]} />
            <${Input} style="width:110px" type="number" value=${r.maxRequests} onInput=${(v) => setRule(i, { maxRequests: Number(v) })} />
            <${Input} style="width:110px" type="number" value=${r.duration} onInput=${(v) => setRule(i, { duration: Number(v) })} />
            <${IconButton} icon="trash" label="Remove" onClick=${() => s.set("rateLimits.rules", rules.filter((_, j) => j !== i))} />
          </div>`
        )}
        <div><${Button} size="sm" icon="plus" onClick=${() => s.set("rateLimits.rules", [...rules, { label: "", audience: "", duration: 10, maxRequests: 100 }])}>Add rule</${Button}></div>
      </div>
      <${Field} label="Never limit these IPs" help="Comma separated IPs or CIDR ranges">
        <${Input} class="mono" value=${(d.rateLimits.excludedIPs || []).join(", ")} onInput=${(v) => s.set("rateLimits.excludedIPs", v.split(",").map((x) => x.trim()).filter(Boolean))} />
      </${Field}>
    </${Card}>
    <${Card} title="Superuser access" subtitle="Restrict where superusers can work from" icon="shield">
      <${Field} label="Allowed IPs" error=${s.errors.superuserIPs} help="Comma separated IPs or CIDR ranges; empty allows any. Don't lock yourself out.">
        <${Input} class="mono" value=${(d.superuserIPs || []).join(", ")} onInput=${(v) => s.set("superuserIPs", v.split(",").map((x) => x.trim()).filter(Boolean))} placeholder="10.0.0.0/8, 203.0.113.7" />
      </${Field}>
      <${CurrentIp} />
    </${Card}>
    <${Card} title="Trusted proxy" subtitle="Read the client IP from proxy headers" icon="server">
      <${Field} label="Headers" help="e.g. X-Forwarded-For, CF-Connecting-IP — only set these behind a proxy you control">
        <${Input} class="mono" value=${(d.trustedProxy.headers || []).join(", ")} onInput=${(v) => s.set("trustedProxy.headers", v.split(",").map((x) => x.trim()).filter(Boolean))} />
      </${Field}>
      <${Switch} checked=${d.trustedProxy.useLeftmostIP} onChange=${(v) => s.set("trustedProxy.useLeftmostIP", v)} label="Use the leftmost IP (the original client) instead of the rightmost" />
    </${Card}>
    <${Card} title="Batch requests" subtitle="Several record writes in one transaction" icon="layers" actions=${html`<${Switch} checked=${d.batch.enabled} onChange=${(v) => s.set("batch.enabled", v)} label=${d.batch.enabled ? "On" : "Off"} />`}>
      ${d.batch.enabled &&
      html`<div class="grid-3 enter">
        <${Field} label="Max requests"><${Input} type="number" value=${d.batch.maxRequests} onInput=${(v) => s.set("batch.maxRequests", Number(v))} /></${Field}>
        <${Field} label="Timeout (seconds)"><${Input} type="number" value=${d.batch.timeout} onInput=${(v) => s.set("batch.timeout", Number(v))} /></${Field}>
        <${Field} label="Max body (bytes)" help="0 for 128 MB"><${Input} type="number" value=${d.batch.maxBodySize} onInput=${(v) => s.set("batch.maxBodySize", Number(v))} /></${Field}>
      </div>`}
    </${Card}>
  </div>`;
}

function CurrentIp() {
  const health = useStore(app, (st) => st.health);
  if (!health || !health.realIP) return null;
  return html`<div class="small muted row"><${Icon} name="info" size=${14} /> Your IP: <code>${health.realIP}</code>${health.possibleProxyHeader && html`<span> · behind a proxy? It sent <code>${health.possibleProxyHeader}</code></span>`}</div>`;
}

function LogsSettings({ s }) {
  const d = s.draft;
  return html`<${Card} title="Logs" subtitle="What's recorded and for how long" icon="logs">
    <div class="grid-2">
      <${Field} label="Keep for (days)" help="0 turns request logging off"><${Input} type="number" min="0" value=${d.logs.maxDays} onInput=${(v) => s.set("logs.maxDays", Number(v))} /></${Field}>
      <${Field} label="Minimum level">
        <${Select} value=${String(d.logs.minLevel)} onChange=${(v) => s.set("logs.minLevel", Number(v))} options=${[{ value: "-4", label: "Debug" }, { value: "0", label: "Info" }, { value: "4", label: "Warning" }, { value: "8", label: "Error" }]} />
      </${Field}>
      <${Field} label="Largest entry data (bytes)" help="Bigger entries keep what fits. 0 is 16 KiB.">
        <${Input} type="number" min="0" value=${d.logs.maxDataSize || 0} onInput=${(v) => s.set("logs.maxDataSize", Number(v))} />
      </${Field}>
    </div>
    <div class="row wrap" style="gap:22px">
      <${Switch} checked=${d.logs.logIP} onChange=${(v) => s.set("logs.logIP", v)} label="Record IP addresses" />
      <${Switch} checked=${d.logs.logAuthId} onChange=${(v) => s.set("logs.logAuthId", v)} label="Record who made each request" />
    </div>
  </${Card}>`;
}

function Crons() {
  const [jobs, setJobs] = useState(null);
  const [running, setRunning] = useState("");
  useEffect(() => {
    api.get("/api/crons").then(setJobs).catch(() => setJobs([]));
  }, []);
  const run = async (id) => {
    setRunning(id);
    try {
      await api.post(`/api/crons/${enc(id)}`);
      toast(`Started ${id}.`);
    } catch (err) {
      toast.error(err);
    } finally {
      setRunning("");
    }
  };
  return html`<${Card} title="Cron jobs" subtitle="Scheduled jobs from hooks and backups (UTC)" icon="clock">
    ${jobs === null
      ? html`<${Skeleton} height=${40} />`
      : jobs.length === 0
      ? html`<${Empty} icon="clock" title="No cron jobs">Add them with <code>cronAdd()</code> in a hook file, or schedule backups.</${Empty}>`
      : html`<div class="stack tight stagger">
          ${jobs.map(
            (j, i) => html`<div class="row" style=${{ "--i": i, padding: "10px 12px", border: "1px solid var(--line)", borderRadius: "var(--r-md)" }}>
              <span class="field-type-badge"><${Icon} name="clock" size=${15} /></span>
              <div class="grow"><b class="mono small">${j.id}</b><div class="tiny muted mono">${j.expression}</div></div>
              <${Button} size="sm" icon="play" loading=${running === j.id} onClick=${() => run(j.id)}>Run now</${Button}>
            </div>`
          )}
        </div>`}
  </${Card}>`;
}

function Transfer() {
  const collections = useStore(app, (st) => st.collections);
  const [text, setText] = useState("");
  const [deleteMissing, setDeleteMissing] = useState(false);
  const [busy, setBusy] = useState(false);
  let parsed = null;
  let parseError = "";
  if (text.trim()) {
    try {
      parsed = JSON.parse(text);
      if (!Array.isArray(parsed)) throw new Error("Expected a list of collections.");
    } catch (err) {
      parseError = err.message;
      parsed = null;
    }
  }
  const exportJson = JSON.stringify(collections.filter((c) => !c.system).map(({ created, updated, ...c }) => c), null, 2);
  const download = () => {
    const blob = new Blob([exportJson], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = "slonbase_collections.json";
    a.click();
    URL.revokeObjectURL(a.href);
  };
  const changes = parsed
    ? {
        added: parsed.filter((p) => !collections.some((c) => c.id === p.id || c.name === p.name)),
        changed: parsed.filter((p) => collections.some((c) => c.id === p.id || c.name === p.name)),
        removed: deleteMissing ? collections.filter((c) => !c.system && !parsed.some((p) => p.id === c.id || p.name === c.name)) : [],
      }
    : null;
  const run = async () => {
    if (changes.removed.length && !(await confirmDialog({ title: `Delete ${plural(changes.removed.length, "collection")}?`, message: changes.removed.map((c) => c.name).join(", "), confirm: "Import and delete", danger: true }))) return;
    setBusy(true);
    try {
      await api.put("/api/collections/import", { collections: parsed, deleteMissing });
      await loadCollections();
      toast("Collections imported.");
      setText("");
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<div class="stack loose">
    <${Card} title="Export" subtitle="Your collections' schema as JSON (no records)" icon="download" actions=${html`<${Button} size="sm" icon="download" onClick=${download}>Download JSON</${Button}>`}>
      <pre class="code" style="max-height:220px">${exportJson}</pre>
    </${Card}>
    <${Card} title="Import" subtitle="Create or update collections from JSON (PocketBase exports work too)" icon="upload">
      <div class="row">
        <label class="btn sm"><${Icon} name="file" size=${15} /><span>Load a file</span><input type="file" accept=".json" hidden onChange=${async (e) => setText(await e.target.files[0].text())} /></label>
      </div>
      <${TextArea} code rows=${8} value=${text} onInput=${setText} placeholder="Paste the exported JSON" invalid=${!!parseError} />
      ${parseError && html`<span class="small" style="color:var(--danger)">${parseError}</span>`}
      <${Switch} checked=${deleteMissing} onChange=${setDeleteMissing} label="Delete collections that aren't in the import" />
      ${changes &&
      html`<div class="row wrap enter">
        <span class="chip ok">+ ${changes.added.length} new</span>
        <span class="chip warn">~ ${changes.changed.length} updated</span>
        ${deleteMissing && html`<span class="chip danger">− ${changes.removed.length} deleted</span>`}
        <span class="spacer"></span>
        <${Button} variant="primary" icon="upload" loading=${busy} onClick=${run}>Import</${Button}>
      </div>`}
    </${Card}>
  </div>`;
}

export function Settings({ section }) {
  const s = useSettings();
  const active = SECTIONS.find((x) => x.id === section) || SECTIONS[0];
  const needsDraft = !["crons", "transfer"].includes(active.id);
  return html`<div class="page enter">
    <div class="page-head"><div class="grow"><div class="eyebrow"><${Icon} name="settings" size=${14} /> Configuration</div><h1>Settings</h1></div></div>
    <div class="settings-layout">
      <nav class="settings-nav">
        ${SECTIONS.map((x) => html`<a href=${"#/settings/" + x.id} class=${cx({ on: x.id === active.id })}><${Icon} name=${x.icon} size=${17} />${x.label}</a>`)}
      </nav>
      <div key=${active.id}>
        ${needsDraft && !s.draft
          ? html`<div class="stack"><${Skeleton} height=${160} /><${Skeleton} height=${120} /></div>`
          : html`
              ${active.id === "general" && html`<${General} s=${s} />`}
              ${active.id === "mail" && html`<${Mail} s=${s} />`}
              ${active.id === "storage" && html`<${Storage} s=${s} />`}
              ${active.id === "backups" && html`<${Backups} s=${s} />`}
              ${active.id === "security" && html`<${Security} s=${s} />`}
              ${active.id === "logs" && html`<${LogsSettings} s=${s} />`}
              ${active.id === "crons" && html`<${Crons} />`}
              ${active.id === "transfer" && html`<${Transfer} />`}
            `}
        ${s.dirty &&
        html`<div class="savebar">
          <${Icon} name="edit" size=${16} />
          <span class="grow">Unsaved settings</span>
          <${Button} variant="ghost" size="sm" icon="undo" onClick=${() => {
            s.reset();
            applyAccent(s.saved.meta.accentColor);
          }}>Discard</${Button}>
          <${Button} variant="primary" size="sm" loading=${s.saving} onClick=${s.save}>Save settings</${Button}>
        </div>`}
      </div>
    </div>
  </div>`;
}
