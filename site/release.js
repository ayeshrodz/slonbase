// The releases on the project page: which to show, what they say and how to install them.
// One module for both places that need it: scripts/build-site.mjs fills the page with it when the
// site is deployed (after every release), and the page itself uses it to refresh from GitHub's
// API between deploys. No DOM here: functions of release data that return text and HTML.

export const REPO = "ayeshrodz/slonbase";
const SITE = "https://slonbase.com";
const DOWNLOAD = `https://github.com/${REPO}/releases/download`;

/** [major, minor, patch, rc] for "v2.7.1" or "2.8.0-rc.1" (rc 0 for a release), or null. */
export function version(tag) {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-rc\.(\d+))?$/.exec(String(tag || ""));
  return m ? [+m[1], +m[2], +m[3], m[4] ? +m[4] : 0] : null;
}

/** Orders versions; a release comes after its candidates (2.8.0-rc.2 < 2.8.0). */
export function compare(a, b) {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] - b[i];
  if (!a[3] !== !b[3]) return a[3] ? -1 : 1;
  return a[3] - b[3];
}

/**
 * The newest release, and the newest candidate when it is newer than that release (an older
 * candidate, a draft, or a tag that isn't a version is never shown).
 */
export function pick(releases) {
  let stable = null;
  let preview = null;
  for (const r of releases || []) {
    const v = version(r.tag_name);
    if (!v || r.draft) continue;
    if (!v[3] && !r.prerelease) {
      if (!stable || compare(v, version(stable.tag_name)) > 0) stable = r;
    } else if (v[3]) {
      if (!preview || compare(v, version(preview.tag_name)) > 0) preview = r;
    }
  }
  if (preview && stable && compare(version(preview.tag_name), version(stable.tag_name)) <= 0) preview = null;
  if (!stable) return { stable: preview, preview: null };
  return { stable, preview };
}

/**
 * What a release says, from its notes: the bold lead of the first paragraph, the rest of that
 * paragraph, and the bold headlines under "Added" (the changelog's own layout).
 */
