// Compares bench/results/slonbase.json and bench/results/pocketbase.json as a Markdown table.
import { readFileSync } from "node:fs";

const load = (t) => JSON.parse(readFileSync(new URL(`./results/${t}.json`, import.meta.url)));
const s = load("slonbase");
const p = load("pocketbase");

const fmt = (n) => (n >= 100 ? Math.round(n).toLocaleString("en-US") : n.toFixed(1));
const ms = (n) => (n == null ? "–" : n < 10 ? n.toFixed(2) : n.toFixed(1));
const lines = [
  `| Scenario | Clients | slonbase req/s | PocketBase req/s | slonbase / PocketBase | p50 ms (s / P) | p99 ms (s / P) |`,
  `|---|--:|--:|--:|--:|--:|--:|`,
];
for (const a of s.scenarios) {
  const b = p.scenarios.find((x) => x.name === a.name);
  if (!b) continue;
  const ratio = b.rps ? `${(a.rps / b.rps).toFixed(2)}×` : "–";
  const failed = a.failed || b.failed ? ` (failed: ${a.failed} / ${b.failed})` : "";
  lines.push(
    `| ${a.name}${failed} | ${a.concurrency} | ${fmt(a.rps)} | ${fmt(b.rps)} | ${ratio} | ${ms(a.p50)} / ${ms(b.p50)} | ${ms(a.p99)} / ${ms(b.p99)} |`,
  );
}
console.log(`${s.version} vs ${p.version}, ${s.duration} per scenario, ${s.cpus} CPUs\n`);
console.log(lines.join("\n"));
