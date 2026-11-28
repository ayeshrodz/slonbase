#!/usr/bin/env node
// The benchmark figures the README and the docs quote, from the newest published comparison
// (bench/published/<version>/ with slonbase, PocketBase and PostgREST results), so a new run is
// published by adding its folder, with no figure typed by hand:
//
//   node scripts/bench-figures.mjs           rewrite the tables between their markers
//   node scripts/bench-figures.mjs --check   exit 1 when a table is out of date (CI)
//
// A table is the text between `<!-- bench:NAME -->` and `<!-- bench:end -->` in a Markdown file.
// figures() is also what the project page's numbers can be built from.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const PUBLISHED = path.join(ROOT, "bench/published");

const semver = (v) => v.split(".").map(Number);
const newer = (a, b) => {
  const x = semver(a), y = semver(b);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  return 0;
};

/** The newest folder that compares all three, and its results. */
export function newest(dir = PUBLISHED) {
  const versions = fs.readdirSync(dir).filter((v) => /^\d+\.\d+\.\d+$/.test(v)).sort(newer).reverse();
  for (const v of versions) {
    const at = (f) => path.join(dir, v, f);
    const compact = v.replace(/\./g, "");
    const main = ["slonbase.json", `slonbase-${compact}.json`].find((f) => fs.existsSync(at(f)));
    if (!main || !fs.existsSync(at("pocketbase.json")) || !fs.existsSync(at("postgrest.json"))) continue;
    const load = (f) => JSON.parse(fs.readFileSync(at(f), "utf8"));
    if (!fs.existsSync(at("machine.txt"))) throw new Error(`bench/published/${v} has no machine.txt`);
    const machine = fs.readFileSync(at("machine.txt"), "utf8").split("\n").map((l) => l.trim()).filter(Boolean);
    const asyncFile = ["slonbase-async.json", `slonbase-${compact}-async.json`].find((f) => fs.existsSync(at(f)));
    return { version: v, slonbase: load(main), pocketbase: load("pocketbase.json"), postgrest: load("postgrest.json"), machine, async: asyncFile ? load(asyncFile) : null };
  }
  throw new Error(`no folder in ${dir} has slonbase, pocketbase.json and postgrest.json`);
}

// numbers as the tables write them
const rps = (n) => (n >= 100 ? Math.round(n).toLocaleString("en-US") : n.toFixed(1));
const ms = (n) => (n == null ? "n/a" : n >= 100 ? String(Math.round(n)) : n.toFixed(1));
const mb = (bytes) => `${(bytes / 1048576).toFixed(1)} MB`;
const mbKb = (kb) => `${Math.round(kb / 1024)} MB`;
const times = (x) => (x >= 10 ? `${Math.round(x)}` : `${x.toFixed(1)}`);
const short = (version) => version.split(" ").pop().split(".").slice(0, 2).join(".");
const full = (version) => version.split(" ").pop();

// the scenarios the summary shows, and how it names them
const SUMMARY = [
  ["health", "Health check"],
  ["view one record", "View one record"],
  ["list 30", "List 30 records"],
  ["list 30, filtered", "List 30, filtered"],
  ["list 30, expand author", "List 30, expand a relation"],
  ["create record", "Create a record"],
];

/** Every figure the README, the docs and the page use, from one comparison. */
export function figures(data = newest()) {
  const by = (r) => Object.fromEntries(r.scenarios.map((s) => [s.name, s]));
  const s = by(data.slonbase), p = by(data.pocketbase), g = by(data.postgrest), a = data.async ? by(data.async) : {};
  const ratio = (x, y) => (x && y && y.rps ? x.rps / y.rps : null);
  const vsPb = Object.keys(s).filter((n) => p[n]).map((n) => ({ name: n, x: ratio(s[n], p[n]) }));
  const vsPg = Object.keys(s).filter((n) => g[n]).map((n) => ({ name: n, x: ratio(s[n], g[n]) }));
  const low = (l) => l.reduce((m, r) => (r.x < m.x ? r : m));
  const high = (l) => l.reduce((m, r) => (r.x > m.x ? r : m));
  return {
    version: data.version,
    names: { slonbase: short(data.slonbase.version), pocketbase: full(data.pocketbase.version), postgrest: full(data.postgrest.version) },
    s, p, g, a,
    vsPocketBase: { low: low(vsPb), high: high(vsPb) },
    vsPostgREST: { low: low(vsPg), high: high(vsPg) },
    list: { x: ratio(s["list 30"], p["list 30"]), p99: p["list 30"].p99 / s["list 30"].p99 },
    filtered: { x: ratio(s["list 30, filtered"], p["list 30, filtered"]) },
    machine: data.machine,
    footprint: { slonbase: data.slonbase.footprint, pocketbase: data.pocketbase.footprint, postgrest: data.postgrest.footprint },
    times,
  };
}

