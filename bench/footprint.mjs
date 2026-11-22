// What a project costs: memory, threads and database connections of one slonbase process hosting
// 0, 1, 5, 20 and 50 projects, without hooks and with hooks (each project then has the same
// `main.pb.js`), idle, after some traffic, and after a pause. See README.md.
//
//   ADMIN_DATABASE_URL=postgres://... SLONBASE_BIN=../target/release/slonbase node footprint.mjs
//
// FOOTPRINT_COUNTS (1,5,20,50), FOOTPRINT_POOL_SIZE (the default of `hooks.pool_size`),
// FOOTPRINT_SERIES (none,hooks), FOOTPRINT_REQUESTS (30 per project), FOOTPRINT_SETTLE (5 seconds
// of waiting before a reading) and FOOTPRINT_PAUSE (20 seconds without traffic) change what runs.
// The server's PostgreSQL needs `max_connections` of at least 6 per project plus some.
import fs from "node:fs";
import path from "node:path";
import { api, measure, sleep, start, stop } from "./lib.mjs";

const counts = (process.env.FOOTPRINT_COUNTS || "1,5,20,50").split(",").map(Number);
const series = (process.env.FOOTPRINT_SERIES || "none,hooks").split(",");
const poolSize = process.env.FOOTPRINT_POOL_SIZE;
const requests = Number(process.env.FOOTPRINT_REQUESTS || 30);
const settle = Number(process.env.FOOTPRINT_SETTLE || 5) * 1000;
const pause = Number(process.env.FOOTPRINT_PAUSE || 20) * 1000;

const HOOK = `
routerAdd("GET", "/api/js/ping", (e) => e.string(200, "ok"));
onRecordCreate((e) => { e.next(); });
`;

async function ready(server, name, route) {
  for (let i = 0; i < 300; i++) {
    try {
      const res = await fetch(`${server.base}/p/${name}${route}`);
      if (res.ok) return;
    } catch {}
    await sleep(100);
  }
  throw new Error(`project ${name} did not become ready`);
}

async function addProject(server, name, withHooks) {
  const started = Date.now();
  const { status, text } = await api(server, "POST", "/api/projects", {
    name,
    superuserEmail: "owner@example.com",
    superuserPassword: "owner-password-1",
  });
  if (status !== 200) throw new Error(`creating ${name}: ${status} ${text}`);
  if (withHooks) {
    const dir = path.join(server.dir, "projects", name, "hooks");
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "main.pb.js"), HOOK);
    // the project restarts when its hook files change; wait for the new version
    await sleep(500);
    await ready(server, name, "/api/js/ping");
  } else {
    await ready(server, name, "/api/health");
  }
  return Date.now() - started;
}

async function traffic(server, names, route) {
  const work = names.flatMap((name) => Array.from({ length: requests }, () => name));
  let next = 0;
  await Promise.all(
    Array.from({ length: 16 }, async () => {
      while (next < work.length) {
        const name = work[next++];
        const res = await fetch(`${server.base}/p/${name}${route}`);
        await res.arrayBuffer();
        if (!res.ok) throw new Error(`${name}${route}: ${res.status}`);
      }
    }),
  );
}

async function run(kind) {
  const withHooks = kind === "hooks";
  const route = withHooks ? "/api/js/ping" : "/api/health";
  const args = poolSize ? ["--hooks-pool-size", poolSize] : [];
  const server = await start({ args });
  const rows = [];
  try {
    await sleep(settle);
    rows.push({ projects: 0, phase: "idle", ...measure(server) });
    const names = [];
    let createMs = [];
    for (const count of counts) {
      while (names.length < count) {
        const name = `p${names.length + 1}`;
        createMs.push(await addProject(server, name, withHooks));
        names.push(name);
      }
      await sleep(settle);
      const idle = measure(server);
      await traffic(server, names, route);
      const busy = measure(server);
      await sleep(pause);
      const later = measure(server);
      const average = Math.round(createMs.reduce((a, b) => a + b, 0) / createMs.length);
      rows.push({ projects: count, phase: "idle", createMs: average, ...idle });
      rows.push({ projects: count, phase: "after traffic", ...busy });
      rows.push({ projects: count, phase: `after ${pause / 1000} s idle`, ...later });
      createMs = [];
    }
  } finally {
    await stop(server);
  }
  return rows;
}

const result = {};
for (const kind of series) {
  console.error(`series: ${kind}`);
  result[kind] = await run(kind);
}
fs.mkdirSync("results", { recursive: true });
fs.writeFileSync("results/footprint.json", JSON.stringify({ poolSize: poolSize || "default", result }, null, 2));

for (const [kind, rows] of Object.entries(result)) {
  console.log(`\n${kind === "hooks" ? "Projects with hooks" : "Projects without hooks"}\n`);
  console.log("| Projects | Reading | RSS (MB) | Threads | Database connections | Create (ms) |");
  console.log("|--:|---|--:|--:|--:|--:|");
  for (const r of rows) {
    console.log(`| ${r.projects} | ${r.phase} | ${r.rssMb} | ${r.threads} | ${r.connections} | ${r.createMs ?? ""} |`);
  }
}
