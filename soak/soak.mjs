// A soak test: realistic mixed traffic for a while against one or more slonbase instances on
// one database, checking that nothing degrades. Fails (exit 1) on request errors, memory that
// keeps growing, a p99 latency that drifts up, database connections still in use once the load
// stops, or realtime events a subscriber never got (from any instance). See README.md.
//
// SOAK_URLS (comma-separated instance URLs), SOAK_METRICS (their metrics URLs, same order),
// SOAK_PIDS (their process ids, for memory), SOAK_EMAIL / SOAK_PASSWORD (a superuser),
// SOAK_DURATION (seconds, 120), SOAK_CLIENTS (concurrent clients, 16), SOAK_SUBSCRIBERS (4),
// SOAK_SAMPLE (seconds between samples, 10), SOAK_MAX_RSS_GROWTH_MB (64), SOAK_MAX_P99_DRIFT (3),
// SOAK_MAX_REALTIME_LAG_MS (2000, the p99 from a create to its event).
import { readFileSync, writeFileSync } from "node:fs";

const env = (name, fallback) => process.env[name] ?? fallback;
const urls = env("SOAK_URLS", "http://127.0.0.1:8097").split(",");
const metricsUrls = env("SOAK_METRICS", "").split(",").filter(Boolean);
const pids = env("SOAK_PIDS", "").split(",").filter(Boolean);
const duration = Number(env("SOAK_DURATION", 120)) * 1000;
const clients = Number(env("SOAK_CLIENTS", 16));
const subscribers = Number(env("SOAK_SUBSCRIBERS", 4));
const sampleEvery = Number(env("SOAK_SAMPLE", 10)) * 1000;
const maxRssGrowth = Number(env("SOAK_MAX_RSS_GROWTH_MB", 64));
const maxP99Drift = Number(env("SOAK_MAX_P99_DRIFT", 3));
const maxRealtimeLag = Number(env("SOAK_MAX_REALTIME_LAG_MS", 2000));
const resultFile = env("SOAK_RESULT", "");

const pick = (list) => list[Math.floor(Math.random() * list.length)];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function call(method, path, { token, body, form, base = pick(urls) } = {}) {
  const headers = token ? { authorization: token } : {};
  let payload;
  if (form) payload = form;
  else if (body !== undefined) {
    headers["content-type"] = "application/json";
    payload = JSON.stringify(body);
  }
  const res = await fetch(base + path, { method, headers, body: payload });
  const text = await res.text();
  if (!res.ok) {
    const err = new Error(`${method} ${path}: ${res.status} ${text.slice(0, 200)}`);
    err.status = res.status;
    throw err;
  }
  return text ? JSON.parse(text) : null;
}

// ------------------------------------------------------------------ setup

const admin = (
  await call("POST", "/api/collections/_superusers/auth-with-password", {
    body: { identity: env("SOAK_EMAIL"), password: env("SOAK_PASSWORD") },
  })
).token;
await call("PATCH", "/api/settings", { token: admin, body: { rateLimits: { enabled: false } } });
const suffix = Math.random().toString(36).slice(2, 8);
const usersName = `soak_users_${suffix}`;
const itemsName = `soak_items_${suffix}`;
const users = await call("POST", "/api/collections", {
  token: admin,
  body: { name: usersName, type: "auth", fields: [{ name: "name", type: "text" }], createRule: "" },
});
await call("POST", "/api/collections", {
  token: admin,
  body: {
    name: itemsName,
    fields: [
      { name: "title", type: "text" },
      { name: "n", type: "number" },
      { name: "owner", type: "relation", collectionId: users.id, maxSelect: 1 },
      { name: "attachment", type: "file", maxSize: 1 << 20 },
    ],
    indexes: [`CREATE INDEX idx_${suffix}_n ON ${itemsName} (n)`],
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: "@request.auth.id != '' && owner = @request.auth.id",
    updateRule: "owner = @request.auth.id",
    deleteRule: "owner = @request.auth.id",
  },
});
const accounts = [];
for (let i = 0; i < clients; i++) {
  const email = `user${i}@${suffix}.soak`;
  await call("POST", `/api/collections/${usersName}/records`, {
    body: { email, password: "soak-password-1", passwordConfirm: "soak-password-1", name: `User ${i}` },
  });
  const auth = await call("POST", `/api/collections/${usersName}/auth-with-password`, {
    body: { identity: email, password: "soak-password-1" },
  });
  accounts.push({ token: auth.token, id: auth.record.id, items: [] });
}
// a steady data set to read
for (let i = 0; i < 1000; i++) {
  const account = accounts[i % accounts.length];
  const item = await call("POST", `/api/collections/${itemsName}/records`, {
    token: account.token,
    body: { title: `seed ${i}`, n: i, owner: account.id },
  });
  account.items.push(item.id);
}

