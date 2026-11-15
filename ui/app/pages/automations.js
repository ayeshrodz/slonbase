// Automations: what runs on a schedule (a list of steps that pass data on, the jobs of hook
// files and the automatic backup), what ran, and the email templates the steps send.
import { html, useState, useEffect, useStore, cx, navigate } from "../lib/core.js";
import { api, enc } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, IconButton, Field, Input, Select, Switch, Skeleton, Empty, Modal, SheetHead, Callout, Tabs, toast, confirmDialog } from "../lib/ui.js";
import { app } from "../lib/state.js";
import { formatDateIn, relativeTime } from "../lib/format.js";
import { Card } from "./settings/shared.js";
import { useMyRole } from "./settings/team.js";
import { JobHistory, StatusChip, ms } from "./settings/operations.js";

const SCHEDULES = [
  ["Every 5 minutes", "*/5 * * * *"],
  ["Hourly", "0 * * * *"],
  ["Daily at 03:00", "0 3 * * *"],
  ["Weekly (Sunday 03:00)", "0 3 * * 0"],
];

// what a step can do: its label, the name a new one gets, and what it gives the steps after it
const STEP_TYPES = [
  { value: "query", label: "Find records", name: "records", gives: "rows, count" },
  { value: "condition", label: "Continue only if", name: "check", gives: "passed" },
  { value: "email", label: "Send a template", name: "send", gives: "sent", repeat: true },
  { value: "write", label: "Change a record", name: "save", gives: "id", repeat: true },
  { value: "http", label: "Call a URL", name: "call", gives: "status, json, text", repeat: true },
  { value: "webhook", label: "Send a signed webhook", name: "hook", gives: "status", repeat: true },
  { value: "script", label: "Run a hook function", name: "run", gives: "result", repeat: true },
  { value: "mail", label: "Send a plain email", name: "notify", gives: "sent", repeat: true },
  { value: "cleanup", label: "Clean up records", name: "cleanup", gives: "deleted" },
  { value: "sql", label: "Run SQL", name: "rows", gives: "rows, count, affectedRows", owner: true },
];
const typeOf = (value) => STEP_TYPES.find((t) => t.value === value) || STEP_TYPES[0];
const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const ACTIONS = [
  { value: "update", label: "Update" },
  { value: "create", label: "Create" },
  { value: "delete", label: "Delete" },
];

