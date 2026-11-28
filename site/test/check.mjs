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
// the project page as it is when a release candidate is newer than the release: the deploy would
// fill it this way (scripts/build-site.mjs), here from fixed releases
const { fill } = await import(path.join(here, "../../scripts/build-site.mjs"));
const { pick } = await import(path.join(here, "../../site/release.js"));
{
  const rel = (tag) => ({
    tag_name: tag, draft: false, prerelease: tag.includes("-rc."), published_at: "2026-12-01T12:00:00Z",
    html_url: `https://github.com/ayeshrodz/slonbase/releases/tag/${tag}`,
    body: "**A lead.** A summary of the release.\n\n### Added\n\n- **One thing.** x\n- **Another.** y\n- **A third.** z\n",
    assets: [],
  });
  const page = fs.readFileSync(path.join(dir, "index.html"), "utf8");
  fs.writeFileSync(path.join(dir, "preview.html"), fill(page, pick([rel("v9.1.0-rc.1"), rel("v9.0.0")])));
}

const PAGES = [
  { name: "home", path: "/" },
  { name: "home-with-preview", path: "/preview.html" },
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

// A link inside a sentence is exempt from the tap-target size: its paragraph, list item or cell
// holds words besides the text of its links. A row of links alone ("One Two", "A · B") is not a
// sentence, so each of those links is measured. (Runs in the page: no outside references.)
function inRunningText(a) {
  const block = a.closest("p, li, td, dd, figcaption");
  if (!block) return false;
  const copy = block.cloneNode(true);
  copy.querySelectorAll("a, button").forEach((link) => link.remove());
  return /[\p{L}\p{N}]{2}/u.test(copy.textContent);
}

const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const failures = [];

// the install box opens on the visitor's system, and Preview switches every command to the candidate
{
  for (const [ua, tab] of [["Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)", "t-mac"], ["Mozilla/5.0 (Windows NT 10.0; Win64; x64)", "t-win"], ["Mozilla/5.0 (X11; Linux x86_64)", "t-linux"]]) {
    const context = await browser.newContext({ userAgent: ua });
    await context.route(/api\.github\.com/, (route) => route.abort());
    const page = await context.newPage();
    await page.goto(base + "/", { waitUntil: "networkidle" });
    const selected = await page.evaluate(() => document.querySelector('#install [role="tab"][aria-selected="true"]')?.id);
    if (selected !== tab) failures.push(`install box: on "${ua}" it opens ${selected}, not ${tab}`);
    await context.close();
  }
  const context = await browser.newContext();
  await context.route(/api\.github\.com/, (route) => route.abort());
  const page = await context.newPage();
  await page.goto(base + "/preview.html", { waitUntil: "networkidle" });
  const shown = () => page.evaluate(() => document.querySelector("#in-bin").innerText);
  if (!(await shown()).includes("v9.0.0")) failures.push("preview page: the binary tab doesn't start on the release");
  await page.click('[data-channel-pick="preview"]');
  if (!(await shown()).includes("v9.1.0-rc.1")) failures.push("preview page: Preview doesn't switch the binary tab to the candidate");
  if (!(await page.evaluate(() => document.querySelector("#in-apt").innerText)).includes("./slonbase_9.1.0-rc.1_amd64.deb")) failures.push("preview page: Preview doesn't give the candidate's package file");
  if (!(await page.evaluate(() => document.querySelector("#in-linux").innerText)).includes("SLONBASE_PACKAGE")) failures.push("preview page: Preview doesn't switch the Linux tab to the candidate's package");
  if (!(await page.locator(".release.preview").count())) failures.push("preview page: no Preview card");
  await context.close();

  // between deploys the page asks GitHub itself, with the same rules; it keeps what it has on failure
  const api = (body, status = 200) => async (route) => {
    if (/\/releases\?/.test(route.request().url())) await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    else await route.abort();
  };
  const rel = (tag) => ({ tag_name: tag, draft: false, prerelease: tag.includes("-rc."), published_at: "2027-01-05T12:00:00Z", html_url: `https://github.com/ayeshrodz/slonbase/releases/tag/${tag}`, body: "**Fresh.** From the API.", assets: [] });
  for (const [name, handler, want] of [
    ["a newer candidate", api([rel("v9.3.0-rc.1"), rel("v9.2.0")]), "Preview v9.3.0-rc.1"],
    ["an API error", api({ message: "rate limited" }, 403), null],
  ]) {
    const ctx = await browser.newContext();
    await ctx.route(/api\.github\.com/, handler);
    const pg = await ctx.newPage();
    const before = (await (await fetch(base + "/")).text()).match(/<a class="pill"[^>]*>([\s\S]*?)<\/a>/)[1].replace(/<!--[\s\S]*?-->/g, "");
    await pg.goto(base + "/", { waitUntil: "networkidle" });
    const pill = await pg.evaluate(() => document.getElementById("release-pill").innerHTML);
    if (want && !pill.includes(want)) failures.push(`runtime refresh, ${name}: the pill doesn't show ${want}`);
    if (!want && pill.replace(/<!--[\s\S]*?-->/g, "") !== before) failures.push(`runtime refresh, ${name}: the pill changed`);
    await ctx.close();
  }
}

// the exemption itself: links in a sentence are exempt, links standing alone or side by side are not
{
  const page = await browser.newPage();
  await page.setContent(`
    <p id="s"><a href="/a">a link</a> inside a sentence.</p>
    <ul><li id="one"><a href="/one">One</a></li>
        <li id="two"><a href="/one">One</a> <a href="/two">Two</a></li>
        <li id="sep"><a href="/one">One</a> · <a href="/two">Two</a>,</li>
        <li id="desc"><a href="/x">Quick install</a>: one command</li></ul>
    <table><tr><td id="cell"><a href="/c">Only a link</a></td></tr></table>`);
  const got = await page.evaluate((src) => {
    const f = new Function(`return ${src}`)();
    return Object.fromEntries(["s", "one", "two", "sep", "desc", "cell"].map((id) => [id, f(document.querySelector(`#${id} a`))]));
  }, inRunningText.toString());
  const want = { s: true, one: false, two: false, sep: false, desc: true, cell: false };
  for (const [id, v] of Object.entries(want)) {
    if (got[id] !== v) failures.push(`self-test: the link in #${id} should ${v ? "" : "not "}count as running text`);
  }
  await page.close();
}
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
      // (the blocked requests to GitHub's API are expected; any other failure is reported)
      page.on("console", (m) => {
        if (m.type() !== "error") return;
        if (/^https:\/\/api\.github\.com\//.test(m.location().url || "")) return;
        errors.push(m.text());
      });
      page.on("requestfailed", (r) => { if (!/^https:\/\/api\.github\.com\//.test(r.url())) errors.push(`request failed: ${r.url()} ${r.failure()?.errorText || ""}`); });
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

      // every button and link can be clicked: nothing (a glow, an overlay) lies on top of it
      const covered = await page.evaluate(() => {
        const out = [];
        for (const el of document.querySelectorAll("button, a[href]")) {
          if (!el.getClientRects().length || el.closest("[hidden]") || getComputedStyle(el).visibility === "hidden") continue;
          el.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });
          const r = el.getBoundingClientRect();
          const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
          if (hit && !el.contains(hit) && !hit.contains(el)) out.push(`${el.tagName.toLowerCase()} "${(el.getAttribute("aria-label") || el.textContent).trim().slice(0, 30)}" under ${hit.tagName.toLowerCase()}${hit.className && typeof hit.className === "string" ? "." + hit.className.trim().split(/\s+/).join(".") : ""}`);
        }
        scrollTo({ top: 0, behavior: "instant" });
        return out;
      });
      for (const c of covered) failures.push(`${where}: ${c}`);

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
        const small = await page.evaluate((runningTextSource) => {
          const inRunningText = new Function(`return ${runningTextSource}`)();
          const out = [];
          for (const el of document.querySelectorAll('a[href], button, [role="tab"], summary, input, select, textarea')) {
            const r = el.getBoundingClientRect();
            const style = getComputedStyle(el);
            if (!r.width || !r.height || style.visibility === "hidden" || el.closest("[hidden], .sr, .skip")) continue;
            if (el.tagName === "A" && inRunningText(el)) continue;
            if (r.width < 44 - 0.5 || r.height < 44 - 0.5) out.push(`${el.tagName.toLowerCase()} "${(el.getAttribute("aria-label") || el.textContent).trim().slice(0, 30)}" ${Math.round(r.width)}x${Math.round(r.height)}`);
          }
          return out;
        }, inRunningText.toString());
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
