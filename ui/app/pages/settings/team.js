// Settings → Team: who can use this dashboard and with which role, and API keys; App users:
// the auth collections your application signs its users in with.
import { html, useState, useEffect, useStore, cx } from "../../lib/core.js";
import { api, enc, session, BASE } from "../../lib/api.js";
import { app } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { Button, IconButton, Field, Input, Select, Segmented, Modal, SheetHead, Skeleton, Empty, Callout, toast, confirmDialog, CollectionGlyph } from "../../lib/ui.js";
import { formatDate, relativeTime } from "../../lib/format.js";
import { branding } from "../../lib/branding.js";
import { TokenLifetimes } from "./storage.js";
import { Card } from "./shared.js";

export const ROLES = [
  { value: "owner", label: "Owner", description: "Everything, including the team, restores and the SQL console" },
  { value: "admin", label: "Admin", description: "Runs the installation: settings, backups, projects and keys, but not the team, restores or the SQL console" },
  { value: "developer", label: "Developer", description: "Builds on it: collections, records, files and cron jobs; the rest only to look at" },
  { value: "viewer", label: "Viewer", description: "Looks at everything, changes nothing" },
];

// What the table of roles says about each capability. Who has which is the server's: it asks the
// function that enforces roles (GET /api/team → capabilities), so the table can't claim more or
// less than is enforced; only the words are here.
const CAPABILITIES = [
  { group: "Look", items: [{ id: "view", label: "See everything", detail: "collections, records, files, logs and settings" }] },
  {
    group: "Build",
    items: [
      { id: "data", label: "Edit collections, records and files", detail: "API rules and batches included" },
      { id: "crons", label: "Run cron jobs now" },
      { id: "cronJobs", label: "Make, change and delete cron jobs" },
    ],
  },
  {
    group: "Operate",
    items: [
      { id: "settings", label: "Change settings and branding", detail: "mail, storage, security, logs" },
      { id: "backups", label: "Create backups" },
      { id: "backupFiles", label: "Download backup files" },
      { id: "projects", label: "Create projects" },
      { id: "keys", label: "Create API keys" },
    ],
  },
  {
    group: "Administer",
    items: [
      { id: "team", label: "Add superusers and change roles" },
      { id: "restore", label: "Restore a backup" },
      { id: "deleteProject", label: "Delete a project" },
      { id: "sql", label: "Use the SQL console", detail: "reads every table, password hashes included" },
    ],
  },
];
const RANK = { viewer: 0, developer: 1, admin: 2, owner: 3 };

// the signed-in superuser's role, for the controls they may use
export function useMyRole() {
  const [role, setRole] = useState(null);
  useEffect(() => {
    api
      .get("/api/team/me")
      .then((m) => setRole(m.role))
      .catch(() => setRole("owner"));
  }, []);
  return role;
}

function RolesTable({ team, myRole }) {
  // on a phone the four columns don't fit: one role at a time, picked above the table
  const [pick, setPick] = useState(myRole || "owner");
  if (!Array.isArray(team.capabilities)) return null;
  const who = Object.fromEntries(team.capabilities.map((c) => [c.id, c.roles]));
  const columns = [...ROLES].reverse(); // from the least to the most
  const picked = ROLES.find((r) => r.value === pick) || ROLES[0];
  return html`<${Card} title="What each role can do" subtitle="A superuser without a role is an owner" icon="shield">
    <div class="perm-picker">
      <${Segmented} value=${pick} onChange=${setPick} options=${columns.map((r) => ({ value: r.value, label: r.label }))} />
      <div class="small muted">${picked.description}</div>
    </div>
    <div class="table-wrap"><table class="table perm-table" data-pick=${pick}>
      <thead><tr>
        <th scope="col"><span class="sr-only">What</span></th>
        ${columns.map(
          (r) => html`<th scope="col" class=${cx("perm-role", "r-" + r.value, myRole === r.value && "me")} title=${r.description}>
            ${r.label}${myRole === r.value && html`<span class="perm-you">you</span>`}
          </th>`
        )}
      </tr></thead>
      <tbody>
        ${CAPABILITIES.flatMap((g) => [
          html`<tr class="perm-group" key=${g.group}><th scope="rowgroup" colspan=${columns.length + 1}>${g.group}</th></tr>`,
          ...g.items.map(
            (c) => html`<tr key=${c.id}>
              <th scope="row">${c.label}${c.detail && html`<span class="perm-detail">${c.detail}</span>`}</th>
              ${columns.map((r) => {
                const yes = (who[c.id] || []).includes(r.value);
                return html`<td class=${cx("perm-cell", "r-" + r.value, yes && "yes", myRole === r.value && "me")}>
                  ${yes ? html`<${Icon} name="check" size=${16} />` : html`<span aria-hidden="true">–</span>`}
                  <span class="sr-only">${yes ? "Yes" : "No"}</span>
                </td>`;
              })}
            </tr>`
          ),
        ])}
      </tbody>
    </table></div>
  </${Card}>`;
}

