// Relation paths after @request.auth in rules: the caller's relations, joined.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { admin, client, uid } from "./lib.mjs";

let pb;
const names = {};

before(async () => {
  pb = await admin();
  for (const key of ["members", "groups", "teams", "docs"]) names[key] = uid(`ap_${key}`);
  const groups = await pb.collections.create({ name: names.groups, fields: [{ name: "name", type: "text" }] });
  const members = await pb.collections.create({
    name: names.members,
    type: "auth",
    fields: [{ name: "name", type: "text" }],
  });
  const teams = await pb.collections.create({
    name: names.teams,
    fields: [
      { name: "name", type: "text" },
      { name: "owner", type: "relation", collectionId: members.id, maxSelect: 1 },
      { name: "settings", type: "json" },
    ],
  });
  await pb.collections.update(members.id, {
    fields: [
      ...members.fields,
      { name: "team", type: "relation", collectionId: teams.id, maxSelect: 1 },
      { name: "groups", type: "relation", collectionId: groups.id, maxSelect: 5 },
    ],
  });
  await pb.collections.create({
    name: names.docs,
    fields: [
      { name: "title", type: "text" },
      { name: "team", type: "relation", collectionId: teams.id, maxSelect: 1 },
    ],
    listRule:
      "(team = @request.auth.team && @request.auth.team.owner = @request.auth.id)" +
      " || @request.auth.groups.name ?= 'editors'" +
      " || (team = @request.auth.team && @request.auth.team.settings.open = true)",
  });

  const editors = await pb.collection(names.groups).create({ name: "editors" });
  const others = await pb.collection(names.groups).create({ name: "others" });
  const ids = {};
  for (const name of ["owner", "member", "editor", "loner"]) {
    const groups = name === "editor" ? [others.id, editors.id] : [others.id];
    ids[name] = (
      await pb.collection(names.members).create({
        email: `${name}@${names.members}.test`,
        password: "password-123",
        passwordConfirm: "password-123",
        name,
        groups,
      })
    ).id;
  }
  const red = await pb.collection(names.teams).create({ name: "red", owner: ids.owner, settings: { open: false } });
  const blue = await pb.collection(names.teams).create({ name: "blue", owner: ids.loner, settings: { open: true } });
  for (const name of ["owner", "member", "editor"]) await pb.collection(names.members).update(ids[name], { team: red.id });
  await pb.collection(names.members).update(ids.loner, { team: blue.id });
  await pb.collection(names.docs).create({ title: "red doc", team: red.id });
  await pb.collection(names.docs).create({ title: "blue doc", team: blue.id });
});

async function titles(name) {
  const c = client();
  if (name) await c.collection(names.members).authWithPassword(`${name}@${names.members}.test`, "password-123");
  return (await c.collection(names.docs).getFullList({ sort: "title" })).map((d) => d.title);
}

test("@request.auth relation paths in rules", async () => {
  assert.deepEqual(await titles("owner"), ["red doc"], "the team's owner");
  assert.deepEqual(await titles("member"), [], "a member who doesn't own the team");
  assert.deepEqual(await titles("editor"), ["blue doc", "red doc"], "any of the caller's groups");
  assert.deepEqual(await titles("loner"), ["blue doc"], "a JSON value through the relation");
  assert.deepEqual(await titles(null), [], "guests");
});
