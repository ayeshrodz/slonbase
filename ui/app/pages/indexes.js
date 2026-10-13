// A collection's indexes (stored as CREATE INDEX statements, like PocketBase).
import { html, useState } from "../lib/core.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Field, Input, Switch, Modal, SheetHead, Check, Empty, Callout, TextArea } from "../lib/ui.js";

export function parseIndex(sql) {
  const m = /CREATE\s+(UNIQUE\s+)?INDEX\s+(?:IF NOT EXISTS\s+)?[`"]?([^`"\s]+)[`"]?\s+ON\s+[`"]?[^`"\s(]+[`"]?\s*\(([^)]*)\)(.*)$/i.exec(sql || "");
  if (!m) return { unique: false, name: "", columns: [], where: "", raw: sql };
  return {
    unique: !!m[1],
    name: m[2],
    columns: m[3].split(",").map((c) => c.trim().replace(/[`"]/g, "")).filter(Boolean),
    where: m[4].trim(),
    raw: sql,
  };
}

function build(collection, { unique, name, columns, where }) {
  const cols = columns.map((c) => (/\s/.test(c) ? c : "`" + c + "`")).join(", ");
  return `CREATE ${unique ? "UNIQUE " : ""}INDEX \`${name}\` ON \`${collection}\` (${cols})${where ? " " + where : ""}`;
}

function IndexEditor({ draft, initial, onSave, onClose }) {
  const [unique, setUnique] = useState(initial ? initial.unique : false);
  const [columns, setColumns] = useState(initial ? initial.columns : []);
  const [name, setName] = useState(initial ? initial.name : "");
  const [raw, setRaw] = useState(false);
  const [rawText, setRawText] = useState(initial ? initial.raw : "");
  const autoName = `idx_${draft.name}_${columns.join("_") || "x"}`.slice(0, 60);
  const fields = draft.fields.map((f) => f.name).filter((n) => n !== "id");
  const toggle = (c) => setColumns(columns.includes(c) ? columns.filter((x) => x !== c) : [...columns, c]);
  const sql = raw ? rawText : build(draft.name, { unique, name: name || autoName, columns, where: initial ? initial.where : "" });
  return html`<${Modal} size="narrow" onClose=${onClose}>
    ${(close) => html`
      <${SheetHead} title=${initial ? "Edit index" : "New index"} icon="zap" onClose=${close} />
      <div class="sheet-body stack">
        ${raw
          ? html`<${Field} label="Statement"><${TextArea} code rows=${4} value=${rawText} onInput=${setRawText} /></${Field}>`
          : html`
              <div class="field">
                <span class="label">Columns (in order)</span>
                <div class="picker-list" style="max-height:220px">
                  ${fields.map(
                    (f) => html`<div class=${"picker-item" + (columns.includes(f) ? " on" : "")} onClick=${() => toggle(f)}>
                      <${Check} checked=${columns.includes(f)} onChange=${() => toggle(f)} />
                      <code class="grow">${f}</code>
                      ${columns.includes(f) && html`<span class="chip accent">${columns.indexOf(f) + 1}</span>`}
                    </div>`
                  )}
                </div>
              </div>
              <${Switch} checked=${unique} onChange=${setUnique} label="Unique values" />
              <${Field} label="Name"><${Input} class="mono" value=${name} placeholder=${autoName} onInput=${setName} /></${Field}>
            `}
        <pre class="code" style="white-space:pre-wrap">${sql}</pre>
        <a class="small" href="#" onClick=${(e) => {
          e.preventDefault();
          if (!raw) setRawText(sql);
          setRaw(!raw);
        }}>${raw ? "Use the column picker" : "Write the statement myself"}</a>
      </div>
      <div class="sheet-foot">
        <span class="spacer"></span>
        <${Button} variant="ghost" onClick=${close}>Cancel</${Button}>
        <${Button} variant="primary" disabled=${!raw && columns.length === 0} onClick=${() => {
          onSave(sql);
          close();
        }}>${initial ? "Update" : "Add index"}</${Button}>
      </div>
    `}
  </${Modal}>`;
}

export function Indexes({ draft, setDraft, errors }) {
  const [editing, setEditing] = useState(null); // index number or "new"
  const list = draft.indexes || [];
  const save = (sql) => {
    const next = editing === "new" ? [...list, sql] : list.map((x, i) => (i === editing ? sql : x));
    setDraft({ ...draft, indexes: next });
  };
  const errorKeys = Object.keys(errors).filter((k) => k.startsWith("indexes"));
  return html`<div class="stack" style="max-width:900px">
    <div class="row">
      <div class="grow small muted">Indexes speed up filters and sorting; unique indexes reject duplicates.</div>
      <${Button} variant="primary" icon="plus" onClick=${() => setEditing("new")}>New index</${Button}>
    </div>
    ${errorKeys.length > 0 && html`<${Callout} kind="danger">${errors[errorKeys[0]]}</${Callout}>`}
    ${list.length === 0
      ? html`<div class="card"><${Empty} icon="zap" title="No indexes">Add one for the fields you filter or sort by most.</${Empty}></div>`
      : html`<div class="stack tight stagger">
          ${list.map((sql, i) => {
            const idx = parseIndex(sql);
            return html`<div class="card" style=${{ "--i": i, padding: "12px 14px" }}>
              <div class="row">
                <span class="field-type-badge"><${Icon} name="zap" size=${15} /></span>
                <div class="grow">
                  <div class="row" style="gap:8px"><code><b>${idx.name || "index"}</b></code>${idx.unique && html`<span class="chip accent">unique</span>`}</div>
                  <div class="chips" style="margin-top:4px">${idx.columns.map((c) => html`<span class="chip outline mono">${c}</span>`)}</div>
                </div>
                <${IconButton} icon="edit" label="Edit" onClick=${() => setEditing(i)} />
                <${IconButton} icon="trash" label="Remove" onClick=${() => setDraft({ ...draft, indexes: list.filter((_, j) => j !== i) })} />
              </div>
            </div>`;
          })}
        </div>`}
    ${editing !== null &&
    html`<${IndexEditor}
      draft=${draft}
      initial=${editing === "new" ? null : parseIndex(list[editing])}
      onSave=${save}
      onClose=${() => setEditing(null)}
    />`}
  </div>`;
}
