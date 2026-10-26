// JavaScript hooks (../hooks/main.pb.js, loaded by both servers): $template, auth request
// hooks, $mails and record tokens, and onRecordEnrich.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { admin, client, failure, eventually, URL } from "./lib.mjs";

let pb;
const users = "hooks_users";
const posts = "hooks_posts";
const mails = "hooks_mails";
const files = "hooks_files";
const secrets = "hooks_secretnotes";
const ids = {};

before(async () => {
  pb = await admin();
  for (const name of [posts, mails, files, secrets, users]) await pb.collections.delete(name).catch(() => {});
  await pb.collections.create({
    name: files,
    viewRule: "",
    fields: [{ name: "file", type: "file", maxSelect: 1, maxSize: 1 << 20 }],
  });
  await pb.collections.create({ name: secrets, listRule: "", viewRule: "", fields: [{ name: "title", type: "text" }] });
  await pb.collections.create({
    name: mails,
    fields: [
      { name: "to", type: "text" },
      { name: "subject", type: "text" },
      { name: "html", type: "text", max: 100000 },
      { name: "created", type: "autodate", onCreate: true },
    ],
  });
  await pb.collections.create({
    name: users,
    type: "auth",
    viewRule: "",
    createRule: "",
    fields: [{ name: "name", type: "text" }],
    otp: { enabled: true },
  });
  await pb.collections.create({
    name: posts,
    listRule: "",
    viewRule: "",
    createRule: "",
    fields: [
      { name: "title", type: "text" },
      { name: "note", type: "text", hidden: true },
      { name: "secret", type: "text" },
      { name: "author", type: "relation", collectionId: (await pb.collections.getOne(users)).id, maxSelect: 1 },
    ],
  });
  for (const [email, name] of [
    ["ann@example.com", "ann"],
    ["bob@blocked.test", "frozen"],
    ["banned@example.com", "banned"],
  ]) {
    const r = await pb.collection(users).create({ email, name, password: "password-123", passwordConfirm: "password-123" });
    ids[name] = r.id;
  }
});

after(async () => {
  for (const name of [posts, mails, files, secrets, users]) await pb.collections.delete(name).catch(() => {});
});

const signIn = (identity, password = "password-123") => client().collection(users).authWithPassword(identity, password);

/** The emails to `to` kept by the hooks, oldest first. */
const mailsTo = async (to) => (await pb.collection(mails).getFullList({ filter: pb.filter("to = {:to}", { to }), sort: "created" }));

/** The token in an email's link. */
const linkToken = (html) => /confirm-[a-z-]+\/([^"<\s]+)/.exec(html)[1];

test("$template renders like Go's html/template", async () => {
  const cases = JSON.parse(readFileSync(new globalThis.URL("./templates.json", import.meta.url), "utf8"));
  for (const c of cases) {
    const res = await fetch(`${URL}/compat/template`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ template: c.template, data: c.data }),
    });
    const body = await res.text();
    if (c.expected === null) {
      assert.equal(res.status, 400, `${c.template} should fail, got ${body}`);
    } else {
      assert.equal(res.status, 200, `${c.template}: ${body}`);
      assert.equal(body, c.expected, c.template);
    }
  }
  const page = await (await fetch(`${URL}/compat/template-files`)).text();
  assert.match(page, /<title>Hi &lt;b&gt;<\/title>/);
  assert.match(page, /<h1>Hello Ann<\/h1>/);
  assert.match(page, /<li data-i="1">b<\/li>/);
  assert.match(page, /<footer>default footer<\/footer>/);
});

test("password sign-in hooks find the record and change the response", async () => {
  const auth = await signIn("@ann");
  assert.equal(auth.record.email, "ann@example.com");
  assert.equal(auth.record.name, "ANN");
  // enriched after onRecordAuthRequest changed the name
  assert.equal(auth.record.initial, "A");
  assert.deepEqual(auth.meta, { method: "password", custom: true });

  assert.equal((await failure(signIn("@ann", "wrong-password-1"))).status, 400);
  assert.equal((await failure(signIn("@nobody"))).status, 400);
  const banned = await failure(signIn("banned@example.com"));
  assert.equal(banned.status, 403);
  assert.equal(banned.response.message, "Banned.");
});

test("refresh hooks", async () => {
  const ann = client();
  await ann.collection(users).authWithPassword("ann@example.com", "password-123");
  const refreshed = await ann.collection(users).authRefresh();
  assert.equal(refreshed.meta.method, "");
  assert.equal(refreshed.record.name, "ANN");

  const bob = client();
  await bob.collection(users).authWithPassword("bob@blocked.test", "password-123");
  const err = await failure(bob.collection(users).authRefresh());
  assert.equal(err.status, 400);
  assert.equal(err.response.message, "No refresh.");
});

test("OTP hooks pick the password", async () => {
  const { otpId } = await client().collection(users).requestOTP("ann@example.com");
  const mail = await eventually(async () => (await mailsTo("ann@example.com")).find((m) => m.html.includes("424242")));
  assert.ok(mail);
  const auth = await client().collection(users).authWithOTP(otpId, "424242");
  assert.equal(auth.meta.method, "otp");
});

