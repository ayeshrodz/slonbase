// Checks of the project page and the documentation site, assembled as the deploy assembles them
// (see run.sh), on phones, a tablet and desktops, in light and dark:
//
//   node check.mjs <directory of the assembled site>
//
// On each page, at each size and theme it fails on:
// - anything wider than the screen (the page scrolls sideways);
// - a control smaller than 44 by 44 pixels where the pointer is a finger (phones, tablet);
// - serious or critical axe-core violations (WCAG 2 A and AA);
// - an id used twice;
// - errors in the console (requests to GitHub's API are blocked: the page must work without them).
// Screenshots of the whole page go to out/<page>-<size>-<theme>.png, for a person to look at.
//
// CHROMIUM_PATH: a Chromium to use instead of Playwright's own.

import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, devices } from "playwright";

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = path.resolve(process.argv[2] || "");
if (!process.argv[2] || !fs.existsSync(path.join(dir, "index.html"))) {
  console.error("usage: node check.mjs <directory of the assembled site>");
  process.exit(2);
}
const out = path.join(here, "out");
fs.mkdirSync(out, { recursive: true });
const axeSource = fs.readFileSync(path.join(here, "node_modules/axe-core/axe.min.js"), "utf8");

const SIZES = [
  { name: "phone-320", ...devices["iPhone SE"] },
  { name: "phone-412", ...devices["Pixel 7"] },
  { name: "tablet-768", viewport: { width: 768, height: 1024 }, hasTouch: true, isMobile: true },
  { name: "laptop-1280", viewport: { width: 1280, height: 800 } },
  { name: "desktop-1440", viewport: { width: 1440, height: 900 } },
];
const PAGES = [
  { name: "home", path: "/" },
  { name: "docs-installation", path: "/docs/installation/" },
  { name: "not-found", path: "/404.html" },
];
const TYPES = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".json": "application/json", ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg", ".webp": "image/webp", ".webm": "video/webm", ".mp4": "video/mp4", ".woff2": "font/woff2", ".txt": "text/plain", ".ico": "image/x-icon", ".webmanifest": "application/manifest+json" };

// a static server for the assembled site, as GitHub Pages serves it (a directory serves its index)
const server = http.createServer((req, res) => {
  let file = path.join(dir, decodeURIComponent(new URL(req.url, "http://x").pathname));
  if (!file.startsWith(dir)) return res.writeHead(403).end();
  if (fs.existsSync(file) && fs.statSync(file).isDirectory()) file = path.join(file, "index.html");
  if (!fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": TYPES[path.extname(file)] || "application/octet-stream" });
  fs.createReadStream(file).pipe(res);
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const base = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const failures = [];
for (const size of SIZES) {
  for (const theme of ["light", "dark"]) {
    const { name, ...options } = size;
    const context = await browser.newContext({ ...options, colorScheme: theme, reducedMotion: "reduce" });
    // the page reads GitHub's API for stars and releases; without it, it keeps what it was built with
    await context.route(/api\.github\.com/, (route) => route.abort());
    await context.addInitScript((t) => { try { localStorage.setItem("theme", t); } catch (e) {} }, theme);
    for (const pg of PAGES) {
      const page = await context.newPage();
      const errors = [];
      page.on("console", (m) => { if (m.type() === "error" && !/api\.github\.com|ERR_FAILED/.test(m.text())) errors.push(m.text()); });
      page.on("pageerror", (e) => errors.push(String(e)));
      await page.goto(base + pg.path, { waitUntil: "networkidle" });
      const where = `${pg.name} ${name} ${theme}`;
      for (const e of errors) failures.push(`${where}: console: ${e}`);

      // ids are unique (a link to #tour went to the first element with that id)
      const dupes = await page.evaluate(() => {
        const seen = new Map();
        for (const el of document.querySelectorAll("[id]")) seen.set(el.id, (seen.get(el.id) || 0) + 1);
        return [...seen].filter(([, n]) => n > 1).map(([id]) => id);
      });
      if (dupes.length && theme === "light" && name === SIZES[0].name) failures.push(`${pg.name}: duplicate ids: ${dupes.join(", ")}`);

      // nothing wider than the screen
      const wide = await page.evaluate(() => {
        const w = document.documentElement.clientWidth;
        const found = [];
        if (document.documentElement.scrollWidth > w + 1) {
          for (const el of document.body.querySelectorAll("*")) {
            const r = el.getBoundingClientRect();
            if (r.width && r.right > w + 1 && getComputedStyle(el).position !== "fixed") {
              // report the outermost overflowing element only
              if (![...found].some((f) => f.el.contains(el))) found.push({ el, d: `${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}${el.className && typeof el.className === "string" ? "." + el.className.trim().split(/\s+/).join(".") : ""} (${Math.round(r.right)}px)` });
            }
          }
          return { scroll: document.documentElement.scrollWidth, w, found: found.slice(0, 5).map((f) => f.d) };
        }
        return null;
      });
      if (wide) failures.push(`${where}: ${wide.scroll}px wide on a ${wide.w}px screen: ${wide.found.join(", ")}`);

      // fingers need 44 by 44 (WCAG 2.5.5); links inside running text are exempt
      if (options.hasTouch) {
        const small = await page.evaluate(() => {
          const out = [];
          for (const el of document.querySelectorAll('a[href], button, [role="tab"], summary, input, select, textarea')) {
            const r = el.getBoundingClientRect();
            const style = getComputedStyle(el);
            if (!r.width || !r.height || style.visibility === "hidden" || el.closest("[hidden], .sr, .skip")) continue;
            if (el.tagName === "A" && el.closest("p, li, td, figcaption, .under, .sub, .fineprint, .content")) continue;
            if (r.width < 44 - 0.5 || r.height < 44 - 0.5) out.push(`${el.tagName.toLowerCase()} "${(el.getAttribute("aria-label") || el.textContent).trim().slice(0, 30)}" ${Math.round(r.width)}x${Math.round(r.height)}`);
          }
          return out;
        });
        for (const s of small) failures.push(`${where}: small tap target: ${s}`);
      }

      // accessibility
      await page.addScriptTag({ content: axeSource });
      const violations = await page.evaluate(async () => {
        const result = await window.axe.run(document, { runOnly: ["wcag2a", "wcag2aa"] });
        return result.violations.filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id}: ${v.nodes.slice(0, 3).map((n) => n.target.join(" ")).join(", ")}`);
      });
      for (const v of violations) failures.push(`${where}: axe ${v}`);

      await page.screenshot({ path: path.join(out, `${pg.name}-${name}-${theme}.png`), fullPage: true });
      await page.close();
    }
    await context.close();
  }
}
await browser.close();
server.close();

if (failures.length) {
  console.error(failures.join("\n"));
  console.error(`\n${failures.length} problem(s); screenshots in ${out}`);
  process.exit(1);
}
console.log(`no problems on ${PAGES.length} pages at ${SIZES.length} sizes in both themes; screenshots in ${out}`);
