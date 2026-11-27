#!/usr/bin/env node
// The design tokens of the dashboard (ui/app/app.css), copied into the pages that are not the
// dashboard: the project page (site/index.html), its 404 page and the documentation site
// (scripts/docs/docs.css). They share the dashboard's palette, type scale and radii, so the
// three look like one product, and a change to the dashboard's tokens reaches all of them.
//
//   node scripts/build-tokens.mjs           rewrite the generated blocks
//   node scripts/build-tokens.mjs --check   exit 1 when a block is out of date (CI)
//
// Each target has one block between `@tokens:start` and `@tokens:end` comments. Everything
// outside the block is the page's own; inside, only this script writes. The block holds the
// dashboard's light and dark tokens, and a few that only the public pages use (code colours,
// the hero's glow): those are defined below, once for all the pages.

import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const SOURCE = path.join(ROOT, "ui/app/app.css");
const TARGETS = ["site/index.html", "site/404.html", "scripts/docs/docs.css"];

// the dashboard's own layout (its sidebar, status bar and page widths): not tokens of the design
const LAYOUT_ONLY = /^--(page-|side-|status-h|header-h)/;

// tokens only the public pages use, in the dashboard's colours where it has them
const EXTRA_LIGHT = {
  "--pg": "var(--tone-2)",
  "--glow": "color-mix(in srgb, var(--accent), transparent 82%)",
  "--control": "var(--surface)",
  "--control-hover": "color-mix(in srgb, var(--control), var(--ink) 4%)",
  "--hero-shadow": "0 30px 80px -20px rgba(31, 35, 40, 0.35), 0 0 0 1px var(--line-2)",
  // code is dark in both themes, as GitHub's dark default
  "--code-bg": "#0d1117",
  "--code-bar": "#161b22",
  "--code-line": "#30363d",
  "--code-ink": "#e6edf3",
  "--code-dim": "#8b949e",
  "--syn-keyword": "#ff7b72",
  "--syn-string": "#a5d6ff",
  "--syn-function": "#d2a8ff",
  "--syn-number": "#79c0ff",
  "--syn-comment": "#8b949e",
  // the window frame's three lights
  "--light-close": "#ec6a5e",
  "--light-min": "#f4be4f",
  "--light-max": "#61c554",
};
const EXTRA_DARK = {
  "--control": "#212830",
  "--control-hover": "#2a313b",
  "--hero-shadow": "0 30px 80px -20px rgba(0, 0, 0, 0.75), 0 0 0 1px var(--line-2)",
  "--code-bg": "#080b10",
  "--code-bar": "#0d1117",
};

/** The declarations of the first rule whose selector is exactly `selector`. */
function block(css, selector) {
  const at = css.indexOf(`\n${selector} {`);
  if (at < 0) throw new Error(`${SOURCE}: no "${selector}" rule`);
  const open = css.indexOf("{", at);
  const close = css.indexOf("\n}", open);
  const body = css.slice(open + 1, close);
  const decls = [];
  // declarations, one per statement, comments dropped
  for (const raw of body.replace(/\/\*[\s\S]*?\*\//g, "").split(";")) {
    const decl = raw.trim().replace(/\s+/g, " ");
    if (!decl) continue;
    const name = decl.slice(0, decl.indexOf(":")).trim();
    if (!name.startsWith("--") && name !== "color-scheme") continue;
    if (LAYOUT_ONLY.test(name)) continue;
    decls.push(decl);
  }
  return decls;
}

function render() {
  const css = fs.readFileSync(SOURCE, "utf8");
  const light = block(css, ":root");
  const dark = block(css, ':root[data-theme="dark"]');
  const extra = (o) => Object.entries(o).map(([k, v]) => `${k}: ${v}`);
  const lines = (decls) => decls.map((d) => `  ${d};`).join("\n");
  // color-scheme last, after the extras
  const scheme = (decls) => [...decls.filter((d) => !d.startsWith("color-scheme")), ...decls.filter((d) => d.startsWith("color-scheme"))];
  return [
    "/* @tokens:start: the dashboard's tokens (ui/app/app.css), written by scripts/build-tokens.mjs. Don't edit by hand. */",
    `:root {\n${lines(scheme([...light, ...extra(EXTRA_LIGHT)]))}\n}`,
    `:root[data-theme="dark"] {\n${lines(scheme([...dark, ...extra(EXTRA_DARK)]))}\n}`,
    "/* @tokens:end */",
  ].join("\n");
}

const START = /\/\* @tokens:start[^*]*\*\//;
const END = "/* @tokens:end */";

const check = process.argv.includes("--check");
const tokens = render();
let stale = [];
for (const rel of TARGETS) {
  const file = path.join(ROOT, rel);
  const text = fs.readFileSync(file, "utf8");
  const start = text.search(START);
  const end = text.indexOf(END);
  if (start < 0 || end < start) {
    console.error(`${rel}: no @tokens:start ... @tokens:end block`);
    process.exit(1);
  }
  const next = text.slice(0, start) + tokens + text.slice(end + END.length);
  if (next === text) continue;
  if (check) stale.push(rel);
  else fs.writeFileSync(file, next);
}
if (stale.length) {
  console.error(`out of date with ui/app/app.css: ${stale.join(", ")}\nrun: node scripts/build-tokens.mjs`);
  process.exit(1);
}

// colours belong in the tokens: a hex colour outside the generated block would drift again
let loose = [];
for (const rel of TARGETS) {
  const text = fs.readFileSync(path.join(ROOT, rel), "utf8");
  const outside = text.slice(0, text.search(START)) + text.slice(text.indexOf(END));
  // (in CSS and inline styles; an id or anchor such as href="#faq" is not a colour)
  for (const [i, line] of outside.split("\n").entries()) {
    for (const m of line.matchAll(/(?<![\w&/="'-])#[0-9a-fA-F]{3,8}\b(?![\w-])/g)) {
      if (/^#[0-9a-fA-F]{3}$|^#[0-9a-fA-F]{4}$|^#[0-9a-fA-F]{6}$|^#[0-9a-fA-F]{8}$/.test(m[0])) loose.push(`${rel}:${i + 1}: ${m[0]}`);
    }
  }
}
if (loose.length) {
  console.error(`colours outside the tokens (use a token, or add one in scripts/build-tokens.mjs):\n${loose.join("\n")}`);
  process.exit(1);
}
if (!check) console.log(`tokens written to ${TARGETS.join(", ")}`);
