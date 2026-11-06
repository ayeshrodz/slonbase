// The installation's branding: its name, accent and images, from /_/context.js (so it applies
// before the first paint and on the sign-in page), updated when the settings are saved.
import { createStore } from "./core.js";
import { PROJECT } from "./api.js";

const DEFAULT_LOGIN = {
  title: "",
  subtitle: "",
  logo: null,
  layout: "split",
  background: "none",
  backgroundImage: null,
  notice: "",
  footerLinks: [],
  showPoweredBy: true,
};

const initial = (window.SLONBASE_CONTEXT || {}).branding || {};

export const branding = createStore({
  appName: initial.appName || "slonbase",
  appURL: initial.appURL || "",
  accent: initial.accent || "sage",
  logo: initial.logo || null,
  logoDark: initial.logoDark || null,
  favicon: initial.favicon || null,
  login: { ...DEFAULT_LOGIN, ...(initial.login || {}) },
});

// the named accents (their colors are in app.css, a light and a dark tone each)
export const ACCENTS = [
  { id: "sage", label: "Sage", description: "Quiet green-grey, the default", light: "#4f7a6c", dark: "#6fc2a2" },
  { id: "slate", label: "Slate", description: "Cool blue-grey for neutral workspaces", light: "#4f6478", dark: "#8fb0cc" },
  { id: "mauve", label: "Mauve", description: "Muted violet with a warm cast", light: "#7a5f86", dark: "#c3a3d0" },
  { id: "clay", label: "Clay", description: "Earthy terracotta", light: "#a2553b", dark: "#e59a7d" },
  { id: "sand", label: "Sand", description: "Warm ochre, like dry stone", light: "#85693a", dark: "#d6b67a" },
  { id: "teal", label: "Teal", description: "Clear blue-green", light: "#1f7676", dark: "#5cc8c4" },
  { id: "ocean", label: "Ocean", description: "Deep sea blue", light: "#1f63a8", dark: "#6aa9ec" },
  { id: "indigo", label: "Indigo", description: "Inky blue-violet", light: "#4c4fb3", dark: "#9b9ff0" },
  { id: "rose", label: "Rose", description: "Dusty red with a soft edge", light: "#a8405e", dark: "#ec8fa8" },
  { id: "forest", label: "Forest", description: "Deep woodland green", light: "#2f6b3f", dark: "#74c38a" },
  { id: "amber", label: "Amber", description: "Burnished gold", light: "#8f5c0e", dark: "#e8ad4f" },
  { id: "plum", label: "Plum", description: "Rich purple-red", light: "#7d3b72", dark: "#d38cc6" },
  { id: "graphite", label: "Graphite", description: "Monochrome, no color at all", light: "#4b5563", dark: "#aab3bf" },
];

// colors earlier versions saved as the default (PocketBase's included): the default accent
const LEGACY_DEFAULTS = ["#1055c9", "#4a5a72", "#5062a6", "#4f7a6c"];

// a saved accent as a named accent's id, or "custom" for a color of its own
export function accentKind(value) {
  if (!value || LEGACY_DEFAULTS.includes(String(value).toLowerCase())) return "sage";
  if (ACCENTS.some((a) => a.id === value)) return value;
  return /^#[0-9a-f]{6}$/i.test(value) ? "custom" : "sage";
}

// applies an accent (a name or #rrggbb) to the whole dashboard
export function applyAccent(value) {
  const root = document.documentElement;
  const kind = accentKind(value);
  root.dataset.accent = kind;
  if (kind === "custom") root.style.setProperty("--accent", value);
  else root.style.removeProperty("--accent");
}

// the logo for the current theme (the dark one, when there is one, on dark backgrounds)
export function logoFor(b, dark) {
  return (dark && b.logoDark) || b.logo || null;
}

export function pageTitle(page) {
  const name = branding.get().appName || "slonbase";
  const scope = PROJECT && PROJECT !== name ? `${name} (${PROJECT})` : name;
  return page ? `${page} · ${scope}` : scope;
}

function setFavicon(url) {
  let link = document.querySelector('link[rel="icon"]');
  if (!link) {
    link = document.createElement("link");
    link.rel = "icon";
    document.head.appendChild(link);
  }
  const href = url || "./favicon.svg";
  if (link.getAttribute("href") !== href) {
    link.setAttribute("href", href);
    if (url) link.removeAttribute("type");
    else link.type = "image/svg+xml";
  }
}

// branding from saved settings (the API's shape), with `base` for the image URLs
export function brandingFromSettings(s, base) {
  const asset = (id) => (id ? `${base}/api/branding/assets/${id}` : null);
  const l = s.login || {};
  return {
    appName: s.meta.appName,
    appURL: s.meta.appURL,
    accent: s.meta.accentColor,
    logo: asset(s.meta.logo),
    logoDark: asset(s.meta.logoDark),
    favicon: asset(s.meta.favicon),
    login: {
      ...DEFAULT_LOGIN,
      ...l,
      logo: asset(l.logo),
      backgroundImage: l.background === "asset" ? asset(l.backgroundAsset) : null,
      backgroundAsset: undefined,
    },
  };
}

export function applyBranding(next) {
  if (next) branding.set(next);
  const b = branding.get();
  applyAccent(b.accent);
  setFavicon(b.favicon);
}
