// Records through the SDK: CRUD, filters, sorting, pagination, fields, expand, modifiers and
// the error shapes the SDK exposes.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { admin, client, uid, failure } from "./lib.mjs";

let pb;
const authors = uid("authors");
const posts = uid("posts");
const comments = uid("comments");

before(async () => {
  pb = await admin();
  const a = await pb.collections.create({
    name: authors,
    type: "base",
    listRule: "",
    viewRule: "",
    fields: [{ name: "name", type: "text", required: true }],
  });
  const p = await pb.collections.create({
    name: posts,
    type: "base",
    listRule: "",
    viewRule: "",
    createRule: "",
    updateRule: "",
    deleteRule: "",
    fields: [
      { name: "title", type: "text", required: true, max: 50 },
      { name: "views", type: "number" },
      { name: "published", type: "bool" },
      { name: "tags", type: "select", maxSelect: 3, values: ["a", "b", "c"] },
      { name: "meta", type: "json" },
      { name: "author", type: "relation", collectionId: a.id, maxSelect: 1 },
      { name: "secret", type: "text", hidden: true },
    ],
  });
  await pb.collections.create({
    name: comments,
    type: "base",
    listRule: "",
    viewRule: "",
    fields: [
      { name: "body", type: "text" },
      { name: "post", type: "relation", collectionId: p.id, maxSelect: 1 },
    ],
  });
});

after(async () => {
  for (const name of [comments, posts, authors]) {
    await pb.collections.delete(name).catch(() => {});
  }
});

test("create, view, update and delete", async () => {
  const guest = client();
  const created = await guest.collection(posts).create({ title: "hello", views: 3, tags: ["a"] });
  assert.equal(created.collectionName, posts);
  assert.equal(created.title, "hello");
  assert.equal(created.views, 3);
  assert.equal(created.published, false);
  assert.deepEqual(created.tags, ["a"]);
  assert.equal(created.meta, null);
  assert.equal(created.author, "");
  assert.equal(created.id.length, 15);
  assert.equal("secret" in created, false, "hidden fields aren't returned");

  const viewed = await guest.collection(posts).getOne(created.id);
  assert.deepEqual(viewed, created);

  const updated = await guest.collection(posts).update(created.id, {
    "views+": 2,
    "tags+": "b",
    meta: { k: [1, 2] },
  });
  assert.equal(updated.views, 5);
  assert.deepEqual(updated.tags, ["a", "b"]);
  assert.deepEqual(updated.meta, { k: [1, 2] });

  await guest.collection(posts).delete(created.id);
  const err = await failure(guest.collection(posts).getOne(created.id));
  assert.equal(err.status, 404);
  assert.equal(err.response.status, 404);
  assert.equal(typeof err.response.message, "string");
  assert.deepEqual(err.response.data, {});
});

test("validation errors", async () => {
  const err = await failure(client().collection(posts).create({ title: "", tags: ["x"] }));
  assert.equal(err.status, 400);
  assert.equal(err.response.message, "Failed to create record.");
  assert.equal(err.response.data.title.code, "validation_required");
  assert.equal(typeof err.response.data.title.message, "string");
  assert.equal(err.response.data.tags.code, "validation_invalid_value");

  const long = await failure(client().collection(posts).create({ title: "x".repeat(51) }));
  assert.equal(long.response.data.title.code, "validation_max_text_constraint");
});

test("lists: filters, sorting, pagination and fields", async () => {
  const c = client().collection(posts);
  for (const [i, title] of ["alpha", "beta", "gamma", "delta", "epsilon"].entries()) {
    await c.create({ title, views: i * 10, published: i % 2 === 0 });
  }

  const page = await c.getList(1, 2, { sort: "-views" });
  assert.equal(page.page, 1);
  assert.equal(page.perPage, 2);
  assert.ok(page.totalItems >= 5);
  assert.equal(page.totalPages, Math.ceil(page.totalItems / 2));
  assert.deepEqual(
    page.items.map((r) => r.title),
    ["epsilon", "delta"],
  );

  const skip = await c.getList(1, 2, { sort: "views", skipTotal: true });
  assert.equal(skip.totalItems, -1);
  assert.equal(skip.totalPages, -1);

  const filtered = await c.getFullList({
    filter: client().filter("published = {:p} && views >= {:min}", { p: true, min: 10 }),
    sort: "title",
  });
  assert.deepEqual(
    filtered.map((r) => r.title),
    ["epsilon", "gamma"],
  );

  const like = await c.getFullList({ filter: 'title ~ "ta"', sort: "title" });
  assert.deepEqual(
    like.map((r) => r.title),
    ["beta", "delta"],
  );

  const first = await c.getFirstListItem('title = "gamma"');
  assert.equal(first.views, 20);
  const missing = await failure(c.getFirstListItem('title = "nope"'));
  assert.equal(missing.status, 404);

  const picked = await c.getList(1, 1, { fields: "id,title", filter: 'title = "beta"' });
  assert.deepEqual(Object.keys(picked.items[0]).sort(), ["id", "title"]);

  const bad = await failure(c.getList(1, 10, { filter: "nonexistent = 1" }));
  assert.equal(bad.status, 400);
});

test("expand, relation paths and back-relations", async () => {
  const pba = pb.collection(authors);
  const ann = await pba.create({ name: "Ann" });
  const post = await client().collection(posts).create({ title: "with author", author: ann.id });
  await pb.collection(comments).create({ body: "first", post: post.id });
  await pb.collection(comments).create({ body: "second", post: post.id });

  const withAuthor = await client().collection(posts).getOne(post.id, { expand: "author" });
  assert.equal(withAuthor.expand.author.name, "Ann");
  assert.equal(withAuthor.expand.author.collectionName, authors);

  const back = await client()
    .collection(posts)
    .getOne(post.id, { expand: `${comments}_via_post` });
  assert.deepEqual(back.expand[`${comments}_via_post`].map((c) => c.body).sort(), ["first", "second"]);

  const byPath = await client().collection(comments).getFullList({ filter: 'post.author.name = "Ann"' });
  assert.equal(byPath.length, 2);

  const nested = await client()
    .collection(comments)
    .getFirstListItem('body = "first"', { expand: "post.author" });
  assert.equal(nested.expand.post.expand.author.name, "Ann");
});

test("rules", async () => {
  // authors have no create rule: only superusers can create
  const err = await failure(client().collection(authors).create({ name: "x" }));
  assert.equal(err.status, 403);
  assert.equal(typeof err.response.message, "string");
  // and no update rule: the record is hidden from others
  const ann = await pb.collection(authors).create({ name: "Bob" });
  const upd = await failure(client().collection(authors).update(ann.id, { name: "y" }));
  assert.equal(upd.status, 403);
});
