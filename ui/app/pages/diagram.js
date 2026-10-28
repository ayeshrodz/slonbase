// The diagram: every collection with its fields, and its relations as curves, laid out by
// dependency (referenced collections to the left of the ones pointing at them) and arranged
// by hand from there. Drawn entirely in SVG with its own styles, so exports look like the screen.
//
// Pan by dragging the background or scrolling, zoom with Ctrl/⌘ + scroll, the buttons or + and -,
// fit with F. Hover or select a collection to focus on its relations; double-click to open it.
import { html, useState, useEffect, useRef, useMemo, useStore, navigate, cx } from "../lib/core.js";
import { api, qs, enc, BASE, PROJECT } from "../lib/api.js";
import { app } from "../lib/state.js";
import { Icon, fieldMeta } from "../lib/icons.js";
import { Button, Segmented, Switch, Menu, toast, tone } from "../lib/ui.js";
import { plural } from "../lib/format.js";

// ------------------------------------------------------------------ geometry

const HEADER = 46;
const ROW = 26;
const FOOT = 10;
const MIN_W = 236;
const MAX_W = 340;
const COL_GAP = 140;
const NODE_GAP = 44;
const MIN_ZOOM = 0.2;
const MAX_ZOOM = 2.2;
const STORE_KEY = "slonbase.diagram" + BASE;
const FONT = "13px Inter, ui-sans-serif, system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif";

let measureCtx;
function textWidth(text, font = FONT) {
  measureCtx = measureCtx || document.createElement("canvas").getContext("2d");
  measureCtx.font = font;
  return measureCtx.measureText(text).width;
}

function clip(text, width, font = FONT) {
  if (textWidth(text, font) <= width) return text;
  let s = text;
  while (s.length > 1 && textWidth(s + "…", font) > width) s = s.slice(0, -1);
  return s + "…";
}

function loadSaved() {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY)) || {};
  } catch (_) {
    return {};
  }
}

function save(state) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(state));
  } catch (_) {}
}

