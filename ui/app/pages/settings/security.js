// Settings → Security & limits: rate limits, browsers' access (CORS), superusers' IPs, the
// trusted proxy, HTTPS only, batch requests and maintenance.
import { html, useStore, useState, useEffect } from "../../lib/core.js";
import { api } from "../../lib/api.js";
import { app } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { Button, IconButton, Field, Input, TextArea, Switch, Select, Callout } from "../../lib/ui.js";
import { Card } from "./shared.js";

const list = (v) =>
  v
    .split(/[,\n]/)
    .map((x) => x.trim())
    .filter(Boolean);

// a list edited one item per line, kept as typed (a new line stays while you type)
function ListArea({ value, onChange, ...rest }) {
  const [text, setText] = useState((value || []).join("\n"));
  return html`<${TextArea}
    code
    value=${text}
    onInput=${(v) => {
      setText(v);
      onChange(list(v));
    }}
    ...${rest}
  />`;
}

function CurrentIp() {
  const health = useStore(app, (st) => st.health);
  if (!health || !health.realIP) return null;
  return html`<div class="small muted row"><${Icon} name="info" size=${14} /> Your IP: <code>${health.realIP}</code>${health.possibleProxyHeader && html`<span> · behind a proxy? It sent <code>${health.possibleProxyHeader}</code></span>`}</div>`;
}

// which rate-limit rule a request would meet (the server's own matcher)
function RuleTester({ dirty }) {
  const [method, setMethod] = useState("POST");
  const [path, setPath] = useState("/api/collections/users/auth-with-password");
  const [audience, setAudience] = useState("@guest");
  const [res, setRes] = useState(null);
  const [error, setError] = useState("");
  const run = async () => {
    setError("");
    try {
      setRes(await api.post("/api/settings/rate-limits/explain", { method, path, audience }));
    } catch (err) {
      setRes(null);
      setError(err.message);
    }
  };
  useEffect(() => {
    run();
  }, []);
  return html`<div class="stack tight rule-tester">
    <div class="row wrap top">
      <${Field} label="Method"><${Select} value=${method} onChange=${setMethod} options=${["GET", "POST", "PATCH", "PUT", "DELETE"]} /></${Field}>
      <${Field} class="grow" label="Path"><${Input} class="mono" value=${path} onInput=${setPath} /></${Field}>
      <${Field} label="Caller"><${Select} value=${audience} onChange=${setAudience} options=${[{ value: "@guest", label: "Guest" }, { value: "@auth", label: "Signed in" }]} /></${Field}>
      <div class="label-pad"><${Button} size="sm" icon="zap" onClick=${run}>Test</${Button}></div>
    </div>
    ${error && html`<div class="small" style="color:var(--danger)">${error}</div>`}
    ${res &&
    html`<div class="small">
      ${!res.enabled && html`<span class="chip warn">rate limits are off</span> `}
      ${res.rule
        ? html`Rule <b>${res.rule.label}</b> (${res.rule.audience || "everyone"}): <b>${res.rule.maxRequests}</b> requests per <b>${res.rule.duration}</b> s.`
        : html`<span class="muted">No rule applies: this request isn't limited.</span>`}
      <div class="tiny muted">Checked in order: ${res.labels.map((l) => html`<code>${l}</code> `)}</div>
      ${dirty && html`<div class="tiny muted">Tests the saved rules: save first.</div>`}
    </div>`}
  </div>`;
}

