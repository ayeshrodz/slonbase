// How long a cascading delete and a batch take: deleting a record that 1,000 and 10,000 records
// refer to (relations with cascade delete), without and with a delete hook on the referring
// collection, and a batch of 100 creates against 100 single requests. See README.md.
//
//   ADMIN_DATABASE_URL=postgres://... SLONBASE_BIN=../target/release/slonbase node cascade.mjs
//
// CASCADE_SIZES (1000,10000) and CASCADE_ROUNDS (5, for the batch) change what runs.
import fs from "node:fs";
import { api, start, stop } from "./lib.mjs";

const sizes = (process.env.CASCADE_SIZES || "1000,10000").split(",").map(Number);
const rounds = Number(process.env.CASCADE_ROUNDS || 5);
const open = { listRule: "", viewRule: "", createRule: "", updateRule: "", deleteRule: "" };

async function must(promise, what) {
  const res = await promise;
  if (res.status >= 300) throw new Error(`${what}: ${res.status} ${res.text}`);
  return res;
}

async function insertAll(server, collection, rows) {
  for (let i = 0; i < rows.length; i += 100) {
    const requests = rows.slice(i, i + 100).map((body) => ({
      method: "POST",
      url: `/api/collections/${collection}/records`,
      body,
    }));
    await must(api(server, "POST", "/api/batch", { requests }), "seeding");
  }
}

const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

async function cascade(withHook) {
  const hooks = withHook ? { "main.pb.js": `onRecordDelete((e) => { e.next(); }, "children");` } : {};
  const server = await start({ hooks });
  const rows = [];
  try {
    await must(api(server, "PATCH", "/api/settings", { batch: { enabled: true, maxRequests: 100, timeout: 120 } }), "settings");
    const parents = (
      await must(api(server, "POST", "/api/collections", { name: "parents", ...open, fields: [{ name: "name", type: "text" }] }), "parents")
    ).body;
    await must(
      api(server, "POST", "/api/collections", {
        name: "children",
        ...open,
        fields: [
          { name: "name", type: "text" },
          { name: "parent", type: "relation", collectionId: parents.id, maxSelect: 1, cascadeDelete: true },
        ],
      }),
      "children",
    );
    for (const size of sizes) {
      const parent = (await must(api(server, "POST", "/api/collections/parents/records", { name: `parent ${size}` }), "parent")).body;
      await insertAll(server, "children", Array.from({ length: size }, (_, i) => ({ name: `child ${i}`, parent: parent.id })));
      const started = performance.now();
      await must(api(server, "DELETE", `/api/collections/parents/records/${parent.id}`), "delete");
      rows.push({ scenario: `delete a record with ${size} cascaded`, hook: withHook, ms: Math.round(performance.now() - started) });
    }
    // a batch of 100 creates against 100 single requests (the same records)
    const batch = [];
    const single = [];
    for (let round = 0; round < rounds; round++) {
      const body = (i) => ({ name: `row ${round}-${i}` });
      const requests = Array.from({ length: 100 }, (_, i) => ({ method: "POST", url: "/api/collections/parents/records", body: body(i) }));
      let started = performance.now();
      await must(api(server, "POST", "/api/batch", { requests }), "batch");
      batch.push(performance.now() - started);
      started = performance.now();
      for (let i = 0; i < 100; i++) await must(api(server, "POST", "/api/collections/parents/records", body(i + 100)), "single");
      single.push(performance.now() - started);
    }
    rows.push({ scenario: "batch of 100 creates", hook: withHook, ms: Math.round(median(batch)) });
    rows.push({ scenario: "100 single creates, one after another", hook: withHook, ms: Math.round(median(single)) });
  } finally {
    await stop(server);
  }
  return rows;
}

const all = [...(await cascade(false)), ...(await cascade(true))];
fs.mkdirSync("results", { recursive: true });
fs.writeFileSync("results/cascade.json", JSON.stringify(all, null, 2));
console.log("| Scenario | Delete hook on the referring collection | Time (ms) |");
console.log("|---|---|--:|");
for (const r of all) console.log(`| ${r.scenario} | ${r.hook ? "yes" : "no"} | ${r.ms} |`);
