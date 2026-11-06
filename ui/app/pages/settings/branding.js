// Settings → Branding: this installation's images (logos, icons, backgrounds), and which of
// them is its logo, its logo on dark backgrounds and its browser icon.
import { html, useState, useEffect, useRef, cx } from "../../lib/core.js";
import { api, BASE } from "../../lib/api.js";
import { Icon, Logo } from "../../lib/icons.js";
import { Button, IconButton, Field, Select, Skeleton, Empty, Input, toast, confirmDialog } from "../../lib/ui.js";
import { bytes, relativeTime } from "../../lib/format.js";
import { Card } from "./shared.js";

export const assetUrl = (id) => (id ? `${BASE}/api/branding/assets/${id}` : null);

// the images, loaded on a visit to the section and again when `key` changes
export function useAssets(key) {
  const [assets, setAssets] = useState(null);
  const load = () =>
    api
      .get("/api/branding/assets")
      .then(setAssets)
      .catch((err) => {
        toast.error(err);
        setAssets([]);
      });
  useEffect(() => {
    load();
  }, [key]);
  return { assets, load };
}

// a choice of image for a setting (or none)
export function AssetSelect({ assets, value, onChange, none = "None" }) {
  return html`<${Select}
    value=${value || ""}
    onChange=${onChange}
    options=${[{ value: "", label: none }, ...(assets || []).map((a) => ({ value: a.id, label: a.name }))]}
  />`;
}

function Upload({ onDone }) {
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  const input = useRef();
  const send = async (files) => {
    const list = [...(files || [])];
    if (!list.length) return;
    setBusy(true);
    try {
      for (const file of list) {
        const form = new FormData();
        form.append("file", file);
        await api.post("/api/branding/assets", form);
      }
      toast(list.length === 1 ? "Image uploaded." : `${list.length} images uploaded.`);
      onDone();
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
      if (input.current) input.current.value = "";
    }
  };
  return html`<label
    class=${cx("dropzone", { over, busy })}
    onDragOver=${(e) => {
      e.preventDefault();
      setOver(true);
    }}
    onDragLeave=${() => setOver(false)}
    onDrop=${(e) => {
      e.preventDefault();
      setOver(false);
      send(e.dataTransfer.files);
    }}
  >
    <${Icon} name="upload" size=${20} />
    <span><b>${busy ? "Uploading…" : "Drop images here"}</b> or click to choose</span>
    <span class="tiny muted">PNG, JPEG, WebP, GIF, ICO or SVG · up to 2 MB each</span>
    <input ref=${input} type="file" accept="image/png,image/jpeg,image/webp,image/gif,image/x-icon,image/svg+xml,.ico,.svg" multiple hidden onChange=${(e) => send(e.target.files)} />
  </label>`;
}

function AssetTile({ asset, onRenamed, onDelete }) {
  const [name, setName] = useState(asset.name);
  const [editing, setEditing] = useState(false);
  const rename = async () => {
    setEditing(false);
    if (name.trim() === asset.name || !name.trim()) return setName(asset.name);
    try {
      await api.patch(`/api/branding/assets/${asset.id}`, { name });
      onRenamed();
    } catch (err) {
      toast.error(err);
      setName(asset.name);
    }
  };
  const copy = () => {
    const url = new URL(assetUrl(asset.id), location.href).href;
    navigator.clipboard.writeText(url).then(() => toast("Image address copied."), toast.error);
  };
  return html`<div class="asset-tile">
    <div class="asset-thumb checker"><img src=${assetUrl(asset.id)} alt=${asset.name} loading="lazy" /></div>
    <div class="asset-meta">
      ${editing
        ? html`<${Input} value=${name} onInput=${setName} autofocus onBlur=${rename} onKeyDown=${(e) => e.key === "Enter" && e.target.blur()} />`
        : html`<button type="button" class="asset-name ellipsis" title="Rename" onClick=${() => setEditing(true)}>${asset.name}</button>`}
      <div class="tiny muted">${asset.mime.replace("image/", "").replace("svg+xml", "svg").replace("x-icon", "ico")} · ${bytes(asset.size)} · ${relativeTime(asset.created)}</div>
      ${asset.usedBy.length > 0 && html`<div class="row wrap tight">${asset.usedBy.map((u) => html`<span class="chip accent">${u}</span>`)}</div>`}
    </div>
    <div class="asset-actions">
      <${IconButton} icon="link" label="Copy address" onClick=${copy} />
      <${IconButton} icon="trash" label="Delete" disabled=${asset.usedBy.length > 0} onClick=${() => onDelete(asset)} />
    </div>
  </div>`;
}

export function Branding({ s }) {
  // where each image is used comes from the saved settings
  const { assets, load } = useAssets(s.saved);
  const d = s.draft;
  const remove = async (asset) => {
    if (!(await confirmDialog({ title: `Delete ${asset.name}?`, message: "Emails already sent that show it will show a broken image.", confirm: "Delete", danger: true }))) return;
    try {
      await api.delete(`/api/branding/assets/${asset.id}`);
      toast("Image deleted.");
      load();
    } catch (err) {
      toast.error(err);
    }
  };
  const roles = [
    { key: "meta.logo", label: "Logo", help: "The sidebar, the sign-in page and emails", value: d.meta.logo },
    { key: "meta.logoDark", label: "Logo on dark backgrounds", help: "Used in the dark theme; the logo when empty", value: d.meta.logoDark },
    { key: "meta.favicon", label: "Browser icon", help: "The tab's icon (a square image, SVG or PNG)", value: d.meta.favicon },
  ];
  return html`<div class="stack loose">
    <${Card} title="Brand" subtitle="Which images stand for this installation" icon="image">
      <div class="brand-roles">
        ${roles.map(
          (r) => html`<div class="brand-role">
            <div class=${cx("brand-role-preview", { dark: r.key === "meta.logoDark" })}>
              ${r.value ? html`<img src=${assetUrl(r.value)} alt="" />` : r.key === "meta.favicon" ? html`<img src="./favicon.svg" alt="" />` : html`<${Logo} size=${30} />`}
            </div>
            <${Field} label=${r.label} help=${r.help} error=${s.errors[r.key]}>
              <${AssetSelect} assets=${assets} value=${r.value} onChange=${(v) => s.set(r.key, v)} none="slonbase's" />
            </${Field}>
          </div>`
        )}
      </div>
      <div class="small muted row"><${Icon} name="info" size=${14} /> In emails, <code>{APP_LOGO_URL}</code> is the logo's address (it needs the application URL to be public).</div>
    </${Card}>
    <${Card} title="Images" subtitle=${assets ? `${assets.length} of 100 · anyone can view them, so the sign-in page and emails can show them` : "Loading…"} icon="layers">
      <${Upload} onDone=${load} />
      ${assets === null
        ? html`<${Skeleton} height=${120} />`
        : assets.length === 0
        ? html`<${Empty} icon="image" title="No images yet">Upload a logo to use it here, on the sign-in page and in emails.</${Empty}>`
        : html`<div class="asset-grid">${assets.map((a) => html`<${AssetTile} key=${a.id + a.name} asset=${a} onRenamed=${load} onDelete=${remove} />`)}</div>`}
    </${Card}>
  </div>`;
}
