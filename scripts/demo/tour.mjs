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

const browser = await chromium.launch({ args: ["--font-render-hinting=none"] });
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

// the tour
await page.goto(base + "/_/");
await page.evaluate(() => localStorage.setItem("slonbase.theme", "light"));
await page.reload();
await page.waitForSelector("input[type=password]");
await wait(1400);
await type(page.locator("input[type=email]"), email);
await type(page.locator("input[type=password]"), password, 35);
await wait(300);
await click(page.locator("button[type=submit]"), 1800);
await page.waitForSelector(".sidebar");
await wait(1800);

// records: filter, a record in the drawer, a record arriving live
await click(page.locator(".sidebar a[href='#/data/tasks'], .sidebar a:has-text('tasks')").first(), 1500);
const filter = page.locator("input[placeholder^='Filter']").first();
await type(filter, "status = 'in progress' && priority >= 3", 38);
await page.keyboard.press("Enter");
await wait(1700);
await filter.fill(""); await page.keyboard.press("Enter"); await wait(900);
await click(page.locator("tbody tr[data-row]").nth(2), 1600);
await page.keyboard.press("Escape"); await wait(600);
// from "another program": a task created through the API shows up in the grid
const login = await (await fetch(base + "/api/collections/_superusers/auth-with-password", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ identity: email, password }) })).json();
await fetch(base + "/api/collections/tasks/records", { method: "POST", headers: { "content-type": "application/json", authorization: login.token }, body: JSON.stringify({ title: "Launch slonbase.com", status: "in progress", priority: 4, estimate: 3, labels: ["feature"] }) });
await wait(2200);

// schema, rules, API
for (const [name, pause] of [["Schema", 1700], ["API rules", 1700], ["API", 1900]]) {
  await click(page.getByRole("tab", { name: new RegExp("^" + name) }).first(), pause);
}
await page.mouse.wheel(0, 380); await wait(1100);

// the SQL console
await click(page.locator(".sidebar a[href='#/sql']"), 1200);
const editor = page.locator("textarea, [contenteditable=true]").first();
await click(editor, 200);
await page.keyboard.press("Control+A");
await page.keyboard.type("select status, count(*) as tasks, round(avg(priority)::numeric, 1) as avg_priority\nfrom slonbase.tasks\ngroup by status\norder by tasks desc;", { delay: 22 });
await page.keyboard.press("Control+Enter");
await wait(2200);

// logs, the command center
await click(page.locator(".sidebar a[href='#/logs']"), 2000);
await click(page.locator(".sidebar a[href='#/projects']"), 3000);

// settings: accents and dark mode
await click(page.locator(".sidebar a[href='#/settings']"), 1400);
for (const name of ["Indigo", "Rose", "Sage"]) {
  await click(page.locator(".accent-card", { hasText: name }).first(), 900);
}
await click(page.locator(".mode-card", { hasText: "Dark" }).first(), 1800);

// the email designer, then the overview in the dark
await click(page.locator("a[href='#/settings/mail']").first(), 1500);
await page.mouse.wheel(0, 700); await wait(1500);
await click(page.locator(".sidebar a[href='#/']").first(), 2400);

const video = page.video();
await ctx.close();
await browser.close();
console.log("recorded", await video.path());
