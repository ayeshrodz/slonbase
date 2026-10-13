// Sign-in options of an auth collection: rule, password, OAuth2, OTP, MFA, tokens and emails.
import { html, useState, useEffect } from "../lib/core.js";
import { api } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Field, Input, TextArea, Switch, Modal, SheetHead, Select, Callout, Tabs } from "../lib/ui.js";
import { duration } from "../lib/format.js";
import { RuleCard } from "./rules.js";

function Section({ icon, title, subtitle, enabled, onToggle, children, open = true }) {
  return html`<div class="card">
    <div class="card-head">
      <span class="field-type-badge" style="background:var(--accent-soft);color:var(--accent)"><${Icon} name=${icon} size=${16} /></span>
      <div class="grow"><h3>${title}</h3>${subtitle && html`<div class="small muted">${subtitle}</div>`}</div>
      ${onToggle && html`<${Switch} checked=${enabled} onChange=${onToggle} label=${enabled ? "On" : "Off"} />`}
    </div>
    ${open && (enabled === undefined || enabled) && html`<div class="card-body stack enter">${children}</div>`}
  </div>`;
}

function Seconds({ label, value, onChange, help }) {
  return html`<${Field} label=${label} help=${help || (value ? duration(value) : "")}>
    <${Input} type="number" min="1" value=${value} onInput=${(v) => onChange(Number(v || 0))} />
  </${Field}>`;
}

function Template({ value, onChange, placeholders }) {
  return html`<div class="stack tight">
    <${Field} label="Subject"><${Input} value=${value.subject} onInput=${(v) => onChange({ ...value, subject: v })} /></${Field}>
    <${Field} label="Body (HTML)" help=${html`Placeholders: ${placeholders.map((p) => html`<code>{${p}}</code> `)}`}>
      <${TextArea} code rows=${7} value=${value.body} onInput=${(v) => onChange({ ...value, body: v })} />
    </${Field}>
  </div>`;
}

function ProviderSheet({ provider, meta, onSave, onRemove, onClose }) {
  const [p, setP] = useState({ ...provider });
  const oidc = p.name.startsWith("oidc");
  const set = (patch) => setP({ ...p, ...patch });
  return html`<${Modal} size="narrow" onClose=${onClose}>
    ${(close) => html`
      <${SheetHead} title=${meta ? meta.displayName : p.name} subtitle="OAuth2 provider" onClose=${close} />
      <div class="sheet-body stack">
        <${Field} label="Client ID"><${Input} value=${p.clientId} onInput=${(v) => set({ clientId: v })} autofocus /></${Field}>
        <${Field} label="Client secret" help="Stored encrypted; leave empty to keep the saved one.">
          <${Input} type="password" value=${p.clientSecret} onInput=${(v) => set({ clientSecret: v })} placeholder="••••••••" />
        </${Field}>
        ${oidc &&
        html`
          <${Field} label="Display name"><${Input} value=${p.displayName} onInput=${(v) => set({ displayName: v })} /></${Field}>
          <${Field} label="Authorization URL"><${Input} value=${p.authURL} onInput=${(v) => set({ authURL: v })} /></${Field}>
          <${Field} label="Token URL"><${Input} value=${p.tokenURL} onInput=${(v) => set({ tokenURL: v })} /></${Field}>
          <${Field} label="User info URL"><${Input} value=${p.userInfoURL} onInput=${(v) => set({ userInfoURL: v })} /></${Field}>
        `}
        <${Switch} checked=${p.pkce !== false} onChange=${(v) => set({ pkce: v })} label="Use PKCE" />
      </div>
      <div class="sheet-foot">
        ${onRemove && html`<${Button} variant="danger-ghost" icon="trash" onClick=${() => {
          onRemove();
          close();
        }}>Remove</${Button}>`}
        <span class="spacer"></span>
        <${Button} variant="ghost" onClick=${close}>Cancel</${Button}>
        <${Button} variant="primary" disabled=${!p.clientId} onClick=${() => {
          onSave(p);
          close();
        }}>Save provider</${Button}>
      </div>
    `}
  </${Modal}>`;
}

