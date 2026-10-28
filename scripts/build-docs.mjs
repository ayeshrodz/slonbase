#!/usr/bin/env node
// Builds the documentation site from docs/*.md and CHANGELOG.md: one page per guide, with a
// sidebar, an outline of the page, search and the dashboard's fonts (Ubuntu Sans). No dependencies beyond
// Node (the Markdown parser is vendored in scripts/vendor).
//
//   node scripts/build-docs.mjs <output directory>     # e.g. site-out/docs
//
// The Pages workflow (.github/workflows/repo.yml) publishes it at /docs/ next to the project's
// page. Links between guides become links between pages; links to other files in the
// repository go to GitHub.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Marked } from "./vendor/marked.esm.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "https://github.com/ayeshrodz/slonbase";
const out = path.resolve(process.argv[2] || "site-out/docs");

// the guides, in reading order
const SECTIONS = [
  { title: "Get started", pages: ["installation", "configuration", "deployment", "platforms"] },
  { title: "Build", pages: ["api", "extending", "migrations", "projects"] },
  { title: "Move over", pages: ["migrating-from-pocketbase"] },
  { title: "Under the hood", pages: ["design", "security-review"] },
  { title: "Releases", pages: ["stability", "changelog"] },
];
const SHORT = {
  installation: "Installation",
  configuration: "Configuration",
  deployment: "Production",
  platforms: "Hosting platforms",
  api: "API",
  extending: "Hooks and extending",
  migrations: "Migrations",
  projects: "Projects",
  "migrating-from-pocketbase": "From PocketBase",
  design: "Design",
  "security-review": "Security review",
  stability: "Stability",
  changelog: "Changelog",
};
const BLURB = {
  installation: "Docker, packages and binaries, the first run and upgrades.",
  configuration: "Server options, environment variables and application settings.",
  deployment: "TLS, proxies, several instances, backups and monitoring.",
  platforms: "Render, Railway, Fly.io, Coolify and Dokploy, step by step.",
  api: "Collections, records, sign-in, filters and rules, files and realtime.",
  extending: "JavaScript hooks and routes, cron jobs, and the Rust library.",
  migrations: "Schema changes as code, applied on start.",
  projects: "Several isolated backends in one installation.",
  "migrating-from-pocketbase": "Moving an app over: data, hooks, migrations and SDKs.",
  design: "How slonbase works inside, and the roadmap.",
  "security-review": "The review of authentication, and what came of it.",
  stability: "What stays compatible from one release to the next within 1.x.",
  changelog: "Every release and what changed in it.",
};

const version = /^version = "([^"]+)"/m.exec(fs.readFileSync(path.join(ROOT, "Cargo.toml"), "utf8"))[1];
const order = SECTIONS.flatMap((s) => s.pages);
const sectionOf = Object.fromEntries(SECTIONS.flatMap((s) => s.pages.map((p) => [p, s.title])));
const source = (slug) => (slug === "changelog" ? "CHANGELOG.md" : `docs/${slug}.md`);

// ------------------------------------------------------------------ helpers

const escape = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const strip = (html) => html.replace(/<[^>]+>/g, "").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'");

// GitHub's heading ids, so links written for GitHub keep working
function slugger() {
  const seen = new Map();
  return (text) => {
    const base = text
      .toLowerCase()
      .trim()
      .replace(/[^\p{L}\p{N}\s_-]/gu, "")
      .replace(/\s/g, "-");
    const n = seen.get(base) || 0;
    seen.set(base, n + 1);
    return n ? `${base}-${n}` : base;
  };
}

