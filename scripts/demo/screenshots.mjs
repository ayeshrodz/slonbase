// Screenshots of the dashboard for README.md, the site and the docs (light and dark, 1440x900 at 2x):
//   node screenshots.mjs http://127.0.0.1:8222 <out dir> [screen,screen]
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
const browser = await chromium.launch({ args: ["--font-render-hinting=none"] });
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
