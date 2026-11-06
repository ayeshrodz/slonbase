// Settings → Login screen: how this installation's sign-in page looks, with a live preview of
// the real page.
import { html, cx, useRef, useEffect } from "../../lib/core.js";
import { BASE } from "../../lib/api.js";
import { Icon } from "../../lib/icons.js";
import { Button, IconButton, Field, Input, TextArea, Switch, Segmented } from "../../lib/ui.js";
import { brandingFromSettings } from "../../lib/branding.js";
import { AuthFrame } from "../login.js";
import { Card } from "./shared.js";
import { useAssets, AssetSelect } from "./branding.js";

const LAYOUTS = [
  { value: "split", label: "Split" },
  { value: "centered", label: "Centered" },
  { value: "minimal", label: "Minimal" },
];
const BACKGROUNDS = [
  { value: "none", label: "Plain" },
  { value: "grid", label: "Grid" },
  { value: "asset", label: "Image" },
];

// the sign-in form, inert, as the preview shows it
function PreviewForm() {
  return html`<div class="stack" aria-hidden="true">
    <div class="field"><span class="label">Email</span><div class="input preview-input">you@example.com</div></div>
    <div class="field"><span class="label">Password</span><div class="input preview-input">••••••••••</div></div>
    <div class="btn primary lg block">Sign in <${Icon} name="arrowRight" size=${16} /></div>
  </div>`;
}

// draws the page at its real width, scaled to fit the preview
function usePreviewScale() {
  const ref = useRef();
  useEffect(() => {
    const el = ref.current;
    if (!el || !window.ResizeObserver) return;
    const ro = new ResizeObserver(([entry]) => el.style.setProperty("--preview-scale", String(entry.contentRect.width / 1280)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return ref;
}

export function LoginScreen({ s }) {
  const { assets } = useAssets();
  const previewRef = usePreviewScale();
  const l = s.draft.login;
  const set = (key, value) => s.set("login." + key, value);
  const links = l.footerLinks || [];
  const setLink = (i, patch) => set("footerLinks", links.map((x, j) => (j === i ? { ...x, ...patch } : x)));
  const preview = brandingFromSettings(s.draft, BASE);
  const err = (key) => s.errors["login." + key];
  return html`<div class="login-designer">
    <div class="stack loose">
      <${Card} title="Layout" subtitle="How the sign-in page is arranged" icon="columns">
        <${Field} label="Layout">
          <${Segmented} value=${l.layout} onChange=${(v) => set("layout", v)} options=${LAYOUTS} />
        </${Field}>
        <div class="small muted">${l.layout === "split" ? "The form beside a panel showing what slonbase is." : l.layout === "centered" ? "The form in a card, on a background of your choice." : "The form alone, nothing around it."}</div>
        ${l.layout !== "split" &&
        html`<div class="grid-2 enter">
          <${Field} label="Background">
            <${Segmented} value=${l.background} onChange=${(v) => set("background", v)} options=${BACKGROUNDS} />
          </${Field}>
          ${l.background === "asset" &&
          html`<${Field} label="Background image" error=${err("backgroundAsset")} help="From Branding's images">
            <${AssetSelect} assets=${assets} value=${l.backgroundAsset} onChange=${(v) => set("backgroundAsset", v)} none="Choose an image" />
          </${Field}>`}
        </div>`}
      </${Card}>
      <${Card} title="Words and logo" icon="text">
        <div class="grid-2">
          <${Field} label="Heading" error=${err("title")} help="Empty: “Welcome back”"><${Input} value=${l.title} onInput=${(v) => set("title", v)} maxlength="80" placeholder="Welcome back" /></${Field}>
          <${Field} label="Logo" help="Empty: the application's logo"><${AssetSelect} assets=${assets} value=${l.logo} onChange=${(v) => set("logo", v)} none="The application's logo" /></${Field}>
        </div>
        <${Field} label="Line under the heading" error=${err("subtitle")}><${Input} value=${l.subtitle} onInput=${(v) => set("subtitle", v)} maxlength="160" placeholder="Sign in with your superuser account." /></${Field}>
        <${Field} label="Notice" error=${err("notice")} help=${`Shown above the form as plain text, e.g. who to ask for access · ${l.notice.length}/500`}>
          <${TextArea} rows=${3} value=${l.notice} onInput=${(v) => set("notice", v)} maxlength="500" placeholder="Access is managed by the platform team (#platform on Slack)." />
        </${Field}>
      </${Card}>
      <${Card} title="Links" subtitle="Up to three, under the form (https: or mailto:)" icon="link">
        ${links.map(
          (x, i) => html`<div class="row top" key=${i}>
            <${Field} class="grow" label=${i === 0 ? "Label" : null} error=${err(`footerLinks.${i}.label`)}><${Input} value=${x.label} onInput=${(v) => setLink(i, { label: v })} maxlength="40" placeholder="Status page" /></${Field}>
            <${Field} class="grow" label=${i === 0 ? "Address" : null} error=${err(`footerLinks.${i}.url`)}><${Input} class="mono" value=${x.url} onInput=${(v) => setLink(i, { url: v })} placeholder="https://status.example.com" /></${Field}>
            <div class=${cx({ "label-pad": i === 0 })}><${IconButton} icon="trash" label="Remove the link" onClick=${() => set("footerLinks", links.filter((_, j) => j !== i))} /></div>
          </div>`
        )}
        <div class="row">
          <${Button} size="sm" icon="plus" disabled=${links.length >= 3} onClick=${() => set("footerLinks", [...links, { label: "", url: "https://" }])}>Add a link</${Button}>
          <span class="spacer"></span>
          <${Switch} checked=${l.showPoweredBy} onChange=${(v) => set("showPoweredBy", v)} label="“powered by slonbase” (with an application name of your own)" />
        </div>
      </${Card}>
    </div>
    <div class="login-preview-wrap">
      <div class="label-row"><span class="eyebrow">Live preview</span><a class="small" href=${BASE + "/_/"} target="_blank" rel="noopener">Open the page <${Icon} name="external" size=${13} /></a></div>
      <div class=${cx("login-preview", "layout-" + l.layout)} ref=${previewRef}>
        <div class="login-preview-scale">
          <${AuthFrame} primary preview=${preview} title="Welcome back" subtitle="Sign in with your superuser account.">
            <${PreviewForm} />
          </${AuthFrame}>
        </div>
      </div>
      <div class="tiny muted">The saved page changes when you save the settings.</div>
    </div>
  </div>`;
}
