// Shared helpers: clients for the server under test (COMPAT_URL) and unique names, so the
// test files don't depend on each other or on what a previous run left behind.
import PocketBase from "pocketbase";

export const URL = process.env.COMPAT_URL || "http://127.0.0.1:8090";
export const EMAIL = process.env.COMPAT_EMAIL || "admin@example.com";
export const PASSWORD = process.env.COMPAT_PASSWORD || "admin-password-1";

/** A client without auth. */
export function client() {
  const pb = new PocketBase(URL);
  pb.autoCancellation(false);
  return pb;
}

/** A client signed in as the superuser. */
export async function admin() {
  const pb = client();
  await pb.collection("_superusers").authWithPassword(EMAIL, PASSWORD);
  return pb;
}

/** A unique name with the given prefix. */
export function uid(prefix) {
  return `${prefix}_${Math.random().toString(36).slice(2, 8)}`;
}

/** The error a promise rejects with (fails the test if it resolves). */
export async function failure(promise) {
  try {
    await promise;
  } catch (err) {
    return err;
  }
  throw new Error("expected the request to fail");
}

/** Waits until `check()` returns something truthy (or fails after `ms`). */
export async function eventually(check, ms = 5000) {
  const until = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > until) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 50));
  }
}
