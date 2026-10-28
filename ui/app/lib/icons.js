// slonbase's icon set: 24px grid, 1.7 stroke, rounded joins.
import { html } from "./core.js";

const paths = {
  overview: `<rect x="3.5" y="3.5" width="7" height="9" rx="2"/><rect x="13.5" y="3.5" width="7" height="5" rx="2"/><rect x="13.5" y="11.5" width="7" height="9" rx="2"/><rect x="3.5" y="15.5" width="7" height="5" rx="2"/>`,
  data: `<ellipse cx="12" cy="5.5" rx="7.5" ry="2.8"/><path d="M4.5 5.5v6.5c0 1.5 3.4 2.8 7.5 2.8s7.5-1.3 7.5-2.8V5.5"/><path d="M4.5 12v6.5c0 1.5 3.4 2.8 7.5 2.8s7.5-1.3 7.5-2.8V12"/>`,
  logs: `<path d="M3 12h3.5l2.5-6 4 12 2.5-6H21"/>`,
  settings: `<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>`,
  terminal: `<rect x="3" y="4.5" width="18" height="15" rx="3"/><path d="m7.5 9.5 3 2.5-3 2.5M12.5 15h4"/>`,
  search: `<circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/>`,
  plus: `<path d="M12 5v14M5 12h14"/>`,
  minus: `<path d="M5 12h14"/>`,
  close: `<path d="m6.5 6.5 11 11M17.5 6.5l-11 11"/>`,
  check: `<path d="m5 12.5 4.5 4.5L19 7.5"/>`,
  chevronRight: `<path d="m9.5 6 6 6-6 6"/>`,
  chevronLeft: `<path d="m14.5 6-6 6 6 6"/>`,
  chevronDown: `<path d="m6 9.5 6 6 6-6"/>`,
  chevronUp: `<path d="m6 14.5 6-6 6 6"/>`,
  more: `<circle cx="5.5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="18.5" cy="12" r="1.2"/>`,
  trash: `<path d="M4.5 7h15M10 4h4M6.5 7l.8 11.2A2 2 0 0 0 9.3 20h5.4a2 2 0 0 0 2-1.8L17.5 7M10 11v5M14 11v5"/>`,
  edit: `<path d="M14.5 5.5 18.5 9.5M4.5 19.5l1-4.5L15.8 4.7a1.8 1.8 0 0 1 2.5 0l1 1a1.8 1.8 0 0 1 0 2.5L9 18.5z"/>`,
  copy: `<rect x="8.5" y="8.5" width="11" height="11" rx="2.5"/><path d="M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>`,
  refresh: `<path d="M19.5 12a7.5 7.5 0 0 1-13.2 4.9M4.5 12a7.5 7.5 0 0 1 13.2-4.9"/><path d="M18 3.5v4h-4M6 20.5v-4h4"/>`,
  filter: `<path d="M4 6h16M7 12h10M10 18h4"/>`,
  sortAsc: `<path d="M7 18V6M4 9l3-3 3 3M14 7h6M14 12h4M14 17h2"/>`,
  sortDesc: `<path d="M7 6v12M4 15l3 3 3-3M14 7h2M14 12h4M14 17h6"/>`,
  eye: `<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.8"/>`,
  eyeOff: `<path d="M4 4l16 16M9.9 6A9 9 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.6 3.3M6.5 7.5A15 15 0 0 0 2.5 12S6 18.5 12 18.5a9 9 0 0 0 4.2-1"/><path d="M9.9 10a2.8 2.8 0 0 0 4 4"/>`,
  lock: `<rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>`,
  unlock: `<rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8 10.5V8a4 4 0 0 1 7.7-1.5"/>`,
  key: `<circle cx="8" cy="15" r="4.5"/><path d="m11.2 11.8 8.3-8.3M16.5 6.5l2.5 2.5M14 9l2 2"/>`,
  user: `<circle cx="12" cy="8.5" r="4"/><path d="M4.5 20c1-3.8 4-5.5 7.5-5.5s6.5 1.7 7.5 5.5"/>`,
  users: `<circle cx="9" cy="8.5" r="3.5"/><path d="M2.5 19.5c.8-3.3 3.3-5 6.5-5s5.7 1.7 6.5 5"/><path d="M15.5 5.3a3.5 3.5 0 0 1 0 6.4M17.5 14.7c2 .6 3.4 2.2 4 4.8"/>`,
  shield: `<path d="M12 3.5 19 6v5.5c0 4.4-3 7.8-7 9-4-1.2-7-4.6-7-9V6z"/><path d="m9 12 2.2 2.2L15.5 10"/>`,
  mail: `<rect x="3" y="5.5" width="18" height="13" rx="2.5"/><path d="m4 7.5 8 6 8-6"/>`,
  bell: `<path d="M6.5 16.5V11a5.5 5.5 0 0 1 11 0v5.5l1.5 2h-14z"/><path d="M10 20.5a2 2 0 0 0 4 0"/>`,
  cloud: `<path d="M7 18.5a4.5 4.5 0 0 1-.6-9 6 6 0 0 1 11.4 1.4A3.8 3.8 0 0 1 17.5 18.5z"/>`,
  archive: `<rect x="3.5" y="4.5" width="17" height="4.5" rx="1.5"/><path d="M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9M10 13h4"/>`,
  clock: `<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>`,
  upload: `<path d="M12 15.5V4M7.5 8.5 12 4l4.5 4.5M4.5 15v2.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15"/>`,
  download: `<path d="M12 4v11.5M7.5 11 12 15.5l4.5-4.5M4.5 15v2.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15"/>`,
  file: `<path d="M13.5 3.5H7.5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2V8.5z"/><path d="M13.5 3.5v5h5"/>`,
  image: `<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><circle cx="9" cy="10" r="1.8"/><path d="m4 17.5 5-4.5 4 3.5 2.5-2 4.5 4"/>`,
  link: `<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>`,
  hash: `<path d="M9.5 4 8 20M16 4l-1.5 16M4.5 9h16M3.5 15h16"/>`,
  text: `<path d="M5 6.5V5h14v1.5M12 5v14M9 19h6"/>`,
  toggle: `<rect x="2.5" y="7" width="19" height="10" rx="5"/><circle cx="16.5" cy="12" r="3"/>`,
  calendar: `<rect x="3.5" y="5" width="17" height="15.5" rx="2.5"/><path d="M3.5 10h17M8 3v4M16 3v4"/>`,
  at: `<circle cx="12" cy="12" r="3.5"/><path d="M15.5 12v1.5a2.5 2.5 0 0 0 5 0V12a8.5 8.5 0 1 0-3.4 6.8"/>`,
  globe: `<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.3 2.4 3.5 5.2 3.5 8.5s-1.2 6.1-3.5 8.5c-2.3-2.4-3.5-5.2-3.5-8.5S9.7 5.9 12 3.5z"/>`,
  list: `<path d="M9 6.5h11M9 12h11M9 17.5h11"/><circle cx="4.8" cy="6.5" r="1"/><circle cx="4.8" cy="12" r="1"/><circle cx="4.8" cy="17.5" r="1"/>`,
  braces: `<path d="M8 4.5c-2 0-2.5 1-2.5 2.5v2.5c0 1.2-.8 2.5-2 2.5 1.2 0 2 1.3 2 2.5V17c0 1.5.5 2.5 2.5 2.5M16 4.5c2 0 2.5 1 2.5 2.5v2.5c0 1.2.8 2.5 2 2.5-1.2 0-2 1.3-2 2.5V17c0 1.5-.5 2.5-2.5 2.5"/>`,
  relation: `<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><circle cx="18" cy="6" r="2.5"/><path d="M8.5 6h7M6 8.5V13a5 5 0 0 0 5 5h4.5"/>`,
  asterisk: `<path d="M12 4.5v15M5.5 8.2l13 7.6M18.5 8.2l-13 7.6"/>`,
  autodate: `<circle cx="12" cy="13" r="7.5"/><path d="M12 9.5V13l2.5 1.5M9.5 2.8h5"/>`,
  editor: `<path d="M4 6h16M4 10h16M4 14h10M4 18h7"/>`,
  sun: `<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/>`,
  moon: `<path d="M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10z"/>`,
  logout: `<path d="M14 4.5H7a2 2 0 0 0-2 2v11a2 2 0 0 0 2 2h7M10.5 12H21M17.5 8.5 21 12l-3.5 3.5"/>`,
  external: `<path d="M14 4.5h5.5V10M19.5 4.5 11 13M17.5 14v4a2 2 0 0 1-2 2H6.5a2 2 0 0 1-2-2V8.5a2 2 0 0 1 2-2h4"/>`,
  grip: `<circle cx="9" cy="6.5" r="1.1"/><circle cx="15" cy="6.5" r="1.1"/><circle cx="9" cy="12" r="1.1"/><circle cx="15" cy="12" r="1.1"/><circle cx="9" cy="17.5" r="1.1"/><circle cx="15" cy="17.5" r="1.1"/>`,
  warning: `<path d="M10.3 4.6 2.9 17.5A2 2 0 0 0 4.6 20.5h14.8a2 2 0 0 0 1.7-3L13.7 4.6a2 2 0 0 0-3.4 0z"/><path d="M12 9.5v4M12 17h.01"/>`,
  info: `<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5M12 7.8h.01"/>`,
  zap: `<path d="M13 3 5 13.5h6.5L10.5 21 19 10.5h-6.5z"/>`,
  server: `<rect x="3.5" y="4" width="17" height="7" rx="2"/><rect x="3.5" y="13" width="17" height="7" rx="2"/><path d="M7.5 7.5h.01M7.5 16.5h.01"/>`,
  play: `<path d="M7.5 5.2v13.6a.8.8 0 0 0 1.2.7l11-6.8a.8.8 0 0 0 0-1.4l-11-6.8a.8.8 0 0 0-1.2.7z"/>`,
  code: `<path d="m8.5 7.5-5 4.5 5 4.5M15.5 7.5l5 4.5-5 4.5M13.5 5l-3 14"/>`,
  layers: `<path d="m12 3.5 8.5 4.5-8.5 4.5L3.5 8z"/><path d="m3.5 12 8.5 4.5 8.5-4.5M3.5 16l8.5 4.5 8.5-4.5"/>`,
  pin: `<path d="M12 21s-6.5-6.2-6.5-11.5a6.5 6.5 0 0 1 13 0C18.5 14.8 12 21 12 21z"/><circle cx="12" cy="9.5" r="2.3"/>`,
  table: `<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M3.5 9.5h17M3.5 14.5h17M9.5 9.5v10"/>`,
  columns: `<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M9.2 4.5v15M14.8 4.5v15"/>`,
  chart: `<path d="M4 20V10M10 20V4M16 20v-7M22 20H2"/>`,
  sparkle: `<path d="M12 3c.8 4.3 2.7 6.2 7 7-4.3.8-6.2 2.7-7 7-.8-4.3-2.7-6.2-7-7 4.3-.8 6.2-2.7 7-7zM19 16c.3 1.5 1 2.2 2.5 2.5-1.5.3-2.2 1-2.5 2.5-.3-1.5-1-2.2-2.5-2.5 1.5-.3 2.2-1 2.5-2.5z"/>`,
  view: `<path d="M3.5 7.5 12 3.5l8.5 4v9L12 20.5l-8.5-4z"/><path d="M3.5 7.5 12 11.5l8.5-4M12 11.5v9"/>`,
  command: `<path d="M9 9V6.5A2.5 2.5 0 1 0 6.5 9H9zm0 0h6m-6 0v6m6-6V6.5A2.5 2.5 0 1 1 17.5 9H15zm0 0v6m0 0v2.5a2.5 2.5 0 1 0 2.5-2.5H15zm0 0H9m0 0v2.5A2.5 2.5 0 1 1 6.5 15H9z"/>`,
  dot: `<circle cx="12" cy="12" r="3.5"/>`,
  arrowRight: `<path d="M4.5 12h15M14 6.5l5.5 5.5-5.5 5.5"/>`,
  expand: `<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>`,
  undo: `<path d="M9 14.5 4.5 10 9 5.5"/><path d="M4.5 10H15a5 5 0 0 1 0 10h-3"/>`,
};

