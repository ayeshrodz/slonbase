// A collection's records: filter, sort, paginate, select, live updates.
import { html, useState, useEffect, useLayoutEffect, useRef, navigate, cx, route, useStore } from "../lib/core.js";
import { api, qs, enc, fileUrl, realtime, getFileToken, remembered, scopedKey } from "../lib/api.js";
import { Icon, fieldMeta } from "../lib/icons.js";
import { Button, IconButton, Check, Skeleton, Empty, Menu, Switch, toast, confirmDialog, CopyButton, Portal, tone } from "../lib/ui.js";
import { formatDate, relativeTime, truncate, stripHtml, isImage, plural, recordLabel } from "../lib/format.js";
import { RecordEditor } from "./record-editor.js";
import { app } from "../lib/state.js";

const PER_PAGE = [20, 50, 100, 200, 500];
// longer pages draw only the rows around the visible ones (plus OVERSCAN on each side)
const WINDOW_FROM = 80;
const OVERSCAN = 15;

// The rows of `count` to draw in the scrolling element `ref`, and the row height: all of them
// for short pages, else those in view and around, kept as the table scrolls or resizes.
function useRowWindow(ref, count) {
  const [range, setRange] = useState([0, count]);
  const height = useRef(43);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || count <= WINDOW_FROM) {
      setRange([0, count]);
      return;
    }
    const update = () => {
      const row = el.querySelector("tbody tr[data-row]");
      if (row) height.current = row.getBoundingClientRect().height || height.current;
      const head = (el.querySelector("thead") || {}).offsetHeight || 0;
      const top = Math.max(0, el.scrollTop - head);
      const start = Math.max(0, Math.floor(top / height.current) - OVERSCAN);
      const end = Math.min(count, Math.ceil((top + el.clientHeight) / height.current) + OVERSCAN);
      setRange((r) => (r[0] === start && r[1] === end ? r : [start, end]));
    };
    update();
    let frame = 0;
    const later = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(update);
    };
    el.addEventListener("scroll", later, { passive: true });
    window.addEventListener("resize", later);
    return () => {
      cancelAnimationFrame(frame);
      el.removeEventListener("scroll", later);
      window.removeEventListener("resize", later);
    };
  }, [count]);
  return count <= WINDOW_FROM ? { start: 0, end: count, rowHeight: height.current } : { start: range[0], end: Math.min(range[1], count), rowHeight: height.current };
}

function hiddenKey(c) {
  return scopedKey("columns." + c.id);
}

function loadHidden(c) {
  try {
    return JSON.parse(localStorage.getItem(hiddenKey(c)) || "null");
  } catch (_) {
    return null;
  }
}

export function columnsOf(c) {
  return c.fields.filter((f) => f.type !== "password" && !(f.system && f.name === "tokenKey"));
}

