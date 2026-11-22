// Helpers shared by footprint.mjs and cascade.mjs: a throwaway slonbase installation on its own
// database, started from the binary at SLONBASE_BIN, and what the scripts measure of it.
//
// Needs `psql` and a PostgreSQL role that can create databases: ADMIN_DATABASE_URL points at any
// existing database (such as `postgres`) of that server. The databases made here are named
// `fp_<random>` and dropped at the end (the projects' too).
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const bin = process.env.SLONBASE_BIN || path.resolve("../target/release/slonbase");
export const adminUrl = process.env.ADMIN_DATABASE_URL;
export const email = "footprint-admin@example.com";
export const password = "footprint-admin-password-1";

if (!adminUrl) {
  console.error("ADMIN_DATABASE_URL must point at a database of a PostgreSQL server the role can create databases on");
  process.exit(2);
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Runs SQL on the admin database and returns the rows as text. */
export function sql(statement, url = adminUrl) {
  return execFileSync("psql", [url, "-Atq", "-v", "ON_ERROR_STOP=1", "-c", statement], {
    encoding: "utf8",
  }).trim();
}

/** The URL of another database on the same server. */
export function urlFor(database) {
  const [base, query] = adminUrl.split("?");
  return `${base.slice(0, base.lastIndexOf("/"))}/${database}${query ? `?${query}` : ""}`;
}

/** Reads one `Name:` line of /proc/<pid>/status as a number (kB for the memory lines). */
function status(pid, key) {
  const text = fs.readFileSync(`/proc/${pid}/status`, "utf8");
  const line = text.split("\n").find((l) => l.startsWith(`${key}:`));
  return line ? Number(line.split(/\s+/)[1]) : NaN;
}

/**
 * A slonbase installation on a new database. `args` go before `serve` (flags such as
 * `--hooks-pool`); `hooks` is a hooks directory to fill before starting.
 */
export async function start({ args = [], hooks = {} } = {}) {
  const name = `fp_${Math.random().toString(36).slice(2, 10)}`;
  sql(`CREATE DATABASE ${name}`);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "slonbase-footprint-"));
  const mainHooks = path.join(dir, "hooks");
  fs.mkdirSync(mainHooks, { recursive: true });
  for (const [file, content] of Object.entries(hooks)) fs.writeFileSync(path.join(mainHooks, file), content);
  const common = [
    "--config", path.join(dir, "slonbase.toml"),
    "--storage-path", path.join(dir, "storage"),
    "--backups-path", path.join(dir, "backups"),
    "--hooks-dir", mainHooks,
    "--projects-path", path.join(dir, "projects"),
  ];
  execFileSync(bin, [...common, "setup", "--database-url", urlFor(name), "--email", email, "--password", password], {
    stdio: "ignore",
  });
  const port = 20000 + Math.floor(Math.random() * 20000);
  const child = spawn(bin, [...common, ...args, "--bind", `127.0.0.1:${port}`, "serve"], {
    stdio: ["ignore", "ignore", fs.openSync(path.join(dir, "server.log"), "a")],
  });
  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 200; i++) {
    try {
      if ((await fetch(`${base}/api/health`)).ok) break;
    } catch {}
    await sleep(100);
  }
  const server = { name, dir, port, base, pid: child.pid, child, token: "" };
  const res = await fetch(`${base}/api/collections/_superusers/auth-with-password`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ identity: email, password }),
  });
  if (!res.ok) throw new Error(`sign-in failed: ${res.status} ${await res.text()}`);
  server.token = (await res.json()).token;
  return server;
}

/** A request with the superuser's token, returning the status and the parsed body. */
export async function api(server, method, url, body, root = server.base) {
  const res = await fetch(`${root}${url}`, {
    method,
    headers: { "content-type": "application/json", authorization: server.token },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {}
  return { status: res.status, body: json, text };
}

/** What the process and the database hold right now. */
export function measure(server) {
  const connections = Number(
    sql(
      `SELECT count(*) FROM pg_stat_activity WHERE datname = '${server.name}' OR datname LIKE '${server.name}\\_%'`,
    ),
  );
  return {
    rssMb: Math.round(status(server.pid, "VmRSS") / 1024),
    threads: status(server.pid, "Threads"),
    connections,
  };
}

/** Stops the server and drops its database and its projects'. */
export async function stop(server) {
  server.child.kill("SIGTERM");
  await new Promise((resolve) => server.child.once("exit", resolve));
  const names = sql(
    `SELECT datname FROM pg_database WHERE datname = '${server.name}' OR datname LIKE '${server.name}\\_%'`,
  )
    .split("\n")
    .filter(Boolean);
  for (const name of names) sql(`DROP DATABASE IF EXISTS "${name}" WITH (FORCE)`);
  fs.rmSync(server.dir, { recursive: true, force: true });
}
