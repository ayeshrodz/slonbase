// API rules: null (superusers only), "" (anyone) or a filter expression.
import { html, useRef } from "../lib/core.js";
import { Icon } from "../lib/icons.js";
import { Button, TextArea, Segmented, Callout } from "../lib/ui.js";

const RULES = [
  { key: "listRule", label: "List & search", icon: "list", hint: "Which records appear in lists; the rule is added to every list filter." },
  { key: "viewRule", label: "View", icon: "eye", hint: "Who can fetch a single record." },
  { key: "createRule", label: "Create", icon: "plus", hint: "Checked against the new record; @request.body holds the submitted data." },
  { key: "updateRule", label: "Update", icon: "edit", hint: "Checked against the current record before the update." },
  { key: "deleteRule", label: "Delete", icon: "trash", hint: "Who can delete a record." },
];

const SNIPPETS = [
  { label: "signed in", text: "@request.auth.id != ''" },
  { label: "owner", text: "user = @request.auth.id" },
  { label: "verified", text: "@request.auth.verified = true" },
  { label: "and", text: " && " },
  { label: "or", text: " || " },
];

export function RuleCard({ rule, value, onChange, fields, error, nullLabel = "Superusers only", allowNull = true }) {
  const ref = useRef();
  const mode = value === null ? "locked" : value.trim() === "" ? "public" : "custom";
  const insert = (text) => {
    const el = ref.current && ref.current.base ? ref.current.base : document.getElementById("rule-" + rule.key);
    const current = value || "";
    if (el && typeof el.selectionStart === "number") {
      const start = el.selectionStart;
      const next = current.slice(0, start) + text + current.slice(el.selectionEnd);
      onChange(next);
      setTimeout(() => {
        el.focus();
        el.setSelectionRange(start + text.length, start + text.length);
      });
    } else {
      onChange(current + text);
    }
  };
  return html`<div class="card rule-card stack tight">
    <div class="row">
      <span class="field-type-badge"><${Icon} name=${rule.icon} size=${16} /></span>
      <div class="grow"><b>${rule.label}</b><div class="small muted">${rule.hint}</div></div>
      <${Segmented}
        value=${mode}
        onChange=${(m) => onChange(m === "locked" ? null : m === "public" ? "" : value || "@request.auth.id != ''")}
        options=${[
          allowNull && { value: "locked", label: html`<${Icon} name="lock" size=${12} /> ${nullLabel}` },
          { value: "public", label: "Everyone" },
          { value: "custom", label: "Custom" },
        ].filter(Boolean)}
      />
    </div>
    ${mode === "locked" && html`<div class="rule-locked enter"><${Icon} name="lock" size=${15} /> Only superusers can ${rule.label.toLowerCase()} records.</div>`}
    ${mode === "public" && html`<div class="rule-locked enter" style="border-color:color-mix(in srgb,var(--warn),transparent 50%)"><${Icon} name="unlock" size=${15} /> Anyone, including guests.</div>`}
    ${mode === "custom" &&
    html`<div class="stack tight enter">
      <${TextArea} id=${"rule-" + rule.key} code rows=${2} class="rule-editor" value=${value} invalid=${!!error} onInput=${onChange} placeholder="@request.auth.id != ''" />
      ${error && html`<span class="small" style="color:var(--danger)">${error}</span>`}
      <div class="chips">
        ${SNIPPETS.map((s) => html`<span class="chip selectable outline" onClick=${() => insert(s.text)}>${s.label}</span>`)}
        ${fields.map((f) => html`<span class="chip selectable" onClick=${() => insert(f)}><code>${f}</code></span>`)}
      </div>
    </div>`}
  </div>`;
}

export function Rules({ draft, setDraft, errors }) {
  const fields = draft.fields.map((f) => f.name).filter((n) => n !== "password" && n !== "tokenKey");
  const set = (key, value) => setDraft({ ...draft, [key]: value });
  return html`<div class="stack" style="max-width:900px">
    <${Callout}>
      Rules are filters like <code>status = 'published' || author = @request.auth.id</code>. They can use the record's
      fields, <code>@request.auth.*</code>, <code>@request.body.*</code>, <code>@request.query.*</code>,
      <code>@request.headers.*</code> and <code>@now</code>. Superusers bypass every rule.
    </${Callout}>
    <div class="stack tight stagger">
      ${RULES.filter((r) => draft.type !== "view" || r.key === "listRule" || r.key === "viewRule").map(
        (r, i) => html`<div style=${{ "--i": i }}>
          <${RuleCard} rule=${r} value=${draft[r.key]} onChange=${(v) => set(r.key, v)} fields=${fields} error=${errors[r.key]} />
        </div>`
      )}
      ${draft.type === "auth" &&
      html`<${RuleCard}
        rule=${{ key: "manageRule", label: "Manage", icon: "shield", hint: "Who can change other records' email, password and verified state without their password." }}
        value=${draft.manageRule}
        onChange=${(v) => set("manageRule", v)}
        fields=${fields}
        error=${errors.manageRule}
        nullLabel="Nobody"
      />`}
    </div>
  </div>`;
}
