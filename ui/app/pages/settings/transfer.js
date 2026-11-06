// Settings → Import & export: collections' schemas (PocketBase's format) and the settings,
// with what an import would change shown, collection by collection, before it's applied.
import { html, useState, useStore, useMemo, cx } from "../../lib/core.js";
import { api } from "../../lib/api.js";
import { app, loadCollections } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { Button, TextArea, Switch, Segmented, Callout, toast, confirmDialog, CollectionGlyph } from "../../lib/ui.js";
import { plural } from "../../lib/format.js";
import { download } from "../../lib/export.js";
import { Card } from "./shared.js";

const RULES = ["listRule", "viewRule", "createRule", "updateRule", "deleteRule"];
const strip = ({ created, updated, ...c }) => c;

// what importing `next` over `current` changes: fields added, removed and changed, rules, indexes
function diff(current, next) {
  const byName = (list) => new Map((list || []).map((f) => [f.name, f]));
  const before = byName(current.fields);
  const after = byName(next.fields);
  const added = [...after.keys()].filter((k) => !before.has(k));
  const removed = [...before.keys()].filter((k) => !after.has(k) && !(before.get(k) || {}).system);
  const changed = [...after.keys()].filter((k) => before.has(k) && JSON.stringify(before.get(k)) !== JSON.stringify(after.get(k)));
  const rules = RULES.filter((r) => r in next && (current[r] ?? null) !== (next[r] ?? null));
  const indexes = JSON.stringify(current.indexes || []) !== JSON.stringify(next.indexes || []);
  return { added, removed, changed, rules, indexes, same: !added.length && !removed.length && !changed.length && !rules.length && !indexes && current.name === next.name };
}

function Export() {
  const collections = useStore(app, (st) => st.collections);
  const [system, setSystem] = useState(false);
  const [pretty, setPretty] = useState(true);
  const candidates = collections.filter((c) => system || !c.system);
  const [picked, setPicked] = useState(null);
  const chosen = picked || new Set(candidates.filter((c) => !c.system).map((c) => c.id));
  const toggle = (id) => {
    const next = new Set(chosen);
    next.has(id) ? next.delete(id) : next.add(id);
    setPicked(next);
  };
  const out = candidates.filter((c) => chosen.has(c.id)).map(strip);
  const json = JSON.stringify(out, null, pretty ? 2 : 0);
  return html`<${Card} title="Export" subtitle="Collections' schemas as JSON (no records), for another installation or PocketBase" icon="download">
    <div class="row wrap">
      <${Switch} checked=${system} onChange=${setSystem} label="Include system collections" />
      <${Switch} checked=${pretty} onChange=${setPretty} label="Indented" />
      <span class="spacer"></span>
      <${Button} size="sm" variant="ghost" onClick=${() => setPicked(new Set(candidates.map((c) => c.id)))}>All</${Button}>
      <${Button} size="sm" variant="ghost" onClick=${() => setPicked(new Set())}>None</${Button}>
    </div>
    <div class="pick-grid">
      ${candidates.map(
        (c) => html`<label class=${cx("pick", { on: chosen.has(c.id) })}>
          <input type="checkbox" checked=${chosen.has(c.id)} onChange=${() => toggle(c.id)} />
          <${CollectionGlyph} collection=${c} size=${20} />
          <span class="ellipsis">${c.name}</span>
          <span class="tiny muted">${c.fields.length}</span>
        </label>`
      )}
    </div>
    <pre class="code" style="max-height:220px" tabindex="0" aria-label="The export">${json}</pre>
    <div class="row">
      <span class="small muted grow">${plural(out.length, "collection")} · ${(json.length / 1024).toFixed(1)} KB</span>
      <${Button} size="sm" icon="copy" disabled=${!out.length} onClick=${() => navigator.clipboard.writeText(json).then(() => toast("Copied."), toast.error)}>Copy</${Button}>
      <${Button} size="sm" variant="primary" icon="download" disabled=${!out.length} onClick=${() => download("slonbase_collections.json", json, "application/json")}>Download JSON</${Button}>
    </div>
  </${Card}>`;
}

