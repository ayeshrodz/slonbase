// Hooks for tests/hooks.test.mjs, loaded by both servers. They only act on the
// `hooks_users` collection (created by the test), so the other tests don't see them.

// templates: {template, data} rendered with $template.loadString()
routerAdd("POST", "/compat/template", (e) => {
  const body = e.requestInfo().body;
  try {
    // data made in JavaScript, as hooks usually pass it
    const data = JSON.parse(JSON.stringify(body.data));
    return e.string(200, $template.loadString(body.template).render(data));
  } catch (err) {
    return e.json(400, { error: String(err) });
  }
});

routerAdd("GET", "/compat/template-files", (e) => {
  const html = $template
    .loadFiles(`${__hooks}/views/layout.html`, `${__hooks}/views/page.html`)
    .render({ title: "Hi <b>", items: ["a", "b"], user: { name: "Ann" } });
  return e.html(200, html);
});

// tokens and emails of a hooks_users record
routerAdd("POST", "/compat/hooks-users/{id}/{action}", (e) => {
  const record = $app.findRecordById("hooks_users", e.request.pathValue("id"));
  switch (e.request.pathValue("action")) {
    case "verification-token":
      return e.json(200, { token: record.newVerificationToken() });
    case "password-reset-token":
      return e.json(200, { token: record.newPasswordResetToken() });
    case "send-verification":
      $mails.sendRecordVerification($app, record);
      return e.noContent(204);
    case "send-password-reset":
      $mails.sendRecordPasswordReset($app, record);
      return e.noContent(204);
  }
  return e.json(404, {});
}, $apis.requireSuperuserAuth());

// emails to hooks_users are kept in hooks_mails instead of being sent
function keepMail(e) {
  const mail = new Record($app.findCollectionByNameOrId("hooks_mails"));
  mail.set("to", e.message.to[0].address);
  mail.set("subject", e.message.subject);
  mail.set("html", e.message.html);
  $app.save(mail);
}
onMailerRecordVerificationSend(keepMail, "hooks_users");
onMailerRecordPasswordResetSend(keepMail, "hooks_users");
onMailerRecordOTPSend(keepMail, "hooks_users");
onMailerRecordEmailChangeSend(keepMail, "hooks_users");

// sign in with "@name" as well as the email
onRecordAuthWithPasswordRequest((e) => {
  if (e.identity.startsWith("@")) {
    try {
      e.record = $app.findFirstRecordByData("hooks_users", "name", e.identity.slice(1));
    } catch (_) {
      e.record = null;
    }
  }
  if (e.identity.startsWith("banned")) throw new ForbiddenError("Banned.");
  e.next();
}, "hooks_users");

onRecordAuthRequest((e) => {
  e.meta = { method: e.authMethod, custom: true };
  e.record.set("name", e.record.getString("name").toUpperCase());
  e.next();
}, "hooks_users");

onRecordAuthRefreshRequest((e) => {
  if (e.record.getString("name") === "frozen") throw new BadRequestError("No refresh.");
  e.next();
}, "hooks_users");

onRecordRequestOTPRequest((e) => {
  e.password = "424242";
  e.next();
}, "hooks_users");

onRecordRequestPasswordResetRequest((e) => {
  if (e.record.email().endsWith("@blocked.test")) return e.json(200, { blocked: true });
  e.next();
}, "hooks_users");

onRecordConfirmVerificationRequest((e) => {
  e.next();
  const mail = new Record($app.findCollectionByNameOrId("hooks_mails"));
  mail.set("to", e.record.email());
  mail.set("subject", "verified");
  $app.save(mail);
}, "hooks_users");

// every hooks_posts record a client gets: a computed key, a hidden field shown, one hidden
onRecordEnrich((e) => {
  e.record.withCustomData(true);
  e.record.set("titleUpper", e.record.getString("title").toUpperCase());
  e.record.unhide("note");
  e.record.hide("secret");
  e.next();
}, "hooks_posts");

// hooks_users records too: in lists, expanded relations and auth responses
onRecordEnrich((e) => {
  e.record.withCustomData(true);
  e.record.set("initial", e.record.getString("name").slice(0, 1));
  e.next();
}, "hooks_users");

// ---------------------------------------------------------------- realtime, files, settings, backups

// subscriptions to hooks_secret topics are dropped
onRealtimeSubscribeRequest((e) => {
  e.subscriptions = e.subscriptions.filter((t) => !t.startsWith("hooks_secret"));
  e.next();
});

