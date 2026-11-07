// Questions to ask of several slonbase instances behind a balancer with no affinity:
//   node scenarios.mjs <balancer port> <instance port>...   (FLEET_DATABASE_URL: the shared database, for SQL checks)
// Each line prints PASS or FAIL; the exit code is the number of failures.
import { execFileSync } from "node:child_process";

const [lbPort, ...ports] = process.argv.slice(2).map(Number);
const LB = `http://127.0.0.1:${lbPort}`;
const direct = ports.map((p) => `http://127.0.0.1:${p}`);
const DATABASE_URL = process.env.FLEET_DATABASE_URL;
const EMAIL = process.env.FLEET_EMAIL || "admin@example.com";
const PASSWORD = process.env.FLEET_PASSWORD || "admin-password-1";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const ok = (name, pass, extra = "") => {
  if (!pass) failures++;
  console.log(`${pass ? "PASS" : "FAIL"}  ${name}${extra ? "  (" + extra + ")" : ""}`);
};

async function call(base, method, url, body, token) {
  const r = await fetch(base + url, { method, headers: { "content-type": "application/json", ...(token ? { authorization: token } : {}) }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  let json = null;
  try { json = text ? JSON.parse(text) : null; } catch { json = text; }
  return { status: r.status, by: r.headers.get("x-served-by"), json };
}
// polls `fn` on `base` until it returns true; the elapsed ms, or -1
async function until(fn, ms = 6000) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await fn()) return Date.now() - t0;
    await sleep(15);
  }
  return -1;
}

const login = await call(LB, "POST", "/api/collections/_superusers/auth-with-password", { identity: EMAIL, password: PASSWORD });
const T = login.json.token;

// 1. tokens carry no session
{
  const seen = new Set(), statuses = [];
  for (let i = 0; i < 6 * ports.length; i++) { const r = await call(LB, "GET", "/api/collections", undefined, T); seen.add(r.by); statuses.push(r.status); }
  ok("a superuser token is accepted by every instance", statuses.every((s) => s === 200) && seen.size === ports.length, `${seen.size} instances answered`);
}
await call(LB, "POST", "/api/collections", { name: "members", type: "auth", listRule: "", viewRule: "" }, T);
await until(async () => (await call(direct[direct.length - 1], "GET", "/api/collections/members")).status !== 404 || (await call(direct[direct.length - 1], "GET", "/api/collections/members", undefined, T)).status === 200);
const mk = async (email) => (await call(LB, "POST", "/api/collections/members/records", { email, password: "a-long-password-1", passwordConfirm: "a-long-password-1" }, T)).json;
const ann = await mk("ann@example.com"), bob = await mk("bob@example.com");
const si = async (email) => (await call(LB, "POST", "/api/collections/members/auth-with-password", { identity: email, password: "a-long-password-1" })).json.token;
const annToken = await si("ann@example.com"), bobToken = await si("bob@example.com");
{
  const rs = []; for (let i = 0; i < 3 * ports.length; i++) rs.push((await call(LB, "POST", "/api/collections/members/auth-refresh", undefined, annToken)).status);
  ok("a user token refreshes on every instance", rs.every((s) => s === 200));
}
// sign out everywhere: the old token dies on every instance at once
{
  const forbidden = (await call(LB, "POST", `/api/collections/members/records/${ann.id}/revoke-tokens`, undefined, bobToken)).status;
  const revoked = (await call(LB, "POST", `/api/collections/members/records/${ann.id}/revoke-tokens`, undefined, annToken)).status;
  const after = []; for (const d of direct) after.push((await call(d, "POST", "/api/collections/members/auth-refresh", undefined, annToken)).status);
  ok("revoking a user's tokens ends them on every instance", forbidden === 403 && revoked === 204 && after.every((s) => s === 401), `others can't: ${forbidden}, self: ${revoked}, old token: ${after}`);
}
// a password change does too, and so does deleting a superuser
{
  const t = await si("bob@example.com");
  await call(LB, "PATCH", `/api/collections/members/records/${bob.id}`, { password: "another-password-2", passwordConfirm: "another-password-2" }, T);
  const after = []; for (const d of direct) after.push((await call(d, "POST", "/api/collections/members/auth-refresh", undefined, t)).status);
  ok("a password change ends the user's tokens on every instance", after.every((s) => s === 401), `statuses ${after}`);
  const su = (await call(LB, "POST", "/api/collections/_superusers/records", { email: "temp@example.com", password: "temp-password-1", passwordConfirm: "temp-password-1" }, T)).json;
  const st = (await call(LB, "POST", "/api/collections/_superusers/auth-with-password", { identity: "temp@example.com", password: "temp-password-1" })).json.token;
  await call(LB, "DELETE", `/api/collections/_superusers/records/${su.id}`, undefined, T);
  const gone = []; for (const d of direct) gone.push((await call(d, "GET", "/api/collections", undefined, st)).status);
  ok("a deleted superuser's token is refused by every instance", gone.every((s) => s === 401 || s === 403 || s === 404), `statuses ${gone}`);
}

