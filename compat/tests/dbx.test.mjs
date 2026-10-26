// $dbx and the $app.db() query builder (../hooks/main.pb.js), on PocketBase's SQLite and
// slonbase's PostgreSQL: the same queries give the same rows.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { admin } from "./lib.mjs";

const t = "dbx_items";
let pb;

before(async () => {
  pb = await admin();
  await pb.collections.delete(t).catch(() => {});
  await pb.collections.create({
    name: t,
    fields: [
      { name: "title", type: "text" },
      { name: "n", type: "number" },
      { name: "tag", type: "text" },
    ],
  });
  for (const [title, n, tag] of [
    ["one", 1, "a"],
    ["two", 2, "a"],
    ["three", 3, "b"],
    ["four", 4, "a"],
    ["50% off", 5, "c"],
    ["50 percent", 6, "c"],
  ]) {
    await pb.collection(t).create({ title, n, tag });
  }
});

test("$dbx expressions and the query builder", async () => {
  const r = await pb.send("/compat/dbx", {});
  const titles = (...list) => list.map((title) => ({ title }));
  // titles in any order (sorting text depends on the database's collation)
  const set = (name) => {
    assert.ok(Array.isArray(r[name]), `${name}: ${JSON.stringify(r[name])}`);
    return r[name].map((row) => row.title).sort();
  };
  assert.deepEqual(set("hash"), ["four", "one", "two"]);
  assert.match(r.hashNullAndList.error, /extra/, "extra isn't a column");
  assert.deepEqual(set("in"), ["one", "three"]);
  assert.deepEqual(set("inOne"), ["two"]);
  assert.deepEqual(set("inNone"), []);
  assert.deepEqual(set("notIn"), ["50 percent", "50% off", "four", "two"]);
  assert.deepEqual(set("like"), ["50% off", "four", "one", "two"]);
  assert.deepEqual(set("likeMatch"), ["three", "two"]);
  assert.deepEqual(set("orLike"), ["one", "two"]);
  assert.deepEqual(set("notLike"), ["50 percent", "three"]);
  assert.deepEqual(set("orNotLike"), ["50 percent", "50% off", "four", "one", "three", "two"]);
  assert.deepEqual(set("between"), ["three", "two"]);
  assert.deepEqual(set("notBetween"), ["50 percent", "50% off", "four", "one"]);
  assert.deepEqual(set("not"), ["50 percent", "50% off", "three"]);
  assert.deepEqual(set("andOr"), ["four", "two"]);
  assert.equal(set("exists").length, 6);
  assert.deepEqual(set("notExists"), []);
  assert.deepEqual(r.andWhereOrWhere, titles("four", "two"));
  assert.deepEqual(r.limitOffset, titles("two", "three"));
  assert.deepEqual(r.offsetOnly, titles("four", "50% off", "50 percent"));
  assert.deepEqual(r.distinct, [{ tag: "a" }, { tag: "b" }, { tag: "c" }]);
  assert.deepEqual(r.groupHaving, [{ tag: "a", total: 3 }, { tag: "c", total: 2 }]);
  assert.deepEqual(r.alias, [{ name: "one" }]);
  assert.deepEqual(r.join, [
    { title: "one", other: "two" },
    { title: "two", other: "three" },
    { title: "three", other: "four" },
    { title: "four", other: "50% off" },
    { title: "50% off", other: "50 percent" },
  ]);
  assert.deepEqual(r.leftJoin, [{ title: "one", missing: "" }]);
  assert.deepEqual(r.union, ["one", "two"]);
  assert.deepEqual(r.bind, titles("two"));
  assert.deepEqual(r.one, { title: "three", n: 3 });
  assert.equal(r.oneMissing, "missing");
  assert.deepEqual(r.quotedNames, titles("two"));
  assert.deepEqual(r.recordQuery, [["four", 4, t], ["two", 2, t], ["one", 1, t]]);
  assert.deepEqual(r.recordQueryOne, ["a", false]);
  assert.deepEqual(r.builder, [
    { id: 1, name: "a", score: 6 },
    { id: 2, name: "b2", score: 20 },
  ]);
});

test("byte helpers and nullable columns", async () => {
  const r = await pb.send("/compat/bytes", {});
  assert.equal(r.roundTrip, "héllo ✓");
  assert.equal(r.length, 3, "UTF-8 bytes");
  assert.deepEqual(r.row, { missing: null, count: 3, name: "x" });
});
