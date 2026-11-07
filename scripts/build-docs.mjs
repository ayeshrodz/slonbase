#!/usr/bin/env node
// Builds the documentation site from docs/*.md and CHANGELOG.md: one page per guide, with a
// sidebar, an outline of the page, search and the dashboard's fonts (IBM Plex Sans, JetBrains Mono). No dependencies beyond
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
import { execFileSync } from "node:child_process";
import { Marked } from "./vendor/marked.esm.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "https://github.com/ayeshrodz/slonbase";
const out = path.resolve(process.argv[2] || "site-out/docs");

// the guides, in reading order
const SECTIONS = [
  { title: "About slonbase", pages: ["about", "features"] },
  { title: "Get started", pages: ["quickstart", "installation", "configuration", "deployment", "platforms"] },
  { title: "Build", pages: ["api", "extending", "migrations", "projects"] },
  { title: "Move over", pages: ["migrating-from-pocketbase"] },
  { title: "Compare", pages: ["slonbase-and-pocketbase", "slonbase-vs-postgrest", "benchmarks"] },
  { title: "Under the hood", pages: ["design", "security-review"] },
  { title: "Releases", pages: ["stability", "changelog"] },
];
const SHORT = {
  about: "About",
  features: "Features",
  quickstart: "Quickstart",
  "slonbase-and-pocketbase": "vs PocketBase",
  "slonbase-vs-postgrest": "vs PostgREST",
  benchmarks: "Benchmarks",
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
  about: "What slonbase is, who it is for, what it is not, and who builds it.",
  features: "Everything it does: data and API, auth, realtime, files, dashboard, projects, scale and operations.",
  quickstart: "From a bare Linux server to a working backend with an API, sign-in and realtime in five minutes.",
  "slonbase-and-pocketbase": "What slonbase and PocketBase share, how they differ, and which to choose.",
  "slonbase-vs-postgrest": "A whole backend platform against a lean REST layer, with measured differences.",
  benchmarks: "Throughput, latency and footprint against PocketBase and PostgREST, with the method and caveats.",
  installation: "Install slonbase on Linux with one command, or with Docker, .deb and .rpm packages or a plain binary, then do the first-run setup and upgrade.",
  configuration: "Every slonbase server option, environment variable and application setting: database, storage, backups, mail, logs, rate limits and security.",
  deployment: "Run slonbase in production: TLS and reverse proxies, several instances behind a load balancer, PostgreSQL tuning, backups and monitoring.",
  platforms: "Deploy slonbase on Render, Railway, Fly.io, Coolify and Dokploy, step by step, with a PostgreSQL database for each.",
  api: "The slonbase REST and realtime API: collections, records, sign-in, filters and rules, files, batch requests and realtime subscriptions.",
  extending: "Extend slonbase with JavaScript hooks, routes and cron jobs, a query builder, and a Rust library API for embedding it in your own program.",
  migrations: "Manage schema changes as code with slonbase migrations in JavaScript, each in a transaction, applied on start with automigrate.",
  projects: "Run several isolated backends from one slonbase installation, each with its own users, collections, settings and PostgreSQL database.",
  "migrating-from-pocketbase": "Move a PocketBase app to slonbase and PostgreSQL: import the data with password hashes, keep your hooks and SDKs, and what differs.",
  design: "How slonbase works inside, the decisions behind it and the roadmap: collections as tables, the filter compiler, realtime and scale-out.",
  "security-review": "The security reviews of slonbase's authentication and request handling, the findings, and the fixes that followed.",
  stability: "What stays compatible between slonbase releases: the API, configuration, your data, hooks and migrations, under Semantic Versioning.",
  changelog: "Every slonbase release and what changed in it, with upgrade notes for each version.",
};

