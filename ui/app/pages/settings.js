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
    label: "Delivery",
    items: [
      { id: "mail", label: "Mail", icon: "mail", sections: ["smtp"] },
      { id: "storage", label: "File storage", icon: "cloud", sections: ["s3"] },
    ],
  },
  {
    label: "Operations",
    items: [
      { id: "backups", label: "Backups", icon: "archive", sections: ["backups"] },
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
];
const SECTIONS = GROUPS.flatMap((g) => g.items);

function Mail({ s }) {
  const d = s.draft;
  const [testTo, setTestTo] = useState("");
  const [template, setTemplate] = useState("verification");
  const [sending, setSending] = useState(false);
  const test = async () => {
    setSending(true);
    try {
      await api.post("/api/settings/test/email", { email: testTo, template });
      toast(`Test email sent to ${testTo}.`);
    } catch (err) {
      toast.error(err);
    } finally {
      setSending(false);
    }
  };
  return html`<div class="stack loose">
    <${Card} title="Sender" icon="mail">
      <div class="grid-2">
        <${Field} label="Sender name"><${Input} value=${d.meta.senderName} onInput=${(v) => s.set("meta.senderName", v)} /></${Field}>
        <${Field} label="Sender address" error=${s.errors["meta.senderAddress"]}><${Input} type="email" value=${d.meta.senderAddress} onInput=${(v) => s.set("meta.senderAddress", v)} /></${Field}>
      </div>
    </${Card}>
    <${Card} title="SMTP server" subtitle=${d.smtp.enabled ? "Emails go through your server" : "Off: emails are written to the server log"} icon="server" actions=${html`<${Switch} checked=${d.smtp.enabled} onChange=${(v) => s.set("smtp.enabled", v)} label=${d.smtp.enabled ? "On" : "Off"} />`}>
      ${d.smtp.enabled &&
      html`<div class="stack enter">
        <div class="grid-3" style="grid-template-columns:2fr 1fr 1fr">
          <${Field} label="Host" error=${s.errors["smtp.host"]}><${Input} value=${d.smtp.host} onInput=${(v) => s.set("smtp.host", v)} /></${Field}>
          <${Field} label="Port" error=${s.errors["smtp.port"]}><${Input} type="number" value=${d.smtp.port} onInput=${(v) => s.set("smtp.port", Number(v))} /></${Field}>
          <${Field} label="Encryption">
            <${Select} value=${d.smtp.tls ? "tls" : "starttls"} onChange=${(v) => s.set("smtp.tls", v === "tls")} options=${[{ value: "starttls", label: "STARTTLS" }, { value: "tls", label: "TLS" }]} />
          </${Field}>
        </div>
        <div class="grid-2">
          <${Field} label="Username"><${Input} value=${d.smtp.username} onInput=${(v) => s.set("smtp.username", v)} autocomplete="off" /></${Field}>
          <${Field} label="Password" help="Leave empty to keep the saved one"><${Input} type="password" value=${d.smtp.password} onInput=${(v) => s.set("smtp.password", v)} autocomplete="new-password" placeholder="••••••••" /></${Field}>
        </div>
        <div class="grid-2">
          <${Field} label="Auth method"><${Select} value=${d.smtp.authMethod} onChange=${(v) => s.set("smtp.authMethod", v)} options=${[{ value: "", label: "Automatic" }, "PLAIN", "LOGIN"]} /></${Field}>
          <${Field} label="HELO name" help="Optional"><${Input} value=${d.smtp.localName} onInput=${(v) => s.set("smtp.localName", v)} /></${Field}>
        </div>
      </div>`}
    </${Card}>
    <${Card} title="Send a test email" icon="arrowRight">
      <div class="row wrap top">
        <${Field} class="grow" label="To"><${Input} type="email" value=${testTo} onInput=${setTestTo} placeholder="you@example.com" /></${Field}>
        <${Field} label="Template">
          <${Select} value=${template} onChange=${setTemplate} options=${[{ value: "verification", label: "Verification" }, { value: "password-reset", label: "Password reset" }, { value: "email-change", label: "Email change" }, { value: "otp", label: "One-time password" }, { value: "login-alert", label: "New device alert" }]} />
        </${Field}>
      </div>
      <div class="row"><span class="small muted grow">Uses the saved settings — save first.</span><${Button} icon="mail" loading=${sending} disabled=${!testTo || s.dirty} onClick=${test}>Send test</${Button}></div>
    </${Card}>
  </div>`;
}

export function Settings({ section }) {
  const s = useSettings();
  const active = SECTIONS.find((x) => x.id === section) || SECTIONS[0];
  const needsDraft = !["crons", "transfer"].includes(active.id);
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
              ${active.id === "transfer" && html`<${Transfer} />`}
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
