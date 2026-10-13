// Inputs for each field type in the record editor.
import { html, useState, useEffect, useRef, cx, useDebounced } from "../lib/core.js";
import { api, qs, enc, fileUrl, getFileToken } from "../lib/api.js";
import { Icon, fieldMeta } from "../lib/icons.js";
import { Button, IconButton, Field, Input, TextArea, Switch, Modal, SheetHead, Check, Skeleton, Empty, Segmented } from "../lib/ui.js";
import { isImage, bytes, recordLabel, truncate } from "../lib/format.js";
import { collectionByName } from "../lib/state.js";

// "2025-01-02 10:00:00.000Z" <-> the local value of <input type=datetime-local>
function toLocalInput(value) {
  if (!value) return "";
  const d = new Date(String(value).replace(" ", "T"));
  if (isNaN(d)) return "";
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}
function fromLocalInput(value) {
  if (!value) return "";
  const d = new Date(value);
  return isNaN(d) ? "" : d.toISOString().replace("T", " ");
}

function JsonInput({ value, onChange, invalid }) {
  const [text, setText] = useState(() => (value === undefined || value === null ? "" : JSON.stringify(value, null, 2)));
  const [bad, setBad] = useState(false);
  return html`<div class="stack tight">
    <${TextArea}
      code
      rows=${6}
      value=${text}
      invalid=${invalid || bad}
      onInput=${(t) => {
        setText(t);
        if (!t.trim()) {
          setBad(false);
          onChange(null);
          return;
        }
        try {
          onChange(JSON.parse(t));
          setBad(false);
        } catch (_) {
          setBad(true);
        }
      }}
    />
    <span class=${"tiny " + (bad ? "" : "faint")} style=${bad ? "color:var(--danger)" : ""}>${bad ? "Not valid JSON yet — the last valid value is kept." : "JSON"}</span>
  </div>`;
}

function EditorInput({ value, onChange, invalid }) {
  const [mode, setMode] = useState("edit");
  return html`<div class="stack tight">
    <${Segmented} value=${mode} onChange=${setMode} options=${[{ value: "edit", label: "HTML" }, { value: "preview", label: "Preview" }]} />
    ${mode === "edit"
      ? html`<${TextArea} rows=${8} value=${value} invalid=${invalid} onInput=${onChange} code />`
      : html`<iframe
          sandbox=""
          title="Preview"
          srcdoc=${`<style>body{font:14px/1.6 system-ui;margin:12px;color:#333}</style>${value || ""}`}
          style="width:100%;min-height:200px;border:1px solid var(--line);border-radius:var(--r-sm);background:#fff"
        ></iframe>`}
  </div>`;
}

function SelectInput({ field, value, onChange }) {
  const multi = field.maxSelect > 1;
  const current = multi ? (Array.isArray(value) ? value : value ? [value] : []) : value || "";
  if (!multi) {
    return html`<div class="chips">
      ${field.values.map(
        (v) => html`<span class=${cx("chip selectable", current === v ? "accent" : "outline")} onClick=${() => onChange(current === v ? "" : v)}>
          ${current === v && html`<${Icon} name="check" size=${12} />`} ${v}
        </span>`
      )}
    </div>`;
  }
  return html`<div class="stack tight">
    <div class="chips">
      ${field.values.map((v) => {
        const on = current.includes(v);
        return html`<span
          class=${cx("chip selectable", on ? "accent" : "outline")}
          onClick=${() => onChange(on ? current.filter((x) => x !== v) : [...current, v].slice(0, field.maxSelect))}
        >${on && html`<${Icon} name="check" size=${12} />`} ${v}</span>`;
      })}
    </div>
    <span class="tiny faint">Up to ${field.maxSelect}</span>
  </div>`;
}

