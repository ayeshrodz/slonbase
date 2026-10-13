// App-wide state: collections, settings and the theme.
import { createStore } from "./core.js";
import { api } from "./api.js";

export const app = createStore({
  collections: [],
  collectionsLoaded: false,
  health: null,
  theme: "system",
  // settings.meta.hideControls: schema editing is locked in the dashboard
  hideControls: false,
});

export async function loadCollections() {
  const res = await api.get("/api/collections?perPage=500&sort=name");
  app.set({ collections: res.items, collectionsLoaded: true });
  return res.items;
}

export function collectionByName(name) {
  return app.get().collections.find((c) => c.name === name || c.id === name);
}

export function upsertCollection(collection) {
  app.set((s) => {
    const others = s.collections.filter((c) => c.id !== collection.id);
    return { ...s, collections: [...others, collection].sort((a, b) => a.name.localeCompare(b.name)) };
  });
}

export function removeCollection(id) {
  app.set((s) => ({ ...s, collections: s.collections.filter((c) => c.id !== id) }));
}

// ------------------------------------------------------------------ theme

const THEME_KEY = "slonbase.theme";

function systemDark() {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function applyTheme(theme) {
  const dark = theme === "dark" || (theme === "system" && systemDark());
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

export function setTheme(theme) {
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch (_) {}
  app.set({ theme });
  applyTheme(theme);
}

export function initTheme() {
  let theme = "system";
  try {
    theme = localStorage.getItem(THEME_KEY) || "system";
  } catch (_) {}
  app.set({ theme });
  applyTheme(theme);
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => applyTheme(app.get().theme));
}
