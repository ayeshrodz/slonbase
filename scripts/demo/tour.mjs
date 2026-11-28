// A screencast of the dashboard for the site: records a scripted tour of the real UI with a
// visible cursor, as a webm in <out dir> (scripts/demo/run.sh turns it into mp4 and webm files).
//   node tour.mjs http://127.0.0.1:8222 <out dir>
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const require = createRequire(path.join(path.dirname(fileURLToPath(import.meta.url)), "../../ui/test/"));
const { chromium } = require("playwright");
const base = (process.argv[2] || "http://127.0.0.1:8222").replace(/\/$/, "");
const out = path.resolve(process.argv[3] || "tour");
fs.mkdirSync(out, { recursive: true });
const email = "admin@slonbase.dev", password = "admin-password-1";

// a cursor and a ripple on every click, installed on every page load
const OVERLAY = `
(() => {
  const css = document.createElement("style");
  css.textContent = ".sb-cursor{position:fixed;z-index:2147483647;left:0;top:0;width:22px;height:22px;pointer-events:none;transition:transform .08s linear;filter:drop-shadow(0 2px 3px rgba(0,0,0,.35))}" +
    ".sb-ripple{position:fixed;z-index:2147483646;width:34px;height:34px;margin:-17px 0 0 -17px;border-radius:50%;border:2px solid #4f7a6c;background:rgba(79,122,108,.22);pointer-events:none;animation:sbr .55s ease-out forwards}" +
    "@keyframes sbr{from{transform:scale(.3);opacity:1}to{transform:scale(1.6);opacity:0}}";
  const mount = () => {
    if (document.querySelector(".sb-cursor")) return;
    document.head.appendChild(css);
    const c = document.createElement("div");
    c.className = "sb-cursor";
    c.innerHTML = '<svg viewBox="0 0 24 24" width="22" height="22"><path d="M5 3l14 8-6 2-3 6z" fill="#fff" stroke="#111" stroke-width="1.5" stroke-linejoin="round"/></svg>';
    document.body.appendChild(c);
    addEventListener("mousemove", (e) => { c.style.transform = "translate(" + e.clientX + "px," + e.clientY + "px)"; }, true);
    addEventListener("mousedown", (e) => {
      const r = document.createElement("div");
      r.className = "sb-ripple"; r.style.left = e.clientX + "px"; r.style.top = e.clientY + "px";
      document.body.appendChild(r); setTimeout(() => r.remove(), 600);
    }, true);
  };
  if (document.body) mount(); else addEventListener("DOMContentLoaded", mount);
})();`;

const browser = await chromium.launch({ args: ["--font-render-hinting=none"], ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
const ctx = await browser.newContext({
  viewport: { width: 1280, height: 800 },
  deviceScaleFactor: 1,
  colorScheme: "light",
  recordVideo: { dir: out, size: { width: 1280, height: 800 } },
});
await ctx.addInitScript(OVERLAY);
const page = await ctx.newPage();
const wait = (ms) => page.waitForTimeout(ms);

async function moveTo(locator, { steps = 22 } = {}) {
  const box = await locator.boundingBox();
  if (!box) throw new Error("no box for " + locator);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps });
}
async function click(locator, pause = 700) {
  await moveTo(locator);
  await wait(120);
  await page.mouse.down(); await page.mouse.up();
  await wait(pause);
}
async function type(locator, text, delay = 45) {
  await click(locator, 150);
  await page.keyboard.type(text, { delay });
}
// a control that is on screen is clicked like a person would; one that isn't (a closed group of
// the sidebar) is reached by its address, so the tour doesn't stop on a layout change
async function open(locator, hash, pause = 1500) {
  const first = locator.first();
  if (await first.isVisible().catch(() => false)) await click(first, pause);
  else { await page.goto(`${base}/_/${hash}`); await wait(pause); }
}

// sign in
await page.goto(base + "/_/");
await page.evaluate(() => localStorage.setItem("slonbase.theme", "light"));
await page.reload();
await page.waitForSelector("input[type=password]");
await wait(1200);
await type(page.locator("input[type=email]"), email);
await type(page.locator("input[type=password]"), password, 35);
await wait(300);
await click(page.locator("button[type=submit]"), 1800);
await page.waitForSelector(".sidebar");
await wait(1400);

// Data: every collection, then one of them
await open(page.locator(".sidebar a[href='#/data']"), "#/data", 2000);
await open(page.locator("main").getByText("tasks", { exact: true }), "#/data/tasks/records", 1600);

// records: filter, a record in the drawer, a record arriving live from another program
const filter = page.locator("input[placeholder^='Filter']").first();
await type(filter, "status = 'in progress' && priority >= 3", 36);
await page.keyboard.press("Enter");
await wait(1600);
await filter.fill(""); await page.keyboard.press("Enter"); await wait(800);
await click(page.locator("tbody tr[data-row]").nth(2), 1500);
await page.keyboard.press("Escape"); await wait(500);
const login = await (await fetch(base + "/api/collections/_superusers/auth-with-password", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ identity: email, password }) })).json();
await fetch(base + "/api/collections/tasks/records", { method: "POST", headers: { "content-type": "application/json", authorization: login.token }, body: JSON.stringify({ title: "Launch slonbase.com", status: "in progress", priority: 4, estimate: 3, labels: ["feature"] }) });
await wait(2200);

// automations: the list, one opened in its panel (steps that pass data on), then a template
await open(page.locator(".sidebar a[href='#/automations']"), "#/automations", 1800);
const auto = page.locator("tr", { hasText: "Remind assignees of stale reviews" }).getByRole("button", { name: "Edit" }).first();
if (await auto.isVisible().catch(() => false)) {
  await click(auto, 1800);
  await page.mouse.wheel(0, 420); await wait(1500);
  await page.keyboard.press("Escape"); await wait(600);
}
await open(page.locator("a[href='#/automations/templates']"), "#/automations/templates", 1300);
const tpl = page.locator("tr", { hasText: "Task reminder" }).getByRole("button", { name: "Edit" }).first();
if (await tpl.isVisible().catch(() => false)) {
  await click(tpl, 2200);
  await page.mouse.wheel(0, 300); await wait(900);
  await page.keyboard.press("Escape"); await wait(600);
}

// the SQL console
await open(page.locator(".sidebar a[href='#/sql']"), "#/sql", 1100);
const editor = page.locator("textarea, [contenteditable=true]").first();
await click(editor, 200);
await page.keyboard.press("Control+A");
await page.keyboard.type("select status, count(*) as tasks, round(avg(priority)::numeric, 1) as avg_priority\nfrom slonbase.tasks\ngroup by status\norder by tasks desc;", { delay: 20 });
await page.keyboard.press("Control+Enter");
await wait(2000);

// Health, the command center
await open(page.locator(".sidebar a[href='#/health']"), "#/health", 2200);
await open(page.locator(".sidebar a[href='#/projects']"), "#/projects", 2600);

// settings: accents and dark mode, then the overview in the dark
await open(page.locator(".sidebar a[href='#/settings']"), "#/settings", 1300);
for (const name of ["Indigo", "Rose", "Sage"]) {
  const card = page.locator(".accent-card", { hasText: name }).first();
  if (await card.isVisible().catch(() => false)) await click(card, 800);
}
const dark = page.locator(".mode-card", { hasText: "Dark" }).first();
if (await dark.isVisible().catch(() => false)) await click(dark, 1600);
await open(page.locator(".sidebar a[href='#/']"), "#/", 2400);

const video = page.video();
await ctx.close();
await browser.close();
console.log("recorded", await video.path());
