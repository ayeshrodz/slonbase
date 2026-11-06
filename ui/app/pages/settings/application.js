// Settings → Application: the name and address, and the workspace's appearance (color mode,
// per browser; accent, for everyone using this installation) with a preview.
import { html, useStore, cx } from "../../lib/core.js";
import { app, setTheme } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { Field, Input, Switch, Button } from "../../lib/ui.js";
import { ACCENTS, accentKind, applyAccent } from "../../lib/branding.js";
import { Card } from "./shared.js";

const MODES = [
  { id: "system", label: "System", description: "Follow the device" },
  { id: "light", label: "Light", description: "Bright, neutral surfaces" },
  { id: "dark", label: "Dark", description: "Low glare, ink surfaces" },
];

// a small drawing of the dashboard in a color mode: sidebar, heading and lines
function ModeArt({ mode }) {
  const half = (m) => html`<div class=${cx("mode-art-half", m)} data-theme=${m}>
    <div class="mode-art-side"><i></i><i></i><i></i></div>
    <div class="mode-art-main"><b></b><i></i><i></i><span></span></div>
  </div>`;
  return html`<div class="mode-art" aria-hidden="true">${mode === "system" ? html`${half("light")}${half("dark")}` : half(mode)}</div>`;
}

function Appearance({ s }) {
  const theme = useStore(app, (st) => st.theme);
  const value = s.draft.meta.accentColor;
  const kind = accentKind(value);
  const savedValue = s.saved.meta.accentColor;
  const pick = (v) => {
    s.set("meta.accentColor", v);
    applyAccent(v);
  };
  const changed = value !== savedValue;
  const current = ACCENTS.find((a) => a.id === kind);
  return html`<${Card} title="Appearance" subtitle="How this workspace looks" icon="eye">
    <div class="stack">
      <div class="label-row"><span class="label">Color mode</span><span class="tiny muted">Saved in this browser</span></div>
      <div class="mode-cards" role="radiogroup" aria-label="Color mode">
        ${MODES.map(
          (m) => html`<button type="button" role="radio" aria-checked=${theme === m.id} class=${cx("mode-card", { on: theme === m.id })} onClick=${() => setTheme(m.id)}>
            <${ModeArt} mode=${m.id} />
            <span class="mode-card-label"><b>${m.label}</b><span class="tiny muted">${m.description}</span></span>
            ${theme === m.id && html`<span class="mode-card-check"><${Icon} name="check" size=${14} /></span>`}
          </button>`
        )}
      </div>
    </div>
    <div class="stack">
      <div class="label-row"><span class="label">Accent</span><span class="tiny muted">For everyone using this ${"installation"}: the dashboard, its sign-in page and emails</span></div>
      <div class="accent-grid" role="radiogroup" aria-label="Accent">
        ${ACCENTS.map(
          (a) => html`<button type="button" role="radio" aria-checked=${kind === a.id} class=${cx("accent-card", { on: kind === a.id })} data-accent=${a.id} onClick=${() => pick(a.id)}>
            <span class="accent-dot"></span>
            <span class="accent-text"><b>${a.label}</b><span class="tiny muted">${a.description}</span></span>
          </button>`
        )}
        <label class=${cx("accent-card", { on: kind === "custom" })}>
          <span class="accent-color" style=${kind === "custom" ? { background: value } : null}>
            <input type="color" value=${kind === "custom" ? value : "#4f7a6c"} onInput=${(e) => pick(e.target.value)} aria-label="A color of your own" />
          </span>
          <span class="accent-text"><b>Custom</b><span class="tiny muted mono">${kind === "custom" ? value : "Any color"}</span></span>
        </label>
      </div>
    </div>
    <div class="appearance-preview" data-accent=${kind === "custom" ? undefined : kind} style=${kind === "custom" ? { "--accent": value } : null}>
      <div class="appearance-preview-head">
        <span class="eyebrow">Little preview</span>
        <span class="tiny muted">${current ? current.label : "Custom"} · ${document.documentElement.dataset.theme === "dark" ? "dark" : "light"}</span>
      </div>
      <div class="appearance-preview-body">
        <div class="row wrap">
          <button type="button" class="btn primary sm" tabindex="-1">Save record</button>
          <button type="button" class="btn sm" tabindex="-1">Cancel</button>
          <span class="chip accent">realtime</span>
          <a href="#/settings/general" onClick=${(e) => e.preventDefault()} tabindex="-1">View the API docs</a>
        </div>
        <div class="appearance-preview-input" aria-hidden="true"><span class="mono">status = "active"</span><span class="caret"></span></div>
        <div class="appearance-preview-rows" aria-hidden="true">
          <div><span class="mono">r8x2k1</span><span>Ada Lovelace</span><span class="chip outline">admin</span></div>
          <div class="on"><span class="mono">p4m9q7</span><span>Grace Hopper</span><span class="chip outline">editor</span></div>
          <div><span class="mono">z1c5v3</span><span>Alan Turing</span><span class="chip outline">viewer</span></div>
        </div>
      </div>
    </div>
    <div class="row">
      <span class="small muted grow">${changed ? "Previewing: not saved yet." : "Saved."}</span>
      <${Button} size="sm" variant="ghost" icon="undo" disabled=${!changed} onClick=${() => pick(savedValue)}>Reset preview</${Button}>
      <${Button} size="sm" variant="primary" icon="check" disabled=${!changed} loading=${s.saving} onClick=${() => s.save({ meta: { accentColor: value } })}>Save appearance</${Button}>
    </div>
  </${Card}>`;
}

export function Application({ s }) {
  const d = s.draft;
  return html`<div class="stack loose">
    <${Card} title="Application" subtitle="Its name and public address, used across the dashboard, its sign-in page and emails" icon="sparkle">
      <div class="grid-2">
        <${Field} label="Application name" error=${s.errors["meta.appName"]} help="The browser tab, the sidebar, the sign-in page and emails">
          <${Input} value=${d.meta.appName} onInput=${(v) => s.set("meta.appName", v)} maxlength="60" />
        </${Field}>
        <${Field} label="Application URL" error=${s.errors["meta.appURL"]} help="Where clients reach the API: links in emails and OAuth2 redirects point here">
          <${Input} value=${d.meta.appURL} onInput=${(v) => s.set("meta.appURL", v)} placeholder="https://api.example.com" />
        </${Field}>
      </div>
      <${Switch} checked=${d.meta.hideControls} onChange=${(v) => s.set("meta.hideControls", v)} label="Hide schema controls (lock collection editing in the dashboard)" />
    </${Card}>
    <${Appearance} s=${s} />
  </div>`;
}
