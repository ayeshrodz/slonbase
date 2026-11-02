// Differential test: the same requests to PocketBase and slonbase, compared as JSON (values
// that differ by nature, such as times, tokens and generated ids, are compared by type).
//
//   node diff.mjs <pocketbase url> <slonbase url> [--seed] [--fuzz N] [--only substring] [-v]
//
// --seed creates the same collections and records (fixed ids) on both; --fuzz N adds N random
// HTML documents (FUZZ_SEED picks the sequence) whose excerpts are compared at 8 settings.
// Both servers need the superuser COMPAT_EMAIL / COMPAT_PASSWORD, and batch requests enabled.
// See run.sh, which starts both and runs this.
const [pbURL, sbURL, ...flags] = process.argv.slice(2);
const seed = flags.includes("--seed");
const only = flags.includes("--only") ? flags[flags.indexOf("--only") + 1] : null;
const verbose = flags.includes("-v");
const EMAIL = process.env.COMPAT_EMAIL || "admin@example.com";
const PASSWORD = process.env.COMPAT_PASSWORD || "admin-password-1";
const fuzzCount = flags.includes("--fuzz") ? Number(flags[flags.indexOf("--fuzz") + 1]) : 0;

// random HTML from a seeded generator: tags (every excluded and inline one, blocks, tables,
// foreign content), entities, comments, unclosed and misnested tags, Unicode and whitespace
function rng(seed) {
  let x = seed >>> 0 || 1;
  return () => ((x ^= x << 13), (x ^= x >>> 17), (x ^= x << 5), (x >>> 0) / 4294967296);
}
const TAGS = ["p", "div", "span", "b", "i", "em", "strong", "a", "code", "br", "ul", "li", "ol", "h1", "h2",
  "table", "tr", "td", "th", "tbody", "blockquote", "pre", "img", "svg", "math", "button", "form", "input",
  "textarea", "select", "option", "script", "style", "template", "iframe", "noscript", "head", "title",
  "label", "small", "sub", "sup", "time", "q", "cite", "abbr", "dialog", "picture", "object", "embed",
  "section", "article", "header", "footer", "hr", "body", "html", "frameset", "caption", "colgroup", "col",
  "dl", "dt", "dd", "figure", "figcaption", "mark", "del", "ins", "s", "u", "font", "center", "marquee"];
const TEXTS = ["hello", "world", "  spaced   out  ", "\t\ttab", "\nnew\nline", "\r\ncrlf", "&amp;", "&lt;", "&gt;",
  "&nbsp;", "&#160;", "&copy;", "&unknown;", "&#x1F600;", "Привет", "日本語", "🎉", "x", "long text that goes on and on",
  "\f", "a&b", "<", ">", "\u00a0", "\u2003em", "word", "123", "ünïcödé", "\u200bzero"];
function randomHTML(r) {
  const pick = (a) => a[Math.floor(r() * a.length)];
  let out = "";
  const n = 1 + Math.floor(r() * 25);
  const open = [];
  for (let i = 0; i < n; i++) {
    const k = r();
    if (k < 0.35) out += pick(TEXTS);
    else if (k < 0.65) { const t = pick(TAGS); out += `<${t}${r() < 0.2 ? ' class="c"' : ""}>`; open.push(t); }
    else if (k < 0.85 && open.length) { out += `</${open.pop()}>`; }
    else if (k < 0.9) out += "<!-- c -->";
    else if (k < 0.95) out += `</${pick(TAGS)}>`;
    else out += " ";
  }
  if (r() < 0.5) while (open.length) out += `</${open.pop()}>`;
  return out;
}


async function login(url) {
  const res = await fetch(url + "/api/collections/_superusers/auth-with-password", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ identity: EMAIL, password: PASSWORD }),
  });
  if (!res.ok) throw new Error(`${url} login: ${res.status} ${await res.text()}`);
  return (await res.json()).token;
}