function OAuth2({ draft, setDraft }) {
  const [meta, setMeta] = useState([]);
  const [editing, setEditing] = useState(null);
  useEffect(() => {
    api.get("/api/collections/meta/oauth2-providers").then(setMeta).catch(() => {});
  }, []);
  const o = draft.oauth2;
  const set = (patch) => setDraft({ ...draft, oauth2: { ...o, ...patch } });
  const providers = o.providers || [];
  const textFields = draft.fields.filter((f) => ["text", "url", "file", "editor"].includes(f.type) && !f.system).map((f) => f.name);
  const logo = (m) => (m && m.logo ? html`<img alt="" width="22" height="22" src=${"data:image/svg+xml;utf8," + encodeURIComponent(m.logo)} />` : html`<${Icon} name="key" />`);
  return html`<${Section} icon="globe" title="OAuth2" subtitle="Sign in with Google, GitHub and other providers" enabled=${o.enabled} onToggle=${(v) => set({ enabled: v })}>
    <div class="type-grid">
      ${meta.map((m) => {
        const configured = providers.find((p) => p.name === m.name);
        return html`<button class="type-tile" onClick=${() => setEditing(configured || { name: m.name, clientId: "", clientSecret: "", authURL: "", tokenURL: "", userInfoURL: "", displayName: "", pkce: null, extra: {} })}>
          <span class="field-type-badge" style="background:var(--surface-2)">${logo(m)}</span>
          <span class="grow"><b>${m.displayName}</b><div class="small ${configured ? "" : "muted"}">${configured ? html`<span class="row" style="gap:5px;color:var(--ok)"><span class="dot ok"></span>Configured</span>` : "Not set up"}</div></span>
        </button>`;
      })}
    </div>
    <div class="section-title" style="margin-top:6px">Fill fields on sign-up</div>
    <div class="grid-3">
      ${[
        ["name", "Name"],
        ["username", "Username"],
        ["avatarURL", "Avatar"],
      ].map(
        ([key, label]) => html`<${Field} label=${label}>
          <${Select} value=${o.mappedFields[key]} onChange=${(v) => set({ mappedFields: { ...o.mappedFields, [key]: v } })} options=${[{ value: "", label: "—" }, ...textFields]} />
        </${Field}>`
      )}
    </div>
    ${editing &&
    html`<${ProviderSheet}
      provider=${editing}
      meta=${meta.find((m) => m.name === editing.name)}
      onClose=${() => setEditing(null)}
      onRemove=${providers.some((p) => p.name === editing.name) ? () => set({ providers: providers.filter((p) => p.name !== editing.name) }) : null}
      onSave=${(p) => set({ providers: providers.some((x) => x.name === p.name) ? providers.map((x) => (x.name === p.name ? p : x)) : [...providers, p] })}
    />`}
  </${Section}>`;
}

const TEMPLATES = [
  { key: "verificationTemplate", label: "Verification", vars: ["APP_NAME", "APP_URL", "TOKEN"] },
  { key: "resetPasswordTemplate", label: "Password reset", vars: ["APP_NAME", "APP_URL", "TOKEN"] },
  { key: "confirmEmailChangeTemplate", label: "Email change", vars: ["APP_NAME", "APP_URL", "TOKEN"] },
  { key: "otp", label: "One-time password", vars: ["APP_NAME", "APP_URL", "OTP"] },
];