// files: kept existing files, removals and new uploads
export function FileInput({ field, record, state, onChange }) {
  const [over, setOver] = useState(false);
  const [token, setToken] = useState("");
  const inputRef = useRef();
  const multi = field.maxSelect > 1;
  const existing = (Array.isArray(record[field.name]) ? record[field.name] : record[field.name] ? [record[field.name]] : []).filter(Boolean);
  const removed = state.removed || [];
  const added = state.added || [];
  useEffect(() => {
    if (field.protected && existing.length) getFileToken().then(setToken).catch(() => {});
  }, []);
  const keep = existing.filter((n) => !removed.includes(n));
  const room = (field.maxSelect || 1) - keep.length - added.length;
  const add = (files) => {
    let list = [...files];
    if (!multi) {
      onChange({ removed: existing, added: list.slice(0, 1) });
      return;
    }
    list = list.slice(0, Math.max(0, room));
    onChange({ removed, added: [...added, ...list] });
  };
  return html`<div
    class=${cx("dropzone", { over })}
    onDragOver=${(e) => {
      e.preventDefault();
      setOver(true);
    }}
    onDragLeave=${() => setOver(false)}
    onDrop=${(e) => {
      e.preventDefault();
      setOver(false);
      add(e.dataTransfer.files);
    }}
  >
    ${(existing.length > 0 || added.length > 0) &&
    html`<div class="file-tiles">
      ${existing.map((name) => {
        const gone = removed.includes(name);
        return html`<div class=${cx("file-tile", { gone })}>
          <a href=${fileUrl(record, name, { token })} target="_blank" rel="noopener" title=${name}>
            ${isImage(name)
              ? html`<img class="preview" src=${fileUrl(record, name, { thumb: "200x140", token })} alt=${name} />`
              : html`<div class="preview"><${Icon} name="file" size=${22} /></div>`}
          </a>
          <div class="name">${name}</div>
          <button
            class="remove"
            type="button"
            aria-label=${gone ? "Keep" : "Remove"}
            onClick=${() => onChange({ added, removed: gone ? removed.filter((n) => n !== name) : [...removed, name] })}
          ><${Icon} name=${gone ? "undo" : "close"} size=${12} /></button>
        </div>`;
      })}
      ${added.map(
        (file, i) => html`<div class="file-tile new">
          ${file.type.startsWith("image/")
            ? html`<img class="preview" src=${URL.createObjectURL(file)} alt=${file.name} />`
            : html`<div class="preview"><${Icon} name="file" size=${22} /></div>`}
          <div class="name">${file.name}</div>
          <div class="name tiny faint" style="padding-top:0">${bytes(file.size)}</div>
          <button class="remove" type="button" aria-label="Remove" onClick=${() => onChange({ removed, added: added.filter((_, j) => j !== i) })}>
            <${Icon} name="close" size=${12} />
          </button>
        </div>`
      )}
    </div>`}
    <div class="row small muted">
      <${Icon} name="upload" size=${16} />
      <span class="grow">Drop ${multi ? "files" : "a file"} here or</span>
      <${Button} size="xs" onClick=${() => inputRef.current.click()} disabled=${multi && room <= 0}>Browse</${Button}>
      <input ref=${inputRef} type="file" hidden multiple=${multi} onChange=${(e) => {
        add(e.target.files);
        e.target.value = "";
      }} />
    </div>
    ${field.protected && html`<span class="tiny faint"><${Icon} name="lock" size=${11} /> Protected: needs a file token to download</span>`}
  </div>`;
}

function RelationPicker({ field, value, onClose, onPick }) {
  const target = collectionByName(field.collectionId);
  const multi = field.maxSelect > 1;
  const [selected, setSelected] = useState(() => (Array.isArray(value) ? value : value ? [value] : []));
  const [query, setQuery] = useState("");
  const q = useDebounced(query, 250);
  const [items, setItems] = useState(null);
  useEffect(() => {
    if (!target) return;
    const text = target.fields.filter((f) => ["text", "email", "url", "editor"].includes(f.type) && !f.hidden).map((f) => f.name);
    const filter = q ? text.map((n) => `${n} ~ ${JSON.stringify(q)}`).join(" || ") : "";
    api
      .get(`/api/collections/${enc(target.name)}/records` + qs({ perPage: 50, filter, sort: target.fields.some((f) => f.name === "created") ? "-created" : "" }))
      .then((r) => setItems(r.items))
      .catch(() => setItems([]));
  }, [q]);
  const toggle = (id) => {
    if (!multi) {
      setSelected([id]);
      return;
    }
    setSelected(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id].slice(0, field.maxSelect || 999));
  };
  return html`<${Modal} size="narrow" onClose=${onClose}>
    ${(close) => html`
      <${SheetHead} title=${`Pick from ${target ? target.name : "?"}`} subtitle=${multi ? `Up to ${field.maxSelect}` : "One record"} icon="relation" onClose=${close} />
      <div class="sheet-body stack">
        <div class="filter-box" style="flex:none"><${Icon} name="search" size=${15} /><input autofocus placeholder="Search" value=${query} onInput=${(e) => setQuery(e.target.value)} style="font-family:var(--font)" /></div>
        <div class="picker-list">
          ${items === null
            ? [0, 1, 2].map(() => html`<${Skeleton} height=${36} />`)
            : items.length === 0
            ? html`<div class="small muted" style="padding:12px">No records found.</div>`
            : items.map(
                (r) => html`<div class=${cx("picker-item", { on: selected.includes(r.id) })} onClick=${() => toggle(r.id)}>
                  <${Check} checked=${selected.includes(r.id)} onChange=${() => toggle(r.id)} />
                  <div class="grow"><div class="ellipsis">${truncate(recordLabel(r, target), 60)}</div><div class="tiny faint mono">${r.id}</div></div>
                </div>`
              )}
        </div>
      </div>
      <div class="sheet-foot">
        <span class="small muted">${selected.length} selected</span><span class="spacer"></span>
        <${Button} variant="ghost" onClick=${close}>Cancel</${Button}>
        <${Button} variant="primary" onClick=${() => {
          onPick(multi ? selected : selected[0] || "");
          close();
        }}>Done</${Button}>
      </div>
    `}
  </${Modal}>`;
}

