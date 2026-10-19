// The dashboard: auth gate, shell (top navigation, command palette) and routes.
import { html, render, useState, useEffect, useRef, useLayoutEffect, useStore, route, navigate, match, cx } from "./lib/core.js";
import { session, refreshAuth, api, PROJECT, listProjects, dashboardUrl } from "./lib/api.js";
import { app, initTheme, setTheme, loadCollections } from "./lib/state.js";
import { Icon, Logo } from "./lib/icons.js";
import { Toasts, Menu, CollectionGlyph, useAutofocus } from "./lib/ui.js";
import { initials } from "./lib/format.js";
import { Login, PasswordResetRequest, PasswordResetConfirm } from "./pages/login.js";
import { PublicAuth } from "./pages/public-auth.js";
import { Overview } from "./pages/overview.js";
import { DataWorkspace } from "./pages/data.js";
import { Logs } from "./pages/logs.js";
import { Settings, applyAccent } from "./pages/settings.js";
import { SqlConsole } from "./pages/sql.js";
import { Projects } from "./pages/projects.js";

initTheme();
if (PROJECT) document.title = `${PROJECT} · slonbase`;

const NAV = [
  { path: "/", match: (p) => p === "/", icon: "overview", label: "Overview" },
  { path: "/data", match: (p) => p.startsWith("/data"), icon: "data", label: "Data" },
  { path: "/logs", match: (p) => p.startsWith("/logs"), icon: "logs", label: "Logs" },
  { path: "/sql", match: (p) => p.startsWith("/sql"), icon: "terminal", label: "SQL" },
  // projects are managed from the main installation
  !PROJECT && { path: "/projects", match: (p) => p.startsWith("/projects"), icon: "layers", label: "Projects" },
  { path: "/settings", match: (p) => p.startsWith("/settings"), icon: "settings", label: "Settings" },
].filter(Boolean);

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

function TopNav({ path }) {
  const ref = useRef();
  const [thumb, setThumb] = useState(null);
  const active = NAV.findIndex((n) => n.match(path));
  useLayoutEffect(() => {
    const measure = () => {
      const el = ref.current && ref.current.querySelector("a.on");
      setThumb(el ? { left: el.offsetLeft, width: el.offsetWidth } : null);
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [active]);
  return html`<nav class="nav" ref=${ref} aria-label="Main">
    ${thumb && html`<span class="nav-thumb" style=${{ left: thumb.left + "px", width: thumb.width + "px" }}></span>`}
    ${NAV.map(
      (n, i) => html`<a href=${"#" + n.path} class=${cx({ on: i === active })} aria-current=${i === active ? "page" : undefined}>
        <${Icon} name=${n.icon} size=${17} /><span>${n.label}</span>
      </a>`
    )}
  </nav>`;
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

function Shell() {
  const { path } = useStore(route);
  const theme = useStore(app, (s) => s.theme);
  const [palette, setPalette] = useState(false);
  const record = session.record || {};

  useEffect(() => {
    loadCollections().catch(() => {});
    api.get("/api/health").then((h) => app.set({ health: h.data })).catch(() => {});
    api
      .get("/api/settings")
      .then((st) => {
        applyAccent(st.meta.accentColor);
        app.set({ hideControls: !!st.meta.hideControls });
      })
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

  let page;
  let params;
  if (path === "/") page = html`<${Overview} />`;
  else if ((params = match("/data/:collection?/:tab?/:record?", path))) page = html`<${DataWorkspace} ...${params} />`;
  else if (path.startsWith("/logs")) page = html`<${Logs} />`;
  else if (path.startsWith("/sql")) page = html`<${SqlConsole} />`;
  else if (path.startsWith("/projects") && !PROJECT) page = html`<${Projects} />`;
  else if ((params = match("/settings/:section?", path))) page = html`<${Settings} section=${params.section || "general"} />`;
  else page = html`<div class="page"><div class="empty"><h3>Page not found</h3><a href="#/">Back to the overview</a></div></div>`;

  const isDark = document.documentElement.dataset.theme === "dark";
  return html`<div class="shell">
    <header class="topbar">
      <a class="brand" href="#/"><${Logo} size=${26} /><span class="brand-name">slon<b>base</b></span></a>
      <${ProjectSwitcher} />
      <${TopNav} path=${path} />
      <span class="spacer"></span>
      <button class="search-trigger" onClick=${() => setPalette(true)} aria-label="Search">
        <${Icon} name="search" size=${16} /><span>Jump to…</span><span class="kbd">${navigator.platform.includes("Mac") ? "⌘" : "Ctrl"} K</span>
      </button>
      <button
        class="icon-btn tip theme-btn"
        data-tip=${isDark ? "Light theme" : "Dark theme"}
        aria-label="Toggle theme"
        onClick=${() => setTheme(isDark ? "light" : "dark")}
        key=${theme}
      >
        <${Icon} name=${isDark ? "sun" : "moon"} size=${18} />
      </button>
      <${Menu}
        trigger=${(toggle) => html`<span class="avatar" onClick=${toggle} title=${record.email}>${initials(record.email)}</span>`}
        items=${[
          { icon: "user", label: record.email || "Superuser", onClick: () => navigate("/data/_superusers") },
          { icon: "settings", label: "Settings", onClick: () => navigate("/settings") },
          { icon: theme === "system" ? "check" : "sparkle", label: "Follow system theme", onClick: () => setTheme("system") },
          "-",
          { icon: "logout", label: "Sign out", danger: true, onClick: () => session.clear() },
        ]}
      />
    </header>
    <main key=${path.split("/")[1]} class="fade-in" style="flex:1;display:flex;flex-direction:column">${page}</main>
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
