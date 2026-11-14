// Settings: application, mail, file storage, backups, security, logs, crons, import/export.
import { html, useState, useEffect, useMemo, useStore, navigate, cx } from "../lib/core.js";
import { api, qs, enc, getFileToken, BASE } from "../lib/api.js";
import { app, loadCollections } from "../lib/state.js";
import { Icon } from "../lib/icons.js";
import {
  Button,
  IconButton,
  Field,
  Input,
  TextArea,
  Switch,
  Select,
  Modal,
  SheetHead,
  Skeleton,
  Empty,
  Callout,
  Segmented,
  toast,
  confirmDialog,
} from "../lib/ui.js";
import { bytes, formatDate, relativeTime, plural } from "../lib/format.js";
import { Card, useSettings } from "./settings/shared.js";
import { Backups, Crons } from "./settings/operations.js";
import { Storage } from "./settings/storage.js";
import { Security, LogsSettings } from "./settings/security.js";
import { Transfer } from "./settings/transfer.js";
import { Extensions } from "./settings/extensions.js";
import { Mail } from "./settings/mail.js";
import { Team, AppUsers } from "./settings/team.js";
import { Application } from "./settings/application.js";
import { Branding } from "./settings/branding.js";
import { LoginScreen } from "./settings/login-screen.js";

// grouped in the side navigation; the ids are linked to from elsewhere (Health's checks, the
// command palette), so they stay
const GROUPS = [
  {
    label: "Workspace",
    items: [
      { id: "general", label: "Application", icon: "sparkle", sections: ["meta"] },
      { id: "branding", label: "Branding", icon: "image", sections: ["meta"] },
      { id: "login", label: "Login screen", icon: "lock", sections: ["login"] },
    ],
  },
  {
    label: "People",
    items: [
      { id: "users", label: "Team", icon: "users" },
      { id: "app-users", label: "App users", icon: "user" },
    ],
  },
  {
    label: "Delivery",
    items: [
      { id: "mail", label: "Mail", icon: "mail", sections: ["smtp", "mailLayout"] },
      { id: "storage", label: "File storage", icon: "cloud", sections: ["s3"] },
    ],
  },
  {
    label: "Operations",
    items: [
      { id: "backups", label: "Backups", icon: "archive", sections: ["backups", "schedule"] },
      { id: "crons", label: "Cron jobs", icon: "clock" },
      { id: "logs", label: "Logs", icon: "logs", sections: ["logs"] },
    ],
  },
  {
    label: "Protection",
    items: [{ id: "security", label: "Security & limits", icon: "shield", sections: ["rateLimits", "superuserIPs", "trustedProxy", "batch", "security"] }],
  },
  {
    label: "Data",
    items: [{ id: "transfer", label: "Import & export", icon: "layers" }],
  },
  {
    label: "Developers",
    items: [{ id: "extensions", label: "Extensions", icon: "code" }],
  },
];
const SECTIONS = GROUPS.flatMap((g) => g.items);

export function Settings({ section }) {
  const s = useSettings();
  const active = SECTIONS.find((x) => x.id === section) || SECTIONS[0];
  const needsDraft = !["crons", "transfer", "users", "app-users", "extensions"].includes(active.id);
  return html`<div class="page enter">
    <div class="page-head"><div class="grow"><div class="eyebrow"><${Icon} name="settings" size=${14} /> Configuration</div><h1>Settings</h1></div></div>
    <div class="settings-layout">
      <nav class="settings-nav" aria-label="Settings">
        ${GROUPS.map(
          (g) => html`<div class="settings-group">
            <div class="settings-group-label">${g.label}</div>
            ${g.items.map(
              (x) => html`<a href=${"#/settings/" + x.id} class=${cx({ on: x.id === active.id })} aria-current=${x.id === active.id ? "page" : undefined}>
                <${Icon} name=${x.icon} size=${17} /><span class="grow">${x.label}</span>${(x.sections || []).some((k) => s.changed(k)) && x.id !== "branding" && html`<span class="unsaved-dot" title="Unsaved changes"></span>`}
              </a>`
            )}
          </div>`
        )}
      </nav>
      <div key=${active.id}>
        ${needsDraft && !s.draft
          ? html`<div class="stack"><${Skeleton} height=${160} /><${Skeleton} height=${120} /></div>`
          : html`
              ${active.id === "general" && html`<${Application} s=${s} />`}
              ${active.id === "branding" && html`<${Branding} s=${s} />`}
              ${active.id === "login" && html`<${LoginScreen} s=${s} />`}
              ${active.id === "mail" && html`<${Mail} s=${s} />`}
              ${active.id === "storage" && html`<${Storage} s=${s} />`}
              ${active.id === "backups" && html`<${Backups} s=${s} />`}
              ${active.id === "security" && html`<${Security} s=${s} />`}
              ${active.id === "logs" && html`<${LogsSettings} s=${s} />`}
              ${active.id === "crons" && html`<${Crons} />`}
              ${active.id === "users" && html`<${Team} />`}
              ${active.id === "app-users" && html`<${AppUsers} />`}
              ${active.id === "transfer" && html`<${Transfer} />`}
              ${active.id === "extensions" && html`<${Extensions} />`}
            `}
        ${s.dirty &&
        html`<div class="savebar">
          <${Icon} name="edit" size=${16} />
          <span class="grow">Unsaved settings</span>
          <${Button} variant="ghost" size="sm" icon="undo" onClick=${s.reset}>Discard</${Button}>
          <${Button} variant="primary" size="sm" loading=${s.saving} onClick=${() => s.save()}>Save settings</${Button}>
        </div>`}
      </div>
    </div>
  </div>`;
}