test("request hooks can answer themselves", async () => {
  const res = await fetch(`${URL}/api/collections/${users}/request-password-reset`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "bob@blocked.test" }),
  });
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { blocked: true });
});

test("$mails and record tokens", async () => {
  const call = (action) => pb.send(`/compat/hooks-users/${ids.ann}/${action}`, { method: "POST" });

  // a verification token from record.newVerificationToken() confirms the email
  const { token } = await call("verification-token");
  await client().collection(users).confirmVerification(token);
  assert.equal((await pb.collection(users).getOne(ids.ann)).verified, true);
  await eventually(async () => (await mailsTo("ann@example.com")).some((m) => m.subject === "verified"));

  // $mails sends the same email as the endpoint, with a working link
  const before = (await mailsTo("ann@example.com")).length;
  await call("send-password-reset");
  const mail = await eventually(async () => (await mailsTo("ann@example.com")).slice(before).find((m) => m.html.includes("confirm-password-reset")));
  await client().collection(users).confirmPasswordReset(linkToken(mail.html), "password-456", "password-456");
  await signIn("ann@example.com", "password-456");

  const reset = await call("password-reset-token");
  await client().collection(users).confirmPasswordReset(reset.token, "password-123", "password-123");
  await signIn("ann@example.com");
});

test("onRecordEnrich changes every record sent to a client", async () => {
  const guest = client();
  // (only superusers can set hidden fields)
  const created = await pb.collection(posts).create({ title: "hello", note: "hidden note", secret: "s3", author: ids.ann }, { expand: "author" });
  assert.equal(created.titleUpper, "HELLO");
  assert.equal(created.expand.author.initial, "a", "expanded records are enriched");
  // superusers see every field (hide() doesn't apply to them)
  assert.equal(created.secret, "s3");
  for (const record of [
    await guest.collection(posts).getOne(created.id, { expand: "author" }),
    (await guest.collection(posts).getList(1, 10, { expand: "author" })).items[0],
  ]) {
    assert.equal(record.titleUpper, "HELLO");
    assert.equal(record.note, "hidden note", "unhidden");
    assert.equal("secret" in record, false, "hidden");
    assert.equal(record.expand.author.initial, "a", "expanded records are enriched");
  }

  // realtime messages
  const events = [];
  const unsubscribe = await guest.collection(posts).subscribe("*", (e) => events.push(e.record));
  await guest.collection(posts).create({ title: "live", secret: "x" });
  const event = await eventually(() => events.find((r) => r.title === "live"));
  assert.equal(event.titleUpper, "LIVE");
  assert.equal("secret" in event, false);
  await unsubscribe();
});

test("realtime hooks filter subscriptions and change or skip messages", async () => {
  const guest = client();
  const seen = [];
  const secret = [];
  const unsubscribe = await guest.collection(posts).subscribe("*", (e) => seen.push(e.record.title));
  const unsubscribeSecret = await guest.collection(secrets).subscribe("*", (e) => secret.push(e.record.title));
  await guest.collection(secrets).create({ title: "hidden" }).catch(() => {});
  await pb.collection(secrets).create({ title: "hidden" });
  for (const title of ["rt-skip", "rt-hello", "plain"]) await guest.collection(posts).create({ title });
  await eventually(() => seen.includes("plain"));
  assert.deepEqual(seen, ["rt-hello!", "plain"]);
  assert.deepEqual(secret, []);
  await unsubscribe();
  await unsubscribeSecret();
});

test("file hooks rename downloads and refuse tokens", async () => {
  const ids = [];
  for (const name of ["public.txt", "secret.txt"]) {
    const form = new FormData();
    form.append("file", new Blob(["content"]), name);
    ids.push(await pb.collection(files).create(form));
  }
  const res = await fetch(pb.files.getURL(ids[0], ids[0].file));
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "content");
  assert.match(res.headers.get("content-disposition"), /renamed-public/);
  assert.equal((await fetch(pb.files.getURL(ids[1], ids[1].file))).status, 403);

  const ann = client();
  await ann.collection(users).authWithPassword("ann@example.com", "password-123");
  assert.ok((await ann.files.getToken()).length > 20);
  const bob = client();
  await bob.collection(users).authWithPassword("bob@blocked.test", "password-123");
  assert.equal((await failure(bob.files.getToken())).status, 403);
});

test("settings hooks change the list and validate updates", async () => {
  const settings = await pb.settings.getAll({ headers: { "x-hooks-test": "1" } });
  assert.equal(settings.meta.appName, "Hooked");
  assert.notEqual((await pb.settings.getAll()).meta.appName, "Hooked");
  const err = await failure(pb.settings.update({ meta: { appName: "Rejected" } }));
  assert.equal(err.status, 400);
  assert.equal(err.response.message, "Rejected by hook.");
});

test("onBackupCreate can rename a backup", async () => {
  await pb.backups.create("compat_hooks.zip");
  const keys = (await pb.backups.getFullList()).map((b) => b.key);
  assert.ok(keys.includes("hooked_compat_hooks.zip"), JSON.stringify(keys));
  assert.ok(!keys.includes("compat_hooks.zip"));
  await pb.backups.delete("hooked_compat_hooks.zip");
});
