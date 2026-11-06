// Accessibility and screenshot checks of the dashboard, against a freshly set-up slonbase:
//
//   node check.mjs <base url> [--update]
//
// SB_EMAIL / SB_PASSWORD: a superuser of that installation. It seeds a small, fixed data set
// (no dates, fixed ids), then on each main screen, in light and dark:
// - runs axe-core (WCAG 2 A and AA) and fails on serious or critical violations;
// - compares a screenshot with baselines/<screen>-<theme>.png, both shrunk 4x (so text
//   anti-aliasing, which differs between machines, averages out), and fails when more than
//   MAX_DIFF of the pixels differ (the actual and diff images go to out/).
// --update writes the baselines instead. The overview is checked by axe only: its chart and
// greeting change with the time of day.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import pixelmatch from "pixelmatch";
import { PNG } from "pngjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const base = (process.argv[2] || "http://127.0.0.1:8090").replace(/\/$/, "");
const update = process.argv.includes("--update");
const email = process.env.SB_EMAIL || "admin@example.com";
const password = process.env.SB_PASSWORD || "admin-password-1";
const MAX_DIFF = Number(process.env.UI_MAX_DIFF || 0.02);
// screenshots are compared at a quarter of their size (each pixel the average of 4x4): text
// anti-aliasing differs between machines' font stacks, layout and colour changes don't average out
const SCALE = 4;
// font rendering pinned as far as Chromium allows
const LAUNCH_ARGS = (process.env.UI_LAUNCH_ARGS || "--font-render-hinting=none --disable-lcd-text --disable-font-subpixel-positioning").split(" ").filter(Boolean);
const VIEWPORT = { width: 1280, height: 800 };
const axeSource = fs.readFileSync(path.join(here, "node_modules/axe-core/axe.min.js"), "utf8");
const baselines = path.join(here, "baselines");
const out = path.join(here, "out");
fs.mkdirSync(baselines, { recursive: true });
fs.mkdirSync(out, { recursive: true });

