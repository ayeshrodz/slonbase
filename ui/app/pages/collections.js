// The first level of Data: every collection as a card, grouped as the sidebar groups them.
// A card opens the collection (records, schema, rules...), the second level (data.js).
import { html, useState, useEffect, useStore, navigate, cx } from "../lib/core.js";
import { api, enc } from "../lib/api.js";
import { app } from "../lib/state.js";
import { Icon } from "../lib/icons.js";
import { Button, CollectionGlyph, Skeleton, Empty } from "../lib/ui.js";
import { plural, relativeTime } from "../lib/format.js";

// The groups of collections, in the sidebar and here: the ones made here (and views of them),
// the ones that sign people in, and the system's.
export function groupCollections(collections) {
  return [
    { id: "general", title: "General", items: collections.filter((c) => !c.system && c.type !== "auth") },
    { id: "auth", title: "Auth", items: collections.filter((c) => !c.system && c.type === "auth") },
    { id: "system", title: "System", items: collections.filter((c) => c.system) },
  ];
}

// who may list its records, in a few words (the list rule: null is superusers only, "" is anyone)
function access(c) {
  const rule = c.listRule;
  if (rule === null || rule === undefined) return { label: "Superusers only", tone: "" };
  if (rule === "") return { label: "Public", tone: "warn" };
  return { label: "By rule", tone: "info" };
}

function CollectionCard({ collection: c, count, estimated, index }) {
  const who = access(c);
  const fields = c.fields.filter((f) => !f.system).length;
  // (a view has no table to count; where the counts can't be had, the card is without them)
  const records =
    c.type === "view" ? "A query" : count === undefined ? html`<${Skeleton} width="42px" height=${12} />` : count === null ? "" : `${estimated ? "~" : ""}${plural(count, "record")}`;
  return html`<a class="card coll-card" href=${"#/data/" + enc(c.name)} style=${{ "--i": index }}>
    <div class="coll-card-top">
      <${CollectionGlyph} collection=${c} size=${34} />
      <div class="grow">
        <div class="coll-card-name ellipsis">${c.name}</div>
        <div class="tiny muted">${c.type === "auth" ? "Auth" : c.type === "view" ? "View" : "Base"}${c.system ? " · system" : ""}</div>
      </div>
      <${Icon} name="chevronRight" size=${16} class="faint" />
    </div>
    <div class="coll-card-stats">
      <div><span class="coll-card-num">${records}</span></div>
      <div class="muted small">${plural(fields, "field")}</div>
    </div>
    <div class="coll-card-foot">
      <span class=${"chip " + (who.tone || "outline")}>${who.label}</span>
      ${c.updated && html`<span class="tiny faint ellipsis" title="When its schema or rules last changed">changed ${relativeTime(c.updated)}</span>`}
    </div>
  </a>`;
}

export function DataOverview({ onNew }) {
  const collections = useStore(app, (s) => s.collections);
  const loaded = useStore(app, (s) => s.collectionsLoaded);
  const locked = useStore(app, (s) => s.hideControls);
  const [query, setQuery] = useState("");
  const [closed, setClosed] = useState({ system: true });
  // records of every collection, in one request (exact for small tables, an estimate for
  // very large ones); `undefined` while they load, `null` when they can't be told
  const [counts, setCounts] = useState(undefined);
  const [estimated, setEstimated] = useState([]);
  useEffect(() => {
    let live = true;
    api
      .get("/api/collections/meta/counts")
      .then((r) => {
        if (!live) return;
        setCounts(r.counts || {});
        setEstimated(r.estimated || []);
      })
      .catch(() => live && setCounts(null));
    return () => {
      live = false;
    };
  }, [collections.length]);

  const q = query.trim().toLowerCase();
  const shown = collections.filter((c) => !q || c.name.toLowerCase().includes(q));
  const groups = groupCollections(shown);
  const user = collections.filter((c) => !c.system);

  return html`<div class="page enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><${Icon} name="data" size=${14} /> Data</div>
        <h1>Collections</h1>
        <p class="muted small" style="margin:4px 0 0">${loaded ? `${plural(user.length, "collection")} you made, and the ones the system keeps. Open one for its records, schema and rules.` : ""}</p>
      </div>
      ${!locked && html`<${Button} variant="primary" icon="plus" onClick=${onNew}>New collection</${Button}>`}
    </div>
    ${!loaded
      ? html`<div class="coll-grid">${[0, 1, 2, 3].map(() => html`<${Skeleton} height=${118} />`)}</div>`
      : html`
          ${user.length === 0 &&
          !q &&
          html`<${Empty} icon="data" title="Create your first collection" action=${!locked && html`<${Button} variant="primary" icon="plus" onClick=${onNew}>New collection</${Button}>`}>
            Collections are tables with an API: fields, validation and access rules.
          </${Empty}>`}
          ${collections.length > 6 &&
          html`<label class="tree-filter coll-search">
            <${Icon} name="search" size=${14} />
            <input placeholder="Filter collections" value=${query} onInput=${(e) => setQuery(e.target.value)} aria-label="Filter collections" />
          </label>`}
          ${groups.map(
            (g) =>
              g.items.length > 0 &&
              html`<section class="coll-section">
                <button class=${cx("coll-section-title", { closed: closed[g.id] && !q })} aria-expanded=${!closed[g.id] || !!q} onClick=${() => setClosed({ ...closed, [g.id]: !closed[g.id] })}>
                  <${Icon} name="chevronDown" size=${13} /> ${g.title}<span class="tree-count">${g.items.length}</span>
                </button>
                ${(!closed[g.id] || q) &&
                html`<div class="coll-grid">
                  ${g.items.map((c, i) => html`<${CollectionCard} key=${c.id} collection=${c} index=${i} count=${counts === undefined ? undefined : counts === null ? null : counts[c.id] ?? null} estimated=${estimated.includes(c.id)} />`)}
                </div>`}
              </section>`
          )}
          ${q && groups.every((g) => g.items.length === 0) && html`<div class="muted small" style="padding:20px 4px">Nothing matches “${query}”.</div>`}
        `}
  </div>`;
}