/** The tables, by marker name. */
export function tables(f = figures()) {
  const n = f.names;
  const head = `| Scenario | slonbase ${n.slonbase} req/s | PocketBase ${n.pocketbase} req/s | PostgREST ${n.postgrest} req/s | p99 ms (slonbase / PocketBase / PostgREST) |\n|---|--:|--:|--:|--:|`;
  const rows = (withAsync) => SUMMARY.map(([key, label]) => {
    const x = f.s[key], y = f.p[key], z = f.g[key];
    const asyncNote = withAsync && f.a[key] && key === "create record" ? ` (${rps(f.a[key].rps)} with async commit)` : "";
    return `| ${label} | ${rps(x.rps)}${asyncNote} | ${y ? rps(y.rps) : "n/a"} | ${z ? rps(z.rps) : "n/a"} | ${ms(x.p99)} / ${y ? ms(y.p99) : "n/a"} / ${z ? ms(z.p99) : "n/a"} |`;
  }).join("\n");
  const fp = f.footprint;
  const footprint = `| | slonbase ${f.version} | PocketBase ${n.pocketbase} | PostgREST ${n.postgrest} |\n|---|--:|--:|--:|\n` +
    `| Binary on disk | ${mb(fp.slonbase.binaryBytes)} | ${mb(fp.pocketbase.binaryBytes)} | ${mb(fp.postgrest.binaryBytes)} |\n` +
    `| Resident memory, idle | ${mbKb(fp.slonbase.idleRssKb)} | ${mbKb(fp.pocketbase.idleRssKb)} | ${mbKb(fp.postgrest.idleRssKb)} |`;
  // how the comparison was run, from its machine.txt and its results
  const line = (re, what) => f.machine.find((l) => re.test(l)) ?? (() => { throw new Error(`machine.txt of ${f.version} has no ${what}`); })();
  const seen = {};
  for (const [k] of SUMMARY) seen[f.s[k].concurrency] = (seen[f.s[k].concurrency] ?? 0) + 1;
  const clients = Object.keys(seen).reduce((m, c) => (seen[c] > seen[m] ? c : m));
  const setup = `slonbase ${f.version}, PocketBase ${n.pocketbase} and PostgREST ${n.postgrest}, each at its defaults (PostgREST with a pool of 16), ` +
    `${line(/^PostgreSQL /, "PostgreSQL version")}, on ${f.machine[0]}: the ${line(/^servers /, "CPU pinning")}. ` +
    `${clients} clients, medians of ${line(/ runs of /, "run settings")}.`;
  return {
    "setup": setup,
    "summary": `${head}\n${rows(false)}`,
    "summary-async": `${head}\n${rows(true)}`,
    "footprint": footprint,
  };
}

const TARGETS = ["README.md", "docs/benchmarks.md"];

function render(text, t) {
  return text.replace(/(<!-- bench:([a-z-]+) -->\n)[\s\S]*?(\n<!-- bench:end -->)/g, (whole, open, name, close) => {
    if (!(name in t)) throw new Error(`unknown table bench:${name}`);
    return `${open}${t[name]}${close}`;
  });
}

const main = process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);
if (main) {
  const check = process.argv.includes("--check");
  const t = tables();
  const stale = [];
  for (const rel of TARGETS) {
    const file = path.join(ROOT, rel);
    const text = fs.readFileSync(file, "utf8");
    if (!text.includes("<!-- bench:")) throw new Error(`${rel} has no bench table`);
    const next = render(text, t);
    if (next === text) continue;
    if (check) stale.push(rel);
    else fs.writeFileSync(file, next);
  }
  if (stale.length) {
    console.error(`out of date with bench/published/${figures().version}: ${stale.join(", ")}\nrun: node scripts/bench-figures.mjs`);
    process.exit(1);
  }
  if (!check) console.log(`tables from bench/published/${figures().version} written to ${TARGETS.join(", ")}`);
}
