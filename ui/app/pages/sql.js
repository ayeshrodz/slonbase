// The SQL console (when enabled on the server).
import { html, useState, useEffect, useRef } from "../lib/core.js";
import { api } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, Callout, Empty, toast, CopyButton } from "../lib/ui.js";
import { plural } from "../lib/format.js";

const HISTORY_KEY = "slonbase.sql.history";

function loadHistory() {
  try {
    return JSON.parse(localStorage.getItem(HISTORY_KEY) || "[]");
  } catch (_) {
    return [];
  }
}

export function SqlConsole() {
  const [query, setQuery] = useState(() => loadHistory()[0] || "SELECT now(), version();");
  const [result, setResult] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const [disabled, setDisabled] = useState(false);
  const [history, setHistory] = useState(loadHistory);
  const ref = useRef();

  const run = async () => {
    const sql = (ref.current && ref.current.selectionStart !== ref.current.selectionEnd
      ? query.slice(ref.current.selectionStart, ref.current.selectionEnd)
      : query
    ).trim();
    if (!sql) return;
    setBusy(true);
    setError(null);
    try {
      const res = await api.post("/api/sql", { query: sql });
      setResult(res);
      const next = [sql, ...history.filter((h) => h !== sql)].slice(0, 20);
      setHistory(next);
      try {
        localStorage.setItem(HISTORY_KEY, JSON.stringify(next));
      } catch (_) {}
    } catch (err) {
      if (err.status === 403 || err.status === 404) setDisabled(true);
      setError(err);
      setResult(null);
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    const fn = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        run();
      }
    };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  });

  const csv = () => {
    if (!result) return "";
    const esc = (v) => (v === null ? "" : /[",\n]/.test(String(v)) ? `"${String(v).replace(/"/g, '""')}"` : String(v));
    return [result.columns.map((c) => esc(c.name)).join(","), ...result.rows.map((r) => r.map(esc).join(","))].join("\n");
  };

  return html`<div class="page enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><${Icon} name="terminal" size=${14} /> Database</div>
        <h1>SQL console</h1>
      </div>
    </div>
    ${disabled &&
    html`<div style="margin-bottom:16px"><${Callout} kind="warn">
      The SQL console is turned off. Start slonbase with <code>--sql-console</code> (or <code>SLONBASE_SQL_CONSOLE=true</code>) to use it.
      Queries run with the database role's privileges; reads run in read-only transactions.
    </${Callout}></div>`}
    <div class="split-main-side">
      <div class="stack">
        <div class="card" style="overflow:hidden">
          <textarea
            ref=${ref}
            class="input code"
            style="border:0;border-radius:0;min-height:180px;padding:16px 18px;font-size:13px;box-shadow:none;background:var(--surface)"
            spellcheck="false"
            value=${query}
            onInput=${(e) => setQuery(e.target.value)}
            aria-label="SQL query"
          ></textarea>
          <div class="card-foot">
            <span class="small muted">Runs the selection, or everything. <span class="kbd">Ctrl</span> <span class="kbd">↵</span></span>
            <span class="spacer"></span>
            <${Button} variant="primary" icon="play" loading=${busy} onClick=${run}>Run</${Button}>
          </div>
        </div>
        ${error && html`<${Callout} kind="danger">${error.message}</${Callout}>`}
        ${result &&
        html`<div class="stack tight enter">
          <div class="row small muted">
            <span>${result.columns.length ? plural(result.rows.length, "row") : plural(result.affectedRows, "affected row")}</span>
            <span>· ${result.execTime} ms</span>
            ${result.rows.length >= 1000 && html`<span class="chip warn">limited to 1000 rows</span>`}
            <span class="spacer"></span>
            ${result.columns.length > 0 && html`<${CopyButton} text=${csv()} label="Copy as CSV" />`}
          </div>
          ${result.columns.length > 0 &&
          html`<div class="grid-wrap">
            <table class="grid sql-grid">
              <thead><tr>${result.columns.map((c) => html`<th><span class="th">${c.name}<span class="faint tiny">${c.type}</span></span></th>`)}</tr></thead>
              <tbody>
                ${result.rows.map((r, i) => html`<tr style=${{ "--i": i % 40, cursor: "default" }}>${r.map((v) => html`<td>${v === null ? html`<span class="faint">NULL</span>` : v}</td>`)}</tr>`)}
              </tbody>
            </table>
            ${result.rows.length === 0 && html`<${Empty} icon="table" title="No rows" />`}
          </div>`}
        </div>`}
      </div>
      <div class="card pad stack tight">
        <div class="section-title">History</div>
        ${history.length === 0 && html`<span class="small faint">Queries you run appear here.</span>`}
        ${history.map(
          (h) => html`<button class="btn ghost sm" style="justify-content:flex-start;height:auto;padding:6px 8px;white-space:normal;text-align:left;border-radius:8px" onClick=${() => setQuery(h)}>
            <code class="tiny" style="display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden">${h}</code>
          </button>`
        )}
      </div>
    </div>
  </div>`;
}
