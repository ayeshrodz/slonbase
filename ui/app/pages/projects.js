// Projects: further installations hosted by this one, each in its own database. Only the main
// installation's dashboard shows this page.
import { html, useState, useEffect } from "../lib/core.js";
import { app } from "../lib/state.js";
import { api, enc, dashboardUrl } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Field, Input, TextArea, Switch, Modal, SheetHead, Empty, Callout, CopyButton, Skeleton, Menu, toast } from "../lib/ui.js";
import { formatDate, relativeTime } from "../lib/format.js";

const NAME_PATTERN = /^[a-z][a-z0-9-]{0,30}[a-z0-9]$/;

const STATUS = {
  ready: ["Ready", "ok"],
  starting: ["Starting", "warn"],
  setup: ["Needs setup", "warn"],
};

export function Projects() {
  const [items, setItems] = useState(null);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState(null);
  const [editingHosts, setEditingHosts] = useState(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    api
      .get("/api/projects")
      .then((res) => {
        setItems(res.items);
        app.set({ projects: res.items });
      })
      .catch((err) => {
        toast.error(err);
        setItems([]);
      });
  }, [tick]);
  // a starting project becomes ready on its own: check again shortly
  useEffect(() => {
    if (!items || !items.some((p) => p.status === "starting")) return;
    const timer = setTimeout(() => setTick((t) => t + 1), 3000);
    return () => clearTimeout(timer);
  }, [items]);
  const reload = () => setTick((t) => t + 1);

  return html`<div class="page enter">
    <div class="page-head">
      <div class="grow">
        <div class="eyebrow"><${Icon} name="layers" size=${14} /> Installation</div>
        <h1>Projects</h1>
      </div>
      <${IconButton} icon="refresh" label="Refresh" onClick=${reload} />
      <${Button} variant="primary" icon="plus" onClick=${() => setCreating(true)}>New project</${Button}>
    </div>
    <p class="muted" style="margin:-8px 0 20px;max-width:760px">
      Each project is a separate installation with its own collections, superusers, settings, files
      and hooks, kept in a PostgreSQL database of its own on this server. It's served under <code>/p/name/</code>,
      with its own dashboard.
    </p>
    ${items === null
      ? html`<div class="project-grid">${[0, 1, 2].map(() => html`<div class="card pad stack"><${Skeleton} width="40%" height=${16} /><${Skeleton} /><${Skeleton} width="70%" /></div>`)}</div>`
      : items.length === 0
        ? html`<div class="card">
            <${Empty}
              icon="layers"
              title="No projects yet"
              action=${html`<${Button} variant="primary" icon="plus" onClick=${() => setCreating(true)}>Create a project</${Button}>`}
            >
              Host several apps from this installation, each isolated in its own database.
            </${Empty}>
          </div>`
        : html`<div class="project-grid">
            ${items.map((p, i) => html`<${ProjectCard} key=${p.name} project=${p} index=${i} onDelete=${() => setDeleting(p)} onHosts=${() => setEditingHosts(p)} />`)}
          </div>`}
    ${creating &&
    html`<${CreateProject}
      onClose=${() => setCreating(false)}
      onCreated=${(project) => {
        const list = [...(items || []), project].sort((a, b) => a.name.localeCompare(b.name));
        setItems(list);
        app.set({ projects: list });
      }}
    />`}
    ${editingHosts &&
    html`<${EditHosts}
      project=${editingHosts}
      onClose=${() => setEditingHosts(null)}
      onSaved=${(project) => {
        const list = (items || []).map((p) => (p.name === project.name ? project : p));
        setItems(list);
        app.set({ projects: list });
      }}
    />`}
    ${deleting &&
    html`<${DeleteProject}
      project=${deleting}
      onClose=${() => setDeleting(null)}
      onDeleted=${(name) => {
        const list = (items || []).filter((p) => p.name !== name);
        setItems(list);
        app.set({ projects: list });
      }}
    />`}
  </div>`;
}

function ProjectCard({ project, index, onDelete, onHosts }) {
  const [label, tone] = STATUS[project.status] || [project.status, "outline"];
  const url = location.origin + project.path.replace(/\/$/, "");
  return html`<div class="card pad project-card" style=${{ "--i": index }}>
    <div class="row">
      <span class="coll-glyph project-glyph">${project.name.slice(0, 1)}</span>
      <div class="grow" style="min-width:0">
        <a class="project-name" href=${dashboardUrl(project.name)}>${project.name}</a>
        <div class="small muted mono truncate">${project.path}</div>
      </div>
      <span class=${"chip " + tone}>${label}</span>
      <${Menu}
        trigger=${(toggle) => html`<${IconButton} icon="more" label="More" onClick=${toggle} />`}
        items=${[
          { icon: "external", label: "Open dashboard", onClick: () => (location.href = dashboardUrl(project.name)) },
          { icon: "copy", label: "Copy API URL", onClick: () => navigator.clipboard.writeText(url).then(() => toast("Copied.")) },
          { icon: "globe", label: "Host names…", onClick: onHosts },
          "-",
          { icon: "trash", label: "Delete project", danger: true, onClick: onDelete },
        ]}
      />
    </div>
    ${project.statusDetail && html`<${Callout} kind="warn">${project.statusDetail}</${Callout}>`}
    <dl class="project-facts small">
      <dt>Database</dt><dd class="mono">${project.database}${project.role && html` <span class="muted">· own role</span>`}</dd>
      ${project.hosts.length > 0 &&
      html`<dt>Hosts</dt><dd>${project.hosts.map((h, i) => html`${i > 0 && ", "}<a href=${`//${h}/_/`} class="mono">${h}</a>`)}</dd>`}
      <dt>Created</dt><dd><span class="tip" data-tip=${formatDate(project.created)}>${relativeTime(project.created)}</span></dd>
    </dl>
    <div class="row" style="margin-top:auto">
      <code class="small truncate grow">new PocketBase("${url}")</code>
      <${CopyButton} text=${`new PocketBase("${url}")`} />
      <a class="btn sm" href=${dashboardUrl(project.name)}>Open <${Icon} name="arrowRight" size=${14} /></a>
    </div>
  </div>`;
}

function CreateProject({ onClose, onCreated }) {
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [ownRole, setOwnRole] = useState(false);
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState(null);
  const nameOk = NAME_PATTERN.test(name);

  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setErrors({});
    try {
      const project = await api.post("/api/projects", { name, superuserEmail: email, superuserPassword: password, ownRole });
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
              <p><b>${created.name}</b> is ready, in the database <code>${created.database}</code>.</p>
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
                <${Switch} checked=${ownRole} onChange=${setOwnRole} label="Own database role" />
                <span class="small muted">
                  The project connects with a PostgreSQL role of its own that owns its database, and other roles can't connect to it. Needs CREATEROLE.
                </span>
              </div>
            </div>
            <div class="sheet-foot">
              <span class="spacer"></span>
              <${Button} variant="ghost" onClick=${close} disabled=${busy}>Cancel</${Button}>
              <${Button} variant="primary" type="submit" loading=${busy} disabled=${!nameOk || !email || !password}>Create project</${Button}>
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
              <p><b>${project.name}</b> and its database were deleted.</p>
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
                    This stops the project and drops its database, <code>${project.database}</code>, with every
                    collection, record and superuser in it.
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
