// Request and app logs: a chart, filters, the list and details.
import { html, useState, useEffect, useStore, route, cx } from "../lib/core.js";
import { api, qs } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Modal, SheetHead, Skeleton, Empty, Segmented, CopyButton, toast, confirmDialog } from "../lib/ui.js";
import { formatDate, relativeTime, parseDate, plural } from "../lib/format.js";

const LEVELS = { "-4": ["debug", "info"], 0: ["info", "info"], 4: ["warn", "warn"], 8: ["error", "danger"] };

function level(n) {
  return LEVELS[n] || LEVELS[0];
}

function Chart({ filter }) {
  const [stats, setStats] = useState(null);
  useEffect(() => {
    api.get("/api/logs/stats" + qs({ filter })).then(setStats).catch(() => setStats([]));
  }, [filter]);
  if (!stats) return html`<${Skeleton} height=${120} />`;
  const map = new Map(stats.map((s) => [parseDate(s.date).getTime(), s.total]));
  const now = new Date();
  now.setMinutes(0, 0, 0);
  const hours = [];
  for (let i = 47; i >= 0; i--) {
    const t = now.getTime() - i * 3600000;
    hours.push({ t, total: map.get(t) || 0 });
  }
  const max = Math.max(1, ...hours.map((h) => h.total));
  const total = hours.reduce((s, h) => s + h.total, 0);
  return html`<div>
    <div class="row small muted" style="margin-bottom:4px"><b style="color:var(--ink)">${total.toLocaleString()}</b> entries in the last 48 hours</div>
    <div class="bars" style="height:110px">
      ${hours.map(
        (h, i) => html`<div class=${"bar tip" + (h.total ? "" : " zero")} data-tip=${`${new Date(h.t).toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit" })}: ${h.total}`} style=${{ "--i": i, height: Math.max(3, (h.total / max) * 100) + "px" }}></div>`
      )}
    </div>
  </div>`;
}

function Detail({ log, onClose }) {
  const data = log.data || {};
  const [lvl, tone] = level(log.level);
  return html`<${Modal} size="mid" onClose=${onClose}>
    ${(close) => html`
      <${SheetHead} title=${log.message} subtitle=${formatDate(log.created) + " · " + relativeTime(log.created)} icon="logs" onClose=${close}>
        <span class=${"chip " + tone}>${lvl}</span>
      </${SheetHead}>
      <div class="sheet-body stack">
        ${data.type === "request" &&
        html`<div class="grid-3">
          ${[
            ["Status", html`<b class=${"status-" + String(data.status)[0]}>${data.status}</b>`],
            ["Method", html`<span class=${"method " + data.method}>${data.method}</span>`],
            ["Time", `${data.execTime} ms`],
            ["IP", data.userIP],
            ["Auth", data.auth || "guest"],
            ["Referer", data.referer || "—"],
          ].map(([k, v]) => html`<div class="meta-item"><span class="k">${k}</span><span class="small ellipsis">${v}</span></div>`)}
        </div>`}
        ${data.url && html`<div class="meta-item"><span class="k">URL</span><code class="small" style="word-break:break-all">${data.url}</code></div>`}
        ${data.userAgent && html`<div class="meta-item"><span class="k">User agent</span><span class="small muted">${data.userAgent}</span></div>`}
        ${data.error && html`<div class="callout danger"><${Icon} name="warning" size=${17} /><div><b>${data.error}</b>${data.details && html`<pre class="code" style="margin-top:8px;white-space:pre-wrap">${data.details}</pre>`}</div></div>`}
        <div style="position:relative">
          <div class="section-title" style="margin-bottom:6px">Data</div>
          <pre class="code">${JSON.stringify(data, null, 2)}</pre>
          <span style="position:absolute;top:28px;right:8px"><${CopyButton} text=${JSON.stringify(log, null, 2)} /></span>
        </div>
      </div>
    `}
  </${Modal}>`;
}

