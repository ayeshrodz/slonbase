// The `fields` query parameter: nested paths, `*`, the `:excerpt` modifier (checked against
// PocketBase's own excerpts of an HTML corpus in excerpts.json), on every JSON answer; and
// record.set() modifiers in hooks (../hooks/main.pb.js).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { admin, client, failure, uid, EMAIL, PASSWORD } from "./lib.mjs";

const authors = uid("fields_authors");
const posts = uid("fields_posts");
const corpus = uid("fields_corpus");
const EXCERPTS = JSON.parse(fs.readFileSync(new URL("./excerpts.json", import.meta.url), "utf8"));
let pb;
let ann;

before(async () => {
  pb = await admin();
  for (const name of ["fields_items", "fields_people"]) await pb.collections.delete(name).catch(() => {});
  const a = await pb.collections.create({
    name: authors, listRule: "", viewRule: "",
    fields: [{ name: "name", type: "text" }, { name: "bio", type: "editor" }],
  });
  await pb.collections.create({
    name: posts, listRule: "", viewRule: "", createRule: "",
    fields: [
      { name: "title", type: "text" },
      { name: "body", type: "editor" },
      { name: "author", type: "relation", collectionId: a.id },
      { name: "readers", type: "relation", collectionId: a.id, maxSelect: 9 },
      { name: "meta", type: "json" },
    ],
  });
  await pb.collections.create({
    name: corpus, listRule: "", fields: [{ name: "html", type: "editor" }, { name: "n", type: "number" }],
  });
  ann = await pb.collection(authors).create({ name: "Ann", bio: "<p>Writes <i>a lot</i> about things.</p>" });
  await pb.collection(posts).create({
    title: "First", body: "<h1>Hello</h1><p>wonderful world</p>", author: ann.id,
    meta: { a: 1, nested: { list: [{ k: 1, v: 2 }, { k: 3, v: 4 }] } },
  });
  await pb.collection(posts).create({ title: "Second", body: "<p>x</p>" });
  const people = await pb.collections.create({ name: "fields_people", fields: [{ name: "name", type: "text" }] });
  await pb.collections.create({
    name: "fields_items",
    fields: [
      { name: "tags", type: "select", values: ["a", "b", "c"], maxSelect: 3 },
      { name: "kind", type: "select", values: ["x", "y", "z"], maxSelect: 1 },
      { name: "owners", type: "relation", collectionId: people.id, maxSelect: 5 },
      { name: "views", type: "number" },
      { name: "code", type: "text", autogeneratePattern: "[0-9]{4}" },
      { name: "note", type: "text" },
    ],
  });
});

after(async () => {
  for (const name of [posts, authors, corpus, "fields_items", "fields_people"]) {
    await pb.collections.delete(name).catch(() => {});
  }
});

const list = (query) => pb.send(`/api/collections/${posts}/records?sort=title&${query}`, {});

test("nested paths, *, and lists", async () => {
  let res = await list("fields=title");
  assert.deepEqual(res.items, [{ title: "First" }, { title: "Second" }]);
  assert.equal(res.totalItems, 2, "the envelope is kept");
  res = await list("expand=author&fields=title,expand.author.name");
  assert.deepEqual(res.items, [{ title: "First", expand: { author: { name: "Ann" } } }, { title: "Second", expand: {} }]);
  res = await list("fields=title,meta.nested.list.k");
  assert.deepEqual(res.items[0], { title: "First", meta: { nested: { list: [{ k: 1 }, { k: 3 }] } } });
  res = await list("expand=author&fields=*,expand.author.id");
  assert.deepEqual(res.items[0].expand, { author: { id: ann.id } });
  assert.equal(res.items[0].collectionName, posts);
  // a multiple relation with nothing found isn't in the expand
  res = await list("expand=readers&fields=title,expand");
  assert.deepEqual(res.items[0], { title: "First", expand: {} });
  res = await list("fields=page");
  assert.deepEqual(res.items, [{}, {}], "picked in the items only");
  res = await list("fields=title:excerpt(3),title");
  assert.ok(["Fir", "First"].includes(res.items[0].title));
});

