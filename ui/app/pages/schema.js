// The schema editor: fields as cards that can be reordered, configured and removed.
import { html, useState, useRef, useEffect, useStore, cx } from "../lib/core.js";
import { Icon, FIELD_TYPES, fieldMeta } from "../lib/icons.js";
import { Button, IconButton, Field, Input, TextArea, Switch, Modal, SheetHead, Menu, Callout, Select } from "../lib/ui.js";
import { app } from "../lib/state.js";

function list(text) {
  return text
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function num(v) {
  return v === "" || v === null || v === undefined ? null : Number(v);
}

function defaults(type) {
  const base = { id: "", name: "", type, system: false, hidden: false, presentable: false, help: "" };
  const extra = {
    text: { min: 0, max: 0, pattern: "", autogeneratePattern: "", required: false, primaryKey: false },
    editor: { maxSize: 0, convertURLs: false, required: false },
    number: { min: null, max: null, onlyInt: false, required: false },
    bool: { required: false },
    email: { exceptDomains: [], onlyDomains: [], required: false },
    url: { exceptDomains: [], onlyDomains: [], required: false },
    date: { min: "", max: "", required: false },
    autodate: { onCreate: true, onUpdate: false },
    select: { values: [], maxSelect: 1, required: false },
    file: { maxSelect: 1, maxSize: 0, mimeTypes: [], thumbs: [], protected: false, required: false },
    relation: { collectionId: "", cascadeDelete: false, minSelect: 0, maxSelect: 1, required: false },
    json: { maxSize: 0, required: false },
    password: { min: 8, max: 0, pattern: "", cost: 0, required: false },
    geoPoint: { required: false },
  };
  return { ...base, ...(extra[type] || {}) };
}

function ValuesEditor({ values, onChange }) {
  const [text, setText] = useState("");
  const add = () => {
    const v = text.trim();
    if (v && !values.includes(v)) onChange([...values, v]);
    setText("");
  };
  return html`<div class="stack tight">
    <div class="chips">
      ${values.map(
        (v) => html`<span class="chip accent">${v}<button type="button" aria-label="Remove" onClick=${() => onChange(values.filter((x) => x !== v))}><${Icon} name="close" size=${12} /></button></span>`
      )}
      ${values.length === 0 && html`<span class="small faint">No values yet</span>`}
    </div>
    <div class="row">
      <${Input} value=${text} onInput=${setText} placeholder="Add a value and press Enter" onKeyDown=${(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          add();
        }
      }} />
      <${Button} size="sm" icon="plus" onClick=${add}>Add</${Button}>
    </div>
  </div>`;
}

function TypeOptions({ field, set, collections, collectionId, errs = {} }) {
  const n = (key, label, help, attrs = {}) => html`<${Field} label=${label} help=${help} error=${errs[key]}>
    <${Input} type="number" value=${field[key] ?? ""} onInput=${(v) => set({ [key]: attrs.nullable ? num(v) : Number(v || 0) })} ...${attrs} />
  </${Field}>`;
  const t = (key, label, help, attrs = {}) => html`<${Field} label=${label} help=${help} error=${errs[key]}>
    <${Input} class=${attrs.mono ? "mono" : ""} value=${field[key] ?? ""} onInput=${(v) => set({ [key]: v })} placeholder=${attrs.placeholder} />
  </${Field}>`;
  const l = (key, label, help, placeholder) => html`<${Field} label=${label} help=${help} error=${errs[key]}>
    <${Input} value=${(field[key] || []).join(", ")} onInput=${(v) => set({ [key]: list(v) })} placeholder=${placeholder} />
  </${Field}>`;
  switch (field.type) {
    case "text":
      return html`<div class="grid-2">
        ${n("min", "Min length")}${n("max", "Max length", "0 for no limit")}
        ${t("pattern", "Pattern", "A regular expression the value must match", { mono: true, placeholder: "^[a-z]+$" })}
        ${t("autogeneratePattern", "Autogenerate", "Fills empty values, e.g. [a-z0-9]{8}", { mono: true })}
      </div>`;
    case "editor":
      return html`<div class="grid-2">${n("maxSize", "Max size (bytes)", "0 for the default")}
        <div class="field"><span class="label">URLs</span><${Switch} checked=${field.convertURLs} onChange=${(v) => set({ convertURLs: v })} label="Convert to relative" /></div></div>`;
    case "number":
      return html`<div class="grid-3">${n("min", "Min", "", { nullable: true, step: "any" })}${n("max", "Max", "", { nullable: true, step: "any" })}
        <div class="field"><span class="label">Integers</span><${Switch} checked=${field.onlyInt} onChange=${(v) => set({ onlyInt: v })} label="Only integers" /></div></div>`;
    case "email":
    case "url":
      return html`<div class="grid-2">${l("onlyDomains", "Only domains", "Comma separated", "example.com")}${l("exceptDomains", "Except domains", "Comma separated")}</div>`;
    case "date":
      return html`<div class="grid-2">${t("min", "Earliest", "", { placeholder: "2025-01-01 00:00:00.000Z" })}${t("max", "Latest")}</div>`;
    case "autodate":
      return html`<div class="row wrap" style="gap:22px">
        <${Switch} checked=${field.onCreate} onChange=${(v) => set({ onCreate: v })} label="Set on create" />
        <${Switch} checked=${field.onUpdate} onChange=${(v) => set({ onUpdate: v })} label="Set on update" />
      </div>`;
    case "select":
      return html`<div class="stack">
        <${Field} label="Values" error=${errs.values}><${ValuesEditor} values=${field.values || []} onChange=${(v) => set({ values: v })} /></${Field}>
        <div class="grid-2">${n("maxSelect", "Max selected", "More than 1 allows several values", { min: 1 })}</div>
      </div>`;
    case "file":
      return html`<div class="grid-2">
        ${n("maxSelect", "Max files", "", { min: 1 })}${n("maxSize", "Max size per file (bytes)", "0 for 5 MB")}
        ${l("mimeTypes", "Allowed types", "Empty allows any", "image/png, image/jpeg")}
        ${l("thumbs", "Thumbnail sizes", "e.g. 100x100, 0x300", "100x100")}
        <div class="field"><span class="label">Access</span><${Switch} checked=${field.protected} onChange=${(v) => set({ protected: v })} label="Protected (needs a file token)" /></div>
      </div>`;
    case "relation":
      return html`<div class="grid-2">
        <${Field} label="Collection" error=${errs.collectionId}>
          <${Select}
            value=${field.collectionId}
            onChange=${(v) => set({ collectionId: v })}
            options=${[{ value: "", label: "Choose a collection" }, ...collections.map((c) => ({ value: c.id, label: c.id === collectionId ? `${c.name} (itself)` : c.name }))]}
          />
        </${Field}>
        ${n("maxSelect", "Max records", "More than 1 stores a list", { min: 1 })}
        ${n("minSelect", "Min records")}
        <div class="field"><span class="label">On delete</span><${Switch} checked=${field.cascadeDelete} onChange=${(v) => set({ cascadeDelete: v })} label="Delete this record too" /></div>
      </div>`;
    case "json":
      return html`<div class="grid-2">${n("maxSize", "Max size (bytes)", "0 for the default")}</div>`;
    case "password":
      return html`<div class="grid-3">${n("min", "Min length")}${n("max", "Max length")}${n("cost", "bcrypt cost", "0 for 10")}</div>`;
    default:
      return null;
  }
}

function FieldCard({ field, index, open, onToggle, onChange, onRemove, onRestore, removed, collections, collectionId, errs = {}, drag, fromView }) {
  const error = Object.values(errs)[0];
  const meta = fieldMeta(field.type);
  const set = (patch) => onChange({ ...field, ...patch });
  const locked = field.system || fromView;
  return html`<div
    class=${cx("field-card", { open, removed, dragging: drag.dragging === index, "drop-target": drag.over === index && drag.dragging !== index })}
    draggable=${drag.handle === index}
    onDragStart=${(e) => {
      e.dataTransfer.effectAllowed = "move";
      drag.start(index);
    }}
    onDragOver=${(e) => {
      e.preventDefault();
      drag.enter(index);
    }}
    onDrop=${(e) => {
      e.preventDefault();
      drag.drop(index);
    }}
    onDragEnd=${drag.end}
  >
    <div class="field-card-head">
      <span class="drag-handle" onMouseDown=${() => drag.grab(index)} onMouseUp=${() => drag.grab(null)} title="Drag to reorder"><${Icon} name="grip" size=${16} /></span>
      <span class="field-type-badge tip" data-tip=${meta.label} style=${{ "--c": `var(--tone-${meta.tone || 1})` }}><${Icon} name=${meta.icon} size=${16} /></span>
      <input
        class="field-name-input"
        value=${field.name}
        disabled=${locked || removed}
        title=${fromView ? "Named by the view's query" : undefined}
        placeholder="field_name"
        aria-label="Field name"
        onInput=${(e) => set({ name: e.target.value })}
      />
      ${error && html`<span class="chip danger tip" data-tip=${error}><${Icon} name="warning" size=${12} /> invalid</span>`}
      ${field.required && html`<span class="chip outline">required</span>`}
      ${field.hidden && html`<span class="chip outline"><${Icon} name="eyeOff" size=${11} /> hidden</span>`}
      ${field.presentable && html`<span class="chip outline">presentable</span>`}
      ${field.system && html`<span class="chip warn"><${Icon} name="lock" size=${11} /> system</span>`}
      ${fromView && !field.system && html`<span class="chip outline">${fieldMeta(field.type).label}</span>`}
      ${removed
        ? html`<${Button} size="xs" icon="undo" onClick=${onRestore}>Restore</${Button}>`
        : html`<${IconButton} size="sm" icon=${open ? "chevronUp" : "settings"} label=${open ? "Close options" : "Options"} onClick=${onToggle} />
            ${!locked && html`<${IconButton} size="sm" icon="trash" label="Remove" onClick=${onRemove} />`}`}
    </div>
    ${open &&
    !removed &&
    html`<div class="field-card-body stack">
      <div class="row wrap" style="gap:22px">
        ${field.type !== "autodate" && field.type !== "bool" && !field.primaryKey && html`<${Switch} checked=${field.required} onChange=${(v) => set({ required: v })} label="Required" />`}
        ${!field.primaryKey && html`<${Switch} checked=${field.hidden} onChange=${(v) => set({ hidden: v })} label="Hidden from the API" />`}
        <${Switch} checked=${field.presentable} onChange=${(v) => set({ presentable: v })} label="Presentable" />
      </div>
      ${errs.name && html`<span class="small" style="color:var(--danger)">Name: ${errs.name}</span>`}
      ${!fromView && html`<${TypeOptions} field=${field} set=${set} collections=${collections} collectionId=${collectionId} errs=${errs} />`}
      <${Field} label="Help text" help="Shown under the input in the record editor">
        <${Input} value=${field.help} onInput=${(v) => set({ help: v })} />
      </${Field}>
    </div>`}
  </div>`;
}

function TypePicker({ onPick, onClose, auth }) {
  const types = FIELD_TYPES.filter((t) => t.type !== "password" || auth);
  return html`<${Modal} size="mid" onClose=${onClose}>
    ${(close) => html`
      <${SheetHead} title="Add a field" subtitle="Pick a type — it can't be changed later" icon="plus" onClose=${close} />
      <div class="sheet-body">
        <div class="type-grid stagger">
          ${types.map(
            (t, i) => html`<button class="type-tile" style=${{ "--i": i }} onClick=${() => {
              onPick(t.type);
              close();
            }}>
              <span class="field-type-badge" style=${{ "--c": `var(--tone-${t.tone})` }}><${Icon} name=${t.icon} size=${16} /></span>
              <span><b>${t.label}</b><div class="small muted">${t.hint}</div></span>
            </button>`
          )}
        </div>
      </div>
    `}
  </${Modal}>`;
}

export function Schema({ draft, setDraft, collection, errors }) {
  const collections = useStore(app, (s) => s.collections);
  const [open, setOpen] = useState(null);
  const [picking, setPicking] = useState(false);
  const [removed, setRemoved] = useState([]);
  const [dragState, setDragState] = useState({ handle: null, dragging: null, over: null });
  const fields = draft.fields;

  const update = (i, field) => setDraft({ ...draft, fields: fields.map((f, j) => (j === i ? field : f)) });
  const add = (type) => {
    const base = defaults(type);
    let name = type === "relation" ? "relation" : type === "file" ? "file" : type;
    let k = 1;
    while (fields.some((f) => f.name === name)) name = `${type}${++k}`;
    base.name = name;
    setDraft({ ...draft, fields: [...fields, base] });
    setOpen(fields.length);
    setTimeout(() => {
      const inputs = document.querySelectorAll(".field-name-input");
      const last = inputs[inputs.length - 1];
      if (last) {
        last.focus();
        last.select();
      }
    }, 60);
  };
  // removed fields stay listed (struck through) until saved
  const remove = (i) => {
    const f = fields[i];
    if (!f.id) {
      setDraft({ ...draft, fields: fields.filter((_, j) => j !== i) });
      return;
    }
    setRemoved([...removed, f]);
    setDraft({ ...draft, fields: fields.filter((_, j) => j !== i) });
  };
  const restore = (f) => {
    setRemoved(removed.filter((r) => r.id !== f.id));
    setDraft({ ...draft, fields: [...fields, f] });
  };

  const drag = {
    ...dragState,
    grab: (i) => setDragState((s) => ({ ...s, handle: i })),
    start: (i) => setDragState((s) => ({ ...s, dragging: i })),
    enter: (i) => setDragState((s) => (s.over === i ? s : { ...s, over: i })),
    drop: (i) => {
      const from = dragState.dragging;
      if (from === null || from === i) return;
      const next = [...fields];
      const [moved] = next.splice(from, 1);
      next.splice(i, 0, moved);
      setDraft({ ...draft, fields: next });
      setOpen(null);
    },
    end: () => setDragState({ handle: null, dragging: null, over: null }),
  };

  const stillRemoved = removed.filter((r) => !fields.some((f) => f.id === r.id) && collection.fields.some((f) => f.id === r.id));
  const fieldErrors = (i) => {
    const prefix = `fields.${i}.`;
    const out = {};
    for (const [k, v] of Object.entries(errors)) if (k.startsWith(prefix)) out[k.slice(prefix.length)] = v;
    return out;
  };
  // open the first field with an error
  useEffect(() => {
    const first = Object.keys(errors).find((k) => /^fields\.\d+\./.test(k));
    if (first) setOpen(Number(first.split(".")[1]));
  }, [errors]);

  return html`<div style="max-width:900px">
    ${collection.type === "view" &&
    html`<div style="margin-bottom:12px"><${Callout}>A view's fields come from its query (edit it in Settings). Here you can set how they're shown and whether they're hidden.</${Callout}></div>`}
    <div class="row" style="margin:4px 0 14px">
      <div class="grow small muted">${fields.length} fields · drag <${Icon} name="grip" size=${13} /> to reorder · system fields are locked</div>
      ${collection.type !== "view" && html`<${Button} variant="primary" icon="plus" onClick=${() => setPicking(true)}>Add field</${Button}>`}
    </div>
    <div class="stack tight">
      ${fields.map(
        (f, i) => html`<${FieldCard}
          key=${f.id || "new-" + i}
          field=${f}
          index=${i}
          open=${open === i}
          onToggle=${() => setOpen(open === i ? null : i)}
          onChange=${(nf) => update(i, nf)}
          onRemove=${() => remove(i)}
          collections=${collections}
          collectionId=${collection.id}
          errs=${fieldErrors(i)}
          drag=${drag}
          fromView=${collection.type === "view"}
        />`
      )}
      ${stillRemoved.map(
        (f) => html`<${FieldCard} key=${"removed-" + f.id} field=${f} removed onRestore=${() => restore(f)} collections=${collections} drag=${{}} />`
      )}
    </div>
    ${stillRemoved.length > 0 &&
    html`<div style="margin-top:12px"><${Callout} kind="warn">Removing a field deletes its column and data when you save${stillRemoved.some((f) => f.type === "file") ? ", and a file field's stored files too" : ""}.</${Callout}></div>`}
    ${picking && html`<${TypePicker} auth=${collection.type === "auth"} onPick=${add} onClose=${() => setPicking(false)} />`}
  </div>`;
}
