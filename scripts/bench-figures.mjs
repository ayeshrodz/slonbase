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
    runs: data.slonbase.runs,
    duration: data.slonbase.duration,
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
    `${pgVersion(f)}, on ${machinePhrase(f)}${clock(f)}: the ${line(/^servers /, "CPU pinning")}. ` +
    `${clients} clients, medians of ${line(/ runs of /, "run settings")}.`;
  return {
    "setup": setup,
    "summary": `${head}\n${rows(false)}`,
    "summary-async": `${head}\n${rows(true)}`,
    "footprint": footprint,
  };
}

// how each scenario reads in a sentence
const PHRASE = {
  "health": "the health check",
  "view one record": "single-record reads",
  "view one record (1 client)": "single-record reads from one client",
  "list 30": "lists of 30",
  "list 30, sorted": "sorted lists",
  "list 30, filtered": "filtered lists",
  "list 30, expand author": "lists that expand a relation",
  "list 30, signed in": "lists for a signed-in user",
  "create record": "creating records",
  "sign in (bcrypt)": "sign-in, which bcrypt bounds",
};
const say = (name) => PHRASE[name] ?? name;
const x = (n) => `${n >= 10 ? Math.round(n) : n.toFixed(1)}×`;
const words = (l) => (l.length < 2 ? l.join("") : `${l.slice(0, -1).join(", ")} and ${l.at(-1)}`);

/** Where slonbase stands against another server, in words: ahead, level (within 5%) or behind. */
function standing(f, other, name, subject = "slonbase was") {
  const of = name ? ` of ${name}` : "";
  const theirs = other === "PocketBase" ? f.p : f.g;
  const rows = Object.keys(f.s).filter((k) => theirs[k]).map((k) => ({ k, r: f.s[k].rps / theirs[k].rps }));
  const ahead = rows.filter((r) => r.r >= 1.05).sort((a, b) => a.r - b.r);
  const level = rows.filter((r) => r.r < 1.05 && r.r > 1 / 1.05);
  const behind = rows.filter((r) => r.r <= 1 / 1.05);
  let text;
  if (ahead.length === rows.length) {
    text = `${subject} ahead${of} in every scenario measured, from ${x(ahead[0].r)} (${say(ahead[0].k)}) to ${x(ahead.at(-1).r)} (${say(ahead.at(-1).k)})`;
  } else {
    const top = ahead.at(-1);
    text = `${subject} ahead${of} in ${ahead.length} of ${rows.length} scenarios${top ? `, up to ${x(top.r)} (${say(top.k)})` : ""}`;
    if (level.length) text += `; level on ${words(level.map((r) => say(r.k)))}`;
    if (behind.length) text += `; behind on ${words(behind.map((r) => `${say(r.k)} (${x(r.r)})`))}`;
  }
  return { text, ahead, level, behind, rows };
}

/** The machine's description, as a phrase after "on". */
function machinePhrase(f) {
  const first = f.machine[0].replace(/\((R|TM)\)/g, "").replace(/\s+/g, " ").trim();
  return /^an? /.test(first) ? first : `one machine (${first})`;
}
// the clock settings machine.txt recorded, as "the performance governor, turbo boost off"
const clock = (f) => {
  const gov = f.machine.find((l) => /^CPU governor: /.test(l))?.slice(14);
  const turbo = f.machine.find((l) => /^turbo boost: /.test(l))?.slice(13);
  const parts = [gov && `the ${gov} governor`, turbo && `turbo boost ${turbo}`].filter(Boolean);
  return parts.length ? `, with ${parts.join(" and ")}` : "";
};
const pgVersion = (f) => (f.machine.find((l) => /^PostgreSQL /.test(l)) ?? "PostgreSQL").replace(/\s*\(.*\)\s*$/, "");