function RoleChip({ role }) {
  return html`<span class=${cx("chip", role === "owner" ? "accent" : "outline")}>${role}</span>`;
}

function AddMember({ onClose, onDone }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState("developer");
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setErrors({});
    try {
      const rec = await api.post("/api/collections/_superusers/records", { email, password, passwordConfirm: password, verified: true });
      if (role !== "owner") await api.patch(`/api/team/members/${enc(rec.id)}`, { role });
      toast(`${email} can sign in now.`);
      onDone();
      onClose();
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<${Modal} onClose=${onClose} size="mid">
    <${SheetHead} title="Add a superuser" subtitle="They sign in to this dashboard with this email and password" onClose=${onClose} />
    <form class="sheet-body stack" onSubmit=${submit}>
      <${Field} label="Email" error=${errors.email}><${Input} type="email" value=${email} onInput=${setEmail} required autofocus /></${Field}>
      <${Field} label="Password" error=${errors.password} help="At least 10 characters; they can change it afterwards"><${Input} type="password" value=${password} onInput=${setPassword} required minlength="10" autocomplete="new-password" /></${Field}>
      <${Field} label="Role">
        <div class="role-options">
          ${ROLES.map(
            (r) => html`<label class=${cx("role-option", { on: role === r.value })}>
              <input type="radio" name="role" value=${r.value} checked=${role === r.value} onChange=${() => setRole(r.value)} />
              <b>${r.label}</b><span class="tiny muted">${r.description}</span>
            </label>`
          )}
        </div>
      </${Field}>
      <div class="row"><span class="spacer"></span><${Button} variant="ghost" onClick=${onClose}>Cancel</${Button}><${Button} variant="primary" type="submit" loading=${busy}>Add superuser</${Button}></div>
    </form>
  </${Modal}>`;
}

function NewKey({ myRole, onClose, onDone }) {
  const [name, setName] = useState("");
  const [role, setRole] = useState("viewer");
  const [days, setDays] = useState("90");
  const [ips, setIps] = useState("");
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  const [made, setMade] = useState(null);
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setErrors({});
    try {
      const res = await api.post("/api/team/keys", {
        name,
        role,
        expiresDays: Number(days) || 0,
        allowedIPs: ips.split(",").map((x) => x.trim()).filter(Boolean),
      });
      setMade(res);
      onDone();
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  const base = (branding.get().appURL || location.origin + BASE).replace(/\/$/, "");
  return html`<${Modal} onClose=${onClose} size="mid" dismissable=${!made}>
    <${SheetHead} title=${made ? "Copy the key now" : "New API key"} subtitle=${made ? "It isn't shown again" : "For scripts, CI and servers: it acts for you, with the role you choose"} onClose=${onClose} />
    ${made
      ? html`<div class="sheet-body stack">
          <${Callout} kind="warn">Store it somewhere safe. Anyone with it has its role here until it's revoked${made.expires ? ` or expires (${formatDate(made.expires)})` : ""}.</${Callout}>
          <div class="row"><code class="grow key-secret">${made.key}</code><${Button} icon="copy" onClick=${() => navigator.clipboard.writeText(made.key).then(() => toast("Copied."), toast.error)}>Copy</${Button}></div>
          <div class="small muted">Send it as <code>Authorization: Bearer ${made.prefix}…</code>, e.g.</div>
          <pre class="code" tabindex="0">curl -H "Authorization: Bearer $SLONBASE_KEY" ${base}/api/collections</pre>
          <div class="row"><span class="spacer"></span><${Button} variant="primary" onClick=${onClose}>Done</${Button}></div>
        </div>`
      : html`<form class="sheet-body stack" onSubmit=${submit}>
          <${Field} label="Name" error=${errors.name} help="What it's for, e.g. “nightly export”"><${Input} value=${name} onInput=${setName} required autofocus maxlength="60" /></${Field}>
          <div class="grid-2">
            <${Field} label="Role" error=${errors.role}>
              <${Select} value=${role} onChange=${setRole} options=${ROLES.filter((r) => RANK[r.value] <= RANK[myRole || "viewer"]).map((r) => ({ value: r.value, label: r.label }))} />
            </${Field}>
            <${Field} label="Expires after (days)" error=${errors.expiresDays} help="0: never"><${Input} type="number" min="0" max="3650" value=${days} onInput=${setDays} /></${Field}>
          </div>
          <${Field} label="Only from these IPs" error=${Object.keys(errors).find((k) => k.startsWith("allowedIPs")) && "Each must be an IP address or a CIDR range."} help="IPs or CIDR ranges, separated by commas; empty: anywhere">
            <${Input} class="mono" value=${ips} onInput=${setIps} placeholder="203.0.113.7, 10.0.0.0/8" />
          </${Field}>
          <div class="row"><span class="spacer"></span><${Button} variant="ghost" onClick=${onClose}>Cancel</${Button}><${Button} variant="primary" type="submit" icon="key" loading=${busy}>Create key</${Button}></div>
        </form>`}
  </${Modal}>`;
}

export function Team() {
  const [team, setTeam] = useState(null);
  const [adding, setAdding] = useState(false);
  const [keying, setKeying] = useState(false);
  const myRole = useMyRole();
  const me = (session.record || {}).id;
  const load = () =>
    api
      .get("/api/team")
      .then(setTeam)
      .catch((err) => {
        toast.error(err);
        setTeam({ members: [], apiKeys: [] });
      });
  useEffect(() => {
    load();
  }, []);
  const owner = myRole === "owner";
  const setRole = async (m, role) => {
    try {
      await api.patch(`/api/team/members/${enc(m.id)}`, { role });
      toast(`${m.email} is now ${role === "admin" ? "an" : "a"} ${role}.`);
      load();
    } catch (err) {
      toast.error(err);
    }
  };
  const remove = async (m) => {
    if (!(await confirmDialog({ title: `Remove ${m.email}?`, message: "They can't sign in anymore; their API keys stop working.", confirm: "Remove", danger: true }))) return;
    try {
      await api.delete(`/api/collections/_superusers/records/${enc(m.id)}`);
      toast("Removed.");
      load();
    } catch (err) {
      toast.error(err);
    }
  };
  const revoke = async (k) => {
    if (!(await confirmDialog({ title: `Revoke ${k.name}?`, message: "What uses it stops working at once.", confirm: "Revoke", danger: true }))) return;
    try {
      await api.delete(`/api/team/keys/${enc(k.id)}`);
      toast("Revoked.");
      load();
    } catch (err) {
      toast.error(err);
    }
  };
  if (!team) return html`<${Skeleton} height=${200} />`;
  const email = (id) => (team.members.find((m) => m.id === id) || {}).email || id;
  return html`<div class="stack loose">
    <${Card}
      title="Superusers"
      subtitle=${`${team.members.length} can sign in to this dashboard${myRole ? ` · you're ${myRole === "admin" ? "an" : "a"} ${myRole}` : ""}`}
      icon="users"
      actions=${owner && html`<${Button} size="sm" variant="primary" icon="plus" onClick=${() => setAdding(true)}>Add superuser</${Button}>`}
    >
      <div class="table-wrap"><table class="table">
        <thead><tr><th>Email</th><th>Role</th><th>Since</th><th></th></tr></thead>
        <tbody>
          ${team.members.map(
            (m) => html`<tr key=${m.id}>
              <td><b>${m.email}</b>${m.id === me && html` <span class="chip outline">you</span>`}</td>
              <td>
                ${owner
                  ? html`<${Select} aria-label=${`Role of ${m.email}`} value=${m.role} onChange=${(v) => setRole(m, v)} options=${ROLES.map((r) => ({ value: r.value, label: r.label }))} />`
                  : html`<${RoleChip} role=${m.role} />`}
              </td>
              <td class="small muted">${relativeTime(m.created)}</td>
              <td class="row end">${owner && m.id !== me && html`<${IconButton} icon="trash" label=${`Remove ${m.email}`} onClick=${() => remove(m)} />`}</td>
            </tr>`
          )}
        </tbody>
      </table></div>
      ${!owner && myRole && html`<div class="small muted row"><${Icon} name="info" size=${14} /> Only owners add superusers and change roles.</div>`}
    </${Card}>
    <${RolesTable} team=${team} myRole=${myRole} />
    <${Card}
      title="API keys"
      subtitle="For scripts, CI and servers: each acts for a superuser, with a role of its own"
      icon="key"
      actions=${(owner || myRole === "admin") && html`<${Button} size="sm" icon="plus" onClick=${() => setKeying(true)}>New key</${Button}>`}
    >
      ${team.apiKeys.length === 0
        ? html`<${Empty} icon="key" title="No API keys">Keys sign requests in without a password, with a role and, if you like, an expiry date and IPs.</${Empty}>`
        : html`<div class="table-wrap"><table class="table">
            <thead><tr><th>Name</th><th>Key</th><th>Role</th><th>For</th><th>Expires</th><th>Last used</th><th></th></tr></thead>
            <tbody>
              ${team.apiKeys.map(
                (k) => html`<tr key=${k.id}>
                  <td><b>${k.name}</b>${k.allowedIPs.length > 0 && html`<div class="tiny muted mono">${k.allowedIPs.join(", ")}</div>`}</td>
                  <td class="mono small">${k.prefix}…</td>
                  <td><${RoleChip} role=${k.role} /></td>
                  <td class="small">${email(k.superuser)}</td>
                  <td class="small">${k.expires ? (new Date(k.expires) < new Date() ? html`<span class="chip danger">expired</span>` : relativeTime(k.expires)) : html`<span class="muted">never</span>`}</td>
                  <td class="small muted">${k.lastUsed ? relativeTime(k.lastUsed) : "—"}</td>
                  <td class="row end">${(owner || k.superuser === me) && html`<${IconButton} icon="trash" label=${`Revoke ${k.name}`} onClick=${() => revoke(k)} />`}</td>
                </tr>`
              )}
            </tbody>
          </table></div>`}
    </${Card}>
    <${TokenLifetimes} only=${["_superusers"]} />
    <${Card} title="Signing in with OAuth2" subtitle="Superusers can sign in with Google, GitHub, Microsoft… when providers are set on _superusers" icon="globe" actions=${html`<a class="btn sm" href="#/data/_superusers/auth"><${Icon} name="settings" size=${15} /><span>Set up providers</span></a>`}>
      <div class="small muted">The sign-in page shows a button per provider. Only existing superusers whose email the provider has verified can sign in this way: nobody gets an account by signing in.</div>
    </${Card}>
    ${adding && html`<${AddMember} onClose=${() => setAdding(false)} onDone=${load} />`}
    ${keying && html`<${NewKey} myRole=${myRole} onClose=${() => setKeying(false)} onDone=${load} />`}
  </div>`;
}

export function AppUsers() {
  const collections = useStore(app, (st) => st.collections).filter((c) => c.type === "auth" && c.name !== "_superusers");
  const [counts, setCounts] = useState({});
  useEffect(() => {
    for (const c of collections) {
      api
        .get(`/api/collections/${enc(c.name)}/records?perPage=1&skipTotal=0&fields=id`)
        .then((r) => setCounts((x) => ({ ...x, [c.name]: r.totalItems })))
        .catch(() => {});
    }
  }, [collections.length]);
  const redirect = (branding.get().appURL || location.origin + BASE).replace(/\/$/, "") + "/api/oauth2-redirect";
  return html`<div class="stack loose">
    <${Card} title="Your application's users" subtitle="The auth collections it signs people in with, and how" icon="users" actions=${html`<a class="btn sm" href="#/data?new=auth"><${Icon} name="plus" size=${15} /><span>New auth collection</span></a>`}>
      ${collections.length === 0
        ? html`<${Empty} icon="users" title="No auth collections yet">Create one (a “users” collection, say) to sign your app's users in with passwords, one-time codes or OAuth2.</${Empty}>`
        : html`<div class="app-users">
            ${collections.map((c) => {
              const providers = (c.oauth2 && c.oauth2.providers) || [];
              const methods = [
                c.passwordAuth && c.passwordAuth.enabled && `password (${(c.passwordAuth.identityFields || []).join(", ")})`,
                c.otp && c.otp.enabled && "one-time code",
                c.oauth2 && c.oauth2.enabled && `OAuth2 (${providers.length})`,
                c.mfa && c.mfa.enabled && "MFA",
              ].filter(Boolean);
              return html`<div class="app-user-card">
                <div class="row">
                  <${CollectionGlyph} collection=${c} size=${28} />
                  <div class="grow"><b>${c.name}</b><div class="tiny muted">${counts[c.name] == null ? "…" : `${counts[c.name].toLocaleString()} user${counts[c.name] === 1 ? "" : "s"}`}</div></div>
                  <a class="btn sm" href=${`#/data/${enc(c.name)}/records`}>Users</a>
                  <a class="btn sm" href=${`#/data/${enc(c.name)}/auth`}><${Icon} name="settings" size=${14} /><span>Sign-in options</span></a>
                </div>
                <div class="row wrap tight">${methods.length ? methods.map((m) => html`<span class="chip outline">${m}</span>`) : html`<span class="chip warn">no way to sign in</span>`}</div>
                ${providers.length > 0 &&
                html`<div class="row wrap tight">${providers.map((p) => html`<span class=${cx("chip", p.clientId ? "ok" : "warn")} title=${p.clientId ? "Configured" : "No client id"}>${p.displayName || p.name}</span>`)}</div>`}
              </div>`;
            })}
          </div>`}
    </${Card}>
    <${Card} title="OAuth2 redirect URL" subtitle="Register it with each provider (Google, GitHub…) for every collection" icon="link">
      <div class="row"><code class="grow ellipsis">${redirect}</code><${IconButton} icon="copy" label="Copy" onClick=${() => navigator.clipboard.writeText(redirect).then(() => toast("Copied."), toast.error)} /></div>
      <div class="small muted">It's the application URL (Settings → Application) followed by <code>/api/oauth2-redirect</code>; the PocketBase SDKs use it as they are.</div>
    </${Card}>
  </div>`;
}
