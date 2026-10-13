// The data workspace: the collections panel and a collection's tabs. Schema, rules,
// indexes, auth options and settings edit one draft, saved from the floating save bar.
import { html, useState, useEffect, useMemo, useStore, navigate, route, cx } from "../lib/core.js";
import { api, enc } from "../lib/api.js";
import { app, loadCollections, upsertCollection, removeCollection } from "../lib/state.js";
import { Icon } from "../lib/icons.js";
import { Button, Tabs, CollectionGlyph, CopyButton, Empty, Modal, SheetHead, Field, Input, Segmented, Callout, toast, confirmDialog, Menu } from "../lib/ui.js";
import { Records } from "./records.js";
import { Schema } from "./schema.js";
import { Rules } from "./rules.js";
import { Indexes } from "./indexes.js";
import { AuthOptions } from "./auth-options.js";
import { ApiDocs } from "./api-docs.js";

function clone(v) {
  return JSON.parse(JSON.stringify(v));
}

function CollectionsPanel({ active, onNew }) {
  const collections = useStore(app, (s) => s.collections);
  const locked = useStore(app, (s) => s.hideControls);
  const [query, setQuery] = useState("");
  const [closed, setClosed] = useState({ system: true });
  const q = query.trim().toLowerCase();
  const visible = collections.filter((c) => !q || c.name.toLowerCase().includes(q));
  const groups = [
    { id: "base", title: "Collections", items: visible.filter((c) => !c.system && c.type !== "auth") },
    { id: "auth", title: "Auth", items: visible.filter((c) => !c.system && c.type === "auth") },
    { id: "system", title: "System", items: visible.filter((c) => c.system) },
  ];
  return html`<aside class="collections-panel" aria-label="Collections">
    <div class="filter-box" style="flex:none;height:36px">
      <${Icon} name="search" size=${15} />
      <input placeholder="Find a collection" value=${query} onInput=${(e) => setQuery(e.target.value)} style="font-family:var(--font)" />
    </div>
    ${groups.map(
      (g) =>
        (g.items.length > 0 || (g.id === "base" && !q)) &&
        html`<div>
          <div
            class=${cx("group-title section-title", { closed: closed[g.id] && !q })}
            onClick=${() => setClosed({ ...closed, [g.id]: !closed[g.id] })}
          >
            <${Icon} name="chevronDown" size=${13} /> ${g.title}<span class="spacer"></span><span>${g.items.length}</span>
          </div>
          ${(!closed[g.id] || q) &&
          html`<div class="stack stagger" style="gap:2px">
            ${g.items.map(
              (c, i) => html`<a href=${"#/data/" + enc(c.name)} class=${cx("coll-item", { on: c.name === active })} style=${{ "--i": i }}>
                <${CollectionGlyph} collection=${c} />
                <span class="grow ellipsis">${c.name}</span>
              </a>`
            )}
            ${g.id === "base" && g.items.length === 0 && html`<div class="small faint" style="padding:6px 10px">No collections yet.</div>`}
          </div>`}
        </div>`
    )}
    <span class="spacer"></span>
    ${!locked && html`<${Button} icon="plus" class="block" onClick=${onNew}>New collection</${Button}>`}
  </aside>`;
}

function NewCollection({ onClose }) {
  const [name, setName] = useState("");
  const [type, setType] = useState("base");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const submit = async (e, close) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const scaffolds = await api.get("/api/collections/meta/scaffolds");
      const body = clone(scaffolds[type] || { type, fields: [] });
      delete body.id;
      body.name = name.trim();
      if (type === "view") body.viewQuery = "SELECT id FROM _superusers";
      const created = await api.post("/api/collections", body);
      upsertCollection(created);
      toast(`Created “${created.name}”.`);
      close();
      navigate(`/data/${enc(created.name)}/schema`);
    } catch (err) {
      const fields = err.fieldErrors ? err.fieldErrors() : {};
      setError(fields.name || Object.values(fields)[0] || err.message);
    } finally {
      setBusy(false);
    }
  };
  const types = [
    { value: "base", label: "Base" },
    { value: "auth", label: "Auth" },
  ];
  const describe = {
    base: "Records with your own fields — posts, products, anything.",
    auth: "Users who can sign in: email, password, OAuth2, OTP and MFA.",
  };
  return html`<${Modal} size="narrow" onClose=${onClose}>
    ${(close) => html`<form onSubmit=${(e) => submit(e, close)}>
      <${SheetHead} title="New collection" icon="data" onClose=${close} />
      <div class="sheet-body stack">
        <${Field} label="Name" error=${error} help="Letters, numbers and underscores, e.g. posts">
          <${Input} value=${name} onInput=${setName} autofocus placeholder="posts" class="mono" required />
        </${Field}>
        <div class="field">
          <span class="label">Type</span>
          <${Segmented} value=${type} onChange=${setType} options=${types} />
          <span class="help">${describe[type]}</span>
        </div>
      </div>
      <div class="sheet-foot">
        <span class="spacer"></span>
        <${Button} variant="ghost" onClick=${close}>Cancel</${Button}>
        <${Button} variant="primary" type="submit" loading=${busy} disabled=${!name.trim()}>Create</${Button}>
      </div>
    </form>`}
  </${Modal}>`;
}