// hooks_posts messages titled "rt-..." get a "!", "rt-skip" isn't sent
onRealtimeMessageSend((e) => {
  if (e.message.name !== "hooks_posts/*") return e.next();
  const data = JSON.parse(toString(e.message.data));
  if (!String(data.record.title).startsWith("rt-")) return e.next();
  if (data.record.title === "rt-skip") return;
  data.record.title += "!";
  e.message.data = JSON.stringify(data);
  e.next();
});

onFileDownloadRequest((e) => {
  if (e.servedName.startsWith("secret")) throw new ForbiddenError("No secrets.");
  e.servedName = "renamed-" + e.servedName;
  e.next();
}, "hooks_files");

onFileTokenRequest((e) => {
  if (e.record.getString("name") === "frozen") throw new ForbiddenError("No file tokens.");
  e.next();
}, "hooks_users");

onSettingsListRequest((e) => {
  if (e.requestInfo().headers["x_hooks_test"] === "1") e.settings.meta.appName = "Hooked";
  e.next();
});

onSettingsUpdateRequest((e) => {
  if (e.newSettings.meta.appName === "Rejected") throw new BadRequestError("Rejected by hook.");
  e.next();
});

onBackupCreate((e) => {
  if (e.name.startsWith("compat_")) e.name = "hooked_" + e.name;
  e.next();
});

// ---------------------------------------------------------------- collections and batches

onCollectionsListRequest((e) => {
  if (e.requestInfo().headers["x_hooks_test"] === "1") e.result.totalItems = 42;
  e.next();
});

onCollectionViewRequest((e) => {
  if (e.collection.name === "hooks_c_locked") throw new ForbiddenError("Locked.");
  e.next();
});

onCollectionCreateRequest((e) => {
  if (e.collection.name === "hooks_c_bad") throw new BadRequestError("Bad collection.");
  if (e.collection.name.startsWith("hooks_c_")) e.collection.listRule = "";
  e.next();
});

onCollectionUpdateRequest((e) => {
  if (e.collection.name.startsWith("hooks_c_")) e.collection.viewRule = "id != ''";
  e.next();
});

onCollectionDeleteRequest((e) => {
  if (e.collection.name === "hooks_c_keep" && !e.requestInfo().query["force"]) throw new BadRequestError("Keep it.");
  e.next();
});

onCollectionsImportRequest((e) => {
  if (e.collectionsData.some((c) => c.name === "hooks_c_noimport")) throw new BadRequestError("No import.");
  e.next();
});

onBatchRequest((e) => {
  for (const r of e.batch) {
    if (!r.body || typeof r.body.title !== "string" || !r.body.title.startsWith("batch-")) continue;
    if (r.body.title === "batch-refuse") throw new BadRequestError("Refused batch.");
    r.body.title += "+";
  }
  e.next();
});