export function notes(body) {
  body = String(body || "");
  const first = body.split(/\n\s*\n/)[0] || "";
  const lead = ((first.match(/^\*\*([^*]+?)\*\*/) || [])[1] || "").replace(/[:.]$/, "");
  const summary = first.replace(/^\*\*[^*]+\*\*\s*/, "").replace(/\s+/g, " ").trim();
  const added = (body.split(/^### Added\s*$/m)[1] || "").split(/^### /m)[0];
  const items = [];
  for (const chunk of added.split(/^- /m)) {
    const m = chunk.match(/^\*\*([^*]+?)\*\*/);
    if (m) items.push(m[1].replace(/[:.]$/, ""));
  }
  return { lead, summary, items };
}

export function escape(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
}

/** Markdown-ish inline text (code, bold, links) to safe HTML: code kept, the rest plain. */
function inline(s) {
  return escape(s).replace(/`([^`]+)`/g, "<code>$1</code>").replace(/\*\*([^*]+)\*\*/g, "$1").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1");
}

function longDate(iso) {
  if (!iso) return "";
  return new Date(iso).toLocaleDateString("en-US", { year: "numeric", month: "long", day: "numeric", timeZone: "UTC" });
}

const cm = (s) => `<span class="cm">${escape(s)}</span>`;

/**
 * The install commands of one release, per tab, as HTML. Built from the files the release really
 * has: a command for an archive it lacks says so and points to the guide.
 */
export function commands(rel, { candidate = false } = {}) {
  const tag = rel.tag_name;
  const v = tag.replace(/^v/, "");
  const assets = new Set((rel.assets || []).map((a) => a.name));
  const has = (name) => assets.size === 0 || assets.has(name);
  const sums = has("SHA256SUMS");
  const base = `${DOWNLOAD}/${tag}`;
  const lines = (...l) => l.filter((x) => x !== null).join("\n");
  const out = {};
  const missing = (what) => cm(`# ${tag} has no ${what}: see ${SITE}/docs/installation/ for the other ways`);
  const clone = `git clone --branch ${tag} --depth 1 https://github.com/${REPO}`;
  const deb = `slonbase_${v}_amd64.deb`;
  const rpm = `slonbase-${v}-1.x86_64.rpm`;

  // the one-command install: the repository's package, or a candidate's own package file
  // (one command per package the candidate has: Debian/Ubuntu's, else Fedora/RHEL's)
  const viaInstaller = (file, note) => lines(
    cm(`# ${tag}, a release candidate: download its package${note}, and the installer`),
    cm("# sets everything up with it instead of the repository's"),
    escape(`curl -fsSLO ${base}/${file}`),
    escape(`curl -fsSL ${SITE}/install.sh | sudo SLONBASE_PACKAGE="$PWD/${file}" sh`),
  );
  out.linux = candidate
    ? (has(deb) ? viaInstaller(deb, has(rpm) ? ` (Debian/Ubuntu shown; on Fedora/RHEL use ${rpm})` : " (Debian/Ubuntu)")
      : has(rpm) ? viaInstaller(rpm, " (Fedora/RHEL)")
      : missing("Linux packages"))
    : lines(
      cm("# Debian, Ubuntu, Fedora, RHEL: installs PostgreSQL if needed, creates its role and database,"),
      cm("# installs slonbase and creates your first superuser"),
      escape(`curl -fsSL ${SITE}/install.sh | sudo sh`),
    );

  const tgz = `slonbase-${v}-linux-amd64.tar.gz`;
  out.bin = !has(tgz) ? missing("Linux archive") : lines(
    candidate ? cm(`# ${tag}: a release candidate, to try on a copy of your installation`) : null,
    escape(`curl -fsSLO ${base}/${tgz}`),
    sums ? escape(`curl -fsSLO ${base}/SHA256SUMS && grep linux-amd64.tar.gz SHA256SUMS | sha256sum -c`) : null,
    escape(`tar xzf ${tgz}`),
    `./slonbase serve    ${cm("# prints the setup link")}`,
  );

  const mac = `slonbase-${v}-macos-arm64.tar.gz`;
  out.mac = has(mac) && sums
    ? lines(
      cm("# macOS on Apple silicon (experimental, not signed): check the download, see the guide"),
      escape(`curl -fsSLO ${base}/${mac}`),
      escape(`curl -fsSLO ${base}/SHA256SUMS`),
      escape("grep macos-arm64.tar.gz SHA256SUMS | shasum -a 256 -c"),
      escape(`tar xzf ${mac}`),
      "./slonbase serve",
    )
    : lines(
      cm(`# ${tag} has no macOS archive: use Docker, or build from source (Rust 1.89+)`),
      escape(`${clone} && cd slonbase`),
      escape("cargo build --release --locked && ./target/release/slonbase serve"),
      cm(`# ${SITE}/docs/installation/#macos`),
    );

  const win = `slonbase-${v}-windows-amd64.zip`;
  out.win = has(win) && sums
    ? lines(
      cm("# Windows x64 (experimental, not signed): check the download, see the guide"),
      escape(`$base = "${base}"`),
      escape(`Invoke-WebRequest "$base/${win}" -OutFile slonbase.zip`),
      escape('Invoke-WebRequest "$base/SHA256SUMS" -OutFile SHA256SUMS'),
      escape("Select-String windows-amd64.zip SHA256SUMS"),
      escape("(Get-FileHash slonbase.zip -Algorithm SHA256).Hash"),
      escape("Expand-Archive slonbase.zip -DestinationPath slonbase"),
      escape(".\\slonbase\\slonbase.exe serve"),
    )
    : lines(
      cm(`# ${tag} has no Windows archive: use Docker, or build from source (Rust 1.89+)`),
      escape(`${clone}; cd slonbase`),
      escape("cargo build --release --locked; .\\target\\release\\slonbase.exe serve"),
      cm(`# ${SITE}/docs/installation/#windows`),
    );

  out.docker = lines(
    escape("mkdir slonbase && cd slonbase"),
    escape(`curl -fsSLO https://raw.githubusercontent.com/${REPO}/main/compose.yaml`),
    escape('echo "POSTGRES_PASSWORD=$(openssl rand -hex 24)" > .env'),
    candidate ? escape(`echo "SLONBASE_VERSION=${v}" >> .env`) + "   " + cm("# the candidate's image") : null,
    escape("docker compose up -d"),
    escape('docker compose logs slonbase | grep "setup?token"'),
  );

  // packages: the repositories carry releases only; a candidate is installed from its file
  if (candidate) {
    out.apt = !has(deb) ? missing("Debian package") : lines(
      cm("# candidates aren't in the apt repository: install the package file over your version"),
      escape(`curl -fsSLO ${base}/${deb}`),
      escape(`sudo apt install ./${deb}`),
    );
    out.dnf = !has(rpm) ? missing("RPM package") : lines(
      cm("# candidates aren't in the dnf repository: install the package file over your version"),
      escape(`curl -fsSLO ${base}/${rpm}`),
      escape(`sudo dnf install ./${rpm}`),
    );
  } else {
    out.apt = lines(
      escape(`curl -fsSL ${SITE}/slonbase.gpg | sudo tee /usr/share/keyrings/slonbase.gpg > /dev/null`),
      escape(`curl -fsSL ${SITE}/slonbase.list | sudo tee /etc/apt/sources.list.d/slonbase.list`),
      escape("sudo apt update && sudo apt install slonbase"),
      escape('sudo journalctl -u slonbase | grep "setup?token"'),
    );
    out.dnf = lines(
      escape(`sudo curl -fsSL -o /etc/yum.repos.d/slonbase.repo ${SITE}/slonbase.repo`),
      escape("sudo dnf install slonbase"),
      escape('sudo journalctl -u slonbase | grep "setup?token"'),
    );
  }
  return out;
}

/**
 * The page's release regions as HTML, by region name (the elements with data-region="..."):
 * the pill above the title, the release cards, the Stable/Preview switch and the install
 * commands that depend on the version.
 */
export function regions({ stable, preview }) {
  const s = notes(stable.body);
  const r = {};
  r.pill =
    `<b>${escape(stable.tag_name)}</b><span class="pill-title">${escape(s.lead || "Release notes")}</span>` +
    (preview ? `<span class="pill-pre">Preview ${escape(preview.tag_name)}</span>` : "") +
    `<span class="arrow" aria-hidden="true">→</span>`;
  r.pillHref = preview ? `https://github.com/${REPO}/releases` : stable.html_url;

  const card = (rel, kind) => {
    const n = notes(rel.body);
    const items = n.items.length >= 3 ? n.items.slice(0, 6) : [];
    return `<div class="release${kind === "preview" ? " preview" : ""}">
      <div class="rel-head">
        <div class="kind">${kind === "preview" ? "Preview: a release candidate" : "Latest release"}</div>
        <div class="ver">${escape(rel.tag_name)}</div>
        <div class="date">${escape(longDate(rel.published_at))}</div>
        <p>${inline(n.summary || n.lead)}</p>
        <a class="btn" href="${escape(rel.html_url)}">Release notes →</a>
      </div>
      ${items.length ? `<ul class="rel-list">${items.map((t) => `<li>${inline(t)}</li>`).join("")}</ul>` : `<div class="rel-list"><p>${kind === "preview" ? "Try it on a copy of a real installation first: it is never installed by updates on its own." : ""}</p></div>`}
    </div>`;
  };
  r.release = card(stable, "stable") + (preview ? card(preview, "preview") : "");

  r.channel = preview
    ? `<div class="channel" role="group" aria-label="Release">
        <button type="button" data-channel-pick="stable" aria-pressed="true">Stable ${escape(stable.tag_name)}</button>
        <button type="button" data-channel-pick="preview" aria-pressed="false">Preview ${escape(preview.tag_name)}</button>
      </div>`
    : "";

  const sc = commands(stable);
  const pc = preview ? commands(preview, { candidate: true }) : null;
  for (const tab of ["linux", "bin", "mac", "win", "docker", "apt", "dnf"]) {
    r[`cmd-${tab}`] = pc
      ? `<span data-channel="stable">${sc[tab]}</span><span data-channel="preview" hidden>${pc[tab]}</span>`
      : sc[tab];
  }
  return r;
}

/** The structured data that names the current version (JSON-LD), for search engines. */
export function softwareFacts({ stable }) {
  const v = stable.tag_name.replace(/^v/, "");
  const assets = new Set((stable.assets || []).map((a) => a.name));
  // (no asset list, as when built offline from CHANGELOG.md: a release has every archive)
  const has = (name) => assets.size === 0 || assets.has(name);
  const os = ["Linux"];
  if (has(`slonbase-${v}-macos-arm64.tar.gz`)) os.push("macOS");
  if (has(`slonbase-${v}-windows-amd64.zip`)) os.push("Windows");
  return { softwareVersion: v, downloadUrl: stable.html_url, operatingSystem: os.join(", ") };
}
