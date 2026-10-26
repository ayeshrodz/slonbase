// a log record for tests/execute.test.mjs and tests/model.test.mjs
module.exports = (app, title, collection) => {
  const log = new Record(app.findCollectionByNameOrId(collection || "hooks_exec_log"));
  log.set("title", title);
  app.save(log);
};
