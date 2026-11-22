// Starts a built slonbase binary on an empty database and uses it the way a person would:
// sets it up, signs in, creates a collection with a file field, stores a record with a file,
// reads both back, calls a JavaScript hook, and opens the dashboard. It runs on Linux, macOS and
// Windows (Node only, no packages), so the archives of each are checked by the same session.
//
//   SLONBASE_BIN=target/release/slonbase DATABASE_URL=postgres://... node scripts/e2e-smoke.mjs
//
// DATABASE_URL names an empty database the role may create tables in.
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const bin = process.env.SLONBASE_BIN;
const databaseUrl = process.env.DATABASE_URL;
if (!bin || !databaseUrl) {
  console.error("SLONBASE_BIN and DATABASE_URL must be set");
  process.exit(2);
}
const port = Number(process.env.E2E_PORT || 18090);
const base = `http://127.0.0.1:${port}`;
const email = "e2e@example.com";
const password = "e2e-password-123";

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "slonbase-e2e-"));
const hooks = path.join(dir, "hooks");
fs.mkdirSync(hooks, { recursive: true });
fs.writeFileSync(
  path.join(hooks, "main.pb.js"),
  'routerAdd("GET", "/api/e2e/ping", (e) => e.string(200, "pong from a hook"));\n',
);
const common = [
  "--config", path.join(dir, "slonbase.toml"),
  "--storage-path", path.join(dir, "storage"),
  "--backups-path", path.join(dir, "backups"),
  "--hooks-dir", hooks,
  "--projects-path", path.join(dir, "projects"),
];

let server = null;
let failed = false;
function step(name) {
  console.log(`- ${name}`);
}
function check(ok, what) {
  if (!ok) {
    failed = true;
    console.error(`  FAILED: ${what}`);
    throw new Error(what);
  }
}
async function wait(ms) {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

try {
  step(`version: ${execFileSync(bin, ["--version"], { encoding: "utf8" }).trim()}`);

  step("setup on an empty database");
  execFileSync(bin, [...common, "setup", "--database-url", databaseUrl, "--email", email, "--password", password], {
    stdio: "inherit",
  });

  step("start");
  server = spawn(bin, [...common, "--bind", `127.0.0.1:${port}`, "serve"], { stdio: "inherit" });
  let up = false;
  for (let i = 0; i < 120 && !up; i++) {
    try {
      up = (await fetch(`${base}/api/health`)).ok;
    } catch {}
    if (!up) await wait(500);
  }
  check(up, "the server answers /api/health");

  step("sign in as the superuser");
  let res = await fetch(`${base}/api/collections/_superusers/auth-with-password`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ identity: email, password }),
  });
  check(res.ok, `sign-in answers 200 (got ${res.status})`);
  const token = (await res.json()).token;
  check(Boolean(token), "sign-in returns a token");
  const auth = { authorization: token };

  step("create a collection with a text and a file field");
  res = await fetch(`${base}/api/collections`, {
    method: "POST",
    headers: { "content-type": "application/json", ...auth },
    body: JSON.stringify({
      name: "notes",
      listRule: "",
      viewRule: "",
      createRule: "",
      fields: [
        { name: "title", type: "text" },
        { name: "attachment", type: "file", maxSelect: 1 },
      ],
    }),
  });
  check(res.ok, `creating the collection answers 200 (got ${res.status})`);

  step("create a record with a file");
  const form = new FormData();
  form.append("title", "hello from the e2e session");
  form.append("attachment", new Blob(["file contents 123"], { type: "text/plain" }), "note.txt");
  res = await fetch(`${base}/api/collections/notes/records`, { method: "POST", body: form });
  check(res.ok, `creating the record answers 200 (got ${res.status})`);
  const record = await res.json();
  check(record.title === "hello from the e2e session", "the record keeps its title");
  check(typeof record.attachment === "string" && record.attachment.endsWith(".txt"), "the file is stored");

  step("list the records and read the file back");
  res = await fetch(`${base}/api/collections/notes/records`);
  check(res.ok && (await res.json()).totalItems === 1, "the list has the record");
  res = await fetch(`${base}/api/files/notes/${record.id}/${record.attachment}`);
  check(res.ok && (await res.text()) === "file contents 123", "the file is served unchanged");

  step("call a JavaScript hook");
  res = await fetch(`${base}/api/e2e/ping`);
  check(res.ok && (await res.text()) === "pong from a hook", "the hook route answers");

  step("open the dashboard");
  res = await fetch(`${base}/_/`);
  const html = await res.text();
  check(res.ok && html.includes("<html"), "the dashboard is served");

  console.log("the session passed");
} catch (err) {
  failed = true;
  console.error(String(err));
} finally {
  if (server) {
    server.kill();
    await wait(500);
  }
  fs.rmSync(dir, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
