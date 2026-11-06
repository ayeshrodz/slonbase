// The SQL console (when enabled on the server): a schema browser and history beside an editor
// with line numbers and highlighting, and the results below it, ready to export.
import { html, useState, useEffect, useRef, useStore, cx } from "../lib/core.js";
import { api, PROJECT, scopedKey } from "../lib/api.js";
import { app } from "../lib/state.js";
import { Icon } from "../lib/icons.js";
import { Button, Callout, Empty, Menu, toast, CollectionGlyph } from "../lib/ui.js";
import { plural } from "../lib/format.js";
import { toCsv, toTsv, toJson, toXlsx, download } from "../lib/export.js";

// each installation keeps its own history
const HISTORY_KEY = scopedKey("sql.history");
const ROWS_KEY = scopedKey("sql.rows");

function load(key, fallback) {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : JSON.parse(v);
  } catch (_) {
    return fallback;
  }
}
function save(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch (_) {}
}

// ------------------------------------------------------------------ highlighting

const KEYWORDS = new Set(
  (
    "select from where and or not in is null as on join left right inner outer full cross natural using group by order having limit offset " +
    "distinct all union intersect except with recursive values insert into update set delete returning create alter drop table view index " +
    "unique primary key foreign references default check constraint if exists case when then else end asc desc nulls first last true false " +
    "like ilike similar between any some cast explain analyze verbose show begin commit rollback truncate grant revoke materialized " +
    "schema sequence function returns language trigger over partition window filter lateral array interval timestamp date time"
  ).split(" ")
);

const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

// SQL as highlighted HTML: comments, strings, quoted names, numbers, keywords and parameters
export function highlight(sql) {
  const re = /(--[^\n]*|\/\*[\s\S]*?(?:\*\/|$))|('(?:[^']|'')*'?)|("(?:[^"]|"")*"?)|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z0-9_$]*)|(\$\d+|:\w+)/g;
  let out = "";
  let last = 0;
  for (let m; (m = re.exec(sql)); ) {
    out += esc(sql.slice(last, m.index));
    const [t] = m;
    const kind = m[1] ? "cm" : m[2] ? "str" : m[3] ? "id" : m[4] ? "num" : m[5] ? (KEYWORDS.has(t.toLowerCase()) ? "kw" : sql[re.lastIndex] === "(" ? "fn" : "") : "param";
    out += kind ? `<span class="sx-${kind}">${esc(t)}</span>` : esc(t);
    last = re.lastIndex;
  }
  return out + esc(sql.slice(last)) + "\n";
}

// ------------------------------------------------------------------ the editor

function Editor({ value, onChange, onRun, errorLine, editorRef }) {
  const pre = useRef();
  const gutter = useRef();
  const lines = value.split("\n").length;
  const sync = (e) => {
    if (pre.current) {
      pre.current.scrollTop = e.target.scrollTop;
      pre.current.scrollLeft = e.target.scrollLeft;
    }
    if (gutter.current) gutter.current.scrollTop = e.target.scrollTop;
  };
  const edit = (text, start, end) => {
    onChange(text);
    requestAnimationFrame(() => {
      const ta = editorRef.current;
      if (ta) ta.setSelectionRange(start, end);
    });
  };
  const keydown = (e) => {
    const ta = e.target;
    const { selectionStart: s, selectionEnd: t } = ta;
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      onRun();
    } else if (e.key === "Tab" && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      edit(value.slice(0, s) + "  " + value.slice(t), s + 2, s + 2);
    } else if ((e.ctrlKey || e.metaKey) && e.key === "/") {
      // comment or uncomment the selected lines
      e.preventDefault();
      const from = value.lastIndexOf("\n", s - 1) + 1;
      const to = value.indexOf("\n", t) === -1 ? value.length : value.indexOf("\n", t);
      const block = value.slice(from, to).split("\n");
      const commented = block.every((l) => /^\s*--/.test(l) || !l.trim());
      const next = block.map((l) => (commented ? l.replace(/^(\s*)-- ?/, "$1") : l.trim() ? "-- " + l : l)).join("\n");
      edit(value.slice(0, from) + next + value.slice(to), from, from + next.length);
    }
  };
  return html`<div class="sql-editor">
    <div class="sql-gutter" ref=${gutter} aria-hidden="true">
      ${Array.from({ length: lines }, (_, i) => html`<div class=${cx({ "sql-gutter-error": errorLine === i + 1 })}>${i + 1}</div>`)}
    </div>
    <div class="sql-code">
      <pre ref=${pre} aria-hidden="true" dangerouslySetInnerHTML=${{ __html: highlight(value) }}></pre>
      <textarea
        ref=${editorRef}
        spellcheck="false"
        autocapitalize="off"
        autocomplete="off"
        wrap="off"
        aria-label="SQL query"
        value=${value}
        onInput=${(e) => onChange(e.target.value)}
        onScroll=${sync}
        onKeyDown=${keydown}
      ></textarea>
    </div>
  </div>`;
}