export function AuthOptions({ draft, setDraft, errors }) {
  const set = (patch) => setDraft({ ...draft, ...patch });
  const [tpl, setTpl] = useState("verificationTemplate");
  const identity = draft.fields.filter((f) => ["email", "text"].includes(f.type) && f.name !== "id" && f.name !== "tokenKey").map((f) => f.name);
  const fields = draft.fields.map((f) => f.name).filter((n) => n !== "password" && n !== "tokenKey");
  const template = tpl === "otp" ? draft.otp.emailTemplate : draft[tpl];
  const setTemplate = (v) => (tpl === "otp" ? set({ otp: { ...draft.otp, emailTemplate: v } }) : set({ [tpl]: v }));
  const tokens = [
    ["authToken", "Auth token"],
    ["verificationToken", "Verification link"],
    ["passwordResetToken", "Password reset link"],
    ["emailChangeToken", "Email change link"],
    ["fileToken", "Protected file token"],
  ];
  return html`<div class="stack loose stagger" style="max-width:900px">
    <${RuleCard}
      rule=${{ key: "authRule", label: "Who can sign in", icon: "key", hint: "An extra condition on the record, e.g. verified = true." }}
      value=${draft.authRule}
      onChange=${(v) => set({ authRule: v })}
      fields=${fields}
      error=${errors.authRule}
      nullLabel="Nobody"
    />

    <${Section} icon="lock" title="Password" subtitle="Identity and password" enabled=${draft.passwordAuth.enabled} onToggle=${(v) => set({ passwordAuth: { ...draft.passwordAuth, enabled: v } })}>
      <${Field} label="Identity fields" help="Fields users can sign in with; they need a unique index.">
        <div class="chips">
          ${identity.map((n) => {
            const on = draft.passwordAuth.identityFields.includes(n);
            return html`<span class=${"chip selectable " + (on ? "accent" : "outline")} onClick=${() =>
              set({ passwordAuth: { ...draft.passwordAuth, identityFields: on ? draft.passwordAuth.identityFields.filter((x) => x !== n) : [...draft.passwordAuth.identityFields, n] } })}>
              ${on && html`<${Icon} name="check" size=${12} />`} ${n}
            </span>`;
          })}
        </div>
      </${Field}>
    </${Section}>

    <${OAuth2} draft=${draft} setDraft=${setDraft} />

    <${Section} icon="mail" title="One-time password" subtitle="A code sent by email" enabled=${draft.otp.enabled} onToggle=${(v) => set({ otp: { ...draft.otp, enabled: v } })}>
      <div class="grid-2">
        <${Seconds} label="Valid for (seconds)" value=${draft.otp.duration} onChange=${(v) => set({ otp: { ...draft.otp, duration: v } })} />
        <${Field} label="Digits"><${Input} type="number" min="4" max="12" value=${draft.otp.length} onInput=${(v) => set({ otp: { ...draft.otp, length: Number(v || 0) } })} /></${Field}>
      </div>
    </${Section}>

    <${Section} icon="shield" title="Multi-factor auth" subtitle="Require a second method after the first" enabled=${draft.mfa.enabled} onToggle=${(v) => set({ mfa: { ...draft.mfa, enabled: v } })}>
      <div class="grid-2">
        <${Seconds} label="Time to finish (seconds)" value=${draft.mfa.duration} onChange=${(v) => set({ mfa: { ...draft.mfa, duration: v } })} />
        <${Field} label="Only for records matching" help="Empty requires MFA for everyone"><${Input} class="mono" value=${draft.mfa.rule} onInput=${(v) => set({ mfa: { ...draft.mfa, rule: v } })} /></${Field}>
      </div>
      ${draft.mfa.enabled && !draft.otp.enabled && !draft.oauth2.enabled && html`<${Callout} kind="warn">MFA needs a second method: turn on OTP or OAuth2.</${Callout}>`}
    </${Section}>

    <${Section} icon="clock" title="Token lifetimes">
      <div class="grid-3">
        ${tokens.map(([key, label]) => html`<${Seconds} label=${label} value=${draft[key].duration} onChange=${(v) => set({ [key]: { duration: v } })} />`)}
      </div>
    </${Section}>

    <${Section} icon="mail" title="Emails">
      <${Tabs} tabs=${TEMPLATES.map((t) => ({ id: t.key, label: t.label }))} active=${tpl} onChange=${setTpl} />
      <${Template} key=${tpl} value=${template} onChange=${setTemplate} placeholders=${TEMPLATES.find((t) => t.key === tpl).vars} />
    </${Section}>
  </div>`;
}
