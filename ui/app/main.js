// The dashboard: auth gate, shell (sidebar navigation, command palette) and routes.
import { html, render, useState, useEffect, useRef, useLayoutEffect, useStore, route, navigate, match, cx, lazy } from "./lib/core.js";
import { session, refreshAuth, api, PROJECT, BASE, listProjects, dashboardUrl } from "./lib/api.js";
import { app, initTheme, setTheme, loadCollections } from "./lib/state.js";
import { Icon } from "./lib/icons.js";
import { Toasts, Menu, CollectionGlyph, useAutofocus } from "./lib/ui.js";
import { initials } from "./lib/format.js";
import { Login, PasswordResetRequest, PasswordResetConfirm } from "./pages/login.js";
import { PublicAuth } from "./pages/public-auth.js";
import { Overview } from "./pages/overview.js";
import { DataWorkspace, CollectionTree } from "./pages/data.js";
import { Settings } from "./pages/settings.js";
import { branding, applyBranding, pageTitle } from "./lib/branding.js";
import { BrandMark } from "./lib/brand-mark.js";
// opened by few visits: loaded when first shown (see `lazy`)
const Logs = lazy(() => import("./pages/logs.js"), "Logs");
const SqlConsole = lazy(() => import("./pages/sql.js"), "SqlConsole");
const Projects = lazy(() => import("./pages/projects.js"), "Projects");
const Health = lazy(() => import("./pages/health.js"), "Health");
const Diagram = lazy(() => import("./pages/diagram.js"), "Diagram");

initTheme();
applyBranding();
document.title = pageTitle();

const NAV = [
  { path: "/", match: (p) => p === "/", icon: "overview", label: "Overview" },
  { path: "/data", match: (p) => p.startsWith("/data"), icon: "data", label: "Data" },
  { path: "/diagram", match: (p) => p.startsWith("/diagram"), icon: "relation", label: "Diagram" },
  { path: "/logs", match: (p) => p.startsWith("/logs"), icon: "logs", label: "Logs" },
  { path: "/sql", match: (p) => p.startsWith("/sql"), icon: "terminal", label: "SQL" },
  { path: "/health", match: (p) => p.startsWith("/health"), icon: "pulse", label: "Health" },
  // projects are managed from the main installation
  !PROJECT && { path: "/projects", match: (p) => p.startsWith("/projects"), icon: "layers", label: "Projects" },
  { path: "/settings", match: (p) => p.startsWith("/settings"), icon: "settings", label: "Settings" },
].filter(Boolean);

// the sidebar folded to its icons (desktop), kept per browser
const COLLAPSED_KEY = "slonbase.sidebar.collapsed";
function storedCollapsed() {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === "1";
  } catch (_) {
    return false;
  }
}

// Shows which installation this dashboard manages, and switches between the main one and its
// projects (when this browser is signed in to the main installation).
function ProjectSwitcher() {
  const projects = useStore(app, (s) => s.projects);
  useEffect(() => {
    listProjects().then((list) => app.set({ projects: list }));
  }, []);
  if (!PROJECT && !(projects && projects.length)) return null;
  const go = (name) => (location.href = dashboardUrl(name));
  const items = [
    { icon: PROJECT ? "server" : "check", label: "Main installation", onClick: () => go(null) },
    ...(projects || []).map((p) => ({ icon: p.name === PROJECT ? "check" : "layers", label: p.name, onClick: () => go(p.name) })),
    "-",
    PROJECT
      ? { icon: "layers", label: "Manage projects", onClick: () => (location.href = dashboardUrl(null) + "#/projects") }
      : { icon: "layers", label: "Manage projects", onClick: () => navigate("/projects") },
  ];
  return html`<${Menu}
    align="left"
    trigger=${(toggle, open) => html`<button class=${cx("project-switch", { on: open })} onClick=${toggle} aria-label="Switch project">
      <span class=${cx("project-dot", { main: !PROJECT })}></span>
      <span class="project-switch-name">${PROJECT || "Main"}</span>
      <${Icon} name="chevronDown" size=${14} />
    </button>`}
    items=${items}
  />`;
}

function SideNav({ path, onNavigate }) {
  return html`<nav class="side-nav" aria-label="Main">
    ${NAV.map((n) => {
      const on = n.match(path);
      return html`<a
        href=${"#" + n.path}
        class=${cx("side-link", { on })}
        aria-current=${on ? "page" : undefined}
        data-tip=${n.label}
        onClick=${onNavigate}
      >
        <${Icon} name=${n.icon} size=${18} /><span class="side-label">${n.label}</span>
      </a>`;
    })}
  </nav>`;
}