// ------------------------------------------------------------------ the schema

function quoteName(name) {
  return /^[a-z_][a-z0-9_]*$/.test(name) ? name : `"${name.replace(/"/g, '""')}"`;
}

function Schema({ onInsert, onQuery }) {
  const collections = useStore(app, (s) => s.collections);
  const [filter, setFilter] = useState("");
  const [open, setOpen] = useState({});
  const f = filter.trim().toLowerCase();
  const shown = collections
    .filter((c) => !f || c.name.toLowerCase().includes(f) || (c.fields || []).some((x) => x.name.toLowerCase().includes(f)))
    .sort((a, b) => (a.system === b.system ? a.name.localeCompare(b.name) : a.system ? 1 : -1));
  return html`<div class="sql-schema">
    <label class="tree-filter">
      <${Icon} name="search" size=${14} />
      <input placeholder="Filter tables and columns" value=${filter} onInput=${(e) => setFilter(e.target.value)} aria-label="Filter tables" />
    </label>
    <div class="sql-schema-list">
      ${shown.map((c) => {
        const expanded = open[c.id] || (f && !c.name.toLowerCase().includes(f));
        return html`<div class="sql-table" key=${c.id}>
          <div class="sql-table-row">
            <button class="sql-table-toggle" aria-expanded=${!!expanded} onClick=${() => setOpen({ ...open, [c.id]: !open[c.id] })}>
              <${Icon} name=${expanded ? "chevronDown" : "chevronRight"} size=${13} />
              <${CollectionGlyph} collection=${c} size=${18} />
              <span class="ellipsis mono">${c.name}</span>
            </button>
            <button class="icon-btn sm" title=${`SELECT * FROM ${c.name}`} aria-label=${`Query ${c.name}`} onClick=${() => onQuery(c)}><${Icon} name="play" size=${13} /></button>
          </div>
          ${expanded &&
          html`<div class="sql-columns">
            ${["id", ...(c.fields || []).map((x) => x.name).filter((n) => n !== "id")].map((name) => {
              const field = (c.fields || []).find((x) => x.name === name);
              return html`<button class="sql-column" onClick=${() => onInsert(quoteName(name))} title="Insert at the cursor">
                <span class="ellipsis mono">${name}</span><span class="faint mono">${field ? field.type : "text"}</span>
              </button>`;
            })}
          </div>`}
        </div>`;
      })}
      ${shown.length === 0 && html`<div class="small faint" style="padding:8px">No tables match.</div>`}
    </div>
  </div>`;
}

// ------------------------------------------------------------------ results

function Results({ result, error, busy }) {
  const [sel, setSel] = useState(null);
  if (busy && !result && !error) return html`<div class="sql-results-empty"><span class="spinner"></span> Running…</div>`;
  if (error) return null;
  if (!result) return html`<div class="sql-results-empty small faint">Run a query to see its results here.</div>`;
  if (!result.columns.length) {
    return html`<div class="sql-results-empty"><${Icon} name="check" size=${16} /> ${result.readOnly ? "Done." : plural(result.affectedRows, "row") + " affected."}</div>`;
  }
  return html`<div class="sql-results-body">
    <div class="sql-grid-wrap" tabindex="0" role="region" aria-label="Query results">
      <table class="grid sql-grid">
        <thead>
          <tr>
            <th class="sql-rownum">#</th>
            ${result.columns.map((c) => html`<th><span class="th">${c.name}${c.type && html`<span class="sql-type">${c.type}</span>`}</span></th>`)}
          </tr>
        </thead>
        <tbody>
          ${result.rows.map(
            (r, i) => html`<tr>
              <td class="sql-rownum">${i + 1}</td>
              ${r.map(
                (v, j) => html`<td
                  class=${cx({ "sql-null": v === null, on: sel && sel[0] === i && sel[1] === j })}
                  title=${v === null ? "NULL" : v.length > 60 ? v.slice(0, 2000) : undefined}
                  onClick=${() => setSel([i, j])}
                >${v === null ? "NULL" : v}</td>`
              )}
            </tr>`
          )}
        </tbody>
      </table>
      ${result.rows.length === 0 && html`<${Empty} icon="table" title="No rows" />`}
    </div>
    ${sel &&
    result.rows[sel[0]] &&
    html`<div class="sql-cell">
      <span class="sql-cell-ref mono">${result.columns[sel[1]].name} · row ${sel[0] + 1}</span>
      <code class="sql-cell-value">${result.rows[sel[0]][sel[1]] === null ? "NULL" : result.rows[sel[0]][sel[1]]}</code>
      <button class="icon-btn sm" aria-label="Copy the value" onClick=${() => navigator.clipboard && navigator.clipboard.writeText(result.rows[sel[0]][sel[1]] ?? "")}><${Icon} name="copy" size=${14} /></button>
      <button class="icon-btn sm" aria-label="Close" onClick=${() => setSel(null)}><${Icon} name="close" size=${14} /></button>
    </div>`}
  </div>`;
}

