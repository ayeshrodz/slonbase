// Extensions: what this instance has loaded (GET /api/extensions): hook files and their
// handlers, routes and cron jobs, the kinds of background jobs it runs, and the sections of the
// settings. Read-only: it answers "is my code running here?" without reading logs.
import { html, useState, useEffect } from "../../lib/core.js";
import { api } from "../../lib/api.js";
import { Icon } from "../../lib/icons.js";
import { Skeleton, Callout, toast } from "../../lib/ui.js";
import { plural } from "../../lib/format.js";
import { Card } from "./shared.js";

function Table({ head, rows, label }) {
  return html`<div class="table-wrap"><table class="table" aria-label=${label}>
    <thead><tr>${head.map((h) => html`<th>${h}</th>`)}</tr></thead>
    <tbody>${rows.map((cells) => html`<tr>${cells.map((c) => html`<td>${c}</td>`)}</tr>`)}</tbody>
  </table></div>`;
}

const code = (text) => html`<code>${text}</code>`;

export function Extensions() {
  const [data, setData] = useState(null);
  const [denied, setDenied] = useState(false);
  useEffect(() => {
    api
      .get("/api/extensions")
      .then(setData)
      .catch((err) => (err.status === 403 ? setDenied(true) : toast.error(err)));
  }, []);
  if (denied) return html`<${Callout} kind="info">Your role can't see what this instance has loaded.</${Callout}>`;
  if (!data) return html`<div class="stack"><${Skeleton} height=${160} /><${Skeleton} height=${120} /></div>`;
  const { hooks, cronJobs, jobKinds, settingsSections } = data;
  return html`<div class="stack">
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
    <${Card} title="Cron jobs registered by code" icon="clock" subtitle="Jobs made in the dashboard are under Cron jobs.">
      ${cronJobs.length > 0
        ? html`<${Table} label="Cron jobs registered by code" head=${["Id", "Schedule"]} rows=${cronJobs.map((j) => [code(j.id), code(j.expression)])} />`
        : null}
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
