// API reference for a collection, with copyable examples.
import { html, useState } from "../lib/core.js";
import { Icon } from "../lib/icons.js";
import { Segmented, CopyButton, Callout } from "../lib/ui.js";

function sample(field) {
  switch (field.type) {
    case "number":
      return 42;
    case "bool":
      return true;
    case "email":
      return "test@example.com";
    case "url":
      return "https://example.com";
    case "date":
      return "2025-01-01 10:00:00.000Z";
    case "select":
      return field.maxSelect > 1 ? (field.values || []).slice(0, 2) : (field.values || [])[0] || "";
    case "json":
      return { key: "value" };
    case "relation":
      return field.maxSelect > 1 ? ["RECORD_ID"] : "RECORD_ID";
    case "geoPoint":
      return { lat: 51.5, lon: -0.12 };
    default:
      return "text";
  }
}

function body(c) {
  const out = {};
  for (const f of c.fields) {
    if (f.system || f.type === "autodate" || f.type === "file") continue;
    out[f.name] = sample(f);
  }
  if (c.type === "auth") {
    out.email = "test@example.com";
    out.password = "a-strong-password";
    out.passwordConfirm = "a-strong-password";
  }
  return out;
}

function ruleText(rule) {
  return rule === null ? "superusers only" : rule === "" ? "anyone" : rule;
}

function endpoints(c) {
  const base = `/api/collections/${c.name}`;
  const data = JSON.stringify(body(c), null, 2);
  const list = [
    {
      method: "GET",
      path: `${base}/records`,
      title: "List records",
      rule: c.listRule,
      params: [
        ["page", "Page number (default 1)"],
        ["perPage", "Records per page (default 30, max 1000)"],
        ["sort", "e.g. -created,title"],
        ["filter", "e.g. (title ~ 'abc' && created > '2025-01-01')"],
        ["expand", "Relations to include, e.g. author,comments_via_post"],
        ["fields", "Fields to return, e.g. id,title"],
        ["skipTotal", "Skip counting for faster lists"],
      ],
      sdk: `const page = await pb.collection("${c.name}").getList(1, 30, {\n  filter: "created > '2025-01-01'",\n  sort: "-created",\n});`,
      curl: `curl "$URL${base}/records?page=1&perPage=30&sort=-created"`,
    },
    {
      method: "GET",
      path: `${base}/records/:id`,
      title: "View a record",
      rule: c.viewRule,
      params: [
        ["expand", "Relations to include"],
        ["fields", "Fields to return"],
      ],
      sdk: `const record = await pb.collection("${c.name}").getOne("RECORD_ID");`,
      curl: `curl "$URL${base}/records/RECORD_ID"`,
    },
  ];
  if (c.type !== "view") {
    list.push(
      {
        method: "POST",
        path: `${base}/records`,
        title: "Create a record",
        rule: c.createRule,
        note: "Send JSON, or multipart form data to upload files.",
        sdk: `const record = await pb.collection("${c.name}").create(${data.replace(/\n/g, "\n")});`,
        curl: `curl -X POST "$URL${base}/records" \\\n  -H "Content-Type: application/json" \\\n  -d '${JSON.stringify(body(c))}'`,
      },
      {
        method: "PATCH",
        path: `${base}/records/:id`,
        title: "Update a record",
        rule: c.updateRule,
        note: "Only the fields you send change. Use field+ / field- to append or remove files and relation ids.",
        sdk: `const record = await pb.collection("${c.name}").update("RECORD_ID", { /* changes */ });`,
        curl: `curl -X PATCH "$URL${base}/records/RECORD_ID" \\\n  -H "Content-Type: application/json" \\\n  -d '{}'`,
      },
      {
        method: "DELETE",
        path: `${base}/records/:id`,
        title: "Delete a record",
        rule: c.deleteRule,
        sdk: `await pb.collection("${c.name}").delete("RECORD_ID");`,
        curl: `curl -X DELETE "$URL${base}/records/RECORD_ID"`,
      },
      {
        method: "GET",
        path: "/api/realtime",
        title: "Subscribe to changes",
        rule: c.listRule,
        note: "Server-sent events; each subscriber only gets records its list/view rules allow.",
        sdk: `pb.collection("${c.name}").subscribe("*", (e) => {\n  console.log(e.action, e.record);\n});`,
        curl: `# realtime uses server-sent events; see the SDK example`,
      }
    );
  }
  if (c.type === "auth") {
    list.push(
      {
        method: "POST",
        path: `${base}/auth-with-password`,
        title: "Sign in with a password",
        rule: c.authRule,
        sdk: `const auth = await pb.collection("${c.name}").authWithPassword("test@example.com", "a-strong-password");`,
        curl: `curl -X POST "$URL${base}/auth-with-password" \\\n  -H "Content-Type: application/json" \\\n  -d '{"identity":"test@example.com","password":"a-strong-password"}'`,
      },
      {
        method: "POST",
        path: `${base}/auth-refresh`,
        title: "Refresh the auth token",
        sdk: `await pb.collection("${c.name}").authRefresh();`,
        curl: `curl -X POST "$URL${base}/auth-refresh" -H "Authorization: TOKEN"`,
      },
      {
        method: "GET",
        path: `${base}/auth-methods`,
        title: "List sign-in methods",
        sdk: `const methods = await pb.collection("${c.name}").listAuthMethods();`,
        curl: `curl "$URL${base}/auth-methods"`,
      },
      {
        method: "POST",
        path: `${base}/request-password-reset`,
        title: "Request a password reset",
        sdk: `await pb.collection("${c.name}").requestPasswordReset("test@example.com");`,
        curl: `curl -X POST "$URL${base}/request-password-reset" -d '{"email":"test@example.com"}' -H "Content-Type: application/json"`,
      }
    );
  }
  return list;
}

