// Checks a slonbase (COMPAT_URL) that imported the PocketBase data of seed-pocketbase.mjs
// against its snapshot (the file given as the first argument).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import PocketBase from "pocketbase";

const snapshot = JSON.parse(readFileSync(process.argv[2] ?? process.env.SNAPSHOT, "utf8"));
const url = process.env.COMPAT_URL;
const client = () => {
  const pb = new PocketBase(url);
  pb.autoCancellation(false);
  return pb;
};
const admin = async (email = process.env.COMPAT_EMAIL, password = process.env.COMPAT_PASSWORD) => {
  const pb = client();
  await pb.collection("_superusers").authWithPassword(email, password);
  return pb;
};

test("collections keep their ids, rules, fields and indexes", async () => {
  const pb = await admin();
  for (const [name, before] of Object.entries(snapshot.collections)) {
    const after = await pb.collections.getOne(name);
    assert.equal(after.id, before.id, name);
    assert.equal(after.type, before.type, name);
    for (const rule of ["listRule", "viewRule", "createRule", "updateRule", "deleteRule"]) {
      assert.equal(after[rule], before[rule], `${name}.${rule}`);
    }
    const fields = (c) => c.fields.map((f) => `${f.name}:${f.type}:${f.id}`).sort();
    assert.deepEqual(fields(after), fields(before), name);
    assert.equal(after.indexes.length, before.indexes.length, `${name} indexes`);
  }
  // the view with SQLite-only SQL isn't there (the importer reports it)
  const missing = await pb.collections.getOne("yearly").catch((e) => e);
  assert.equal(missing.status, 404);
});

test("records are the same", async () => {
  const pb = await admin();
  for (const [name, before] of Object.entries(snapshot.records)) {
    const after = await pb.collection(name).getFullList({ sort: "id" });
    assert.equal(after.length, before.length, name);
    for (let i = 0; i < before.length; i++) {
      assert.deepEqual(after[i], before[i], `${name} ${before[i].id}`);
    }
  }
});

test("users sign in with their PocketBase passwords", async () => {
  const pb = client();
  const auth = await pb.collection("members").authWithPassword("ann@example.com", "ann-password-1");
  assert.equal(auth.record.nickname, "Ann");
  // the rules came along: Ann sees only herself
  const list = await pb.collection("members").getFullList();
  assert.deepEqual(
    list.map((m) => m.email),
    ["ann@example.com"],
  );
  await client().collection("members").authWithPassword("bob@example.com", "bob-password-1");
  const wrong = await client().collection("members").authWithPassword("bob@example.com", "nope").catch((e) => e);
  assert.equal(wrong.status, 400);
});

test("superusers and OAuth2 links", async () => {
  // the PocketBase superuser (same credentials in this test) signs in
  const pb = await admin();
  const links = await pb.collection("_externalAuths").getFullList();
  assert.ok(
    links.some((l) => l.provider === "github" && l.providerId === "gh-12345"),
    JSON.stringify(links),
  );
});

test("files", async () => {
  const pb = await admin();
  const token = await pb.files.getToken();
  for (const [key, content] of Object.entries(snapshot.files)) {
    const [id, name] = key.split("/");
    const record = await pb.collection("articles").getOne(id);
    const res = await fetch(pb.files.getURL(record, name, { token }));
    assert.equal(res.status, 200, key);
    assert.equal(await res.text(), content, key);
  }
});

test("settings", async () => {
  const settings = await (await admin()).settings.getAll();
  assert.equal(settings.meta.appName, "Imported App");
  assert.equal(settings.meta.appURL, "https://imported.example.com");
});
