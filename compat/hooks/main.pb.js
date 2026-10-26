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
