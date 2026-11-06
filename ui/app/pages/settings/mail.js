// Settings → Mail: the sender, the SMTP server, and the emails themselves: a layout around
// every email and each auth collection's templates, edited as HTML with a live preview of what
// would be sent (rendered by the server, shown in a sandboxed frame that runs nothing).
import { html, useState, useEffect, useRef, useStore, cx } from "../../lib/core.js";
import { api, enc, BASE } from "../../lib/api.js";
import { branding } from "../../lib/branding.js";
import { app, upsertCollection } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { Button, Field, Input, TextArea, Switch, Select, Segmented, Empty, toast } from "../../lib/ui.js";
import { Card } from "./shared.js";

const TEMPLATES = [
  { id: "verification", label: "Verification", path: ["verificationTemplate"], hint: "{TOKEN} goes in the link that verifies the address" },
  { id: "password-reset", label: "Password reset", path: ["resetPasswordTemplate"], hint: "{TOKEN} goes in the link to choose a new password" },
  { id: "email-change", label: "Email change", path: ["confirmEmailChangeTemplate"], hint: "{TOKEN} goes in the link that confirms the new address" },
  { id: "otp", label: "One-time password", path: ["otp", "emailTemplate"], hint: "Must contain {OTP}" },
  { id: "login-alert", label: "New sign-in alert", path: ["authAlert", "emailTemplate"], hint: "{ALERT_INFO} says when and from where" },
];

const PLACEHOLDERS = {
  common: ["{APP_NAME}", "{APP_URL}", "{APP_LOGO}", "{APP_LOGO_URL}", "{ACCENT}", "{RECORD:email}", "{RECORD:name}"],
  verification: ["{TOKEN}"],
  "password-reset": ["{TOKEN}"],
  "email-change": ["{TOKEN}"],
  otp: ["{OTP}", "{OTP_ID}"],
  "login-alert": ["{ALERT_INFO}"],
  layout: ["{CONTENT}"],
};

const get = (obj, path) => path.reduce((o, k) => (o ? o[k] : undefined), obj);
function setIn(obj, path, value) {
  const copy = JSON.parse(JSON.stringify(obj));
  let o = copy;
  for (const k of path.slice(0, -1)) o = o[k] = o[k] || {};
  o[path[path.length - 1]] = value;
  return copy;
}

// inserts text at the caret of the editor
function insertAt(ref, text, value, onChange) {
  const el = ref.current;
  if (!el) return onChange(value + text);
  const start = el.selectionStart ?? value.length;
  const end = el.selectionEnd ?? value.length;
  onChange(value.slice(0, start) + text + value.slice(end));
  requestAnimationFrame(() => {
    el.focus();
    el.selectionStart = el.selectionEnd = start + text.length;
  });
}

