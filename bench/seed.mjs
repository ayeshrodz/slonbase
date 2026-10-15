// Creates the benchmark collections and data on the server at BENCH_URL and writes what the
// scenarios need (ids, a user token) as JSON to stdout.
import PocketBase from "pocketbase";


async function main() {
  const url = process.env.BENCH_URL;
  const posts = Number(process.env.BENCH_POSTS || 10000);
  const pb = new PocketBase(url);
  pb.autoCancellation(false);
  await pb.collection("_superusers").authWithPassword(process.env.BENCH_EMAIL, process.env.BENCH_PASSWORD);
  await pb.settings.update({ batch: { enabled: true, maxRequests: 100, timeout: 30 } });

  const open = { listRule: "", viewRule: "", createRule: "", updateRule: "", deleteRule: "" };
  const authors = await pb.collections.create({
    name: "authors",
    ...open,
    fields: [
      { name: "name", type: "text" },
      { name: "created", type: "autodate", onCreate: true },
    ],
  });
  await pb.collections.create({
    name: "posts",
    ...open,
    fields: [
      { name: "title", type: "text", required: true },
      { name: "body", type: "text" },
      { name: "views", type: "number" },
      { name: "published", type: "bool" },
      { name: "tags", type: "select", maxSelect: 3, values: ["news", "tech", "life", "misc"] },
      { name: "author", type: "relation", collectionId: authors.id, maxSelect: 1 },
      { name: "created", type: "autodate", onCreate: true },
    ],
    indexes: ["CREATE INDEX idx_posts_views ON posts (views)"],
  });
  await pb.collections.create({
    name: "bench_users",
    type: "auth",
    ...open,
    fields: [{ name: "name", type: "text" }],
  });

  const authorIds = [];
  for (let i = 0; i < 100; i += 50) {
    const batch = pb.createBatch();
    for (let j = i; j < i + 50; j++) batch.collection("authors").create({ name: `Author ${j}` });
    for (const r of await batch.send()) authorIds.push(r.body.id);
  }
  const tags = ["news", "tech", "life", "misc"];
  const words = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor".split(" ");
  let firstPost = "";
  for (let i = 0; i < posts; i += 100) {
    const batch = pb.createBatch();
    for (let j = i; j < Math.min(i + 100, posts); j++) {
      batch.collection("posts").create({
        title: `Post ${j}`,
        body: Array.from({ length: 40 }, (_, k) => words[(j + k) % words.length]).join(" "),
        views: (j * 7919) % 1000,
        published: j % 3 !== 0,
        tags: [tags[j % 4], tags[(j + 1) % 4]],
        author: authorIds[j % authorIds.length],
      });
    }
    const results = await batch.send();
    if (!firstPost) firstPost = results[0].body.id;
  }

  await pb.collection("bench_users").create({ email: "bench@example.com", password: "bench-password-1", passwordConfirm: "bench-password-1" });
  const user = new PocketBase(url);
  const auth = await user.collection("bench_users").authWithPassword("bench@example.com", "bench-password-1");
  console.log(JSON.stringify({ post: firstPost, author: authorIds[0], token: auth.token }));
}

main().catch((err) => {
  console.error(`seeding failed: ${err.url || ""} ${err.status || ""} ${JSON.stringify(err.response || err.message)}`);
  process.exit(1);
});