async function call(url, token, method, path, body) {
  const res = await fetch(url + path, {
    method,
    headers: { "content-type": "application/json", ...(token ? { authorization: token } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = text;
  }
  return { status: res.status, json };
}

// the HTML corpus for excerpts: real editor output and the parser's corner cases
export const HTML = [
  "<p>Hello <b>world</b>!</p><p>Second   paragraph</p>",
  "<h1>Title</h1><p>Body with <a href='#'>a link</a> and <code>code</code>.</p>",
  "plain text without tags",
  "  leading and trailing   spaces  ",
  "<p>a<p>b<p>c",
  "<div><p>nested <span>inline <em>deep</em></span></p><ul><li>one</li><li>two</li></ul></div>",
  "<p>entities: &amp; &lt;tag&gt; &quot;q&quot; &#39;s&#39; &copy; &nbsp;x&nbsp;&nbsp;y</p>",
  "<style>p{color:red}</style><script>alert(1)</script><p>visible</p>",
  "<p>before<img src=x alt=image>after</p><svg><text>svg text</text></svg><p>end</p>",
  "<button>btn</button><form><input value=v><textarea>ta</textarea></form><select><option>o</option></select>ok",
  "<template><p>hidden</p></template><dialog>d</dialog><iframe>if</iframe>shown",
  "<table><tr><td>cell1</td><td>cell2</td></tr></table>",
  "<table>foster<tr><td>x</td></tr></table>",
  "<p>line<br>break<br/>again</p>",
  "<!-- a comment --><p>after comment</p>",
  "<title>head title</title><p>body</p>",
  "<p>Привет, мир! Это длинный текст на русском языке для проверки обрезки.</p>",
  "<p>日本語のテキストです。これは長い文章で、切り詰めのテストに使います。</p>",
  "<p>emoji 🎉🎉🎉 party 🥳 time</p>",
  "<p>tabs\tand\nnewlines\r\nand\fform feeds</p>",
  "<noscript>no script text</noscript><p>after</p>",
  "<p>unclosed <b>bold <i>italic</p> after",
  "<pre>  pre   formatted\n  text  </pre>",
  "<p>a</p>\n\n<p>b</p>",
  "<blockquote>quote</blockquote><hr><p>after hr</p>",
  "<p>" + "word ".repeat(60) + "</p>",
  "<p><b>" + "x".repeat(120) + "</b></p>",
  "<p>short</p>",
  "",
  "<p></p><p> </p>",
  "<span>in</span><span>line</span><div>block</div><span>again</span>",
  "<p>&amp;amp; &unknown; &#x1F600; &#128512;</p>",
  "<math><mi>x</mi></math> math",
  "<p>x</p>" + "<p>y</p>".repeat(30),
];

const IDS = {
  author1: "author000000001",
  author2: "author000000002",
};
const postId = (i) => "post" + String(i).padStart(11, "0");

async function seedFuzz(url) {
  const token = await login(url);
  await call(url, token, "DELETE", "/api/collections/fuzz");
  let r = await call(url, token, "POST", "/api/collections", {
    name: "fuzz", type: "base", listRule: "", viewRule: "", fields: [{ name: "html", type: "editor" }],
  });
  if (r.status !== 200) throw new Error(`${url} fuzz: ${JSON.stringify(r.json)}`);
  const rand = rng(Number(process.env.FUZZ_SEED || 12345));
  const requests = [];
  for (let i = 0; i < fuzzCount; i++) {
    requests.push({ method: "POST", url: "/api/collections/fuzz/records",
      body: { id: "fuzz" + String(i).padStart(11, "0"), html: randomHTML(rand) } });
  }
  for (let i = 0; i < requests.length; i += 50) {
    r = await call(url, token, "POST", "/api/batch", { requests: requests.slice(i, i + 50) });
    if (r.status !== 200) throw new Error(`${url} fuzz batch: ${JSON.stringify(r.json).slice(0, 300)}`);
  }
}

const DEEP = {
  ok510: "<div>".repeat(510) + "x",
  div511: "<div>".repeat(511) + "x",
  bold600: "<b>".repeat(600) + "x",
  table300: "<table><tr><td>".repeat(300) + "x",
  p2000: "<p>a".repeat(2000),
  closed: "<div>".repeat(600) + "x" + "</div>".repeat(600),
  fine: "<p>fine</p>",
};

async function seedDeep(url) {
  const token = await login(url);
  await call(url, token, "DELETE", "/api/collections/deepdocs");
  const r = await call(url, token, "POST", "/api/collections", {
    name: "deepdocs", type: "base", listRule: "", viewRule: "",
    fields: [{ name: "name", type: "text" }, { name: "html", type: "editor" }],
  });
  if (r.status !== 200) throw new Error(`${url} deepdocs: ${JSON.stringify(r.json)}`);
  for (const [i, [name, html]] of Object.entries(DEEP).entries()) {
    const c = await call(url, token, "POST", "/api/collections/deepdocs/records", { id: "deep" + String(i).padStart(11, "0"), name, html });
    if (c.status !== 200) throw new Error(`${url} deep ${name}: ${JSON.stringify(c.json)}`);
  }
}

async function seedServer(url) {
  await seedDeep(url);
  const token = await login(url);
  for (const name of ["posts", "authors"]) {
    await call(url, token, "DELETE", `/api/collections/${name}`);
  }
  let r = await call(url, token, "POST", "/api/collections", {
    name: "authors",
    type: "base",
    listRule: "",
    viewRule: "",
    fields: [
      { name: "name", type: "text" },
      { name: "bio", type: "editor" },
    ],
  });
  if (r.status !== 200) throw new Error(`${url} authors: ${JSON.stringify(r.json)}`);
  const authors = r.json;
  r = await call(url, token, "POST", "/api/collections", {
    name: "posts",
    type: "base",
    listRule: "",
    viewRule: "",
    fields: [
      { name: "title", type: "text" },
      { name: "body", type: "editor" },
      { name: "summary", type: "text" },
      { name: "views", type: "number" },
      { name: "tags", type: "select", values: ["a", "b", "c"], maxSelect: 3 },
      { name: "author", type: "relation", collectionId: authors.id, maxSelect: 1 },
      { name: "meta", type: "json" },
    ],
  });
  if (r.status !== 200) throw new Error(`${url} posts: ${JSON.stringify(r.json)}`);
  for (const [i, id] of [IDS.author1, IDS.author2].entries()) {
    r = await call(url, token, "POST", "/api/collections/authors/records", {
      id,
      name: `Author ${i + 1}`,
      bio: `<p>Bio of <b>author ${i + 1}</b>, who writes a lot about many things.</p>`,
    });
    if (r.status !== 200) throw new Error(`${url} author: ${JSON.stringify(r.json)}`);
  }
  for (const [i, html] of HTML.entries()) {
    r = await call(url, token, "POST", "/api/collections/posts/records", {
      id: postId(i),
      title: `Post ${i}`,
      body: html,
      summary: html,
      views: i,
      tags: ["a", "b"].slice(0, i % 3),
      author: i % 2 ? IDS.author1 : IDS.author2,
      meta: { n: i, nested: { deep: "x", list: [{ k: 1, v: 2 }] } },
    });
    if (r.status !== 200) throw new Error(`${url} post ${i}: ${JSON.stringify(r.json)}`);
  }
}

// the requests: [name, method, path, body?, auth?]
function cases() {
  const out = [];
  const one = postId(0);
  const sort = "&sort=id&perPage=100";
  const list = (q) => `/api/collections/posts/records?${q}${sort}`;
  out.push(["plain pick", "GET", list("fields=id,title")]);
  out.push(["star", "GET", list("fields=*")]);
  out.push(["star and expand pick", "GET", list("expand=author&fields=*,expand.author.name")]);
  out.push(["only expand name", "GET", list("expand=author&fields=id,expand.author.name")]);
  out.push(["expand whole", "GET", list("expand=author&fields=id,expand")]);
  out.push(["expand nested star", "GET", list("expand=author&fields=id,expand.author.*")]);
  out.push(["json nested", "GET", list("fields=id,meta.nested.deep")]);
  out.push(["json array of maps", "GET", list("fields=id,meta.nested.list.k")]);
  out.push(["unknown field", "GET", list("fields=id,nope")]);
  out.push(["spaces", "GET", list("fields= id , title ")]);
  out.push(["view one", "GET", `/api/collections/posts/records/${one}?fields=id,title`]);
  out.push(["view expand", "GET", `/api/collections/posts/records/${one}?expand=author&fields=id,expand.author.bio:excerpt(10)`]);
  out.push(["list envelope keys", "GET", list("fields=page")]);
  for (const max of [1, 5, 10, 20, 50, 100, 200]) {
    out.push([`excerpt ${max}`, "GET", list(`fields=id,body:excerpt(${max})`)]);
    out.push([`excerpt ${max} ellipsis`, "GET", list(`fields=id,body:excerpt(${max},true)`)]);
  }
  out.push(["excerpt on text field", "GET", list("fields=id,summary:excerpt(30,true)")]);
  out.push(["excerpt on number", "GET", list("fields=id,views:excerpt(3)")]);
  out.push(["excerpt on array", "GET", list("fields=id,tags:excerpt(3)")]);
  out.push(["excerpt with star", "GET", list("fields=*,body:excerpt(15,1)")]);
  out.push(["excerpt spaces in args", "GET", list("fields=id,body:excerpt( 12 , true )")]);
  out.push(["excerpt ellipsis false", "GET", list("fields=id,body:excerpt(12,false)")]);
  out.push(["excerpt ellipsis t", "GET", list("fields=id,body:excerpt(12,t)")]);
  out.push(["excerpt ellipsis junk", "GET", list("fields=id,body:excerpt(12,junk)")]);
  out.push(["excerpt float max", "GET", list("fields=id,body:excerpt(12.7)")]);
  out.push(["excerpt zero", "GET", list("fields=id,body:excerpt(0)")]);
  out.push(["excerpt negative", "GET", list("fields=id,body:excerpt(-3)")]);
  out.push(["excerpt no args", "GET", list("fields=id,body:excerpt()")]);
  out.push(["excerpt too many args", "GET", list("fields=id,body:excerpt(1,2,3)")]);
  out.push(["unknown modifier", "GET", list("fields=id,body:nope(1)")]);
  out.push(["excerpt bare", "GET", list("fields=id,body:excerpt")]);
  out.push(["auth fields", "POST", "/api/collections/_superusers/auth-with-password?fields=record.email", { identity: EMAIL, password: PASSWORD }, false]);
  out.push(["auth fields token", "POST", "/api/collections/_superusers/auth-with-password?fields=token,record.collectionName", { identity: EMAIL, password: PASSWORD }, false]);
  out.push(["error response not picked", "GET", "/api/collections/posts/records/missing000000000?fields=id"]);
  out.push(["health fields", "GET", "/api/health?fields=message"]);
  out.push(["create picked", "POST", "/api/collections/posts/records?fields=title,body:excerpt(4,true)", { id: "newpost00000001", title: "new", body: "<p>Fresh post body</p>" }]);
  out.push(["update picked", "PATCH", `/api/collections/posts/records/${postId(3)}?fields=id,views`, { views: 99 }]);
  out.push(["expand excerpt", "GET", list("expand=author&fields=id,expand.author.bio:excerpt(12,true)")]);
  out.push(["star modifier", "GET", list("fields=*:excerpt(4)")]);
  out.push(["duplicate names", "GET", list("fields=title,title:excerpt(3)")]);
  out.push(["duplicate names reversed", "GET", list("fields=title:excerpt(3),title")]);
  out.push(["trailing dot", "GET", list("expand=author&fields=id,expand.")]);
  out.push(["double dot", "GET", list("expand=author&fields=id,expand..author")]);
  out.push(["settings fields", "GET", "/api/settings?fields=meta.hideControls,batch"]);
  out.push(["collection view fields", "GET", "/api/collections/posts?fields=name,fields.name"]);
  out.push(["batch sub fields", "POST", "/api/batch", { requests: [
    { method: "PATCH", url: `/api/collections/posts/records/${postId(5)}?fields=id,body:excerpt(6,true)`, body: { views: 5 } },
    { method: "PATCH", url: `/api/collections/posts/records/${postId(6)}`, body: { views: 6 } },
  ] }]);
  out.push(["batch top fields", "POST", "/api/batch?fields=status", { requests: [
    { method: "PATCH", url: `/api/collections/posts/records/${postId(7)}`, body: { views: 7 } },
  ] }]);
  for (const name of Object.keys(DEEP)) {
    out.push([`deep ${name}`, "GET", `/api/collections/deepdocs/records?filter=name='${name}'&fields=name,html:excerpt(20)`]);
  }
  out.push(["deep list (one fails: all unpicked)", "GET", "/api/collections/deepdocs/records?sort=id&fields=name,html:excerpt(20)"]);
  out.push(["deep view", "GET", "/api/collections/deepdocs/records/deep00000000001?fields=name,html:excerpt(20)"]);
  out.push(["deep without modifier", "GET", "/api/collections/deepdocs/records?sort=id&fields=name"]);
  if (fuzzCount) {
    for (const m of ["1", "3", "10", "25", "80", "400", "3,true", "25,true"]) {
      for (let page = 1; page <= Math.ceil(fuzzCount / 500); page++) {
        out.push([`fuzz excerpt(${m}) page ${page}`, "GET",
          `/api/collections/fuzz/records?sort=id&perPage=500&page=${page}&fields=id,html:excerpt(${m})`]);
      }
    }
  }
  out.push(["collections list fields", "GET", "/api/collections?fields=name&sort=name&filter=" + encodeURIComponent("name~'posts'")]);
  return out;
}

// values that differ between the two servers by nature
function normalize(v, key) {
  if (Array.isArray(v)) return v.map((x) => normalize(x, key));
  if (v && typeof v === "object") {
    const o = {};
    for (const k of Object.keys(v).sort()) {
      if (["created", "updated", "token", "collectionId"].includes(k)) {
        o[k] = typeof v[k];
        continue;
      }
      o[k] = normalize(v[k], k);
    }
    return o;
  }
  return v;
}

async function main() {
  if (seed) {
    await seedServer(pbURL);
    await seedServer(sbURL);
    console.log("seeded");
  }
  if (fuzzCount) {
    await seedFuzz(pbURL);
    await seedFuzz(sbURL);
    console.log(`fuzz: ${fuzzCount} documents`);
  }
  const tokens = [await login(pbURL), await login(sbURL)];
  let failed = 0;
  let passed = 0;
  for (const [name, method, path, body, auth = true] of cases()) {
    if (only && !name.includes(only)) continue;
    const [a, b] = await Promise.all(
      [pbURL, sbURL].map((u, i) => call(u, auth ? tokens[i] : null, method, path, body))
    );
    const statusOk = a.status === b.status;
    // error bodies differ in wording; compare their status only
    const ja = a.status < 300 ? JSON.stringify(normalize(a.json)) : "";
    const jb = b.status < 300 ? JSON.stringify(normalize(b.json)) : "";
    if (statusOk && ja === jb) {
      passed++;
      if (verbose) console.log("ok  ", name);
      continue;
    }
    failed++;
    console.log("DIFF", name, `(${method} ${path})`);
    console.log("  pocketbase:", a.status, JSON.stringify(a.json).slice(0, 600));
    console.log("  slonbase:  ", b.status, JSON.stringify(b.json).slice(0, 600));
    if (Array.isArray(a.json?.items) && Array.isArray(b.json?.items)) {
      for (let i = 0; i < Math.max(a.json.items.length, b.json.items.length); i++) {
        const x = JSON.stringify(normalize(a.json.items[i]));
        const y = JSON.stringify(normalize(b.json.items[i]));
        if (x !== y) {
          console.log(`  item ${i}:\n    pb ${x}\n    sb ${y}`);
        }
      }
    }
  }
  console.log(`${passed} same, ${failed} different`);
  process.exit(failed ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