function Import() {
  const collections = useStore(app, (st) => st.collections);
  const [text, setText] = useState("");
  const [mode, setMode] = useState("merge");
  const [busy, setBusy] = useState(false);
  const [errors, setErrors] = useState(null);
  const parsed = useMemo(() => {
    if (!text.trim()) return { list: null, error: "" };
    try {
      const v = JSON.parse(text);
      const list = Array.isArray(v) ? v : Array.isArray(v.collections) ? v.collections : null;
      if (!list) return { list: null, error: "Expected a list of collections." };
      if (list.some((c) => !c || typeof c !== "object" || !c.name)) return { list: null, error: "Every collection needs a name." };
      return { list, error: "" };
    } catch (err) {
      return { list: null, error: err.message };
    }
  }, [text]);
  const plan = useMemo(() => {
    if (!parsed.list) return null;
    const find = (p) => collections.find((c) => c.id === p.id || c.name === p.name);
    const rows = parsed.list.map((p) => {
      const current = find(p);
      return { name: p.name, type: p.type || "base", status: current ? "update" : "create", diff: current ? diff(current, p) : null };
    });
    const removed = mode === "replace" ? collections.filter((c) => !c.system && !parsed.list.some((p) => p.id === c.id || p.name === c.name)) : [];
    return { rows, removed };
  }, [parsed, collections, mode]);
  const run = async () => {
    if (plan.removed.length && !(await confirmDialog({ title: `Delete ${plural(plan.removed.length, "collection")}?`, message: `${plan.removed.map((c) => c.name).join(", ")} — with their records.`, confirm: "Import and delete", danger: true, typed: "delete" }))) return;
    setBusy(true);
    setErrors(null);
    try {
      await api.put("/api/collections/import", { collections: parsed.list, deleteMissing: mode === "replace" });
      await loadCollections();
      toast("Collections imported.");
      setText("");
    } catch (err) {
      setErrors(err.data && err.data.collections ? err.data.collections : err.data || null);
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  const created = plan ? plan.rows.filter((r) => r.status === "create").length : 0;
  const updated = plan ? plan.rows.filter((r) => r.status === "update" && !r.diff.same).length : 0;
  const same = plan ? plan.rows.length - created - updated : 0;
  return html`<${Card} title="Import" subtitle="Create or update collections from JSON (PocketBase exports work too)" icon="upload">
    <div class="row">
      <label class="btn sm"><${Icon} name="file" size=${15} /><span>Load a file</span><input type="file" accept=".json,application/json" hidden onChange=${async (e) => e.target.files[0] && setText(await e.target.files[0].text())} /></label>
      <span class="spacer"></span>
      <${Segmented} value=${mode} onChange=${setMode} options=${[{ value: "merge", label: "Merge" }, { value: "replace", label: "Replace" }]} />
    </div>
    <div class="small muted">${mode === "merge" ? "Collections in the file are created or updated; the others stay." : "The others are deleted, with their records: the installation ends up with exactly the file's collections."}</div>
    <${TextArea} code rows=${8} value=${text} onInput=${setText} placeholder="Paste the exported JSON" invalid=${!!parsed.error} aria-label="The collections to import, as JSON" />
    ${parsed.error && html`<span class="small" style="color:var(--danger)">${parsed.error}</span>`}
    ${plan &&
    html`<div class="stack tight enter">
      <div class="row wrap">
        <span class="chip ok">+ ${created} new</span>
        <span class="chip warn">~ ${updated} changed</span>
        ${same > 0 && html`<span class="chip outline">${same} unchanged</span>`}
        ${mode === "replace" && html`<span class="chip danger">− ${plan.removed.length} deleted</span>`}
      </div>
      <div class="table-wrap"><table class="table import-plan">
        <thead><tr><th>Collection</th><th>What happens</th></tr></thead>
        <tbody>
          ${plan.rows.map(
            (r) => html`<tr>
              <td><b>${r.name}</b> <span class="tiny muted">${r.type}</span></td>
              <td class="small">
                ${r.status === "create"
                  ? html`<span class="chip ok">new</span>`
                  : r.diff.same
                  ? html`<span class="muted">no change</span>`
                  : html`<span class="diff-line">
                      ${r.diff.added.length > 0 && html`<span class="diff-add">+ ${r.diff.added.join(", ")}</span>`}
                      ${r.diff.changed.length > 0 && html`<span class="diff-change">~ ${r.diff.changed.join(", ")}</span>`}
                      ${r.diff.removed.length > 0 && html`<span class="diff-remove">− ${r.diff.removed.join(", ")} (with their data)</span>`}
                      ${r.diff.rules.length > 0 && html`<span class="diff-change">rules: ${r.diff.rules.map((x) => x.replace("Rule", "")).join(", ")}</span>`}
                      ${r.diff.indexes && html`<span class="diff-change">indexes</span>`}
                    </span>`}
              </td>
            </tr>`
          )}
          ${plan.removed.map((c) => html`<tr><td><b>${c.name}</b></td><td class="small"><span class="diff-remove">deleted, with its records</span></td></tr>`)}
        </tbody>
      </table></div>
      ${errors && html`<${Callout} kind="danger"><pre class="small" style="margin:0;white-space:pre-wrap">${JSON.stringify(errors, null, 2)}</pre></${Callout}>`}
      <div class="row"><span class="spacer"></span><${Button} variant="primary" icon="upload" loading=${busy} disabled=${!created && !updated && !plan.removed.length} onClick=${run}>Import</${Button}></div>
    </div>`}
  </${Card}>`;
}

// the settings, without passwords and secrets, to set up another installation alike
function SettingsTransfer() {
  const [busy, setBusy] = useState(false);
  const exportSettings = async () => {
    try {
      const s = await api.get("/api/settings");
      download("slonbase_settings.json", JSON.stringify(s, null, 2), "application/json");
    } catch (err) {
      toast.error(err);
    }
  };
  const importSettings = async (file) => {
    if (!file) return;
    setBusy(true);
    try {
      const s = JSON.parse(await file.text());
      if (!s || typeof s !== "object" || !s.meta) throw new Error("That isn't a settings export.");
      if (!(await confirmDialog({ title: "Apply these settings?", message: "They replace this installation's settings, section by section. Passwords and secrets that are empty in the file are kept.", confirm: "Apply" }))) return;
      // images are this installation's own
      if (s.meta) for (const k of ["logo", "logoDark", "favicon"]) delete s.meta[k];
      if (s.login) for (const k of ["logo", "backgroundAsset"]) delete s.login[k];
      if (s.login && s.login.background === "asset") s.login.background = "none";
      await api.patch("/api/settings", s);
      toast("Settings applied. Reload to see them everywhere.");
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<${Card} title="Settings" subtitle="Every setting, without passwords and secrets, as JSON" icon="settings">
    <div class="row">
      <span class="small muted grow">Images (Branding) aren't included: upload them on the other installation.</span>
      <label class=${cx("btn sm", { disabled: busy })}><${Icon} name="upload" size=${15} /><span>Apply a file</span><input type="file" accept=".json,application/json" hidden onChange=${(e) => importSettings(e.target.files[0])} /></label>
      <${Button} size="sm" icon="download" onClick=${exportSettings}>Export settings</${Button}>
    </div>
  </${Card}>`;
}

export function Transfer() {
  return html`<div class="stack loose"><${Export} /><${Import} /><${SettingsTransfer} /></div>`;
}