// titles that say what the page is about, for search results (the others are "<title> · slonbase docs")
const SEO_TITLE = {
  about: "About slonbase: an open-source backend platform on PostgreSQL",
  features: "slonbase features: API, auth, realtime, files and a dashboard on PostgreSQL",
  quickstart: "Quickstart: a self-hosted backend on PostgreSQL in five minutes",
  "slonbase-and-pocketbase": "slonbase and PocketBase: how they differ",
  "slonbase-vs-postgrest": "slonbase vs PostgREST: a backend platform or a REST layer",
  benchmarks: "slonbase benchmarks against PocketBase and PostgREST",
  "migrating-from-pocketbase": "Migrate from PocketBase to PostgreSQL with slonbase",
  installation: "Install slonbase: Linux, Docker, packages and binaries",
  configuration: "slonbase configuration: options, environment variables and settings",
  deployment: "Run slonbase in production: TLS, proxies, scaling and backups",
  platforms: "Deploy slonbase on Render, Railway, Fly.io, Coolify and Dokploy",
  api: "slonbase API reference: REST, filters, auth, files and realtime",
  extending: "Extend slonbase with JavaScript hooks, routes, cron jobs and Rust",
  migrations: "slonbase migrations: schema changes as code",
  stability: "slonbase stability promise and versioning",
  design: "How slonbase works: design notes and roadmap",
  "security-review": "slonbase security reviews and fixes",
  changelog: "slonbase changelog: every release",
  projects: "Many backends in one installation: slonbase projects",
};
const SITE = (process.env.SITE_URL || "https://slonbase.com").replace(/\/$/, "");
const lastModified = (file) => {
  try {
    const d = execFileSync("git", ["log", "-1", "--format=%cI", "--", file], { cwd: ROOT, stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
    if (d) return d.slice(0, 10);
  } catch {}
  return new Date().toISOString().slice(0, 10);
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

function page({ slug, title, description, content, prefix, toc, seoTitle }) {
  const url = `${SITE}/docs/${slug ? slug + "/" : ""}`;
  const docTitle = seoTitle || `${title} · slonbase docs`;
  const ld = [
    { "@context": "https://schema.org", "@type": "BreadcrumbList", itemListElement: [
      { "@type": "ListItem", position: 1, name: "slonbase", item: SITE + "/" },
      { "@type": "ListItem", position: 2, name: "Docs", item: SITE + "/docs/" },
      ...(slug ? [{ "@type": "ListItem", position: 3, name: SHORT[slug], item: url }] : []),
    ] },
    ...(slug ? [{ "@context": "https://schema.org", "@type": "TechArticle", headline: title, description, url, inLanguage: "en",
      dateModified: lastModified(source(slug)),
      author: { "@type": "Person", name: "Ayesh Rodrigo", url: "https://ayesh.dev/" },
      publisher: { "@type": "Organization", name: "slonbase", url: SITE + "/", logo: { "@type": "ImageObject", url: SITE + "/assets/logo.svg" } },
      mainEntityOfPage: url }] : []),
  ];
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
<title>${escape(docTitle)}</title>
<meta name="description" content="${escape(description)}">
<link rel="canonical" href="${url}">
${slug ? `<link rel="alternate" type="text/markdown" href="${SITE}/docs/${slug}.md">\n` : ""}<meta name="theme-color" content="#eff2f5" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#0b0f14" media="(prefers-color-scheme: dark)">
<meta property="og:type" content="${slug ? "article" : "website"}">
<meta property="og:site_name" content="slonbase">
<meta property="og:title" content="${escape(docTitle)}">
<meta property="og:description" content="${escape(description)}">
<meta property="og:url" content="${url}">
<meta property="og:image" content="${SITE}/assets/social-preview.png">
<meta name="twitter:card" content="summary_large_image">
<script type="application/ld+json">${JSON.stringify(ld).replace(/</g, "\\u003c")}</script>
<link rel="icon" href="${prefix}../assets/logo.svg" type="image/svg+xml">
<link rel="preload" href="${prefix}fonts/ibm-plex-sans-latin.woff2" as="font" type="font/woff2" crossorigin>
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
  fs.writeFileSync(path.join(dir, "index.html"), page({ slug, title: r.title, description: BLURB[slug], content, prefix: "../", toc: r.toc, seoTitle: SEO_TITLE[slug] }));
  index.push({ t: r.title, u: `${slug}/`, h: "", x: r.intro.slice(0, 400), s: SHORT[slug] });
  for (const s of r.sections) index.push({ t: r.title, u: `${slug}/#${s.id}`, h: s.heading, x: s.text, s: SHORT[slug] });
});

// the start page: every guide, by section
const cards = SECTIONS.map(
  (s) => `<section class="home-section"><h2>${escape(s.title)}</h2><div class="cards">${s.pages
    .map((p) => `<a class="card" href="${p}/"><b>${escape(SHORT[p])}</b><span>${escape(BLURB[p])}</span>${ICON.arrow}</a>`)
    .join("")}</div></section>`
).join("");
// the latest release's lead line, from the changelog
const latest = /^## \[(\d+\.\d+\.\d+)\][^\n]*\n+\*\*([^*]+)\*\*/m.exec(fs.readFileSync(path.join(ROOT, "CHANGELOG.md"), "utf8"));
const whatsNew = latest
  ? `<a class="whats-new" href="changelog/"><b>New in ${escape(latest[1])}</b><span>${escape(latest[2].replace(/\.$/, ""))}</span>${ICON.arrow}</a>`
  : "";
const home = `<div class="home">
  <div class="eyebrow">Documentation · v${escape(version)}</div>
  <h1>Build on slonbase</h1>
  <p class="lede">Everything about running slonbase and building on it: install it, define collections, call the API from any client, extend it with JavaScript hooks, and run it in production.</p>
  <div class="quick">
    <a class="btn primary" href="installation/">Install slonbase ${ICON.arrow}</a>
    <a class="btn" href="api/">Read the API</a>
  </div>
  ${whatsNew}
  ${cards}
</div>`;
fs.writeFileSync(path.join(out, "index.html"), page({ slug: "", title: "Documentation", description: "The slonbase documentation: install it, define collections, call the API, extend it with hooks and run it in production.", content: home, prefix: "", toc: null, seoTitle: "slonbase documentation: install, build, extend and run" }));

// ------------------------------------------------------------------ for search engines and assistants

// every guide as Markdown next to its page (/docs/api.md), with its links made absolute: assistants
// read Markdown far better than styled HTML
function absoluteLink(href, fromSlug) {
  if (/^([a-z]+:|#|\/\/)/i.test(href)) return href;
  const [file, hash] = href.split("#");
  const base = fromSlug === "changelog" ? ROOT : path.join(ROOT, "docs");
  const target = path.relative(ROOT, path.resolve(base, file));
  const anchor = hash ? "#" + hash : "";
  if (target === "CHANGELOG.md") return `${SITE}/docs/changelog.md${anchor}`;
  const m = /^docs\/([\w-]+)\.md$/.exec(target);
  if (m && order.includes(m[1])) return `${SITE}/docs/${m[1]}.md${anchor}`;
  if (target.startsWith("docs/assets/")) return `${SITE}/assets/${target.slice("docs/assets/".length)}`;
  const kind = fs.existsSync(path.join(ROOT, target)) && fs.statSync(path.join(ROOT, target)).isDirectory() ? "tree" : "blob";
  return `${REPO}/${kind}/main/${target}${anchor}`;
}
const markdown = {};
for (const slug of order) {
  const md = fs.readFileSync(path.join(ROOT, source(slug)), "utf8").replace(/(\]\()([^)\s]+)(\))/g, (_, a, href, c) => a + absoluteLink(href, slug) + c);
  markdown[slug] = md;
  fs.writeFileSync(path.join(out, `${slug}.md`), md);
}

// the site's root files, when the docs are built into <site>/docs
if (path.basename(out) === "docs") {
  const root = path.dirname(out);
  const urls = [
    [`${SITE}/`, lastModified("site/index.html")],
    [`${SITE}/docs/`, lastModified("scripts/build-docs.mjs")],
    ...order.map((slug) => [`${SITE}/docs/${slug}/`, lastModified(source(slug))]),
    [`${SITE}/packages/`, lastModified("scripts/build-repo.sh")],
  ];
  fs.writeFileSync(
    path.join(root, "sitemap.xml"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls
      .map(([loc, mod]) => `  <url><loc>${escape(loc)}</loc><lastmod>${mod}</lastmod></url>`)
      .join("\n")}\n</urlset>\n`
  );

  const SUMMARY =
    "slonbase is an open-source backend platform on PostgreSQL: one Rust binary with a REST and realtime API, authentication with roles, file storage, JavaScript hooks and an admin dashboard that manages many projects. It is self-hosted, MIT-licensed, scales across several instances on one database, and works with the PocketBase SDKs. It is an independent project (not a fork of PocketBase) built and maintained by Ayesh Rodrigo.";
  const optional = new Set(["design", "security-review", "changelog"]);
  const line = (slug) => `- [${SHORT[slug]}](${SITE}/docs/${slug}.md): ${BLURB[slug]}`;
  const groups = SECTIONS.map((g) => ({ title: g.title, pages: g.pages.filter((p) => !optional.has(p)) })).filter((g) => g.pages.length);
  const llms = `# slonbase

> ${SUMMARY}

Key facts: license MIT; written in Rust; needs PostgreSQL 14 or newer; Linux x86_64 and aarch64 (static binaries, .deb and .rpm packages, a Docker image); install with \`curl -fsSL ${SITE}/install.sh | sudo sh\`; current version ${version}. Website: ${SITE}/ . Source: ${REPO} .

${groups.map((g) => `## ${g.title}\n\n${g.pages.map(line).join("\n")}`).join("\n\n")}

## Optional

${[...optional].map(line).join("\n")}
- [Benchmark method and raw results](https://github.com/ayeshrodz/slonbase/tree/main/bench): scripts and the runs behind the numbers
- [Source code](${REPO}): the repository, issues and releases
`;
  fs.writeFileSync(path.join(root, "llms.txt"), llms);
  const full = order.filter((slug) => !optional.has(slug))
    .map((slug) => `<!-- ${SITE}/docs/${slug}/ -->\n${markdown[slug]}`)
    .join("\n\n---\n\n");
  fs.writeFileSync(path.join(root, "llms-full.txt"), `# slonbase documentation\n\n> ${SUMMARY}\n\nThis file is the guides of ${SITE}/docs/ as one document (the design notes, the security review and the changelog are left out: see ${SITE}/llms.txt).\n\n---\n\n${full}\n`);
}

fs.writeFileSync(path.join(out, "search.json"), JSON.stringify(index));
for (const f of ["docs.css", "docs.js"]) fs.copyFileSync(path.join(ROOT, "scripts/docs", f), path.join(out, f));
fs.mkdirSync(path.join(out, "fonts"), { recursive: true });
for (const f of ["ibm-plex-sans-latin.woff2", "ibm-plex-sans-latin-ext.woff2", "jetbrains-mono-latin.woff2", "jetbrains-mono-latin-ext.woff2", "LICENSE-ibm-plex-OFL.txt", "LICENSE-jetbrains-mono-OFL.txt"]) {
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
