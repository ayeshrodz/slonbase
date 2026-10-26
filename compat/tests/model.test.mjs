// Generic model hooks (../hooks/main.pb.js): onModel* for records (tagged with collection
// names) and collections (tagged "_collections").
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { admin, failure, eventually } from "./lib.mjs";

const items = "hooks_model";
const log = "hooks_model_log";
let pb;

before(async () => {
  pb = await admin();
  for (const name of [items, log, "hooks_c_model_ok", "hooks_c_model_refuse"]) {
    await pb.collections.delete(name).catch(() => {});
  }
  await pb.collections.create({ name: log, fields: [{ name: "title", type: "text" }] });
  await pb.collections.create({ name: items, createRule: "", fields: [{ name: "title", type: "text", max: 20 }] });
});

const logged = (title) =>
  eventually(async () => (await pb.collection(log).getFullList({ filter: pb.filter("title = {:t}", { t: title }) })).length);

test("onModel* hooks on records", async () => {
  assert.equal((await pb.collection(items).create({ title: "model-change" })).title, "model-changed");
  assert.equal(
    (await pb.collection(items).create({ title: "order" })).title,
    "order-record-model",
    "the record hook runs first",
  );
  const refused = await failure(pb.collection(items).create({ title: "model-invalid" }));
  assert.equal(refused.status, 400);
  assert.match(refused.response.message, /Refused by model validate\./);

  await logged("model-success:model-changed");
  await failure(pb.collection(items).create({ title: "x".repeat(30) }));
  await logged("model-error:" + "x".repeat(30));
});

test("onModel* hooks on collections", async () => {
  await pb.collections.create({ name: "hooks_c_model_ok" });
  await logged("collection-success:hooks_c_model_ok");
  const refused = await failure(pb.collections.create({ name: "hooks_c_model_refuse" }));
  assert.equal(refused.status, 400);
  await failure(pb.collections.getOne("hooks_c_model_refuse"));
});
