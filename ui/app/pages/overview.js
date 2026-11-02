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
  const [status, setStatus] = useState(null);

  useEffect(() => {
    api.get("/api/logs/stats" + qs({ filter: "data.type = 'request'" })).then(setStats).catch(() => setStats([]));
    api
      .get("/api/logs" + qs({ perPage: 1, filter: `data.status >= 500 && created >= '${new Date(Date.now() - 86400000).toISOString().replace("T", " ")}'` }))
      .then((r) => setErrors(r.totalItems))
      .catch(() => setErrors(0));
    api.get("/api/backups").then(setBackups).catch(() => setBackups([]));
    api.get("/api/system/status").then(setStatus).catch(() => {});
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

  const issues = status ? status.checks.filter((c) => c.status === "warn" || c.status === "error").length : null;
  const largest = Math.max(1, ...user.map((c) => counts[c.id] || 0));
  const today = new Date().toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" });

  return html`<div class="page overview enter">
    <section class="hero card">
      <div class="hero-text">
        <div class="eyebrow"><span class=${"dot " + (issues ? "warn" : "live")}></span> ${issues ? plural(issues, "thing") + " to look at" : "All systems running"} · ${today}</div>
        <h1>${greeting()}${name ? `, ${name}` : ""}</h1>
        <p class="muted">Your PostgreSQL backend at a glance: what's stored, what's asked of it and how it's doing.</p>
        <div class="row wrap hero-actions">
          <${Button} icon="plus" variant="primary" onClick=${() => navigate("/data?new=1")}>New collection</${Button}>
          <${Button} icon="terminal" onClick=${() => navigate("/sql")}>SQL console</${Button}>
          <${Button} icon="archive" variant="ghost" onClick=${() => navigate("/settings/backups")}>Backups</${Button}>
        </div>
      </div>
      <div class="hero-chart">
        <div class="row hero-chart-head">
          <div>
            <div class="small muted">Requests, last 24 hours</div>
            <div class="hero-total">${stats === null ? html`<${Skeleton} width="80px" height=${30} />` : html`<${CountUp} value=${requests} />`}</div>
          </div>
          <span class="spacer"></span>
          <a class="small" href="#/logs">Open logs <${Icon} name="arrowRight" size=${13} /></a>
        </div>
        ${stats === null ? html`<${Skeleton} height=${150} />` : html`<${AreaChart} hours=${hourly} max=${max} total=${requests} />`}
      </div>
    </section>

    <div class="metrics stagger">
      ${[
        { label: "Collections", value: loaded ? user.length : undefined, icon: "data", to: "/data", color: "var(--tone-1)" },
        { label: "Records", value: Object.keys(counts).length || (loaded && !user.length) ? totalRecords : undefined, icon: "table", to: "/data", color: "var(--tone-2)" },
        { label: "Server errors (24h)", value: errors ?? undefined, icon: "warning", to: "/logs?filter=" + encodeURIComponent("data.status >= 500"), color: errors ? "var(--danger)" : "var(--tone-3)" },
        { label: "Database round trip", value: status ? status.database.latencyMs : undefined, suffix: " ms", icon: "server", to: "/health", color: "var(--tone-4)", raw: true },
      ].map(
        (s, i) => html`<a class="card metric" href=${"#" + s.to} style=${{ "--i": i, "--c": s.color }}>
          <span class="metric-icon"><${Icon} name=${s.icon} size=${17} /></span>
          <span class="metric-label">${s.label}</span>
          <span class="metric-value">${s.value === undefined ? html`<${Skeleton} width="60px" height=${26} />` : s.raw ? s.value + (s.suffix || "") : html`<${CountUp} value=${s.value} />`}</span>
        </a>`
      )}
    </div>

    <div class="overview-grid">
      <section class="card">
        <div class="card-head">
          <h3 class="grow">Collections</h3>
          <span class="muted small">${loaded ? plural(user.length, "collection") : ""}</span>
          <a class="small" href="#/diagram">Diagram <${Icon} name="arrowRight" size=${13} /></a>
        </div>
        ${!loaded
          ? html`<div class="card-body stack">${[0, 1, 2].map(() => html`<${Skeleton} height=${34} />`)}</div>`
          : user.length === 0
          ? html`<div class="empty">
              <div class="empty-art"><${Icon} name="data" size=${28} /></div>
              <h3>No collections yet</h3>
              <p class="small">Collections hold your records. Create one to define its fields and API rules.</p>
              <${Button} variant="primary" icon="plus" onClick=${() => navigate("/data?new=1")}>Create a collection</${Button}>
            </div>`
          : html`<div class="coll-rows stagger">
              ${user
                .slice()
                .sort((a, b) => (counts[b.id] || 0) - (counts[a.id] || 0))
                .map(
                  (c, i) => html`<a class="coll-row" href=${"#/data/" + enc(c.name)} style=${{ "--i": i }}>
                    <${CollectionGlyph} collection=${c} size=${32} />
                    <div class="coll-row-name">
                      <div class="ellipsis">${c.name}</div>
                      <div class="tiny muted">${c.type === "auth" ? "Auth" : c.type === "view" ? "View" : "Base"} · ${plural(c.fields.length, "field")}</div>
                    </div>
                    <div class="coll-row-bar" aria-hidden="true"><span style=${{ width: Math.max(2, ((counts[c.id] || 0) / largest) * 100) + "%" }}></span></div>
                    <div class="coll-row-count">${counts[c.id] == null ? "…" : (counts[c.id] || 0).toLocaleString()}</div>
                    <${Icon} name="chevronRight" size=${16} class="faint" />
                  </a>`
                )}
            </div>`}
      </section>

      <section class="card">
        <div class="card-head">
          <h3 class="grow">Health</h3>
          <a class="small" href="#/health">Details <${Icon} name="arrowRight" size=${13} /></a>
        </div>
        <div class="checklist">
          <${Check} ok=${true} title="API and database" detail=${status ? status.database.latencyMs + " ms round trip" : "answering"} />
          <${Check}
            ok=${issues === 0}
            title="Checks"
            detail=${issues === null ? "…" : issues ? plural(issues, "thing") + " to look at" : "all good"}
            href="#/health"
          />
          <${Check}
            ok=${!!lastBackup}
            title="Last backup"
            detail=${backups === null ? "…" : lastBackup ? relativeTime(lastBackup.modified) : "none yet"}
            href="#/settings/backups"
          />
          <${Check} ok=${!errors} danger=${!!errors} title="Server errors today" detail=${errors ?? "…"} href="#/logs" />
        </div>
      </section>
    </div>
  </div>`;
}

function Check({ ok, danger, title, detail, href }) {
  const body = html`<span class=${"check-dot " + (ok ? "ok" : danger ? "danger" : "warn")}>
      <${Icon} name=${ok ? "check" : "warning"} size=${13} />
    </span>
    <span class="grow">${title}</span>
    <span class="muted small">${detail}</span>`;
  return href ? html`<a class="check-row" href=${href}>${body}</a>` : html`<div class="check-row">${body}</div>`;
}

// Requests per hour as a smooth area, with the hour under the pointer.
function AreaChart({ hours, max, total }) {
  const [hover, setHover] = useState(null);
  const W = 600;
  const H = 150;
  const step = W / (hours.length - 1);
  const pts = hours.map((h, i) => [i * step, H - 6 - (h.total / max) * (H - 22)]);
  // a Catmull-Rom curve through the points, as cubic Béziers
  let line = `M${pts[0][0]},${pts[0][1]}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const [p0, p1, p2, p3] = [pts[i - 1] || pts[i], pts[i], pts[i + 1], pts[i + 2] || pts[i + 1]];
    const c1 = [p1[0] + (p2[0] - p0[0]) / 6, Math.min(H - 6, p1[1] + (p2[1] - p0[1]) / 6)];
    const c2 = [p2[0] - (p3[0] - p1[0]) / 6, Math.min(H - 6, p2[1] - (p3[1] - p1[1]) / 6)];
    line += ` C${c1[0]},${c1[1]} ${c2[0]},${c2[1]} ${p2[0]},${p2[1]}`;
  }
  const area = `${line} L${W},${H} L0,${H} Z`;
  const h = hover === null ? null : hours[hover];
  return html`<div class="area-chart" role="img" aria-label=${`${total} requests in the last 24 hours`} onPointerLeave=${() => setHover(null)}>
    <svg viewBox=${`0 0 ${W} ${H}`} preserveAspectRatio="none">
      <defs>
        <linearGradient id="area-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="var(--accent)" stop-opacity="0.28" />
          <stop offset="100%" stop-color="var(--accent)" stop-opacity="0" />
        </linearGradient>
      </defs>
      <path d=${area} fill="url(#area-fill)" />
      <path d=${line} fill="none" stroke="var(--accent)" stroke-width="2" vector-effect="non-scaling-stroke" stroke-linejoin="round" />
    </svg>
    <div class="area-cols">
      ${hours.map((_, i) => html`<span onPointerEnter=${() => setHover(i)} class=${hover === i ? "on" : ""}></span>`)}
    </div>
    ${h &&
    html`<div class="area-tip" style=${{ left: `${(hover / (hours.length - 1)) * 100}%` }}>
      <b>${h.total.toLocaleString()}</b> <span class="muted">at ${h.label}</span>
    </div>`}
    <div class="row tiny faint area-axis"><span>${hours[0].label}</span><span>${hours[12].label}</span><span>now</span></div>
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