// ------------------------------------------------------------------ realtime subscribers

const created = new Map(); // record id -> time created (only while every subscriber listens)
const deleted = new Set();
const subscriberState = [];

async function subscribe(index) {
  const base = urls[index % urls.length];
  const account = accounts[index % accounts.length];
  const state = { base, seen: new Set(), lags: [], connected: false, errors: 0 };
  subscriberState.push(state);
  const controller = new AbortController();
  state.close = () => controller.abort();
  const res = await fetch(base + "/api/realtime", { signal: controller.signal });
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  (async () => {
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let end;
        while ((end = buffer.indexOf("\n\n")) >= 0) {
          const block = buffer.slice(0, end);
          buffer = buffer.slice(end + 2);
          let event = "";
          let data = "";
          for (const line of block.split("\n")) {
            if (line.startsWith("event:")) event = line.slice(6).trim();
            else if (line.startsWith("data:")) data += line.slice(5).trim();
          }
          if (event === "PB_CONNECT") {
            const { clientId } = JSON.parse(data);
            await call("POST", "/api/realtime", {
              base: pick(urls), // any instance: the subscription finds the client's
              token: account.token,
              body: { clientId, subscriptions: [`${itemsName}/*`] },
            });
            state.connected = true;
          } else if (event === `${itemsName}/*`) {
            const message = JSON.parse(data);
            if (message.action === "create") {
              state.seen.add(message.record.id);
              const at = created.get(message.record.id);
              if (at) state.lags.push(Date.now() - at);
            }
          }
        }
      }
    } catch (err) {
      if (err.name !== "AbortError") state.errors++;
    }
  })();
}

for (let i = 0; i < subscribers; i++) await subscribe(i);
for (let i = 0; i < 100 && !subscriberState.every((s) => s.connected); i++) await sleep(100);
if (!subscriberState.every((s) => s.connected)) throw new Error("the realtime subscribers didn't connect");

// ------------------------------------------------------------------ the load

const windows = []; // per sample: { latencies: [], errors: 0, ops: 0 }
let current = { latencies: [], errors: 0, ops: 0 };
const errors = [];
const opCounts = {};
let running = true;

