// Projects: further installations hosted by this one, each in its own database. Only the main
// installation's dashboard shows this page: its command center (command-center.js), and the
// dialogs to create, rename the hosts of and delete projects.
import { html, useState } from "../lib/core.js";
import { CommandCenter } from "./command-center.js";
import { app } from "../lib/state.js";
import { api, enc, dashboardUrl } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, Field, Input, TextArea, Switch, Modal, SheetHead, Callout, CopyButton, Segmented, toast } from "../lib/ui.js";

const NAME_PATTERN = /^[a-z][a-z0-9-]{0,30}[a-z0-9]$/;

export function Projects() {
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState(null);
  const [editingHosts, setEditingHosts] = useState(null);
  const [version, setVersion] = useState(0);
  // the project switcher in the sidebar follows the list
  const changed = () => {
    setVersion((v) => v + 1);
    api.get("/api/projects").then((res) => app.set({ projects: res.items })).catch(() => {});
  };
  return html`<${CommandCenter}
      version=${version}
      onCreate=${() => setCreating(true)}
      onDelete=${setDeleting}
      onHosts=${setEditingHosts}
    />
    ${creating && html`<${CreateProject} onClose=${() => setCreating(false)} onCreated=${changed} />`}
    ${editingHosts && html`<${EditHosts} project=${editingHosts} onClose=${() => setEditingHosts(null)} onSaved=${changed} />`}
    ${deleting && html`<${DeleteProject} project=${deleting} onClose=${() => setDeleting(null)} onDeleted=${changed} />`}`;
}

function CreateProject({ onClose, onCreated }) {
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [ownRole, setOwnRole] = useState(false);
  const [where, setWhere] = useState("here");
  const [databaseUrl, setDatabaseUrl] = useState("");
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState(null);
  const nameOk = NAME_PATTERN.test(name);

  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setErrors({});
    try {
      const project = await api.post("/api/projects", {
        name,
        superuserEmail: email,
        superuserPassword: password,
        ownRole: where === "here" && ownRole,
        databaseUrl: where === "connect" ? databaseUrl : undefined,
      });
      onCreated(project);
      setCreated(project);
    } catch (err) {
      const fields = err.fieldErrors ? err.fieldErrors() : {};
      setErrors(Object.keys(fields).length ? fields : { form: err.message });
    } finally {
      setBusy(false);
    }
  };

  return html`<${Modal} size="narrow" onClose=${onClose} dismissable=${!busy}>
    ${(close) =>
      created
        ? html`<${SheetHead} title="Project created" icon="check" onClose=${close} />
            <div class="sheet-body stack">
              <p><b>${created.name}</b> is ready, in the database <code>${created.database}</code>${created.external ? html` on <code>${created.server}</code>` : ""}.</p>
              <p class="muted small">
                Sign in to its dashboard with the superuser you just created. Its API is at <code>${location.origin + created.path}api/</code>.
              </p>
            </div>
            <div class="sheet-foot">
              <span class="spacer"></span>
              <${Button} variant="ghost" onClick=${close}>Close</${Button}>
              <a class="btn primary" href=${dashboardUrl(created.name)}>Open its dashboard <${Icon} name="arrowRight" size=${15} /></a>
            </div>`
        : html`<form onSubmit=${submit}>
            <${SheetHead} title="New project" subtitle="A separate installation with its own database" icon="layers" onClose=${close} />
            <div class="sheet-body stack">
              ${errors.form && html`<${Callout} kind="danger">${errors.form}</${Callout}>`}
              <${Field}
                label="Name"
                error=${errors.name}
                help=${name && !nameOk
                  ? "Lowercase letters, digits and dashes, starting with a letter (2–32 characters)."
                  : html`Served at <code>/p/${name || "name"}/</code>`}
              >
                <${Input} value=${name} onInput=${(v) => setName(v.toLowerCase())} autofocus placeholder="blog" class="mono" invalid=${!!name && !nameOk} required />
              </${Field}>
              <div class="stack tight" style="margin-top:4px">
                <span class="label">First superuser</span>
                <span class="small muted">Signs in to the project's dashboard. Superusers of this installation don't.</span>
              </div>
              <${Field} label="Email" error=${errors.superuserEmail}>
                <${Input} type="email" value=${email} onInput=${setEmail} placeholder="owner@example.com" autocomplete="off" required />
              </${Field}>
              <${Field} label="Password" error=${errors.superuserPassword} help="At least 10 characters">
                <${Input} type="password" value=${password} onInput=${setPassword} autocomplete="new-password" required />
              </${Field}>
              <div class="stack tight" style="margin-top:4px">
                <span class="label">Database</span>
                <${Segmented}
                  value=${where}
                  onChange=${setWhere}
                  options=${[
                    { value: "here", label: "Create on this server" },
                    { value: "connect", label: "Connect a database" },
                  ]}
                />
              </div>
              ${where === "here"
                ? html`<div class="stack tight">
                    <${Switch} checked=${ownRole} onChange=${setOwnRole} label="Own database role" />
                    <span class="small muted">
                      The project connects with a PostgreSQL role of its own that owns its database, and other roles can't connect to it. Needs CREATEROLE.
                    </span>
                  </div>`
                : html`<${Field}
                    label="Connection URL"
                    error=${errors.databaseUrl || errors.ownRole}
                    help="An existing database, on any PostgreSQL 14+ server. slonbase adds its tables to it; a database that already holds a slonbase installation keeps its data and superusers. Deleting the project leaves the database as it is."
                  >
                    <${Input} value=${databaseUrl} onInput=${setDatabaseUrl} class="mono" placeholder="postgres://user:password@db2.internal:5432/shop?sslmode=require" autocomplete="off" required />
                  </${Field}>`}
            </div>
            <div class="sheet-foot">
              <span class="spacer"></span>
              <${Button} variant="ghost" onClick=${close} disabled=${busy}>Cancel</${Button}>
              <${Button} variant="primary" type="submit" loading=${busy} disabled=${!nameOk || !email || !password || (where === "connect" && !databaseUrl.trim())}>${where === "connect" ? "Connect project" : "Create project"}</${Button}>
            </div>
          </form>`}
  </${Modal}>`;
}

