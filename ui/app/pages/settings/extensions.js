// Extensions: what this instance has loaded (GET /api/extensions): hook files and their
// handlers, routes and cron jobs, the kinds of background jobs it runs, and the sections of the
// settings. Read-only: it answers "is my code running here?" without reading logs.
import { html, useState, useEffect } from "../../lib/core.js";
import { api } from "../../lib/api.js";
import { Icon } from "../../lib/icons.js";
import { Button, Skeleton, Callout, Field, Input, Select, Switch, toast } from "../../lib/ui.js";
import { useMyRole } from "./team.js";
import { plural } from "../../lib/format.js";
import { Card } from "./shared.js";

function Table({ head, rows, label }) {
  return html`<div class="table-wrap"><table class="table" aria-label=${label}>
    <thead><tr>${head.map((h) => html`<th>${h}</th>`)}</tr></thead>
    <tbody>${rows.map((cells) => html`<tr>${cells.map((c) => html`<td>${c}</td>`)}</tr>`)}</tbody>
  </table></div>`;
}

const code = (text) => html`<code>${text}</code>`;

// A settings page a hook file declared with settingsAdd(): its form, drawn from its fields.
function SettingsPage({ page, canSave }) {
  const [values, setValues] = useState(page.values || {});
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  const set = (name) => (value) => setValues((v) => ({ ...v, [name]: value }));
  const save = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      const saved = await api.patch(`/api/extensions/settings/${encodeURIComponent(page.name)}`, { values });
      setValues(saved.values || {});
      setErrors({});
      toast("Saved.");
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  const control = (f) => {
    const value = values[f.name];
    switch (f.type) {
      case "bool":
        return html`<${Switch} checked=${!!value} onChange=${set(f.name)} label=${value ? "On" : "Off"} disabled=${!canSave} />`;
      case "select":
        return html`<${Select} value=${value || ""} onChange=${set(f.name)} options=${[...(f.required ? [] : [{ value: "", label: "None" }]), ...f.options.map((o) => ({ value: o, label: o }))]} disabled=${!canSave} />`;
      case "textarea":
        return html`<textarea class="input" rows="4" value=${value || ""} onInput=${(ev) => set(f.name)(ev.target.value)} disabled=${!canSave}></textarea>`;
      case "number":
        return html`<${Input} type="number" step="any" value=${value == null ? "" : value} onInput=${set(f.name)} disabled=${!canSave} />`;
      case "secret":
        return html`<${Input} type="password" autocomplete="off" value=${value || ""} onInput=${set(f.name)} onFocus=${(ev) => ev.target.select()} disabled=${!canSave} />`;
      default:
        return html`<${Input} value=${value || ""} onInput=${set(f.name)} disabled=${!canSave} />`;
    }
  };
  return html`<${Card} title=${page.title || page.name} icon="settings" subtitle=${html`Declared by a hook file; read there with <code>${`$app.settings().ext("${page.name}")`}</code>.`}>
    <form class="stack" onSubmit=${save}>
      ${page.fields.map(
        (f) => html`<${Field} label=${(f.label || f.name) + (f.required ? " *" : "")} error=${errors[`values.${f.name}`]} help=${f.type === "secret" ? [f.help, "Saved values are hidden; leave it as it is to keep it."].filter(Boolean).join(" ") : f.help}>${control(f)}</${Field}>`
      )}
      ${canSave && html`<div class="row"><span class="spacer"></span><${Button} variant="primary" type="submit" loading=${busy}>Save</${Button}></div>`}
    </form>
  </${Card}>`;
}

export function Extensions() {
  const role = useMyRole();
  const [pages, setPages] = useState([]);
  useEffect(() => {
    api.get("/api/extensions/settings").then(setPages).catch(() => {});
  }, []);
  const [data, setData] = useState(null);
  const [denied, setDenied] = useState(false);
  const [failed, setFailed] = useState(false);
  const load = () => {
    setFailed(false);
    api
      .get("/api/extensions")
      .then(setData)
      .catch((err) => (err.status === 403 ? setDenied(true) : setFailed(true)));
  };
  useEffect(load, []);
  if (denied) return html`<${Callout} kind="info">Your role can't see what this instance has loaded.</${Callout}>`;
  if (failed)
    return html`<${Callout} kind="warn">
      <div class="row"><span class="grow">What this instance has loaded couldn't be read.</span><${Button} size="sm" icon="refresh" onClick=${load}>Try again</${Button}></div>
    </${Callout}>`;
  if (!data) return html`<div class="stack"><${Skeleton} height=${160} /><${Skeleton} height=${120} /></div>`;
  const { hooks, cronJobs, jobKinds, settingsSections } = data;
  const stepFunctions = data.stepFunctions || [];
  return html`<div class="stack">
    ${pages.map((page) => html`<${SettingsPage} key=${page.name} page=${page} canSave=${role === "admin" || role === "owner"} />`)}
    <${Card}
      title="Hooks"
      icon="zap"
      subtitle=${hooks.files.length
        ? `${plural(hooks.files.length, "file")} loaded from the hooks directory, ${plural(hooks.handlers.length, "handler")} and ${plural(hooks.routes.length, "route")} registered.`
        : "No hook files are loaded on this instance."}
    >
      ${hooks.files.length > 0 && html`<div class="row wrap">${hooks.files.map((f) => html`<span class="chip"><${Icon} name="code" size=${13} /> ${f}</span>`)}</div>`}
      ${hooks.handlers.length > 0 &&
      html`<${Table}
        label="Handlers"
        head=${["Event", "Collections"]}
        rows=${hooks.handlers.map((h) => [code(h.event), h.collections.length ? h.collections.join(", ") : html`<span class="muted">All</span>`])}
      />`}
      ${hooks.routes.length > 0 &&
      html`<${Table} label="Routes" head=${["Method", "Path"]} rows=${hooks.routes.map((r) => [code(r.method), code(r.path)])} />`}
    </${Card}>
    <${Card} title="Cron jobs registered by code" icon="clock" subtitle="Jobs made in the dashboard are under Automations.">
      ${cronJobs.length > 0
        ? html`<${Table} label="Cron jobs registered by code" head=${["Id", "Schedule"]} rows=${cronJobs.map((j) => [code(j.id), code(j.expression)])} />`
        : null}
    </${Card}>
    <${Card} title="Step functions" icon="code" subtitle=${html`Functions hook files register with <code>automationStep()</code>, for the “Run a hook function” step of automations.`}>
      ${stepFunctions.length > 0 ? html`<div class="row wrap">${stepFunctions.map((f) => html`<span class="chip mono">${f}</span>`)}</div>` : null}
    </${Card}>
    <${Card} title="Background jobs" icon="layers" subtitle="The kinds of queued work this instance's workers run.">
      <${Table} label="Background jobs" head=${["Kind", "What it does"]} rows=${jobKinds.map((k) => [code(k.kind), k.title])} />
    </${Card}>
    <${Card} title="Settings sections" icon="settings" subtitle="Each section is stored and changed on its own.">
      <${Table}
        label="Settings sections"
        head=${["Section", "What it holds", "Changed through"]}
        rows=${settingsSections.map((s) => [code(s.name), s.title, s.private ? "Its own API" : code("/api/settings")])}
      />
    </${Card}>
  </div>`;
}
