// The overview: counts, recent traffic and the collections at a glance.
import { html, useState, useEffect, useStore, navigate } from "../lib/core.js";
import { api, qs, enc, session } from "../lib/api.js";
import { app } from "../lib/state.js";
import { Icon } from "../lib/icons.js";
import { Button, CollectionGlyph, CountUp, Skeleton } from "../lib/ui.js";
import { parseDate, plural, relativeTime } from "../lib/format.js";

function greeting() {
  const h = new Date().getHours();
  return h < 5 ? "Working late" : h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
}

export function Overview() {
  const collections = useStore(app, (s) => s.collections);
  const loaded = useStore(app, (s) => s.collectionsLoaded);
  const [counts, setCounts] = useState({});
  const [stats, setStats] = useState(null);
  const [errors, setErrors] = useState(null);
  const [backups, setBackups] = useState(null);

  useEffect(() => {
    api.get("/api/logs/stats" + qs({ filter: "data.type = 'request'" })).then(setStats).catch(() => setStats([]));
    api
      .get("/api/logs" + qs({ perPage: 1, filter: `data.status >= 500 && created >= '${new Date(Date.now() - 86400000).toISOString().replace("T", " ")}'` }))
      .then((r) => setErrors(r.totalItems))
      .catch(() => setErrors(0));
    api.get("/api/backups").then(setBackups).catch(() => setBackups([]));
  }, []);

  useEffect(() => {
    let live = true;
    const visible = collections.filter((c) => c.type !== "view").slice(0, 40);
    Promise.all(
      visible.map((c) =>
        api
          .get(`/api/collections/${enc(c.name)}/records` + qs({ perPage: 1, fields: "id" }))
          .then((r) => [c.id, r.totalItems])
          .catch(() => [c.id, null])
      )
    ).then((pairs) => live && setCounts(Object.fromEntries(pairs)));
    return () => {
      live = false;
    };
  }, [collections.length]);

  const user = collections.filter((c) => !c.system);
  const totalRecords = Object.entries(counts)
    .filter(([id]) => user.some((c) => c.id === id))
    .reduce((sum, [, n]) => sum + (n || 0), 0);
  const hourly = buildHours(stats || []);
  const requests = hourly.reduce((s, h) => s + h.total, 0);
  const max = Math.max(1, ...hourly.map((h) => h.total));
  const lastBackup = backups && backups.length ? backups.slice().sort((a, b) => (a.modified < b.modified ? 1 : -1))[0] : null;
  const name = (session.record && session.record.email || "").split("@")[0];

  return html`<div class="page enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><span class="dot live"></span> All systems running</div>
        <h1>${greeting()}${name ? `, ${name}` : ""}</h1>
      </div>
      <${Button} icon="plus" variant="primary" onClick=${() => navigate("/data?new=1")}>New collection</${Button}>
    </div>

    <div class="grid-3 stagger" style="grid-template-columns:repeat(auto-fit,minmax(160px,1fr));margin-bottom:18px">
      ${[
        { label: "Collections", value: loaded ? user.length : undefined, icon: "data", to: "/data" },
        { label: "Records", value: Object.keys(counts).length || (loaded && !user.length) ? totalRecords : undefined, icon: "table", to: "/data" },
        { label: "Requests (24h)", value: stats ? requests : undefined, icon: "chart", to: "/logs" },
        { label: "Server errors (24h)", value: errors ?? undefined, icon: "warning", to: "/logs?filter=" + encodeURIComponent("data.status >= 500"), tone: errors ? "danger" : "" },
      ].map(
        (s, i) => html`<a class="card stat" href=${"#" + s.to} style=${{ "--i": i, color: "inherit", textDecoration: "none" }}>
          <span class="stat-icon" style=${s.tone === "danger" ? "color:var(--danger)" : ""}><${Icon} name=${s.icon} /></span>
          <span class="small muted">${s.label}</span>
          <span class="stat-value">${s.value === undefined ? html`<${Skeleton} width="60px" height=${28} />` : html`<${CountUp} value=${s.value} />`}</span>
        </a>`
      )}
    </div>

    <div class="split-2-1" style="margin-bottom:28px">
      <div class="card pad">
        <div class="row" style="margin-bottom:6px">
          <h3>Traffic</h3><span class="muted small">requests per hour, last 24 hours</span>
          <span class="spacer"></span>
          <a class="small" href="#/logs">Open logs <${Icon} name="arrowRight" size=${13} /></a>
        </div>
        ${stats === null
          ? html`<${Skeleton} height=${140} />`
          : html`<div class="bars" role="img" aria-label=${`${requests} requests in the last 24 hours`}>
              ${hourly.map(
                (h, i) => html`<div
                  class=${"bar tip" + (h.total ? "" : " zero")}
                  data-tip=${`${h.label}: ${h.total}`}
                  style=${{ "--i": i, height: Math.max(3, (h.total / max) * 130) + "px" }}
                ></div>`
              )}
            </div>
            <div class="row tiny faint" style="margin-top:6px;justify-content:space-between">
              <span>${hourly[0].label}</span><span>now</span>
            </div>`}
      </div>
      <div class="card pad stack">
        <h3>Health</h3>
        <div class="stack tight small">
          <div class="row"><span class="dot ok"></span> API and database <span class="spacer"></span><span class="muted">healthy</span></div>
          <div class="row">
            <span class=${"dot " + (lastBackup ? "ok" : "warn")}></span> Last backup
            <span class="spacer"></span>
            <span class="muted">${backups === null ? "…" : lastBackup ? relativeTime(lastBackup.modified) : "never"}</span>
          </div>
          <div class="row">
            <span class=${"dot " + (errors ? "danger" : "ok")}></span> Server errors today
            <span class="spacer"></span><span class="muted">${errors ?? "…"}</span>
          </div>
        </div>
        <span class="spacer"></span>
        <div class="row wrap">
          <${Button} size="sm" icon="archive" onClick=${() => navigate("/settings/backups")}>Backups</${Button}>
          <${Button} size="sm" icon="mail" onClick=${() => navigate("/settings/mail")}>Mail</${Button}>
        </div>
      </div>
    </div>

    <div class="row" style="margin-bottom:12px">
      <h2>Collections</h2>
      <span class="muted small">${loaded ? plural(user.length, "collection") : ""}</span>
    </div>
    ${!loaded
      ? html`<div class="coll-cards">${[0, 1, 2, 3].map(() => html`<div class="card coll-card"><${Skeleton} height=${40} /></div>`)}</div>`
      : user.length === 0
      ? html`<div class="card"><div class="empty">
          <div class="empty-art"><${Icon} name="data" size=${28} /></div>
          <h3>No collections yet</h3>
          <p class="small">Collections hold your records. Create one to define its fields and API rules.</p>
          <${Button} variant="primary" icon="plus" onClick=${() => navigate("/data?new=1")}>Create a collection</${Button}>
        </div></div>`
      : html`<div class="coll-cards stagger">
          ${user.map(
            (c, i) => html`<a class="card coll-card" href=${"#/data/" + enc(c.name)} style=${{ "--i": i }}>
              <${CollectionGlyph} collection=${c} size=${38} />
              <div class="grow">
                <div style="font-weight:600" class="ellipsis">${c.name}</div>
                <div class="small muted">${c.type === "auth" ? "Auth" : c.type === "view" ? "View" : "Base"} · ${counts[c.id] == null ? "…" : plural(counts[c.id], "record")}</div>
              </div>
              <${Icon} name="chevronRight" class="faint" />
            </a>`
          )}
        </div>`}
  </div>`;
}

function buildHours(stats) {
  const byHour = new Map(stats.map((s) => [parseDate(s.date).getTime(), s.total]));
  const now = new Date();
  now.setMinutes(0, 0, 0);
  const out = [];
  for (let i = 23; i >= 0; i--) {
    const t = now.getTime() - i * 3600000;
    const d = new Date(t);
    out.push({ total: byHour.get(t) || 0, label: d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }) });
  }
  return out;
}
