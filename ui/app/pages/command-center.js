// The command center (the main installation's Projects page): every installation side by side,
// live. Fleet totals, requests per minute for each, a card (or a row) per installation with its
// numbers and configuration, and a drawer to look closer, change a project's settings, its host
// names, restart or delete it. Projects manage themselves from their own dashboards; only the
// main installation sees and controls them all.
import { html, useState, useEffect, useRef, cx } from "../lib/core.js";
import { api, enc, dashboardUrl } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Field, Input, Switch, Modal, SheetHead, Callout, CopyButton, Skeleton, Menu, Segmented, Tabs, Empty, toast, tone } from "../lib/ui.js";
import { bytes, plural, formatDate, relativeTime } from "../lib/format.js";

const POLL_MS = 5000;
const VIEW_KEY = "slonbase.projects.view";

const STATUS = {
  ready: ["Running", "ok"],
  starting: ["Starting", "warn"],
  setup: ["Needs setup", "warn"],
  degraded: ["Degraded", "danger"],
};

// distinct colours: the main installation keeps the accent, projects take the tones in name
// order (from the second, the first being close to the accent), so no two of eight look alike
const palette = new Map();
function assignColors(items) {
  palette.clear();
  items
    .filter((i) => !i.main)
    .map((i) => i.name)
    .sort()
    .forEach((name, n) => palette.set(name, `var(--tone-${((n + 1) % 8) + 1})`));
}
const colorOf = (item) => (item.main ? "var(--accent)" : palette.get(item.name) || tone(item.name));
const num = (n) => (n == null ? "—" : Number(n).toLocaleString());
const ms = (n) => (n == null ? "—" : `${n < 10 ? n.toFixed(1) : Math.round(n)} ms`);
const sum = (list, f) => list.reduce((s, x) => s + (f(x) || 0), 0);
const lastMinute = (item) => (item.traffic ? item.traffic.perMinute[59] + item.traffic.perMinute[58] : 0) / 2;

function load(key, fallback) {
  try {
    return localStorage.getItem(key) || fallback;
  } catch (_) {
    return fallback;
  }
}

// ------------------------------------------------------------------ charts