const ops = [
  // weight, name, run(account)
  [30, "list", (a) => call("GET", `/api/collections/${itemsName}/records?perPage=30&sort=-n`, { token: a.token })],
  [15, "list filtered", (a) =>
    call("GET", `/api/collections/${itemsName}/records?perPage=30&filter=${encodeURIComponent(`n > ${Math.floor(Math.random() * 900)} && owner != ''`)}&expand=owner`, { token: a.token })],
  [15, "view", async (a) => {
    if (!a.items.length) return;
    await call("GET", `/api/collections/${itemsName}/records/${pick(a.items)}`, { token: a.token });
  }],
  [12, "create", async (a) => {
    const item = await call("POST", `/api/collections/${itemsName}/records`, {
      token: a.token,
      body: { title: "soak", n: Math.floor(Math.random() * 1000), owner: a.id },
    });
    a.items.push(item.id);
    created.set(item.id, Date.now());
  }],
  [10, "update", async (a) => {
    if (!a.items.length) return;
    await call("PATCH", `/api/collections/${itemsName}/records/${pick(a.items)}`, {
      token: a.token,
      body: { n: Math.floor(Math.random() * 1000) },
    });
  }],
  // as many deletes as creates and uploads, so the data set stays the same size
  [16, "delete", async (a) => {
    if (a.items.length < 30) return;
    const id = a.items.splice(Math.floor(Math.random() * a.items.length), 1)[0];
    await call("DELETE", `/api/collections/${itemsName}/records/${id}`, { token: a.token });
    deleted.add(id);
  }],
  [4, "upload", async (a) => {
    const form = new FormData();
    form.set("title", "with a file");
    form.set("owner", a.id);
    form.set("attachment", new Blob([new Uint8Array(4096 + Math.floor(Math.random() * 8192))]), "data.bin");
    const item = await call("POST", `/api/collections/${itemsName}/records`, { token: a.token, form });
    a.items.push(item.id);
    created.set(item.id, Date.now());
  }],
  [4, "refresh", async (a) => {
    const auth = await call("POST", `/api/collections/${usersName}/auth-refresh`, { token: a.token });
    a.token = auth.token;
  }],
];
const totalWeight = ops.reduce((s, [w]) => s + w, 0);
function chooseOp() {
  let r = Math.random() * totalWeight;
  for (const op of ops) {
    if ((r -= op[0]) < 0) return op;
  }
  return ops[0];
}

async function worker(account) {
  while (running) {
    const [, name, run] = chooseOp();
    const started = performance.now();
    try {
      await run(account);
      current.latencies.push(performance.now() - started);
      opCounts[name] = (opCounts[name] ?? 0) + 1;
    } catch (err) {
      current.errors++;
      if (errors.length < 20) errors.push(`${name}: ${err.message}`);
    }
    current.ops++;
  }
}

function rssMb(pid) {
  try {
    const line = readFileSync(`/proc/${pid}/status`, "utf8").split("\n").find((l) => l.startsWith("VmRSS:"));
    return Number(line.split(/\s+/)[1]) / 1024;
  } catch {
    return NaN;
  }
}

async function metrics(url) {
  try {
    const text = await (await fetch(url)).text();
    const value = (pattern) => {
      const line = text.split("\n").find((l) => l.startsWith(pattern));
      return line ? Number(line.split(" ").pop()) : NaN;
    };
    return {
      inUse: value('slonbase_db_connections{state="in_use"}'),
      waiting: value("slonbase_db_waiting"),
      realtimeClients: value("slonbase_realtime_clients"),
    };
  } catch {
    return {};
  }
}

const percentile = (values, p) => {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))];
};

const baseline = await Promise.all(metricsUrls.map(metrics));
const samples = [];
const start = Date.now();
const workers = accounts.map((a) => worker(a));
console.log(
  `soaking ${urls.length} instance(s) for ${duration / 1000}s: ${clients} clients, ${subscribers} subscribers`,
);
while (Date.now() - start < duration) {
  await sleep(Math.min(sampleEvery, duration - (Date.now() - start)));
  const window = current;
  current = { latencies: [], errors: 0, ops: 0 };
  windows.push(window);
  const sample = {
    t: Math.round((Date.now() - start) / 1000),
    rps: Math.round(window.ops / (sampleEvery / 1000)),
    p50: Number(percentile(window.latencies, 50).toFixed(1)),
    p99: Number(percentile(window.latencies, 99).toFixed(1)),
    errors: window.errors,
    rssMb: pids.map((pid) => Number(rssMb(pid).toFixed(1))),
    pool: await Promise.all(metricsUrls.map(metrics)),
  };
  samples.push(sample);
  console.log(
    `${String(sample.t).padStart(5)}s  ${String(sample.rps).padStart(5)} req/s  p50 ${sample.p50} ms  p99 ${sample.p99} ms  errors ${sample.errors}  rss ${sample.rssMb.join("/")} MB`,
  );
}
running = false;
await Promise.all(workers);

