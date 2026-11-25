// Health: what the installation looks like right now (GET /api/system/status), refreshed every
// 10 seconds: checks with what to do, this instance, the database and its pool, the other
// instances, storage and backups.
import { html, useState, useEffect, useRef, cx } from "../lib/core.js";
import { api } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, Skeleton } from "../lib/ui.js";
import { bytes, duration, relativeTime, plural } from "../lib/format.js";

const REFRESH = 10_000;
const REQUEST_TIMEOUT = 60_000;
const ORDER = { error: 0, warn: 1, info: 2, ok: 3 };
const STATUS = {
  ok: { icon: "check", label: "Healthy", tone: "ok" },
  info: { icon: "info", label: "Healthy", tone: "info" },
  warn: { icon: "warning", label: "Needs attention", tone: "warn" },
  error: { icon: "warning", label: "Something's wrong", tone: "danger" },
};

// the worst status of the checks
function overall(checks) {
  return checks.reduce((worst, c) => (ORDER[c.status] < ORDER[worst] ? c.status : worst), "ok");
}

// a small line of the last samples (newest right)
function Sparkline({ values, max }) {
  if (values.length < 2) return html`<div class="spark-empty"></div>`;
  const top = Math.max(max || 0, ...values) || 1;
  const w = 120;
  const h = 32;
  const step = w / (values.length - 1);
  const points = values.map((v, i) => `${(i * step).toFixed(1)},${(h - 2 - (v / top) * (h - 4)).toFixed(1)}`);
  return html`<svg class="spark" viewBox=${`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden="true">
    <polyline class="spark-area" points=${`0,${h} ${points.join(" ")} ${w},${h}`} />
    <polyline class="spark-line" points=${points.join(" ")} />
  </svg>`;
}

function Meter({ value, max, tone, label }) {
  const pct = max ? Math.min(100, (value / max) * 100) : 0;
  return html`<div class="meter" role="meter" aria-label=${label} aria-valuemin="0" aria-valuenow=${value} aria-valuemax=${max}>
    <span class=${cx("meter-fill", tone)} style=${{ width: pct + "%" }}></span>
  </div>`;
}

function Tile({ label, value, sub, icon, color, children, i }) {
  return html`<div class="card stat health-tile" style=${{ "--i": i }}>
    <div class="row">
      <span class="stat-icon" style=${{ "--c": color }}><${Icon} name=${icon} /></span>
      <span class="small muted">${label}</span>
    </div>
    <span class="stat-value">${value}</span>
    ${sub && html`<span class="tiny faint">${sub}</span>`}
    ${children}
  </div>`;
}

function Row({ label, children }) {
  return html`<div class="kv"><span class="muted">${label}</span><span class="kv-value">${children}</span></div>`;
}

export function Health() {
  const [status, setStatus] = useState(null);
  const [error, setError] = useState(null);
  const [updated, setUpdated] = useState(null);
  const [loading, setLoading] = useState(false);
  const history = useRef({ latency: [], memory: [], pool: [] });
  // Requests are numbered. An answer is applied unless a newer request's answer was applied
  // already (so an older one arriving late can't undo it, and a slow server still gets its
  // answers shown). The timed refreshes wait for the request that is out; each request is
  // given a minute (far more than a slow answer takes) and is then cancelled, so one that never
  // completes can't hold up the next. "Try again" replaces a request that is out (the Refresh
  // button waits for it: it is disabled while loading).
  const requested = useRef(0);
  const applied = useRef(0);
  const controller = useRef(null);

  const load = async (options) => {
    if (options?.auto && controller.current) return;
    controller.current?.abort();
    const ctl = new AbortController();
    controller.current = ctl;
    const mine = ++requested.current;
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      ctl.abort();
    }, REQUEST_TIMEOUT);
    setLoading(true);
    try {
      const s = await api.get("/api/system/status", { signal: ctl.signal });
      // (an answer whose body stalled and was cancelled comes back empty, not as an error)
      if (ctl.signal.aborted) throw new DOMException("The request was cancelled.", "AbortError");
      if (!s?.database) throw new Error("The status answer was incomplete.");
      if (mine < applied.current) return;
      applied.current = mine;
      const h = history.current;
      const push = (list, v) => {
        list.push(v);
        if (list.length > 30) list.shift();
      };
      push(h.latency, s.database.latencyMs);
      push(h.memory, s.instance.memoryBytes || 0);
      push(h.pool, s.database.pool.inUse);
      setStatus(s);
      setError(null);
      setUpdated(Date.now());
    } catch (err) {
      // (cancelled by a newer request or by leaving the page: nothing to say)
      if (err.name !== "AbortError") {
        if (mine > applied.current) setError(err.message || "The status couldn't be loaded.");
      } else if (timedOut && mine > applied.current) {
        setError("The status took more than a minute to answer.");
      }
    } finally {
      clearTimeout(timer);
      if (controller.current === ctl) {
        controller.current = null;
        setLoading(false);
      }
    }
  };
  useEffect(() => {
    load();
    const timer = setInterval(() => document.visibilityState === "visible" && load({ auto: true }), REFRESH);
    // a tab that was in the background shows what it last read: read again as it comes back
    const back = () => document.visibilityState === "visible" && load({ auto: true });
    document.addEventListener("visibilitychange", back);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", back);
      const out = controller.current;
      controller.current = null;
      out?.abort();
    };
  }, []);
  // "refreshed 4 s ago" stays current
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, []);

  if (!status) {
    return html`<div class="page enter">
      <div class="page-head"><div class="grow"><h1>Health</h1></div></div>
      ${error
        ? html`<div class="card"><div class="empty"><h3>Couldn't load the status</h3><p class="small">${error}</p><${Button} onClick=${load}>Try again</${Button}></div></div>`
        : html`<div class="grid-3" style="grid-template-columns:repeat(auto-fit,minmax(180px,1fr))">${[0, 1, 2, 3].map(() => html`<div class="card stat"><${Skeleton} height=${70} /></div>`)}</div>`}
    </div>`;
  }

  const checks = [...status.checks].sort((a, b) => ORDER[a.status] - ORDER[b.status]);
  const worst = overall(checks);
  const problems = checks.filter((c) => c.status === "warn" || c.status === "error");
  const notes = checks.filter((c) => c.status === "info");
  const healthy = checks.filter((c) => c.status === "ok");
  const db = status.database;
  const pool = db.pool;
  const h = history.current;
  const seconds = updated ? Math.round((Date.now() - updated) / 1000) : 0;
  // the instances sharing the database (this one first, before its first heartbeat is written);
  // one is live while its age, as it advances, is under a minute, whatever the last answer said
  // (a failed refresh leaves that answer on the page)
  const aged = (i) => i.ageSeconds + seconds;
  const reported = status.instances.length ? status.instances : [{ id: status.instance.id, current: true, live: true, ageSeconds: 0 }];
  const instances = reported
    .map((i) => ({ ...i, live: i.live && aged(i) < 60 }))
    .sort((a, b) => b.current - a.current || b.live - a.live || (a.host || a.id).localeCompare(b.host || b.id));
  const live = instances.filter((i) => i.live).length;
  const sinceRead = seconds;

  return html`<div class="page enter health">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><span class=${cx("dot", STATUS[worst].tone === "info" ? "ok" : STATUS[worst].tone, { live: worst === "ok" || worst === "info" })}></span> ${STATUS[worst].label}</div>
        <h1>Health</h1>
        <p class="muted small" style="margin:4px 0 0">slonbase ${status.version} · up ${duration(status.uptimeSeconds)} · ${plural(live, "instance")}</p>
      </div>
      <span class="tiny faint">${error ? html`<span style="color:var(--danger)">${error}</span>` : `Refreshed ${seconds < 2 ? "just now" : seconds + " s ago"}`}</span>
      <${Button} size="sm" icon="refresh" loading=${loading} onClick=${load}>Refresh</${Button}>
    </div>

    <div class="grid-3" style="grid-template-columns:repeat(auto-fit,minmax(190px,1fr));margin-bottom:18px">
      <${Tile} i=${0} label="Database round trip" icon="data" color="var(--tone-2)" value=${db.latencyMs + " ms"} sub=${db.version.split(" ")[0] && "PostgreSQL " + db.version.split(" ")[0]}>
        <${Sparkline} values=${h.latency} />
      </${Tile}>
      <${Tile} i=${1} label="Connections in use" icon="layers" color="var(--tone-1)" value=${`${pool.inUse} / ${pool.max}`} sub=${pool.waiting ? `${pool.waiting} waiting` : `${pool.idle} idle`}>
        <${Meter} label="Database connections in use" value=${pool.inUse} max=${pool.max} tone=${pool.waiting ? "warn" : pool.inUse / pool.max > 0.8 ? "warn" : ""} />
      </${Tile}>
      <${Tile} i=${2} label="Memory (this instance)" icon="server" color="var(--tone-8)" value=${status.instance.memoryBytes ? bytes(status.instance.memoryBytes) : "—"} sub=${`${status.instance.realtimeClients} realtime ${status.instance.realtimeClients === 1 ? "client" : "clients"}`}>
        <${Sparkline} values=${h.memory} />
      </${Tile}>
      <${Tile} i=${3} label="Requests, last 24 hours" icon="chart" color=${status.logs.errors24h ? "var(--danger)" : "var(--tone-3)"} value=${status.logs.requests24h.toLocaleString() + " requests"} sub=${`${plural(status.logs.errors24h, "error")} · ${plural(status.logs.warnings24h, "warning")}`} />
    </div>

    <div class="split-2-1" style="margin-bottom:18px">
      <div class="card pad stack">
        <div class="row"><h3>Checks</h3><span class="spacer"></span><span class="tiny faint">${problems.length ? plural(problems.length, "thing") + " to look at" : "nothing to fix"}</span></div>
        <div class="checks">
          ${[...problems, ...notes].map(
            (c, i) => html`<div class=${cx("check-item", STATUS[c.status].tone)} style=${{ "--i": i }}>
              <span class="check-icon"><${Icon} name=${STATUS[c.status].icon} size=${16} /></span>
              <div class="grow">
                <div class="check-title">${c.title}</div>
                <div class="small muted">${c.detail}</div>
              </div>
              ${c.link && html`<a class="btn sm ghost" href=${c.link}>${c.status === "info" ? "Open" : "Fix"} <${Icon} name="arrowRight" size=${13} /></a>`}
            </div>`
          )}
          ${healthy.map(
            (c) => html`<div class="check-item ok compact">
              <span class="check-icon"><${Icon} name="check" size=${14} /></span>
              <div class="grow"><span class="check-title">${c.title}</span> <span class="small muted">${c.detail}</span></div>
            </div>`
          )}
        </div>
      </div>

      <div class="stack">
        <div class="card pad stack tight">
          <h3>Database</h3>
          <${Row} label="Server">PostgreSQL ${db.version.split(" ")[0]}</${Row}>
          <${Row} label="Database"><code>${db.name}</code></${Row}>
          <${Row} label="Size">${bytes(db.sizeBytes)}</${Row}>
          <${Row} label="System migrations">${db.migration}</${Row}>
          <${Row} label="App migrations">${db.appMigrations}</${Row}>
        </div>
        <div class="card pad stack tight">
          <h3>Files and backups</h3>
          <${Row} label="Files">${status.storage.kind === "s3" ? "S3" : "Local"}</${Row}>
          <div class="tiny faint ellipsis" title=${status.storage.description}>${status.storage.description}</div>
          <${Row} label="Last backup">${status.backups.last ? relativeTime(status.backups.last.modified) : "never"}</${Row}>
          <${Row} label="Backups">${status.backups.count} · ${status.backups.inS3 ? "in S3" : "on this server"}</${Row}>
          <${Row} label="Schedule">${status.backups.schedule ? html`<code>${status.backups.schedule}</code>` : "none"}</${Row}>
          <${Row} label="Encryption">${status.backups.encrypted ? html`<span class="chip ok">on</span>` : html`<span class="chip">off</span>`}</${Row}>
        </div>
      </div>
    </div>

    <div class="card pad">
      <div class="row" style="margin-bottom:10px">
        <h3>Instances</h3>
        <span class="muted small">every slonbase process connected to this database; each reports in every 20 seconds</span>
      </div>
      <div class="instances">
        ${instances.map((inst, i) => {
          // the age is the database's clock (not the browser's), plus the time since it was read
          const age = inst.ageSeconds + sinceRead;
          const state = !inst.live ? "" : inst.sync === "broken" ? "warn" : "ok";
          // a silent instance's last figures would only mislead
          const facts = inst.live
            ? [
                inst.uptimeSeconds != null && "up " + duration(inst.uptimeSeconds + age),
                inst.memoryBytes && bytes(inst.memoryBytes),
                inst.pool && "pool " + inst.pool.inUse + "/" + inst.pool.max,
                inst.realtimeClients != null && plural(inst.realtimeClients, "realtime client"),
              ].filter(Boolean)
            : [];
          return html`<div class=${cx("instance", { current: inst.current, gone: !inst.live })} style=${{ "--i": i }} title=${"Instance id " + inst.id}>
            <span class=${cx("dot", state, { live: inst.live })}></span>
            <div class="grow stack tight" style="min-width:0">
              <div class="row" style="gap:6px;flex-wrap:wrap">
                <strong class="ellipsis">${inst.host || inst.id}</strong>
                ${inst.current && html`<span class="chip accent">this one</span>`}
                ${inst.version && html`<span class="chip">${inst.version}</span>`}
                ${inst.sync === "broken" && html`<span class="chip warn">not receiving changes</span>`}
              </div>
              <div class="tiny faint">
                ${inst.live ? (age < 2 ? "reported just now" : "reported " + duration(age) + " ago") : "not heard from for " + duration(age) + ": stopped or unreachable"}
                ${facts.length > 0 && " · " + facts.join(" · ")}
              </div>
            </div>
          </div>`;
        })}
      </div>
      ${live === 1 && html`<p class="muted small" style="margin:10px 0 0">Only this instance is connected. Another one started with the same database URL appears here within 20 seconds, with nothing else to set up.</p>`}
    </div>
  </div>`;
}
