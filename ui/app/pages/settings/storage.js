// Settings → File storage: where uploads are kept (this server's disk, or an S3 bucket), how
// much room there is and how much they take.
import { html, useState, useEffect, useStore } from "../../lib/core.js";
import { api, enc } from "../../lib/api.js";
import { app, upsertCollection } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { Button, IconButton, Field, Input, Switch, Callout, Skeleton, toast, confirmDialog } from "../../lib/ui.js";
import { bytes, relativeTime } from "../../lib/format.js";
import { Card } from "./shared.js";

export function S3Fields({ s, path, errors }) {
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

export function TestS3({ filesystem, disabled }) {
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

function Meter({ used, total }) {
  const pct = total ? Math.min(100, (used / total) * 100) : 0;
  return html`<div class="meter" role="img" aria-label=${`${pct.toFixed(0)}% used`}><span style=${{ width: pct + "%" }} class=${"meter-fill" + (pct > 75 ? " warn" : "")}></span></div>`;
}

function Where({ refreshKey }) {
  const [info, setInfo] = useState(null);
  const [busy, setBusy] = useState(false);
  const load = (refresh) =>
    api
      .get("/api/settings/storage" + (refresh ? "?refresh=1" : ""))
      .then((i) => {
        setInfo(i);
        // measured in the background: ask again shortly
        if (i.measuring || (i.kind === "local" && !i.usage)) setTimeout(() => load(false), 1500);
      })
      .catch(toast.error);
  useEffect(() => {
    load(false);
  }, [refreshKey]);
  const copy = (text) => navigator.clipboard.writeText(text).then(() => toast("Copied."), toast.error);
  const clearThumbs = async () => {
    if (!(await confirmDialog({ title: "Delete the generated thumbnails?", message: "They're made again the next time they're asked for.", confirm: "Delete thumbnails" }))) return;
    setBusy(true);
    try {
      const res = await api.post("/api/settings/storage/clear-thumbs");
      toast(`Deleted ${bytes(res.bytes)} of thumbnails.`);
      load(true);
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  if (!info) return html`<${Skeleton} height=${140} />`;
  const u = info.usage;
  const disk = info.disk;
  return html`<${Card} title="Where files are" subtitle=${info.description} icon="server" actions=${html`<${IconButton} icon="refresh" label="Measure again" onClick=${() => load(true)} />`}>
    <div class="storage-facts">
      <div>
        <div class="tiny muted">Kept in</div>
        ${info.kind === "local"
          ? html`<div class="row tight"><code class="ellipsis">${info.path}</code><${IconButton} icon="copy" label="Copy the path" onClick=${() => copy(info.path)} /></div>`
          : info.kind === "s3"
          ? html`<b>An S3 bucket</b><div class="tiny muted">Local directory when it's off: <code>${info.localPath}</code></div>`
          : html`<b class="danger-text">Unavailable</b>`}
      </div>
      ${disk &&
      html`<div>
        <div class="tiny muted">Disk</div>
        <div><b>${bytes(disk.free)}</b> free of ${bytes(disk.total)}</div>
        <${Meter} used=${disk.total - disk.free} total=${disk.total} />
      </div>`}
      ${u
        ? html`<div>
            <div class="tiny muted">Uploads</div>
            <div><b>${bytes(u.bytes)}</b> in ${u.files.toLocaleString()} files</div>
            <div class="tiny muted">${bytes(u.thumbBytes)} of it thumbnails · measured ${relativeTime(u.measured)}</div>
          </div>`
        : info.kind === "local" && html`<div><div class="tiny muted">Uploads</div><div class="muted">Measuring…</div></div>`}
    </div>
    ${u &&
    u.collections.length > 0 &&
    html`<div class="table-wrap"><table class="table">
      <thead><tr><th>Collection</th><th class="num">Files</th><th class="num">Size</th><th>Share</th></tr></thead>
      <tbody>
        ${u.collections.map(
          (c) => html`<tr>
            <td>${c.name ? html`<b>${c.name}</b>` : html`<span class="muted mono small">${c.id}</span>`}</td>
            <td class="num mono small">${c.files.toLocaleString()}</td>
            <td class="num mono small">${bytes(c.bytes)}</td>
            <td style="width:30%"><${Meter} used=${c.bytes} total=${u.bytes} /></td>
          </tr>`
        )}
      </tbody>
    </table></div>`}
    ${info.kind === "local" &&
    html`<div class="row"><span class="small muted grow">Thumbnails are made when first asked for, and kept.</span><${Button} size="sm" icon="trash" loading=${busy} disabled=${!u || !u.thumbBytes} onClick=${clearThumbs}>Clear thumbnails</${Button}></div>`}
  </${Card}>`;
}

// how long an auth token and a protected-file token last, per auth collection
export function TokenLifetimes({ only }) {
  const collections = useStore(app, (st) => st.collections).filter((c) => c.type === "auth" && (only ? only.includes(c.name) : c.name !== "_superusers"));
  const [busy, setBusy] = useState("");
  const set = async (c, key, minutes) => {
    const seconds = Math.round(Number(minutes) * 60);
    if (!(seconds >= 60)) return toast.error(new Error("At least one minute."));
    setBusy(c.id + key);
    try {
      const res = await api.patch(`/api/collections/${enc(c.id)}`, { ...c, [key]: { ...c[key], duration: seconds } });
      upsertCollection(res);
      toast("Saved.");
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy("");
    }
  };
  if (!collections.length) return null;
  const rows = [
    ["fileToken", "Protected-file token"],
    ["authToken", "Session"],
  ];
  return html`<${Card} title="Token lifetimes" subtitle="How long a session and a protected-file token last, per auth collection" icon="clock">
    <div class="table-wrap"><table class="table">
      <thead><tr><th>Collection</th>${rows.map(([, label]) => html`<th>${label} (minutes)</th>`)}</tr></thead>
      <tbody>
        ${collections.map(
          (c) => html`<tr key=${c.id}>
            <td><b>${c.name}</b></td>
            ${rows.map(
              ([key, label]) => html`<td><input class="input" type="number" min="1" aria-label=${`${c.name}: ${label} in minutes`} defaultValue=${Math.round(((c[key] || {}).duration || 0) / 60)} disabled=${busy === c.id + key} onBlur=${(e) => Number(e.target.value) * 60 !== (c[key] || {}).duration && set(c, key, e.target.value)} style="max-width:140px" /></td>`
            )}
          </tr>`
        )}
      </tbody>
    </table></div>
    <div class="small muted">Saved when you leave the field.</div>
  </${Card}>`;
}

export function Storage({ s }) {
  const on = s.draft.s3.enabled;
  return html`<div class="stack loose">
    <${Where} refreshKey=${s.saved && s.saved.s3.enabled} />
    <${Card}
      title="S3-compatible storage"
      subtitle=${on ? "Uploads are stored in your bucket" : "Uploads are stored on this server's disk"}
      icon="cloud"
      actions=${html`<${Switch} checked=${on} onChange=${(v) => s.set("s3.enabled", v)} label=${on ? "On" : "Off"} />`}
    >
      ${on && html`<${S3Fields} s=${s} path="s3" errors=${s.errors} />`}
      ${on && html`<div class="row"><${Callout}>Existing files aren't moved when you switch storage.</${Callout}><span class="spacer"></span><${TestS3} filesystem="storage" disabled=${s.dirty} /></div>`}
    </${Card}>
    <${Card} title="Uploads" subtitle="How large files and requests may be, and thumbnails" icon="upload">
      <div class="grid-2">
        <${Field} label="Largest request (MB)" error=${s.errors["security.maxUploadMb"]} help="The whole request, files together. Backup uploads aren't limited.">
          <${Input} type="number" min="1" max="16384" value=${s.draft.security.maxUploadMb} onInput=${(v) => s.set("security.maxUploadMb", Number(v))} />
        </${Field}>
        <${Field} label="Default file size (MB)" error=${s.errors["files.defaultMaxSize"]} help="For file fields without a size of their own; 0 is 5 MB">
          <${Input} type="number" min="0" step="0.5" value=${Math.round(((s.draft.files.defaultMaxSize || 0) / 1048576) * 100) / 100} onInput=${(v) => s.set("files.defaultMaxSize", Math.round(Number(v) * 1048576))} />
        </${Field}>
      </div>
      <${Switch} checked=${s.draft.files.thumbs} onChange=${(v) => s.set("files.thumbs", v)} label="Make thumbnails when asked for (off: the original file is served, whatever the size)" />
    </${Card}>
    <${TokenLifetimes} />
  </div>`;
}
