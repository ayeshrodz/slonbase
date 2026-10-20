// Checks that PocketBase's migration files, applied to a new database (the slonbase project
// REPLAY_PROJECT), give the same collections as the import of PocketBase's data (COMPAT_URL).
import { test } from "node:test";
import assert from "node:assert/strict";
import PocketBase from "pocketbase";

const admin = async (url, email, password) => {
  const pb = new PocketBase(url);
  pb.autoCancellation(false);
  await pb.collection("_superusers").authWithPassword(email, password);
  return pb;
};

test("the migrations give the imported collections", async () => {
  const imported = await admin(process.env.COMPAT_URL, process.env.COMPAT_EMAIL, process.env.COMPAT_PASSWORD);
  const replayed = await admin(
    `${process.env.COMPAT_URL}/p/${process.env.REPLAY_PROJECT}`,
    process.env.REPLAY_EMAIL,
    process.env.REPLAY_PASSWORD,
  );
  const fields = (c) => c.fields.map((f) => `${f.name}:${f.type}:${f.id}:${f.required}:${f.hidden}`);
  // PocketBase creates "users" itself, not in a migration file
  const collections = (await imported.collections.getFullList()).filter((c) => !c.system && c.name !== "users");
  assert.ok(collections.length > 0);
  for (const a of collections) {
    const b = await replayed.collections.getOne(a.name);
    assert.equal(b.id, a.id, a.name);
    assert.equal(b.type, a.type, a.name);
    for (const key of ["listRule", "viewRule", "createRule", "updateRule", "deleteRule", "viewQuery"]) {
      assert.equal(b[key], a[key], `${a.name}.${key}`);
    }
    assert.deepEqual(fields(b), fields(a), a.name);
    assert.deepEqual([...b.indexes].sort(), [...a.indexes].sort(), a.name);
  }
  // the view whose SQLite query was ported to PostgreSQL in its migration
  const yearly = await replayed.collections.getOne("yearly");
  assert.deepEqual(yearly.fields.map((f) => f.name), ["id", "year"]);
});