function ThemeSwitch() {
  const theme = useStore(app, (s) => s.theme);
  const options = [
    { value: "light", icon: "sun", label: "Light" },
    { value: "dark", icon: "moon", label: "Dark" },
    { value: "system", icon: "monitor", label: "System" },
  ];
  return html`<div class="theme-switch" role="radiogroup" aria-label="Theme">
    ${options.map(
      (o) => html`<button
        role="radio"
        aria-checked=${theme === o.value}
        aria-label=${o.label + " theme"}
        class=${cx({ on: theme === o.value })}
        data-tip=${o.label}
        onClick=${() => setTheme(o.value)}
      ><${Icon} name=${o.icon} size=${15} /></button>`
    )}
  </div>`;
}

function Sidebar({ path, collapsed, onCollapse, open, onClose, onSearch }) {
  const record = session.record || {};
  const active = (match("/data/:collection?/:tab?/:record?", path) || {}).collection;
  const mac = navigator.platform.includes("Mac");
  return html`<aside class=${cx("sidebar", { open })} aria-label="Navigation">
    <div class="side-top">
      <a class="brand" href="#/" onClick=${onClose}><${BrandMark} size=${26} /></a>
      <button class="icon-btn side-fold" aria-label=${collapsed ? "Expand the sidebar" : "Collapse the sidebar"} data-tip=${collapsed ? "Expand" : "Collapse"} onClick=${onCollapse}>
        <${Icon} name="sidebar" size=${17} />
      </button>
    </div>
    <div class="side-project"><${ProjectSwitcher} /></div>
    <button class="side-search" onClick=${onSearch} aria-label="Search" data-tip="Search">
      <${Icon} name="search" size=${16} /><span class="side-label grow">Search</span><span class="kbd side-label">${mac ? "⌘" : "Ctrl"} K</span>
    </button>
    <${SideNav} path=${path} onNavigate=${onClose} />
    <div class="side-collections">
      <${CollectionTree} active=${active} onNavigate=${onClose} />
    </div>
    <div class="side-foot">
      <${ThemeSwitch} />
      <${Menu}
        align="left"
        up=${true}
        trigger=${(toggle) => html`<button class="side-user" onClick=${toggle} aria-label="Account">
          <span class="avatar">${initials(record.email)}</span>
          <span class="side-label grow ellipsis">${record.email || "Superuser"}</span>
          <${Icon} name="chevronUp" size=${14} class="side-label" />
        </button>`}
        items=${[
          { icon: "user", label: "Superusers", onClick: () => navigate("/data/_superusers") },
          { icon: "settings", label: "Settings", onClick: () => navigate("/settings") },
          "-",
          { icon: "logout", label: "Sign out", danger: true, onClick: () => session.clear() },
        ]}
      />
    </div>
  </aside>`;
}

// ------------------------------------------------------------------ command palette