const TABS = (c, locked) =>
  [
    { id: "records", label: "Records", icon: "table" },
    !locked && { id: "schema", label: "Schema", icon: "columns", badge: c.fields.filter((f) => !f.system).length },
    !locked && { id: "rules", label: "API rules", icon: "shield" },
    !locked && c.type === "auth" && { id: "auth", label: "Sign-in", icon: "key" },
    !locked && { id: "indexes", label: "Indexes", icon: "zap", badge: c.indexes.length },
    { id: "api", label: "API", icon: "code" },
    !locked && { id: "settings", label: "Settings", icon: "settings" },
  ].filter(Boolean);

function CollectionSettings({ draft, setDraft, collection }) {
  const remove = async () => {
    const ok = await confirmDialog({
      title: `Delete “${collection.name}”?`,
      message: "Its table and every record and file in it are deleted. This can't be undone.",
      confirm: "Delete collection",
      danger: true,
      typed: collection.name,
    });
    if (!ok) return;
    try {
      await api.delete(`/api/collections/${enc(collection.id)}`);
      removeCollection(collection.id);
      toast(`Deleted “${collection.name}”.`);
      navigate("/data");
    } catch (err) {
      toast.error(err);
    }
  };
  const truncate = async () => {
    const ok = await confirmDialog({
      title: `Delete every record of “${collection.name}”?`,
      message: "Cascading relations and files are deleted too.",
      confirm: "Delete all records",
      danger: true,
      typed: collection.name,
    });
    if (!ok) return;
    try {
      await api.delete(`/api/collections/${enc(collection.id)}/truncate`);
      toast("All records deleted.");
    } catch (err) {
      toast.error(err);
    }
  };
  const duplicate = async () => {
    const copy = clone(collection);
    delete copy.id;
    delete copy.created;
    delete copy.updated;
    copy.name = collection.name + "_copy";
    copy.indexes = [];
    copy.fields = copy.fields.map((f) => ({ ...f, id: "" }));
    try {
      const created = await api.post("/api/collections", copy);
      upsertCollection(created);
      toast(`Created “${created.name}”.`);
      navigate(`/data/${enc(created.name)}/schema`);
    } catch (err) {
      toast.error(err);
    }
  };
  return html`<div class="stack loose enter" style="max-width:760px">
    <div class="card">
      <div class="card-head"><h3>General</h3></div>
      <div class="card-body stack">
        <${Field} label="Name" help=${collection.system ? "System collections can't be renamed." : "Renaming keeps records, but clients using the old name break."}>
          <${Input} class="mono" value=${draft.name} disabled=${collection.system} onInput=${(v) => setDraft({ ...draft, name: v })} />
        </${Field}>
        <div class="row small muted">ID <code>${collection.id}</code><${CopyButton} text=${collection.id} /></div>
      </div>
    </div>
    ${!collection.system &&
    html`<div class="card" style="border-color:color-mix(in srgb,var(--danger),transparent 70%)">
      <div class="card-head"><h3 style="color:var(--danger)">Danger zone</h3></div>
      <div class="card-body stack">
        <div class="row"><div class="grow"><b>Duplicate</b><div class="small muted">A new collection with the same fields and rules, without records.</div></div><${Button} icon="copy" onClick=${duplicate}>Duplicate</${Button}></div>
        <div class="row"><div class="grow"><b>Delete all records</b><div class="small muted">Empties the collection; the schema stays.</div></div><${Button} variant="danger-ghost" icon="trash" onClick=${truncate}>Truncate</${Button}></div>
        <div class="row"><div class="grow"><b>Delete the collection</b><div class="small muted">Removes the table, its records and files.</div></div><${Button} variant="danger" icon="trash" onClick=${remove}>Delete</${Button}></div>
      </div>
    </div>`}
  </div>`;
}

