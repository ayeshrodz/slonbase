// Batch requests and realtime subscriptions through the SDK.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { admin, client, uid, failure, eventually } from "./lib.mjs";

let pb;
const tasks = uid("tasks");

before(async () => {
  pb = await admin();
  await pb.settings.update({ batch: { enabled: true, maxRequests: 50, timeout: 3 } });
  await pb.collections.create({
    name: tasks,
    type: "base",
    listRule: "",
    viewRule: "",
    createRule: "",
    updateRule: "",
    deleteRule: "",
    fields: [
      { name: "title", type: "text", required: true },
      { name: "done", type: "bool" },
    ],
  });
});

after(async () => {
  await pb.collections.delete(tasks).catch(() => {});
});

test("batch requests", async () => {
  const existing = await client().collection(tasks).create({ title: "existing" });
  const batch = client().createBatch();
  batch.collection(tasks).create({ id: "batchtask000001", title: "one" });
  batch.collection(tasks).update(existing.id, { done: true });
  batch.collection(tasks).upsert({ id: "batchtask000001", title: "one, upserted" });
  batch.collection(tasks).upsert({ id: "batchtask000002", title: "two" });
  batch.collection(tasks).delete(existing.id);
  const results = await batch.send();
  assert.equal(results.length, 5);
  assert.deepEqual(
    results.map((r) => r.status),
    [200, 200, 200, 200, 204],
  );
  assert.equal(results[0].body.title, "one");
  assert.equal(results[1].body.done, true);
  assert.equal(results[2].body.title, "one, upserted");
  assert.equal(results[3].body.id, "batchtask000002");

  // all or nothing
  const failing = client().createBatch();
  failing.collection(tasks).create({ id: "batchtask000003", title: "three" });
  failing.collection(tasks).create({ title: "" });
  const err = await failure(failing.send());
  assert.equal(err.status, 400);
  assert.ok(err.response.data.requests, JSON.stringify(err.response.data));
  assert.equal(err.response.data.requests["1"].code, "batch_request_failed");
  const three = await failure(client().collection(tasks).getOne("batchtask000003"));
  assert.equal(three.status, 404, "the first request was rolled back");
});

test("realtime subscriptions", async () => {
  const listener = client();
  const events = [];
  const unsubscribe = await listener.collection(tasks).subscribe("*", (e) => {
    events.push([e.action, e.record.title]);
  });

  const writer = client().collection(tasks);
  const created = await writer.create({ title: "live" });
  await writer.update(created.id, { title: "live, updated" });
  await writer.delete(created.id);

  await eventually(() => events.length >= 3);
  assert.deepEqual(events.slice(0, 3), [
    ["create", "live"],
    ["update", "live, updated"],
    ["delete", "live, updated"],
  ]);
  await unsubscribe();

  // a single record, with a filter on the topic's options
  const target = await writer.create({ title: "watched" });
  const one = [];
  const stop = await listener.collection(tasks).subscribe(target.id, (e) => one.push(e.action));
  const filtered = [];
  const stopFiltered = await listener
    .collection(tasks)
    .subscribe("*", (e) => filtered.push(e.record.title), { filter: "done = true" });
  await writer.update(target.id, { title: "watched, changed" });
  await writer.create({ title: "finished", done: true });
  await writer.create({ title: "open", done: false });
  await eventually(() => one.length >= 1 && filtered.length >= 1);
  // give stray events a moment to arrive
  await new Promise((r) => setTimeout(r, 300));
  assert.deepEqual(one, ["update"]);
  assert.deepEqual(filtered, ["finished"]);
  await stop();
  await stopFiltered();
  listener.realtime.unsubscribe();
});

test("realtime events with expand", async () => {
  const people = uid("people");
  const notes = uid("notes");
  const peopleC = await pb.collections.create({
    name: people,
    listRule: "",
    viewRule: "",
    fields: [{ name: "name", type: "text" }],
  });
  await pb.collections.create({
    name: notes,
    listRule: "",
    viewRule: "",
    createRule: "",
    fields: [
      { name: "text", type: "text" },
      { name: "owner", type: "relation", collectionId: peopleC.id, maxSelect: 1 },
    ],
  });
  try {
    const ann = await pb.collection(people).create({ name: "Ann" });
    const listener = client();
    const events = [];
    const stop = await listener
      .collection(notes)
      .subscribe("*", (e) => events.push(e.record), { expand: "owner" });
    await client().collection(notes).create({ text: "hello", owner: ann.id });
    await eventually(() => events.length >= 1);
    assert.equal(events[0].text, "hello");
    assert.equal(events[0].expand?.owner?.name, "Ann", JSON.stringify(events[0]));
    await stop();
    listener.realtime.unsubscribe();
  } finally {
    await pb.collections.delete(notes).catch(() => {});
    await pb.collections.delete(people).catch(() => {});
  }
});
