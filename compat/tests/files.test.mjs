// File fields through the SDK: multipart uploads, file URLs and content, thumbnails,
// appending and removing files, and protected files with file tokens.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { admin, client, uid, failure } from "./lib.mjs";

// a 2x2 PNG
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR4nGP4z8DwHwyBNAMDA8N/BgYAP/8H+dYzcN0AAAAASUVORK5CYII=",
  "base64",
);

let pb;
const docs = uid("docs");
const vault = uid("vault");

before(async () => {
  pb = await admin();
  await pb.collections.create({
    name: docs,
    type: "base",
    listRule: "",
    viewRule: "",
    createRule: "",
    updateRule: "",
    fields: [
      { name: "title", type: "text" },
      { name: "files", type: "file", maxSelect: 5, maxSize: 1 << 20, thumbs: ["10x10"] },
      { name: "images", type: "file", maxSelect: 1, maxSize: 1 << 20, mimeTypes: ["image/png"] },
      { name: "small", type: "file", maxSelect: 1, maxSize: 4 },
    ],
  });
  // only superusers can view: protected files need a file token
  await pb.collections.create({
    name: vault,
    type: "base",
    createRule: "",
    fields: [{ name: "private", type: "file", maxSelect: 1, maxSize: 1 << 20, protected: true }],
  });
});

after(async () => {
  await pb.collections.delete(docs).catch(() => {});
  await pb.collections.delete(vault).catch(() => {});
});

function form(fields, files) {
  const data = new FormData();
  for (const [k, v] of Object.entries(fields)) data.append(k, v);
  for (const [k, name, content, type] of files) {
    data.append(k, new Blob([content], { type: type || "application/octet-stream" }), name);
  }
  return data;
}

async function download(url) {
  const res = await fetch(url);
  return { status: res.status, type: res.headers.get("content-type"), body: Buffer.from(await res.arrayBuffer()) };
}

test("upload and serve", async () => {
  const guest = client();
  const record = await guest.collection(docs).create(
    form({ title: "two files" }, [
      ["files", "notes.txt", "hello"],
      ["files", "pic.png", PNG, "image/png"],
    ]),
  );
  assert.equal(record.files.length, 2);
  const [txt, png] = record.files;
  assert.match(txt, /^notes_\w+\.txt$/);
  assert.match(png, /^pic_\w+\.png$/);

  const url = guest.files.getURL(record, txt);
  const file = await download(url);
  assert.equal(file.status, 200);
  assert.equal(file.body.toString(), "hello");

  const image = await download(guest.files.getURL(record, png));
  assert.equal(image.type, "image/png");
  const thumb = await download(guest.files.getURL(record, png, { thumb: "10x10" }));
  assert.equal(thumb.status, 200);
  assert.ok(thumb.type.startsWith("image/"), thumb.type);

  const missing = await download(guest.files.getURL(record, "missing_abc.txt"));
  assert.equal(missing.status, 404);
});

test("append, remove and replace", async () => {
  const c = client().collection(docs);
  const record = await c.create(form({}, [["files", "a.txt", "a"]]));
  const appended = await c.update(record.id, form({}, [["files+", "b.txt", "b"]]));
  assert.equal(appended.files.length, 2);
  assert.equal(appended.files[0], record.files[0]);

  const removed = await c.update(record.id, { "files-": [record.files[0]] });
  assert.deepEqual(removed.files, [appended.files[1]]);

  const replaced = await c.update(record.id, form({}, [["files", "c.txt", "c"]]));
  assert.equal(replaced.files.length, 1);
  assert.match(replaced.files[0], /^c\w*_\w+\.txt$/);

  const cleared = await c.update(record.id, { files: [] });
  assert.deepEqual(cleared.files, []);
});

test("file validation", async () => {
  const c = client().collection(docs);
  const tooMany = await failure(
    c.create(
      form(
        {},
        Array.from({ length: 6 }, (_, i) => ["files", `f${i}.txt`, "x"]),
      ),
    ),
  );
  assert.equal(tooMany.status, 400);
  assert.ok(tooMany.response.data.files, JSON.stringify(tooMany.response.data));

  const wrongType = await failure(c.create(form({}, [["images", "notes.txt", "not an image"]])));
  assert.equal(wrongType.status, 400);
  assert.equal(wrongType.response.data.images.code, "validation_invalid_mime_type");

  const tooLarge = await failure(c.create(form({}, [["small", "big.txt", "more than four bytes"]])));
  assert.equal(tooLarge.status, 400);
  assert.equal(tooLarge.response.data.small.code, "validation_file_size_limit");
});

test("protected files", async () => {
  const record = await client().collection(vault).create(form({}, [["private", "secret.txt", "top secret"]]));
  const name = record.private;
  const plain = await download(client().files.getURL(record, name));
  assert.ok(plain.status >= 400, `a guest without a token gets ${plain.status}`);
  const noToken = await download(pb.files.getURL(record, name));
  assert.ok(noToken.status >= 400, `a superuser without a token gets ${noToken.status}`);

  const token = await pb.files.getToken();
  assert.equal(typeof token, "string");
  const withToken = await download(pb.files.getURL(record, name, { token }));
  assert.equal(withToken.status, 200);
  assert.equal(withToken.body.toString(), "top secret");

  const guestToken = await failure(client().files.getToken());
  assert.ok(guestToken.status >= 400);
});
