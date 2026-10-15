// Auth collections through the SDK: sign-up, password sign-in, refresh, auth methods, rules
// with @request.auth, email visibility, password changes and impersonation.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { admin, client, uid, failure } from "./lib.mjs";

let pb;
const users = uid("users");

before(async () => {
  pb = await admin();
  await pb.collections.create({
    name: users,
    type: "auth",
    listRule: "id = @request.auth.id",
    viewRule: "",
    createRule: "",
    updateRule: "id = @request.auth.id",
    deleteRule: "id = @request.auth.id",
    fields: [{ name: "name", type: "text" }],
  });
});

after(async () => {
  await pb.collections.delete(users).catch(() => {});
});

async function signUp(email, password = "password-123", extra = {}) {
  return client()
    .collection(users)
    .create({ email, password, passwordConfirm: password, ...extra });
}

test("sign-up", async () => {
  const ann = await signUp("ann@example.com", "password-123", { name: "Ann" });
  assert.equal(ann.collectionName, users);
  assert.equal(ann.name, "Ann");
  assert.equal(ann.verified, false);
  assert.equal(ann.emailVisibility, false);
  assert.equal("email" in ann, false, "the email isn't visible to a guest");
  assert.equal("password" in ann, false);
  assert.equal("tokenKey" in ann, false);

  // (PocketBase compares emails case-sensitively here; slonbase also rejects other casings)
  const dup = await failure(signUp("ann@example.com"));
  assert.equal(dup.status, 400);
  assert.equal(dup.response.data.email.code, "validation_not_unique");

  const mismatch = await failure(
    client().collection(users).create({ email: "x@example.com", password: "password-123", passwordConfirm: "nope-nope-1" }),
  );
  assert.equal(mismatch.response.data.passwordConfirm.code, "validation_values_mismatch");

  const short = await failure(signUp("y@example.com", "short"));
  assert.equal(short.response.data.password.code, "validation_min_text_constraint");
});

test("password sign-in and refresh", async () => {
  await signUp("bob@example.com");
  const bob = client();
  const auth = await bob.collection(users).authWithPassword("bob@example.com", "password-123");
  assert.equal(typeof auth.token, "string");
  assert.equal(auth.record.email, "bob@example.com", "the record sees its own email");
  assert.equal(bob.authStore.isValid, true);
  assert.equal(bob.authStore.record.id, auth.record.id);

  const refreshed = await bob.collection(users).authRefresh();
  assert.equal(refreshed.record.id, auth.record.id);
  assert.equal(typeof refreshed.token, "string");

  const wrong = await failure(client().collection(users).authWithPassword("bob@example.com", "wrong-password"));
  assert.equal(wrong.status, 400);
  assert.equal(wrong.response.message, "Failed to authenticate.");
  assert.deepEqual(wrong.response.data, {});

  const unknown = await failure(client().collection(users).authWithPassword("nobody@example.com", "password-123"));
  assert.equal(unknown.status, 400);
  assert.equal(unknown.response.message, "Failed to authenticate.");

  const guestRefresh = await failure(client().collection(users).authRefresh());
  assert.equal(guestRefresh.status, 401);
});

test("auth methods", async () => {
  const methods = await client().collection(users).listAuthMethods();
  assert.equal(methods.password.enabled, true);
  assert.deepEqual(methods.password.identityFields, ["email"]);
  assert.equal(methods.oauth2.enabled, false);
  assert.deepEqual(methods.oauth2.providers, []);
  assert.equal(methods.mfa.enabled, false);
  assert.equal(methods.otp.enabled, false);
  assert.equal(typeof methods.mfa.duration, "number");
  assert.equal(typeof methods.otp.duration, "number");
});