export function Logs() {
  const { query } = useStore(route);
  const [text, setText] = useState(query.filter || "");
  const [filter, setFilter] = useState(query.filter || "");
  const [kind, setKind] = useState("all");
  const [items, setItems] = useState(null);
  const [page, setPage] = useState(1);
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState(null);
  const [tick, setTick] = useState(0);

  const kinds = {
    all: "",
    errors: "level >= 8 || data.status >= 500",
    warnings: "level >= 4 || data.status >= 400",
    app: "data.type != 'request'",
  };
  const combined = [kinds[kind], filter].filter(Boolean).map((f) => `(${f})`).join(" && ");

  useEffect(() => {
    setLoading(true);
    api
      .get("/api/logs" + qs({ page, perPage: 50, sort: "-created", filter: combined }))
      .then((res) => {
        setItems((prev) => (page === 1 ? res.items : [...(prev || []), ...res.items]));
        setMore(res.page < res.totalPages);
      })
      .catch((err) => {
        toast.error(err);
        setItems([]);
      })
      .finally(() => setLoading(false));
  }, [combined, page, tick]);

  const apply = (e) => {
    e.preventDefault();
    setPage(1);
    setFilter(text.trim());
  };

  const clearAll = async () => {
    if (!(await confirmDialog({ title: "Delete all logs?", message: "Request and app logs are removed for good.", confirm: "Delete logs", danger: true }))) return;
    try {
      await api.delete("/api/logs");
      toast("Logs deleted.");
      setPage(1);
      setTick((t) => t + 1);
    } catch (err) {
      toast.error(err);
    }
  };

  return html`<div class="page enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><${Icon} name="logs" size=${14} /> Observability</div>
        <h1>Logs</h1>
      </div>
      <${IconButton} icon="refresh" label="Refresh" onClick=${() => {
        setPage(1);
        setTick((t) => t + 1);
      }} />
      <${Button} variant="danger-ghost" icon="trash" onClick=${clearAll}>Delete all</${Button}>
    </div>
    <div class="card pad" style="margin-bottom:16px"><${Chart} filter=${combined} /></div>
    <div class="toolbar">
      <${Segmented} value=${kind} onChange=${(k) => {
        setKind(k);
        setPage(1);
      }} options=${[{ value: "all", label: "All" }, { value: "errors", label: "Errors" }, { value: "warnings", label: "4xx & warnings" }, { value: "app", label: "App" }]} />
      <form class="filter-box" onSubmit=${apply}>
        <${Icon} name="filter" size=${16} />
        <input placeholder="e.g. data.url ~ '/api/collections' && data.execTime > 100" value=${text} onInput=${(e) => setText(e.target.value)} />
        <${Button} size="xs" type="submit" variant=${text.trim() !== filter ? "primary" : "ghost"}>Apply</${Button}>
      </form>
    </div>
    <div class="grid-wrap" style="max-height:none">
      <table class="grid">
        <thead><tr><th style="width:170px">Time</th><th style="width:90px">Level</th><th>Message</th><th style="width:90px">Status</th><th style="width:90px">Time</th><th style="width:120px">Auth</th></tr></thead>
        <tbody>
          ${items === null
            ? [0, 1, 2, 3, 4].map(() => html`<tr>${[0, 1, 2, 3, 4, 5].map(() => html`<td><${Skeleton} /></td>`)}</tr>`)
            : items.map((log, i) => {
                const d = log.data || {};
                const [lvl, tone] = level(log.level);
                return html`<tr key=${log.id} style=${{ "--i": i % 50 }} onClick=${() => setOpen(log)}>
                  <td class="small"><span class="tip" data-tip=${relativeTime(log.created)}>${formatDate(log.created)}</span></td>
                  <td><span class=${"chip " + tone}>${lvl}</span></td>
                  <td class="ellipsis">${d.method && html`<span class=${"method " + d.method} style="margin-right:8px">${d.method}</span>`}${d.url || log.message}</td>
                  <td>${d.status ? html`<b class=${"status-" + String(d.status)[0]}>${d.status}</b>` : ""}</td>
                  <td class="small muted">${d.execTime !== undefined ? `${d.execTime} ms` : ""}</td>
                  <td class="small muted">${d.auth || ""}</td>
                </tr>`;
              })}
        </tbody>
      </table>
      ${items && items.length === 0 && html`<${Empty} icon="logs" title="No logs">${combined ? "Nothing matches these filters." : "Requests show up here as they happen."}</${Empty}>`}
    </div>
    ${more && html`<div class="row" style="justify-content:center;margin-top:14px"><${Button} loading=${loading} onClick=${() => setPage(page + 1)}>Load more</${Button}></div>`}
    ${open && html`<${Detail} log=${open} onClose=${() => setOpen(null)} />`}
  </div>`;
}