async function api(method, url, body, token) {
  const res = await fetch(base + url, {
    method,
    headers: { "content-type": "application/json", ...(token ? { authorization: token } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  return { status: res.status, json: text ? JSON.parse(text) : null };
}

async function seed() {
  const login = await api("POST", "/api/collections/_superusers/auth-with-password", { identity: email, password });
  if (login.status !== 200) throw new Error(`sign-in failed: ${JSON.stringify(login.json)}`);
  const token = login.json.token;
  if ((await api("GET", "/api/collections/projects", undefined, token)).status === 200) return;
  const projects = await api("POST", "/api/collections", {
    id: "pbc_projects001", name: "projects", listRule: "", viewRule: "",
    fields: [{ name: "name", type: "text", required: true, presentable: true }],
  }, token);
  if (projects.status !== 200) throw new Error(`projects: ${JSON.stringify(projects.json)}`);
  const tasks = await api("POST", "/api/collections", {
    id: "pbc_tasks000001", name: "tasks", listRule: "", viewRule: "", createRule: "@request.auth.id != ''",
    fields: [
      { name: "title", type: "text", required: true, max: 120 },
      { name: "status", type: "select", values: ["backlog", "todo", "in progress", "review", "done"], maxSelect: 1 },
      { name: "labels", type: "select", values: ["bug", "feature", "docs", "infra", "design"], maxSelect: 3 },
      { name: "project", type: "relation", collectionId: "pbc_projects001", maxSelect: 1 },
      { name: "priority", type: "number", onlyInt: true },
      { name: "done", type: "bool" },
      { name: "notes", type: "editor" },
    ],
  }, token);
  if (tasks.status !== 200) throw new Error(`tasks: ${JSON.stringify(tasks.json)}`);
  const names = ["Mobile app", "Platform", "Website", "Billing"];
  for (const [i, name] of names.entries()) {
    const r = await api("POST", "/api/collections/projects/records", { id: `project0000000${i}`, name }, token);
    if (r.status !== 200) throw new Error(`project: ${JSON.stringify(r.json)}`);
  }
  const titles = ["Ship it on Postgres", "Dark mode for settings", "Fix a crash on cold start", "Push notification preferences",
    "Offline sync for drafts", "Mentions in comments", "Bulk edit tasks", "Audit log", "Weekly digest email", "Two-factor sign-in"];
  const statuses = ["done", "todo", "review", "backlog", "in progress"];
  const labels = [["feature"], ["bug", "infra"], ["docs"], ["design"], []];
  for (const [i, title] of titles.entries()) {
    const r = await api("POST", "/api/collections/tasks/records", {
      id: `task0000000000${i}`, title, status: statuses[i % 5], labels: labels[i % 5],
      project: `project0000000${i % 4}`, priority: (i % 4) + 1, done: i % 3 === 0, notes: `<p>Notes for ${title}.</p>`,
    }, token);
    if (r.status !== 200) throw new Error(`task: ${JSON.stringify(r.json)}`);
  }
}

// what changes from run to run, hidden from screenshots
const MASK = `
  *, *::before, *::after { transition: none !important; animation: none !important; caret-color: transparent !important; }
  .live-dot, .spinner, [aria-busy="true"] { visibility: hidden !important; }
`;

// `png` at 1/SCALE of its size, each pixel the average of a SCALE x SCALE block
function shrink(png) {
  const width = Math.floor(png.width / SCALE);
  const height = Math.floor(png.height / SCALE);
  const out = new PNG({ width, height });
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const sum = [0, 0, 0, 0];
      for (let dy = 0; dy < SCALE; dy++) {
        for (let dx = 0; dx < SCALE; dx++) {
          const i = ((y * SCALE + dy) * png.width + x * SCALE + dx) * 4;
          for (let c = 0; c < 4; c++) sum[c] += png.data[i + c];
        }
      }
      const o = (y * width + x) * 4;
      for (let c = 0; c < 4; c++) out.data[o + c] = Math.round(sum[c] / (SCALE * SCALE));
    }
  }
  return out;
}

async function check(page, name, { screenshot = true } = {}, theme, failures) {
  await page.addScriptTag({ content: axeSource });
  const violations = await page.evaluate(async () => {
    const result = await window.axe.run(document, { runOnly: ["wcag2a", "wcag2aa"] });
    return result.violations
      .filter((v) => v.impact === "serious" || v.impact === "critical")
      .map((v) => `${v.impact} ${v.id}: ${v.nodes.slice(0, 3).map((n) => n.target.join(" ")).join(", ")}`);
  });
  for (const v of violations) failures.push(`${name} (${theme}): ${v}`);
  if (!screenshot) return;
  await page.addStyleTag({ content: MASK });
  await page.mouse.move(VIEWPORT.width - 2, VIEWPORT.height - 2);
  await page.waitForTimeout(300);
  const file = `${name}-${theme}.png`;
  const shot = await page.screenshot();
  if (process.env.UI_SAVE_ALL) fs.writeFileSync(path.join(out, file), shot);
  if (update || !fs.existsSync(path.join(baselines, file))) {
    fs.writeFileSync(path.join(baselines, file), shot);
    console.log(`wrote baselines/${file}`);
    return;
  }
  const expected = PNG.sync.read(fs.readFileSync(path.join(baselines, file)));
  const actual = PNG.sync.read(shot);
  if (expected.width !== actual.width || expected.height !== actual.height) {
    failures.push(`${name} (${theme}): screenshot is ${actual.width}x${actual.height}, baseline ${expected.width}x${expected.height}`);
    fs.writeFileSync(path.join(out, file), shot);
    return;
  }
  const [e, a] = [shrink(expected), shrink(actual)];
  const diff = new PNG({ width: a.width, height: a.height });
  const differing = pixelmatch(e.data, a.data, diff.data, a.width, a.height, { threshold: 0.1 });
  const ratio = differing / (a.width * a.height);
  console.log(`${name} (${theme}): ${violations.length} accessibility problems, ${(ratio * 100).toFixed(2)}% of pixels differ`);
  if (ratio > MAX_DIFF) {
    failures.push(`${name} (${theme}): ${(ratio * 100).toFixed(2)}% of the pixels differ from the baseline`);
    fs.writeFileSync(path.join(out, file), shot);
    fs.writeFileSync(path.join(out, `diff-${file}`), PNG.sync.write(diff));
  }
}

await seed();
const browser = await chromium.launch({ args: LAUNCH_ARGS });
const failures = [];
const errors = [];
for (const theme of ["light", "dark"]) {
  // (the dashboard's CSP rightly refuses injected scripts; axe is the test's own)
  const context = await browser.newContext({ viewport: VIEWPORT, colorScheme: theme, bypassCSP: true, reducedMotion: "reduce" });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(`${theme}: ${e}`));
  await page.goto(base + "/_/");
  await page.evaluate((t) => localStorage.setItem("slonbase.theme", t), theme);
  await page.reload();
  await page.waitForSelector("input[type=password]");
  await page.waitForTimeout(500);
  await check(page, "sign-in", {}, theme, failures);
  await page.fill("input[type=email]", email);
  await page.fill("input[type=password]", password);
  await page.click("button[type=submit]");
  await page.waitForSelector(".sidebar");
  const screens = [
    ["overview", "#/", { screenshot: false }],
    ["records", "#/data/tasks/records", {}],
    ["schema", "#/data/tasks/schema", {}],
    ["rules", "#/data/tasks/rules", {}],
    ["settings", "#/settings", {}],
    // accessibility only
    ["branding", "#/settings/branding", { screenshot: false }],
    ["login screen", "#/settings/login", { screenshot: false }],
    ["backups", "#/settings/backups", { screenshot: false }],
    ["security", "#/settings/security", { screenshot: false }],
    ["crons", "#/settings/crons", { screenshot: false }],
    ["storage", "#/settings/storage", { screenshot: false }],
    ["logs settings", "#/settings/logs", { screenshot: false }],
    ["import", "#/settings/transfer", { screenshot: false }],
  ];
  for (const [name, hash, options] of screens) {
    await page.goto(`${base}/_/${hash}`);
    await page.waitForLoadState("networkidle");
    await page.waitForTimeout(800);
    await check(page, name, options, theme, failures);
  }
  // a record open in the drawer
  await page.goto(`${base}/_/#/data/tasks/records`);
  await page.waitForSelector("tbody tr[data-row]");
  await page.locator("tbody tr[data-row]").first().click();
  await page.waitForTimeout(800);
  await check(page, "editor", {}, theme, failures);
  await context.close();
}
await browser.close();
for (const e of errors) failures.push(`page error: ${e}`);
if (failures.length) {
  console.error(`\n${failures.length} problem(s):\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log(update ? "baselines written" : "dashboard checks passed");