// ------------------------------------------------------------------ the page

export function SqlConsole() {
  const [info, setInfo] = useState(null);
  const [query, setQuery] = useState(() => load(HISTORY_KEY, [])[0] || "SELECT now(), version();");
  const [result, setResult] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const [history, setHistory] = useState(() => load(HISTORY_KEY, []));
  const [rows, setRows] = useState(() => load(ROWS_KEY, 1000));
  const [side, setSide] = useState("schema");
  const [ran, setRan] = useState("");
  const editor = useRef();

  useEffect(() => {
    api.get("/api/sql").then(setInfo).catch(() => setInfo({ enabled: false }));
  }, []);

  const run = async () => {
    const ta = editor.current;
    const sql = (ta && ta.selectionStart !== ta.selectionEnd ? query.slice(ta.selectionStart, ta.selectionEnd) : query).trim();
    if (!sql || busy) return;
    setBusy(true);
    setError(null);
    try {
      const res = await api.post("/api/sql", { query: sql, maxRows: rows });
      setResult(res);
      setRan(sql);
      const next = [sql, ...history.filter((h) => h !== sql)].slice(0, 30);
      setHistory(next);
      save(HISTORY_KEY, next);
    } catch (err) {
      setError(err);
      setResult(null);
    } finally {
      setBusy(false);
    }
  };

  const insert = (text) => {
    const ta = editor.current;
    const s = ta ? ta.selectionStart : query.length;
    const t = ta ? ta.selectionEnd : query.length;
    const next = query.slice(0, s) + text + query.slice(t);
    setQuery(next);
    requestAnimationFrame(() => {
      if (!ta) return;
      ta.focus();
      ta.setSelectionRange(s + text.length, s + text.length);
    });
  };
  const queryTable = (c) => setQuery(`SELECT *\nFROM ${quoteName(c.name)}\nORDER BY ${c.fields && c.fields.some((f) => f.name === "created") ? "created DESC" : "id"}\nLIMIT 100;`);

  const base = `${PROJECT ? PROJECT + "-" : ""}query-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")}`;
  const exportAs = (kind) => {
    const { columns, rows: data } = result;
    if (kind === "csv") download(base + ".csv", "﻿" + toCsv(columns, data), "text/csv;charset=utf-8");
    if (kind === "tsv") download(base + ".tsv", "﻿" + toTsv(columns, data), "text/tab-separated-values;charset=utf-8");
    if (kind === "json") download(base + ".json", toJson(columns, data), "application/json");
    if (kind === "xlsx") download(base + ".xlsx", toXlsx(columns, data, "Query"));
    toast(`Exported ${plural(data.length, "row")}`);
  };
  const copy = (text, what) => {
    navigator.clipboard && navigator.clipboard.writeText(text);
    toast(`Copied ${what}`);
  };

  const disabled = info && !info.enabled;
  const errorLine = error && error.data && error.data.line;
  const tooLong = info && query.length > info.maxQueryLength;

  return html`<div class="page sql-page enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><${Icon} name="terminal" size=${14} /> Database${PROJECT ? ` · ${PROJECT}` : ""}</div>
        <h1>SQL console</h1>
      </div>
      ${info &&
      info.enabled &&
      html`<div class="row sql-mode">
        <span class=${cx("chip", info.writes ? "warn" : "ok")}><${Icon} name=${info.writes ? "unlock" : "lock"} size=${12} /> ${info.writes ? "Read-write" : "Read-only"}</span>
        <span class="tiny faint mono">${info.timeoutSeconds}s timeout · ${info.maxRows.toLocaleString()} rows max</span>
      </div>`}
    </div>
    ${disabled &&
    html`<div style="margin-bottom:16px"><${Callout} kind="warn">
      The SQL console is turned off. Start slonbase with <code>--sql-console</code> (or <code>SLONBASE_SQL_CONSOLE=true</code>) to use it. It then
      runs reads only; add <code>--sql-console-writes</code> (<code>SLONBASE_SQL_CONSOLE_WRITES=true</code>) to let it change data.
    </${Callout}></div>`}
    <div class="sql-layout">
      <aside class="card sql-side">
        <div class="seg sql-side-tabs" role="tablist">
          <button role="tab" aria-selected=${side === "schema"} class=${cx({ on: side === "schema" })} onClick=${() => setSide("schema")}>Tables</button>
          <button role="tab" aria-selected=${side === "history"} class=${cx({ on: side === "history" })} onClick=${() => setSide("history")}>History</button>
        </div>
        ${side === "schema"
          ? html`<${Schema} onInsert=${insert} onQuery=${queryTable} />`
          : html`<div class="sql-history">
              ${history.length === 0 && html`<span class="small faint" style="padding:8px">Queries you run appear here.</span>`}
              ${history.map(
                (h) => html`<button class="sql-history-item" onClick=${() => setQuery(h)} title=${h}><code>${h}</code></button>`
              )}
              ${history.length > 0 &&
              html`<button class="btn ghost xs" style="align-self:flex-start;margin-top:6px" onClick=${() => (setHistory([]), save(HISTORY_KEY, []))}>Clear history</button>`}
            </div>`}
      </aside>
      <section class="sql-main">
        <div class="card sql-editor-card">
          <${Editor} value=${query} onChange=${setQuery} onRun=${run} errorLine=${errorLine} editorRef=${editor} />
          <div class="sql-toolbar">
            <${Button} variant="primary" icon="play" loading=${busy} disabled=${disabled || tooLong} onClick=${run}>Run</${Button}>
            <label class="sql-limit">
              <span class="tiny muted">Rows</span>
              <select class="input" value=${rows} aria-label="Rows to fetch" onChange=${(e) => (setRows(Number(e.target.value)), save(ROWS_KEY, Number(e.target.value)))}>
                ${[100, 500, 1000, 5000].filter((n) => !info || n <= info.maxRows).map((n) => html`<option value=${n}>${n.toLocaleString()}</option>`)}
              </select>
            </label>
            <span class="tiny faint sql-keys">
              <span class="kbd">Ctrl</span> <span class="kbd">↵</span> run (the selection, if any) · <span class="kbd">Ctrl</span> <span class="kbd">/</span> comment
            </span>
            <span class="spacer"></span>
            <span class=${cx("tiny mono", tooLong ? "sql-over" : "faint")}>${query.length.toLocaleString()}${info ? " / " + info.maxQueryLength.toLocaleString() : ""}</span>
            <button class="icon-btn sm" aria-label="Clear the editor" title="Clear" onClick=${() => setQuery("")}><${Icon} name="trash" size=${14} /></button>
          </div>
        </div>

        <div class="card sql-results">
          <div class="sql-results-head">
            ${error
              ? html`<span class="sql-status bad"><${Icon} name="warning" size=${14} /> Error</span>`
              : result
                ? html`<span class="sql-status ok"><${Icon} name="check" size=${14} /> ${result.columns.length ? plural(result.rows.length, "row") : plural(result.affectedRows, "row") + " affected"}</span>
                    <span class="tiny faint mono">${result.execTime} ms · ${result.readOnly ? "read-only" : "committed"}</span>
                    ${result.truncated && html`<span class="chip warn">first ${result.rows.length.toLocaleString()} rows</span>`}`
                : html`<span class="section-title">Results</span>`}
            <span class="spacer"></span>
            ${result &&
            result.columns.length > 0 &&
            html`<${Menu}
              trigger=${(toggle) => html`<${Button} size="sm" icon="download" onClick=${toggle}>Export</${Button}>`}
              items=${[
                { icon: "table", label: "Excel (.xlsx)", onClick: () => exportAs("xlsx") },
                { icon: "file", label: "CSV (.csv)", onClick: () => exportAs("csv") },
                { icon: "file", label: "TSV (.tsv)", onClick: () => exportAs("tsv") },
                { icon: "braces", label: "JSON (.json)", onClick: () => exportAs("json") },
                "-",
                { icon: "copy", label: "Copy as CSV", onClick: () => copy(toCsv(result.columns, result.rows), "as CSV") },
                { icon: "copy", label: "Copy as JSON", onClick: () => copy(toJson(result.columns, result.rows), "as JSON") },
                { icon: "code", label: "Copy the query", onClick: () => copy(ran, "the query") },
              ]}
            />`}
          </div>
          ${error &&
          html`<div class="sql-error">
            <pre>${error.message}</pre>
          </div>`}
          <${Results} result=${result} error=${error} busy=${busy} />
        </div>
      </section>
    </div>
  </div>`;
}
