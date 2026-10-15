// Turns the raw runs of bench/run.sh into one result per scenario: the median of each metric,
// with every run's throughput kept for the spread.
import { readFileSync } from "node:fs";

const [raw, name, version, duration, cpus, note] = process.argv.slice(2);
const rows = readFileSync(raw, "utf8").trim().split("\n").map((l) => JSON.parse(l));
const median = (xs) => {
  const v = xs.filter((x) => x != null).sort((a, b) => a - b);
  if (!v.length) return null;
  const m = Math.floor(v.length / 2);
  return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
};
const names = [...new Set(rows.map((r) => r.name))];
const scenarios = names.map((n) => {
  const runs = rows.filter((r) => r.name === n);
  return {
    name: n,
    concurrency: runs[0].concurrency,
    rps: median(runs.map((r) => r.rps)),
    p50: median(runs.map((r) => r.p50)),
    p90: median(runs.map((r) => r.p90)),
    p99: median(runs.map((r) => r.p99)),
    failed: runs.reduce((a, r) => a + r.failed, 0),
    runs: runs.map((r) => r.rps),
  };
});
console.log(JSON.stringify({ name, version, duration, cpus, runs: rows.length / names.length, note: note || "", scenarios }, null, 1));