/** The sentences around the tables that state what the numbers say. */
export function prose(f = figures()) {
  const fp = f.footprint;
  const pb = standing(f, "PocketBase", "PocketBase");
  const pg = standing(f, "PostgREST", "", "it was");
  const idle = (k) => mbKb(fp[k].idleRssKb);
  const create = f.a["create record"];
  const durability = create
    ? `PostgreSQL waits for each commit to reach disk by default. PocketBase's SQLite in WAL mode does not, so with PostgreSQL's \`synchronous_commit = off\` (the same trade-off) slonbase creates ${rps(create.rps)} records a second.`
    : "PostgreSQL waits for each commit to reach disk by default; PocketBase's SQLite in WAL mode does not. The writes above are at PostgreSQL's default durability. `synchronous_commit = off` makes the same trade-off as SQLite and is faster (see [why](deployment.md#postgresql)).";
  const g = (k) => f.s[k] && f.g[k] ? f.s[k].rps / f.g[k].rps : null;
  return {
    "claims": `${pb.text}. Against PostgREST, ${pg.text}.`,
    "notes": `${clientsOf(f)} clients, medians of ${f.runs} runs of ${f.duration} on ${machinePhrase(f)}. ${pb.text}. ` +
      `PostgREST is a leaner program that does much less (no sign-in, files, realtime or dashboard): a ${idle("postgrest")} process against slonbase's ${idle("slonbase")}.`,
    "durability": durability,
    "lightest": (() => {
      const k = ["slonbase", "pocketbase", "postgrest"].reduce((m, n) => (fp[n].idleRssKb < fp[m].idleRssKb ? n : m));
      const lead = k === "postgrest" ? "PostgREST is the leanest, and it does the least: no sign-in, files, realtime or dashboard." : `${{ slonbase: "slonbase", pocketbase: "PocketBase" }[k]} used the least memory when idle.`;
      return `${lead} slonbase and PostgREST keep their data in a PostgreSQL server whose memory isn't counted; PocketBase's SQLite runs inside its process.`;
    })(),
    "postgrest": "Measured on the same machine, the same data and the same HTTP workloads, with PostgREST serving the tables slonbase created ([benchmarks](benchmarks.md)): " +
      `slonbase answered ${x(g("list 30"))} as many list requests per second (${rps(f.s["list 30"].rps)} against ${rps(f.g["list 30"].rps)}), ` +
      `${x(g("view one record"))} as many single-record reads and ${x(g("create record"))} as many creates` +
      (pg.behind.length ? `, and fewer on ${words(pg.behind.map((r) => say(r.k)))}` : "") +
      `. PostgREST is much lighter in memory (a ${idle("postgrest")} process against ${idle("slonbase")})` +
      (fp.postgrest.binaryBytes < fp.slonbase.binaryBytes ? ` and has a smaller binary (${mb(fp.postgrest.binaryBytes)} against ${mb(fp.slonbase.binaryBytes)})` : "") +
      ", since it does much less. " +
      "PostgREST was run at its defaults with a pool of 16; a tuned PostgREST may do better.",
    "faq": `In the public benchmark it served ${x(f.list.x)} as many list requests per second as PocketBase and ${x(g("list 30"))} as many as PostgREST.`,
  };
}
const clientsOf = (f) => {
  const seen = {};
  for (const [k] of SUMMARY) seen[f.s[k].concurrency] = (seen[f.s[k].concurrency] ?? 0) + 1;
  return Object.keys(seen).reduce((m, c) => (seen[c] > seen[m] ? c : m));
};

// the page: the chart's rows, top to bottom
const CHART = [
  ["list 30, filtered", "Filtered list"],
  ["list 30", "List 30 records"],
  ["list 30, sorted", "List, sorted"],
  ["list 30, expand author", "List with expand"],
  ["view one record", "View one record"],
  ["view one record (1 client)", "View one, one client"],
  ["create record", "Create a record"],
  ["sign in (bcrypt)", "Sign in (bcrypt)"],
];
const esc = (t) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const pct = (v, max) => `${((v / max) * 100).toFixed(1)}%`;