test("excerpts like PocketBase's", async () => {
  for (const [n, doc] of EXCERPTS.entries()) {
    await pb.collection(corpus).create({ html: doc.html, n });
  }
  for (const m of Object.keys(EXCERPTS[0].excerpts)) {
    const res = await pb.send(`/api/collections/${corpus}/records?sort=n&perPage=1000&fields=n,html:excerpt(${m})`, {});
    assert.equal(res.items.length, EXCERPTS.length);
    for (const item of res.items) {
      assert.equal(item.html, EXCERPTS[item.n].excerpts[m], `excerpt(${m}) of ${JSON.stringify(EXCERPTS[item.n].html)}`);
    }
  }
  // the arguments, read as Go reads them
  const one = async (args) =>
    (await list(`fields=body:excerpt(${encodeURIComponent(args)})`)).items[0].body;
  assert.equal(await one("12.7"), "Hello wonder");
  assert.equal(await one("0x10"), "Hello wonderful");
  assert.equal(await one("010,True"), "Hello wo...");
  assert.equal(await one("5 true"), "Hello...");
  assert.equal(await one("5,yes"), "Hello");
  // modifiers leave other values alone
  const tags = await list("fields=title:excerpt(2),meta:excerpt(1)");
  assert.equal(tags.items[0].title, "Fi");
  assert.equal(tags.items[0].meta.a, 1);
});

test("HTML nested too deep for an excerpt", async () => {
  // Go's parser stops at 512 open elements; the answer is then sent unpicked, as a whole
  const deep = uid("fields_deep");
  await pb.collections.create({ name: deep, listRule: "", viewRule: "", fields: [{ name: "html", type: "editor" }] });
  try {
    const ok = await pb.collection(deep).create({ html: "<div>".repeat(510) + "x" });
    const over = await pb.collection(deep).create({ html: "<div>".repeat(511) + "x" });
    const one = await pb.send(`/api/collections/${deep}/records/${ok.id}?fields=html:excerpt(5)`, {});
    assert.deepEqual(one, { html: "x" });
    const whole = await pb.send(`/api/collections/${deep}/records/${over.id}?fields=html:excerpt(5)`, {});
    assert.equal(whole.id, over.id, "unpicked");
    const list = await pb.send(`/api/collections/${deep}/records?sort=id&fields=html:excerpt(5)`, {});
    assert.equal(list.items.length, 2);
    assert.ok(list.items.every((item) => item.id && item.collectionName === deep), "all of it unpicked");
    // without a modifier, nothing to fail
    const plain = await pb.send(`/api/collections/${deep}/records?fields=id`, {});
    assert.ok(plain.items.every((item) => Object.keys(item).length === 1));
  } finally {
    await pb.collections.delete(deep).catch(() => {});
  }
});

test("on every JSON answer", async () => {
  const created = await pb.send(`/api/collections/${posts}/records?fields=title,body:excerpt(4,true)`, {
    method: "POST", body: { title: "Third", body: "<p>Fresh body</p>" },
  });
  assert.deepEqual(created, { title: "Third", body: "Fres..." });
  const auth = await client().send(`/api/collections/_superusers/auth-with-password?fields=record.email`, {
    method: "POST", body: { identity: EMAIL, password: PASSWORD },
  });
  assert.deepEqual(auth, { record: { email: EMAIL } });
  const settings = await pb.send("/api/settings?fields=batch.maxRequests", {});
  assert.deepEqual(Object.keys(settings), ["batch"]);
  assert.deepEqual(Object.keys(settings.batch), ["maxRequests"]);
  const health = await client().send("/api/health?fields=message", {});
  assert.deepEqual(health, { message: "API is healthy." });
  // errors aren't picked
  const missing = await failure(pb.send(`/api/collections/${posts}/records/nope000000000a?fields=id`, {}));
  assert.equal(missing.status, 404);
});

test("a fields parameter that can't be parsed", async () => {
  for (const bad of ["body:excerpt(0)", "body:nope(1)", "id,(title", "body:excerpt()", "body:excerpt(1,2,3)", "body:excerpt(1e1)"]) {
    const err = await failure(list(`fields=${encodeURIComponent(bad)}`));
    assert.equal(err.status, 400, bad);
  }
});

test("record.set() modifiers in hooks", async () => {
  const res = await pb.send("/compat/record-set", { method: "POST" });
  const [annId, bobId] = res.ids;
  const s = res.steps;
  assert.deepEqual(s.tagsBefore, ["c", "a", "b"]);
  assert.deepEqual(s.tags, ["c", "b"]);
  assert.deepEqual(s.tagsJson, ["c", "b", "a"]);
  assert.equal(s.kindOne, "x");
  assert.equal(s.kindLast, "z");
  assert.equal(s.kindNone, "");
  assert.deepEqual(s.owners, [bobId, annId]);
  assert.equal(s.views, 6.5);
  assert.match(s.code, /^INV-[0-9]{4}$/);
  assert.equal(s.note, "42");
  assert.equal(s.custom, 1);
  assert.deepEqual(s.expand, { a: 1 });
  assert.deepEqual(s.expandGet, { a: 1 });
  assert.deepEqual(res.saved, { tags: ["c", "b", "a"], owners: [bobId, annId], views: 6.5, code: s.code });
  assert.match(res.regex, /^(ab|cd)-[0-9]{3}$/);
});
