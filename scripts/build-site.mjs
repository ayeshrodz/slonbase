#!/usr/bin/env node
// Fills the project page's release regions from the releases themselves, so a release (or a
// release candidate) needs no edit of the page. Run when the site is deployed, which every
// release triggers (.github/workflows/repo.yml):
//
//   node scripts/build-site.mjs <index.html> [--offline]
//   node scripts/build-site.mjs --test
//
// It reads the releases from GitHub's API (GITHUB_TOKEN if set) and picks the newest release,
// and the newest release candidate when it is newer (site/release.js decides, the same code the
// page runs to refresh itself). Offline, or when the API can't be reached, the release comes from
// Cargo.toml and its section of CHANGELOG.md. Each region is a `<!-- @site:NAME -->` ...
// `<!-- @site:end -->` pair in the page; one that is missing fails the build.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { REPO, pick, regions, softwareFacts, notes, version, compare, commands } from "../site/release.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REGIONS = ["pill", "release", "channel", "cmd-linux", "cmd-bin", "cmd-mac", "cmd-win", "cmd-docker", "cmd-apt", "cmd-dnf"];

/** The release Cargo.toml names, as the API would describe it, from its CHANGELOG section. */
export function localRelease() {
  const v = /^version\s*=\s*"([^"]+)"/m.exec(fs.readFileSync(path.join(ROOT, "Cargo.toml"), "utf8"))[1];
  const log = fs.readFileSync(path.join(ROOT, "CHANGELOG.md"), "utf8");
  const head = new RegExp(`^## \\[${v.replace(/\./g, "\\.")}\\] - (\\d{4}-\\d{2}-\\d{2})\\s*$`, "m").exec(log);
  let body = "";
  let date = null;
  if (head) {
    date = head[1];
    const rest = log.slice(head.index + head[0].length);
    body = rest.slice(0, rest.search(/^## \[/m) === -1 ? undefined : rest.search(/^## \[/m)).trim();
  }
  return {
    tag_name: `v${v}`,
    prerelease: v.includes("-rc."),
    draft: false,
    published_at: date ? `${date}T12:00:00Z` : null,
    html_url: `https://github.com/${REPO}/releases/tag/v${v}`,
    body,
    assets: [],
  };
}

async function fetchReleases() {
  const headers = { accept: "application/vnd.github+json", "user-agent": "slonbase-site-build" };
  if (process.env.GITHUB_TOKEN) headers.authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=30`, { headers, signal: AbortSignal.timeout(15000) });
  if (!res.ok) throw new Error(`GitHub API: ${res.status}`);
  return res.json();
}

/** The page with its regions, pill link and structured data filled for these releases. */
export function fill(html, chosen) {
  const r = regions(chosen);
  for (const name of REGIONS) {
    const re = new RegExp(`(<!-- @site:${name} -->)[\\s\\S]*?(<!-- @site:end -->)`);
    if (!re.test(html)) throw new Error(`the page has no @site:${name} region`);
    html = html.replace(re, (_, open, close) => `${open}${r[name]}${close}`);
  }
  const pill = /(<a class="pill" id="release-pill"[^>]*? href=")[^"]*(")/;
  if (!pill.test(html)) throw new Error("the page has no release pill");
  html = html.replace(pill, `$1${r.pillHref}$2`);
  // the structured data of the application: its current version and where to get it
  html = html.replace(/<script type="application\/ld\+json">(\{[^<]*?"@type":\s*"SoftwareApplication"[^<]*?)<\/script>/, (whole, json) => {
    const data = JSON.parse(json);
    Object.assign(data, softwareFacts(chosen));
    return `<script type="application/ld+json">${JSON.stringify(data).replace(/</g, "\\u003c")}</script>`;
  });
  return html;
}

function test() {
  const rel = (tag, extra = {}) => ({
    tag_name: tag, draft: false, prerelease: tag.includes("-rc."), published_at: "2026-11-27T12:00:00Z",
    html_url: `https://github.com/${REPO}/releases/tag/${tag}`,
    body: "**A lead.** A summary.\n\n### Added\n\n- **One.** x\n- **Two.** y\n- **Three.** z\n",
    assets: ["SHA256SUMS", `slonbase-${tag.slice(1)}-linux-amd64.tar.gz`, `slonbase-${tag.slice(1)}-macos-arm64.tar.gz`, `slonbase_${tag.slice(1)}_amd64.deb`, `slonbase-${tag.slice(1)}-1.x86_64.rpm`].map((name) => ({ name })),
    ...extra,
  });
  const cases = [
    ["stable only", [rel("v2.7.1"), rel("v2.7.0")], "v2.7.1", null],
    ["a newer candidate", [rel("v2.8.0-rc.1"), rel("v2.7.1")], "v2.7.1", "v2.8.0-rc.1"],
    ["an older candidate is hidden", [rel("v2.7.1"), rel("v2.7.0-rc.4")], "v2.7.1", null],
    ["the newest of several candidates", [rel("v2.8.0-rc.1"), rel("v2.8.0-rc.2"), rel("v2.7.1")], "v2.7.1", "v2.8.0-rc.2"],
    ["the release after its candidates", [rel("v2.8.0"), rel("v2.8.0-rc.2"), rel("v2.7.1")], "v2.8.0", null],
    ["drafts are ignored", [rel("v2.9.0", { draft: true }), rel("v2.8.0-rc.1", { draft: true }), rel("v2.7.1")], "v2.7.1", null],
    ["a backport isn't the newest", [rel("v2.6.5"), rel("v2.7.1")], "v2.7.1", null],
    ["not a version", [rel("nightly"), rel("v2.7.1")], "v2.7.1", null],
  ];
  let failed = 0;
  for (const [name, list, stable, preview] of cases) {
    const got = pick(list);
    const ok = got.stable?.tag_name === stable && (got.preview?.tag_name ?? null) === preview;
    if (!ok) failed++;
    console.log(`${ok ? "ok  " : "FAIL"} ${name}: ${got.stable?.tag_name} / ${got.preview?.tag_name ?? "-"}`);
  }
  const check = (ok, what) => { if (!ok) failed++; console.log(`${ok ? "ok  " : "FAIL"} ${what}`); };
  check(compare(version("2.8.0-rc.2"), version("2.8.0")) < 0 && compare(version("2.8.0-rc.2"), version("2.8.0-rc.10")) < 0, "versions order");
  const n = notes(rel("v2.7.1").body);
  check(n.lead === "A lead" && n.summary === "A summary." && n.items.join() === "One,Two,Three", "notes are read");
  // the page itself fills, with and without a candidate, and its regions survive a second fill
  const page = fs.readFileSync(path.join(ROOT, "site/index.html"), "utf8");
  const one = fill(page, pick([rel("v2.7.1")]));
  const two = fill(one, pick([rel("v2.8.0-rc.1"), rel("v2.7.1")]));
  check(one.includes("<!-- @site:channel --><!-- @site:end -->") && one.includes("v2.7.1"), "without a candidate: no switch");
  check(two.includes('data-channel-pick="preview"') && two.includes("Preview v2.8.0-rc.1") && two.includes("SLONBASE_VERSION=2.8.0-rc.1"), "with a candidate: the switch, the pill and its commands");
  check(/"softwareVersion":"2.7.1"/.test(two) && /"operatingSystem":"Linux, macOS"/.test(two), "structured data: version and systems");
  check(fill(two, pick([rel("v2.7.1")])) === one, "filling again gives the same page");
  check(!/<script>[^]*?<\/script>/.test(regions(pick([rel("v2.7.1", { body: "**<script>x</script>.** <img onerror=x>" })])).pill), "release text is escaped");
  const cand = commands(rel("v2.8.0-rc.1"), { candidate: true });
  check(cand.linux.includes("SLONBASE_PACKAGE=&quot;$PWD/slonbase_2.8.0-rc.1_amd64.deb&quot;") && cand.linux.includes("releases/download/v2.8.0-rc.1/slonbase_2.8.0-rc.1_amd64.deb"), "a candidate's Linux command installs its own package");
  const bare = commands(rel("v2.8.0-rc.1", { assets: [{ name: "SHA256SUMS" }] }), { candidate: true });
  check(["linux", "bin", "apt", "dnf"].every((t) => !bare[t].includes("curl -fsSLO")) && bare.bin.includes("has no Linux archive"), "a candidate without Linux files gets no download of them");
  check(bare.win.includes("git clone --branch v2.8.0-rc.1 --depth 1") && bare.mac.includes("git clone --branch v2.8.0-rc.1"), "the source fallback builds the release's own tag");
  const debOnly = commands(rel("v2.8.0-rc.1", { assets: [{ name: "SHA256SUMS" }, { name: "slonbase_2.8.0-rc.1_amd64.deb" }] }), { candidate: true });
  const rpmOnly = commands(rel("v2.8.0-rc.1", { assets: [{ name: "SHA256SUMS" }, { name: "slonbase-2.8.0-rc.1-1.x86_64.rpm" }] }), { candidate: true });
  check(debOnly.linux.includes("SLONBASE_PACKAGE=&quot;$PWD/slonbase_2.8.0-rc.1_amd64.deb") && !debOnly.linux.includes(".rpm") && rpmOnly.linux.includes("SLONBASE_PACKAGE=&quot;$PWD/slonbase-2.8.0-rc.1-1.x86_64.rpm"), "a candidate with one Linux package gets that one");
  const local = localRelease();
  check(version(local.tag_name) !== null && notes(local.body).lead.length > 0, `offline: ${local.tag_name} from Cargo.toml and CHANGELOG.md`);
  if (failed) {
    console.error(`${failed} failed`);
    process.exit(1);
  }
}

const main = process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);
if (main && process.argv.includes("--test")) {
  test();
} else if (main) {
  const file = process.argv[2];
  if (!file || file.startsWith("--")) {
    console.error("usage: node scripts/build-site.mjs <index.html> [--offline] | --test");
    process.exit(2);
  }
  let releases = null;
  if (!process.argv.includes("--offline")) {
    try {
      releases = await fetchReleases();
    } catch (e) {
      console.warn(`releases from GitHub unavailable (${e.message}): using Cargo.toml and CHANGELOG.md`);
    }
  }
  const chosen = releases ? pick(releases) : null;
  const final = chosen && chosen.stable ? chosen : { stable: localRelease(), preview: null };
  fs.writeFileSync(file, fill(fs.readFileSync(file, "utf8"), final));
  console.log(`site: ${final.stable.tag_name}${final.preview ? `, preview ${final.preview.tag_name}` : ""}`);
}
