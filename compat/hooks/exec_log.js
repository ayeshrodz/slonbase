// a hooks_exec_log record (tests/execute.test.mjs)
module.exports = (app, title) => {
  const log = new Record(app.findCollectionByNameOrId("hooks_exec_log"));
  log.set("title", title);
  app.save(log);
};