export function Icon({ name, size = 18, class: cls = "", title }) {
  const body = paths[name] || paths.dot;
  return html`<svg
    class=${"icon " + cls}
    width=${size}
    height=${size}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden=${title ? undefined : "true"}
    role=${title ? "img" : undefined}
    dangerouslySetInnerHTML=${{ __html: (title ? `<title>${title}</title>` : "") + body }}
  ></svg>`;
}

// the slonbase mark: a monochrome elephant, drawn in the current ink color
export function Logo({ size = 28 }) {
  return html`<svg class="logo" width=${size} height=${size} viewBox="0 0 40 40" aria-label="slonbase">
    <rect x="1" y="1" width="38" height="38" rx="10" fill="var(--ink)" />
    <g fill="var(--canvas)">
      <ellipse cx="11.6" cy="17" rx="5.6" ry="6.6" opacity=".55" />
      <ellipse cx="28.4" cy="17" rx="5.6" ry="6.6" opacity=".55" />
      <circle cx="20" cy="16" r="7.4" />
    </g>
    <path d="M20 19.5v6.8c0 2.3 1.6 3.6 3.4 3.6 1.4 0 2.4-.9 2.6-2.2" fill="none" stroke="var(--canvas)" stroke-width="4" stroke-linecap="round" stroke-linejoin="round" />
    <circle cx="17.3" cy="15.2" r="1.15" fill="var(--ink)" />
    <circle cx="22.7" cy="15.2" r="1.15" fill="var(--ink)" />
  </svg>`;
}

