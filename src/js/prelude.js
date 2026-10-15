// The JavaScript API of slonbase's hooks (`*.pb.js`), modeled on PocketBase's JSVM.
// It runs once in every runtime before the hook files. The Rust side provides
// __native_call(name, argsJson), __native_next(stateJson) and __native_log(level, message);
// both calls return {"ok": value} or {"err": {status, message, data}} as JSON.
(function (g) {
  "use strict";

  const nativeCall = g.__native_call;
  const nativeNext = g.__native_next;
  const nativeLog = g.__native_log;
  delete g.__native_call;
  delete g.__native_next;
  delete g.__native_log;

  // ------------------------------------------------------------------ errors

  class ApiError extends Error {
    constructor(status, message, data) {
      super(message || "Something went wrong while processing your request.");
      this.name = "ApiError";
      this.status = status || 400;
      this.data = data || {};
    }
  }
  class BadRequestError extends ApiError {
    constructor(message, data) {
      super(400, message || "Something went wrong while processing your request.", data);
      this.name = "BadRequestError";
    }
  }
  class UnauthorizedError extends ApiError {
    constructor(message, data) {
      super(401, message || "Missing or invalid authentication.", data);
      this.name = "UnauthorizedError";
    }
  }
  class ForbiddenError extends ApiError {
    constructor(message, data) {
      super(403, message || "You are not allowed to perform this request.", data);
      this.name = "ForbiddenError";
    }
  }
  class NotFoundError extends ApiError {
    constructor(message, data) {
      super(404, message || "The requested resource wasn't found.", data);
      this.name = "NotFoundError";
    }
  }
  class TooManyRequestsError extends ApiError {
    constructor(message, data) {
      super(429, message || "Too Many Requests.", data);
      this.name = "TooManyRequestsError";
    }
  }
  class InternalServerError extends ApiError {
    constructor(message, data) {
      super(500, message || "Something went wrong while processing your request.", data);
      this.name = "InternalServerError";
    }
  }

  function toError(e) {
    const classes = {
      400: BadRequestError,
      401: UnauthorizedError,
      403: ForbiddenError,
      404: NotFoundError,
      429: TooManyRequestsError,
      500: InternalServerError,
    };
    const Class = classes[e.status];
    return Class ? new Class(e.message, e.data) : new ApiError(e.status, e.message, e.data);
  }

  function errorJson(err) {
    if (err instanceof ApiError) {
      return { status: err.status, message: err.message, data: err.data || {} };
    }
    // other errors don't reach the client (like PocketBase), but they're logged
    const text = err && err.stack ? String(err) + "\n" + err.stack : String(err);
    return { status: 400, message: "Something went wrong while processing your request.", data: {}, details: text };
  }

  function unwrap(raw) {
    const res = JSON.parse(raw);
    if (res.err) throw toError(res.err);
    return res.ok;
  }

  function call(name, ...args) {
    return unwrap(nativeCall(name, JSON.stringify(args === undefined ? [] : args)));
  }

  function callNext(state) {
    return unwrap(nativeNext(JSON.stringify(state || {})));
  }

  // ------------------------------------------------------------------ console

  function format(args) {
    return args
      .map((a) => {
        if (typeof a === "string") return a;
        if (a instanceof Error) return String(a);
        try {
          return JSON.stringify(a);
        } catch (_) {
          return String(a);
        }
      })
      .join(" ");
  }
  g.console = {
    log: (...a) => nativeLog("info", format(a)),
    info: (...a) => nativeLog("info", format(a)),
    debug: (...a) => nativeLog("debug", format(a)),
    warn: (...a) => nativeLog("warn", format(a)),
    error: (...a) => nativeLog("error", format(a)),
  };

  // ------------------------------------------------------------------ records

  const NON_FIELDS = ["collectionId", "collectionName", "expand"];

  function clone(v) {
    return v === undefined ? undefined : JSON.parse(JSON.stringify(v));
  }

  class Record {
    // `collection` is a collection from $app.findCollectionByNameOrId() (or its name)
    constructor(collection, data) {
      Object.defineProperty(this, "_c", { value: null, writable: true });
      Object.defineProperty(this, "_ref", { value: null, writable: true });
      Object.defineProperty(this, "_data", { value: {}, writable: true });
      Object.defineProperty(this, "_original", { value: null, writable: true });
      Object.defineProperty(this, "_expand", { value: {}, writable: true });
      if (typeof collection === "string") {
        this._ref = { id: "", name: collection };
      } else if (collection) {
        this._c = collection;
        this._ref = { id: collection.id, name: collection.name };
      }
      if (data) this.load(data);
    }

    static _stored(data) {
      const ref = { id: data.collectionId || "", name: data.collectionName || "" };
      const record = new Record(null, data);
      record._ref = ref;
      record._original = clone(record._data);
      return record;
    }

    collection() {
      if (!this._c && this._ref) this._c = $app.findCollectionByNameOrId(this._ref.name || this._ref.id);
      return this._c;
    }
    get id() {
      return this._data.id || "";
    }
    set id(v) {
      this._data.id = v;
    }
    isNew() {
      return this._original === null;
    }
    markAsNew() {
      this._original = null;
    }
    get(key) {
      const v = this._data[key];
      return v === undefined ? null : v;
    }
    set(key, value) {
      this._data[key] = value;
    }
    load(data) {
      for (const key of Object.keys(data || {})) {
        if (key === "expand") {
          this._expand = data.expand || {};
        } else if (!NON_FIELDS.includes(key)) {
          this._data[key] = data[key];
        }
      }
    }
    getString(key) {
      const v = this.get(key);
      if (v === null) return "";
      return typeof v === "object" ? JSON.stringify(v) : String(v);
    }
    getInt(key) {
      const n = Number(this.get(key));
      return Number.isFinite(n) ? Math.trunc(n) : 0;
    }
    getFloat(key) {
      const n = Number(this.get(key));
      return Number.isFinite(n) ? n : 0;
    }
    getBool(key) {
      const v = this.get(key);
      return v === true || v === 1 || v === "true" || v === "1";
    }
    getStringSlice(key) {
      const v = this.get(key);
      if (Array.isArray(v)) return v.map(String);
      return v === null || v === "" ? [] : [String(v)];
    }
    getDateTime(key) {
      return this.getString(key);
    }
    email() {
      return this.getString("email");
    }
    setEmail(v) {
      this.set("email", v);
    }
    verified() {
      return this.getBool("verified");
    }
    setVerified(v) {
      this.set("verified", !!v);
    }
    emailVisibility() {
      return this.getBool("emailVisibility");
    }
    setEmailVisibility(v) {
      this.set("emailVisibility", !!v);
    }
    setPassword(v) {
      this.set("password", v);
    }
    validatePassword(password) {
      return call("validatePassword", this._ref.name || this._ref.id, this.id, String(password));
    }
    newAuthToken() {
      return call("authToken", this._ref.name || this._ref.id, this.id);
    }
    isSuperuser() {
      return this._ref && this._ref.name === "_superusers";
    }
    expandedOne(key) {
      const v = this._expand[key];
      if (!v) return null;
      const one = Array.isArray(v) ? v[0] : v;
      return one ? Record._stored(one) : null;
    }
    expandedAll(key) {
      const v = this._expand[key];
      if (!v) return [];
      return (Array.isArray(v) ? v : [v]).map((r) => Record._stored(r));
    }
    expand() {
      return clone(this._expand);
    }
    // the fields that changed since the record was loaded (all of them for new records)
    _changes() {
      if (this._original === null) return clone(this._data);
      const out = {};
      for (const key of Object.keys(this._data)) {
        if (JSON.stringify(this._data[key]) !== JSON.stringify(this._original[key])) {
          out[key] = clone(this._data[key]);
        }
      }
      out.id = this.id;
      return out;
    }
    _fields() {
      return clone(this._data);
    }
    publicExport() {
      const out = { collectionId: this._ref ? this._ref.id : "", collectionName: this._ref ? this._ref.name : "" };
      for (const key of Object.keys(this._data)) {
        if (key !== "password" && key !== "passwordConfirm" && key !== "oldPassword") out[key] = this._data[key];
      }
      if (Object.keys(this._expand).length) out.expand = clone(this._expand);
      return out;
    }
    toJSON() {
      return this.publicExport();
    }
    clone() {
      const r = new Record(this._c || (this._ref && this._ref.name), this._data);
      r._ref = this._ref;
      r._original = clone(this._original);
      return r;
    }
  }

  // ------------------------------------------------------------------ $app

  function stored(raw) {
    return raw ? Record._stored(raw) : null;
  }

  class DynamicModel {
    constructor(shape) {
      Object.assign(this, shape || {});
    }
  }
  function arrayOf(model) {
    const items = [];
    Object.defineProperty(items, "_model", { value: model });
    return items;
  }

  class Query {
    constructor(sql) {
      this.sql = sql;
      this.params = {};
    }
    bind(params) {
      Object.assign(this.params, params || {});
      return this;
    }
    all(result) {
      const rows = call("dbQuery", this.sql, this.params, "all");
      if (Array.isArray(result)) {
        result.length = 0;
        for (const row of rows) result.push(Object.assign(new DynamicModel(), row));
        return result;
      }
      return rows;
    }
    one(result) {
      const rows = call("dbQuery", this.sql, this.params, "all");
      if (!rows.length) throw new NotFoundError("sql: no rows in result set");
      if (result && typeof result === "object") return Object.assign(result, rows[0]);
      return rows[0];
    }
    execute() {
      return { rowsAffected: call("dbQuery", this.sql, this.params, "execute") };
    }
  }

  class MailerMessage {
    constructor(data) {
      Object.assign(this, { to: [], subject: "", html: "" }, data || {});
    }
  }

  const $app = {
    findCollectionByNameOrId: (nameOrId) => call("collection", nameOrId),
    findAllCollections: () => call("collections"),
    findRecordById: (collection, id) => {
      const raw = call("findRecordById", collectionName(collection), id);
      if (!raw) throw new NotFoundError("sql: no rows in result set");
      return stored(raw);
    },
    findRecordsByIds: (collection, ids) =>
      (ids || [])
        .map((id) => call("findRecordById", collectionName(collection), String(id)))
        .filter(Boolean)
        .map(stored),
    findRecordsByFilter: (collection, filter, sort, limit, offset, params) =>
      call("findRecordsByFilter", collectionName(collection), filter || "", sort || "", limit || 0, offset || 0, params || {}).map(stored),
    findFirstRecordByFilter: (collection, filter, params) => {
      const raw = call("findRecordsByFilter", collectionName(collection), filter || "", "", 1, 0, params || {})[0];
      if (!raw) throw new NotFoundError("sql: no rows in result set");
      return stored(raw);
    },
    findFirstRecordByData: (collection, field, value) => {
      const raw = call("findRecordsByFilter", collectionName(collection), field + " = {:value}", "", 1, 0, { value })[0];
      if (!raw) throw new NotFoundError("sql: no rows in result set");
      return stored(raw);
    },
    findAuthRecordByEmail: (collection, email) => {
      const raw = call("findRecordsByFilter", collectionName(collection), "email = {:email}", "", 1, 0, { email })[0];
      if (!raw) throw new NotFoundError("sql: no rows in result set");
      return stored(raw);
    },
    countRecords: (collection, filter, params) =>
      call("countRecords", collectionName(collection), filter || "", params || {}),
    save: (record) => {
      if (!(record instanceof Record)) throw new BadRequestError("$app.save() expects a Record.");
      const saved = call("save", record._ref.name || record._ref.id, record._changes());
      record._data = {};
      record.load(saved);
      record._ref = { id: saved.collectionId, name: saved.collectionName };
      record._original = clone(record._data);
    },
    delete: (record) => {
      if (!(record instanceof Record)) throw new BadRequestError("$app.delete() expects a Record.");
      call("delete", record._ref.name || record._ref.id, record.id);
    },
    db: () => ({ newQuery: (sql) => new Query(sql) }),
    settings: () => call("settings"),
    newMailClient: () => ({
      send: (message) => {
        const to = (Array.isArray(message.to) ? message.to : [message.to])
          .map((t) => (typeof t === "string" ? t : t && t.address))
          .filter(Boolean);
        call("sendMail", to, message.subject || "", message.html || message.text || "");
      },
    }),
    logger: () => g.console,
    isBootstrapped: () => call("isReady"),
  };

  function collectionName(c) {
    return typeof c === "string" ? c : c && (c.name || c.id);
  }

  // ------------------------------------------------------------------ helpers

  const $security = {
    randomString: (n) => call("randomString", n || 0, ""),
    randomStringWithAlphabet: (n, alphabet) => call("randomString", n || 0, alphabet || ""),
    pseudorandomString: (n) => call("randomString", n || 0, ""),
    sha256: (text) => call("hash", "sha256", String(text)),
    sha512: (text) => call("hash", "sha512", String(text)),
    md5: (text) => call("hash", "md5", String(text)),
    hs256: (text, secret) => call("hmac", String(text), String(secret)),
    equal: (a, b) => call("equal", String(a), String(b)),
  };

  const $http = {
    send: (options) => {
      const o = Object.assign({ method: "GET", headers: {}, body: "", timeout: 120 }, options || {});
      const body = typeof o.body === "string" ? o.body : JSON.stringify(o.body);
      const res = call("http", o.url || "", String(o.method).toUpperCase(), o.headers || {}, body, o.timeout);
      let json = null;
      try {
        json = res.raw === "" ? null : JSON.parse(res.raw);
      } catch (_) {}
      return { statusCode: res.statusCode, headers: res.headers, raw: res.raw, body: res.raw, json };
    },
  };

  const $os = {
    getenv: (name) => call("env", String(name)),
  };

  function sleep(ms) {
    call("sleep", ms || 0);
  }

  function toString(v) {
    if (typeof v === "string") return v;
    if (Array.isArray(v) && v.every((x) => typeof x === "number")) return String.fromCharCode(...v);
    return v === null || v === undefined ? "" : JSON.stringify(v);
  }

  // CommonJS modules from the hooks directory: require(`${__hooks}/utils.js`)
  const modules = {};
  function require(path) {
    const resolved = call("resolveModule", String(path));
    if (modules[resolved.path]) return modules[resolved.path].exports;
    const module = { exports: {} };
    modules[resolved.path] = module;
    const fn = new Function("module", "exports", "require", "__filename", resolved.source);
    fn(module, module.exports, require, resolved.path);
    return module.exports;
  }

  // ------------------------------------------------------------------ events

  function responseMethods(e) {
    e._response = null;
    e.json = (status, data) => {
      e._response = { status, json: data === undefined ? null : data, hasJson: true, headers: e._headers };
    };
    e.string = (status, text) => {
      e._response = { status, body: String(text), contentType: "text/plain; charset=utf-8", headers: e._headers };
    };
    e.html = (status, html) => {
      e._response = { status, body: String(html), contentType: "text/html; charset=utf-8", headers: e._headers };
    };
    e.noContent = (status) => {
      e._response = { status: status || 204, body: "", headers: e._headers };
    };
    e.redirect = (status, url) => {
      e._headers.Location = url;
      e._response = { status: status || 302, body: "", headers: e._headers };
    };
    e.blob = (status, contentType, data) => {
      e._response = { status, body: toString(data), contentType, headers: e._headers };
    };
    e.error = (status, message, data) => new ApiError(status, message, data);
    e.badRequestError = (message, data) => new BadRequestError(message, data);
    e.notFoundError = (message, data) => new NotFoundError(message, data);
    e.forbiddenError = (message, data) => new ForbiddenError(message, data);
    e.unauthorizedError = (message, data) => new UnauthorizedError(message, data);
    e.tooManyRequestsError = (message, data) => new TooManyRequestsError(message, data);
    e.internalServerError = (message, data) => new InternalServerError(message, data);
  }

  function requestEvent(raw) {
    const e = { app: $app };
    e._headers = {};
    responseMethods(e);
    const info = raw.requestInfo || {};
    e.auth = raw.auth ? stored(raw.auth) : null;
    const headers = {};
    for (const key of Object.keys(info.headers || {})) headers[key.toLowerCase()] = info.headers[key];
    const query = info.query || {};
    e.requestInfo = () => ({
      method: info.method,
      query: Object.assign({}, query),
      headers: Object.assign({}, info.headers || {}),
      body: clone(info.body || {}),
      auth: e.auth,
      context: info.context || "default",
    });
    e.hasSuperuserAuth = () => !!(e.auth && e.auth.isSuperuser());
    e.isAuth = () => !!e.auth;
    e.realIP = () => info.realIP || "";
    e.remoteIP = () => info.realIP || "";
    e.request = {
      method: info.method,
      url: {
        path: info.path || "",
        rawQuery: info.rawQuery || "",
        query: () => ({
          get: (k) => (query[k] === undefined ? "" : query[k]),
          has: (k) => query[k] !== undefined,
        }),
        string: () => (info.path || "") + (info.rawQuery ? "?" + info.rawQuery : ""),
      },
      header: {
        get: (name) => headers[String(name).toLowerCase().replace(/-/g, "_")] || headers[String(name).toLowerCase()] || "",
      },
      pathValue: (name) => (info.pathParams || {})[name] || "",
    };
    e.setHeader = (name, value) => {
      e._headers[name] = String(value);
    };
    // files of a multipart request (each usable as a file field value in $app.save)
    e.findUploadedFiles = (key) => clone((info.files || {})[key] || []);
    return e;
  }

  function recordRequestEvent(raw) {
    const e = requestEvent(raw);
    e.collection = raw.collection;
    e.record = new Record(raw.collection, raw.record);
    if (raw.stored) e.record._original = clone(e.record._data);
    e.next = () => {
      const res = callNext({ record: e.record._fields() });
      e.record = new Record(raw.collection, res.record);
      e.record._original = clone(e.record._data);
    };
    e._state = () => ({ record: e.record._fields(), response: e._response });
    return e;
  }

  function listRequestEvent(raw) {
    const e = requestEvent(raw);
    e.collection = raw.collection;
    e.records = raw.records.map((r) => Record._stored(r));
    e.result = { page: raw.page, perPage: raw.perPage, totalItems: raw.totalItems, totalPages: raw.totalPages };
    Object.defineProperty(e.result, "items", { get: () => e.records, set: (v) => (e.records = v), enumerable: true });
    const state = () => ({
      records: e.records.map((r) => (r instanceof Record ? r.publicExport() : r)),
      page: e.result.page,
      perPage: e.result.perPage,
      totalItems: e.result.totalItems,
      totalPages: e.result.totalPages,
    });
    e.next = () => {
      const res = callNext(state());
      e.records = res.records.map((r) => Record._stored(r));
      Object.assign(e.result, { page: res.page, perPage: res.perPage, totalItems: res.totalItems, totalPages: res.totalPages });
    };
    e._state = () => Object.assign(state(), { response: e._response });
    return e;
  }

  function recordEvent(raw) {
    const e = { app: $app, collection: raw.collection, record: Record._stored(raw.record) };
    e.next = () => {
      callNext({});
    };
    e._state = () => ({});
    return e;
  }

  // model hooks: e.record is written once the handlers return
  function recordModelEvent(raw) {
    const e = { app: $app, collection: raw.collection, record: new Record(raw.collection, raw.record) };
    if (raw.stored) e.record._original = clone(e.record._data);
    e.next = () => {
      const res = callNext({ record: e.record._fields() });
      const original = e.record._original;
      e.record = new Record(raw.collection, res.record);
      e.record._original = original;
    };
    e._state = () => ({ record: e.record._fields() });
    return e;
  }

  function collectionEvent(raw) {
    const e = { app: $app, collection: raw.collection };
    e.next = () => {
      const res = callNext({ collection: e.collection });
      e.collection = res.collection;
    };
    e._state = () => ({ collection: e.collection });
    return e;
  }

  function mailerEvent(raw) {
    const e = { app: $app, message: raw.message, kind: raw.kind };
    if (raw.collection) e.collection = raw.collection;
    if (raw.record) {
      e.record = new Record(raw.collection, raw.record);
      e.record._original = clone(e.record._data);
    }
    e.next = () => {
      const res = callNext({ message: e.message });
      e.message = res.message;
    };
    e._state = () => ({});
    return e;
  }

  function routeEvent(raw) {
    const e = requestEvent(raw);
    e._state = () => ({ response: e._response });
    return e;
  }

  // `routerUse` middleware: e.next() runs the rest of the request
  function middlewareEvent(raw) {
    const e = requestEvent(raw);
    e.next = () => {
      callNext({});
    };
    e._state = () => ({ response: e._response, headers: e._headers });
    return e;
  }

  function simpleEvent() {
    const e = { app: $app, next: () => {} };
    e._state = () => ({});
    return e;
  }

  const builders = {
    recordRequest: recordRequestEvent,
    recordsList: listRequestEvent,
    record: recordEvent,
    recordModel: recordModelEvent,
    collection: collectionEvent,
    mailer: mailerEvent,
    route: routeEvent,
    middleware: middlewareEvent,
    simple: simpleEvent,
  };

  // ------------------------------------------------------------------ registration

  const handlers = [];
  const registrations = [];

  function register(kind, event, fn, extra) {
    if (typeof fn !== "function") throw new TypeError(kind + ": the handler must be a function");
    handlers.push({ fn, event });
    registrations.push(Object.assign({ kind, index: handlers.length - 1 }, extra || {}));
  }

  function collections(list) {
    return list.map((c) => (typeof c === "string" ? c : c && (c.name || c.id))).filter(Boolean);
  }

  const recordRequestHooks = {
    onRecordCreateRequest: "recordCreateRequest",
    onRecordUpdateRequest: "recordUpdateRequest",
    onRecordDeleteRequest: "recordDeleteRequest",
    onRecordViewRequest: "recordViewRequest",
  };
  for (const name of Object.keys(recordRequestHooks)) {
    g[name] = (fn, ...c) => register(recordRequestHooks[name], "recordRequest", fn, { collections: collections(c) });
  }
  g.onRecordsListRequest = (fn, ...c) => register("recordsListRequest", "recordsList", fn, { collections: collections(c) });
  const modelHooks = {
    onRecordCreate: "recordCreate",
    onRecordUpdate: "recordUpdate",
    onRecordDelete: "recordDelete",
  };
  for (const name of Object.keys(modelHooks)) {
    g[name] = (fn, ...c) => register(modelHooks[name], "recordModel", fn, { collections: collections(c) });
  }
  g.onRecordValidate = (fn, ...c) => register("recordValidate", "recordModel", fn, { collections: collections(c) });
  const collectionHooks = {
    onCollectionCreate: "collectionCreate",
    onCollectionUpdate: "collectionUpdate",
    onCollectionDelete: "collectionDelete",
    onCollectionAfterCreateSuccess: "collectionAfterCreateSuccess",
    onCollectionAfterUpdateSuccess: "collectionAfterUpdateSuccess",
    onCollectionAfterDeleteSuccess: "collectionAfterDeleteSuccess",
  };
  for (const name of Object.keys(collectionHooks)) {
    g[name] = (fn, ...c) => register(collectionHooks[name], "collection", fn, { collections: collections(c) });
  }
  const mailerHooks = {
    onMailerSend: "",
    onMailerRecordVerificationSend: "verification",
    onMailerRecordPasswordResetSend: "passwordReset",
    onMailerRecordEmailChangeSend: "emailChange",
    onMailerRecordOTPSend: "otp",
    onMailerRecordAuthAlertSend: "authAlert",
  };
  for (const name of Object.keys(mailerHooks)) {
    g[name] = (fn, ...c) => register("mailerSend", "mailer", fn, { mail: mailerHooks[name], collections: collections(c) });
  }
  const afterHooks = {
    onRecordAfterCreateSuccess: "recordAfterCreateSuccess",
    onRecordAfterUpdateSuccess: "recordAfterUpdateSuccess",
    onRecordAfterDeleteSuccess: "recordAfterDeleteSuccess",
  };
  for (const name of Object.keys(afterHooks)) {
    g[name] = (fn, ...c) => register(afterHooks[name], "record", fn, { collections: collections(c) });
  }
  g.onBootstrap = (fn) => register("ready", "simple", fn);
  g.onServe = (fn) => register("ready", "simple", fn);

  function middlewareChain(handler, middlewares) {
    const list = middlewares.map((m) => (typeof m === "function" ? m : m && m.func)).filter(Boolean);
    return (e) => {
      const run = (i) => {
        if (i < list.length) {
          e.next = () => run(i + 1);
          return list[i](e);
        }
        e.next = () => {};
        return handler(e);
      };
      return run(0);
    };
  }
  g.routerAdd = (method, path, handler, ...middlewares) => {
    if (typeof handler !== "function") throw new TypeError("routerAdd: the handler must be a function");
    register("route", "route", middlewareChain(handler, middlewares), { method: String(method).toUpperCase(), path: String(path) });
  };
  // global middleware for every request (the API, the dashboard and routerAdd routes)
  g.routerUse = (...fns) => {
    for (const m of fns) {
      const fn = typeof m === "function" ? m : m && m.func;
      register("routerUse", "middleware", fn);
    }
  };
  g.cronAdd = (id, expression, fn) => register("cron", "simple", fn, { id: String(id), expression: String(expression) });
  g.cronRemove = (id) => registrations.push({ kind: "cronRemove", id: String(id) });

  const $filesystem = {
    // a file from a string or an array of bytes, for a file field in $app.save()
    fileFromBytes: (data, name) => call("fileFromBytes", typeof data === "string" ? data : Array.from(data || []), String(name)),
    // a file on the server's disk, or downloaded (http/https)
    fileFromPath: (path) => call("fileFromPath", String(path)),
    fileFromURL: (url) => call("fileFromURL", String(url)),
  };

  const $apis = {
    requireAuth: (...c) => (e) => {
      if (!e.auth) throw new UnauthorizedError("The request requires valid record authorization token.");
      if (c.length && !c.includes(e.auth._ref.name) && !c.includes(e.auth._ref.id)) {
        throw new ForbiddenError("The authorized record is not allowed to perform this action.");
      }
      return e.next();
    },
    requireSuperuserAuth: () => (e) => {
      if (!e.hasSuperuserAuth()) throw new UnauthorizedError("The request requires valid superuser authorization token.");
      return e.next();
    },
    requireSuperuserOrOwnerAuth: (param) => (e) => {
      const id = e.request.pathValue(param || "id");
      if (!e.hasSuperuserAuth() && !(e.auth && e.auth.id === id)) {
        throw new ForbiddenError("The authorized record is not allowed to perform this action.");
      }
      return e.next();
    },
    requireGuestOnly: () => (e) => {
      if (e.auth) throw new BadRequestError("The request can be accessed only by guests.");
      return e.next();
    },
  };

  // called by the runtime: runs handler `index` with the event JSON, returns {ok}/{err} JSON
  g.__dispatch = (index, json) => {
    try {
      const handler = handlers[index];
      const e = builders[handler.event](JSON.parse(json));
      handler.fn(e);
      return JSON.stringify({ ok: e._state() });
    } catch (err) {
      return JSON.stringify({ err: errorJson(err) });
    }
  };
  g.__registrations = () => JSON.stringify(registrations);

  Object.assign(g, {
    ApiError,
    BadRequestError,
    UnauthorizedError,
    ForbiddenError,
    NotFoundError,
    TooManyRequestsError,
    InternalServerError,
    Record,
    DynamicModel,
    MailerMessage,
    arrayOf,
    $app,
    $apis,
    $filesystem,
    $security,
    $http,
    $os,
    sleep,
    toString,
    require,
  });
})(globalThis);