export function ApiDocs({ collection: c }) {
  const [open, setOpen] = useState(0);
  const [lang, setLang] = useState("sdk");
  const list = endpoints(c);
  return html`<div class="stack" style="max-width:960px">
    <div class="row">
      <${Callout}>
        slonbase speaks PocketBase's API, so the official <a href="https://github.com/pocketbase/js-sdk" target="_blank" rel="noopener">JavaScript</a>
        and Dart SDKs work as they are: <code>new PocketBase("${location.origin}")</code>.
      </${Callout}>
    </div>
    <div class="row"><span class="grow section-title">Endpoints</span><${Segmented} value=${lang} onChange=${setLang} options=${[{ value: "sdk", label: "JS SDK" }, { value: "curl", label: "curl" }]} /></div>
    <div class="stack tight stagger">
      ${list.map(
        (e, i) => html`<div style=${{ "--i": i }}>
          <div class=${"endpoint" + (open === i ? " open" : "")} onClick=${() => setOpen(open === i ? -1 : i)}>
            <span class=${"method " + e.method}>${e.method}</span>
            <code class="grow ellipsis">${e.path}</code>
            <span class="small muted nowrap">${e.title}</span>
            <${Icon} name=${open === i ? "chevronUp" : "chevronDown"} size=${16} class="faint" />
          </div>
          ${open === i &&
          html`<div class="card pad stack enter" style="margin-top:6px">
            ${e.rule !== undefined && html`<div class="row small"><${Icon} name="shield" size=${15} /> Access: <code>${ruleText(e.rule)}</code></div>`}
            ${e.note && html`<p class="small muted">${e.note}</p>`}
            ${e.params &&
            html`<table class="grid" style="font-size:12.5px">
              <tbody>${e.params.map(([k, v]) => html`<tr style="cursor:default"><td style="width:140px"><code>${k}</code></td><td class="muted">${v}</td></tr>`)}</tbody>
            </table>`}
            <div style="position:relative">
              <pre class="code">${lang === "sdk" ? e.sdk : e.curl.replace(/\$URL/g, location.origin)}</pre>
              <span style="position:absolute;top:8px;right:8px"><${CopyButton} text=${lang === "sdk" ? e.sdk : e.curl.replace(/\$URL/g, location.origin)} /></span>
            </div>
          </div>`}
        </div>`
      )}
    </div>
  </div>`;
}