// headers are edited as lines of "Name: value"; saved values come back masked and stay as they are
const headerLines = (h) => Object.entries(h || {}).map(([k, v]) => `${k}: ${v}`).join("\n");
const headerObject = (text) => {
  const out = {};
  for (const line of String(text || "").split("\n")) {
    const i = line.indexOf(":");
    if (i > 0) out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return out;
};
// objects are edited as JSON; what isn't an object is sent as typed, so the server says what's wrong
const jsonText = (v) => (v && typeof v === "object" && Object.keys(v).length ? JSON.stringify(v, null, 2) : "");
const jsonValue = (text) => {
  const t = String(text || "").trim();
  if (!t) return {};
  try {
    return JSON.parse(t);
  } catch {
    return t;
  }
};
const num = (v) => (String(v == null ? "" : v).trim() ? Number(v) : null);

// a step's settings as the form holds them (texts), from how they are stored
function formOf(type, c = {}) {
  switch (type) {
    case "query":
      return { collection: c.collection || "", filter: c.filter || "", sort: c.sort || "", limit: String(c.limit || 100), params: jsonText(c.params) };
    case "condition":
      return { expression: c.expression || "" };
    case "email":
      return { template: c.template || "", to: Array.isArray(c.to) ? c.to.join(", ") : c.to || "", data: jsonText(c.data) };
    case "write":
      return { action: c.action || "update", collection: c.collection || "", id: c.id || "", data: jsonText(c.data) };
    case "http":
      return { url: c.url || "", method: c.method || "GET", headers: headerLines(c.headers), body: c.body || "", timeout: String(c.timeoutSeconds || 30), expect: c.expectStatus ? String(c.expectStatus) : "" };
    case "webhook":
      // (the secret is the server's: sent back as it came, hidden, or asked to be replaced)
      return { url: c.url || "", payload: jsonText(c.payload), timeout: String(c.timeoutSeconds || 30), secret: c.secret || "", rotate: false, previousUntil: c.previousUntil || "" };
    case "script":
      return { function: c.function || "", input: jsonText(c.input) };
    case "mail":
      return { to: Array.isArray(c.to) ? c.to.join(", ") : c.to || "", subject: c.subject || "", body: c.body || "" };
    case "cleanup":
      return { collection: c.collection || "", filter: c.filter || "", days: c.olderThanDays ? String(c.olderThanDays) : "", field: c.field || "created", maxRows: String(c.maxRows || 10000) };
    case "sql":
      return { statement: c.statement || "" };
    default:
      return {};
  }
}

// and back: what is sent
function configOf(type, f) {
  switch (type) {
    case "query":
      return { collection: f.collection, filter: f.filter, sort: f.sort, limit: num(f.limit), params: jsonValue(f.params) };
    case "condition":
      return { expression: f.expression };
    case "email":
      return { template: f.template, to: f.to, data: jsonValue(f.data) };
    case "write":
      return { action: f.action, collection: f.collection, id: f.id, data: jsonValue(f.data) };
    case "http":
      return { url: f.url, method: f.method, headers: headerObject(f.headers), body: f.body, timeoutSeconds: num(f.timeout), expectStatus: num(f.expect) };
    case "webhook":
      return { url: f.url, payload: jsonValue(f.payload), timeoutSeconds: num(f.timeout), secret: f.secret, rotate: f.rotate || undefined };
    case "script":
      return { function: f.function, input: jsonValue(f.input) };
    case "mail":
      return { to: f.to, subject: f.subject, body: f.body };
    case "cleanup":
      return { collection: f.collection, filter: f.filter, field: f.field, olderThanDays: num(f.days), maxRows: num(f.maxRows) };
    case "sql":
      return { statement: f.statement };
    default:
      return {};
  }
}

let nextUid = 1;
const newStep = (type, taken, config, name, forEach) => {
  const base = typeOf(type).name;
  let unique = name || base;
  for (let n = 2; !name && taken.includes(unique); n++) unique = `${base}_${n}`;
  // `was`: the name it was saved under, so its saved secrets follow it when it is renamed
  return { uid: nextUid++, name: unique, was: name || "", type, forEach: forEach || "", f: formOf(type, config) };
};

// the steps of a job as the form holds them: a job of one type (made before steps) is one step
function stepsOf(job) {
  if (!job) return [];
  const stored = job.type === "steps" ? (job.config && job.config.steps) || [] : [{ type: job.type, config: job.config }];
  const out = [];
  for (const s of stored) out.push(newStep(s.type, out.map((x) => x.name), s.config, s.name, s.forEach));
  return out;
}

// Ready-made starting points: they fill the form, nothing is saved until "Make automation"
const EXAMPLES = [
  {
    label: "Email a reminder for each record",
    needs: "template",
    fill: (ctx) => ({
      name: "Send reminders",
      expression: "0 8 * * *",
      steps: [
        { type: "query", name: "due", config: { collection: ctx.collection, filter: "", limit: 100 } },
        { type: "condition", name: "any", config: { expression: "steps.due.count > 0" } },
        { type: "email", name: "remind", forEach: "steps.due.rows", config: { template: ctx.template, to: "{{ item.email }}" } },
      ],
    }),
  },
  { label: "Delete expired records", fill: (ctx) => ({ name: "Delete old records", expression: "0 3 * * *", steps: [{ type: "cleanup", config: { collection: ctx.collection, olderThanDays: 30, field: "created", maxRows: 10000 } }] }) },
  { label: "Ping a URL every 5 minutes", fill: () => ({ name: "Ping a URL", expression: "*/5 * * * *", retries: "2", steps: [{ type: "http", config: { url: "https://example.com/health", method: "GET" } }] }) },
  { label: "Weekly email", fill: () => ({ name: "Weekly reminder", expression: "0 8 * * 1", steps: [{ type: "mail", config: { subject: "Weekly reminder", body: "A reminder for this week." } }] }) },
];

function StepFields({ step, set, errors, at, collections, templates, sqlOn, functions }) {
  const f = step.f;
  const e = (key) => errors[`${at}.config.${key}`];
  const up = (key) => (value) => set({ f: { ...f, [key]: value } });
  const area = (key, rows, placeholder, mono = true) =>
    html`<textarea class=${"input" + (mono ? " mono" : "")} rows=${rows} value=${f[key]} onInput=${(ev) => up(key)(ev.target.value)} placeholder=${placeholder}></textarea>`;
  const names = collections.map((x) => ({ value: x.name, label: x.name }));
  const writable = collections.filter((x) => x.type !== "view").map((x) => ({ value: x.name, label: x.name }));
  switch (step.type) {
    case "query":
      return html`<div class="stack">
        <div class="grid-2">
          <${Field} label="Collection" error=${e("collection")}><${Select} value=${f.collection} onChange=${up("collection")} options=${names} /></${Field}>
          <${Field} label="At most" error=${e("limit")} help="Records, up to 1000"><${Input} type="number" min="1" max="1000" value=${f.limit} onInput=${up("limit")} /></${Field}>
        </div>
        <${Field} label="Filter" error=${e("filter")} help="Same syntax as API rules. Data from earlier steps goes in through parameters: status = {:status}"><${Input} class="mono" value=${f.filter} onInput=${up("filter")} placeholder="overdue = true && reminded = false" /></${Field}>
        <div class="grid-2">
          <${Field} label="Sort" error=${e("sort")}><${Input} class="mono" value=${f.sort} onInput=${up("sort")} placeholder="-created" /></${Field}>
          <${Field} label="Parameters" error=${e("params")} help="JSON, for the filter's {:names}">${area("params", 2, '{ "status": "open" }')}</${Field}>
        </div>
      </div>`;
    case "condition":
      return html`<${Field} label="Expression" error=${e("expression")} help="The steps after this one only run when it holds, otherwise the run ends here, successfully"><${Input} class="mono" value=${f.expression} onInput=${up("expression")} placeholder="steps.records.count > 0" /></${Field}>`;
    case "email":
      return html`<div class="stack">
        ${templates.length === 0 && html`<${Callout} kind="info">There is no template yet: make one under <a href="#/automations/templates">Templates</a>.</${Callout}>`}
        <div class="grid-2">
          <${Field} label="Template" error=${e("template")}><${Select} value=${f.template} onChange=${up("template")} options=${[{ value: "", label: "Choose…" }, ...templates.map((t) => ({ value: t.id, label: t.name }))]} /></${Field}>
          <${Field} label="To" error=${e("to")} help="Addresses, or where one is: {{ item.email }}"><${Input} class="mono" value=${f.to} onInput=${up("to")} placeholder="{{ item.email }}" /></${Field}>
        </div>
        <${Field} label="Extra data for the template" error=${e("data")} help="JSON. The template also sees item and steps.">${area("data", 2, '{ "total": "{{ steps.records.count }}" }')}</${Field}>
      </div>`;
    case "write":
      return html`<div class="stack">
        <div class="grid-2">
          <${Field} label="What" error=${e("action")}><${Select} value=${f.action} onChange=${up("action")} options=${ACTIONS} /></${Field}>
          <${Field} label="Collection" error=${e("collection")}><${Select} value=${f.collection} onChange=${up("collection")} options=${writable} /></${Field}>
        </div>
        ${f.action !== "create" && html`<${Field} label="Record id" error=${e("id")}><${Input} class="mono" value=${f.id} onInput=${up("id")} placeholder="{{ item.id }}" /></${Field}>`}
        ${f.action !== "delete" && html`<${Field} label="Fields to set" error=${e("data")} help="JSON">${area("data", 3, '{ "reminded": true }')}</${Field}>`}
      </div>`;
    case "http":
      return html`<div class="stack">
        <div class="grid-2">
          <${Field} label="Method"><${Select} value=${f.method} onChange=${up("method")} options=${METHODS} /></${Field}>
          <${Field} label="URL" error=${e("url")} help="Public addresses only. Data can fill in the path, not the server: https://example.com/orders/{{ item.id }}"><${Input} class="mono" value=${f.url} onInput=${up("url")} placeholder="https://example.com/hook" /></${Field}>
        </div>
        <${Field} label="Headers" error=${e("headers")} help="One per line, Name: value. Saved values are hidden; leave a line as it is to keep it.">${area("headers", 2, "Authorization: Bearer …")}</${Field}>
        ${f.method !== "GET" && html`<${Field} label="Body" error=${e("body")}>${area("body", 3, '{ "count": {{ steps.records.count }} }')}</${Field}>`}
        <div class="grid-2">
          <${Field} label="Time limit (seconds)" error=${e("timeoutSeconds")}><${Input} type="number" min="1" max="60" value=${f.timeout} onInput=${up("timeout")} /></${Field}>
          <${Field} label="Counts as success" error=${e("expectStatus")} help="A status code; empty means any 2xx"><${Input} type="number" min="100" max="599" value=${f.expect} onInput=${up("expect")} placeholder="2xx" /></${Field}>
        </div>
      </div>`;
    case "webhook":
      return html`<div class="stack">
        <${Field} label="URL" error=${e("url")} help="An https:// address that gets a JSON POST, signed so its receiver can check it came from here (Standard Webhooks). Data can fill in the path, not the server."><${Input} class="mono" value=${f.url} onInput=${up("url")} placeholder="https://example.com/hooks/orders" /></${Field}>
        <${Field} label="What to send" error=${e("payload")} help="JSON. Empty: what started the run (trigger) and the item.">${area("payload", 3, '{ "order": "{{ trigger.record.id }}" }')}</${Field}>
        <div class="grid-2">
          <${Field} label="Time limit (seconds)" error=${e("timeoutSeconds")}><${Input} type="number" min="1" max="60" value=${f.timeout} onInput=${up("timeout")} /></${Field}>
          <${Field} label="Signing secret" error=${e("secret")} help=${f.secret ? (f.previousUntil ? `The one before it also signs until ${f.previousUntil}` : "Shown once, when it was made") : "Made when you save, and shown once"}>
            ${f.secret ? html`<${Switch} checked=${f.rotate} onChange=${up("rotate")} label=${f.rotate ? "A new one is made when you save" : "Replace it"} />` : html`<span class="small muted">None yet</span>`}
          </${Field}>
        </div>
      </div>`;
    case "script":
      return html`<div class="stack">
        ${functions && functions.length === 0 && html`<${Callout} kind="info">No hook file registers a function yet: add <code>automationStep("name", (input, context) => …)</code> to a hook file.</${Callout}>`}
        <${Field} label="Function" error=${e("function")} help="Registered in a hook file with automationStep(). What it returns is this step's result."><${Select} value=${f.function} onChange=${up("function")} options=${[{ value: "", label: "Choose…" }, ...(functions || []).map((n) => ({ value: n, label: n })), ...(f.function && !(functions || []).includes(f.function) ? [{ value: f.function, label: f.function }] : [])]} /></${Field}>
        <${Field} label="Input" error=${e("input")} help="JSON, given to the function">${area("input", 3, '{ "id": "{{ trigger.record.id }}" }')}</${Field}>
      </div>`;
    case "mail":
      return html`<div class="stack">
        <${Field} label="To" error=${e("to")} help="Up to 10 addresses, separated by commas. Sent through Settings → Mail."><${Input} value=${f.to} onInput=${up("to")} placeholder="ops@example.com" /></${Field}>
        <${Field} label="Subject" error=${e("subject")}><${Input} value=${f.subject} onInput=${up("subject")} maxlength="200" /></${Field}>
        <${Field} label="Message" error=${e("body")} help="Plain text">${area("body", 4, "", false)}</${Field}>
      </div>`;
    case "cleanup":
      return html`<div class="stack">
        <${Field} label="Collection" error=${e("collection")} help="Its records are deleted, with their files and anything that cascades, as when you delete them yourself"><${Select} value=${f.collection} onChange=${up("collection")} options=${writable} /></${Field}>
        <div class="grid-2">
          <${Field} label="Older than (days)" error=${e("olderThanDays")} help="Leave empty to go by the filter only"><${Input} type="number" min="1" value=${f.days} onInput=${up("days")} /></${Field}>
          <${Field} label="Age is read from" error=${e("field")} help="The collection needs that date field"><${Select} value=${f.field} onChange=${up("field")} options=${["created", "updated"]} /></${Field}>
        </div>
        <${Field} label="Filter" error=${e("filter")} help="Same syntax as API rules. Both are applied when you give both."><${Input} class="mono" value=${f.filter} onInput=${up("filter")} placeholder="status = 'expired'" /></${Field}>
        <${Field} label="At most this many records per run" error=${e("maxRows")}><${Input} type="number" min="1" max="100000" value=${f.maxRows} onInput=${up("maxRows")} /></${Field}>
      </div>`;
    case "sql":
      return html`<div class="stack">
        ${sqlOn === false && html`<${Callout} kind="warn">The SQL console is off on this server. Start slonbase with <code>--sql-console</code> to save SQL steps.</${Callout}>`}
        <${Field} label="Statement" error=${e("statement")} help="Runs as the SQL console does. It is run as saved: data from other steps can't be put into it. Its values reach later steps as text.">${area("statement", 4, "SELECT email, total FROM slonbase.invoices WHERE due < now()")}</${Field}>
      </div>`;
    default:
      return null;
  }
}

function AutomationEditor({ job, copyOf, onClose, onDone }) {
  const editing = !!job && !copyOf; // a copy starts from a job but makes a new one
  const all = useStore(app, (st) => st.collections) || [];
  const collections = all.filter((c) => !c.system && !String(c.name).startsWith("_"));
  const role = useMyRole();
  const [templates, setTemplates] = useState([]);
  const [sqlOn, setSqlOn] = useState(null);
  const [name, setName] = useState(job ? (copyOf ? `Copy of ${job.name}`.slice(0, 60) : job.name) : "");
  const [expression, setExpression] = useState(job ? job.expression : "0 3 * * *");
  // what starts it: its schedule, or a record's change
  const byRecord = !!(job && job.trigger && job.trigger.type === "record");
  const [starts, setStarts] = useState(byRecord ? "record" : "schedule");
  const [watched, setWatched] = useState(byRecord ? job.trigger.collection || "" : "");
  const [actions, setActions] = useState(byRecord ? job.trigger.actions || [] : ["create"]);
  const [condition, setCondition] = useState(byRecord ? job.trigger.condition || "" : "");
  const [functions, setFunctions] = useState(null);
  const [made, setMade] = useState(null);
  const [rotateNotice, setRotateNotice] = useState(false);
  const [steps, setSteps] = useState(() => stepsOf(job));
  const [retries, setRetries] = useState(job ? String(job.retries || 0) : "0");
  const [retryDelay, setRetryDelay] = useState(job && job.retries ? String(job.retryDelaySeconds) : "30");
  const [notifyEmails, setNotifyEmails] = useState(job && job.notify ? job.notify.emails.join(", ") : "");
  const [notifyHook, setNotifyHook] = useState(job && job.notify ? job.notify.webhook : "");
  const [enabled, setEnabled] = useState(job ? job.enabled : true);
  const [errors, setErrors] = useState({});
  const [preview, setPreview] = useState(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.get("/api/templates").then((r) => setTemplates(r.items || [])).catch(() => {});
    api.get("/api/extensions").then((r) => setFunctions(r.stepFunctions || [])).catch(() => setFunctions([]));
  }, []);
  // SQL steps are for owners, and only when the server runs the SQL console
  useEffect(() => {
    if (role === "owner") api.get("/api/sql").then((i) => setSqlOn(!!i.enabled)).catch(() => setSqlOn(false));
  }, [role]);
  const types = STEP_TYPES.filter((t) => !t.owner || role === "owner");
  const first = (collections[0] && collections[0].name) || "";
  const add = (type) => {
    const config = ["query", "write", "cleanup"].includes(type) ? { collection: first } : {};
    setSteps((list) => [...list, newStep(type, list.map((s) => s.name), config)]);
  };
  const change = (uid, patch) => setSteps((list) => list.map((s) => (s.uid === uid ? { ...s, ...patch } : s)));
  const retype = (uid, type) =>
    setSteps((list) =>
      list.map((s) => {
        if (s.uid !== uid) return s;
        const config = ["query", "write", "cleanup"].includes(type) ? { collection: s.f.collection || first } : {};
        const renamed = s.name === typeOf(s.type).name ? newStep(type, list.filter((x) => x.uid !== uid).map((x) => x.name)).name : s.name;
        return { ...s, type, name: renamed, forEach: typeOf(type).repeat ? s.forEach : "", f: formOf(type, config) };
      })
    );
  const move = (i, by) =>
    setSteps((list) => {
      const next = [...list];
      const [s] = next.splice(i, 1);
      next.splice(i + by, 0, s);
      return next;
    });
  const start = (example) => {
    const filled = example.fill({ collection: first, template: (templates[0] && templates[0].id) || "" });
    setName(filled.name);
    setExpression(filled.expression);
    setRetries(filled.retries || "0");
    setRetryDelay("30");
    const out = [];
    for (const s of filled.steps) out.push(newStep(s.type, out.map((x) => x.name), s.config, s.name, s.forEach));
    setSteps(out);
  };
  const body = () => ({
    id: editing ? job.id : undefined,
    copyOf: copyOf || undefined,
    name,
    expression: starts === "record" ? "" : expression,
    trigger: starts === "record" ? { type: "record", collection: watched || first, actions, condition: condition.trim() } : { type: "schedule" },
    type: "steps",
    enabled,
    notify: { emails: notifyEmails, webhook: notifyHook.trim(), rotateSecret: rotateNotice || undefined },
    retries: Number(retries),
    retryDelaySeconds: retryDelay.trim() ? Number(retryDelay) : null,
    config: { steps: steps.map((s) => ({ name: s.name.trim(), was: s.was || undefined, type: s.type, forEach: s.forEach.trim() || undefined, config: configOf(s.type, s.f) })) },
  });
  // checked as it is being made (nothing is saved or run)
  const signature = JSON.stringify([name, expression, starts, watched, actions, condition, steps.map((s) => [s.name, s.type, s.forEach, s.f]), retries, retryDelay, notifyEmails, notifyHook]);
  useEffect(() => {
    // (an answer that comes back after the form changed again is not this form's)
    let stale = false;
    const t = setTimeout(async () => {
      try {
        const checked = await api.post("/api/cron-preview", body());
        if (stale) return;
        setPreview(checked);
        setErrors({});
      } catch (err) {
        if (stale) return;
        setPreview(null);
        setErrors(err.fieldErrors ? err.fieldErrors() : {});
      }
    }, 450);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [signature]);
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      const saved = editing ? await api.patch(`/api/automations/${enc(job.id)}`, body()) : await api.post("/api/automations", body());
      toast(editing ? "Saved." : "Automation made.");
      onDone();
      // signing secrets made by this save are shown now, and never again
      const secrets = saved && saved.secrets;
      const list = secrets ? [...Object.entries(secrets.steps || {}).map(([step, secret]) => ({ label: `Step ${step}`, secret })), ...(secrets.notify ? [{ label: "Failure notices", secret: secrets.notify }] : [])] : [];
      if (list.length) setMade(list);
      else onClose();
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  const examples = EXAMPLES.filter((x) => x.needs !== "template" || templates.length > 0);
  const toggleAction = (a) => setActions((list) => (list.includes(a) ? list.filter((x) => x !== a) : [...list, a]));
  if (made)
    return html`<${Modal} onClose=${onClose} size="mid" labelledBy="sheet-title">
      <${SheetHead} title="Signing secrets" subtitle="Give each to the receiver of its webhook. They are shown this once." onClose=${onClose} />
      <div class="sheet-body stack">
        <${Callout} kind="warn">Copy them now: they can't be shown again, only replaced.</${Callout}>
        ${made.map((m) => html`<${Field} label=${m.label}><${Input} class="mono" value=${m.secret} readonly onFocus=${(ev) => ev.target.select()} /></${Field}>`)}
        <div class="small muted">The receiver checks the <code>webhook-signature</code> header with it (Standard Webhooks: HMAC-SHA256 over id.timestamp.body).</div>
        <div class="row"><span class="spacer"></span><${Button} variant="primary" onClick=${onClose}>Done</${Button}></div>
      </div>
    </${Modal}>`;
  return html`<${Modal} onClose=${onClose} size="mid" labelledBy="sheet-title">
    <${SheetHead} title=${editing ? "Edit automation" : copyOf ? "Copy automation" : "New automation"} subtitle="Runs its steps in order, on a schedule or when a record changes, and shows in the history" onClose=${onClose} />
    <form class="sheet-body stack" onSubmit=${submit}>
      ${!job && html`<div class="stack tight"><span class="small muted">Start from an example</span><div class="chips">${examples.map((x) => html`<button type="button" class="chip selectable outline" onClick=${() => start(x)}>${x.label}</button>`)}</div></div>`}
      <${Field} label="Name" error=${errors.name} help="What it's for, e.g. “Remind customers of overdue invoices”"><${Input} value=${name} onInput=${setName} required autofocus maxlength="60" /></${Field}>
      <${Field} label="Runs" error=${errors["trigger.type"]}><${Select} value=${starts} onChange=${setStarts} options=${[{ value: "schedule", label: "On a schedule" }, { value: "record", label: "When a record changes" }]} /></${Field}>
      ${starts === "schedule" &&
      html`<${Field} label="Schedule" error=${errors.expression} help=${preview ? `${preview.description || "Custom schedule"}, on ${preview.timezone} time` : "minute hour day month weekday"}>
          <${Input} class="mono" value=${expression} onInput=${setExpression} required />
        </${Field}>
        <div class="chips">
          ${SCHEDULES.map(([label, value]) => html`<button type="button" class=${"chip selectable " + (expression === value ? "accent" : "outline")} onClick=${() => setExpression(value)}>${label}</button>`)}
        </div>`}
      ${starts === "record" &&
      html`<div class="grid-2">
          <${Field} label="Collection" error=${errors["trigger.collection"]} help="A change of one of its records starts a run, whatever made the change"><${Select} value=${collections.some((c) => c.id === watched) ? collections.find((c) => c.id === watched).name : watched || first} onChange=${setWatched} options=${collections.filter((c) => c.type !== "view").map((c) => ({ value: c.name, label: c.name }))} /></${Field}>
          <${Field} label="When a record is" error=${errors["trigger.actions"]}>
            <div class="chips" role="group" aria-label="Changes that start it">
              ${[["create", "Created"], ["update", "Updated"], ["delete", "Deleted"]].map(([a, label]) => html`<button type="button" aria-pressed=${actions.includes(a)} class=${"chip selectable " + (actions.includes(a) ? "accent" : "outline")} onClick=${() => toggleAction(a)}>${label}</button>`)}
            </div>
          </${Field}>
        </div>
        <${Field} label="Only if" error=${errors["trigger.condition"]} help=${html`Optional. The steps see the change as <code>trigger</code>: <code>trigger.action</code>, <code>trigger.record</code> and, for an update, the changed fields in <code>trigger.changed</code>.`}><${Input} class="mono" value=${condition} onInput=${setCondition} placeholder=${'trigger.record.status == "paid" and "status" in trigger.changed'} /></${Field}>
        <div class="small muted">A change this automation makes itself doesn't start it again. Changes made while it is off are not run later.</div>`}
      <div class="stack">
        <div class="row"><b>Steps</b><span class="small muted grow">Each step can use what the ones before it gave: <code>${"{{ steps.name.count }}"}</code></span></div>
        ${errors["config.steps"] && html`<span class="error small">${errors["config.steps"]}</span>`}
        ${steps.map((s, i) => {
          const at = `config.steps.${i}`;
          const t = typeOf(s.type);
          return html`<div class="card step-card" key=${s.uid}>
            <div class="card-head">
              <span class="field-type-badge">${i + 1}</span>
              <div class="grow grid-2">
                <${Field} label="Does" error=${errors[`${at}.type`]}><${Select} value=${s.type} onChange=${(v) => retype(s.uid, v)} options=${types.some((x) => x.value === s.type) ? types : [...types, t]} /></${Field}>
                <${Field} label="Called" error=${errors[`${at}.name`]} help=${`Gives steps.${s.name || "name"}: ${t.gives}`}><${Input} class="mono" value=${s.name} onInput=${(v) => change(s.uid, { name: v })} maxlength="30" /></${Field}>
              </div>
              <div class="row tight">
                <${IconButton} icon="chevronUp" label="Move up" size="sm" disabled=${i === 0} onClick=${() => move(i, -1)} />
                <${IconButton} icon="chevronDown" label="Move down" size="sm" disabled=${i === steps.length - 1} onClick=${() => move(i, 1)} />
                <${IconButton} icon="trash" label="Remove step" size="sm" onClick=${() => setSteps((list) => list.filter((x) => x.uid !== s.uid))} />
              </div>
            </div>
            <div class="card-body stack">
              ${errors[`${at}.config`] && html`<span class="error small">${errors[`${at}.config`]}</span>`}
              ${t.repeat && html`<${Field} label="Once for each item of" error=${errors[`${at}.forEach`]} help="A list from an earlier step; each item is item in this step. Empty: the step runs once."><${Input} class="mono" value=${s.forEach} onInput=${(v) => change(s.uid, { forEach: v })} placeholder="steps.records.rows" /></${Field}>`}
              <${StepFields} step=${s} set=${(patch) => change(s.uid, patch)} errors=${errors} at=${at} collections=${collections} templates=${templates} sqlOn=${sqlOn} functions=${functions} />
            </div>
          </div>`;
        })}
        ${steps.length < 20 &&
        html`<div class="stack tight"><span class="small muted">Add a step</span><div class="chips">${types.map((t) => html`<button type="button" class="chip selectable outline" onClick=${() => add(t.value)}><${Icon} name="plus" size=${12} /> ${t.label}</button>`)}</div></div>`}
      </div>
      <div class="grid-2">
        <${Field} label="If it fails, try again" error=${errors.retries} help="From the step that failed, waiting longer each time"><${Select} value=${retries} onChange=${setRetries} options=${[0, 1, 2, 3, 4, 5].map((n) => ({ value: String(n), label: n === 0 ? "Don't retry" : `${n} time${n === 1 ? "" : "s"}` }))} /></${Field}>
        ${Number(retries) > 0 && html`<${Field} label="First retry after (seconds)" error=${errors.retryDelaySeconds} help="Then twice as long, at most 5 minutes"><${Input} type="number" min="1" max="300" value=${retryDelay} onInput=${setRetryDelay} /></${Field}>`}
      </div>
      <${Field} label="Tell me when it fails (email)" error=${errors["notify.emails"]} help="Up to 10 addresses, through Settings → Mail. One message when it starts failing, one when it works again."><${Input} value=${notifyEmails} onInput=${setNotifyEmails} placeholder="ops@example.com" /></${Field}>
      <${Field} label="Tell me when it fails (webhook)" error=${errors["notify.webhook"]} help="An https:// address that gets a signed JSON POST (event, job, error). Saved URLs are hidden; leave it as it is to keep it. Its signing secret is shown once, when it is made."><${Input} class="mono" value=${notifyHook} onInput=${setNotifyHook} placeholder="https://hooks.example.com/…" /></${Field}>
      ${editing && job.notify && job.notify.webhook && notifyHook === job.notify.webhook && html`<${Switch} checked=${rotateNotice} onChange=${setRotateNotice} label=${rotateNotice ? "A new signing secret for notices is made when you save" : "Replace the notices' signing secret"} />`}
      ${preview && preview.next && preview.next.length > 0 && html`<div class="small muted">Next runs: ${preview.next.slice(0, 3).map((t) => formatDateIn(t, preview.timezone)).join(" · ")}</div>`}
      <div class="row"><${Switch} checked=${enabled} onChange=${setEnabled} label=${enabled ? "On" : "Off: it won't run"} /><span class="spacer"></span>
        <${Button} variant="ghost" onClick=${onClose}>Cancel</${Button}><${Button} variant="primary" type="submit" loading=${busy}>${editing ? "Save" : "Make automation"}</${Button}></div>
    </form>
  </${Modal}>`;
}

// what an automation made here does, in a few words
function stepsLabel(j) {
  if (j.kind !== "user") return j.kind === "backup" ? "Settings → Backups" : "hook";
  const count = j.type === "steps" ? ((j.config && j.config.steps) || []).length : 1;
  const what = count ? `${count} step${count === 1 ? "" : "s"}` : "made here";
  return j.enabled ? what : `${what} · off`;
}

function AutomationList() {
  const [jobs, setJobs] = useState(null);
  const [running, setRunning] = useState("");
  const [selected, setSelected] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [zone, setZone] = useState("UTC");
  const [paused, setPaused] = useState(false);
  const [editing, setEditing] = useState(null);
  const role = useMyRole();
  const canMake = role === "admin" || role === "owner";
  const load = () =>
    api
      .get("/api/automations")
      .then(setJobs)
      .catch(() => setJobs([]));
  useEffect(() => {
    load();
    api
      .get("/api/settings")
      .then((st) => {
        setZone((st.schedule && st.schedule.timezone) || "UTC");
        setPaused(!!(st.schedule && st.schedule.pauseJobs));
      })
      .catch(() => {});
  }, [refresh]);
  const run = async (j) => {
    setRunning(j.id);
    try {
      await api.post(`/api/automations/${enc(j.id)}`);
      toast(`Started ${j.kind === "user" ? j.name : j.id}.`);
      // the run is recorded in the background
      setTimeout(() => setRefresh((n) => n + 1), 1200);
    } catch (err) {
      toast.error(err);
    } finally {
      setRunning("");
    }
  };
  // stops every automation made here from running on its schedule (a maintenance window)
  const pause = async (on) => {
    try {
      await api.patch("/api/settings", { schedule: { pauseJobs: on } });
      setPaused(on);
      toast(on ? "Automations paused." : "Automations resumed.");
    } catch (err) {
      toast.error(err);
    }
  };
  const toggle = async (j, on) => {
    try {
      await api.patch(`/api/automations/${enc(j.id)}`, { enabled: on });
    } catch (err) {
      toast.error(err);
    }
    setRefresh((n) => n + 1);
  };
  const remove = async (j) => {
    if (!(await confirmDialog({ title: `Delete “${j.name}”?`, message: "It stops running. Its history stays until it ages out.", confirm: "Delete", danger: true }))) return;
    try {
      await api.delete(`/api/automations/${enc(j.id)}`);
      toast("Deleted.");
      if (selected === j.id) setSelected("");
      setRefresh((n) => n + 1);
    } catch (err) {
      toast.error(err);
    }
  };
  const label = (j) => (j.id === "__pbAutoBackup__" ? "Automatic backup" : j.kind === "user" ? j.name : j.id);
  // a statement among the steps is an owner's: admins see that it exists, not what it holds
  const mine = (j) => j.kind === "user" && canMake && (role === "owner" || (j.config && Object.keys(j.config).length > 0));
  const historyOf = jobs && jobs.find((j) => j.id === selected);
  return html`<div class="stack loose">
    <${Card} title="Automations" subtitle=${`Made here, in hook files (cronAdd) and the backup, on ${(jobs && jobs[0] && jobs[0].timezone) || zone} time`} icon="zap" actions=${html`${canMake && html`<${Switch} checked=${paused} onChange=${pause} label="Pause all" />`}${canMake && html`<${Button} size="sm" variant="primary" icon="plus" onClick=${() => setEditing({})}>New automation</${Button}>`}<a class="small" href="#/settings/backups">Time zone</a><${IconButton} icon="refresh" label="Refresh" onClick=${() => setRefresh((n) => n + 1)} />`}>
      ${paused && html`<${Callout} kind="warn">Automations made here are paused: none runs on its schedule until you turn this off. Run now still works. Jobs from hook files and the automatic backup are not affected.</${Callout}>`}
      ${jobs === null
        ? html`<${Skeleton} height=${80} />`
        : jobs.length === 0
        ? html`<${Empty} icon="zap" title="No automations">${canMake ? "Make one with New automation, " : ""}add jobs with <code>cronAdd()</code> in a hook file, or schedule backups.</${Empty}>`
        : html`<div class="table-wrap"><table class="table crons-table">
            <thead><tr><th>Automation</th><th>Runs</th><th>Next run</th><th>Last run</th><th></th></tr></thead>
            <tbody>
              ${jobs.map(
                (j) => html`<tr key=${j.id} class=${cx({ on: selected === j.id })}>
                  <td><b class=${j.kind === "user" ? "small" : "mono small"}>${label(j)}</b><div class="tiny muted">${stepsLabel(j)}</div></td>
                  <td>${j.trigger && j.trigger.type === "record" ? html`<span class="small">${j.description}</span>` : html`<code>${j.expression}</code>${j.description && html`<div class="tiny muted">${j.description}</div>`}`}</td>
                  <td>${j.next ? html`<span class="small">${relativeTime(j.next)}</span><div class="tiny muted mono" title=${`${formatDateIn(j.next, "UTC")}`}>${formatDateIn(j.next, j.timezone || zone)}</div>` : html`<span class="muted">${j.trigger && j.trigger.type === "record" && j.enabled ? "on a change" : "never"}</span>`}</td>
                  <td>${j.lastRun ? html`<${StatusChip} status=${j.lastRun.status} /><div class="tiny muted">${relativeTime(j.lastRun.started)} · ${ms(j.lastRun.durationMs)}</div>` : html`<span class="muted small">not yet</span>`}</td>
                  <td class="row tight end">
                    <${IconButton} icon="list" label="History" size="sm" onClick=${() => setSelected(selected === j.id ? "" : j.id)} />
                    ${mine(j) && html`<${Switch} checked=${j.enabled} onChange=${(on) => toggle(j, on)} ariaLabel=${`${j.name} is ${j.enabled ? "on" : "off"}`} /><${IconButton} icon="copy" label="Duplicate" size="sm" onClick=${() => setEditing({ ...j, copyOf: j.id })} /><${IconButton} icon="edit" label="Edit" size="sm" onClick=${() => setEditing(j)} /><${IconButton} icon="trash" label="Delete" size="sm" onClick=${() => remove(j)} />`}
                    <${Button} size="sm" icon="play" loading=${running === j.id} onClick=${() => run(j)}>Run now</${Button}>
                  </td>
                </tr>`
              )}
            </tbody>
          </table></div>`}
      <div class="small muted row"><${Icon} name="info" size=${14} /> Each runs once per minute it's due, on one instance.</div>
    </${Card}>
    ${editing && html`<${AutomationEditor} job=${editing.id ? editing : null} copyOf=${editing.copyOf || ""} onClose=${() => setEditing(null)} onDone=${() => setRefresh((n) => n + 1)} />`}
    <${Card} title=${historyOf ? `History: ${label(historyOf)}` : "History"} subtitle=${historyOf ? "Its last runs" : "The last runs of everything; choose one above to see only its own"} icon="list">
      <${JobHistory}
        kind=${historyOf ? (historyOf.kind === "backup" ? "backup" : "cron") : undefined}
        job=${historyOf ? (historyOf.kind === "backup" ? "auto" : historyOf.id) : undefined}
        refreshKey=${refresh}
        names=${Object.fromEntries((jobs || []).filter((j) => j.kind === "user").map((j) => [j.id, j.name]))}
      />
    </${Card}>
  </div>`;
}

const STARTER_BODY = `<p>Hello {{ item.name }},</p>
<p>This is a reminder.</p>`;

function TemplateEditor({ template, onClose, onDone }) {
  const editing = !!template;
  const [name, setName] = useState(editing ? template.name : "");
  const [description, setDescription] = useState(editing ? template.description : "");
  const [subject, setSubject] = useState(editing ? template.subject : "Hello {{ item.name }}");
  const [body, setBody] = useState(editing ? template.body : STARTER_BODY);
  const [sample, setSample] = useState(editing ? JSON.stringify(template.sample || {}, null, 2) : '{\n  "item": { "name": "Ann" }\n}');
  const [errors, setErrors] = useState({});
  const [preview, setPreview] = useState(null);
  const [problem, setProblem] = useState("");
  const [busy, setBusy] = useState(false);
  const [testTo, setTestTo] = useState("");
  const [sending, setSending] = useState(false);
  const data = () => {
    try {
      return JSON.parse(sample || "{}");
    } catch {
      return null;
    }
  };
  // the email as it would be, with the sample data (nothing is saved or sent)
  useEffect(() => {
    let stale = false;
    const t = setTimeout(async () => {
      const d = data();
      if (d === null) {
        setProblem("The sample data is not JSON.");
        return;
      }
      try {
        const rendered = await api.post("/api/templates/preview", { subject, body, data: d });
        if (stale) return;
        setPreview(rendered);
        setProblem("");
      } catch (err) {
        if (!stale) setProblem(err.message || "The template can't be rendered.");
      }
    }, 400);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [subject, body, sample]);
  const payload = () => ({ name, description, subject, body, sample: data() === null ? sample : data() });
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      if (editing) await api.patch(`/api/templates/${enc(template.id)}`, payload());
      else await api.post("/api/templates", payload());
      toast(editing ? "Saved." : "Template made.");
      onDone();
      onClose();
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  const sendTest = async () => {
    setSending(true);
    try {
      await api.post(`/api/templates/${enc(template.id)}/test`, { email: testTo, data: data() || undefined });
      toast(`Sent to ${testTo}.`);
    } catch (err) {
      toast.error(err);
    } finally {
      setSending(false);
    }
  };
  return html`<${Modal} onClose=${onClose} size="wide" labelledBy="sheet-title">
    <${SheetHead} title=${editing ? "Edit template" : "New template"} subtitle="An email written once and sent by automations with the data of each run" onClose=${onClose} />
    <form class="sheet-body stack" onSubmit=${submit}>
      <div class="grid-2">
        <${Field} label="Name" error=${errors.name}><${Input} value=${name} onInput=${setName} required autofocus maxlength="60" /></${Field}>
        <${Field} label="What it's for" error=${errors.description}><${Input} value=${description} onInput=${setDescription} maxlength="300" /></${Field}>
      </div>
      <${Field} label="Subject" error=${errors.subject}><${Input} value=${subject} onInput=${setSubject} required maxlength="300" /></${Field}>
      <div class="grid-2 template-edit">
        <div class="stack">
          <${Field} label="Body (HTML)" error=${errors.body} help=${html`Values are written with <code>${"{{ item.name }}"}</code> and escaped. <code>${"{% if %}"}</code>, <code>${"{% for %}"}</code> and filters such as <code>${"| date"}</code> work as in Jinja.`}>
            <textarea class="input mono" rows="14" value=${body} onInput=${(e) => setBody(e.target.value)} required></textarea>
          </${Field}>
          <${Field} label="Sample data" error=${errors.sample} help="JSON to preview with, shaped like what the automation gives: item, steps, and the step's extra data">
            <textarea class="input mono" rows="6" value=${sample} onInput=${(e) => setSample(e.target.value)}></textarea>
          </${Field}>
        </div>
        <div class="stack">
          <span class="small muted">Preview</span>
          ${problem ? html`<${Callout} kind="warn">${problem}</${Callout}>` : null}
          ${preview && html`<div class="mail-preview-subject"><span class="tiny muted">Subject</span> <b>${preview.subject}</b></div>`}
          <div class="mail-preview-frame template-frame"><iframe title="Preview of the email" sandbox="" srcdoc=${preview ? preview.html : ""} referrerpolicy="no-referrer"></iframe></div>
          ${editing &&
          html`<div class="row">
            <${Input} type="email" value=${testTo} onInput=${setTestTo} placeholder="you@example.com" aria-label="Send a test to" />
            <${Button} icon="mail" loading=${sending} disabled=${!testTo.includes("@")} onClick=${sendTest}>Send a test</${Button}>
          </div>
          <div class="tiny muted">The saved template is sent, with this sample data.</div>`}
        </div>
      </div>
      <div class="row"><span class="spacer"></span><${Button} variant="ghost" onClick=${onClose}>Cancel</${Button}><${Button} variant="primary" type="submit" loading=${busy}>${editing ? "Save" : "Make template"}</${Button}></div>
    </form>
  </${Modal}>`;
}

function TemplateList() {
  const [items, setItems] = useState(null);
  const [editing, setEditing] = useState(null);
  const [refresh, setRefresh] = useState(0);
  const role = useMyRole();
  const canMake = role === "admin" || role === "owner";
  useEffect(() => {
    api
      .get("/api/templates")
      .then((r) => setItems(r.items || []))
      .catch(() => setItems([]));
  }, [refresh]);
  const remove = async (t) => {
    if (!(await confirmDialog({ title: `Delete “${t.name}”?`, message: "Automations can no longer send it.", confirm: "Delete", danger: true }))) return;
    try {
      await api.delete(`/api/templates/${enc(t.id)}`);
      toast("Deleted.");
      setRefresh((n) => n + 1);
    } catch (err) {
      toast.error(err);
    }
  };
  return html`<div class="stack loose">
    <${Card} title="Email templates" subtitle="Written once, sent by automations with the data of each run. They use the mail layout of Settings → Mail." icon="mail" actions=${canMake && html`<${Button} size="sm" variant="primary" icon="plus" onClick=${() => setEditing({})}>New template</${Button}>`}>
      ${items === null
        ? html`<${Skeleton} height=${80} />`
        : items.length === 0
        ? html`<${Empty} icon="mail" title="No templates">${canMake ? "Make one with New template, then send it from an automation's “Send a template” step." : "Admins and owners make them."}</${Empty}>`
        : html`<div class="table-wrap"><table class="table">
            <thead><tr><th>Template</th><th>Subject</th><th>Changed</th><th></th></tr></thead>
            <tbody>
              ${items.map(
                (t) => html`<tr key=${t.id}>
                  <td><b class="small">${t.name}</b>${t.description && html`<div class="tiny muted">${t.description}</div>`}</td>
                  <td><code>${t.subject}</code></td>
                  <td><span class="small">${relativeTime(t.updated)}</span></td>
                  <td class="row tight end">${canMake && html`<${IconButton} icon="edit" label="Edit" size="sm" onClick=${() => setEditing(t)} /><${IconButton} icon="trash" label="Delete" size="sm" onClick=${() => remove(t)} />`}</td>
                </tr>`
              )}
            </tbody>
          </table></div>`}
    </${Card}>
    ${editing && html`<${TemplateEditor} template=${editing.id ? editing : null} onClose=${() => setEditing(null)} onDone=${() => setRefresh((n) => n + 1)} />`}
  </div>`;
}

const TABS = [
  { id: "automations", label: "Automations", icon: "zap" },
  { id: "templates", label: "Templates", icon: "mail" },
];

export function Automations({ tab }) {
  const active = TABS.some((t) => t.id === tab) ? tab : "automations";
  return html`<div class="page enter">
    <div class="page-head"><div class="grow"><div class="eyebrow"><${Icon} name="zap" size=${14} /> Schedules and steps</div><h1>Automations</h1></div></div>
    <${Tabs} tabs=${TABS} active=${active} onChange=${(id) => navigate(id === "automations" ? "/automations" : `/automations/${id}`)} />
    <div class="tab-body">${active === "templates" ? html`<${TemplateList} />` : html`<${AutomationList} />`}</div>
  </div>`;
}