function Palette({ onClose }) {
  const collections = useStore(app, (s) => s.collections);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const listRef = useRef();
  const rootRef = useRef();
  useAutofocus(rootRef);
  useEffect(() => {
    const fn = (e) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, []);
  const theme = useStore(app, (s) => s.theme);
  const projects = useStore(app, (s) => s.projects) || [];
  const items = [
    ...NAV.map((n) => ({ icon: n.icon, label: n.label, hint: "Go to", run: () => navigate(n.path) })),
    ...collections.map((c) => ({
      glyph: c,
      label: c.name,
      hint: c.type === "auth" ? "Auth collection" : c.type === "view" ? "View collection" : "Collection",
      run: () => navigate(`/data/${encodeURIComponent(c.name)}`),
    })),
    ...collections.map((c) => ({
      icon: "columns",
      label: `${c.name} — schema`,
      hint: "Edit fields",
      run: () => navigate(`/data/${encodeURIComponent(c.name)}/schema`),
    })),
    { icon: "plus", label: "New collection", hint: "Create", run: () => navigate("/data?new=1") },
    ...projects
      .filter((p) => p.name !== PROJECT)
      .map((p) => ({ icon: "layers", label: p.name, hint: "Open project", run: () => (location.href = dashboardUrl(p.name)) })),
    PROJECT && { icon: "server", label: "Main installation", hint: "Open dashboard", run: () => (location.href = dashboardUrl(null)) },
    { icon: "shield", label: "Health", hint: "Status and checks", run: () => navigate("/health") },
    { icon: "archive", label: "Backups", hint: "Settings", run: () => navigate("/settings/backups") },
    { icon: "mail", label: "Mail settings", hint: "Settings", run: () => navigate("/settings/mail") },
    { icon: "cloud", label: "File storage", hint: "Settings", run: () => navigate("/settings/storage") },
    { icon: "zap", label: "Rate limits", hint: "Settings", run: () => navigate("/settings/security") },
    { icon: theme === "dark" ? "sun" : "moon", label: "Toggle theme", hint: "Appearance", run: () => setTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark") },
    { icon: "logout", label: "Sign out", hint: "Account", run: () => session.clear() },
  ].filter(Boolean);
  const q = query.trim().toLowerCase();
  const filtered = q
    ? items
        .map((it) => ({ it, score: score(it.label.toLowerCase(), q) }))
        .filter((x) => x.score > 0)
        .sort((a, b) => b.score - a.score)
        .map((x) => x.it)
    : items.slice(0, 12);
  useEffect(() => setIndex(0), [query]);
  useEffect(() => {
    const el = listRef.current && listRef.current.querySelector(".palette-item.on");
    if (el) el.scrollIntoView({ block: "nearest" });
  }, [index]);
  const run = (item) => {
    onClose();
    item.run();
  };
  return html`<div class="backdrop" ref=${rootRef} onMouseDown=${(e) => e.target === e.currentTarget && onClose()}>
    <div class="palette" role="dialog" aria-label="Command palette">
      <div class="palette-input">
        <${Icon} name="search" size=${19} />
        <input
          autofocus
          placeholder="Search collections, pages and actions…"
          value=${query}
          onInput=${(e) => setQuery(e.target.value)}
          onKeyDown=${(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setIndex((i) => Math.min(filtered.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter" && filtered[index]) {
              run(filtered[index]);
            }
          }}
        />
        <span class="kbd">esc</span>
      </div>
      <div class="palette-list" ref=${listRef}>
        ${filtered.length === 0 && html`<div class="empty small">Nothing matches “${query}”.</div>`}
        ${filtered.map(
          (it, i) => html`<div
            class=${cx("palette-item", { on: i === index })}
            style=${{ "--i": i }}
            onMouseMove=${() => setIndex(i)}
            onClick=${() => run(it)}
          >
            ${it.glyph ? html`<${CollectionGlyph} collection=${it.glyph} size=${24} />` : html`<${Icon} name=${it.icon} />`}
            <span>${it.label}</span>
            <span class="hint">${it.hint}</span>
          </div>`
        )}
      </div>
    </div>
  </div>`;
}

// subsequence match, rewarding prefixes and consecutive letters
function score(text, q) {
  if (text.includes(q)) return 100 - text.indexOf(q);
  let ti = 0;
  let s = 0;
  let streak = 0;
  for (const ch of q) {
    const found = text.indexOf(ch, ti);
    if (found < 0) return 0;
    streak = found === ti ? streak + 1 : 0;
    s += 1 + streak;
    ti = found + 1;
  }
  return s;
}

// ------------------------------------------------------------------ shell

// The status bar along the bottom, as in an editor: whether the server answers, the database
// and its round trip, the API base to point clients at, and the version. Refreshed every 30 s.
function StatusBar() {
  const [status, setStatus] = useState(null);
  const [down, setDown] = useState(false);
  useEffect(() => {
    let live = true;
    const load = () =>
      api
        .get("/api/system/status")
        .then((st) => live && (setStatus(st), setDown(false)))
        .catch(() => live && setDown(true));
    load();
    const timer = setInterval(load, 30_000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, []);
  const apiBase = location.host + BASE + "/api";
  const db = status && status.database;
  const pg = db && db.version ? db.version.split(" ")[0] : null;
  return html`<footer class="statusbar" role="status">
    <span class=${cx("sb-item", down ? "bad" : status ? "ok" : "")}><span class="sb-dot"></span>${down ? "unreachable" : status ? "connected" : "connecting"}</span>
    ${pg && html`<span class="sb-sep"></span><span class="sb-item sb-wide">PostgreSQL ${pg}</span>`}
    ${db && html`<span class="sb-sep"></span><span class="sb-item">db ${db.latencyMs} ms</span>`}
    <span class="spacer"></span>
    ${PROJECT && html`<span class="sb-item">project ${PROJECT}</span><span class="sb-sep"></span>`}
    <a class="sb-item sb-wide" href=${location.protocol + "//" + apiBase + "/"} target="_blank" rel="noopener">${apiBase}</a>
    <span class="sb-sep sb-wide"></span>
    <span class="sb-item">${status ? "v" + status.version : "slonbase"}</span>
  </footer>`;
}

function Shell() {
  const { path } = useStore(route);
  const theme = useStore(app, (s) => s.theme);
  const [palette, setPalette] = useState(false);
  const [collapsed, setCollapsed] = useState(storedCollapsed);
  const [navOpen, setNavOpen] = useState(false);
  const toggleCollapsed = () =>
    setCollapsed((c) => {
      try {
        localStorage.setItem(COLLAPSED_KEY, c ? "0" : "1");
      } catch (_) {}
      return !c;
    });

  useEffect(() => {
    loadCollections().catch(() => {});
    api.get("/api/health").then((h) => app.set({ health: h.data })).catch(() => {});
    api
      .get("/api/settings")
      .then((st) => app.set({ hideControls: !!st.meta.hideControls }))
      .catch(() => {});
    const timer = setInterval(() => {
      if (session.expiresIn() < 3600) refreshAuth();
    }, 60_000);
    if (session.expiresIn() < 3600) refreshAuth();
    const key = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
      }
    };
    window.addEventListener("keydown", key);
    return () => {
      clearInterval(timer);
      window.removeEventListener("keydown", key);
    };
  }, []);

  const navLabel = (NAV.find((n) => n.path !== "/" && n.match(path)) || (path === "/" ? { label: "Overview" } : {})).label;
  const appName = useStore(branding, (b) => b.appName);
  useEffect(() => {
    document.title = pageTitle(navLabel);
  }, [navLabel, appName]);

  let page;
  let params;
  if (path === "/") page = html`<${Overview} />`;
  else if ((params = match("/data/:collection?/:tab?/:record?", path))) page = html`<${DataWorkspace} ...${params} />`;
  else if (path.startsWith("/logs")) page = html`<${Logs} />`;
  else if (path.startsWith("/diagram")) page = html`<${Diagram} />`;
  else if (path.startsWith("/health")) page = html`<${Health} />`;
  else if (path.startsWith("/sql")) page = html`<${SqlConsole} />`;
  else if (path.startsWith("/projects") && !PROJECT) page = html`<${Projects} />`;
  else if ((params = match("/settings/:section?", path))) page = html`<${Settings} section=${params.section || "general"} />`;
  else page = html`<div class="page"><div class="empty"><h3>Page not found</h3><a href="#/">Back to the overview</a></div></div>`;

  return html`<div class=${cx("app", { collapsed, "nav-open": navOpen })}>
    <${Sidebar}
      path=${path}
      collapsed=${collapsed}
      onCollapse=${toggleCollapsed}
      open=${navOpen}
      onClose=${() => setNavOpen(false)}
      onSearch=${() => {
        setNavOpen(false);
        setPalette(true);
      }}
    />
    ${navOpen && html`<div class="side-scrim" onClick=${() => setNavOpen(false)}></div>`}
    <div class="app-main">
      <header class="mobile-bar">
        <button class="icon-btn" aria-label="Menu" onClick=${() => setNavOpen(true)}><${Icon} name="menu" size=${20} /></button>
        <a class="brand" href="#/"><${BrandMark} size=${24} /></a>
        <span class="spacer"></span>
        <button class="icon-btn" aria-label="Search" onClick=${() => setPalette(true)}><${Icon} name="search" size=${18} /></button>
      </header>
      <main key=${path.split("/")[1]} class="fade-in app-page">${page}</main>
      <${StatusBar} />
    </div>
    ${palette && html`<${Palette} onClose=${() => setPalette(false)} />`}
  </div>`;
}

function Root() {
  const [signedIn, setSignedIn] = useState(!!session.token);
  const { path } = useStore(route);
  useEffect(() => session.subscribe((a) => setSignedIn(!!a)), []);

  // pages for links in emails work without signing in
  let params;
  if ((params = match("/auth/:action/:token", path))) {
    return html`<${PublicAuth} action=${params.action} token=${params.token} /><${Toasts} />`;
  }
  if (!signedIn) {
    let page = html`<${Login} />`;
    if (path === "/request-password-reset") page = html`<${PasswordResetRequest} />`;
    else if ((params = match("/confirm-password-reset/:token", path))) page = html`<${PasswordResetConfirm} token=${params.token} />`;
    return html`${page}<${Toasts} />`;
  }
  return html`<${Shell} /><${Toasts} />`;
}

render(html`<${Root} />`, document.getElementById("app"));
