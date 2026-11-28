// Screenshots of the dashboard for README.md, the site and the docs (light and dark, 1440x900 at 2x):
//   node screenshots.mjs http://127.0.0.1:8222 <out dir> [screen,screen]
// Each is written as PNG and as WebP (smaller, for the site). CHROMIUM_PATH: a Chromium to use
// instead of Playwright's own.
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const require = createRequire(path.join(path.dirname(fileURLToPath(import.meta.url)), "../../ui/test/"));
const { chromium } = require("playwright");
const base = (process.argv[2] || "http://127.0.0.1:8222").replace(/\/$/, "");
const out = process.argv[3] || "screenshots";
const only = process.argv[4] ? process.argv[4].split(",") : null;
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch({ args: ["--font-render-hinting=none"], ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
for (const theme of ["light", "dark"]) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  const page = await ctx.newPage();
  await page.goto(base + "/_/");
  await page.evaluate((t) => localStorage.setItem("slonbase.theme", t), theme);
  await page.reload();
  await page.waitForSelector("input[type=password]");
  await page.fill("input[type=email]", "admin@slonbase.dev");
  await page.fill("input[type=password]", "admin-password-1");
  await page.click("button[type=submit]");
  await page.waitForSelector(".sidebar");
  const go = async (hash, wait = 1200) => { await page.goto(`${base}/_/${hash}`); await page.waitForTimeout(wait); };
  const shot = async (name) => { if (only && !only.includes(name)) return; await page.mouse.move(1430, 890); await page.screenshot({ path: `${out}/${name}-${theme}.png` }); console.log("shot", name, theme); };
  await go("#/data", 1800); await shot("data");
  await go("#/data/tasks/records"); await shot("records");
  await page.locator("tbody tr[data-row]").nth(1).click(); await page.waitForTimeout(900); await shot("editor");
  await go("#/data/tasks/schema"); await shot("schema");
  await go("#/data/tasks/rules"); await shot("rules");
  await go("#/data/tasks/api"); await shot("api");
  await go("#/"); await shot("overview");
  await go("#/diagram", 2000); await shot("diagram");
  await go("#/logs", 1500);
  const f = page.locator("input[placeholder^='e.g. data.url']");
  if (await f.count()) { await f.fill("data.url ~ 'tasks' || data.url ~ 'comments' || data.url ~ 'nope' || data.url ~ 'auth-with'"); await page.keyboard.press("Enter"); await page.waitForTimeout(1500); }
  await shot("logs");
  await go("#/projects", 2500); await shot("projects");
  await go("#/settings"); await shot("settings");
  await go("#/settings/mail", 1800); await shot("mail");
  await go("#/automations", 1800); await shot("automations");
  const auto = page.locator("tr", { hasText: "Remind assignees of stale reviews" }).getByRole("button", { name: "Edit" }).first();
  if (await auto.count()) { await auto.click(); await page.waitForTimeout(1400); await shot("automation-editor"); await page.keyboard.press("Escape"); await page.waitForTimeout(400); }
  await go("#/automations/templates", 1500);
  const tpl = page.locator("tr", { hasText: "Task reminder" }).getByRole("button", { name: "Edit" }).first();
  if (await tpl.count()) { await tpl.click(); await page.waitForTimeout(1500); }
  await shot("templates");
  await go("#/health", 2200); await shot("health");
  await go("#/sql", 1000);
  const ed = page.locator("textarea, [contenteditable=true]").first();
  if (await ed.count()) {
    await ed.click(); await page.keyboard.press("Control+A");
    await page.keyboard.type("select status, count(*) as tasks, round(avg(priority)::numeric, 1) as avg_priority\nfrom slonbase.tasks\ngroup by status\norder by tasks desc;");
    await page.keyboard.press("Control+Enter"); await page.waitForTimeout(1500);
  }
  await shot("sql");
  await ctx.close();
}
await browser.close();

// WebP copies for the site (the PNGs stay for README.md and the docs)
const { execFileSync } = await import("node:child_process");
for (const f of fs.readdirSync(out).filter((f) => f.endsWith(".png"))) {
  try {
    execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-i", path.join(out, f), "-c:v", "libwebp", "-quality", "82", path.join(out, f.replace(/\.png$/, ".webp"))]);
  } catch (e) {
    console.warn("no webp for", f, e.message);
  }
}
