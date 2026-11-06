import { html, useStore } from "./core.js";
import { app } from "./state.js";
import { Logo } from "./icons.js";
import { branding, logoFor } from "./branding.js";

// The installation's logo, or its name beside slonbase's mark: slonbase's own (its lowercase
// wordmark) until the settings give it others. A logo stands alone (it usually has the name).
export function BrandMark({ size = 26, label = true }) {
  const b = useStore(branding);
  const dark = useStore(app, (s) => s.theme) && document.documentElement.dataset.theme === "dark";
  const logo = logoFor(b, dark);
  const name = b.appName && b.appName !== "slonbase" ? b.appName : null;
  return html`${logo ? html`<img class="brand-logo" src=${logo} alt="" style=${{ height: size + "px" }} />` : html`<${Logo} size=${size} />`}${label && !logo && html`<span class="brand-name side-label">${name ? html`<span class="brand-custom">${name}</span>` : html`slon<b>base</b>`}</span>`}`;
}

