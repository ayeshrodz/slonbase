// After the upgrade (upgrade/run.sh): the snapshot of the old release against this build's
// API, and writes, sign-ins, old tokens, rules, indexes and realtime on the upgraded data.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import PocketBase from "pocketbase";

const snapshot = JSON.parse(readFileSync(process.env.SNAPSHOT, "utf8"));
const url = process.env.COMPAT_URL;
const client = () => {
  const pb = new PocketBase(url);
  pb.autoCancellation(false);
  return pb;
};
let pb;

before(async () => {
  pb = client();
  await pb.collection("_superusers").authWithPassword(process.env.COMPAT_EMAIL, process.env.COMPAT_PASSWORD);
});

/** Every key of `old` is in `now` with the same value (newer releases may add keys). */
function assertKept(now, old, path) {
  if (old && typeof old === "object" && !Array.isArray(old)) {
    assert.ok(now && typeof now === "object" && !Array.isArray(now), `${path} is no longer an object`);
    for (const key of Object.keys(old)) assertKept(now[key], old[key], `${path}.${key}`);
  } else if (Array.isArray(old)) {
    assert.ok(Array.isArray(now), `${path} is no longer a list`);
    assert.equal(now.length, old.length, `${path} has ${now.length} items, had ${old.length}`);
    old.forEach((item, i) => assertKept(now[i], item, `${path}[${i}]`));
  } else {
    assert.deepEqual(now, old, path);
  }
}

test("collections are kept", async () => {
  const now = await pb.collections.getFullList({ sort: "name" });
  for (const old of snapshot.collections) {
    const found = now.find((c) => c.id === old.id);
    assert.ok(found, `collection ${old.name} is gone`);
    // (a migration may touch the definition's timestamp)
    const { updated, ...kept } = old;
    assertKept(found, kept, old.name);
  }
});

test("records and files are kept", async () => {
  for (const [name, records] of Object.entries(snapshot.records)) {
    const now = await pb.collection(name).getFullList({ sort: "id" });
    assert.equal(now.length, records.length, `${name}: ${now.length} records, had ${records.length}`);
    records.forEach((r, i) => assertKept(now[i], r, `${name}/${r.id}`));
  }
  for (const [path, sha] of Object.entries(snapshot.files)) {
    const [collection, id, name] = path.split("/");
    const res = await fetch(`${url}/api/files/${collection}/${id}/${name}`);
    assert.equal(res.status, 200, path);
    const hash = createHash("sha256").update(Buffer.from(await res.arrayBuffer())).digest("hex");
    assert.equal(hash, sha, `${path} changed`);
  }
  // thumbnails of old images
  const ann = snapshot.records.up_users.find((u) => u.email === "ann@example.com");
  const thumb = await fetch(`${url}/api/files/up_users/${ann.id}/${ann.avatar}?thumb=10x10`);
  assert.equal(thumb.status, 200);
});

test("settings are kept", async () => {
  assertKept((await pb.settings.getAll()).meta, snapshot.settings, "settings.meta");
});

test("sign-ins and tokens from before the upgrade work", async () => {
  const user = client();
  user.authStore.save(snapshot.tokens.user);
  const refreshed = await user.collection("up_users").authRefresh();
  assert.equal(refreshed.record.id, snapshot.tokens.userId, "the old user token is still valid");

  const admin = client();
  admin.authStore.save(snapshot.tokens.superuser);
  assert.ok((await admin.collections.getFullList()).length > 0, "the old superuser token is still valid");

  const signedIn = await client().collection("up_users").authWithPassword("bob@example.com", snapshot.password);
  assert.equal(signedIn.record.name, "Bob", "passwords hashed by the old release verify");
});

test("writes, rules and indexes on the upgraded data", async () => {
  const ann = client();
  await ann.collection("up_users").authWithPassword("ann@example.com", snapshot.password);
  const bob = client();
  await bob.collection("up_users").authWithPassword("bob@example.com", snapshot.password);

  const post = await ann.collection("up_posts").create({
    title: "after the upgrade",
    author: ann.authStore.record.id,
    tags: [snapshot.records.up_tags[0].id],
    status: "live",
    secret: "s",
  });
  assert.equal(post.secret, undefined, "hidden fields stay hidden");
  assert.equal((await ann.collection("up_posts").update(post.id, { views: 7 })).views, 7);
  // the update rule: Bob can't change Ann's post
  await assert.rejects(bob.collection("up_posts").update(post.id, { title: "no" }), (err) => err.status === 404);
  const old = snapshot.records.up_posts.find((p) => p.author === ann.authStore.record.id);
  assert.equal((await ann.collection("up_posts").update(old.id, { score: 1.5 })).score, 1.5, "old records update");

  // the unique index
  await assert.rejects(pb.collection("up_tags").create({ label: "rust" }), (err) => err.status === 400);

  // filters and expands over old records
  const live = await client().collection("up_posts").getFullList({ filter: 'status = "live"', expand: "tags,author" });
  assert.ok(live.length >= 3);
  assert.ok(live.some((p) => p.expand?.tags?.length), "relations expand");

  // the view counts the new post
  const stats = await client().collection("up_post_stats").getFullList();
  assert.equal(stats.find((s) => s.id === ann.authStore.record.id).posts, 3);

  await ann.collection("up_posts").delete(post.id);
  // cascade: deleting a user deletes their posts
  await pb.collection("up_users").delete(bob.authStore.record.id);
  const left = await pb.collection("up_posts").getFullList({ filter: `author = "${bob.authStore.record.id}"` });
  assert.equal(left.length, 0);
});

test("realtime on the upgraded server", async () => {
  const listener = client();
  const events = [];
  const unsubscribe = await listener.collection("up_tags").subscribe("*", (e) => events.push(e));
  const tag = await pb.collection("up_tags").create({ label: "after" });
  for (let i = 0; i < 100 && !events.length; i++) await new Promise((r) => setTimeout(r, 50));
  await unsubscribe();
  assert.equal(events[0]?.action, "create");
  assert.equal(events[0]?.record.id, tag.id);
});

test("backups of the upgraded data", async () => {
  await pb.backups.create("after-upgrade.zip");
  const list = await pb.backups.getFullList();
  assert.ok(list.some((b) => b.key === "after-upgrade.zip"));
});