function RelationInput({ field, value, record, onChange }) {
  const [open, setOpen] = useState(false);
  const target = collectionByName(field.collectionId);
  const ids = Array.isArray(value) ? value : value ? [value] : [];
  const expanded = (record.expand && record.expand[field.name]) || [];
  const known = Array.isArray(expanded) ? expanded : [expanded];
  return html`<div class="stack tight">
    <div class="chips">
      ${ids.map((id) => {
        const r = known.find((x) => x && x.id === id);
        return html`<span class="chip outline"><${Icon} name="link" size=${11} />${r ? truncate(recordLabel(r, target), 30) : id}
          <button type="button" aria-label="Remove" onClick=${() => onChange(field.maxSelect > 1 ? ids.filter((x) => x !== id) : "")}><${Icon} name="close" size=${12} /></button>
        </span>`;
      })}
      <${Button} size="xs" icon="plus" onClick=${() => setOpen(true)}>${ids.length ? "Change" : "Pick"}</${Button}>
    </div>
    ${!target && html`<span class="tiny" style="color:var(--danger)">The related collection is missing.</span>`}
    ${open && target && html`<${RelationPicker} field=${field} value=${value} onClose=${() => setOpen(false)} onPick=${onChange} />`}
  </div>`;
}

function GeoInput({ value, onChange }) {
  const v = value || { lat: 0, lon: 0 };
  return html`<div class="grid-2">
    <${Field} label="Latitude"><${Input} type="number" step="any" min="-90" max="90" value=${v.lat} onInput=${(x) => onChange({ ...v, lat: Number(x) })} /></${Field}>
    <${Field} label="Longitude"><${Input} type="number" step="any" min="-180" max="180" value=${v.lon} onInput=${(x) => onChange({ ...v, lon: Number(x) })} /></${Field}>
  </div>`;
}

// the input for one field; file fields use `files` state instead of `value`
export function FieldInput({ field, value, record, onChange, error, files, onFiles, isNew }) {
  const meta = fieldMeta(field.type);
  const required = field.required && field.type !== "bool";
  const label = html`<span class="row" style="gap:6px"><${Icon} name=${meta.icon} size=${14} />${field.name}</span>`;
  let input;
  switch (field.type) {
    case "bool":
      return html`<div class="field">
        <${Switch} checked=${!!value} onChange=${onChange} label=${html`<span class="row" style="gap:6px"><${Icon} name=${meta.icon} size=${14} />${field.name}</span>`} />
        ${error && html`<span class="error">${error}</span>`}
        ${field.help && html`<span class="help">${field.help}</span>`}
      </div>`;
    case "number":
      input = html`<${Input} type="number" step=${field.onlyInt ? 1 : "any"} min=${field.min ?? undefined} max=${field.max ?? undefined} value=${value} invalid=${!!error} onInput=${(v) => onChange(v === "" ? 0 : Number(v))} />`;
      break;
    case "email":
      input = html`<${Input} type="email" value=${value} invalid=${!!error} onInput=${onChange} />`;
      break;
    case "url":
      input = html`<${Input} type="url" value=${value} invalid=${!!error} onInput=${onChange} placeholder="https://" />`;
      break;
    case "date":
      input = html`<div class="row"><${Input} type="datetime-local" step="1" value=${toLocalInput(value)} invalid=${!!error} onInput=${(v) => onChange(fromLocalInput(v))} />
        ${value && html`<${IconButton} icon="close" label="Clear" onClick=${() => onChange("")} />`}</div>`;
      break;
    case "autodate":
      input = html`<${Input} value=${value} readonly placeholder="Set automatically" />`;
      break;
    case "select":
      input = html`<${SelectInput} field=${field} value=${value} onChange=${onChange} />`;
      break;
    case "json":
      input = html`<${JsonInput} value=${value} onChange=${onChange} invalid=${!!error} />`;
      break;
    case "editor":
      input = html`<${EditorInput} value=${value} onChange=${onChange} invalid=${!!error} />`;
      break;
    case "file":
      input = html`<${FileInput} field=${field} record=${record} state=${files || {}} onChange=${onFiles} />`;
      break;
    case "relation":
      input = html`<${RelationInput} field=${field} value=${value} record=${record} onChange=${onChange} />`;
      break;
    case "geoPoint":
      input = html`<${GeoInput} value=${value} onChange=${onChange} />`;
      break;
    default:
      if (field.name === "id") {
        input = html`<${Input} class="mono" value=${value} readonly=${!isNew} invalid=${!!error} onInput=${onChange} placeholder=${isNew ? "Leave empty to generate" : ""} />`;
      } else {
        input = field.max > 255 || (!field.max && field.pattern === "" && /description|content|body|bio|notes?/.test(field.name))
          ? html`<${TextArea} value=${value} invalid=${!!error} onInput=${onChange} rows=${3} />`
          : html`<${Input} value=${value} invalid=${!!error} onInput=${onChange} />`;
      }
  }
  return html`<${Field} label=${label} required=${required} error=${error} help=${field.help}>${input}</${Field}>`;
}