// a link in a guide, as it should be on the site (pages live at /docs/<slug>/)
function rewriteLink(href, fromSlug) {
  if (/^([a-z]+:|#|\/\/)/i.test(href)) return href;
  const [file, hash] = href.split("#");
  const base = fromSlug === "changelog" ? ROOT : path.join(ROOT, "docs");
  const target = path.relative(ROOT, path.resolve(base, file));
  const anchor = hash ? "#" + hash : "";
  if (target === "CHANGELOG.md") return "../changelog/" + anchor;
  const m = /^docs\/([\w-]+)\.md$/.exec(target);
  if (m && order.includes(m[1])) return `../${m[1]}/${anchor}`;
  if (target.startsWith("docs/assets/")) return "../../assets/" + target.slice("docs/assets/".length);
  const kind = fs.existsSync(path.join(ROOT, target)) && fs.statSync(path.join(ROOT, target)).isDirectory() ? "tree" : "blob";
  return `${REPO}/${kind}/main/${target}${anchor}`;
}

// ------------------------------------------------------------------ code

const KEYWORDS = {
  js: "const let var function return if else for while of in new await async try catch throw class extends import export from default true false null undefined this typeof",
  rust: "fn let mut pub use mod struct enum impl trait for in if else match return async await move where self Self crate true false Some None Ok Err",
  sql: "select from where insert into values update set delete create table index unique on and or not null primary key references default alter drop as join left order by limit returning begin commit",
  sh: "sudo cd export echo if then fi for do done in",
  toml: "true false",
  nginx: "server listen location proxy_pass proxy_set_header proxy_http_version proxy_buffering proxy_read_timeout return",
};
const ALIASES = { javascript: "js", ts: "js", bash: "sh", shell: "sh", console: "sh", zsh: "sh" };

// a small tokenizer: comments, strings, numbers and keywords, enough to make examples readable
function highlight(code, lang) {
  lang = ALIASES[lang] || lang;
  const words = new Set((KEYWORDS[lang] || "").split(" ").filter(Boolean));
  const comment = { js: /\/\/[^\n]*|\/\*[\s\S]*?\*\//y, rust: /\/\/[^\n]*/y, sql: /--[^\n]*/y, sh: /#[^\n]*/y, toml: /#[^\n]*/y, nginx: /#[^\n]*/y }[lang];
  if (!comment && !words.size) return escape(code);
  const string = /"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`/y;
  const number = /\b\d[\d_.]*\b/y;
  const word = /[A-Za-z_$][\w$]*/y;
  let html = "";
  let i = 0;
  const at = (re) => {
    re.lastIndex = i;
    const m = re.exec(code);
    return m && m.index === i ? m[0] : null;
  };
  while (i < code.length) {
    let t;
    if (comment && (lang !== "sh" || i === 0 || /\s/.test(code[i - 1])) && (t = at(comment))) html += `<span class="c">${escape(t)}</span>`;
    else if ((t = at(string))) html += `<span class="s">${escape(t)}</span>`;
    else if ((t = at(number))) html += `<span class="n">${t}</span>`;
    else if ((t = at(word))) {
      const kw = words.has(lang === "sql" ? t.toLowerCase() : t);
      const call = !kw && code[i + t.length] === "(";
      html += kw ? `<span class="k">${t}</span>` : call ? `<span class="f">${t}</span>` : t;
    } else {
      t = code[i];
      html += escape(t);
    }
    i += t.length;
  }
  return html;
}

// ------------------------------------------------------------------ rendering

function render(slug) {
  let md = fs.readFileSync(path.join(ROOT, source(slug)), "utf8");
  const slug_ = slugger();
  const toc = [];
  const sections = []; // for search: [heading, id, text]
  let title = SHORT[slug];
  let titleId = "";
  const marked = new Marked({ gfm: true });
  marked.use({
    renderer: {
      heading({ tokens, depth }) {
        const html = this.parser.parseInline(tokens);
        const text = strip(html);
        if (depth === 1 && !toc.length && title === SHORT[slug]) {
          title = text;
          titleId = slug_(text);
          return "";
        }
        const id = slug_(text);
        // the changelog's outline is its releases; elsewhere, sections and subsections
        if (depth === 2 || (depth === 3 && slug !== "changelog")) toc.push({ depth, id, text });
        sections.push({ heading: text, id, text: "" });
        return `<h${depth} id="${id}"><a class="anchor" href="#${id}" aria-label="Link to this section">#</a>${html}</h${depth}>\n`;
      },
      link({ href, title: t, tokens }) {
        const url = rewriteLink(href, slug);
        const ext = /^https?:/.test(url) && !url.startsWith(REPO + "/") ? ' rel="noopener"' : "";
        return `<a href="${escape(url)}"${t ? ` title="${escape(t)}"` : ""}${ext}>${this.parser.parseInline(tokens)}</a>`;
      },
      image({ href, title: t, text }) {
        return `<img src="${escape(rewriteLink(href, slug))}" alt="${escape(text)}"${t ? ` title="${escape(t)}"` : ""} loading="lazy">`;
      },
      codespan({ text }) {
        // long identifiers wrap at their punctuation, not mid-word
        return `<code>${escape(text).replace(/([,/{(.](?!$))/g, "$1<wbr>")}</code>`;
      },
      code({ text, lang }) {
        const l = (lang || "").split(/\s/)[0];
        return `<div class="code"><div class="code-bar"><span>${escape(l || "text")}</span><button class="copy" type="button" aria-label="Copy">Copy</button></div><pre><code class="lang-${escape(l)}">${highlight(text, l)}</code></pre></div>\n`;
      },
      table(token) {
        // the default rendering, wrapped so wide tables scroll on their own
        const head = token.header.map((c) => `<th${c.align ? ` style="text-align:${c.align}"` : ""}>${this.parser.parseInline(c.tokens)}</th>`).join("");
        const rows = token.rows
          .map((r) => "<tr>" + r.map((c) => `<td${c.align ? ` style="text-align:${c.align}"` : ""}>${this.parser.parseInline(c.tokens)}</td>`).join("") + "</tr>")
          .join("\n");
        return `<div class="table"><table><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table></div>\n`;
      },
    },
  });
  if (slug === "changelog") {
    // the intro and link references aren't useful on the site
    md = md.replace(/^\[[^\]]+\]: http.*$/gm, "").replace(/^## \[([^\]]+)\]/gm, "## $1");
  }
  let body = marked.parse(md);
  // search text: what follows each heading, up to the next
  const parts = body.split(/<h[1-6] id="/);
  const intro = strip(parts[0]).replace(/\s+/g, " ").trim();
  parts.slice(1).forEach((p, i) => {
    if (sections[i]) sections[i].text = strip(p.slice(p.indexOf(">") + 1)).replace(/\s+/g, " ").trim().slice(0, 600);
  });
  return { title, titleId, body, toc, sections, intro };
}

// ------------------------------------------------------------------ the page

const ICON = {
  search: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.2-4.2"/></svg>',
  menu: '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M4 7h16M4 12h16M4 17h16"/></svg>',
  theme: '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5Z"/></svg>',
  github: '<svg viewBox="0 0 24 24" width="17" height="17" fill="currentColor"><path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.7c-2.78.6-3.37-1.34-3.37-1.34-.45-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.6.07-.6 1 .07 1.53 1.03 1.53 1.03.9 1.52 2.34 1.08 2.91.83.09-.65.35-1.09.63-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.64 0 0 .84-.27 2.75 1.02a9.6 9.6 0 0 1 5 0c1.91-1.29 2.75-1.02 2.75-1.02.55 1.37.2 2.39.1 2.64.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.69-4.57 4.93.36.31.68.92.68 1.85v2.75c0 .27.18.58.69.48A10 10 0 0 0 12 2Z"/></svg>',
  arrow: '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 12h15M14 6.5l5.5 5.5-5.5 5.5"/></svg>',
  back: '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M19.5 12h-15M10 6.5 4.5 12l5.5 5.5"/></svg>',
};

function page({ slug, title, description, content, prefix, toc }) {
  const nav = SECTIONS.map(
    (s) => `<div class="nav-group"><div class="nav-title">${escape(s.title)}</div>${s.pages
      .map((p) => `<a href="${prefix}${p}/"${p === slug ? ' class="on" aria-current="page"' : ""}>${escape(SHORT[p])}</a>`)
      .join("")}</div>`
  ).join("");
  const outline = toc && toc.length
    ? `<aside class="outline" aria-label="On this page"><div class="nav-title">On this page</div>${toc
        .map((t) => `<a href="#${t.id}" class="d${t.depth}">${escape(t.text)}</a>`)
        .join("")}</aside>`
    : "";
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escape(title)} · slonbase docs</title>
<meta name="description" content="${escape(description)}">
<link rel="icon" href="${prefix}../assets/logo.svg" type="image/svg+xml">
<link rel="preload" href="${prefix}fonts/ubuntu-sans-latin.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="${prefix}docs.css">
<script>
  try { var t = localStorage.getItem("theme"); } catch (e) {}
  document.documentElement.dataset.theme = t || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
</script>
</head>
<body data-prefix="${prefix}">
<header class="top">
  <button class="icon-btn menu-btn" aria-label="Menu">${ICON.menu}</button>
  <a class="brand" href="${prefix}../"><img src="${prefix}../assets/logo.svg" alt="" width="26" height="26"><span>slon<b>base</b></span></a>
  <a class="crumb" href="${prefix}">Docs</a>
  <span class="version">v${escape(version)}</span>
  <button class="search-btn" aria-label="Search the docs">${ICON.search}<span>Search the docs</span><kbd>/</kbd></button>
  <span class="spacer"></span>
  <a class="icon-btn" href="${REPO}" aria-label="slonbase on GitHub">${ICON.github}</a>
  <button class="icon-btn" id="theme" aria-label="Switch between light and dark">${ICON.theme}</button>
</header>
<div class="layout">
  <nav class="sidebar" aria-label="Guides">${nav}</nav>
  <main class="content">${content}</main>
  ${outline}
</div>
<div class="search" hidden>
  <div class="search-box" role="dialog" aria-label="Search the docs">
    <div class="search-input">${ICON.search}<input placeholder="Search the docs" aria-label="Search the docs" autocomplete="off" spellcheck="false"><kbd>esc</kbd></div>
    <div class="search-results"></div>
  </div>
</div>
<script src="${prefix}docs.js" defer></script>
</body>
</html>
`;
}

// ------------------------------------------------------------------ build

fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
const index = [];
const rendered = Object.fromEntries(order.map((slug) => [slug, render(slug)]));

order.forEach((slug, i) => {
  const r = rendered[slug];
  const prev = order[i - 1];
  const next = order[i + 1];
  const pager = `<nav class="pager">${prev ? `<a class="prev" href="../${prev}/"><span>${ICON.back} Previous</span><b>${escape(SHORT[prev])}</b></a>` : "<span></span>"}${
    next ? `<a class="next" href="../${next}/"><span>Next ${ICON.arrow}</span><b>${escape(SHORT[next])}</b></a>` : ""
  }</nav>`;
  const content = `<article class="doc">
  <div class="eyebrow">${escape(sectionOf[slug])}</div>
  <h1 id="${r.titleId}">${escape(r.title)}</h1>
  ${r.body}
  <div class="doc-foot"><a href="${REPO}/edit/main/${source(slug)}">Edit this page on GitHub</a></div>
  ${pager}
</article>`;
  const dir = path.join(out, slug);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "index.html"), page({ slug, title: r.title, description: BLURB[slug], content, prefix: "../", toc: r.toc }));
  index.push({ t: r.title, u: `${slug}/`, h: "", x: r.intro.slice(0, 400), s: SHORT[slug] });
  for (const s of r.sections) index.push({ t: r.title, u: `${slug}/#${s.id}`, h: s.heading, x: s.text, s: SHORT[slug] });
});

// the start page: every guide, by section
const cards = SECTIONS.map(
  (s) => `<section class="home-section"><h2>${escape(s.title)}</h2><div class="cards">${s.pages
    .map((p) => `<a class="card" href="${p}/"><b>${escape(SHORT[p])}</b><span>${escape(BLURB[p])}</span>${ICON.arrow}</a>`)
    .join("")}</div></section>`
).join("");
const home = `<div class="home">
  <div class="eyebrow">Documentation · v${escape(version)}</div>
  <h1>Build on slonbase</h1>
  <p class="lede">Everything about running slonbase and building on it: install it, define collections, call the API from any client, extend it with JavaScript hooks, and run it in production.</p>
  <div class="quick">
    <a class="btn primary" href="installation/">Install slonbase ${ICON.arrow}</a>
    <a class="btn" href="api/">Read the API</a>
  </div>
  ${cards}
</div>`;
fs.writeFileSync(path.join(out, "index.html"), page({ slug: "", title: "Documentation", description: "The slonbase documentation.", content: home, prefix: "", toc: null }));

fs.writeFileSync(path.join(out, "search.json"), JSON.stringify(index));
for (const f of ["docs.css", "docs.js"]) fs.copyFileSync(path.join(ROOT, "scripts/docs", f), path.join(out, f));
fs.mkdirSync(path.join(out, "fonts"), { recursive: true });
for (const f of ["ubuntu-sans-latin.woff2", "ubuntu-sans-latin-ext.woff2", "ubuntu-sans-mono-latin.woff2", "ubuntu-sans-mono-latin-ext.woff2", "LICENSE-ubuntu-font-licence.txt"]) {
  fs.copyFileSync(path.join(ROOT, "ui/app/fonts", f), path.join(out, "fonts", f));
}
// every link between pages, and every #anchor, has to lead somewhere
const broken = [];
const pages = ["index.html", ...order.map((p) => `${p}/index.html`)];
const html = Object.fromEntries(pages.map((p) => [p, fs.readFileSync(path.join(out, p), "utf8")]));
for (const p of pages) {
  for (const [, href] of html[p].matchAll(/href="([^"]+)"/g)) {
    if (/^([a-z]+:|\/\/)/i.test(href)) continue;
    const [file, anchor] = href.split("#");
    const target = path.posix.normalize(path.posix.join(path.posix.dirname(p), file || ""));
    if (target.startsWith("..")) continue; // the project page and its assets
    const page = /\.(css|js|json|svg|woff2|png)$/.test(target) ? target : path.posix.join(target, "index.html").replace(/^\.\//, "");
    if (!fs.existsSync(path.join(out, page))) broken.push(`${p}: ${href} (no such page)`);
    else if (anchor && html[page] && !html[page].includes(`id="${anchor}"`)) broken.push(`${p}: ${href} (no such heading)`);
  }
}
if (broken.length) {
  console.error("broken links:\n  " + broken.join("\n  "));
  process.exit(1);
}
console.log(`built ${order.length} pages and ${index.length} search entries into ${path.relative(process.cwd(), out) || "."}`);
