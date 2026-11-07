// Adds data to the demo projects and sends some traffic, so the logs, charts and command center have shape:
//   node traffic.mjs http://127.0.0.1:8222
const base = process.argv[2] || "http://127.0.0.1:8222";
const J = (r) => r.text().then((t) => (t ? JSON.parse(t) : null));
async function login(prefix) {
  const r = await fetch(`${base}${prefix}/api/collections/_superusers/auth-with-password`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ identity: "admin@slonbase.dev", password: "admin-password-1" }) });
  return (await J(r)).token;
}
async function post(prefix, url, body, token) {
  const r = await fetch(base + prefix + url, { method: "POST", headers: { "content-type": "application/json", authorization: token }, body: JSON.stringify(body) });
  if (r.status >= 300) throw new Error(url + " " + r.status + await r.text());
  return J(r);
}
const sets = { "/p/analytics": ["events", 60], "/p/shop": ["orders", 45], "/p/blog": ["posts", 25] };
for (const [prefix, [name, n]] of Object.entries(sets)) {
  const t = await login(prefix);
  await post(prefix, "/api/collections", { name, listRule: "", viewRule: "", fields: [{ name: "title", type: "text" }, { name: "value", type: "number" }] }, t);
  for (let i = 0; i < n; i++) await post(prefix, `/api/collections/${name}/records`, { title: `${name} ${i}`, value: i * 3 }, t);
}
// traffic, in a few rounds so the charts have shape
const targets = [
  ["", "/api/collections/tasks/records?page=1&perPage=20&sort=-created", 200],
  ["", "/api/collections/tasks/records?filter=status%3D'done'&expand=project,assignee", 200],
  ["", "/api/collections/projects/records", 200],
  ["", "/api/collections/comments/records?perPage=50", 200],
  ["", "/api/collections/nope/records", 404],
  ["", "/api/collections/tasks/records/doesnotexist1", 404],
  ["", "/api/collections/users/records", 200],
  ["/p/analytics", "/api/collections/events/records?perPage=50", 200],
  ["/p/shop", "/api/collections/orders/records?perPage=50", 200],
  ["/p/blog", "/api/collections/posts/records", 200],
];
for (let round = 0; round < 12; round++) {
  await Promise.all(targets.flatMap(([p, u]) => Array.from({ length: 6 + round }, () => fetch(base + p + u).then((r) => r.arrayBuffer()))));
  await fetch(`${base}/api/collections/users/auth-with-password`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ identity: `nobody${round}@example.com`, password: "wrong-password" }) }).then((r) => r.arrayBuffer());
  await new Promise((r) => setTimeout(r, 400));
}
console.log("traffic done");