// ------------------------------------------------------------------ checks

const failures = [];
const totalErrors = windows.reduce((s, w) => s + w.errors, 0);
if (totalErrors > 0) failures.push(`${totalErrors} failed requests, e.g. ${errors.slice(0, 5).join("; ")}`);

// realtime: every subscriber got every create (waiting for stragglers)
const missingOf = (s) => [...created.keys()].filter((id) => !s.seen.has(id));
for (let i = 0; i < 30 && subscriberState.some((s) => missingOf(s).length); i++) await sleep(500);
for (const [i, s] of subscriberState.entries()) {
  const missing = missingOf(s);
  if (missing.length) {
    const gone = missing.filter((id) => deleted.has(id)).length;
    failures.push(
      `subscriber ${i} (${s.base}) missed ${missing.length} of ${created.size} create events (${gone} of them for records deleted since)`,
    );
  }
  if (s.errors) failures.push(`subscriber ${i}'s stream broke`);
}

// memory: after the first quarter (warm-up), it mustn't keep growing
const warm = Math.max(1, Math.floor(samples.length / 4));
for (let i = 0; i < pids.length; i++) {
  const after = samples.slice(warm).map((s) => s.rssMb[i]).filter((v) => !Number.isNaN(v));
  if (after.length >= 2) {
    const growth = Math.max(...after.slice(-2)) - Math.min(...after.slice(0, 2));
    if (growth > maxRssGrowth) failures.push(`instance ${i}'s memory grew ${growth.toFixed(1)} MB after warm-up`);
  }
}

// latency: the last windows' p99 against the first ones' (after warm-up)
if (samples.length >= 4) {
  const early = Math.max(...samples.slice(warm, warm + 2).map((s) => s.p99));
  const late = Math.max(...samples.slice(-2).map((s) => s.p99));
  if (early > 0 && late > early * maxP99Drift) {
    failures.push(`p99 latency drifted from ${early} ms to ${late} ms`);
  }
}

// connections: back to what they were before the load (the realtime listener holds one)
await sleep(2000);
for (const [i, url] of metricsUrls.entries()) {
  const m = await metrics(url);
  if (m.inUse > baseline[i].inUse || m.waiting > 0) {
    failures.push(
      `instance ${i} has ${m.inUse} connections in use (${baseline[i].inUse} before the load) and ${m.waiting} waiting`,
    );
  }
}

const lags = subscriberState.flatMap((s) => s.lags);
const lagP99 = percentile(lags, 99);
if (lagP99 > maxRealtimeLag) failures.push(`realtime p99 delivery took ${lagP99} ms`);
for (const s of subscriberState) s.close();
const all = windows.flatMap((w) => w.latencies);
const summary = {
  instances: urls.length,
  durationSeconds: duration / 1000,
  clients,
  requests: windows.reduce((s, w) => s + w.ops, 0),
  errors: totalErrors,
  operations: opCounts,
  realtimeCreates: created.size,
  realtimeLag: { p50: percentile(lags, 50), p99: lagP99, max: lags.reduce((m, v) => Math.max(m, v), 0) },
  p50: Number(percentile(all, 50).toFixed(1)),
  p99: Number(percentile(all, 99).toFixed(1)),
  samples,
  failures,
};
if (resultFile) writeFileSync(resultFile, JSON.stringify(summary, null, 2));
console.log(
  `\n${summary.requests} requests (${Math.round(summary.requests / (duration / 1000))}/s), p50 ${summary.p50} ms, p99 ${summary.p99} ms, ${created.size} realtime creates checked on ${subscribers} subscribers (delivered in p50 ${summary.realtimeLag.p50} ms, p99 ${summary.realtimeLag.p99} ms, max ${summary.realtimeLag.max} ms)`,
);
if (failures.length) {
  console.log(`FAILED:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("passed");
process.exit(0);
