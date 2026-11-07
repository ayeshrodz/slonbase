// Seeds a demo installation (users, projects, tasks, comments) for the screenshots and the tour:
//   node seed.mjs http://127.0.0.1:8222
const base = process.argv[2] || "http://127.0.0.1:8222";
const email = "admin@slonbase.dev", password = "admin-password-1";
async function api(method, url, body, token) {
  const res = await fetch(base + url, { method, headers: { "content-type": "application/json", ...(token ? { authorization: token } : {}) }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  const json = text ? JSON.parse(text) : null;
  return { status: res.status, json };
}
async function must(p, what) { const r = await p; if (r.status >= 300) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.json)}`); return r.json; }
const token = (await must(api("POST", "/api/collections/_superusers/auth-with-password", { identity: email, password }), "login")).token;
const T = (m, u, b) => must(api(m, u, b, token), `${m} ${u}`);

// users (auth)
const users = await T("POST", "/api/collections", { name: "users", type: "auth", listRule: "id = @request.auth.id", viewRule: "id = @request.auth.id", fields: [{ name: "name", type: "text" }] });
const people = ["Kwame Mensah", "Lena Novak", "Maya Lindqvist", "Tomás Herrera", "Aiko Tanaka", "Priya Raman"];
const userIds = [];
for (const [i, name] of people.entries()) {
  const r = await T("POST", "/api/collections/users/records", { name, email: `${name.split(" ")[0].toLowerCase().normalize("NFD").replace(/[^a-z]/g, "")}@example.com`, password: "password-" + i + "1234", passwordConfirm: "password-" + i + "1234", verified: true });
  userIds.push(r.id);
}
const projects = await T("POST", "/api/collections", { name: "projects", listRule: "", viewRule: "", fields: [{ name: "name", type: "text", required: true, presentable: true }, { name: "archived", type: "bool" }] });
const projNames = ["Platform", "Website", "Billing", "Mobile app", "Design system"];
const projIds = [];
for (const name of projNames) projIds.push((await T("POST", "/api/collections/projects/records", { name })).id);
const tasks = await T("POST", "/api/collections", {
  name: "tasks", listRule: "", viewRule: "", createRule: "@request.auth.id != ''", updateRule: "@request.auth.id != ''", deleteRule: "@request.auth.id != ''",
  fields: [
    { name: "title", type: "text", required: true, max: 120 },
    { name: "project", type: "relation", collectionId: projects.id, maxSelect: 1 },
    { name: "assignee", type: "relation", collectionId: users.id, maxSelect: 1 },
    { name: "status", type: "select", values: ["backlog", "todo", "in progress", "review", "done"], maxSelect: 1 },
    { name: "priority", type: "number", onlyInt: true, min: 1, max: 4 },
    { name: "estimate", type: "number", onlyInt: true },
    { name: "labels", type: "select", values: ["bug", "feature", "docs", "infra", "design"], maxSelect: 3 },
    { name: "notes", type: "editor" },
    { name: "created", type: "autodate", onCreate: true },
    { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
  ],
  indexes: ["CREATE INDEX idx_tasks_status ON tasks (status)", "CREATE INDEX idx_tasks_project ON tasks (project)"],
});
const comments = await T("POST", "/api/collections", { name: "comments", listRule: "", viewRule: "", createRule: "@request.auth.id != ''", fields: [
  { name: "task", type: "relation", collectionId: tasks.id, maxSelect: 1, cascadeDelete: true }, { name: "author", type: "relation", collectionId: users.id, maxSelect: 1 }, { name: "body", type: "text" } ] });
const titles = ["Dark mode for settings screen", "Fix crash on Android 12 cold start", "Push notification preferences", "Offline sync for drafts", "Mentions in comments", "Bulk edit tasks", "Audit log", "Weekly digest email", "Image uploads in comments", "Two-factor sign-in", "Slack integration", "CSV export for reports", "Keyboard shortcuts", "Rate limits per API key", "Faster search", "Onboarding checklist"];
const statuses = ["done", "todo", "review", "backlog", "in progress"];
const labelSets = [["infra"], ["bug", "infra"], ["feature"], ["design"], [], ["docs"], ["bug"]];
const taskIds = [];
for (let i = 0; i < 64; i++) {
  const title = `${titles[i % titles.length]} (${Math.floor(i / titles.length) + 2 + (i % 3)})`;
  const r = await T("POST", "/api/collections/tasks/records", {
    title, project: projIds[(i * 3) % 5], assignee: i % 7 === 6 ? "" : userIds[(i * 5) % 6], status: statuses[(i * 3) % 5],
    priority: (i * 7) % 4 + 1, estimate: [1, 2, 3, 5, 8][(i * 3) % 5], labels: labelSets[i % labelSets.length],
    notes: `<p>Notes for <strong>${title}</strong>: what to build, how we know it is done, and who to ask.</p><ul><li>Spec reviewed</li><li>Tests added</li></ul>`,
  });
  taskIds.push(r.id);
}
for (let i = 0; i < 40; i++) await T("POST", "/api/collections/comments/records", { task: taskIds[i % taskIds.length], author: userIds[i % 6], body: ["Looks good to me.", "Can we ship this on Friday?", "Blocked on the design review.", "Added a test for the edge case."][i % 4] });
console.log("seeded");
