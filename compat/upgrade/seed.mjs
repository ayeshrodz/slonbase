// Seeds the old release (upgrade/run.sh) through the SDK and writes a snapshot of what its
// API returns: the collections, every record, the files' contents and tokens to reuse after
// the upgrade. Only uses what every release has.
import { writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import PocketBase from "pocketbase";

const url = process.env.COMPAT_URL;
const pb = new PocketBase(url);
pb.autoCancellation(false);
await pb.collection("_superusers").authWithPassword(process.env.COMPAT_EMAIL, process.env.COMPAT_PASSWORD);

const users = await pb.collections.create({
  name: "up_users",
  type: "auth",
  listRule: "id = @request.auth.id",
  viewRule: "id = @request.auth.id",
  createRule: "",
  updateRule: "id = @request.auth.id",
  fields: [
    { name: "name", type: "text", max: 50 },
    { name: "avatar", type: "file", maxSelect: 1, maxSize: 1 << 20, mimeTypes: ["image/png"] },
  ],
  otp: { enabled: true },
});
const tags = await pb.collections.create({
  name: "up_tags",
  listRule: "",
  viewRule: "",
  fields: [{ name: "label", type: "text", required: true }],
  indexes: ["CREATE UNIQUE INDEX idx_up_tags_label ON up_tags (label)"],
});
await pb.collections.create({
  name: "up_posts",
  listRule: "",
  viewRule: "",
  createRule: "@request.auth.id != ''",
  updateRule: "author = @request.auth.id",
  deleteRule: "author = @request.auth.id",
  fields: [
    { name: "title", type: "text", required: true, max: 100 },
    { name: "body", type: "editor" },
    { name: "views", type: "number", onlyInt: true, min: 0 },
    { name: "score", type: "number" },
    { name: "published", type: "bool" },
    { name: "contact", type: "email" },
    { name: "link", type: "url" },
    { name: "due", type: "date" },
    { name: "status", type: "select", values: ["draft", "live", "archived"], maxSelect: 1 },
    { name: "labels", type: "select", values: ["a", "b", "c"], maxSelect: 3 },
    { name: "meta", type: "json" },
    { name: "place", type: "geoPoint" },
    { name: "files", type: "file", maxSelect: 3, maxSize: 1 << 20 },
    { name: "author", type: "relation", collectionId: users.id, maxSelect: 1, cascadeDelete: true },
    { name: "tags", type: "relation", collectionId: tags.id, maxSelect: 5 },
    { name: "secret", type: "text", hidden: true },
    { name: "created", type: "autodate", onCreate: true },
    { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
  ],
  indexes: ["CREATE INDEX idx_up_posts_status ON up_posts (status)"],
});
await pb.collections.create({
  name: "up_post_stats",
  type: "view",
  listRule: "",
  viewQuery: "SELECT p.author AS id, count(*) AS posts FROM up_posts p GROUP BY p.author",
});

const tagIds = [];
for (const label of ["rust", "postgres", "ünïcode ✓"]) tagIds.push((await pb.collection("up_tags").create({ label })).id);

const password = "user-password-123";
const people = [];
for (const [email, name] of [
  ["ann@example.com", "Ann"],
  ["bob@example.com", "Bob"],
]) {
  const form = new FormData();
  for (const [k, v] of Object.entries({ email, name, password, passwordConfirm: password, verified: "true" })) form.append(k, v);
  // a 1x1 PNG
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
  form.append("avatar", new Blob([png], { type: "image/png" }), "avatar.png");
  people.push(await pb.collection("up_users").create(form));
}

for (let i = 0; i < 4; i++) {
  const form = new FormData();
  const data = {
    title: `post ${i} — ✓`,
    body: `<p>Body <b>${i}</b></p>`,
    views: String(i * 10),
    score: String(i + 0.25),
    published: String(i % 2 === 0),
    contact: `c${i}@example.com`,
    link: `https://example.com/${i}`,
    due: `2026-0${i + 1}-15 10:30:00.000Z`,
    status: ["draft", "live", "archived", "live"][i],
    meta: JSON.stringify({ n: i, nested: { list: [1, "two", null] } }),
    place: JSON.stringify({ lon: 13.4 + i, lat: 52.5 }),
    author: people[i % 2].id,
    secret: `secret ${i}`,
  };
  for (const [k, v] of Object.entries(data)) form.append(k, v);
  for (const l of ["a", "c"].slice(0, (i % 2) + 1)) form.append("labels", l);
  for (const t of tagIds.slice(0, i)) form.append("tags", t);
  for (let f = 0; f < (i % 3); f++) {
    form.append("files", new Blob([`file ${f} of post ${i}\n`], { type: "text/plain" }), `note-${f}.txt`);
  }
  await pb.collection("up_posts").create(form);
}

const settings = await pb.settings.getAll();
const changes = { meta: { ...settings.meta, appName: "Upgrade test", senderName: "Upgrader" } };
// the checks sign in often (releases from 0.4.2 on start with rate limits)
if (settings.rateLimits) changes.rateLimits = { ...settings.rateLimits, enabled: false };
await pb.settings.update(changes);

// the snapshot: what the API returns, as the superuser sees it
const collections = await pb.collections.getFullList({ sort: "name" });
const records = {};
const files = {};
for (const c of collections.filter((c) => c.name.startsWith("up_"))) {
  records[c.name] = await pb.collection(c.name).getFullList({ sort: "id" });
  for (const r of records[c.name]) {
    for (const field of c.fields.filter((f) => f.type === "file")) {
      for (const name of [].concat(r[field.name] || [])) {
        const res = await fetch(pb.files.getURL(r, name));
        if (!res.ok) throw new Error(`file ${name}: ${res.status}`);
        files[`${c.name}/${r.id}/${name}`] = createHash("sha256").update(Buffer.from(await res.arrayBuffer())).digest("hex");
      }
    }
  }
}
const signedIn = await new PocketBase(url).collection("up_users").authWithPassword("ann@example.com", password);
writeFileSync(
  process.argv[2],
  JSON.stringify({
    collections,
    records,
    files,
    settings: (await pb.settings.getAll()).meta,
    password,
    tokens: { superuser: pb.authStore.token, user: signedIn.token, userId: signedIn.record.id },
  }, null, 2),
);
console.log(`seeded ${Object.values(records).flat().length} records, ${Object.keys(files).length} files`);