/** The project page's benchmark regions, as HTML. */
export function site(f = figures()) {
  const n = f.names, fp = f.footprint, pr = prose(f);
  const bars = (vals, d) => {
    const max = Math.max(...vals.filter((v) => v != null));
    return `<div class="tracks">${[["us", vals[0]], ["pb", vals[1]], ["pr", vals[2]]].filter(([, v]) => v != null)
      .map(([c, v]) => `<div class="track"><i class="${c}" style="--w:${pct(v, max)};--d:${d}"></i></div>`).join("")}</div>`;
  };
  let d = 0;
  const clients = Number(clientsOf(f));
  const rows = CHART.filter(([k]) => f.s[k] && f.p[k]).map(([k, label]) => {
    const s = f.s[k], p = f.p[k], g = f.g[k];
    // a scenario run at another concurrency than the header's says so (one client is in its label)
    if (s.concurrency !== clients && s.concurrency !== 1) label += `, ${s.concurrency} clients`;
    return `<div class="bar-row"><div class="label">${label}<small>${rps(s.rps)} · PocketBase ${rps(p.rps)} · PostgREST ${g ? rps(g.rps) : "n/a"}</small></div>` +
      `${bars([s.rps, p.rps, g ? g.rps : null], d++)}<div class="x">${x(s.rps / p.rps)}<small>vs PocketBase</small></div></div>`;
  });
  for (const [k, label] of [["list 30, filtered", "p99 latency, filtered list"], ["list 30", "p99 latency, list of 30"]]) {
    const s = f.s[k], p = f.p[k], g = f.g[k];
    rows.push(`<div class="bar-row"><div class="label">${label}<small>${ms(s.p99)} ms · PocketBase ${ms(p.p99)} ms · PostgREST ${g ? `${ms(g.p99)} ms` : "n/a"}</small></div>` +
      `${bars([s.p99, p.p99, g ? g.p99 : null], d++)}<div class="x">${x(p.p99 / s.p99)}<small>lower p99</small></div></div>`);
  }
  const card = (title, value, lower = true) => {
    const vals = ["slonbase", "pocketbase", "postgrest"].map((k) => value(fp[k]));
    const best = vals.reduce((m, v, i) => ((lower ? v.n < vals[m].n : v.n > vals[m].n) ? i : m), 0);
    return `<div class="fc"><h3>${title}</h3><table>${["slonbase", "PocketBase", "PostgREST"].map((name, i) => `<tr${i === best ? ' class="best"' : ""}><td>${name}</td><td>${vals[i].t}</td></tr>`).join("")}</table></div>`;
  };
  const legend = `<div class="legend"><span><i style="background:linear-gradient(90deg,var(--accent),var(--pg))"></i>slonbase ${f.version}</span><span><i style="background:color-mix(in srgb,var(--warn),var(--surface-2) 45%)"></i>PocketBase ${n.pocketbase}</span><span><i style="background:color-mix(in srgb,var(--info),var(--surface-2) 40%)"></i>PostgREST ${n.postgrest}</span></div>`;
  const perf = `<div class="perf-head">${legend}<div class="meta">${clientsOf(f)} clients unless noted · loopback, medians of ${f.runs} runs of ${f.duration}</div></div>${rows.join("")}`;
  const cards = card("Binary on disk", (v) => ({ n: v.binaryBytes, t: mb(v.binaryBytes) })) +
    card("Startup", (v) => ({ n: v.startupMs, t: `${v.startupMs} ms` })) +
    card("Memory, idle → under load", (v) => ({ n: v.idleRssKb, t: `${mbKb(v.idleRssKb)} → ${mbKb(v.peakRssKb)}` }));
  const honest = `<b>Where each one wins.</b> ${esc(pr.claims.replace(/`/g, ""))} It isn't free: slonbase and PostgREST need a PostgreSQL server (its memory isn't counted in these figures), and PocketBase is one file with nothing else to run. ` +
    `PostgREST is much lighter, with a resident process of ${mbKb(fp.postgrest.idleRssKb)}, because it does much less: no sign-in, files, realtime or dashboard. ` +
    `Measured on ${esc(machinePhrase(f))}, loopback, the database on the same cores as the server: the numbers show how the three compare here, and they will differ on your hardware.`;
  return {
    "stats": `<div class="stat" data-label="Lists"><b><span data-count="${x(f.list.x).slice(0, -1)}">${x(f.list.x).slice(0, -1)}</span>×</b><span>the requests per second of PocketBase on a list of 30, at ${x(f.list.p99)} lower p99 latency</span></div>` +
      `<div class="stat" data-label="Filtered"><b><span data-count="${x(f.filtered.x).slice(0, -1)}">${x(f.filtered.x).slice(0, -1)}</span>×</b><span>on a filtered list, where PostgreSQL plans the query and uses your index</span></div>` +
      `<div class="stat" data-label="Footprint"><b><span data-count="${Math.round(fp.slonbase.binaryBytes / 1048576)}">${Math.round(fp.slonbase.binaryBytes / 1048576)}</span><small>MB</small></b><span>one static binary with the dashboard built in; ${mbKb(fp.slonbase.idleRssKb)} of memory when idle</span></div>`,
    "perf": perf,
    "cards": cards,
    "honest": honest,
    "faq": esc(pr.faq),
  };
}

const TARGETS = ["README.md", "docs/benchmarks.md", "docs/slonbase-vs-postgrest.md", "site/index.html"];

function render(text, t) {
  text = text.replace(/(<!-- bench:([a-z-]+) -->\n?)[\s\S]*?(\n?<!-- bench:end -->)/g, (whole, open, name, close) => {
    if (!(name in t)) throw new Error(`unknown region bench:${name}`);
    return `${open}${t[name]}${close}`;
  });
  // the page's FAQ structured data says what its FAQ says
  if (t.faqJson) {
    text = text.replace(/<script type="application\/ld\+json">(\{[^<]*?"@type":\s*"FAQPage"[^<]*?)<\/script>/, (whole, json) => {
      const data = JSON.parse(json);
      for (const q of data.mainEntity) q.acceptedAnswer.text = q.acceptedAnswer.text.replace(/In the public benchmark.*?as PostgREST\./, t.faqJson);
      return `<script type="application/ld+json">${JSON.stringify(data).replace(/</g, "\\u003c")}</script>`;
    });
  }
  return text;
}

/** Every region, by name: the Markdown tables and sentences, and the page's HTML. */
export function all(f = figures()) {
  const pr = prose(f);
  return { ...tables(f), ...pr, ...Object.fromEntries(Object.entries(site(f)).map(([k, v]) => [`site-${k}`, v])), faqJson: pr.faq };
}

const main = process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);
if (main) {
  const check = process.argv.includes("--check");
  const t = all();
  const stale = [];
  for (const rel of TARGETS) {
    const file = path.join(ROOT, rel);
    const text = fs.readFileSync(file, "utf8");
    if (!text.includes("<!-- bench:")) throw new Error(`${rel} has no bench region`);
    const next = render(text, t);
    // a region belongs in the page's markup, never inside its structured data
    for (const [, body] of next.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)) {
      if (body.includes("<!-- bench:")) throw new Error(`${rel}: a bench region is inside a <script>`);
    }
    if (next === text) continue;
    if (check) stale.push(rel);
    else fs.writeFileSync(file, next);
  }
  if (stale.length) {
    console.error(`out of date with bench/published/${figures().version}: ${stale.join(", ")}\nrun: node scripts/bench-figures.mjs`);
    process.exit(1);
  }
  if (!check) console.log(`figures from bench/published/${figures().version} written to ${TARGETS.join(", ")}`);
}
