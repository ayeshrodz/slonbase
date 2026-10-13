// Preact with htm, a tiny shared store and the hash router.
import { h, render, Fragment, createContext } from "../vendor/preact.js";
import { useState, useEffect, useRef, useMemo, useCallback, useLayoutEffect, useContext } from "../vendor/hooks.js";
import htm from "../vendor/htm.js";

export const html = htm.bind(h);
export { h, render, Fragment, createContext, useState, useEffect, useRef, useMemo, useCallback, useLayoutEffect, useContext };

// ------------------------------------------------------------------ store

export function createStore(initial) {
  let state = initial;
  const listeners = new Set();
  return {
    get: () => state,
    set(update) {
      state = typeof update === "function" ? update(state) : Object.assign({}, state, update);
      listeners.forEach((fn) => fn(state));
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

export function useStore(store, select = (s) => s) {
  const [value, setValue] = useState(() => select(store.get()));
  const selectRef = useRef(select);
  selectRef.current = select;
  useEffect(() => store.subscribe((s) => setValue(selectRef.current(s))), [store]);
  return value;
}

// ------------------------------------------------------------------ router

function currentRoute() {
  const raw = location.hash.replace(/^#/, "") || "/";
  const [path, query = ""] = raw.split("?");
  return { path, query: Object.fromEntries(new URLSearchParams(query)) };
}

export const route = createStore(currentRoute());
window.addEventListener("hashchange", () => route.set(currentRoute()));

export function navigate(path, replace = false) {
  const target = "#" + path;
  if (replace) history.replaceState(null, "", target);
  else history.pushState(null, "", target);
  route.set(currentRoute());
}

// matches "/collections/:name/:tab?" against a path
export function match(pattern, path) {
  const p = pattern.split("/").filter(Boolean);
  const s = path.split("/").filter(Boolean);
  const params = {};
  for (let i = 0; i < p.length; i++) {
    const optional = p[i].endsWith("?");
    const key = p[i].replace(/^:/, "").replace(/\?$/, "");
    if (p[i].startsWith(":")) {
      if (s[i] === undefined) {
        if (optional) continue;
        return null;
      }
      params[key] = decodeURIComponent(s[i]);
    } else if (p[i] !== s[i]) {
      return null;
    }
  }
  if (s.length > p.length) return null;
  return params;
}

// ------------------------------------------------------------------ helpers

export function cx(...parts) {
  return parts
    .flatMap((p) => (p && typeof p === "object" && !Array.isArray(p) ? Object.keys(p).filter((k) => p[k]) : [p]))
    .filter(Boolean)
    .join(" ");
}

export function useDebounced(value, ms = 250) {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return debounced;
}

// runs `load` when deps change and keeps { data, error, loading, reload }
export function useLoader(load, deps) {
  const [state, setState] = useState({ data: undefined, error: null, loading: true });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let live = true;
    setState((s) => ({ ...s, loading: true, error: null }));
    Promise.resolve()
      .then(load)
      .then(
        (data) => live && setState({ data, error: null, loading: false }),
        (error) => live && setState({ data: undefined, error, loading: false })
      );
    return () => {
      live = false;
    };
  }, [...deps, tick]);
  return { ...state, reload: () => setTick((t) => t + 1) };
}

export function useKey(handler, deps = []) {
  useEffect(() => {
    const fn = (e) => handler(e);
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, deps);
}

export function useClickOutside(ref, onOutside, active = true) {
  useEffect(() => {
    if (!active) return;
    const fn = (e) => {
      if (ref.current && !ref.current.contains(e.target)) onOutside(e);
    };
    setTimeout(() => document.addEventListener("mousedown", fn));
    return () => document.removeEventListener("mousedown", fn);
  }, [active]);
}

export const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;