// 2. changes made on one instance reach the others
await call(direct[0], "POST", "/api/collections", { name: "notes", listRule: "", viewRule: "", createRule: "", fields: [{ name: "title", type: "text" }] }, T);
{
  const lat = [];
  for (const d of direct.slice(1)) lat.push(await until(async () => (await call(d, "GET", "/api/collections/notes/records")).status === 200));
  ok("a new collection becomes visible on the other instances", lat.every((x) => x >= 0), `after ms: ${lat}`);
  const col = (await call(direct[0], "GET", "/api/collections/notes", undefined, T)).json;
  await call(direct[direct.length - 1], "PATCH", "/api/collections/notes", { fields: [...col.fields, { name: "extra", type: "number" }] }, T);
  const lat2 = [];
  for (const d of direct.slice(0, -1)) lat2.push(await until(async () => { const r = await call(d, "POST", "/api/collections/notes/records", { title: "x", extra: 5 }); return r.status === 200 && r.json.extra === 5; }));
  ok("a new field is usable on the other instances", lat2.every((x) => x >= 0), `after ms: ${lat2}`);
  await call(direct[0], "PATCH", "/api/settings", { meta: { appName: "fleet-test" } }, T);
  const lat3 = [];
  for (const d of direct.slice(1)) lat3.push(await until(async () => (await call(d, "GET", "/api/settings", undefined, T)).json?.meta?.appName === "fleet-test"));
  ok("a settings change reaches the other instances", lat3.every((x) => x >= 0), `after ms: ${lat3}`);
}

// 3. rate limits count across instances
{
  await call(LB, "PATCH", "/api/settings", { rateLimits: { enabled: true, rules: [{ label: "/api/collections/notes/records", maxRequests: 30, duration: 20 }] } }, T);
  await sleep(500);
  const codes = []; for (let i = 0; i < 90; i++) codes.push((await call(LB, "GET", "/api/collections/notes/records")).status);
  const allowed = codes.filter((c) => c === 200).length;
  ok("a rate limit of 30 holds across the instances", allowed >= 28 && allowed <= 34, `allowed ${allowed} of 90; a per-instance limit would allow ${Math.min(90, 30 * ports.length)}`);
  await call(LB, "PATCH", "/api/settings", { rateLimits: { enabled: false } }, T);
  await sleep(300);
}

// 4. realtime: the stream on one instance, subscriptions and writes through the balancer
{
  const ctl = new AbortController(), events = [];
  const sse = await fetch(direct[0] + "/api/realtime", { signal: ctl.signal });
  const reader = sse.body.getReader(), dec = new TextDecoder();
  let buf = "", clientId = "";
  (async () => { try { for (;;) { const { value, done } = await reader.read(); if (done) break; buf += dec.decode(value); let i; while ((i = buf.indexOf("\n\n")) >= 0) { const b = buf.slice(0, i); buf = buf.slice(i + 2); const ev = /event: (.*)/.exec(b)?.[1], data = /data: (.*)/.exec(b)?.[1]; if (ev === "PB_CONNECT") clientId = JSON.parse(data).clientId; else if (ev) events.push(data || ""); } } } catch {} })();
  await until(async () => !!clientId, 4000);
  const subs = []; for (let i = 0; i < 3 * ports.length; i++) subs.push((await call(LB, "POST", "/api/realtime", { clientId, subscriptions: ["notes"] })).status);
  ok("subscribe requests are accepted whichever instance receives them", subs.every((s) => s === 204 || s === 200), `statuses ${[...new Set(subs)]}`);
  events.length = 0;
  for (let i = 0; i < 6; i++) await call(LB, "POST", "/api/collections/notes/records", { title: "rt" + i });
  const got = await until(async () => events.length >= 6, 4000);
  ok("events written through every instance reach a stream held by one", got >= 0, `${events.length} of 6`);
  if (DATABASE_URL) {
    execFileSync("psql", [DATABASE_URL, "-qc", "insert into slonbase.notes (id, title) values ('sqlinsert0000001', 'from sql')"]);
    const sql = await until(async () => events.some((e) => /from sql/.test(e)), 4000);
    ok("a change made in plain SQL reaches the stream", sql >= 0);
  }
  ctl.abort();
}

// 5. projects: created through one instance, served by all of them
{
  const r = await call(LB, "POST", "/api/projects", { name: "fleet", superuserEmail: "o@example.com", superuserPassword: "owner-password-1" }, T);
  const lat = [];
  for (const d of direct) lat.push(await until(async () => (await call(d, "GET", "/p/fleet/api/health")).status === 200, 15000));
  ok("a project created through one instance is served by every instance", r.status === 200 && lat.every((x) => x >= 0), `after ms: ${lat}`);
  const pt = (await call(LB, "POST", "/p/fleet/api/collections/_superusers/auth-with-password", { identity: "o@example.com", password: "owner-password-1" })).json?.token;
  const codes = []; for (let i = 0; i < 3 * ports.length; i++) codes.push((await call(LB, "GET", "/p/fleet/api/collections", undefined, pt)).status);
  ok("a project superuser's token works through the balancer", codes.every((c) => c === 200));
}

// 6. what Health says: the instances hear each other, files are flagged when local
{
  await sleep(21000); // the self-test runs every 20 s
  const status = (await call(direct[0], "GET", "/api/system/status", undefined, T)).json;
  const checks = status.checks || [];
  const sync = checks.find((c) => c.id === "sync");
  ok("Health says the instances stay in step", sync && sync.status === "ok", sync ? `${sync.status}: ${sync.title}` : "no sync check");
  const files = checks.find((c) => c.id === "shared-storage");
  console.log(`INFO  Health on instance 1: ${checks.filter((c) => c.status !== "ok").map((c) => c.id).join(", ")}${files ? "" : " (no shared-storage warning)"}`);
}

if (DATABASE_URL) {
  const n = execFileSync("psql", [DATABASE_URL, "-qtAc", "select count(*) from pg_stat_activity where datname like current_database() || '%'"], { encoding: "utf8" }).trim();
  console.log(`INFO  connections to the database and its projects, after the run: ${n}`);
}
console.log(failures ? `\n${failures} failed` : "\nall passed");
process.exit(failures);