// what the server would send, refreshed shortly after each change
function usePreview(request) {
  const [preview, setPreview] = useState(null);
  const key = JSON.stringify(request);
  useEffect(() => {
    if (!request) return;
    let live = true;
    const t = setTimeout(() => {
      api
        .post("/api/settings/mail/preview", request)
        .then((p) => live && setPreview(p))
        .catch(() => {});
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [key]);
  return preview;
}

// the email's images from this dashboard's address (the dashboard allows no others; a real
// email uses the application URL's)
function local(htmlText) {
  const appURL = (branding.get().appURL || "").replace(/\/$/, "");
  if (!appURL) return htmlText;
  return htmlText.split(appURL + "/api/branding/assets/").join(location.origin + BASE + "/api/branding/assets/");
}

function Preview({ preview }) {
  const [width, setWidth] = useState("desktop");
  return html`<div class="mail-preview">
    <div class="mail-preview-bar">
      <span class="eyebrow">Preview</span>
      <span class="spacer"></span>
      <${Segmented} value=${width} onChange=${setWidth} options=${[{ value: "desktop", label: "Desktop" }, { value: "mobile", label: "Phone" }]} />
    </div>
    ${preview
      ? html`<div class="mail-preview-subject"><span class="tiny muted">Subject</span> <b>${preview.subject || "(no subject)"}</b></div>
          <div class=${cx("mail-preview-frame", width)}>
            <iframe title="Email preview" sandbox="" srcdoc=${local(preview.html)} referrerpolicy="no-referrer"></iframe>
          </div>`
      : html`<div class="mail-preview-frame"><div class="empty small muted">Rendering…</div></div>`}
    <div class="tiny muted">Sample values: a token TEST_TOKEN, a password 123456; links point to the application URL.</div>
  </div>`;
}

function TemplateEditor({ collection, template }) {
  const t = TEMPLATES.find((x) => x.id === template);
  const saved = get(collection, t.path) || { subject: "", body: "" };
  const [draft, setDraft] = useState(saved);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [testTo, setTestTo] = useState("");
  const [sending, setSending] = useState(false);
  const ref = useRef();
  useEffect(() => setDraft(get(collection, t.path) || { subject: "", body: "" }), [collection.id, template]);
  const preview = usePreview({ collection: collection.name, template, subject: draft.subject, body: draft.body });
  const dirty = draft.subject !== saved.subject || draft.body !== saved.body;
  const save = async () => {
    setBusy(true);
    setError("");
    try {
      const next = setIn(collection, t.path, draft);
      const res = await api.patch(`/api/collections/${enc(collection.id)}`, next);
      upsertCollection(res);
      toast("Template saved.");
    } catch (err) {
      const fe = err.fieldErrors ? err.fieldErrors() : {};
      setError(Object.values(fe)[0] || err.message);
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  const test = async () => {
    setSending(true);
    try {
      await api.post("/api/settings/test/email", { email: testTo, template, collection: collection.name });
      toast(`Sent to ${testTo}.`);
    } catch (err) {
      toast.error(err);
    } finally {
      setSending(false);
    }
  };
  return html`<div class="mail-editor">
    <div class="stack">
      <div class="row wrap tight">
        <b>${collection.name}</b><span class="muted">·</span><span>${t.label}</span>
        ${dirty && html`<span class="chip warn">unsaved</span>`}
      </div>
      <${Field} label="Subject"><${Input} value=${draft.subject} onInput=${(v) => setDraft({ ...draft, subject: v })} /></${Field}>
      <${Field} label="Body (HTML)" help=${t.hint} error=${error}>
        <${TextArea} code rows=${16} inputRef=${ref} value=${draft.body} onInput=${(v) => setDraft({ ...draft, body: v })} spellcheck="false" />
      </${Field}>
      <div class="placeholder-chips">
        <span class="tiny muted">Insert:</span>
        ${[...PLACEHOLDERS[template], ...PLACEHOLDERS.common].map(
          (p) => html`<button type="button" class="chip outline mono selectable" onClick=${() => insertAt(ref, p, draft.body, (body) => setDraft({ ...draft, body }))}>${p}</button>`
        )}
      </div>
      <div class="row">
        <${Button} size="sm" variant="ghost" icon="undo" disabled=${!dirty} onClick=${() => setDraft(saved)}>Discard</${Button}>
        <span class="spacer"></span>
        <${Button} size="sm" variant="primary" icon="check" disabled=${!dirty} loading=${busy} onClick=${save}>Save template</${Button}>
      </div>
      <div class="row top mail-test">
        <${Field} class="grow" label="Send a test to"><${Input} type="email" value=${testTo} onInput=${setTestTo} placeholder="you@example.com" /></${Field}>
        <div class="label-pad"><${Button} size="sm" icon="mail" loading=${sending} disabled=${!testTo || dirty} onClick=${test}>Send test</${Button}></div>
      </div>
      ${dirty && html`<div class="tiny muted">Tests use the saved template: save first.</div>`}
    </div>
    <${Preview} preview=${preview} />
  </div>`;
}

function LayoutEditor({ s }) {
  const layout = s.draft.mailLayout;
  const ref = useRef();
  const collections = useStore(app, (st) => st.collections).filter((c) => c.type === "auth");
  const sample = collections.find((c) => c.name !== "_superusers") || collections[0];
  const preview = usePreview(sample ? { collection: sample.name, template: "verification", layout } : null);
  const changed = s.changed("mailLayout");
  return html`<div class="mail-editor">
    <div class="stack">
      <div class="row wrap tight"><b>Layout</b><span class="muted">· around every email</span>${changed && html`<span class="chip warn">unsaved</span>`}</div>
      <${Switch} checked=${layout.enabled} onChange=${(v) => s.set("mailLayout.enabled", v)} label="Put every email in this layout (emails that are whole documents of their own aren't)" />
      <${Field} label="Layout (HTML)" error=${s.errors["mailLayout.html"]} help="{CONTENT} is where each email's body goes. Use inline styles and tables: email clients ignore most CSS.">
        <${TextArea} code rows=${18} inputRef=${ref} value=${layout.html} onInput=${(v) => s.set("mailLayout.html", v)} spellcheck="false" />
      </${Field}>
      <div class="placeholder-chips">
        <span class="tiny muted">Insert:</span>
        ${[...PLACEHOLDERS.layout, ...PLACEHOLDERS.common.slice(0, 5)].map(
          (p) => html`<button type="button" class="chip outline mono selectable" onClick=${() => insertAt(ref, p, layout.html, (v) => s.set("mailLayout.html", v))}>${p}</button>`
        )}
      </div>
      <div class="small muted">Saved with the settings (the bar below). The logo comes from Settings → Branding, the color from the accent.</div>
    </div>
    <${Preview} preview=${preview} />
  </div>`;
}

function Designer({ s }) {
  const collections = useStore(app, (st) => st.collections).filter((c) => c.type === "auth");
  const [selected, setSelected] = useState({ kind: "layout" });
  const current = selected.kind === "template" && collections.find((c) => c.id === selected.collection);
  return html`<${Card} title="Emails" subtitle="The layout around every email, and each auth collection's messages" icon="edit" class="mail-designer-card">
    <div class="mail-designer">
      <nav class="mail-list" aria-label="Emails">
        <button type="button" class=${cx("mail-item", { on: selected.kind === "layout" })} onClick=${() => setSelected({ kind: "layout" })}>
          <${Icon} name="layers" size=${15} /><span>Layout</span>
        </button>
        ${collections.map(
          (c) => html`<div class="mail-group">
            <div class="mail-group-label">${c.name}</div>
            ${TEMPLATES.map(
              (t) => html`<button
                type="button"
                class=${cx("mail-item", { on: current && current.id === c.id && selected.template === t.id })}
                onClick=${() => setSelected({ kind: "template", collection: c.id, template: t.id })}
              ><${Icon} name="mail" size=${15} /><span>${t.label}</span></button>`
            )}
          </div>`
        )}
      </nav>
      <div class="mail-main">
        ${selected.kind === "layout"
          ? html`<${LayoutEditor} s=${s} />`
          : current
          ? html`<${TemplateEditor} key=${current.id + selected.template} collection=${current} template=${selected.template} />`
          : html`<${Empty} icon="mail" title="Choose an email" />`}
      </div>
    </div>
  </${Card}>`;
}

export function Mail({ s }) {
  const d = s.draft;
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
    <${Designer} s=${s} />
  </div>`;
}