// $dbx and the $app.db() builder against the dbx_items collection (made by
// tests/dbx.test.mjs): each case's rows, or its error
routerAdd("GET", "/compat/dbx", (e) => {
  const t = "dbx_items";
  const rows = (q, shape) => {
    const out = arrayOf(new DynamicModel(shape || { title: "" }));
    q.all(out);
    return out.map((r) => Object.assign({}, r));
  };
  const titles = (where, order) =>
    rows($app.db().select("title").from(t).where(where).orderBy(order || "title"));
  const cases = {
    hash: () => titles($dbx.hashExp({ tag: "a" })),
    hashNullAndList: () => titles($dbx.hashExp({ tag: ["a", "b"], extra: null })),
    in: () => titles($dbx.in("title", "one", "three")),
    inOne: () => titles($dbx.in("title", "two")),
    inNone: () => titles($dbx.in("title")),
    notIn: () => titles($dbx.notIn("title", "one", "three")),
    like: () => titles($dbx.like("title", "o")),
    likeMatch: () => titles($dbx.like("title", "t").match(false, true)),
    orLike: () => titles($dbx.orLike("title", "one", "two")),
    notLike: () => titles($dbx.notLike("title", "o")),
    orNotLike: () => titles($dbx.orNotLike("title", "one", "two")),
    between: () => titles($dbx.between("n", 2, 3)),
    notBetween: () => titles($dbx.notBetween("n", 2, 3)),
    not: () => titles($dbx.not($dbx.hashExp({ tag: "a" }))),
    andOr: () =>
      titles($dbx.or($dbx.and($dbx.hashExp({ tag: "a" }), $dbx.exp("n > {:n}", { n: 1 })), $dbx.hashExp({ title: "four" }))),
    exists: () =>
      titles($dbx.exists($dbx.exp(`SELECT 1 FROM ${t} x WHERE x.n > {:big}`, { big: 3 }))),
    notExists: () =>
      titles($dbx.notExists($dbx.exp(`SELECT 1 FROM ${t} x WHERE x.n > {:big}`, { big: 3 }))),
    andWhereOrWhere: () =>
      rows(
        $app.db().select("title").from(t)
          .where($dbx.hashExp({ tag: "a" }))
          .andWhere($dbx.exp("n >= {:min}", { min: 2 }))
          .orWhere($dbx.hashExp({ title: "four" }))
          .orderBy("n DESC"),
      ),
    limitOffset: () => rows($app.db().select("title").from(t).orderBy("n").limit(2).offset(1)),
    offsetOnly: () => rows($app.db().select("title").from(t).orderBy("n").offset(3)),
    distinct: () => rows($app.db().select("tag").distinct(true).from(t).orderBy("tag"), { tag: "" }),
    groupHaving: () =>
      rows(
        $app.db().select("tag", "count(*) as total").from(t).groupBy("tag")
          .having($dbx.exp("count(*) > {:min}", { min: 1 })).orderBy("tag"),
        { tag: "", total: 0 },
      ),
    alias: () => rows($app.db().select("a.title AS name").from(`${t} a`).orderBy("a.n").limit(1), { name: "" }),
    join: () =>
      rows(
        $app.db().select("a.title", "b.title as other").from(`${t} a`)
          .innerJoin(`${t} b`, $dbx.exp("b.n = a.n + 1")).orderBy("a.n"),
        { title: "", other: "" },
      ),
    leftJoin: () =>
      rows(
        $app.db().select("a.title").from(`${t} a`)
          .leftJoin(`${t} b`, $dbx.exp("b.n = a.n + 10")).andSelect("coalesce(b.title, '') as missing")
          .orderBy("a.n").limit(1),
        { title: "", missing: "" },
      ),
    union: () =>
      rows(
        $app.db().select("title").from(t).where($dbx.exp("title = {:a}", { a: "one" }))
          .union($app.db().select("title").from(t).where($dbx.exp("title = {:b}", { b: "two" })).build()),
      ).map((r) => r.title).sort(),
    bind: () =>
      rows($app.db().select("title").from(t).where($dbx.exp("title = {:x}")).bind({ x: "two" })),
    one: () => {
      const m = new DynamicModel({ title: "", n: 0 });
      $app.db().select("title", "n").from(t).where($dbx.hashExp({ title: "three" })).one(m);
      return { title: m.title, n: m.n };
    },
    oneMissing: () => {
      try {
        $app.db().select("title").from(t).where($dbx.hashExp({ title: "nope" })).one(new DynamicModel({ title: "" }));
        return "found";
      } catch (err) {
        return "missing";
      }
    },
    quotedNames: () =>
      rows($app.db().newQuery(`SELECT [[title]] FROM {{${t}}} WHERE [[n]] = {:n}`).bind({ n: 2 })),
    recordQuery: () => {
      const records = arrayOf(new Record());
      $app.recordQuery(t).where($dbx.hashExp({ tag: "a" })).orderBy("n DESC").all(records);
      return records.map((r) => [r.get("title"), r.get("n"), r.collection().name]);
    },
    recordQueryOne: () => {
      const record = new Record();
      $app.recordQuery(t).andWhere($dbx.hashExp({ title: "two" })).limit(1).one(record);
      record.set("title", "two!");
      $app.save(record);
      const again = $app.findRecordById(t, record.id);
      again.set("title", "two");
      $app.save(again);
      return [record.get("tag"), record.isNew()];
    },
    builder: () => {
      const db = $app.db();
      try {
        db.dropTable("dbx_plain").execute();
      } catch (err) {}
      db.createTable("dbx_plain", { id: "INTEGER PRIMARY KEY", name: "TEXT", score: "INTEGER" }).execute();
      db.insert("dbx_plain", { id: 1, name: "a", score: 1 }).execute();
      db.insert("dbx_plain", { id: 2, name: "b", score: 2 }).execute();
      db.update("dbx_plain", { name: "b2", score: 20 }, $dbx.hashExp({ id: 2 })).execute();
      db.update("dbx_plain", { score: $dbx.exp("score + {:inc}", { inc: 5 }) }, $dbx.hashExp({ id: 1 })).execute();
      db.insert("dbx_plain", { id: 3, name: "c", score: 3 }).execute();
      db.delete("dbx_plain", $dbx.hashExp({ name: "c" })).execute();
      db.createIndex("dbx_plain", "dbx_plain_name", "name").execute();
      const out = rows(db.select("id", "name", "score").from("dbx_plain").orderBy("id"), { id: 0, name: "", score: 0 });
      db.dropTable("dbx_plain").execute();
      return out;
    },
  };
  const result = {};
  for (const [name, run] of Object.entries(cases)) {
    try {
      result[name] = run();
    } catch (err) {
      result[name] = { error: String(err) };
    }
  }
  return e.json(200, result);
}, $apis.requireSuperuserAuth());

