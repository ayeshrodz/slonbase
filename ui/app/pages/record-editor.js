// The record sheet: every field, auth options, and actions.
import { html, useState, useEffect } from "../lib/core.js";
import { api, enc, qs } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Field, Input, Switch, Modal, SheetHead, Skeleton, Menu, Callout, CopyButton, toast, confirmDialog } from "../lib/ui.js";
import { formatDate, relativeTime, recordLabel } from "../lib/format.js";
import { FieldInput } from "./field-inputs.js";

// a record to start a new one from (Duplicate)
let pendingCopy = null;

const AUTH_SPECIAL = ["email", "emailVisibility", "verified", "password", "tokenKey"];

function blank(c) {
  const r = {};
  for (const f of c.fields) {
    if (f.type === "bool") r[f.name] = false;
    else if (f.type === "number") r[f.name] = 0;
    else if (f.type === "json") r[f.name] = null;
    else if ((f.type === "select" || f.type === "relation" || f.type === "file") && f.maxSelect > 1) r[f.name] = [];
    else if (f.type === "geoPoint") r[f.name] = { lat: 0, lon: 0 };
    else r[f.name] = "";
  }
  return r;
}

export function RecordEditor({ collection: c, recordId, onClose, onSaved, onDeleted }) {
  const isNew = recordId === "new";
  const isAuth = c.type === "auth";
  const readonly = c.type === "view";
  const [record, setRecord] = useState(() => {
    if (!isNew) return null;
    const start = pendingCopy && pendingCopy.collectionId === c.id ? pendingCopy : blank(c);
    pendingCopy = null;
    return start;
  });
  const [original, setOriginal] = useState(null);
  const [files, setFiles] = useState({});
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");
  const [changePassword, setChangePassword] = useState(isNew);
  const [errors, setErrors] = useState({});
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState(null);

  useEffect(() => {
    if (isNew) return;
    const relations = c.fields.filter((f) => f.type === "relation").map((f) => f.name);
    api
      .get(`/api/collections/${enc(c.name)}/records/${enc(recordId)}` + qs({ expand: relations.join(",") }))
      .then((r) => {
        setRecord(r);
        setOriginal(r);
      })
      .catch(setLoadError);
  }, [recordId]);

  const set = (name, value) => setRecord((r) => ({ ...r, [name]: value }));
  const fields = c.fields.filter((f) => !(isAuth && AUTH_SPECIAL.includes(f.name)) && f.type !== "password" && f.type !== "autodate");
  const autodates = c.fields.filter((f) => f.type === "autodate");

  const save = async (keepOpen = false) => {
    setSaving(true);
    setErrors({});
    try {
      const data = {};
      for (const f of c.fields) {
        if (f.type === "file" || f.type === "autodate" || f.type === "password") continue;
        if (!isNew && JSON.stringify(record[f.name]) === JSON.stringify(original[f.name])) continue;
        if (f.name === "id" && (!isNew || !record.id)) continue;
        data[f.name] = record[f.name];
      }
      if (isAuth && changePassword && password) {
        data.password = password;
        data.passwordConfirm = passwordConfirm;
      }
      const fileFields = c.fields.filter((f) => f.type === "file" && files[f.name]);
      let body = data;
      if (fileFields.length) {
        body = new FormData();
        for (const f of fileFields) {
          const state = files[f.name];
          if (state.removed && state.removed.length) data[f.name + "-"] = state.removed;
          for (const file of state.added || []) body.append(f.name + "+", file);
        }
        body.append("@jsonPayload", JSON.stringify(data));
      }
      const saved = isNew
        ? await api.post(`/api/collections/${enc(c.name)}/records`, body)
        : await api.patch(`/api/collections/${enc(c.name)}/records/${enc(recordId)}`, body);
      toast(isNew ? "Record created." : "Record saved.");
      setFiles({});
      setPassword("");
      setPasswordConfirm("");
      onSaved(saved, keepOpen);
      if (keepOpen && !isNew) {
        setRecord({ ...saved, expand: record.expand });
        setOriginal({ ...saved, expand: record.expand });
      }
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    const ok = await confirmDialog({ title: "Delete this record?", message: "Cascading relations and its files are deleted too.", confirm: "Delete", danger: true });
    if (!ok) return;
    try {
      await api.delete(`/api/collections/${enc(c.name)}/records/${enc(recordId)}`);
      toast("Record deleted.");
      onDeleted();
    } catch (err) {
      toast.error(err);
    }
  };

  const impersonate = async () => {
    try {
      const res = await api.post(`/api/collections/${enc(c.name)}/impersonate/${enc(recordId)}`, { duration: 3600 });
      await navigator.clipboard.writeText(res.token);
      toast("A one-hour auth token was copied.", "info");
    } catch (err) {
      toast.error(err);
    }
  };

  const requestVerification = async () => {
    try {
      await api.post(`/api/collections/${enc(c.name)}/request-verification`, { email: record.email });
      toast("Verification email sent.", "info");
    } catch (err) {
      toast.error(err);
    }
  };

  const requestReset = async () => {
    try {
      await api.post(`/api/collections/${enc(c.name)}/request-password-reset`, { email: record.email });
      toast("Password reset email sent.", "info");
    } catch (err) {
      toast.error(err);
    }
  };

  useEffect(() => {
    const fn = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && record && !readonly) {
        e.preventDefault();
        save(false);
      }
    };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  });

  const dirty = isNew || Object.keys(files).length > 0 || (password && changePassword) || (original && JSON.stringify(record) !== JSON.stringify(original));

  return html`<${Modal} size="wide" onClose=${onClose}>
    ${(close) => html`
      <${SheetHead}
        title=${isNew ? `New ${c.name} record` : record ? recordLabel(record, c) : "Loading…"}
        subtitle=${isNew ? "Fill in the fields and create" : html`<span class="mono">${recordId}</span>`}
        onClose=${close}
      >
        ${!isNew && !readonly &&
        html`<${Menu}
          trigger=${(toggle) => html`<${IconButton} icon="more" label="More" onClick=${toggle} />`}
          items=${[
            { icon: "copy", label: "Copy JSON", onClick: () => navigator.clipboard.writeText(JSON.stringify(record, null, 2)).then(() => toast("Copied.")) },
            {
              icon: "layers",
              label: "Duplicate",
              onClick: () => {
                const copy = { ...record, id: "" };
                // files and unique values aren't copied
                for (const f of c.fields) if (f.type === "file") copy[f.name] = f.maxSelect > 1 ? [] : "";
                if (isAuth) copy.email = "";
                pendingCopy = copy;
                toast("Edit the copy, then create it.", "info");
                location.hash = `#/data/${enc(c.name)}/records/new`;
              },
            },
            isAuth && "-",
            isAuth && { icon: "key", label: "Impersonate (copy token)", onClick: impersonate },
            isAuth && !record?.verified && { icon: "mail", label: "Send verification email", onClick: requestVerification },
            isAuth && { icon: "lock", label: "Send password reset email", onClick: requestReset },
            "-",
            { icon: "trash", label: "Delete", danger: true, onClick: remove },
          ]}
        />`}
      </${SheetHead}>
      ${loadError
        ? html`<div class="sheet-body"><${Callout} kind="danger">${loadError.message}</${Callout}></div>`
        : !record
        ? html`<div class="sheet-body stack">${[0, 1, 2, 3].map(() => html`<${Skeleton} height=${58} />`)}</div>`
        : html`<div class="record-layout">
            <form class="record-fields stack loose" onSubmit=${(e) => {
              e.preventDefault();
              save();
            }}>
              ${isAuth &&
              html`<div class="card pad stack" style="background:var(--surface-2)">
                <div class="row"><${Icon} name="key" size=${16} /><b>Account</b></div>
                <${Field} label="Email" error=${errors.email} required=${c.fields.find((f) => f.name === "email")?.required}>
                  <${Input} type="email" value=${record.email} invalid=${!!errors.email} onInput=${(v) => set("email", v)} />
                </${Field}>
                <div class="row wrap" style="gap:20px">
                  <${Switch} checked=${record.emailVisibility} onChange=${(v) => set("emailVisibility", v)} label="Public email" />
                  <${Switch} checked=${record.verified} onChange=${(v) => set("verified", v)} label="Verified" />
                </div>
                ${!isNew && html`<${Switch} checked=${changePassword} onChange=${setChangePassword} label="Change password" />`}
                ${changePassword &&
                html`<div class="grid-2 enter">
                  <${Field} label="Password" required=${isNew} error=${errors.password}><${Input} type="password" autocomplete="new-password" value=${password} invalid=${!!errors.password} onInput=${setPassword} /></${Field}>
                  <${Field} label="Confirm password" required=${isNew} error=${errors.passwordConfirm}><${Input} type="password" autocomplete="new-password" value=${passwordConfirm} invalid=${!!errors.passwordConfirm} onInput=${setPasswordConfirm} /></${Field}>
                </div>`}
              </div>`}
              ${fields.map(
                (f) => html`<${FieldInput}
                  key=${f.name}
                  field=${f}
                  value=${record[f.name]}
                  record=${record}
                  isNew=${isNew}
                  error=${errors[f.name]}
                  onChange=${(v) => set(f.name, v)}
                  files=${files[f.name]}
                  onFiles=${(state) => setFiles({ ...files, [f.name]: state })}
                />`
              )}
              <button type="submit" hidden></button>
            </form>
            <aside class="record-meta">
              ${!isNew &&
              html`<div class="meta-item"><span class="k">ID</span><span class="row mono small" style="gap:4px">${record.id}<${CopyButton} text=${record.id} /></span></div>`}
              ${autodates.map(
                (f) => html`<div class="meta-item">
                  <span class="k">${f.name}</span>
                  <span class="small">${record[f.name] ? html`${formatDate(record[f.name])}<div class="tiny faint">${relativeTime(record[f.name])}</div>` : html`<span class="faint">on save</span>`}</span>
                </div>`
              )}
              <div class="meta-item"><span class="k">Collection</span><span class="small">${c.name}</span></div>
              ${!isNew &&
              html`<div class="meta-item">
                <span class="k">API</span>
                <code class="tiny" style="word-break:break-all">/api/collections/${c.name}/records/${record.id}</code>
              </div>`}
              <div class="tiny faint" style="margin-top:18px"><span class="kbd">Ctrl</span> <span class="kbd">↵</span> save</div>
            </aside>
          </div>`}
      ${!readonly &&
      html`<div class="sheet-foot">
        ${dirty && !isNew && html`<span class="small muted row" style="gap:6px"><span class="dot warn"></span>Unsaved changes</span>`}
        <span class="spacer"></span>
        <${Button} variant="ghost" onClick=${close}>Cancel</${Button}>
        ${!isNew && html`<${Button} onClick=${() => save(true)} loading=${saving} disabled=${!dirty}>Save and keep editing</${Button}>`}
        <${Button} variant="primary" onClick=${() => save(false)} loading=${saving} disabled=${!record || !dirty}>${isNew ? "Create record" : "Save"}</${Button}>
      </div>`}
    `}
  </${Modal}>`;
}
