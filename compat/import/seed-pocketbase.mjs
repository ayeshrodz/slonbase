// Creates varied data on a running PocketBase (COMPAT_URL) for the import test and writes a
// snapshot of it (as PocketBase's API returns it) to the file given as the first argument.
import { writeFileSync } from "node:fs";
import PocketBase from "pocketbase";

async function main() {
  const pb = new PocketBase(process.env.COMPAT_URL);
  pb.autoCancellation(false);
  await pb.collection("_superusers").authWithPassword(process.env.COMPAT_EMAIL, process.env.COMPAT_PASSWORD);
  await pb.settings.update({ meta: { appName: "Imported App", appURL: "https://imported.example.com" } });

  const open = { listRule: "", viewRule: "", createRule: "", updateRule: "", deleteRule: "" };
  // PocketBase's automigrate names its files by the second: one collection per second keeps
  // the files in creation order, so they replay (see run.sh)
  const createCollection = async (data) => {
    await new Promise((resolve) => setTimeout(resolve, 1100));
    return pb.collections.create(data);
  };
  const tags = await createCollection({
    name: "tags",
    ...open,
    fields: [{ name: "label", type: "text", required: true }],
    indexes: ["CREATE UNIQUE INDEX idx_tags_label ON tags (label)"],
  });
  const members = await createCollection({
    name: "members",
    type: "auth",
    ...open,
    listRule: "id = @request.auth.id",
    fields: [{ name: "nickname", type: "text" }],
  });
  await createCollection({
    name: "articles",
    ...open,
    listRule: "published = true || @request.auth.id != ''",
    fields: [
      { name: "title", type: "text", required: true },
      { name: "body", type: "editor" },
      { name: "views", type: "number" },
      { name: "rating", type: "number" },
      { name: "published", type: "bool" },
      { name: "status", type: "select", values: ["draft", "live"], maxSelect: 1 },
      { name: "labels", type: "select", values: ["a", "b", "c"], maxSelect: 3 },
      { name: "meta", type: "json" },
      { name: "site", type: "url" },
      { name: "contact", type: "email" },
      { name: "due", type: "date" },
      { name: "place", type: "geoPoint" },
      { name: "author", type: "relation", collectionId: members.id, maxSelect: 1 },
      { name: "tags", type: "relation", collectionId: tags.id, maxSelect: 5 },
      { name: "cover", type: "file", maxSelect: 1, maxSize: 1 << 20 },
      { name: "attachments", type: "file", maxSelect: 5, maxSize: 1 << 20 },
      { name: "created", type: "autodate", onCreate: true },
      { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
    ],
  });
  // a view whose query runs on both, and one with SQLite-only SQL
  await createCollection({
    name: "article_stats",
    type: "view",
    listRule: "",
    viewRule: "",
    viewQuery: "SELECT id, title, views FROM articles WHERE published = true",
  });
  await createCollection({
    name: "yearly",
    type: "view",
    listRule: "",
    viewQuery: "SELECT id, strftime('%Y', created) AS year FROM articles",
  });

  const tagIds = [];
  for (const label of ["red", "green", "blue"]) tagIds.push((await pb.collection("tags").create({ label })).id);
  const ann = await pb.collection("members").create({
    email: "ann@example.com",
    password: "ann-password-1",
    passwordConfirm: "ann-password-1",
    nickname: "Ann",
    verified: true,
    emailVisibility: true,
  });
  await pb.collection("members").create({ email: "bob@example.com", password: "bob-password-1", passwordConfirm: "bob-password-1" });
  await pb.collection("_externalAuths").create({
    collectionRef: members.id,
    recordRef: ann.id,
    provider: "github",
    providerId: "gh-12345",
  });

  for (let i = 0; i < 25; i++) {
    const data = new FormData();
    data.append("title", `Article ${i}`);
    data.append("body", `<p>Body ${i} &amp; more</p>`);
    data.append("views", String(i * 3));
    data.append("rating", String(i / 4));
    data.append("published", String(i % 2 === 0));
    data.append("status", i % 2 ? "draft" : "live");
    data.append("labels", JSON.stringify(i % 3 ? ["a", "c"] : []));
    data.append("meta", JSON.stringify({ n: i, nested: { ok: true }, list: [1, "two"] }));
    data.append("site", `https://example.com/${i}`);
    data.append("contact", `writer${i}@example.com`);
    if (i % 4) data.append("due", `2026-0${(i % 9) + 1}-15 10:30:00.000Z`);
    data.append("place", JSON.stringify({ lon: 23.3 + i / 100, lat: 42.7 }));
    data.append("author", i % 2 ? ann.id : "");
    data.append("tags", JSON.stringify(tagIds.slice(0, i % 4)));
    if (i < 5) {
      data.append("cover", new Blob([`cover of ${i}`]), `cover${i}.txt`);
      data.append("attachments", new Blob([`first ${i}`]), "one.txt");
      data.append("attachments", new Blob([`second ${i}`]), "two.txt");
    }
    await pb.collection("articles").create(data);
  }

  // the snapshot: everything as the superuser sees it (password hashes aren't returned)
  const snapshot = { collections: {}, records: {} };
  for (const c of await pb.collections.getFullList()) {
    if (["members", "articles", "tags", "article_stats"].includes(c.name)) snapshot.collections[c.name] = c;
  }
  for (const name of ["tags", "members", "articles", "article_stats"]) {
    snapshot.records[name] = await pb.collection(name).getFullList({ sort: "id" });
  }
  snapshot.files = {};
  for (const a of snapshot.records.articles.filter((r) => r.cover)) {
    for (const name of [a.cover, ...a.attachments]) {
      const res = await fetch(pb.files.getURL(a, name, { token: await pb.files.getToken() }));
      snapshot.files[`${a.id}/${name}`] = await res.text();
    }
  }
  writeFileSync(process.argv[2], JSON.stringify(snapshot, null, 1));
  // a PocketBase backup of all this (pb_data/backups/import-test.zip)
  await pb.backups.create("import-test.zip");
  console.log(`seeded PocketBase: ${snapshot.records.articles.length} articles, ${Object.keys(snapshot.files).length} files`);
}

main().catch((err) => {
  console.error(`seeding PocketBase failed: ${err.url || ""} ${err.status || ""} ${JSON.stringify(err.response || err.message)}`);
  process.exit(1);
});
