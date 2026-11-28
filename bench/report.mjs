// Compares the results of bench/run.sh as a Markdown table: slonbase (and, when measured,
// slonbase with asynchronous commits and a previous release), PocketBase and PostgREST.
//
//   node report.mjs [results dir]     (default: bench/results)
//
// Each server is named by the version its run recorded, so a table says what it compares.
import { readFileSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.resolve(process.argv[2] || path.join(path.dirname(fileURLToPath(import.meta.url)), "results"));
const load = (n) => (existsSync(path.join(dir, `${n}.json`)) ? JSON.parse(readFileSync(path.join(dir, `${n}.json`))) : null);
const s = load("slonbase");
const a = load("slonbase-async");
const v = load("slonbase-previous");
const p = load("pocketbase");
const g = load("postgrest");
if (!s) {
  console.error(`no slonbase.json in ${dir}`);
  process.exit(1);
}

const num = (n) => (n >= 100 ? Math.round(n).toLocaleString("en-US") : n.toFixed(1));
const ms = (n) => (n == null ? "–" : n < 10 ? n.toFixed(2) : n.toFixed(1));
const spread = (r) => {
  const lo = Math.min(...r.runs);
  const hi = Math.max(...r.runs);
  return r.rps ? `±${Math.round(((hi - lo) / 2 / r.rps) * 100)}%` : "";
};
const ratio = (x, y) => (x && y?.rps ? `${(x.rps / y.rps).toFixed(2)}×` : "–");
const label = (r) => r.version.replace(/^(slonbase|pocketbase|postgrest) /, (_, n) => ({ slonbase: "slonbase ", pocketbase: "PocketBase ", postgrest: "PostgREST " })[n]);
const find = (r, name) => r?.scenarios.find((x) => x.name === name);

const head = ["Scenario", "Clients", `${label(s)} req/s`];
if (v) head.push(`${label(v)} req/s`, "vs previous");
if (a) head.push("async commit req/s");
if (p) head.push(`${label(p)} req/s`, "slonbase / PocketBase");
if (g) head.push(`${label(g)} req/s`, "slonbase / PostgREST");
const who = ["slonbase", p && "PB", g && "PostgREST"].filter(Boolean).join(" / ");
head.push(`p50 ms (${who})`, `p99 ms (${who})`);
const lines = [`| ${head.join(" | ")} |`, `|---|${head.slice(1).map(() => "--:").join("|")}|`];
for (const x of s.scenarios) {
  const y = find(p, x.name), z = find(a, x.name), w = find(v, x.name), q = find(g, x.name);
  const cell = (r) => (r ? `${num(r.rps)} ${spread(r)}` : "n/a");
  const row = [x.name, x.concurrency, cell(x)];
  if (v) row.push(cell(w), w ? ratio(x, w) : "–");
  if (a) row.push(cell(z));
  if (p) row.push(cell(y), ratio(x, y));
  if (g) row.push(cell(q), ratio(x, q));
  const servers = [x, ...(p ? [y] : []), ...(g ? [q] : [])];
  const lat = (k) => servers.map((r) => (r ? ms(r[k]) : "n/a")).join(" / ");
  row.push(lat("p50"), lat("p99"));
  lines.push(`| ${row.join(" | ")} |`);
  const failed = [x, y, q, z, w].filter(Boolean).reduce((n, r) => n + r.failed, 0);
  if (failed) lines.push(`|  ↳ failed requests: slonbase ${x.failed}${y ? `, PocketBase ${y.failed}` : ""}${q ? `, PostgREST ${q.failed}` : ""}${z ? `, async ${z.failed}` : ""}${w ? `, previous ${w.failed}` : ""} |${" |".repeat(head.length - 1)}`);
}
const others = [p, g].filter(Boolean).map(label).join(" and ");
console.log(`${label(s)}${others ? ` against ${others}` : ""}: median of ${s.runs} runs of ${s.duration} per scenario, ${s.cpus} CPUs (± is half the spread between runs)\n`);
console.log(lines.join("\n"));