export function Cell({ field, record, collections, fileToken }) {
  const v = record[field.name];
  const empty = html`<span class="faint">—</span>`;
  switch (field.type) {
    case "bool":
      return html`<span class="cell-bool"><span class=${"dot " + (v ? "ok" : "")}></span>${v ? "True" : "False"}</span>`;
    case "number":
      return html`<span class="mono">${v ?? 0}</span>`;
    case "date":
    case "autodate":
      return v ? html`<span class="tip" data-tip=${v}>${formatDate(v)}</span>` : empty;
    case "select": {
      const list = Array.isArray(v) ? v : v ? [v] : [];
      return list.length ? html`<span class="chips" style="flex-wrap:nowrap">${list.map((x) => html`<span class="chip tone" style=${{ "--c": tone(x) }}>${x}</span>`)}</span>` : empty;
    }
    case "file": {
      const list = Array.isArray(v) ? v : v ? [v] : [];
      if (!list.length) return empty;
      return html`<span class="thumbs">
        ${list.slice(0, 4).map((name) =>
          isImage(name)
            ? html`<img class="thumb" loading="lazy" src=${fileUrl(record, name, { thumb: "100x100", token: field.protected ? fileToken : "" })} alt=${name} />`
            : html`<span class="thumb tip" data-tip=${name}><${Icon} name="file" size=${14} /></span>`
        )}
        ${list.length > 4 && html`<span class="chip">+${list.length - 4}</span>`}
      </span>`;
    }
    case "relation": {
      const list = Array.isArray(v) ? v : v ? [v] : [];
      if (!list.length) return empty;
      const expanded = record.expand && record.expand[field.name];
      const items = Array.isArray(expanded) ? expanded : expanded ? [expanded] : [];
      const target = collections.find((c) => c.id === field.collectionId);
      return html`<span class="chips" style="flex-wrap:nowrap">
        ${list.slice(0, 3).map((id) => {
          const r = items.find((x) => x.id === id);
          return html`<span class="chip outline"><${Icon} name="link" size=${11} />${r ? truncate(recordLabel(r, target), 24) : id}</span>`;
        })}
        ${list.length > 3 && html`<span class="chip">+${list.length - 3}</span>`}
      </span>`;
    }
    case "json":
      return v === null || v === undefined ? empty : html`<span class="cell-json">${truncate(JSON.stringify(v), 60)}</span>`;
    case "editor":
      return v ? truncate(stripHtml(v), 70) : empty;
    case "email":
      return v ? html`<span>${v}</span>` : empty;
    case "url":
      // only web links are links (a value written around the validation could be javascript:)
      if (v && !/^https?:\/\//i.test(v)) return truncate(v, 40);
      return v ? html`<a href=${v} target="_blank" rel="noopener noreferrer" onClick=${(e) => e.stopPropagation()}>${truncate(v, 40)}</a>` : empty;
    case "geoPoint":
      return v && (v.lat || v.lon) ? html`<span class="mono small"><${Icon} name="pin" size=${12} /> ${v.lat.toFixed(4)}, ${v.lon.toFixed(4)}</span>` : empty;
    default:
      if (field.name === "id") return html`<span class="cell-id">${v}<${CopyButton} text=${v} label="Copy id" /></span>`;
      return v === "" || v == null ? empty : truncate(String(v), 80);
  }
}

export function Records({ collection: c, recordId }) {
  const collections = useStore(app, (s) => s.collections);
  const { query: routeQuery } = useStore(route);
  const columns = columnsOf(c);
  const [hidden, setHidden] = useState(() => loadHidden(c) || columns.filter((f) => f.hidden || f.type === "editor" || f.type === "json").map((f) => f.name).filter((n) => n !== "id"));
  const [filterText, setFilterText] = useState(routeQuery.filter || "");
  const [filter, setFilter] = useState(routeQuery.filter || "");
  const [sort, setSort] = useState(c.fields.some((f) => f.name === "created") ? "-created" : "");
  const [page, setPage] = useState(1);
  const [perPage, setPerPage] = useState(50);
  const relations = c.fields.filter((f) => f.type === "relation").map((f) => f.name);
  const listUrl = `/api/collections/${enc(c.name)}/records` + qs({ page, perPage, sort, filter, expand: relations.join(",") });
  // the last answer to the same request, shown at once while the fresh one loads
  const [result, setResult] = useState(() => remembered.get(listUrl) || null);
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState(new Set());
  const [flash, setFlash] = useState(new Set());
  const [live, setLive] = useState(true);
  const [tick, setTick] = useState(0);
  const [fileToken, setFileToken] = useState("");
  const isView = c.type === "view";

  useEffect(() => {
    if (c.fields.some((f) => f.type === "file" && f.protected)) getFileToken().then(setFileToken).catch(() => {});
  }, [c.id]);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    const known = remembered.get(listUrl);
    if (known) setResult(known);
    const controller = new AbortController();
    api
      .get(listUrl, { signal: controller.signal })
      .then((res) => {
        if (!alive) return;
        remembered.set(listUrl, res);
        setResult(res);
        setError(null);
      })
      .catch((err) => {
        if (alive && err.name !== "AbortError") setError(err);
      })
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
      controller.abort();
    };
  }, [c.id, c.updated, page, perPage, sort, filter, tick]);

  useEffect(() => {
    setSelected(new Set());
  }, [c.id, filter, page]);

  // live updates
  const resultRef = useRef(result);
  resultRef.current = result;
  useEffect(() => {
    if (!live || isView) return;
    let timer;
    const stop = realtime([`${c.name}/*`], (_, event) => {
      const r = resultRef.current;
      if (!r) return;
      if (event.action === "update" && r.items.some((x) => x.id === event.record.id)) {
        setResult({ ...r, items: r.items.map((x) => (x.id === event.record.id ? { ...x, ...event.record, expand: x.expand } : x)) });
      } else {
        clearTimeout(timer);
        timer = setTimeout(() => setTick((t) => t + 1), 250);
      }
      setFlash((f) => new Set([...f, event.record.id]));
      setTimeout(() => setFlash((f) => new Set([...f].filter((id) => id !== event.record.id))), 1500);
    });
    return () => {
      clearTimeout(timer);
      stop();
    };
  }, [c.name, live]);

  const toggleHidden = (name) => {
    const next = hidden.includes(name) ? hidden.filter((n) => n !== name) : [...hidden, name];
    setHidden(next);
    try {
      localStorage.setItem(hiddenKey(c), JSON.stringify(next));
    } catch (_) {}
  };

  const sortBy = (name) => {
    setSort(sort === name ? "-" + name : sort === "-" + name ? "" : name);
    setPage(1);
  };

  const items = (result && result.items) || [];
  const shown = columns.filter((f) => !hidden.includes(f.name));
  const allSelected = items.length > 0 && items.every((r) => selected.has(r.id));

  const deleteSelected = async () => {
    const ids = [...selected];
    const ok = await confirmDialog({
      title: `Delete ${plural(ids.length, "record")}?`,
      message: "Records related through cascading relations are deleted too.",
      confirm: "Delete",
      danger: true,
    });
    if (!ok) return;
    // gone at once; back if the server refuses
    const before = result;
    const gone = new Set(ids);
    if (before) setResult({ ...before, items: before.items.filter((r) => !gone.has(r.id)), totalItems: Math.max(0, before.totalItems - ids.length) });
    setSelected(new Set());
    const results = await Promise.allSettled(ids.map((id) => api.delete(`/api/collections/${enc(c.name)}/records/${enc(id)}`)));
    const failed = results.filter((r) => r.status === "rejected");
    if (failed.length) {
      toast.error(`${failed.length} couldn't be deleted: ${failed[0].reason.message}`);
      if (before && failed.length === ids.length) setResult(before);
    } else toast(`Deleted ${plural(ids.length, "record")}.`);
    setTick((t) => t + 1);
  };

  const open = (id) => navigate(`/data/${enc(c.name)}/records/${enc(id)}`);
  const closeEditor = () => navigate(`/data/${enc(c.name)}/records`);

  const total = result ? result.totalItems : 0;
  const pages = result ? Math.max(1, result.totalPages) : 1;
  const wrap = useRef(null);
  const { start, end, rowHeight } = useRowWindow(wrap, items.length);
  const span = shown.length + (isView ? 1 : 2);

  return html`<div>
    <div class="toolbar">
      <form
        class="filter-box"
        onSubmit=${(e) => {
          e.preventDefault();
          setFilter(filterText.trim());
          setPage(1);
        }}
      >
        <${Icon} name="filter" size=${16} />
        <input
          placeholder=${`Filter, e.g. ${c.fields.some((f) => f.name === "created") ? "created > '2025-01-01' && " : ""}id != ''`}
          value=${filterText}
          onInput=${(e) => setFilterText(e.target.value)}
          aria-label="Filter"
        />
        ${filter &&
        html`<${IconButton} size="sm" icon="close" label="Clear filter" onClick=${() => {
          setFilterText("");
          setFilter("");
        }} />`}
        <${Button} size="xs" type="submit" variant=${filterText.trim() !== filter ? "primary" : "ghost"}>Apply</${Button}>
      </form>
      <${Menu}
        trigger=${(toggle) => html`<${Button} icon="columns" onClick=${toggle}>Columns</${Button}>`}
        items=${columns.map((f) => ({
          icon: hidden.includes(f.name) ? "eyeOff" : "eye",
          label: f.name,
          onClick: () => toggleHidden(f.name),
        }))}
      />
      ${!isView &&
      html`<span class="tip" data-tip=${live ? "Live updates on" : "Live updates off"}>
        <${Switch} checked=${live} onChange=${setLive} label=${html`<span class="row small" style="gap:6px"><span class=${"dot " + (live ? "live" : "")}></span>Live</span>`} />
      </span>`}
      <${IconButton} icon="refresh" label="Refresh" onClick=${() => setTick((t) => t + 1)} class=${loading ? "spinning" : ""} />
      ${!isView && html`<${Button} variant="primary" icon="plus" onClick=${() => open("new")}>New record</${Button}>`}
    </div>

    ${error
      ? html`<div class="card"><${Empty} icon="warning" title="Couldn't load the records">${error.message}</${Empty}></div>`
      : html`<div class="grid-wrap" ref=${wrap}>
          <table class=${cx("grid", { windowed: items.length > WINDOW_FROM })}>
            <thead>
              <tr>
                ${!isView &&
                html`<th class="col-check">
                  <${Check}
                    checked=${allSelected}
                    partial=${selected.size > 0}
                    label="Select all"
                    onChange=${(on) => setSelected(on ? new Set(items.map((r) => r.id)) : new Set())}
                  />
                </th>`}
                ${shown.map((f) => {
                  const dir = sort === f.name ? "asc" : sort === "-" + f.name ? "desc" : "";
                  return html`<th class="sortable" onClick=${() => sortBy(f.name)} aria-sort=${dir ? (dir === "asc" ? "ascending" : "descending") : undefined}>
                    <span class="th">
                      <${Icon} name=${fieldMeta(f.type).icon} size=${14} />
                      ${f.name}
                      ${dir && html`<${Icon} name=${dir === "asc" ? "chevronUp" : "chevronDown"} size=${13} class="sort-ind" />`}
                    </span>
                  </th>`;
                })}
                <th class="col-actions"></th>
              </tr>
            </thead>
            <tbody>
              ${!result && loading
                ? [0, 1, 2, 3, 4, 5].map(
                    () => html`<tr>
                      ${!isView && html`<td class="col-check"></td>`}
                      ${shown.map(() => html`<td><${Skeleton} width=${40 + Math.random() * 50 + "%"} /></td>`)}<td></td>
                    </tr>`
                  )
                : [
                  start > 0 && html`<tr class="spacer" aria-hidden="true" key="above"><td colspan=${span} style=${{ height: start * rowHeight + "px" }}></td></tr>`,
                  ...items.slice(start, end).map(
                    (r, i) => html`<tr
                      key=${r.id}
                      data-row
                      style=${{ "--i": start ? 0 : i }}
                      class=${cx({ selected: selected.has(r.id), flash: flash.has(r.id) })}
                      onClick=${() => open(r.id)}
                    >
                      ${!isView &&
                      html`<td class="col-check" onClick=${(e) => e.stopPropagation()}>
                        <${Check}
                          checked=${selected.has(r.id)}
                          label="Select"
                          onChange=${(on) => {
                            const next = new Set(selected);
                            on ? next.add(r.id) : next.delete(r.id);
                            setSelected(next);
                          }}
                        />
                      </td>`}
                      ${shown.map((f) => html`<td><${Cell} field=${f} record=${r} collections=${collections} fileToken=${fileToken} /></td>`)}
                      <td class="col-actions"><${Icon} name="chevronRight" size=${16} class="faint" /></td>
                    </tr>`
                  ),
                  end < items.length && html`<tr class="spacer" aria-hidden="true" key="below"><td colspan=${span} style=${{ height: (items.length - end) * rowHeight + "px" }}></td></tr>`,
                ]}
            </tbody>
          </table>
          ${result &&
          items.length === 0 &&
          html`<${Empty}
            icon=${filter ? "search" : "table"}
            title=${filter ? "No records match the filter" : "No records yet"}
            action=${!filter && !isView && html`<${Button} variant="primary" icon="plus" onClick=${() => open("new")}>New record</${Button}>`}
          >${filter ? html`<code>${filter}</code>` : "Records you create here, or through the API, show up instantly."}</${Empty}>`}
        </div>`}

    ${result &&
    total > 0 &&
    html`<div class="pager">
      <span>${plural(total, "record")}</span>
      <span class="spacer"></span>
      <span>Per page</span>
      <select class="input" aria-label="Records per page" style="width:auto;min-height:30px;padding:4px 30px 4px 10px" value=${perPage} onChange=${(e) => {
        setPerPage(Number(e.target.value));
        setPage(1);
      }}>
        ${PER_PAGE.map((n) => html`<option value=${n} selected=${n === perPage}>${n}</option>`)}
      </select>
      <div class="pages">
        <button onClick=${() => setPage(Math.max(1, page - 1))} disabled=${page <= 1} aria-label="Previous page"><${Icon} name="chevronLeft" size=${15} /></button>
        ${pageList(page, pages).map((p) =>
          p === "…" ? html`<button disabled>…</button>` : html`<button class=${cx({ on: p === page })} onClick=${() => setPage(p)}>${p}</button>`
        )}
        <button onClick=${() => setPage(Math.min(pages, page + 1))} disabled=${page >= pages} aria-label="Next page"><${Icon} name="chevronRight" size=${15} /></button>
      </div>
    </div>`}

    ${selected.size > 0 &&
    html`<${Portal}><div class="bulkbar">
      <b>${plural(selected.size, "record")}</b> selected
      <${Button} size="sm" variant="ghost" onClick=${() => setSelected(new Set())}>Clear</${Button}>
      <${Button} size="sm" variant="danger" icon="trash" onClick=${deleteSelected}>Delete</${Button}>
    </div></${Portal}>`}

    ${recordId &&
    html`<${RecordEditor}
      key=${recordId}
      collection=${c}
      recordId=${recordId}
      onClose=${closeEditor}
      onSaved=${(saved, keepOpen) => {
        // the saved values at once (the refresh brings the expanded relations)
        if (saved && result && result.items.some((x) => x.id === saved.id)) {
          setResult({ ...result, items: result.items.map((x) => (x.id === saved.id ? { ...saved, expand: x.expand } : x)) });
        }
        setTick((t) => t + 1);
        if (!keepOpen) closeEditor();
        else if (recordId === "new") navigate(`/data/${enc(c.name)}/records/${enc(saved.id)}`, true);
      }}
      onDeleted=${() => {
        setTick((t) => t + 1);
        closeEditor();
      }}
    />`}
  </div>`;
}

function pageList(page, pages) {
  const out = [];
  for (let p = 1; p <= pages; p++) {
    if (p === 1 || p === pages || Math.abs(p - page) <= 1) out.push(p);
    else if (out[out.length - 1] !== "…") out.push("…");
  }
  return out;
}