function CollectionView({ collection, tab, recordId }) {
  const [draft, setDraft] = useState(() => clone(collection));
  const [saving, setSaving] = useState(false);
  const [errors, setErrors] = useState({});
  useEffect(() => {
    setDraft(clone(collection));
    setErrors({});
  }, [collection.id, collection.updated]);
  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(collection), [draft, collection]);
  const locked = useStore(app, (s) => s.hideControls);
  const collections = useStore(app, (s) => s.collections);
  const tabs = TABS(collection, locked);
  const active = tabs.some((t) => t.id === tab) ? tab : "records";

  const save = async () => {
    setSaving(true);
    setErrors({});
    try {
      const saved = await api.patch(`/api/collections/${enc(collection.id)}`, draft);
      upsertCollection(saved);
      toast("Changes saved.");
      if (saved.name !== collection.name) navigate(`/data/${enc(saved.name)}/${active}`, true);
    } catch (err) {
      const fields = err.fieldErrors ? err.fieldErrors() : {};
      setErrors(fields);
      const key = Object.keys(fields)[0];
      if (key) {
        const m = /^fields\.(\d+)\.(.+)$/.exec(key);
        const where = m ? `${(draft.fields[Number(m[1])] || {}).name || "field"} → ${m[2]}` : key;
        toast.error(`${where}: ${fields[key]}`);
      } else {
        toast.error(err);
      }
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    const fn = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s" && dirty) {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, [dirty, draft]);

  const props = { draft, setDraft, collection, errors };
  return html`<div class="enter" key=${collection.id}>
    <div class="coll-header">
      <${CollectionGlyph} collection=${collection} />
      <div class="grow">
        <div class="row" style="gap:8px">
          <h1 style="font-size:22px">${collection.name}</h1>
          <span class=${"chip " + (collection.type === "auth" ? "accent" : "outline")}>${collection.type}</span>
          ${collection.system && html`<span class="chip warn"><${Icon} name="lock" size=${12} /> system</span>`}
        </div>
        <div class="small muted row" style="gap:4px">${collection.id}<${CopyButton} text=${collection.id} label="Copy id" /></div>
      </div>
      <select class="input mobile-only" style="width:auto" aria-label="Collection" value=${collection.name} onChange=${(e) => navigate(`/data/${enc(e.target.value)}`)}>
        ${collections.map((c) => html`<option value=${c.name} selected=${c.id === collection.id}>${c.name}</option>`)}
      </select>
    </div>
    ${locked && html`<div style="margin-bottom:14px"><${Callout} icon="lock">Schema editing is locked in Settings → Application.</${Callout}></div>`}
    <${Tabs} tabs=${tabs} active=${active} onChange=${(id) => navigate(`/data/${enc(collection.name)}/${id}`)} />
    <div key=${active} class="enter">
      ${active === "records" && html`<${Records} collection=${collection} recordId=${recordId} />`}
      ${active === "schema" && html`<${Schema} ...${props} />`}
      ${active === "rules" && html`<${Rules} ...${props} />`}
      ${active === "auth" && html`<${AuthOptions} ...${props} />`}
      ${active === "indexes" && html`<${Indexes} ...${props} />`}
      ${active === "api" && html`<${ApiDocs} collection=${collection} />`}
      ${active === "settings" && html`<${CollectionSettings} ...${props} />`}
    </div>
    ${dirty &&
    active !== "records" &&
    html`<div class="savebar">
      <${Icon} name="edit" size=${16} />
      <span class="grow">Unsaved changes to <b>${collection.name}</b></span>
      <${Button} variant="ghost" size="sm" icon="undo" onClick=${() => {
        setDraft(clone(collection));
        setErrors({});
      }}>Discard</${Button}>
      <${Button} variant="primary" size="sm" loading=${saving} onClick=${save}>Save changes</${Button}>
    </div>`}
  </div>`;
}

export function DataWorkspace({ collection: name, tab, record }) {
  const collections = useStore(app, (s) => s.collections);
  const loaded = useStore(app, (s) => s.collectionsLoaded);
  const { query } = useStore(route);
  const [creating, setCreating] = useState(false);
  useEffect(() => {
    if (query.new) setCreating(true);
  }, [query.new]);
  useEffect(() => {
    if (!loaded) loadCollections().catch(() => {});
  }, []);

  const collection = name && collections.find((c) => c.name === name || c.id === name);
  // without a collection, open the first user collection
  useEffect(() => {
    if (!name && loaded) {
      const first = collections.find((c) => !c.system) || collections[0];
      if (first && !query.new) navigate(`/data/${enc(first.name)}`, true);
    }
  }, [name, loaded]);

  return html`<div class="workspace">
    <${CollectionsPanel} active=${collection && collection.name} onNew=${() => setCreating(true)} />
    <section class="workspace-main">
      ${collection
        ? html`<${CollectionView} key=${collection.id} collection=${collection} tab=${tab || "records"} recordId=${record} />`
        : loaded && name
        ? html`<${Empty} icon="search" title="Collection not found">There's no collection named “${name}”.</${Empty}>`
        : loaded && !collections.some((c) => !c.system)
        ? html`<${Empty} icon="data" title="Create your first collection" action=${html`<${Button} variant="primary" icon="plus" onClick=${() => setCreating(true)}>New collection</${Button}>`}>
            Collections are tables with an API: fields, validation and access rules.
          </${Empty}>`
        : null}
    </section>
    ${creating &&
    html`<${NewCollection}
      onClose=${() => {
        setCreating(false);
        if (query.new) navigate(location.hash.replace(/^#/, "").replace(/[?&]new=1/, ""), true);
      }}
    />`}
  </div>`;
}