export function Security({ s }) {
  const d = s.draft;
  const rules = d.rateLimits.rules;
  const setRule = (i, patch) => s.set("rateLimits.rules", rules.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const sec = d.security;
  const anyOrigin = sec.allowedOrigins.includes("*");
  const https = (d.meta.appURL || "").startsWith("https://");
  return html`<div class="stack loose">
    <${Card}
      title="Maintenance"
      subtitle=${sec.maintenance ? "On: only superusers get answers from the API" : "The API answers everyone"}
      icon="warning"
      class=${sec.maintenance ? "card-warn" : ""}
      actions=${html`<${Switch} checked=${sec.maintenance} onChange=${(v) => s.set("security.maintenance", v)} label=${sec.maintenance ? "On" : "Off"} />`}
    >
      ${sec.maintenance &&
      html`<${Field} label="Message" error=${s.errors["security.maintenanceMessage"]} help="Sent with the 503 answer; empty for “Down for maintenance. Try again shortly.”">
        <${Input} value=${sec.maintenanceMessage} onInput=${(v) => s.set("security.maintenanceMessage", v)} maxlength="300" placeholder="Upgrading the database, back at 14:00 UTC." />
      </${Field}>`}
    </${Card}>
    <${Card} title="Rate limits" subtitle="Requests per IP, per rule" icon="zap" actions=${html`<${Switch} checked=${d.rateLimits.enabled} onChange=${(v) => s.set("rateLimits.enabled", v)} label=${d.rateLimits.enabled ? "On" : "Off"} />`}>
      <div class="stack tight">
        <div class="row tiny faint" style="padding:0 4px" aria-hidden="true"><span style="flex:2">Label (path, or collection:action)</span><span style="flex:1">Audience</span><span style="width:110px">Max requests</span><span style="width:110px">Per seconds</span><span style="width:32px"></span></div>
        ${rules.map(
          (r, i) => html`<div class="row" key=${i}>
            <${Input} style="flex:2" class="mono" value=${r.label} onInput=${(v) => setRule(i, { label: v })} placeholder="/api/ or *:auth" aria-label=${`Rule ${i + 1}: label`} invalid=${!!s.errors[`rateLimits.rules.${i}.label`]} />
            <${Select} style="flex:1" aria-label=${`Rule ${i + 1}: audience`} value=${r.audience} onChange=${(v) => setRule(i, { audience: v })} options=${[{ value: "", label: "Everyone" }, { value: "@guest", label: "Guests" }, { value: "@auth", label: "Signed in" }]} />
            <${Input} style="width:110px" type="number" value=${r.maxRequests} onInput=${(v) => setRule(i, { maxRequests: Number(v) })} aria-label=${`Rule ${i + 1}: max requests`} />
            <${Input} style="width:110px" type="number" value=${r.duration} onInput=${(v) => setRule(i, { duration: Number(v) })} aria-label=${`Rule ${i + 1}: per seconds`} />
            <${IconButton} icon="trash" label="Remove" onClick=${() => s.set("rateLimits.rules", rules.filter((_, j) => j !== i))} />
          </div>`
        )}
        <div><${Button} size="sm" icon="plus" onClick=${() => s.set("rateLimits.rules", [...rules, { label: "", audience: "", duration: 10, maxRequests: 100 }])}>Add rule</${Button}></div>
      </div>
      <details class="small muted">
        <summary>How labels match</summary>
        <div class="stack tight" style="padding-top:8px">
          <div><code>*:auth</code> — signing in to any auth collection; <code>users:create</code> — creating records in <code>users</code> (also <code>list</code>, <code>view</code>, <code>update</code>, <code>delete</code>, <code>authRefresh</code>, <code>otp</code>…).</div>
          <div><code>POST /api/files</code> — a method and a path; <code>/api/</code> — any request whose path starts so. The most specific rule wins.</div>
        </div>
      </details>
      <details class="small muted" open>
        <summary>Test a request</summary>
        <div style="padding-top:8px"><${RuleTester} dirty=${s.dirty} /></div>
      </details>
      <${Field} label="Never limit these IPs" help="IPs or CIDR ranges, separated by commas">
        <${Input} class="mono" value=${(d.rateLimits.excludedIPs || []).join(", ")} onInput=${(v) => s.set("rateLimits.excludedIPs", list(v))} />
      </${Field}>
    </${Card}>
    <${Card} title="Browsers (CORS)" subtitle=${anyOrigin ? "Pages on any site may call the API" : sec.allowedOrigins.length ? `Pages on ${sec.allowedOrigins.length} site(s) may call the API` : "Only pages served from the API's own address"} icon="globe">
      <div class="row wrap">
        <${Switch} checked=${anyOrigin} onChange=${(v) => s.set("security.allowedOrigins", v ? ["*"] : [])} label="Any site (like PocketBase)" />
      </div>
      ${!anyOrigin &&
      html`<${Field} label="Allowed origins" error=${Object.keys(s.errors).find((k) => k.startsWith("security.allowedOrigins")) && "Each must be an origin such as https://app.example.com (no path)."} help="One per line, e.g. https://app.example.com or http://localhost:5173">
        <${ListArea} rows=${4} value=${sec.allowedOrigins} onChange=${(v) => s.set("security.allowedOrigins", v)} placeholder="https://app.example.com" />
      </${Field}>`}
      <div class="small muted">Apps that aren't browsers (servers, mobile apps) aren't concerned: they can always call the API.</div>
    </${Card}>
    <${Card} title="Superuser access" subtitle="Restrict where superusers can work from" icon="shield">
      <${Field} label="Allowed IPs" error=${s.errors.superuserIPs} help="IPs or CIDR ranges, separated by commas; empty allows any. Don't lock yourself out.">
        <${Input} class="mono" value=${(d.superuserIPs || []).join(", ")} onInput=${(v) => s.set("superuserIPs", list(v))} placeholder="10.0.0.0/8, 203.0.113.7" />
      </${Field}>
      <${CurrentIp} />
    </${Card}>
    <${Card} title="Signing in and auth emails" subtitle="Slowing down password guessing and email floods" icon="key">
      <div class="grid-3">
        <${Field} label="Lock out after (failures)" error=${s.errors["security.loginLockout.attempts"]} help="Password sign-ins for one account from one address; 0 is off">
          <${Input} type="number" min="0" max="1000" value=${sec.loginLockout.attempts} onInput=${(v) => s.set("security.loginLockout.attempts", Number(v))} />
        </${Field}>
        <${Field} label="For (minutes)" error=${s.errors["security.loginLockout.minutes"]} help="The window failures are counted in, and how long the lockout lasts">
          <${Input} type="number" min="1" max="60" value=${sec.loginLockout.minutes} onInput=${(v) => s.set("security.loginLockout.minutes", Number(v))} />
        </${Field}>
        <${Field} label="One auth email per (seconds)" error=${s.errors["security.authEmailInterval"]} help="Verification, password reset, email change and one-time passwords, per account">
          <${Input} type="number" min="5" max="86400" value=${sec.authEmailInterval} onInput=${(v) => s.set("security.authEmailInterval", Number(v))} />
        </${Field}>
      </div>
    </${Card}>
    <${Card} title="HTTPS only" subtitle="Strict-Transport-Security: browsers use HTTPS for a year" icon="lock" actions=${html`<${Switch} checked=${sec.hsts} onChange=${(v) => s.set("security.hsts", v)} label=${sec.hsts ? "On" : "Off"} />`}>
      ${sec.hsts && !https && html`<${Callout} kind="warn">Sent only when the application URL is https (Settings → Application).</${Callout}>`}
    </${Card}>
    <${Card} title="Trusted proxy" subtitle="Read the client IP from proxy headers" icon="server">
      <${Field} label="Headers" help="e.g. X-Forwarded-For, CF-Connecting-IP — only set these behind a proxy you control">
        <${Input} class="mono" value=${(d.trustedProxy.headers || []).join(", ")} onInput=${(v) => s.set("trustedProxy.headers", list(v))} />
      </${Field}>
      <${Field} label="Proxy addresses" help="IPs or ranges of your proxies, e.g. 10.0.0.0/8, 192.168.1.5. The headers are believed only from connections coming from these, so a client that reaches slonbase directly can't forge its address.">
        <${Input} class="mono" value=${(d.trustedProxy.peers || []).join(", ")} onInput=${(v) => s.set("trustedProxy.peers", list(v))} />
      </${Field}>
      ${(d.trustedProxy.headers || []).length > 0 && !(d.trustedProxy.peers || []).length && html`<${Callout} kind="warn">The headers are believed from any connection. List your proxies' addresses above unless nothing but the proxy can reach slonbase.</${Callout}>`}
      <${Switch} checked=${d.trustedProxy.useLeftmostIP} onChange=${(v) => s.set("trustedProxy.useLeftmostIP", v)} label="Use the leftmost IP (the original client) instead of the rightmost" />
    </${Card}>
    <${Card} title="Batch requests" subtitle="Several record writes in one transaction" icon="layers" actions=${html`<${Switch} checked=${d.batch.enabled} onChange=${(v) => s.set("batch.enabled", v)} label=${d.batch.enabled ? "On" : "Off"} />`}>
      ${d.batch.enabled &&
      html`<div class="grid-3 enter">
        <${Field} label="Max requests"><${Input} type="number" value=${d.batch.maxRequests} onInput=${(v) => s.set("batch.maxRequests", Number(v))} /></${Field}>
        <${Field} label="Timeout (seconds)"><${Input} type="number" value=${d.batch.timeout} onInput=${(v) => s.set("batch.timeout", Number(v))} /></${Field}>
        <${Field} label="Max body (bytes)" help="0 for 128 MB"><${Input} type="number" value=${d.batch.maxBodySize} onInput=${(v) => s.set("batch.maxBodySize", Number(v))} /></${Field}>
      </div>`}
    </${Card}>
  </div>`;
}

export function LogsSettings({ s }) {
  const d = s.draft;
  const off = d.logs.maxDays === 0;
  return html`<div class="stack loose">
    <${Card} title="Request logs" subtitle=${off ? "Off: requests aren't logged" : `Kept ${d.logs.maxDays} day(s)${d.logs.maxRows ? `, at most ${d.logs.maxRows.toLocaleString()} entries` : ""}`} icon="logs" actions=${html`<a class="btn sm" href="#/logs"><${Icon} name="logs" size=${15} /><span>Open the logs</span></a>`}>
      <div class="grid-3">
        <${Field} label="Keep for (days)" help="0 turns request logging off"><${Input} type="number" min="0" value=${d.logs.maxDays} onInput=${(v) => s.set("logs.maxDays", Number(v))} /></${Field}>
        <${Field} label="At most (entries)" help="0 for no limit; the oldest go first"><${Input} type="number" min="0" value=${d.logs.maxRows} onInput=${(v) => s.set("logs.maxRows", Number(v))} /></${Field}>
        <${Field} label="Minimum level" help="Requests that succeed are Info; those that fail, Error">
          <${Select} value=${String(d.logs.minLevel)} onChange=${(v) => s.set("logs.minLevel", Number(v))} options=${[{ value: "-4", label: "Debug" }, { value: "0", label: "Info" }, { value: "4", label: "Warning" }, { value: "8", label: "Error" }]} />
        </${Field}>
      </div>
      <div class="grid-3">
        <${Field} label="Slow requests (ms)" help="Slower ones are logged as warnings, whatever the level. 0 is off.">
          <${Input} type="number" min="0" value=${d.logs.slowRequestMs} onInput=${(v) => s.set("logs.slowRequestMs", Number(v))} placeholder="1000" />
        </${Field}>
        <${Field} label="Largest entry data (bytes)" help="Bigger entries keep what fits. 0 is 16 KiB.">
          <${Input} type="number" min="0" value=${d.logs.maxDataSize || 0} onInput=${(v) => s.set("logs.maxDataSize", Number(v))} />
        </${Field}>
      </div>
      <${Field} label="Don't log these paths" error=${Object.keys(s.errors).find((k) => k.startsWith("logs.excludePaths")) && "Each must start with /."} help="Paths starting so, one per line (server errors are logged all the same), e.g. /api/realtime">
        <${ListArea} rows=${3} value=${d.logs.excludePaths} onChange=${(v) => s.set("logs.excludePaths", v)} placeholder="/api/realtime" />
      </${Field}>
      <div class="row wrap" style="gap:22px">
        <${Switch} checked=${d.logs.logIP} onChange=${(v) => s.set("logs.logIP", v)} label="Record IP addresses" />
        <${Switch} checked=${d.logs.logAuthId} onChange=${(v) => s.set("logs.logAuthId", v)} label="Record who made each request" />
      </div>
    </${Card}>
  </div>`;
}