test("rules with @request.auth", async () => {
  await signUp("carl@example.com");
  await signUp("dina@example.com");
  const carl = client();
  const { record } = await carl.collection(users).authWithPassword("carl@example.com", "password-123");

  const listed = await carl.collection(users).getFullList();
  assert.deepEqual(
    listed.map((u) => u.id),
    [record.id],
    "the list rule shows only the caller",
  );
  const guestList = await client().collection(users).getList(1, 10);
  assert.equal(guestList.totalItems, 0);

  const dina = await pb.collection(users).getFirstListItem('email = "dina@example.com"');
  const viewed = await carl.collection(users).getOne(dina.id);
  assert.equal("email" in viewed, false, "other records' emails stay hidden");
  assert.equal(viewed.id, dina.id);

  const updated = await carl.collection(users).update(record.id, { name: "Carl" });
  assert.equal(updated.name, "Carl");
  const other = await failure(carl.collection(users).update(dina.id, { name: "x" }));
  assert.equal(other.status, 404);

  // superusers see everything, emails included
  const all = await pb.collection(users).getFullList({ filter: 'email ~ "@example.com"' });
  assert.ok(all.length >= 2);
  assert.ok(all.every((u) => typeof u.email === "string"));
});

test("email visibility", async () => {
  await signUp("eve@example.com", "password-123", { emailVisibility: true });
  const eve = await pb.collection(users).getFirstListItem('email = "eve@example.com"');
  const seen = await client().collection(users).getOne(eve.id);
  assert.equal(seen.email, "eve@example.com");
});

test("password changes", async () => {
  await signUp("fay@example.com");
  const fay = client();
  const { record } = await fay.collection(users).authWithPassword("fay@example.com", "password-123");

  const noOld = await failure(
    fay.collection(users).update(record.id, { password: "new-password-1", passwordConfirm: "new-password-1" }),
  );
  assert.equal(noOld.status, 400);
  assert.equal(noOld.response.data.oldPassword.code, "validation_required");
  const wrongOld = await failure(
    fay.collection(users).update(record.id, {
      oldPassword: "not-the-password",
      password: "new-password-1",
      passwordConfirm: "new-password-1",
    }),
  );
  assert.equal(wrongOld.response.data.oldPassword.code, "validation_invalid_old_password");

  await fay.collection(users).update(record.id, {
    oldPassword: "password-123",
    password: "new-password-1",
    passwordConfirm: "new-password-1",
  });
  // the old token was signed with the previous token key
  const stale = await failure(fay.collection(users).authRefresh());
  assert.equal(stale.status, 401);
  await client().collection(users).authWithPassword("fay@example.com", "new-password-1");

  // superusers change passwords without the old one
  await pb.collection(users).update(record.id, { password: "admin-set-1", passwordConfirm: "admin-set-1" });
  await client().collection(users).authWithPassword("fay@example.com", "admin-set-1");
});

test("password reset and verification requests", async () => {
  await signUp("gus@example.com");
  assert.equal(await client().collection(users).requestPasswordReset("gus@example.com"), true);
  // unknown emails get the same answer
  assert.equal(await client().collection(users).requestPasswordReset("nobody@example.com"), true);
  assert.equal(await client().collection(users).requestVerification("gus@example.com"), true);
  const bad = await failure(client().collection(users).confirmPasswordReset("not-a-token", "x-password-1", "x-password-1"));
  assert.equal(bad.status, 400);
});

test("impersonation", async () => {
  await signUp("hal@example.com");
  const hal = await pb.collection(users).getFirstListItem('email = "hal@example.com"');
  const as = await pb.collection(users).impersonate(hal.id, 3600);
  assert.equal(as.authStore.record.id, hal.id);
  const me = await as.collection(users).getFullList();
  assert.deepEqual(
    me.map((u) => u.id),
    [hal.id],
  );

  const guest = await failure(client().collection(users).impersonate(hal.id, 60));
  assert.ok(guest.status === 401 || guest.status === 403, `status ${guest.status}`);
});

test("superuser auth", async () => {
  const err = await failure(client().collection("_superusers").authWithPassword("admin@example.com", "nope-nope-1"));
  assert.equal(err.status, 400);
  assert.equal(pb.authStore.isSuperuser, true);
  const forbidden = await failure(client().collections.getFullList());
  assert.equal(forbidden.status, 401);
});