// execute and after-error hooks (tests/execute.test.mjs): they act on hooks_exec titles and
// log to hooks_exec_log (with exec_log.js: PocketBase runs each handler on its own)
onRecordCreateExecute((e) => {
  const title = e.record.get("title");
  if (title === "exec-refuse") throw new BadRequestError("Refused by execute.");
  if (title === "exec-change") e.record.set("title", "exec-changed");
  e.next();
  if (title === "exec-after") require(`${__hooks}/exec_log.js`)(e.app, "after-next:" + (e.record.id ? "id" : "no id"));
}, "hooks_exec");
onRecordUpdateExecute((e) => {
  if (e.record.get("title") === "exec-upd-fail") throw new BadRequestError("Refused by execute.");
  e.next();
}, "hooks_exec");
onRecordDeleteExecute((e) => {
  if (e.record.get("title") === "exec-keep") throw new BadRequestError("Refused by execute.");
  e.next();
}, "hooks_exec");
onRecordAfterCreateError((e) => {
  require(`${__hooks}/exec_log.js`)(e.app, "create-error:" + e.record.get("title"));
  e.next();
}, "hooks_exec");
onRecordAfterUpdateError((e) => {
  require(`${__hooks}/exec_log.js`)(e.app, "update-error:" + e.record.get("title"));
  e.next();
}, "hooks_exec");
onRecordAfterDeleteError((e) => {
  require(`${__hooks}/exec_log.js`)(e.app, "delete-error:" + e.record.get("title"));
  e.next();
}, "hooks_exec");
onCollectionValidate((e) => {
  if (e.collection.name === "hooks_c_exec_invalid") throw new BadRequestError("Refused by validate.");
  e.next();
});
onCollectionCreateExecute((e) => {
  if (e.collection.name === "hooks_c_exec_refuse") throw new BadRequestError("Refused by execute.");
  e.next();
});
onCollectionAfterCreateError((e) => {
  if (e.collection.name.startsWith("hooks_c_exec")) require(`${__hooks}/exec_log.js`)(e.app, "collection-error:" + e.collection.name);
  e.next();
});

// generic model hooks (tests/model.test.mjs), on hooks_model records and hooks_c_model_*
// collections, logging to hooks_model_log. The model handler is registered first, but the
// record hooks run before it.
onModelCreate((e) => {
  const title = e.model.get("title");
  if (title === "model-change") e.model.set("title", "model-changed");
  if (title === "order-record") e.model.set("title", "order-record-model");
  e.next();
}, "hooks_model");
onRecordCreate((e) => {
  if (e.record.get("title") === "order") e.record.set("title", "order-record");
  e.next();
}, "hooks_model");
onModelValidate((e) => {
  if (e.model.get("title") === "model-invalid") throw new BadRequestError("Refused by model validate.");
  e.next();
}, "hooks_model");
onModelAfterCreateSuccess((e) => {
  require(`${__hooks}/exec_log.js`)(e.app, "model-success:" + e.model.get("title"), "hooks_model_log");
  e.next();
}, "hooks_model");
onModelAfterCreateError((e) => {
  require(`${__hooks}/exec_log.js`)(e.app, "model-error:" + e.model.get("title"), "hooks_model_log");
  e.next();
}, "hooks_model");
onModelCreate((e) => {
  if (e.model.name === "hooks_c_model_refuse") throw new BadRequestError("Refused by model.");
  e.next();
}, "_collections");
onModelAfterCreateSuccess((e) => {
  if (e.model.name.startsWith("hooks_c_model")) {
    require(`${__hooks}/exec_log.js`)(e.app, "collection-success:" + e.model.name, "hooks_model_log");
  }
  e.next();
}, "_collections");

// byte and string helpers, and nullable DynamicModel columns (tests/dbx.test.mjs)
routerAdd("GET", "/compat/bytes", (e) => {
  const row = new DynamicModel({ missing: nullString(), count: nullInt(), name: "" });
  $app.db().newQuery("SELECT NULL AS missing, 3 AS count, 'x' AS name").one(row);
  return e.json(200, {
    roundTrip: toString(toBytes("héllo ✓")),
    length: toBytes("hé").length,
    row,
  });
}, $apis.requireSuperuserAuth());
