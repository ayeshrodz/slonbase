// Formatting helpers.

export function parseDate(value) {
  if (!value) return null;
  const d = new Date(String(value).replace(" ", "T"));
  return isNaN(d) ? null : d;
}

export function formatDate(value, withTime = true) {
  const d = parseDate(value);
  if (!d) return "";
  const opts = { year: "numeric", month: "short", day: "numeric" };
  if (withTime) Object.assign(opts, { hour: "2-digit", minute: "2-digit" });
  return d.toLocaleString(undefined, opts);
}

export function relativeTime(value) {
  const d = parseDate(value);
  if (!d) return "";
  const s = (Date.now() - d.getTime()) / 1000;
  const units = [
    [60, "second", 1],
    [3600, "minute", 60],
    [86400, "hour", 3600],
    [604800, "day", 86400],
    [2629800, "week", 604800],
    [31557600, "month", 2629800],
    [Infinity, "year", 31557600],
  ];
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [limit, unit, div] of units) {
    if (Math.abs(s) < limit) return rtf.format(-Math.round(s / div), unit);
  }
  return "";
}

export function bytes(n) {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return (n / Math.pow(1024, i)).toFixed(i ? 1 : 0) + " " + units[i];
}

export function duration(seconds) {
  if (!seconds) return "0s";
  const parts = [];
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (d) parts.push(d + "d");
  if (h) parts.push(h + "h");
  if (m) parts.push(m + "m");
  if (s) parts.push(s + "s");
  return parts.join(" ");
}

export function truncate(text, n = 80) {
  const s = String(text ?? "");
  return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

// The text of user-written HTML. Parsed into an inert document: nothing in it loads or runs
// (an element given innerHTML would load images, and run their handlers if the CSP let it).
export function stripHtml(html) {
  return new DOMParser().parseFromString(html || "", "text/html").body.textContent || "";
}

export function isImage(name) {
  return /\.(jpe?g|png|gif|webp|svg|avif|bmp)$/i.test(name || "");
}

export function initials(text) {
  const s = (text || "?").replace(/@.*/, "");
  const parts = s.split(/[._\-\s]+/).filter(Boolean);
  return ((parts[0] || "?")[0] + (parts[1] ? parts[1][0] : "")).toUpperCase();
}

export function plural(n, word) {
  return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
}

// a record's display label: its presentable fields, or a common name field, or the id
export function recordLabel(record, collection) {
  if (!record) return "";
  const fields = (collection && collection.fields) || [];
  const presentable = fields.filter((f) => f.presentable).map((f) => record[f.name]).filter((v) => v !== "" && v != null);
  if (presentable.length) return presentable.map(String).join(" · ");
  for (const key of ["name", "title", "email", "username", "label", "slug"]) {
    if (record[key]) return String(record[key]);
  }
  return record.id;
}