// fields in a single-column unique index
function uniqueFields(c) {
  const out = new Set();
  for (const idx of c.indexes || []) {
    const m = /create\s+unique\s+index[^(]*\(([^)]*)\)/i.exec(idx);
    if (!m) continue;
    const cols = m[1].split(",").map((s) => s.trim().replace(/["`\[\]]/g, "").split(/\s+/)[0]);
    if (cols.length === 1) out.add(cols[0]);
  }
  if (c.type === "auth") out.add("email");
  return out;
}

// ------------------------------------------------------------------ the graph

function buildGraph(collections, { showSystem, detail }) {
  const visible = collections.filter((c) => showSystem || !c.system);
  const byId = new Map(collections.map((c) => [c.id, c]));
  const shown = new Set(visible.map((c) => c.id));
  const nodes = [];
  const edges = [];
  for (const c of visible) {
    const unique = uniqueFields(c);
    const all = c.fields.filter((f) => f.name !== "id");
    let fields;
    if (detail === "names") fields = [];
    else if (detail === "keys") fields = all.filter((f) => f.type === "relation" || unique.has(f.name));
    else fields = all;
    const rows = [{ name: "id", type: "pk", field: null }, ...fields.map((f) => ({ name: f.name, type: f.type, field: f }))];
    if (detail === "names") rows.length = 0;
    // width: the header and the widest row
    let w = textWidth(c.name, "600 14px Inter, ui-sans-serif, system-ui, sans-serif") + 112;
    for (const r of rows) {
      const right = r.type === "relation" ? "→ " + ((byId.get(r.field.collectionId) || {}).name || "?") : r.type === "pk" ? "id" : fieldMeta(r.type).label;
      w = Math.max(w, textWidth(r.name) + textWidth(right, "12px Inter, ui-sans-serif, system-ui, sans-serif") + 84);
    }
    w = Math.min(MAX_W, Math.max(MIN_W, Math.ceil(w / 2) * 2));
    const hiddenFields = detail === "names" ? all.length : all.length - fields.length;
    const h = HEADER + rows.length * ROW + (rows.length ? (hiddenFields ? FOOT + 14 : FOOT) : 0);
    nodes.push({ id: c.id, c, rows, w, h, unique, hiddenFields });
  }
  for (const c of visible) {
    for (const f of c.fields) {
      if (f.type !== "relation" || !f.collectionId || !shown.has(f.collectionId)) continue;
      edges.push({
        id: c.id + ":" + f.name,
        from: c.id,
        to: f.collectionId,
        field: f,
        many: (f.maxSelect || 1) > 1,
        required: !!f.required,
        cascade: !!f.cascadeDelete,
        self: c.id === f.collectionId,
      });
    }
  }
  return { nodes, edges };
}

// Layered layout: a collection's column is one more than the furthest collection it refers to,
// so references point left; columns are ordered to keep curves short (barycenters), and
// collections without relations go in a grid below.
function autoLayout(nodes, edges) {
  const out = new Map(nodes.map((n) => [n.id, []]));
  const inc = new Map(nodes.map((n) => [n.id, []]));
  for (const e of edges) {
    if (e.self) continue;
    out.get(e.from).push(e.to);
    inc.get(e.to).push(e.from);
  }
  const rank = new Map();
  const visiting = new Set();
  const visit = (id) => {
    if (rank.has(id)) return rank.get(id);
    if (visiting.has(id)) return 0; // a cycle: break it here
    visiting.add(id);
    let r = 0;
    for (const to of out.get(id)) r = Math.max(r, visit(to) + 1);
    visiting.delete(id);
    rank.set(id, r);
    return r;
  };
  const connected = nodes.filter((n) => out.get(n.id).length || inc.get(n.id).length);
  const loose = nodes.filter((n) => !out.get(n.id).length && !inc.get(n.id).length);
  connected.forEach((n) => visit(n.id));

  const columns = [];
  for (const n of connected) (columns[rank.get(n.id)] = columns[rank.get(n.id)] || []).push(n);
  for (const col of columns) col && col.sort((a, b) => b.rows.length - a.rows.length || a.c.name.localeCompare(b.c.name));
  const order = new Map();
  const index = () => columns.forEach((col) => col && col.forEach((n, i) => order.set(n.id, i)));
  index();
  // barycenter sweeps, right and then left
  for (let pass = 0; pass < 6; pass++) {
    const forward = pass % 2 === 0;
    const cols = forward ? columns.map((_, i) => i) : columns.map((_, i) => i).reverse();
    for (const ci of cols) {
      const col = columns[ci];
      if (!col) continue;
      const score = (n) => {
        const near = [...out.get(n.id), ...inc.get(n.id)].filter((id) => order.has(id));
        return near.length ? near.reduce((s, id) => s + order.get(id), 0) / near.length : order.get(n.id);
      };
      col.sort((a, b) => score(a) - score(b));
      col.forEach((n, i) => order.set(n.id, i));
    }
  }

  const positions = {};
  let x = 0;
  const heights = columns.map((col) => (col ? col.reduce((s, n) => s + n.h, 0) + NODE_GAP * (col.length - 1) : 0));
  const tallest = Math.max(0, ...heights);
  columns.forEach((col, ci) => {
    if (!col) return;
    const width = Math.max(...col.map((n) => n.w));
    let y = (tallest - heights[ci]) / 2;
    for (const n of col) {
      positions[n.id] = { x: x + (width - n.w) / 2, y };
      y += n.h + NODE_GAP;
    }
    x += width + COL_GAP;
  });
  // the rest in rows below
  if (loose.length) {
    const perRow = Math.max(2, Math.min(6, Math.ceil(Math.sqrt(loose.length * 1.6))));
    let y = connected.length ? tallest + NODE_GAP * 2.5 : 0;
    for (let i = 0; i < loose.length; i += perRow) {
      const row = loose.slice(i, i + perRow);
      let lx = 0;
      for (const n of row) {
        positions[n.id] = { x: lx, y };
        lx += n.w + NODE_GAP;
      }
      y += Math.max(...row.map((n) => n.h)) + NODE_GAP;
    }
  }
  return positions;
}

function bounds(nodes, positions) {
  let x1 = Infinity;
  let y1 = Infinity;
  let x2 = -Infinity;
  let y2 = -Infinity;
  for (const n of nodes) {
    const p = positions[n.id];
    if (!p) continue;
    x1 = Math.min(x1, p.x);
    y1 = Math.min(y1, p.y);
    x2 = Math.max(x2, p.x + n.w);
    y2 = Math.max(y2, p.y + n.h);
  }
  return x1 === Infinity ? { x: 0, y: 0, w: 1, h: 1 } : { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
}

// the curve of a relation, from its field row to the target's header
function edgeGeometry(e, nodes, positions) {
  const a = nodes.get(e.from);
  const b = nodes.get(e.to);
  const pa = positions[e.from];
  const pb = positions[e.to];
  if (!a || !b || !pa || !pb) return null;
  const rowIndex = a.rows.findIndex((r) => r.field && r.field.name === e.field.name);
  const y1 = pa.y + (rowIndex >= 0 ? HEADER + rowIndex * ROW + ROW / 2 : HEADER / 2);
  const y2 = pb.y + HEADER / 2;
  if (e.self) {
    const x = pa.x + a.w;
    const d = `M ${x} ${y1} C ${x + 70} ${y1}, ${x + 70} ${y2}, ${x} ${y2}`;
    return { d, x1: x, y1, x2: x, y2, dir1: 1, dir2: 1, mid: { x: x + 52, y: (y1 + y2) / 2 } };
  }
  const ca = pa.x + a.w / 2;
  const cb = pb.x + b.w / 2;
  let x1;
  let x2;
  let dir1;
  let dir2;
  const overlap = pa.x < pb.x + b.w && pb.x < pa.x + a.w;
  if (overlap) {
    // one above the other: both ends on the left
    x1 = pa.x;
    x2 = pb.x;
    dir1 = -1;
    dir2 = -1;
  } else if (cb > ca) {
    x1 = pa.x + a.w;
    x2 = pb.x;
    dir1 = 1;
    dir2 = -1;
  } else {
    x1 = pa.x;
    x2 = pb.x + b.w;
    dir1 = -1;
    dir2 = 1;
  }
  const reach = overlap ? 70 + Math.abs(y2 - y1) * 0.08 : Math.max(50, Math.abs(x2 - x1) * 0.45);
  const c1 = x1 + dir1 * reach;
  const c2 = x2 + dir2 * reach;
  const d = `M ${x1} ${y1} C ${c1} ${y1}, ${c2} ${y2}, ${x2} ${y2}`;
  // the curve's middle (t = 0.5)
  const mid = { x: 0.125 * x1 + 0.375 * c1 + 0.375 * c2 + 0.125 * x2, y: (y1 + y2) / 2 };
  return { d, x1, y1, x2, y2, dir1, dir2, mid };
}

// the markers at the ends: one (a bar) or many (a crow's foot) at the target, required (a bar)
// or optional (a ring) at the source
function endMarkers(g, e) {
  const t = g.dir2; // the target end points outwards along dir2
  const x = g.x2;
  const y = g.y2;
  const target = e.many
    ? `M ${x + t * 12} ${y} L ${x} ${y - 6} M ${x + t * 12} ${y} L ${x} ${y} M ${x + t * 12} ${y} L ${x} ${y + 6} M ${x + t * 15} ${y - 6} L ${x + t * 15} ${y + 6}`
    : `M ${x + t * 9} ${y - 6} L ${x + t * 9} ${y + 6} M ${x + t * 14} ${y - 6} L ${x + t * 14} ${y + 6}`;
  const s = g.dir1;
  const source = e.required ? `M ${g.x1 + s * 8} ${g.y1 - 5} L ${g.x1 + s * 8} ${g.y1 + 5}` : null;
  return { target, source, ring: e.required ? null : { cx: g.x1 + s * 9, cy: g.y1 } };
}

// ------------------------------------------------------------------ styles (inside the SVG)

const SVG_CSS = `
.dg-node { cursor: grab; transition: opacity .18s ease; }
.dg-node.dragging { cursor: grabbing; }
.dg-card { fill: var(--surface); stroke: var(--line-2); stroke-width: 1; }
.dg-node.selected .dg-card { stroke: var(--accent); stroke-width: 2; }
.dg-node.related .dg-card { stroke: color-mix(in srgb, var(--accent), var(--line-2) 55%); }
.dg-head { fill: var(--surface-2); }
.dg-stripe { fill: var(--c); }
.dg-glyph { fill: color-mix(in srgb, var(--c), transparent 86%); stroke: color-mix(in srgb, var(--c), transparent 72%); }
.dg-glyph-text { fill: var(--c); font: 650 12px Inter, ui-sans-serif, system-ui, sans-serif; }
.dg-title { fill: var(--ink); font: 600 14px Inter, ui-sans-serif, system-ui, sans-serif; letter-spacing: -0.01em; }
.dg-kind { fill: var(--muted); font: 500 11px Inter, ui-sans-serif, system-ui, sans-serif; }
.dg-divider { stroke: var(--line); }
.dg-row-bg { fill: transparent; }
.dg-row:hover .dg-row-bg, .dg-row.lit .dg-row-bg { fill: var(--hover); }
.dg-row.flash .dg-row-bg { fill: var(--accent-soft); }
.dg-name { fill: var(--ink-2); font: 13px Inter, ui-sans-serif, system-ui, sans-serif; }
.dg-name.system { fill: var(--muted); }
.dg-name.pk { fill: var(--ink); font-weight: 600; }
.dg-type { fill: var(--faint); font: 12px Inter, ui-sans-serif, system-ui, sans-serif; }
.dg-type.rel { fill: var(--tone-7); }
.dg-req { fill: var(--accent); }
.dg-more { fill: var(--faint); font: 11.5px Inter, ui-sans-serif, system-ui, sans-serif; }
.dg-edge { fill: none; vector-effect: non-scaling-stroke; stroke: color-mix(in srgb, var(--ec, var(--muted)), var(--line-2) 50%); stroke-width: 1.5; transition: opacity .18s ease, stroke .18s ease; }
.dg-edge.optional { stroke-dasharray: 5 4; }
.dg-edge.lit { stroke: var(--ec); stroke-width: 2.2; }
.dg-edge.lit.flow { stroke-dasharray: 7 6; animation: dg-flow 0.9s linear infinite; }
.dg-mark { fill: none; stroke: color-mix(in srgb, var(--ec, var(--muted)), var(--line-2) 50%); stroke-width: 1.5; stroke-linecap: round; transition: opacity .18s ease; }
.dg-mark.lit { stroke: var(--ec); stroke-width: 2; }
.dg-ring { fill: var(--canvas); stroke: color-mix(in srgb, var(--ec, var(--muted)), var(--line-2) 50%); stroke-width: 1.5; }
.dg-ring.lit { stroke: var(--ec); }
.dg-hit { fill: none; stroke: transparent; stroke-width: 14; cursor: pointer; }
.dg-cascade { fill: var(--surface); stroke: var(--danger); stroke-width: 1.3; }
.dg-cascade-x { stroke: var(--danger); stroke-width: 1.3; stroke-linecap: round; }
.dim { opacity: .18; }
@keyframes dg-flow { to { stroke-dashoffset: -13; } }
@keyframes dg-in { from { opacity: 0; transform: translateY(6px) scale(.98); } }
.dg-enter { animation: dg-in .45s cubic-bezier(.22,1,.36,1) both; }
`;

// the tokens the SVG uses, resolved, for exports
const TOKENS = ["--surface", "--surface-2", "--canvas", "--ink", "--ink-2", "--muted", "--faint", "--line", "--line-2", "--hover", "--accent", "--accent-soft", "--danger", "--tone-1", "--tone-2", "--tone-3", "--tone-4", "--tone-5", "--tone-6", "--tone-7", "--tone-8"];

// ------------------------------------------------------------------ pieces

function NodeView({ n, p, state, onPointerDown, onDblClick, flashField, enterDelay, counts, onRowEnter, onRowLeave, litRows }) {
  const c = n.c;
  const color = c.system ? "var(--muted)" : tone(c.name.replace(/^_/, ""));
  const kind = c.type === "auth" ? "auth" : c.type === "view" ? "view" : c.system ? "system" : "";
  const count = counts[c.id];
  const glyphIcon = c.type === "auth" ? "users" : c.type === "view" ? "view" : null;
  return html`<g
    class=${cx("dg-node", state, enterDelay != null && "dg-enter")}
    transform=${`translate(${p.x} ${p.y})`}
    style=${{ "--c": color, animationDelay: enterDelay != null ? enterDelay + "ms" : undefined }}
    onPointerDown=${(e) => onPointerDown(e, n)}
    onDblClick=${() => onDblClick(n)}
    data-id=${n.id}
  >
    <rect class="dg-card" width=${n.w} height=${n.h} rx="12" filter="url(#dg-shadow)" />
    <path class="dg-head" d=${`M 0 12 Q 0 0 12 0 L ${n.w - 12} 0 Q ${n.w} 0 ${n.w} 12 L ${n.w} ${HEADER} L 0 ${HEADER} Z`} />
    <rect class="dg-stripe" x="0" y="10" width="3" height=${HEADER - 20} rx="1.5" />
    <rect class="dg-glyph" x="14" y=${(HEADER - 26) / 2} width="26" height="26" rx="7" />
    ${glyphIcon
      ? html`<g transform=${`translate(20 ${(HEADER - 14) / 2})`} style="color:var(--c)"><${Icon} name=${glyphIcon} size=${14} /></g>`
      : html`<text class="dg-glyph-text" x="27" y=${HEADER / 2 + 4} text-anchor="middle">${c.name.replace(/^_/, "").slice(0, 1).toUpperCase()}</text>`}
    <text class="dg-title" x="50" y=${HEADER / 2 + (kind || count != null ? -1 : 5)}>${clip(c.name, n.w - 70, "600 14px Inter, ui-sans-serif, system-ui, sans-serif")}</text>
    ${(kind || count != null) &&
    html`<text class="dg-kind" x="50" y=${HEADER / 2 + 13}>${[kind, count != null ? plural(count, "record") : ""].filter(Boolean).join(" · ")}</text>`}
    ${n.rows.length > 0 && html`<line class="dg-divider" x1="0" x2=${n.w} y1=${HEADER} y2=${HEADER} />`}
    ${n.rows.map((r, i) => {
      const y = HEADER + i * ROW;
      const meta = r.type === "pk" ? { icon: "key", tone: 4 } : fieldMeta(r.type);
      const target = r.type === "relation" ? (app.get().collections.find((x) => x.id === r.field.collectionId) || {}).name || "?" : "";
      const right = r.type === "relation" ? (r.field.maxSelect > 1 ? "→ many " : "→ ") + target : r.type === "pk" ? "primary key" : meta.label.toLowerCase();
      const system = r.field && (r.field.system || r.field.hidden);
      return html`<g
        class=${cx("dg-row", { flash: flashField === r.name, lit: litRows && litRows.has(r.name) })}
        transform=${`translate(0 ${y})`}
        onPointerEnter=${() => onRowEnter(n, r)}
        onPointerLeave=${onRowLeave}
      >
        <rect class="dg-row-bg" x="4" y="1" width=${n.w - 8} height=${ROW - 2} rx="6" />
        <g transform=${`translate(14 ${(ROW - 14) / 2})`} style=${{ color: `var(--tone-${meta.tone || 1})` }}><${Icon} name=${meta.icon} size=${14} /></g>
        <text class=${cx("dg-name", { system, pk: r.type === "pk" })} x="36" y=${ROW / 2 + 4.5}>${clip(r.name, n.w * 0.52)}</text>
        ${r.field && r.field.required && html`<circle class="dg-req" cx=${38 + Math.min(textWidth(r.name), n.w * 0.52) + 6} cy=${ROW / 2} r="2.2" />`}
        <text class=${cx("dg-type", { rel: r.type === "relation" })} x=${n.w - 14} y=${ROW / 2 + 4} text-anchor="end">
          ${clip(right + (n.unique.has(r.name) && r.type !== "pk" ? " · unique" : ""), n.w * 0.42, "12px Inter, ui-sans-serif, system-ui, sans-serif")}
        </text>
      </g>`;
    })}
    ${n.hiddenFields > 0 && n.rows.length > 0 &&
    html`<text class="dg-more" x="36" y=${n.h - 11}>+${n.hiddenFields} more</text>`}
  </g>`;
}

function Minimap({ nodes, positions, view, size, onMove }) {
  const b = bounds(nodes, positions);
  const W = 200;
  const H = 128;
  const pad = 40;
  const scale = Math.min(W / (b.w + pad * 2), H / (b.h + pad * 2));
  const ox = (W - (b.w + pad * 2) * scale) / 2;
  const oy = (H - (b.h + pad * 2) * scale) / 2;
  const toMap = (x, y) => [ox + (x - b.x + pad) * scale, oy + (y - b.y + pad) * scale];
  // the visible part of the world
  const vx = -view.x / view.k;
  const vy = -view.y / view.k;
  const [x0, y0] = toMap(vx, vy);
  const [x1, y1] = toMap(vx + size.w / view.k, vy + size.h / view.k);
  const rx = Math.max(1, Math.min(W - 6, x0));
  const ry = Math.max(1, Math.min(H - 6, y0));
  const rw = Math.max(6, Math.min(W - 1, x1) - rx);
  const rh = Math.max(6, Math.min(H - 1, y1) - ry);
  const move = (e) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const wx = (mx - ox) / scale + b.x - pad;
    const wy = (my - oy) / scale + b.y - pad;
    onMove(wx, wy);
  };
  return html`<svg
    class="dg-minimap"
    width=${W}
    height=${H}
    onPointerDown=${(e) => {
      e.currentTarget.setPointerCapture(e.pointerId);
      move(e);
    }}
    onPointerMove=${(e) => e.buttons === 1 && move(e)}
  >
    ${nodes.map((n) => {
      const p = positions[n.id];
      if (!p) return null;
      const [x, y] = toMap(p.x, p.y);
      return html`<rect x=${x} y=${y} width=${Math.max(2, n.w * scale)} height=${Math.max(2, n.h * scale)} rx="2" style=${{ fill: n.c.system ? "var(--faint)" : tone(n.c.name.replace(/^_/, "")), opacity: 0.55 }} />`;
    })}
    ${!(x0 <= 0 && y0 <= 0 && x1 >= W && y1 >= H) && html`<rect class="dg-minimap-view" x=${rx} y=${ry} width=${rw} height=${rh} rx="3" />`}
  </svg>`;
}

function ruleLabel(rule) {
  if (rule === null || rule === undefined) return { label: "Superusers", cls: "" };
  if (rule === "") return { label: "Everyone", cls: "warn" };
  return { label: "Rule", cls: "accent" };
}

function Inspector({ n, graph, counts, onFocus, onClose }) {
  const c = n.c;
  const byId = new Map(graph.nodes.map((x) => [x.id, x]));
  const outgoing = c.fields.filter((f) => f.type === "relation");
  const incoming = graph.edges.filter((e) => e.to === c.id && !e.self);
  const rules = [
    ["listRule", "List"],
    ["viewRule", "View"],
    c.type !== "view" && ["createRule", "Create"],
    c.type !== "view" && ["updateRule", "Update"],
    c.type !== "view" && ["deleteRule", "Delete"],
  ].filter(Boolean);
  const collectionName = (id) => (app.get().collections.find((x) => x.id === id) || {}).name || "?";
  return html`<aside class="dg-inspector card" onPointerDown=${(e) => e.stopPropagation()}>
    <div class="row" style="gap:10px">
      <span class="coll-glyph" style=${{ "--c": c.system ? "var(--ink-3)" : tone(c.name.replace(/^_/, "")), width: "34px", height: "34px", fontSize: "14px" }}>
        ${c.type === "auth" ? html`<${Icon} name="users" size=${16} />` : c.type === "view" ? html`<${Icon} name="view" size=${16} />` : c.name.replace(/^_/, "").slice(0, 1)}
      </span>
      <div class="grow" style="min-width:0">
        <div class="ellipsis" style="font-weight:650">${c.name}</div>
        <div class="tiny muted">${c.type}${c.system ? " · system" : ""}${counts[c.id] != null ? " · " + plural(counts[c.id], "record") : ""}</div>
      </div>
      <button class="icon-btn" aria-label="Close" onClick=${onClose}><${Icon} name="close" size=${16} /></button>
    </div>
    <div class="dg-facts">
      <div><b>${c.fields.length}</b><span>fields</span></div>
      <div><b>${outgoing.length}</b><span>links out</span></div>
      <div><b>${incoming.length}</b><span>links in</span></div>
      <div><b>${(c.indexes || []).length}</b><span>indexes</span></div>
    </div>
    ${outgoing.length > 0 &&
    html`<div class="dg-section">
      <div class="section-title">References</div>
      ${outgoing.map(
        (f) => html`<button class="dg-link" onClick=${() => byId.has(f.collectionId) && onFocus(f.collectionId)} disabled=${!byId.has(f.collectionId)}>
          <span class="ellipsis"><code>${f.name}</code> → <b>${collectionName(f.collectionId)}</b></span>
          <span class="spacer"></span>
          <span class="tiny faint">${f.maxSelect > 1 ? "many" : "one"}${f.required ? " · required" : ""}${f.cascadeDelete ? " · cascade" : ""}</span>
        </button>`
      )}
    </div>`}
    ${incoming.length > 0 &&
    html`<div class="dg-section">
      <div class="section-title">Referenced by</div>
      ${incoming.map(
        (e) => html`<button class="dg-link" onClick=${() => onFocus(e.from)}>
          <span class="ellipsis"><b>${collectionName(e.from)}</b>.<code>${e.field.name}</code></span>
          <span class="spacer"></span>
          <span class="tiny faint">${e.many ? "many" : "one"}${e.cascade ? " · cascade" : ""}</span>
        </button>`
      )}
    </div>`}
    <div class="dg-section">
      <div class="section-title">API rules</div>
      <div class="dg-rules">
        ${rules.map(([key, label]) => {
          const r = ruleLabel(c[key]);
          return html`<div class="dg-rule tip" data-tip=${c[key] ? c[key] : r.label === "Everyone" ? "No rule: anyone" : "Only superusers"}>
            <span class="muted">${label}</span><span class=${"chip " + r.cls}>${r.label}</span>
          </div>`;
        })}
      </div>
    </div>
    <div class="row wrap" style="gap:6px;margin-top:4px">
      <${Button} size="sm" icon="table" onClick=${() => navigate(`/data/${enc(c.name)}`)}>Records</${Button}>
      ${!app.get().hideControls && html`<${Button} size="sm" icon="columns" onClick=${() => navigate(`/data/${enc(c.name)}/schema`)}>Schema</${Button}>`}
      ${!app.get().hideControls && html`<${Button} size="sm" icon="shield" onClick=${() => navigate(`/data/${enc(c.name)}/rules`)}>Rules</${Button}>`}
    </div>
  </aside>`;
}

// ------------------------------------------------------------------ the page

export function Diagram() {
  const collections = useStore(app, (s) => s.collections);
  const loaded = useStore(app, (s) => s.collectionsLoaded);
  const saved = useMemo(loadSaved, []);
  const [detail, setDetail] = useState(saved.detail || "all");
  const [showSystem, setShowSystem] = useState(!!saved.showSystem);
  const [positions, setPositions] = useState(saved.positions || {});
  const [view, setView] = useState({ x: 0, y: 0, k: 1 });
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [hover, setHover] = useState(null);
  const [selected, setSelected] = useState(null);
  const [hoverEdge, setHoverEdge] = useState(null);
  const [hoverRow, setHoverRow] = useState(null);
  const [flash, setFlash] = useState(null);
  const [query, setQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchIndex, setSearchIndex] = useState(0);
  const [counts, setCounts] = useState({});
  const [entered, setEntered] = useState(false);
  const [legend, setLegend] = useState(saved.legend !== false);
  const wrap = useRef();
  const svgRef = useRef();
  const searchRef = useRef();
  const drag = useRef(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  const anim = useRef(0);

  const graph = useMemo(() => buildGraph(collections, { showSystem, detail }), [collections, showSystem, detail]);
  const nodeMap = useMemo(() => new Map(graph.nodes.map((n) => [n.id, n])), [graph]);

  // positions for collections that have none yet: the auto layout's, kept clear of the rest
  const placed = useMemo(() => {
    const missing = graph.nodes.filter((n) => !positions[n.id]);
    if (!missing.length) return positions;
    const auto = autoLayout(graph.nodes, graph.edges);
    const known = graph.nodes.filter((n) => positions[n.id]);
    if (!known.length) return { ...positions, ...auto };
    const b = bounds(known, positions);
    const out = { ...positions };
    let y = b.y;
    for (const n of missing) {
      out[n.id] = { x: b.x + b.w + COL_GAP, y };
      y += n.h + NODE_GAP;
    }
    return out;
  }, [graph, positions]);

  // remember the arrangement and the options
  useEffect(() => {
    const t = setTimeout(() => save({ positions: placed, detail, showSystem, legend }), 250);
    return () => clearTimeout(t);
  }, [placed, detail, showSystem, legend]);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  // record counts in the headers
  useEffect(() => {
    let live = true;
    const list = collections.filter((c) => c.type !== "view" && (showSystem || !c.system)).slice(0, 60);
    Promise.all(
      list.map((c) =>
        api
          .get(`/api/collections/${enc(c.name)}/records` + qs({ perPage: 1, fields: "id" }))
          .then((r) => [c.id, r.totalItems])
          .catch(() => [c.id, null])
      )
    ).then((pairs) => live && setCounts(Object.fromEntries(pairs.filter(([, n]) => n != null))));
    return () => {
      live = false;
    };
  }, [collections.length, showSystem]);

  // ---------------------------------------------------------------- the camera

  const animateTo = (target, ms = 420) => {
    cancelAnimationFrame(anim.current);
    const from = viewRef.current;
    const start = performance.now();
    const step = (t) => {
      const p = Math.min(1, (t - start) / ms);
      const e = 1 - Math.pow(1 - p, 3);
      setView({ x: from.x + (target.x - from.x) * e, y: from.y + (target.y - from.y) * e, k: from.k + (target.k - from.k) * e });
      if (p < 1) anim.current = requestAnimationFrame(step);
    };
    anim.current = requestAnimationFrame(step);
  };

  const fitView = (nodes = graph.nodes, animate = true, maxK = 1.1) => {
    const b = bounds(nodes, placed);
    const pad = 70;
    const inspector = selected ? 320 : 0;
    const w = Math.max(200, size.w - inspector);
    const h = size.h - (legend ? 48 : 0);
    const k = Math.max(MIN_ZOOM, Math.min(maxK, (w - pad * 2) / b.w, (h - pad * 2) / b.h));
    const target = { k, x: (w - b.w * k) / 2 - b.x * k, y: (h - b.h * k) / 2 - b.y * k };
    animate ? animateTo(target) : setView(target);
  };

  const zoomAt = (factor, cx = size.w / 2, cy = size.h / 2, animate = false) => {
    const v = viewRef.current;
    const k = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, v.k * factor));
    const target = { k, x: cx - ((cx - v.x) / v.k) * k, y: cy - ((cy - v.y) / v.k) * k };
    animate ? animateTo(target, 220) : setView(target);
  };

  const focusOn = (id, field) => {
    const n = nodeMap.get(id);
    const p = placed[id];
    if (!n || !p) return;
    setSelected(id);
    const k = Math.max(viewRef.current.k, 0.85);
    const w = size.w - 320;
    animateTo({ k, x: w / 2 - (p.x + n.w / 2) * k, y: size.h / 2 - (p.y + Math.min(n.h, size.h / k - 80) / 2) * k });
    if (field) {
      setFlash({ id, field });
      setTimeout(() => setFlash(null), 1600);
    }
  };

  // fit once the first layout is known
  const fitted = useRef(false);
  useEffect(() => {
    if (!fitted.current && graph.nodes.length && size.w > 300) {
      fitted.current = true;
      fitView(graph.nodes, false);
      setTimeout(() => setEntered(true), 900);
    }
  }, [graph.nodes.length, size.w]);

  // wheel: scroll pans, Ctrl/⌘ (or a pinch) zooms at the cursor
  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const fn = (e) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) {
        zoomAt(Math.exp(-e.deltaY * 0.006), e.clientX - rect.left, e.clientY - rect.top);
      } else {
        const v = viewRef.current;
        setView({ ...v, x: v.x - e.deltaX, y: v.y - e.deltaY });
      }
    };
    el.addEventListener("wheel", fn, { passive: false });
    return () => el.removeEventListener("wheel", fn);
  }, [size]);

  // keys
  useEffect(() => {
    const fn = (e) => {
      if (e.target.closest && e.target.closest("input, textarea, select")) {
        if (e.key === "Escape") e.target.blur();
        return;
      }
      if (e.key === "f" || e.key === "F") fitView();
      else if (e.key === "+" || e.key === "=") zoomAt(1.2, undefined, undefined, true);
      else if (e.key === "-") zoomAt(1 / 1.2, undefined, undefined, true);
      else if (e.key === "0") zoomAt(1 / viewRef.current.k, undefined, undefined, true);
      else if (e.key === "/") {
        e.preventDefault();
        searchRef.current && searchRef.current.focus();
      } else if (e.key === "Escape") setSelected(null);
      else return;
    };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, [graph, placed, size, selected]);

  // ---------------------------------------------------------------- dragging and panning

  const toWorld = (clientX, clientY) => {
    const rect = wrap.current.getBoundingClientRect();
    const v = viewRef.current;
    return { x: (clientX - rect.left - v.x) / v.k, y: (clientY - rect.top - v.y) / v.k };
  };

  const onNodeDown = (e, n) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    cancelAnimationFrame(anim.current);
    const w = toWorld(e.clientX, e.clientY);
    const p = placed[n.id];
    drag.current = { kind: "node", id: n.id, dx: w.x - p.x, dy: w.y - p.y, moved: false, sx: e.clientX, sy: e.clientY };
    wrap.current.setPointerCapture(e.pointerId);
  };

  const onBackgroundDown = (e) => {
    if (e.button !== 0 && e.button !== 1) return;
    cancelAnimationFrame(anim.current);
    const v = viewRef.current;
    drag.current = { kind: "pan", sx: e.clientX, sy: e.clientY, vx: v.x, vy: v.y, moved: false };
    wrap.current.setPointerCapture(e.pointerId);
  };

  const onMove = (e) => {
    const d = drag.current;
    if (!d) return;
    if (Math.abs(e.clientX - d.sx) + Math.abs(e.clientY - d.sy) > 3) d.moved = true;
    if (d.kind === "pan") {
      setView({ ...viewRef.current, x: d.vx + e.clientX - d.sx, y: d.vy + e.clientY - d.sy });
    } else if (d.moved) {
      const w = toWorld(e.clientX, e.clientY);
      // snap to an 8 px grid
      const x = Math.round((w.x - d.dx) / 8) * 8;
      const y = Math.round((w.y - d.dy) / 8) * 8;
      setPositions((ps) => ({ ...placed, ...ps, [d.id]: { x, y } }));
    }
  };

  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (!d.moved) {
      if (d.kind === "node") setSelected(d.id === selected ? null : d.id);
      else setSelected(null);
    }
  };

  // ---------------------------------------------------------------- tidy up

  const tidy = () => {
    const target = autoLayout(graph.nodes, graph.edges);
    const from = placed;
    const start = performance.now();
    const step = (t) => {
      const p = Math.min(1, (t - start) / 520);
      const e = 1 - Math.pow(1 - p, 3);
      const next = {};
      for (const id of Object.keys(target)) {
        const a = from[id] || target[id];
        next[id] = { x: a.x + (target[id].x - a.x) * e, y: a.y + (target[id].y - a.y) * e };
      }
      setPositions(next);
      if (p < 1) requestAnimationFrame(step);
      else {
        setPositions(target);
        const b = bounds(graph.nodes, target);
        const pad = 70;
        const h = size.h - (legend ? 48 : 0);
        const k = Math.max(MIN_ZOOM, Math.min(1.1, (size.w - pad * 2) / b.w, (h - pad * 2) / b.h));
        animateTo({ k, x: (size.w - b.w * k) / 2 - b.x * k, y: (h - b.h * k) / 2 - b.y * k });
      }
    };
    requestAnimationFrame(step);
  };

  // ---------------------------------------------------------------- export

  const exportFile = async (format) => {
    const svg = svgRef.current;
    if (!svg) return;
    const b = bounds(graph.nodes, placed);
    const pad = 40;
    const clone = svg.cloneNode(true);
    const world = clone.querySelector(".dg-world");
    world.setAttribute("transform", `translate(${pad - b.x} ${pad - b.y})`);
    clone.querySelectorAll(".dim").forEach((el) => el.classList.remove("dim"));
    clone.querySelectorAll(".dg-enter").forEach((el) => el.classList.remove("dg-enter"));
    const W = Math.ceil(b.w + pad * 2);
    const H = Math.ceil(b.h + pad * 2);
    clone.setAttribute("width", W);
    clone.setAttribute("height", H);
    clone.setAttribute("viewBox", `0 0 ${W} ${H}`);
    clone.setAttribute("xmlns", "http://www.w3.org/2000/svg");
    const css = getComputedStyle(document.documentElement);
    clone.setAttribute("style", TOKENS.map((t) => `${t}:${css.getPropertyValue(t).trim()}`).join(";") + ";background:" + css.getPropertyValue("--canvas").trim());
    const bg = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    bg.setAttribute("width", W);
    bg.setAttribute("height", H);
    bg.setAttribute("fill", css.getPropertyValue("--canvas").trim());
    clone.insertBefore(bg, clone.firstChild.nextSibling);
    const text = new XMLSerializer().serializeToString(clone);
    const name = `${PROJECT || "slonbase"}-diagram`;
    const download = (blob, file) => {
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = file;
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 2000);
    };
    if (format === "svg") {
      download(new Blob([text], { type: "image/svg+xml" }), name + ".svg");
      return;
    }
    const img = new Image();
    img.onload = () => {
      const scale = 2;
      const canvas = document.createElement("canvas");
      canvas.width = W * scale;
      canvas.height = H * scale;
      const ctx = canvas.getContext("2d");
      ctx.scale(scale, scale);
      ctx.drawImage(img, 0, 0);
      canvas.toBlob((blob) => (blob ? download(blob, name + ".png") : toast.error("The image couldn't be made.")), "image/png");
    };
    img.onerror = () => toast.error("The image couldn't be made.");
    img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(text);
  };

  // ---------------------------------------------------------------- focus

  const focus = hover || selected;
  const related = useMemo(() => {
    if (!focus) return null;
    const set = new Set([focus]);
    for (const e of graph.edges) {
      if (e.from === focus) set.add(e.to);
      if (e.to === focus) set.add(e.from);
    }
    return set;
  }, [focus, graph]);
  const edgeLit = (e) => (hoverEdge ? hoverEdge === e.id : focus ? e.from === focus || e.to === focus : false) || (hoverRow && hoverRow.id === e.from && hoverRow.field === e.field.name);

  // search
  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const out = [];
    for (const n of graph.nodes) {
      if (n.c.name.toLowerCase().includes(q)) out.push({ id: n.id, label: n.c.name, sub: n.c.type, score: n.c.name.toLowerCase().startsWith(q) ? 3 : 2 });
      for (const f of n.c.fields) {
        if (f.name.toLowerCase().includes(q)) out.push({ id: n.id, field: f.name, label: `${n.c.name}.${f.name}`, sub: fieldMeta(f.type).label, score: f.name.toLowerCase().startsWith(q) ? 1.5 : 1 });
      }
    }
    return out.sort((a, b) => b.score - a.score || a.label.localeCompare(b.label)).slice(0, 8);
  }, [query, graph]);
  const pick = (r) => {
    if (!r) return;
    if (r.field && detail !== "all" && !nodeMap.get(r.id).rows.some((x) => x.name === r.field)) setDetail("all");
    focusOn(r.id, r.field);
    setQuery("");
    setSearchOpen(false);
    searchRef.current && searchRef.current.blur();
  };

  // ---------------------------------------------------------------- render

  const edgesSorted = [...graph.edges].sort((a, b) => Number(edgeLit(a)) - Number(edgeLit(b)));
  const hoveredEdge = hoverEdge && graph.edges.find((e) => e.id === hoverEdge);
  const hoveredGeom = hoveredEdge && edgeGeometry(hoveredEdge, nodeMap, placed);
  const selectedNode = selected && nodeMap.get(selected);
  const relations = graph.edges.length;

  if (loaded && !collections.some((c) => !c.system) && !showSystem) {
    return html`<div class="page enter"><div class="card"><div class="empty">
      <div class="empty-art"><${Icon} name="relation" size=${28} /></div>
      <h3>Nothing to draw yet</h3>
      <p class="small">The diagram shows your collections and how they relate. Create a collection, or show the system collections.</p>
      <div class="row" style="justify-content:center;gap:8px">
        <${Button} variant="primary" icon="plus" onClick=${() => navigate("/data?new=1")}>New collection</${Button}>
        <${Button} onClick=${() => setShowSystem(true)}>Show system collections</${Button}>
      </div>
    </div></div></div>`;
  }

  return html`<div class="dg-page">
    <div class="dg-toolbar">
      <div class="dg-title-block">
        <h1>Diagram</h1>
        <span class="small muted">${plural(graph.nodes.length, "collection")} · ${plural(relations, "relation")}</span>
      </div>
      <div class="dg-search" onPointerDown=${(e) => e.stopPropagation()}>
        <${Icon} name="search" size=${15} />
        <input
          ref=${searchRef}
          placeholder="Find a collection or field"
          value=${query}
          onInput=${(e) => {
            setQuery(e.target.value);
            setSearchIndex(0);
            setSearchOpen(true);
          }}
          onFocus=${() => setSearchOpen(true)}
          onBlur=${() => setTimeout(() => setSearchOpen(false), 150)}
          onKeyDown=${(e) => {
            if (e.key === "ArrowDown") setSearchIndex((i) => Math.min(results.length - 1, i + 1));
            else if (e.key === "ArrowUp") setSearchIndex((i) => Math.max(0, i - 1));
            else if (e.key === "Enter") pick(results[searchIndex]);
            else return;
            e.preventDefault();
          }}
        />
        <span class="kbd">/</span>
        ${searchOpen && results.length > 0 &&
        html`<div class="dg-results">
          ${results.map(
            (r, i) => html`<button class=${cx("dg-result", { on: i === searchIndex })} onMouseDown=${(e) => e.preventDefault()} onClick=${() => pick(r)}>
              <${Icon} name=${r.field ? "columns" : "data"} size=${14} />
              <span class="grow ellipsis">${r.label}</span>
              <span class="tiny faint">${r.sub}</span>
            </button>`
          )}
        </div>`}
      </div>
      <span class="spacer"></span>
      <${Segmented}
        value=${detail}
        onChange=${setDetail}
        options=${[
          { value: "all", label: "Fields" },
          { value: "keys", label: "Keys" },
          { value: "names", label: "Names" },
        ]}
      />
      <${Switch} checked=${showSystem} onChange=${setShowSystem} label="System" />
      <${Button} size="sm" icon="sparkle" onClick=${tidy}>Tidy up</${Button}>
      <${Menu}
        trigger=${(toggle) => html`<${Button} size="sm" icon="download" onClick=${toggle}>Export</${Button}>`}
        items=${[
          { icon: "image", label: "PNG image", onClick: () => exportFile("png") },
          { icon: "code", label: "SVG (vector)", onClick: () => exportFile("svg") },
        ]}
      />
    </div>

    <div
      class=${cx("dg-canvas", { panning: drag.current && drag.current.kind === "pan" })}
      ref=${wrap}
      onPointerDown=${onBackgroundDown}
      onPointerMove=${onMove}
      onPointerUp=${onUp}
      onPointerCancel=${onUp}
      style=${{ "--dot-size": Math.max(0.6, view.k) + "px", "--grid": 22 * view.k + "px", "--gx": view.x + "px", "--gy": view.y + "px" }}
    >
      <svg ref=${svgRef} class="dg-svg" width=${size.w} height=${size.h}>
        <style>${SVG_CSS}</style>
        <defs>
          <filter id="dg-shadow" x="-10%" y="-10%" width="120%" height="130%">
            <feDropShadow dx="0" dy="2" stdDeviation="3" flood-color="#0b1020" flood-opacity="0.08" />
          </filter>
        </defs>
        <g class="dg-world" transform=${`translate(${view.x} ${view.y}) scale(${view.k})`}>
          <g class="dg-edges">
            ${edgesSorted.map((e) => {
              const g = edgeGeometry(e, nodeMap, placed);
              if (!g) return null;
              const lit = edgeLit(e);
              const dim = (focus || hoverEdge) && !lit;
              const m = endMarkers(g, e);
              const color = tone(((nodeMap.get(e.from) || {}).c || { name: "" }).name.replace(/^_/, ""));
              return html`<g key=${e.id} style=${{ "--ec": color }} class=${cx({ dim })}>
                <path class=${cx("dg-edge", { optional: !e.required, lit, flow: lit && !hoverEdge })} d=${g.d} />
                <path class=${cx("dg-mark", { lit })} d=${m.target} />
                ${m.source && html`<path class=${cx("dg-mark", { lit })} d=${m.source} />`}
                ${m.ring && html`<circle class=${cx("dg-ring", { lit })} cx=${m.ring.cx} cy=${m.ring.cy} r="3.4" />`}
                ${e.cascade &&
                html`<g transform=${`translate(${g.mid.x} ${g.mid.y})`}>
                  <circle class="dg-cascade" r="6.5" /><path class="dg-cascade-x" d="M -2.6 -2.6 L 2.6 2.6 M 2.6 -2.6 L -2.6 2.6" />
                </g>`}
                <path
                  class="dg-hit"
                  d=${g.d}
                  onPointerEnter=${() => setHoverEdge(e.id)}
                  onPointerLeave=${() => setHoverEdge(null)}
                  onPointerDown=${(ev) => {
                    ev.stopPropagation();
                    focusOn(e.from, e.field.name);
                  }}
                />
              </g>`;
            })}
          </g>
          <g class="dg-nodes">
            ${graph.nodes.map((n, i) => {
              const p = placed[n.id];
              if (!p) return null;
              const state = cx({
                selected: selected === n.id,
                related: related && related.has(n.id) && selected !== n.id,
                dim: related && !related.has(n.id),
                dragging: drag.current && drag.current.id === n.id,
              });
              const litRows = hoverEdge && hoveredEdge && hoveredEdge.from === n.id ? new Set([hoveredEdge.field.name]) : null;
              return html`<g key=${n.id} onPointerEnter=${() => !drag.current && setHover(n.id)} onPointerLeave=${() => setHover(null)}>
                <${NodeView}
                  n=${n}
                  p=${p}
                  state=${state}
                  counts=${counts}
                  enterDelay=${entered ? null : Math.min(i * 35, 600)}
                  flashField=${flash && flash.id === n.id ? flash.field : null}
                  litRows=${litRows}
                  onPointerDown=${onNodeDown}
                  onDblClick=${(node) => navigate(`/data/${enc(node.c.name)}`)}
                  onRowEnter=${(node, r) => r.field && r.field.type === "relation" && setHoverRow({ id: node.id, field: r.name })}
                  onRowLeave=${() => setHoverRow(null)}
                />
              </g>`;
            })}
          </g>
        </g>
      </svg>

      ${hoveredEdge &&
      hoveredGeom &&
      html`<div
        class="dg-tooltip"
        style=${{ left: hoveredGeom.mid.x * view.k + view.x + "px", top: hoveredGeom.mid.y * view.k + view.y + "px" }}
      >
        <div><b>${nodeMap.get(hoveredEdge.from).c.name}</b>.<code>${hoveredEdge.field.name}</code> → <b>${nodeMap.get(hoveredEdge.to).c.name}</b></div>
        <div class="tiny muted">
          ${hoveredEdge.many ? `up to ${hoveredEdge.field.maxSelect} records` : "one record"} · ${hoveredEdge.required ? "required" : "optional"}${hoveredEdge.cascade ? ` · deleted with its ${nodeMap.get(hoveredEdge.to).c.name} record` : ""}
        </div>
      </div>`}

      ${legend
        ? html`<div class="dg-legend" onPointerDown=${(e) => e.stopPropagation()}>
            <svg width="44" height="16" class="dg-legend-svg"><style>${SVG_CSS}</style><path class="dg-edge" d="M 2 8 L 40 8" /><path class="dg-mark" d="M 31 2 L 31 14 M 36 2 L 36 14" /></svg><span>one</span>
            <svg width="44" height="16" class="dg-legend-svg"><style>${SVG_CSS}</style><path class="dg-edge" d="M 2 8 L 40 8" /><path class="dg-mark" d="M 28 8 L 40 2 M 28 8 L 40 8 M 28 8 L 40 14 M 25 2 L 25 14" /></svg><span>many</span>
            <svg width="44" height="16" class="dg-legend-svg"><style>${SVG_CSS}</style><path class="dg-edge optional" d="M 2 8 L 40 8" /><circle class="dg-ring" cx="9" cy="8" r="3.4" /></svg><span>optional</span>
            <svg width="44" height="16" class="dg-legend-svg"><style>${SVG_CSS}</style><path class="dg-edge" d="M 2 8 L 40 8" /><g transform="translate(21 8)"><circle class="dg-cascade" r="6.5" /><path class="dg-cascade-x" d="M -2.6 -2.6 L 2.6 2.6 M 2.6 -2.6 L -2.6 2.6" /></g></svg><span>cascade delete</span>
            <svg width="10" height="16" class="dg-legend-svg"><style>${SVG_CSS}</style><circle class="dg-req" cx="5" cy="8" r="2.4" /></svg><span>required</span>
            <span class="dg-legend-sep"></span>
            <span class="faint tip" data-tip=${`Drag to arrange · scroll to pan · ${navigator.platform.includes("Mac") ? "⌘" : "Ctrl"} + scroll to zoom · F to fit · / to search`}><${Icon} name="info" size=${14} /></span>
            <button class="icon-btn" aria-label="Hide the legend" onClick=${() => setLegend(false)}><${Icon} name="close" size=${13} /></button>
          </div>`
        : html`<button class="dg-legend-toggle btn sm" onPointerDown=${(e) => e.stopPropagation()} onClick=${() => setLegend(true)}><${Icon} name="info" size=${14} /> Legend</button>`}

      <div class="dg-corner" onPointerDown=${(e) => e.stopPropagation()}>
        <${Minimap} nodes=${graph.nodes} positions=${placed} view=${view} size=${size} onMove=${(wx, wy) => setView({ ...viewRef.current, x: size.w / 2 - wx * viewRef.current.k, y: size.h / 2 - wy * viewRef.current.k })} />
        <div class="dg-zoom">
          <button class="icon-btn" aria-label="Zoom out" onClick=${() => zoomAt(1 / 1.2, undefined, undefined, true)}><${Icon} name="minus" size=${15} /></button>
          <button class="dg-zoom-value" onClick=${() => zoomAt(1 / viewRef.current.k, undefined, undefined, true)} title="Reset to 100%">${Math.round(view.k * 100)}%</button>
          <button class="icon-btn" aria-label="Zoom in" onClick=${() => zoomAt(1.2, undefined, undefined, true)}><${Icon} name="plus" size=${15} /></button>
          <span class="dg-zoom-sep"></span>
          <button class="icon-btn tip" data-tip="Fit (F)" aria-label="Fit to view" onClick=${() => fitView()}><${Icon} name="expand" size=${15} /></button>
        </div>
      </div>

      ${selectedNode && html`<${Inspector} n=${selectedNode} graph=${graph} counts=${counts} onFocus=${(id) => focusOn(id)} onClose=${() => setSelected(null)} />`}
    </div>
  </div>`;
}