// lines over shared minutes (oldest left), one per series; hovering shows each one's value
function LiveChart({ series, points = 60, height = 190, unit = "requests" }) {
  const [hover, setHover] = useState(null);
  const W = 800;
  const H = height;
  const top = Math.max(4, ...series.flatMap((s) => s.values));
  const nice = Math.pow(10, Math.floor(Math.log10(top)));
  const max = Math.ceil(top / nice) * nice;
  const x = (i) => (i / (points - 1)) * W;
  const y = (v) => H - 4 - (v / max) * (H - 16);
  const path = (values) => values.map((v, i) => `${i ? "L" : "M"}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  return html`<div class="cc-chart" style=${{ height: H + "px" }} role="img" aria-label=${`${unit} per minute over the last hour`} onPointerLeave=${() => setHover(null)}>
    <svg viewBox=${`0 0 ${W} ${H}`} preserveAspectRatio="none">
      ${[0.25, 0.5, 0.75, 1].map((f) => html`<line class="cc-gridline" x1="0" x2=${W} y1=${y(max * f)} y2=${y(max * f)} />`)}
      ${series.map(
        (s) => html`<path d=${`${path(s.values)} L${W},${H} L0,${H} Z`} fill=${s.color} opacity="0.06" />
          <path d=${path(s.values)} fill="none" stroke=${s.color} stroke-width="1.75" vector-effect="non-scaling-stroke" stroke-linejoin="round" />`
      )}
      ${hover !== null && html`<line class="cc-cursor" x1=${x(hover)} x2=${x(hover)} y1="0" y2=${H} />`}
    </svg>
    <div class="cc-chart-max tiny faint mono">${num(max)}/min</div>
    <div class="cc-chart-cols">${Array.from({ length: points }, (_, i) => html`<span onPointerEnter=${() => setHover(i)}></span>`)}</div>
    ${hover !== null &&
    html`<div class=${cx("cc-tip", { left: hover > points * 0.6 })} style=${{ left: `${(hover / (points - 1)) * 100}%` }}>
      <div class="tiny faint mono">${points - 1 - hover === 0 ? "this minute" : `${points - 1 - hover} min ago`}</div>
      ${series
        .slice()
        .sort((a, b) => b.values[hover] - a.values[hover])
        .slice(0, 8)
        .map((s) => html`<div class="cc-tip-row"><i style=${{ background: s.color }}></i><span class="grow">${s.name}</span><b class="mono">${num(s.values[hover])}</b></div>`)}
    </div>`}
  </div>`;
}

function Sparkline({ values, color }) {
  const W = 120;
  const H = 30;
  const top = Math.max(1, ...values);
  const pts = values.map((v, i) => `${((i / (values.length - 1)) * W).toFixed(1)},${(H - 2 - (v / top) * (H - 4)).toFixed(1)}`);
  return html`<svg class="cc-spark" viewBox=${`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-hidden="true">
    <polyline points=${`0,${H} ${pts.join(" ")} ${W},${H}`} fill=${color} opacity="0.12" />
    <polyline points=${pts.join(" ")} fill="none" stroke=${color} stroke-width="1.5" vector-effect="non-scaling-stroke" />
  </svg>`;
}

// 24 hourly bars
function HourBars({ values, color }) {
  const top = Math.max(1, ...values);
  return html`<div class="cc-hours" role="img" aria-label="Requests per hour, the last 24 hours">
    ${values.map((v, i) => html`<span title=${`${num(v)} requests, ${23 - i}h ago`} style=${{ height: `${Math.max(2, (v / top) * 100)}%`, background: color }}></span>`)}
  </div>`;
}

// a bar split between the installations (traffic or storage)
function Split({ items, value, format, label }) {
  const total = sum(items, value);
  return html`<div class="cc-split">
    <div class="row"><span class="section-title grow">${label}</span><span class="tiny mono faint">${format(total)}</span></div>
    <div class="cc-split-bar" role="img" aria-label=${label}>
      ${total > 0 &&
      items
        .filter((i) => value(i) > 0)
        .map((i) => html`<span style=${{ flex: value(i), background: colorOf(i) }} title=${`${i.name}: ${format(value(i))}`}></span>`)}
    </div>
    <div class="cc-split-legend">
      ${items
        .slice()
        .sort((a, b) => (value(b) || 0) - (value(a) || 0))
        .slice(0, 6)
        .map((i) => html`<span><i style=${{ background: colorOf(i) }}></i>${i.name} <b class="mono">${total ? Math.round(((value(i) || 0) / total) * 100) : 0}%</b></span>`)}
    </div>
  </div>`;
}

function Meter({ value, max, label }) {
  const pct = max ? Math.min(100, (value / max) * 100) : 0;
  return html`<div class="cc-meter" role="meter" aria-label=${label} aria-valuenow=${value} aria-valuemax=${max}>
    <span style=${{ width: pct + "%" }} class=${cx({ hot: pct >= 80 })}></span>
  </div>`;
}

// ------------------------------------------------------------------ the page

export function CommandCenter({ onCreate, onDelete, onHosts, version }) {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [live, setLive] = useState(true);
  const [view, setView] = useState(() => load(VIEW_KEY, "cards"));
  const [filter, setFilter] = useState("all");
  const [query, setQuery] = useState("");
  const [hidden, setHidden] = useState({});
  const [selected, setSelected] = useState(null);
  const [updated, setUpdated] = useState(null);
  const busy = useRef(false);

  const refresh = async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const res = await api.get("/api/projects/overview");
      assignColors(res.items);
      setData(res.items);
      setUpdated(new Date());
      setError(null);
    } catch (err) {
      setError(err);
    } finally {
      busy.current = false;
    }
  };
  useEffect(() => {
    refresh();
  }, [version]);
  useEffect(() => {
    if (!live) return;
    const timer = setInterval(() => !document.hidden && refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [live]);

  const items = data || [];
  const projects = items.filter((i) => !i.main);
  const issues = (i) => i.status !== "ready" || (i.traffic && i.traffic.errors > 0);
  const shown = items.filter((i) => {
    if (query && !`${i.name} ${i.database} ${i.server} ${(i.hosts || []).join(" ")}`.toLowerCase().includes(query.toLowerCase())) return false;
    if (filter === "issues") return issues(i);
    if (filter === "external") return i.external;
    return true;
  });
  const ready = items.filter((i) => i.status === "ready").length;
  const servers = new Set(items.map((i) => i.server)).size;
  const selectedItem = selected && items.find((i) => i.name === selected);
  const series = items.filter((i) => i.traffic && !hidden[i.name]).map((i) => ({ name: i.name, color: colorOf(i), values: i.traffic.perMinute }));
  const slowest = items.reduce((m, i) => (i.traffic && i.traffic.p95Ms > (m ? m.traffic.p95Ms : -1) ? i : m), null);

  return html`<div class="page cc enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><${Icon} name="layers" size=${14} /> Command center</div>
        <h1>Projects</h1>
      </div>
      <div class="row cc-head-actions">
        <button class=${cx("cc-live", { on: live })} onClick=${() => setLive(!live)} aria-pressed=${live} title=${live ? "Pause the updates" : "Update every 5 seconds"}>
          <span class="cc-live-dot"></span>${live ? "Live" : "Paused"}
          ${updated && html`<span class="faint mono">${updated.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</span>`}
        </button>
        <${IconButton} icon="refresh" label="Refresh now" onClick=${refresh} />
        <${Button} variant="primary" icon="plus" onClick=${onCreate}>New project</${Button}>
      </div>
    </div>
    ${error && html`<div style="margin-bottom:14px"><${Callout} kind="danger">Couldn't load the overview: ${error.message}</${Callout}></div>`}

    <div class="cc-tiles">
      <${Tile} label="Installations" value=${data ? `${ready}/${items.length}` : null} sub=${data && `${plural(projects.length, "project")} · ${plural(servers, "server")}`} tone=${ready < items.length ? "warn" : "ok"} />
      <${Tile} label="Requests, 24h" value=${data && num(sum(items, (i) => i.traffic && i.traffic.requests))} sub=${data && `${num(Math.round(sum(items, lastMinute)))}/min now`} />
      <${Tile} label="Server errors, 24h" value=${data && num(sum(items, (i) => i.traffic && i.traffic.errors))} tone=${sum(items, (i) => i.traffic && i.traffic.errors) ? "danger" : ""} sub=${data && `${items.filter((i) => i.traffic && i.traffic.errors).length} with errors`} />
      <${Tile} label="Slowest p95" value=${slowest ? ms(slowest.traffic.p95Ms) : data && "—"} sub=${slowest && slowest.name} />
      <${Tile} label="Data" value=${data && bytes(sum(items, (i) => i.db && i.db.sizeBytes))} sub=${data && `${num(sum(items, (i) => i.records))} records`} />
      <${Tile} label="Realtime" value=${data && num(sum(items, (i) => i.realtimeClients))} sub="clients connected" />
    </div>

    <div class="cc-overview">
      <div class="card cc-traffic">
        <div class="card-head">
          <h3 class="grow">Requests per minute</h3>
          <span class="tiny faint">last hour, every installation</span>
        </div>
        <div class="card-body">
          ${data ? html`<${LiveChart} series=${series} />` : html`<${Skeleton} height=${190} />`}
          <div class="cc-legend">
            ${items.map(
              (i) => html`<button class=${cx("cc-legend-item", { off: hidden[i.name] })} onClick=${() => setHidden({ ...hidden, [i.name]: !hidden[i.name] })} aria-pressed=${!hidden[i.name]}>
                <i style=${{ background: colorOf(i) }}></i>${i.name}
              </button>`
            )}
          </div>
        </div>
      </div>
      <div class="card cc-splits">
        <div class="card-head"><h3 class="grow">Where it goes</h3></div>
        <div class="card-body stack">
          ${data
            ? html`<${Split} label="Traffic, 24h" items=${items} value=${(i) => i.traffic && i.traffic.requests} format=${(n) => num(n) + " req"} />
                <${Split} label="Storage" items=${items} value=${(i) => i.db && i.db.sizeBytes} format=${bytes} />
                <${Split} label="Records" items=${items} value=${(i) => i.records} format=${num} />`
            : html`<${Skeleton} height=${150} />`}
        </div>
      </div>
    </div>

    <div class="cc-toolbar">
      <${Segmented}
        value=${filter}
        onChange=${setFilter}
        options=${[
          { value: "all", label: `All ${items.length}` },
          { value: "issues", label: `Needs attention ${items.filter(issues).length}` },
          { value: "external", label: `Other servers ${items.filter((i) => i.external).length}` },
        ]}
      />
      <label class="tree-filter cc-search">
        <${Icon} name="search" size=${14} />
        <input placeholder="Filter by name, database, server or host" value=${query} onInput=${(e) => setQuery(e.target.value)} aria-label="Filter installations" />
      </label>
      <span class="spacer"></span>
      <${Segmented}
        value=${view}
        onChange=${(v) => {
          setView(v);
          try {
            localStorage.setItem(VIEW_KEY, v);
          } catch (_) {}
        }}
        options=${[
          { value: "cards", label: "Cards" },
          { value: "table", label: "Table" },
        ]}
      />
    </div>

    ${!data
      ? html`<div class="cc-grid">${[0, 1, 2].map(() => html`<div class="card pad stack"><${Skeleton} width="40%" height=${16} /><${Skeleton} height=${40} /><${Skeleton} width="70%" /></div>`)}</div>`
      : shown.length === 0
        ? html`<div class="card"><${Empty} icon="layers" title=${projects.length ? "Nothing matches" : "No projects yet"} action=${!projects.length && html`<${Button} variant="primary" icon="plus" onClick=${onCreate}>Create a project</${Button}>`}>
            ${projects.length ? "Change the filter to see more." : "Host several apps from this installation: each gets its own database (here or on another PostgreSQL server), superusers, settings, files and hooks."}
          </${Empty}></div>`
        : view === "table"
          ? html`<${Table} items=${shown} onOpen=${setSelected} />`
          : html`<div class="cc-grid">${shown.map((i) => html`<${Card} key=${i.name} item=${i} onOpen=${() => setSelected(i.name)} onHosts=${() => onHosts(i)} onDelete=${() => onDelete(i)} />`)}</div>`}

    ${selectedItem && html`<${Drawer} item=${selectedItem} onClose=${() => setSelected(null)} onHosts=${() => onHosts(selectedItem)} onDelete=${() => (setSelected(null), onDelete(selectedItem))} onChanged=${refresh} />`}
  </div>`;
}

function Tile({ label, value, sub, tone }) {
  return html`<div class=${cx("card cc-tile", tone)}>
    <span class="metric-label">${label}</span>
    <span class="cc-tile-value">${value == null ? html`<${Skeleton} width="60%" height=${22} />` : value}</span>
    <span class="tiny faint ellipsis">${sub || " "}</span>
  </div>`;
}

function StatusChip({ item }) {
  const [label, chipTone] = STATUS[item.status] || [item.status, "outline"];
  return html`<span class=${`chip ${chipTone}`} title=${item.statusDetail || undefined}><span class="cc-dot"></span>${label}</span>`;
}

function ServerBadge({ item }) {
  return html`<span class=${cx("cc-server mono", { external: item.external })} title=${`${item.server}/${item.database}: ${item.external ? "a connected database" : "on the main installation's server"}`}>
    <${Icon} name="server" size=${12} />${item.server}<span class="faint">/${item.database}</span>
  </span>`;
}

function ConfigChips({ config }) {
  if (!config) return null;
  const chip = (on, label, title) => html`<span class=${cx("cc-cfg", { on })} title=${title}><${Icon} name=${on ? "check" : "minus"} size=${11} />${label}</span>`;
  return html`<div class="cc-cfgs">
    ${chip(config.smtp, "Mail", config.smtp ? `SMTP: ${config.smtpHost}` : "Emails aren't sent (no SMTP)")}
    ${chip(config.s3, config.s3 ? "S3 files" : "Local files", config.s3 ? "Files in an S3 bucket" : "Files on this server")}
    ${chip(!!config.backupsCron, config.backupsCron ? "Backups" : "No backups", config.backupsCron ? `Backups at ${config.backupsCron}, keeping ${config.backupsKeep}` : "No automatic backups")}
    ${chip(config.logsDays > 0, config.logsDays > 0 ? "Logs" : "No logs", config.logsDays > 0 ? `Request logs kept ${config.logsDays} day(s)` : "Request logging is off: traffic isn't counted")}
    ${chip(config.rateLimits, "Rate limits", config.rateLimits ? "Rate limits on" : "Rate limits off")}
    ${chip(config.settingsEncrypted, "Encrypted", config.settingsEncrypted ? "Settings encrypted at rest" : "Settings stored plain")}
  </div>`;
}

function Card({ item, onOpen, onHosts, onDelete }) {
  const t = item.traffic;
  const color = colorOf(item);
  return html`<article class=${cx("card cc-card", { down: item.status !== "ready" })} style=${{ "--cc": color }}>
    <header class="cc-card-head">
      <span class="coll-glyph" style=${{ "--c": color }}>${item.main ? html`<${Icon} name="server" size=${14} />` : item.name.slice(0, 1)}</span>
      <div class="grow" style="min-width:0">
        <button class="cc-card-name" onClick=${onOpen}>${item.name}${item.main && html`<span class="chip outline">main</span>`}</button>
        <${ServerBadge} item=${item} />
      </div>
      <${StatusChip} item=${item} />
      <${Menu}
        trigger=${(toggle) => html`<${IconButton} icon="more" label="More" size="sm" onClick=${toggle} />`}
        items=${[
          { icon: "chart", label: "Details", onClick: onOpen },
          { icon: "external", label: "Open dashboard", onClick: () => (location.href = item.main ? dashboardUrl(null) : dashboardUrl(item.name)) },
          !item.main && { icon: "globe", label: "Host names…", onClick: onHosts },
          !item.main && "-",
          !item.main && { icon: "trash", label: "Delete project", danger: true, onClick: onDelete },
        ]}
      />
    </header>
    ${item.status !== "ready" && item.statusDetail && html`<div class="cc-card-alert tiny">${item.statusDetail}</div>`}
    <button class="cc-card-live" onClick=${onOpen} aria-label=${`${item.name}: details`}>
      ${t ? html`<${Sparkline} values=${t.perMinute} color=${color} />` : html`<div class="cc-spark"></div>`}
      <span class="cc-card-rate"><b class="mono">${t ? num(Math.round(lastMinute(item))) : "—"}</b><span class="tiny faint">req/min</span></span>
    </button>
    <dl class="cc-stats">
      <div><dt>Req 24h</dt><dd>${t ? num(t.requests) : "—"}</dd></div>
      <div class=${cx({ bad: t && t.errors })}><dt>Errors</dt><dd>${t ? num(t.errors) : "—"}</dd></div>
      <div><dt>p95</dt><dd>${t ? ms(t.p95Ms) : "—"}</dd></div>
      <div><dt>Records</dt><dd title=${item.recordsEstimated ? "Large tables are counted from PostgreSQL's estimate" : undefined}>${item.recordsEstimated ? "~" : ""}${num(item.records)}</dd></div>
    </dl>
    <div class="cc-res">
      <div class="cc-res-row"><span class="tiny faint">Connections</span><span class="tiny mono">${item.pool ? `${item.pool.inUse}/${item.pool.max}` : "—"}</span></div>
      <${Meter} value=${item.pool ? item.pool.inUse : 0} max=${item.pool ? item.pool.max : 1} label="Connections in use" />
      <div class="cc-res-row"><span class="tiny faint">${item.db ? `PostgreSQL ${item.db.version} · ${ms(item.db.latencyMs)}` : "Database"}</span><span class="tiny mono">${item.db ? bytes(item.db.sizeBytes) : "—"}</span></div>
    </div>
    <${ConfigChips} config=${item.config} />
  </article>`;
}

const COLUMNS = [
  ["name", "Installation", (i) => i.name],
  ["status", "Status", (i) => i.status],
  ["rate", "Req/min", (i) => lastMinute(i)],
  ["requests", "Requests 24h", (i) => i.traffic && i.traffic.requests],
  ["errors", "Errors", (i) => i.traffic && i.traffic.errors],
  ["p95", "p95", (i) => i.traffic && i.traffic.p95Ms],
  ["size", "Size", (i) => i.db && i.db.sizeBytes],
  ["records", "Records", (i) => i.records],
  ["pool", "Connections", (i) => i.pool && i.pool.inUse],
];

function Table({ items, onOpen }) {
  const [sort, setSort] = useState(["requests", -1]);
  const [key, dir] = sort;
  const get = COLUMNS.find((c) => c[0] === key)[2];
  const rows = items.slice().sort((a, b) => {
    const [x, y] = [get(a), get(b)];
    return (x > y ? 1 : x < y ? -1 : 0) * dir;
  });
  return html`<div class="card cc-table-wrap">
    <table class="grid cc-table">
      <thead>
        <tr>
          ${COLUMNS.map(
            ([k, label]) => html`<th class="sortable" aria-sort=${k === key ? (dir > 0 ? "ascending" : "descending") : "none"}>
              <button class="cc-th" onClick=${() => setSort([k, k === key ? -dir : -1])}>${label}${k === key && html`<${Icon} name=${dir > 0 ? "sortAsc" : "sortDesc"} size=${12} />`}</button>
            </th>`
          )}
          <th>Last hour</th>
          <th>Server</th>
        </tr>
      </thead>
      <tbody>
        ${rows.map(
          (i) => html`<tr onClick=${() => onOpen(i.name)} class="cc-row">
            <td><span class="row" style="gap:8px"><i class="cc-swatch" style=${{ background: colorOf(i) }}></i><b>${i.name}</b>${i.main && html`<span class="chip outline">main</span>`}</span></td>
            <td><${StatusChip} item=${i} /></td>
            <td class="mono num">${num(Math.round(lastMinute(i)))}</td>
            <td class="mono num">${i.traffic ? num(i.traffic.requests) : "—"}</td>
            <td class=${cx("mono num", { "cc-bad": i.traffic && i.traffic.errors })}>${i.traffic ? num(i.traffic.errors) : "—"}</td>
            <td class="mono num">${i.traffic ? ms(i.traffic.p95Ms) : "—"}</td>
            <td class="mono num">${i.db ? bytes(i.db.sizeBytes) : "—"}</td>
            <td class="mono num">${num(i.records)}</td>
            <td class="mono num">${i.pool ? `${i.pool.inUse}/${i.pool.max}` : "—"}</td>
            <td style="width:130px">${i.traffic && html`<${Sparkline} values=${i.traffic.perMinute} color=${colorOf(i)} />`}</td>
            <td class="mono tiny">${i.server}${i.external && html` <span class="chip outline">other</span>`}</td>
          </tr>`
        )}
      </tbody>
    </table>
  </div>`;
}

// ------------------------------------------------------------------ the drawer

function Drawer({ item, onClose, onHosts, onDelete, onChanged }) {
  const [tab, setTab] = useState("overview");
  const [restarting, setRestarting] = useState(false);
  const t = item.traffic;
  const color = colorOf(item);
  const url = location.origin + (item.main ? "" : `/p/${item.name}`);
  const restart = async () => {
    setRestarting(true);
    try {
      await api.post(`/api/projects/${enc(item.name)}/restart`);
      toast(`Restarted ${item.name}`);
      onChanged();
    } catch (err) {
      toast.error(err);
    } finally {
      setRestarting(false);
    }
  };
  const tabs = [
    { id: "overview", label: "Overview", icon: "chart" },
    { id: "settings", label: "Settings", icon: "settings" },
    { id: "connection", label: "Connection", icon: "data" },
  ];
  return html`<${Modal} size="drawer" onClose=${onClose}>
    ${(close) => html`<${SheetHead} title=${item.name} subtitle=${item.main ? "The main installation" : `Project · ${item.server}/${item.database}`} icon=${item.main ? "server" : "layers"} onClose=${close}>
        <${StatusChip} item=${item} />
      </${SheetHead}>
      <div class="sheet-body cc-drawer" style=${{ "--cc": color }}>
        <div class="row" style="gap:8px;flex-wrap:wrap">
          <a class="btn sm" href=${item.main ? dashboardUrl(null) : dashboardUrl(item.name)}><${Icon} name="external" size=${14} /> Open dashboard</a>
          ${!item.main && html`<${Button} size="sm" icon="refresh" loading=${restarting} onClick=${restart}>Restart</${Button}>`}
          ${!item.main && html`<${Button} size="sm" icon="globe" onClick=${onHosts}>Host names</${Button}>`}
          <span class="spacer"></span>
          ${!item.main && html`<${Button} size="sm" variant="danger-ghost" icon="trash" onClick=${onDelete}>Delete</${Button}>`}
        </div>
        <${Tabs} tabs=${tabs} active=${tab} onChange=${setTab} />
        ${tab === "overview" &&
        html`<div class="stack">
          ${item.statusDetail && html`<${Callout} kind=${item.status === "ready" ? "info" : "warn"}>${item.statusDetail}</${Callout}>`}
          <div class="cc-drawer-stats">
            <div><span class="metric-label">Req/min</span><b>${t ? num(Math.round(lastMinute(item))) : "—"}</b></div>
            <div><span class="metric-label">Requests 24h</span><b>${t ? num(t.requests) : "—"}</b></div>
            <div><span class="metric-label">Server errors</span><b class=${cx({ "cc-bad": t && t.errors })}>${t ? num(t.errors) : "—"}</b></div>
            <div><span class="metric-label">Avg · p95</span><b>${t ? `${ms(t.avgMs)} · ${ms(t.p95Ms)}` : "—"}</b></div>
          </div>
          ${t &&
          html`<div class="stack tight">
            <span class="section-title">Requests per minute, last hour</span>
            <${LiveChart} series=${[{ name: item.name, color, values: t.perMinute }]} height=${150} />
            <span class="section-title" style="margin-top:8px">Requests per hour, last 24 hours</span>
            <${HourBars} values=${t.perHour} color=${color} />
          </div>`}
          <dl class="cc-facts">
            <dt>Collections</dt><dd>${num(item.collections)}</dd>
            <dt>Records</dt><dd>${num(item.records)}</dd>
            <dt>Database size</dt><dd>${item.db ? bytes(item.db.sizeBytes) : "—"}</dd>
            <dt>Round trip</dt><dd>${item.db ? ms(item.db.latencyMs) : "—"}</dd>
            <dt>Connections</dt><dd>${item.pool ? `${item.pool.inUse} in use, ${item.pool.size} open, ${item.pool.max} at most` : "—"}</dd>
            <dt>Realtime</dt><dd>${plural(item.realtimeClients || 0, "client")}</dd>
          </dl>
          <div class="stack tight"><span class="section-title">Configuration</span><${ConfigChips} config=${item.config} /></div>
        </div>`}
        ${tab === "settings" &&
        (item.main
          ? html`<${Callout}>The main installation's settings are on its own <a href="#/settings">Settings</a> page.</${Callout}>`
          : item.status !== "ready"
            ? html`<${Callout} kind="warn">The project isn't running, so its settings can't be read.</${Callout}>`
            : html`<${ProjectSettings} name=${item.name} onSaved=${onChanged} />`)}
        ${tab === "connection" &&
        html`<div class="stack">
          <dl class="cc-facts">
            <dt>Server</dt><dd class="mono">${item.server}${item.external ? html` <span class="chip outline">connected</span>` : html` <span class="chip outline">main server</span>`}</dd>
            <dt>Database</dt><dd class="mono">${item.database}</dd>
            ${item.db && html`<dt>PostgreSQL</dt><dd class="mono">${item.db.version}</dd>`}
            ${!item.main && html`<dt>Role</dt><dd class="mono">${item.role || (item.external ? "the URL's" : "the main installation's")}</dd>`}
            ${item.created && html`<dt>Created</dt><dd><span title=${formatDate(item.created)}>${relativeTime(item.created)}</span></dd>`}
            ${item.hosts && item.hosts.length > 0 && html`<dt>Hosts</dt><dd class="mono">${item.hosts.join(", ")}</dd>`}
          </dl>
          <div class="field"><span class="label">API</span><div class="endpoint"><span class="endpoint-method">REST</span><span class="endpoint-url">${url}/api/</span><${CopyButton} text=${url + "/api/"} label="Copy the API URL" /></div></div>
          <div class="field"><span class="label">SDK</span><div class="endpoint"><span class="endpoint-url">new PocketBase("${url}")</span><${CopyButton} text=${`new PocketBase("${url}")`} label="Copy" /></div></div>
          ${item.external &&
          html`<${Callout}>This project's database was connected by URL, not created by slonbase. Deleting the project leaves it as it is. The URL (with its password) is stored encrypted when the settings encryption key is set.</${Callout}>`}
        </div>`}
      </div>`}
  </${Modal}>`;
}

// the settings a superuser of the main installation may change for a project
function ProjectSettings({ name, onSaved }) {
  const [form, setForm] = useState(null);
  const [initial, setInitial] = useState(null);
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api
      .get(`/api/projects/${enc(name)}/settings`)
      .then((s) => {
        const f = {
          appName: s.meta.appName,
          appURL: s.meta.appURL,
          senderName: s.meta.senderName,
          senderAddress: s.meta.senderAddress,
          hideControls: s.meta.hideControls,
          smtpEnabled: s.smtp.enabled,
          smtpHost: s.smtp.host,
          smtpPort: s.smtp.port,
          smtpUsername: s.smtp.username,
          smtpPassword: "",
          smtpTls: s.smtp.tls,
          backupsCron: s.backups.cron,
          backupsKeep: s.backups.cronMaxKeep,
          logsDays: s.logs.maxDays,
          rateLimits: s.rateLimits.enabled,
        };
        setForm(f);
        setInitial(f);
      })
      .catch((err) => setErrors({ form: err.message }));
  }, [name]);
  if (!form) return errors.form ? html`<${Callout} kind="danger">${errors.form}</${Callout}>` : html`<${Skeleton} height=${200} />`;
  const set = (k) => (v) => setForm({ ...form, [k]: v });
  const changed = Object.keys(form).filter((k) => form[k] !== initial[k]);
  const save = async (e) => {
    e.preventDefault();
    setBusy(true);
    setErrors({});
    try {
      await api.patch(`/api/projects/${enc(name)}/settings`, {
        meta: { appName: form.appName, appURL: form.appURL, senderName: form.senderName, senderAddress: form.senderAddress, hideControls: form.hideControls },
        smtp: { enabled: form.smtpEnabled, host: form.smtpHost, port: Number(form.smtpPort) || 0, username: form.smtpUsername, password: form.smtpPassword, tls: form.smtpTls },
        backups: { cron: form.backupsCron, cronMaxKeep: Number(form.backupsKeep) || 0 },
        logs: { maxDays: Number(form.logsDays) || 0 },
        rateLimits: { enabled: form.rateLimits },
      });
      const next = { ...form, smtpPassword: "" };
      setForm(next);
      setInitial(next);
      toast(`Saved ${name}'s settings`);
      onSaved();
    } catch (err) {
      const fields = err.fieldErrors ? err.fieldErrors() : {};
      setErrors(Object.keys(fields).length ? fields : { form: err.message });
    } finally {
      setBusy(false);
    }
  };
  return html`<form class="stack cc-settings" onSubmit=${save}>
    ${errors.form && html`<${Callout} kind="danger">${errors.form}</${Callout}>`}
    ${Object.keys(errors).filter((k) => k !== "form").length > 0 &&
    html`<${Callout} kind="danger">${Object.entries(errors).filter(([k]) => k !== "form").map(([k, v]) => html`<div><code>${k}</code>: ${v}</div>`)}</${Callout}>`}
    <section class="stack tight">
      <span class="section-title">Application</span>
      <div class="grid-2">
        <${Field} label="Name"><${Input} value=${form.appName} onInput=${set("appName")} /></${Field}>
        <${Field} label="URL" help="Email links point here"><${Input} value=${form.appURL} onInput=${set("appURL")} class="mono" /></${Field}>
        <${Field} label="Sender name"><${Input} value=${form.senderName} onInput=${set("senderName")} /></${Field}>
        <${Field} label="Sender address"><${Input} type="email" value=${form.senderAddress} onInput=${set("senderAddress")} /></${Field}>
      </div>
      <${Switch} checked=${form.hideControls} onChange=${set("hideControls")} label="Lock collection editing in its dashboard" />
    </section>
    <section class="stack tight">
      <span class="section-title">Mail</span>
      <${Switch} checked=${form.smtpEnabled} onChange=${set("smtpEnabled")} label="Send emails through SMTP" />
      ${form.smtpEnabled &&
      html`<div class="grid-2">
        <${Field} label="Host"><${Input} value=${form.smtpHost} onInput=${set("smtpHost")} class="mono" /></${Field}>
        <${Field} label="Port"><${Input} type="number" value=${form.smtpPort} onInput=${set("smtpPort")} class="mono" /></${Field}>
        <${Field} label="Username"><${Input} value=${form.smtpUsername} onInput=${set("smtpUsername")} autocomplete="off" /></${Field}>
        <${Field} label="Password" help="Leave empty to keep it"><${Input} type="password" value=${form.smtpPassword} onInput=${set("smtpPassword")} autocomplete="new-password" /></${Field}>
      </div>
      <${Switch} checked=${form.smtpTls} onChange=${set("smtpTls")} label="TLS from the start (port 465)" />`}
    </section>
    <section class="stack tight">
      <span class="section-title">Backups and logs</span>
      <div class="grid-3">
        <${Field} label="Backup schedule" help="Cron, UTC; empty for none"><${Input} value=${form.backupsCron} onInput=${set("backupsCron")} placeholder="0 3 * * *" class="mono" /></${Field}>
        <${Field} label="Backups kept"><${Input} type="number" value=${form.backupsKeep} onInput=${set("backupsKeep")} class="mono" /></${Field}>
        <${Field} label="Log days"><${Input} type="number" value=${form.logsDays} onInput=${set("logsDays")} class="mono" /></${Field}>
      </div>
      <${Switch} checked=${form.rateLimits} onChange=${set("rateLimits")} label="Rate limits" />
    </section>
    <div class="row cc-settings-foot">
      <span class="tiny faint grow">${changed.length ? plural(changed.length, "change") + " not saved" : "Everything else (OAuth2, S3, its superusers) is in its own dashboard."}</span>
      <${Button} variant="primary" type="submit" loading=${busy} disabled=${!changed.length}>Save</${Button}>
    </div>
  </form>`;
}
