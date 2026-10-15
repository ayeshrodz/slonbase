// Compares the results in bench/results as a Markdown table: slonbase, slonbase with
// asynchronous commits (if measured), and PocketBase.
import { readFileSync, existsSync } from "node:fs";

const path = (n) => new URL(`./results/${n}.json`, import.meta.url);
const load = (n) => (existsSync(path(n)) ? JSON.parse(readFileSync(path(n))) : null);
const s = load("slonbase");
const a = load("slonbase-async");
const p = load("pocketbase");

const num = (n) => (n >= 100 ? Math.round(n).toLocaleString("en-US") : n.toFixed(1));
const ms = (n) => (n == null ? "–" : n < 10 ? n.toFixed(2) : n.toFixed(1));
const spread = (r) => {
  const lo = Math.min(...r.runs);
  const hi = Math.max(...r.runs);
  return r.rps ? `±${Math.round(((hi - lo) / 2 / r.rps) * 100)}%` : "";
};
const ratio = (x, y) => (y?.rps ? `${(x.rps / y.rps).toFixed(2)}×` : "–");

const head = ["Scenario", "Clients", "slonbase req/s"];
if (a) head.push("slonbase, async commit");
head.push("PocketBase req/s", "slonbase / PocketBase");
if (a) head.push("async / PocketBase");
head.push("p50 ms (slonbase / PB)", "p99 ms (slonbase / PB)");
const lines = [`| ${head.join(" | ")} |`, `|---|${head.slice(1).map(() => "--:").join("|")}|`];
for (const x of s.scenarios) {
  const y = p.scenarios.find((r) => r.name === x.name);
  const z = a?.scenarios.find((r) => r.name === x.name);
  const row = [x.name, x.concurrency, `${num(x.rps)} ${spread(x)}`];
  if (a) row.push(z ? `${num(z.rps)} ${spread(z)}` : "–");
  row.push(`${num(y.rps)} ${spread(y)}`, ratio(x, y));
  if (a) row.push(z ? ratio(z, y) : "–");
  row.push(`${ms(x.p50)} / ${ms(y.p50)}`, `${ms(x.p99)} / ${ms(y.p99)}`);
  lines.push(`| ${row.join(" | ")} |`);
  if (x.failed || y.failed || z?.failed) lines.push(`|  ↳ failed requests: ${x.failed} / ${z?.failed ?? "–"} / ${y.failed} | | | | | | | | |`);
}
console.log(`${s.version} vs ${p.version}: median of ${s.runs} runs of ${s.duration} per scenario, ${s.cpus} CPUs (± is half the spread between runs)\n`);
console.log(lines.join("\n"));
