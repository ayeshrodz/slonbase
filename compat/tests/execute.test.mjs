// Execute and after-error hooks (../hooks/main.pb.js): the write inside e.next(), and the
// hooks of failed writes.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { admin, failure, eventually } from "./lib.mjs";

const items = "hooks_exec";
const log = "hooks_exec_log";
let pb;

before(async () => {
  pb = await admin();
  for (const name of [items, log, "hooks_c_exec_refuse", "hooks_c_exec_invalid"]) await pb.collections.delete(name).catch(() => {});
  await pb.collections.create({ name: log, fields: [{ name: "title", type: "text" }] });
  await pb.collections.create({
    name: items,
    createRule: "",
    updateRule: "",
    deleteRule: "",
    fields: [{ name: "title", type: "text", max: 20 }],
  });
});

const logged = (title) =>
  eventually(async () => (await pb.collection(log).getFullList({ filter: pb.filter("title = {:t}", { t: title }) })).length);
const exists = async (title) =>
  (await pb.collection(items).getFullList({ filter: pb.filter("title = {:t}", { t: title }) })).length > 0;

test("record execute hooks wrap the write", async () => {
  const changed = await pb.collection(items).create({ title: "exec-change" });
  assert.equal(changed.title, "exec-changed", "changes before e.next() are written");
  assert.equal((await pb.collection(items).getOne(changed.id)).title, "exec-changed");

  await pb.collection(items).create({ title: "exec-after" });
  await logged("after-next:id");

  const err = await failure(pb.collection(items).create({ title: "exec-refuse" }));
  assert.equal(err.status, 400);
  assert.equal(err.response.message, "Refused by execute.");
  assert.equal(await exists("exec-refuse"), false);
});

test("after-error hooks see failed creates, updates and deletes", async () => {
  await failure(pb.collection(items).create({ title: "exec-refuse" }));
  await logged("create-error:exec-refuse");

  // a validation error
  const long = "x".repeat(30);
  const invalid = await failure(pb.collection(items).create({ title: long }));
  assert.equal(invalid.status, 400);
  await logged("create-error:" + long);

  const record = await pb.collection(items).create({ title: "exec-upd" });
  await failure(pb.collection(items).update(record.id, { title: "exec-upd-fail" }));
  assert.equal((await pb.collection(items).getOne(record.id)).title, "exec-upd");
  await logged("update-error:exec-upd-fail");

  const kept = await pb.collection(items).create({ title: "exec-keep" });
  const refused = await failure(pb.collection(items).delete(kept.id));
  assert.equal(refused.status, 400);
  assert.equal((await pb.collection(items).getOne(kept.id)).title, "exec-keep");
  await logged("delete-error:exec-keep");
});

test("collection execute and after-error hooks", async () => {
  const err = await failure(pb.collections.create({ name: "hooks_c_exec_refuse" }));
  assert.equal(err.status, 400);
  await failure(pb.collections.getOne("hooks_c_exec_refuse"));
  await logged("collection-error:hooks_c_exec_refuse");

  const invalid = await failure(pb.collections.create({ name: "hooks_c_exec_invalid" }));
  // PocketBase wraps it ("Failed to create collection. Raw error: ...")
  assert.match(invalid.response.message, /Refused by validate\./);
  await failure(pb.collections.getOne("hooks_c_exec_invalid"));
  await logged("collection-error:hooks_c_exec_invalid");
});
