// strftime() in filters: SQLite's strftime, with date, text, number and relation values.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { admin, uid } from "./lib.mjs";

let pb;
const names = {};

before(async () => {
  pb = await admin();
  names.tags = uid("sf_tags");
  names.events = uid("sf_events");
  const tags = await pb.collections.create({
    name: names.tags,
    listRule: "",
    fields: [
      { name: "name", type: "text" },
      { name: "since", type: "date" },
    ],
  });
  await pb.collections.create({
    name: names.events,
    listRule: "",
    fields: [
      { name: "name", type: "text" },
      { name: "at", type: "date" },
      { name: "note", type: "text" },
      { name: "n", type: "number" },
      { name: "tags", type: "relation", collectionId: tags.id, maxSelect: 5 },
    ],
  });
  const old = await pb.collection(names.tags).create({ name: "old", since: "2019-06-01 00:00:00.000Z" });
  const fresh = await pb.collection(names.tags).create({ name: "fresh", since: "2024-02-29 12:00:00.000Z" });
  const events = [
    { name: "leap", at: "2024-02-29 23:30:00.000Z", note: "2024-02-29", n: 2460370.5, tags: [old.id, fresh.id] },
    { name: "newyear", at: "2025-01-01 00:00:00.000Z", note: "12:30", n: 0, tags: [fresh.id] },
    { name: "sunday", at: "2023-01-01 08:15:30.250Z", note: "not a date", n: 1700000000, tags: [old.id] },
    { name: "empty", note: "" },
  ];
  for (const e of events) await pb.collection(names.events).create(e);
});

async function matching(filter) {
  const list = await pb.collection(names.events).getFullList({ filter, sort: "name" });
  return list.map((r) => r.name);
}

test("formats date fields", async () => {
  assert.deepEqual(await matching("strftime('%Y-%m', at) = '2024-02'"), ["leap"]);
  assert.deepEqual(await matching("strftime('%w', at) = '0'"), ["sunday"], "2023-01-01 is a Sunday");
  assert.deepEqual(await matching("strftime('%H:%M:%f', at) = '08:15:30.250'"), ["sunday"]);
  assert.deepEqual(await matching("strftime('%j', at) = '060'"), ["leap"]);
  assert.deepEqual(await matching("strftime('%Y', at) = ''"), ["empty"], "empty dates format as ''");
});

test("applies modifiers", async () => {
  assert.deepEqual(await matching("strftime('%Y-%m-%d', at, '+1 day') = '2024-03-01'"), ["leap"]);
  assert.deepEqual(await matching("strftime('%Y-%m-%d', at, '+1 year') = '2025-03-01'"), ["leap"]);
  assert.deepEqual(await matching("strftime('%Y-%m-%d', at, 'start of month', '-1 day') = '2024-12-31'"), ["newyear"]);
  assert.deepEqual(await matching("strftime('%Y-%m-%d', at, 'weekday 1') = '2023-01-02'"), ["sunday"]);
});

test("parses text and number values", async () => {
  assert.deepEqual(await matching("strftime('%d', note) = '29'"), ["leap"]);
  assert.deepEqual(await matching("strftime('%H', note) = '12'"), ["newyear"], "a time alone is 2000-01-01");
  assert.deepEqual(await matching("strftime('%Y-%m-%d', n) = '2024-03-01'"), ["leap"], "numbers are Julian days");
  assert.deepEqual(await matching("strftime('%Y', n, 'unixepoch') = '2023'"), ["sunday"]);
  assert.deepEqual(await matching("note != '' && strftime('%Y', note) = ''"), ["sunday"]);
});

test("literals and the current time", async () => {
  const year = String(new Date().getUTCFullYear());
  assert.equal((await matching(`strftime('%Y') = '${year}'`)).length, 4);
  assert.equal((await matching(`strftime('%Y', 'now') = '${year}'`)).length, 4);
  assert.equal((await matching(`strftime('%Y', @now) = '${year}'`)).length, 4);
  assert.equal((await matching("strftime('%Y-%m-%d', '2024-01-31', '+1 month') = '2024-03-02'")).length, 4);
  assert.equal((await matching("strftime('%s', '2024-01-01') = '1704067200'")).length, 4);
});

test("multi-valued relations", async () => {
  assert.deepEqual(await matching("strftime('%Y', tags.since) ?= '2019'"), ["leap", "sunday"]);
  assert.deepEqual(await matching("strftime('%Y', tags.since) = '2024'"), ["newyear"], "all of them");
});

test("rejects bad arguments", async () => {
  for (const filter of ["strftime(1) = ''", "strftime('%Y', at, 5) = ''", "strftime(at) = ''"]) {
    await assert.rejects(matching(filter), (err) => err.status === 400, filter);
  }
});
