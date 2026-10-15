// Collections, settings and health through the SDK.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { admin, client, uid, failure } from "./lib.mjs";

let pb;

before(async () => {
  pb = await admin();
});

test("health", async () => {
  const health = await client().health.check();
  assert.equal(health.code, 200);
  assert.equal(typeof health.message, "string");
});

test("collection CRUD", async () => {
  const name = uid("things");
  const created = await pb.collections.create({
    name,
    type: "base",
    fields: [
      { name: "title", type: "text", required: true },
      { name: "n", type: "number" },
    ],
    indexes: [`CREATE INDEX idx_${name}_title ON ${name} (title)`],
  });
  assert.equal(created.name, name);
  assert.equal(created.type, "base");
  assert.equal(created.system, false);
  assert.equal(created.listRule, null, "rules default to superusers only");
  assert.equal(created.id.length > 0, true);
  const names = created.fields.map((f) => f.name);
  for (const n of ["id", "title", "n"]) assert.ok(names.includes(n), `${n} in ${names}`);
  const id = created.fields.find((f) => f.name === "id");
  assert.equal(id.primaryKey, true);
  assert.equal(id.system, true);
  assert.equal(created.indexes.length, 1);

  const byName = await pb.collections.getOne(name);
  assert.equal(byName.id, created.id);
  const byId = await pb.collections.getOne(created.id);
  assert.equal(byId.name, name);

  const list = await pb.collections.getList(1, 50, { filter: `name = "${name}"` });
  assert.equal(list.totalItems, 1);
  const first = await pb.collections.getFirstListItem(`name = "${name}"`);
  assert.equal(first.id, created.id);

  const fields = [...created.fields, { name: "extra", type: "bool" }];
  const updated = await pb.collections.update(name, { listRule: "", fields });
  assert.equal(updated.listRule, "");
  assert.ok(updated.fields.some((f) => f.name === "extra"));

  await pb.collection(name).create({ title: "x" });
  const records = await client().collection(name).getList(1, 10);
  assert.equal(records.totalItems, 1);
  await pb.collections.truncate(name);
  assert.equal((await client().collection(name).getList(1, 10)).totalItems, 0);

  await pb.collections.delete(name);
  const gone = await failure(pb.collections.getOne(name));
  assert.equal(gone.status, 404);
});

test("collection filters and sorting", async () => {
  const system = await pb.collections.getFullList({ filter: "system = true", sort: "-name" });
  assert.ok(system.length >= 2);
  assert.ok(system.every((c) => c.system));
  assert.ok(system.some((c) => c.name === "_superusers"));
  const names = system.map((c) => c.name);
  assert.deepEqual(names, [...names].sort().reverse());

  const auth = await pb.collections.getFullList({ filter: 'type = "auth" && name = "_superusers"' });
  assert.deepEqual(
    auth.map((c) => c.name),
    ["_superusers"],
  );
  const bad = await failure(pb.collections.getList(1, 10, { filter: "nope = 1" }));
  assert.equal(bad.status, 400);
});

test("collection validation", async () => {
  const dup = await failure(pb.collections.create({ name: "_superusers", type: "base" }));
  assert.equal(dup.status, 400);
  assert.ok(dup.response.data.name, JSON.stringify(dup.response.data));

  const badType = await failure(
    pb.collections.create({ name: uid("bad"), type: "base", fields: [{ name: "x", type: "nope" }] }),
  );
  assert.equal(badType.status, 400);

  const system = await failure(pb.collections.delete("_superusers"));
  assert.equal(system.status, 400);
});

test("scaffolds", async () => {
  const scaffolds = await pb.collections.getScaffolds();
  for (const type of ["base", "auth", "view"]) {
    assert.equal(scaffolds[type].type, type);
  }
  const authFields = scaffolds.auth.fields.map((f) => f.name);
  for (const n of ["id", "password", "tokenKey", "email", "emailVisibility", "verified"]) {
    assert.ok(authFields.includes(n), `${n} in ${authFields}`);
  }
});

test("settings", async () => {
  const settings = await pb.settings.getAll();
  for (const key of ["meta", "smtp", "s3", "backups", "batch", "rateLimits", "trustedProxy", "logs"]) {
    assert.ok(key in settings, `settings.${key}`);
  }
  assert.equal(typeof settings.meta.appName, "string");
  const updated = await pb.settings.update({ meta: { appName: "Compat App" } });
  assert.equal(updated.meta.appName, "Compat App");
  assert.equal((await pb.settings.getAll()).meta.appName, "Compat App");

  const guest = await failure(client().settings.getAll());
  assert.equal(guest.status, 401);
});

test("errors for unknown collections and routes", async () => {
  const missing = await failure(client().collection("does_not_exist").getList(1, 10));
  assert.equal(missing.status, 404);
  assert.equal(typeof missing.response.message, "string");

  const route = await failure(client().send("/api/no/such/route", { method: "GET" }));
  assert.equal(route.status, 404);
});