function DeleteProject({ project, onClose, onDeleted }) {
  const [typed, setTyped] = useState("");
  const [backup, setBackup] = useState(true);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState(null);
  const [error, setError] = useState("");

  const remove = async () => {
    setBusy(true);
    setError("");
    try {
      const res = await api.delete(`/api/projects/${enc(project.name)}?confirm=${enc(project.name)}${backup ? "" : "&backup=false"}`);
      onDeleted(project.name);
      setResult(res || {});
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  };

  return html`<${Modal} size="narrow" onClose=${onClose} dismissable=${!busy}>
    ${(close) =>
      result
        ? html`<${SheetHead} title="Project deleted" icon="check" onClose=${close} />
            <div class="sheet-body stack">
              <p>${result.databaseKept ? html`<b>${project.name}</b> was deleted. Its database, <code>${result.databaseKept}</code>, was left as it is.` : html`<b>${project.name}</b> and its database were deleted.`}</p>
              ${result.backup &&
              html`<div class="field"><span class="label">Backup</span><div class="row"><code class="small grow truncate">${result.backup}</code><${CopyButton} text=${result.backup} /></div></div>`}
              ${result.files &&
              html`<div class="field"><span class="label">Files</span><div class="row"><code class="small grow truncate">${result.files}</code><${CopyButton} text=${result.files} /></div></div>`}
            </div>
            <div class="sheet-foot"><span class="spacer"></span><${Button} variant="primary" onClick=${close}>Done</${Button}></div>`
        : html`<div class="sheet-body stack">
              <div class="row top">
                <div class="field-type-badge" style="color:var(--danger)"><${Icon} name="warning" /></div>
                <div class="stack tight grow">
                  <h3>Delete ${project.name}?</h3>
                  <p class="muted">
                    ${project.external
                      ? html`This stops the project and removes it from this installation, with its files. Its database, <code>${project.database}</code> on <code>${project.server}</code>, isn't slonbase's to drop: it's left as it is.`
                      : html`This stops the project and drops its database, <code>${project.database}</code>, with every collection, record and superuser in it.`}
                  </p>
                </div>
              </div>
              ${error && html`<${Callout} kind="danger">${error}</${Callout}>`}
              <div class="row">
                <${Switch} checked=${backup} onChange=${setBackup} label="Back it up first" />
                <span class="small muted">${backup ? "Kept with the project's files" : "Not recommended"}</span>
              </div>
              <${Field} label=${html`Type <code>${project.name}</code> to confirm`}>
                <${Input} value=${typed} onInput=${setTyped} autofocus class="mono" />
              </${Field}>
            </div>
            <div class="sheet-foot">
              <span class="spacer"></span>
              <${Button} variant="ghost" onClick=${close} disabled=${busy}>Cancel</${Button}>
              <${Button} variant="danger" icon="trash" loading=${busy} disabled=${typed !== project.name} onClick=${remove}>
                ${backup ? "Back up and delete" : "Delete"}
              </${Button}>
            </div>`}
  </${Modal}>`;
}

function EditHosts({ project, onClose, onSaved }) {
  const [text, setText] = useState(project.hosts.join("\n"));
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const save = async (e, close) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const hosts = text.split(/[\s,]+/).map((h) => h.trim()).filter(Boolean);
      const updated = await api.patch(`/api/projects/${enc(project.name)}`, { hosts });
      onSaved(updated);
      toast(hosts.length ? "Host names saved." : "Host names removed.");
      close();
    } catch (err) {
      const fields = err.fieldErrors ? err.fieldErrors() : {};
      setError(fields.hosts || err.message);
    } finally {
      setBusy(false);
    }
  };
  return html`<${Modal} size="narrow" onClose=${onClose}>
    ${(close) => html`<form onSubmit=${(e) => save(e, close)}>
      <${SheetHead} title="Host names" subtitle=${project.name} icon="globe" onClose=${close} />
      <div class="sheet-body stack">
        <p class="muted small">
          Requests for these host names go to ${project.name}, at the root path: its API at <code>https://host/api/</code>, its dashboard at <code>/_/</code>. It stays available at <code>${project.path}</code> too.
        </p>
        <${Field} label="One host name per line" error=${error}>
          <${TextArea} rows=${4} class="mono" value=${text} onInput=${setText} placeholder="shop.example.com" autofocus />
        </${Field}>
        <${Callout}>Point each name's DNS at this server, and give your reverse proxy a certificate for it. Set the project's app URL in its settings so emails link to the new address.</${Callout}>
      </div>
      <div class="sheet-foot">
        <span class="spacer"></span>
        <${Button} variant="ghost" onClick=${close}>Cancel</${Button}>
        <${Button} variant="primary" type="submit" loading=${busy}>Save</${Button}>
      </div>
    </form>`}
  </${Modal}>`;
}