// field type metadata: icon, label, description
export const FIELD_TYPES = [
  { type: "text", tone: 1, icon: "text", label: "Text", hint: "Plain text, with length and pattern checks" },
  { type: "editor", tone: 1, icon: "editor", label: "Rich text", hint: "Long-form HTML content" },
  { type: "number", tone: 3, icon: "hash", label: "Number", hint: "Integers or decimals with bounds" },
  { type: "bool", tone: 2, icon: "toggle", label: "Boolean", hint: "A yes / no switch" },
  { type: "email", tone: 8, icon: "at", label: "Email", hint: "An email address, with domain rules" },
  { type: "url", tone: 8, icon: "globe", label: "URL", hint: "A web address, with domain rules" },
  { type: "date", tone: 4, icon: "calendar", label: "Date", hint: "A date and time" },
  { type: "autodate", tone: 4, icon: "autodate", label: "Autodate", hint: "Set on create and/or update" },
  { type: "select", tone: 6, icon: "list", label: "Select", hint: "One or more predefined values" },
  { type: "file", tone: 5, icon: "image", label: "File", hint: "Uploads, with thumbnails for images" },
  { type: "relation", tone: 7, icon: "relation", label: "Relation", hint: "Links to records of a collection" },
  { type: "json", tone: 2, icon: "braces", label: "JSON", hint: "Any JSON value" },
  { type: "geoPoint", tone: 3, icon: "pin", label: "Geo point", hint: "Longitude and latitude, filterable by distance" },
  { type: "password", tone: 5, icon: "asterisk", label: "Password", hint: "A hashed secret, never returned" },
];

export function fieldMeta(type) {
  return FIELD_TYPES.find((t) => t.type === type) || { type, icon: "dot", label: type, hint: "" };
}
