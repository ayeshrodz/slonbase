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
      // fields left out of publicExport() (hide), and hidden ones to send (unhide)
      Object.defineProperty(this, "_hidden", { value: new Set(), writable: true });
      Object.defineProperty(this, "_unhidden", { value: new Set(), writable: true });
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
    newStaticAuthToken(duration) {
      // PocketBase takes a Go duration (nanoseconds); 0 uses the collection's
      const seconds = Math.floor(Number(duration || 0) / 1e9);
      return call("recordToken", this._ref.name || this._ref.id, this.id, "staticAuth", seconds);
    }
    newVerificationToken() {
      return call("recordToken", this._ref.name || this._ref.id, this.id, "verification");
    }
    newPasswordResetToken() {
      return call("recordToken", this._ref.name || this._ref.id, this.id, "passwordReset");
    }
    newEmailChangeToken(newEmail) {
      return call("recordToken", this._ref.name || this._ref.id, this.id, "emailChange", String(newEmail));
    }
    newFileToken() {
      return call("recordToken", this._ref.name || this._ref.id, this.id, "file");
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
    hide(...names) {
      for (const name of names) {
        this._hidden.add(name);
        this._unhidden.delete(name);
      }
      return this;
    }
    unhide(...names) {
      for (const name of names) {
        this._hidden.delete(name);
        this._unhidden.add(name);
      }
      return this;
    }
    // custom keys set on a record are always exported (PocketBase needs this for them)
    withCustomData(_state) {
      return this;
    }
    ignoreEmailVisibility(_state) {
      return this;
    }
    publicExport() {
      const out = { collectionId: this._ref ? this._ref.id : "", collectionName: this._ref ? this._ref.name : "" };
      for (const key of Object.keys(this._data)) {
        if (this._hidden.has(key)) continue;
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

  // ------------------------------------------------------------------ collections

  // a field of a collection: its JSON definition (`new Field({type: "text", name: "title"})`)
  class Field {
    constructor(data) {
      Object.assign(this, clone(data || {}));
    }
  }
  // typed fields: `new TextField({name: "title"})`
  const fieldClasses = {};
  for (const [name, type] of [
    ["TextField", "text"],
    ["EditorField", "editor"],
    ["NumberField", "number"],
    ["BoolField", "bool"],
    ["EmailField", "email"],
    ["URLField", "url"],
    ["DateField", "date"],
    ["AutodateField", "autodate"],
    ["SelectField", "select"],
    ["FileField", "file"],
    ["RelationField", "relation"],
    ["JSONField", "json"],
    ["GeoPointField", "geoPoint"],
    ["PasswordField", "password"],
  ]) {
    fieldClasses[name] = class extends Field {
      constructor(data) {
        super(data);
        this.type = type;
      }
    };
    Object.defineProperty(fieldClasses[name], "name", { value: name });
  }

  // a collection's list of fields, with PocketBase's FieldsList methods
  function fieldsList(items) {
    const list = (items || []).map((f) => (f instanceof Field ? f : new Field(f)));
    const at = (f) => {
      const byId = f.id ? list.findIndex((x) => x.id === f.id) : -1;
      return byId >= 0 ? byId : list.findIndex((x) => !f.id && x.name === f.name);
    };
    const methods = {
      // replaces the fields with the same id (or name, without an id), or appends them
      add: (...fields) => {
        for (const f of fields) {
          const field = f instanceof Field ? f : new Field(f);
          const i = at(field);
          if (i >= 0) list[i] = field;
          else list.push(field);
        }
      },
      // like add(), but moves the fields to `pos`
      addAt: (pos, ...fields) => {
        if (pos < 0) return methods.add(...fields);
        let p = pos;
        for (const f of fields) {
          const field = f instanceof Field ? f : new Field(f);
          const i = at(field);
          if (i >= 0) list.splice(i, 1);
          list.splice(Math.min(p, list.length), 0, field);
          p++;
        }
      },
      getById: (id) => list.find((f) => f.id === id) || null,
      getByName: (name) => list.find((f) => f.name === name) || null,
      removeById: (id) => {
        const i = list.findIndex((f) => f.id === id);
        if (i >= 0) list.splice(i, 1);
      },
      removeByName: (name) => {
        const i = list.findIndex((f) => f.name === name);
        if (i >= 0) list.splice(i, 1);
      },
      fieldNames: () => list.map((f) => f.name),
      asMap: () => Object.fromEntries(list.map((f) => [f.name, f])),
      clone: () => fieldsList(clone(list)),
    };
    for (const [name, fn] of Object.entries(methods)) Object.defineProperty(list, name, { value: fn });
    return list;
  }

  // a collection definition, as $app.findCollectionByNameOrId() returns and $app.save() takes
  class Collection {
    constructor(data) {
      const d = clone(data || {});
      if (!d.type) d.type = "base";
      Object.assign(this, d);
      this.fields = fieldsList(this.fields);
      if (!Array.isArray(this.indexes)) this.indexes = [];
    }
    isBase() {
      return this.type === "base";
    }
    isAuth() {
      return this.type === "auth";
    }
    isView() {
      return this.type === "view";
    }
    _load(data) {
      for (const key of Object.keys(this)) delete this[key];
      Object.assign(this, new Collection(data));
    }
  }

  // copies the JSON data into a model (a collection, a record or any object)
  function unmarshal(data, target) {
    const d = typeof data === "string" ? JSON.parse(data) : clone(data || {});
    if (target instanceof Record) {
      target.load(d);
    } else {
      Object.assign(target, d);
      if (target instanceof Collection) target.fields = fieldsList(target.fields);
    }
    return target;
  }

  function settingsModel(raw) {
    Object.defineProperty(raw, "_settings", { value: true });
    return raw;
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

  // ------------------------------------------------------------------ $dbx

  // PocketBase's dbx query builder, writing PostgreSQL. Expressions build SQL with {:name}
  // parameters, which dbQuery binds; names are quoted as dbx quotes them.
  class Expression {
    constructor(build) {
      Object.defineProperty(this, "_build", { value: build });
    }
    build(params) {
      return this._build(params || {});
    }
  }
  // plain objects work as hash expressions
  function buildExp(e, params) {
    if (e === null || e === undefined) return "";
    if (e instanceof Expression) return e._build(params);
    if (typeof e === "object") return hashExp(e)._build(params);
    throw new BadRequestError("Expected a $dbx expression.");
  }
  function param(params, value) {
    const name = "p" + Object.keys(params).length;
    params[name] = value;
    return "{:" + name + "}";
  }
  function quoteSimple(name) {
    name = String(name);
    return name.includes('"') || name === "*" ? name : '"' + name + '"';
  }
  function quoteTable(name) {
    name = String(name);
    if (name.includes("(") || name.includes("{{")) return name;
    return name.split(".").map(quoteSimple).join(".");
  }
  function quoteColumn(name) {
    name = String(name);
    if (name.includes("(") || name.includes("{{") || name.includes("[[")) return name;
    const dot = name.lastIndexOf(".");
    if (dot < 0) return quoteSimple(name);
    return quoteTable(name.slice(0, dot)) + "." + quoteSimple(name.slice(dot + 1));
  }
  const aliasPattern = /(?:\s+as\s+|\s+)([\w\-.]+)$/i;
  function withAlias(name, quote) {
    const m = String(name).match(aliasPattern);
    if (!m) return quote(name);
    return quote(name.slice(0, name.length - m[0].length)) + (quote === quoteTable ? " " : " AS ") + quoteSimple(m[1]);
  }
  function hashExp(data) {
    return new Expression((params) => {
      const parts = [];
      for (const name of Object.keys(data || {}).sort()) {
        const value = data[name];
        if (value === null || value === undefined) {
          parts.push(quoteColumn(name) + " IS NULL");
        } else if (value instanceof Expression) {
          const sql = value._build(params);
          if (sql) parts.push("(" + sql + ")");
        } else if (Array.isArray(value)) {
          const sql = inExp(name, value, false)._build(params);
          if (sql) parts.push(sql);
        } else {
          parts.push(quoteColumn(name) + "=" + param(params, value));
        }
      }
      return parts.join(" AND ");
    });
  }
  function andOr(exps, op) {
    return new Expression((params) => {
      const parts = exps.map((e) => buildExp(e, params)).filter(Boolean);
      return parts.length === 1 ? parts[0] : parts.length ? "(" + parts.join(") " + op + " (") + ")" : "";
    });
  }
  function inExp(col, values, not) {
    return new Expression((params) => {
      if (!values.length) return not ? "" : "0=1";
      const sqls = values.map((v) =>
        v === null || v === undefined ? "NULL" : v instanceof Expression ? v._build(params) : param(params, v),
      );
      if (sqls.length === 1) return quoteColumn(col) + (not ? "<>" : "=") + sqls[0];
      return quoteColumn(col) + (not ? " NOT IN (" : " IN (") + sqls.join(", ") + ")";
    });
  }
  const likeEscape = ["\\", "\\\\", "%", "\\%", "_", "\\_"];
  class LikeExpression extends Expression {
    constructor(col, values, like, or) {
      super((params) => {
        const parts = values.map((value) => {
          value = String(value);
          for (let i = 0; i + 1 < this._escape.length; i += 2) value = value.split(this._escape[i]).join(this._escape[i + 1]);
          value = (this._left ? "%" : "") + value + (this._right ? "%" : "");
          return quoteColumn(col) + " " + like + " " + param(params, value);
        });
        return parts.join(or ? " OR " : " AND ");
      });
      Object.defineProperty(this, "_escape", { value: likeEscape, writable: true });
      Object.defineProperty(this, "_left", { value: true, writable: true });
      Object.defineProperty(this, "_right", { value: true, writable: true });
    }
    escape(...chars) {
      if (chars.length % 2) throw new BadRequestError("LikeExp.Escape must be a slice of even number of strings");
      this._escape = chars;
      return this;
    }
    match(left, right) {
      this._left = !!left;
      this._right = !!right;
      return this;
    }
  }
  function existsExp(e, not) {
    return new Expression((params) => {
      const sql = buildExp(e, params);
      if (!sql) return not ? "" : "0=1";
      return (not ? "NOT EXISTS (" : "EXISTS (") + sql + ")";
    });
  }
  function betweenExp(col, from, to, not) {
    return new Expression((params) => {
      const a = param(params, from);
      const b = param(params, to);
      return quoteColumn(col) + (not ? " NOT BETWEEN " : " BETWEEN ") + a + " AND " + b;
    });
  }
  const $dbx = {
    exp: (sql, params) =>
      new Expression((all) => {
        Object.assign(all, params || {});
        return String(sql);
      }),
    hashExp,
    not: (e) =>
      new Expression((params) => {
        const sql = buildExp(e, params);
        return sql ? "NOT (" + sql + ")" : "";
      }),
    and: (...exps) => andOr(exps, "AND"),
    or: (...exps) => andOr(exps, "OR"),
    in: (col, ...values) => inExp(col, values, false),
    notIn: (col, ...values) => inExp(col, values, true),
    like: (col, ...values) => new LikeExpression(col, values, "LIKE", false),
    orLike: (col, ...values) => new LikeExpression(col, values, "LIKE", true),
    notLike: (col, ...values) => new LikeExpression(col, values, "NOT LIKE", false),
    orNotLike: (col, ...values) => new LikeExpression(col, values, "NOT LIKE", true),
    exists: (e) => existsExp(e, false),
    notExists: (e) => existsExp(e, true),
    between: (col, from, to) => betweenExp(col, from, to, false),
    notBetween: (col, from, to) => betweenExp(col, from, to, true),
    enclose: (e) =>
      new Expression((params) => {
        const sql = buildExp(e, params);
        return sql ? "(" + sql + ")" : "";
      }),
  };

  // fills a result of all()/one(): records for $app.recordQuery(), else plain objects (or the
  // DynamicModel of arrayOf())
  function intoRecord(target, raw) {
    const r = stored(raw);
    target._c = null;
    target._ref = r._ref;
    target._data = r._data;
    target._original = r._original;
    return target;
  }

  class Query {
    constructor(sql, params, records) {
      this.sql = sql;
      this.params = Object.assign({}, params || {});
      Object.defineProperty(this, "_records", { value: records || "" });
    }
    bind(params) {
      Object.assign(this.params, params || {});
      return this;
    }
    _rows() {
      return call("dbQuery", this.sql, this.params, "all", this._records);
    }
    all(result) {
      const rows = this._rows();
      if (this._records) {
        const records = rows.map(stored);
        if (!Array.isArray(result)) return records;
        result.length = 0;
        result.push(...records);
        return result;
      }
      if (Array.isArray(result)) {
        result.length = 0;
        const Model = result._model ? result._model.constructor : DynamicModel;
        for (const row of rows) result.push(Object.assign(Model === Object ? {} : new Model(), row));
        return result;
      }
      return rows;
    }
    one(result) {
      const rows = this._rows();
      if (!rows.length) throw new NotFoundError("sql: no rows in result set");
      if (this._records) return result instanceof Record ? intoRecord(result, rows[0]) : stored(rows[0]);
      if (result && typeof result === "object") return Object.assign(result, rows[0]);
      return rows[0];
    }
    // the first column of each row
    column(result) {
      const values = this._rows().map((row) => Object.values(row)[0]);
      if (!Array.isArray(result)) return values;
      result.length = 0;
      result.push(...values);
      return result;
    }
    execute() {
      return { rowsAffected: call("dbQuery", this.sql, this.params, "execute") };
    }
  }

  class SelectQuery {
    constructor(cols, records) {
      Object.defineProperty(this, "_q", {
        value: {
          selects: cols, distinct: false, option: "", from: [], join: [], where: null,
          groupBy: [], having: null, orderBy: [], union: [], limit: -1, offset: 0,
          params: {}, pre: "", post: "",
        },
      });
      Object.defineProperty(this, "_records", { value: records || "" });
    }
    _set(key, value) {
      this._q[key] = value;
      return this;
    }
    select(...cols) { return this._set("selects", cols); }
    andSelect(...cols) { return this._set("selects", this._q.selects.concat(cols)); }
    distinct(v) { return this._set("distinct", !!v); }
    selectOption(option) { return this._set("option", String(option || "")); }
    from(...tables) { return this._set("from", tables); }
    where(e) { return this._set("where", e); }
    andWhere(e) { return this._set("where", $dbx.and(this._q.where, e)); }
    orWhere(e) { return this._set("where", $dbx.or(this._q.where, e)); }
    join(type, table, on) { return this._set("join", this._q.join.concat([{ type, table, on }])); }
    innerJoin(table, on) { return this.join("INNER JOIN", table, on); }
    leftJoin(table, on) { return this.join("LEFT JOIN", table, on); }
    rightJoin(table, on) { return this.join("RIGHT JOIN", table, on); }
    orderBy(...cols) { return this._set("orderBy", cols); }
    andOrderBy(...cols) { return this._set("orderBy", this._q.orderBy.concat(cols)); }
    groupBy(...cols) { return this._set("groupBy", cols); }
    andGroupBy(...cols) { return this._set("groupBy", this._q.groupBy.concat(cols)); }
    having(e) { return this._set("having", e); }
    andHaving(e) { return this._set("having", $dbx.and(this._q.having, e)); }
    orHaving(e) { return this._set("having", $dbx.or(this._q.having, e)); }
    union(q) { return this._set("union", this._q.union.concat([{ all: false, q }])); }
    unionAll(q) { return this._set("union", this._q.union.concat([{ all: true, q }])); }
    limit(n) { return this._set("limit", Number(n)); }
    offset(n) { return this._set("offset", Number(n)); }
    bind(params) { return this._set("params", Object.assign({}, params || {})); }
    andBind(params) { return this._set("params", Object.assign(this._q.params, params || {})); }
    preFragment(sql) { return this._set("pre", String(sql || "")); }
    postFragment(sql) { return this._set("post", String(sql || "")); }
    build() {
      const q = this._q;
      const params = Object.assign({}, q.params);
      let select = "SELECT " + (q.distinct ? "DISTINCT " : "") + (q.option ? q.option + " " : "");
      select += q.selects.length ? q.selects.map((c) => withAlias(c, quoteColumn)).join(", ") : "*";
      const clauses = [q.pre, select];
      if (q.from.length) clauses.push("FROM " + q.from.map((t) => withAlias(t, quoteTable)).join(", "));
      for (const j of q.join) {
        const on = buildExp(j.on, params);
        clauses.push(j.type + " " + withAlias(j.table, quoteTable) + (on ? " ON " + on : ""));
      }
      const where = buildExp(q.where, params);
      if (where) clauses.push("WHERE " + where);
      if (q.groupBy.length) clauses.push("GROUP BY " + q.groupBy.map(quoteColumn).join(", "));
      const having = buildExp(q.having, params);
      if (having) clauses.push("HAVING " + having);
      if (q.orderBy.length) {
        clauses.push(
          "ORDER BY " +
            q.orderBy
              .map((c) => {
                const m = String(c).match(/\s+(ASC|DESC)$/i);
                return m ? quoteColumn(c.slice(0, c.length - m[0].length)) + " " + m[1] : quoteColumn(c);
              })
              .join(", "),
        );
      }
      // dbx's: an offset without a limit takes the largest one
      const limit = q.limit >= 0 ? String(q.limit) : q.offset > 0 ? "9223372036854775807" : "";
      if (limit) clauses.push("LIMIT " + limit);
      if (q.offset > 0) clauses.push("OFFSET " + q.offset);
      if (q.post) clauses.push(q.post);
      let sql = clauses.filter(Boolean).join(" ");
      if (q.union.length) {
        const unions = q.union.map((u) => {
          const built = u.q instanceof SelectQuery ? u.q.build() : u.q;
          Object.assign(params, built.params);
          return (u.all ? "UNION ALL (" : "UNION (") + built.sql + ")";
        });
        sql = "(" + sql + ") " + unions.join(" ");
      }
      return new Query(sql, params, this._records);
    }
    all(result) { return this.build().all(result); }
    one(result) { return this.build().one(result); }
    column(result) { return this.build().column(result); }
  }

  function columnValues(cols, params) {
    return Object.keys(cols || {})
      .sort()
      .map((name) => {
        const value = cols[name];
        return [quoteColumn(name), value instanceof Expression ? value._build(params) : param(params, value)];
      });
  }
  const quoteColumns = (cols) => cols.map(quoteColumn).join(", ");
  const dbBuilder = {
    newQuery: (sql) => new Query(sql),
    select: (...cols) => new SelectQuery(cols),
    insert: (table, cols) => {
      const params = {};
      const pairs = columnValues(cols, params);
      const sql = pairs.length
        ? "INSERT INTO " + quoteTable(table) + " (" + pairs.map((p) => p[0]).join(", ") + ") VALUES (" + pairs.map((p) => p[1]).join(", ") + ")"
        : "INSERT INTO " + quoteTable(table) + " DEFAULT VALUES";
      return new Query(sql, params);
    },
    upsert: (table, cols, ...constraints) => {
      const q = dbBuilder.insert(table, cols);
      const lines = columnValues(cols, q.params).map((p) => p[0] + "=" + p[1]);
      q.sql += " ON CONFLICT " + (constraints.length ? "(" + quoteColumns(constraints) + ") " : "") + "DO UPDATE SET " + lines.join(", ");
      return q;
    },
    update: (table, cols, where) => {
      const params = {};
      let sql = "UPDATE " + quoteTable(table) + " SET " + columnValues(cols, params).map((p) => p[0] + "=" + p[1]).join(", ");
      const w = buildExp(where, params);
      if (w) sql += " WHERE " + w;
      return new Query(sql, params);
    },
    delete: (table, where) => {
      const params = {};
      let sql = "DELETE FROM " + quoteTable(table);
      const w = buildExp(where, params);
      if (w) sql += " WHERE " + w;
      return new Query(sql, params);
    },
    createTable: (table, cols, ...options) => {
      const columns = Object.keys(cols || {})
        .sort()
        .map((name) => quoteColumn(name) + " " + cols[name]);
      return new Query(["CREATE TABLE " + quoteTable(table) + " (" + columns.join(", ") + ")"].concat(options).join(" "));
    },
    renameTable: (from, to) => new Query("ALTER TABLE " + quoteTable(from) + " RENAME TO " + quoteTable(to)),
    dropTable: (table) => new Query("DROP TABLE " + quoteTable(table)),
    truncateTable: (table) => new Query("TRUNCATE TABLE " + quoteTable(table)),
    addColumn: (table, col, type) => new Query("ALTER TABLE " + quoteTable(table) + " ADD " + quoteColumn(col) + " " + type),
    dropColumn: (table, col) => new Query("ALTER TABLE " + quoteTable(table) + " DROP COLUMN " + quoteColumn(col)),
    renameColumn: (table, from, to) =>
      new Query("ALTER TABLE " + quoteTable(table) + " RENAME COLUMN " + quoteColumn(from) + " TO " + quoteColumn(to)),
    alterColumn: (table, col, type) =>
      new Query("ALTER TABLE " + quoteTable(table) + " ALTER COLUMN " + quoteColumn(col) + " TYPE " + type),
    addPrimaryKey: (table, name, ...cols) =>
      new Query("ALTER TABLE " + quoteTable(table) + " ADD CONSTRAINT " + quoteColumn(name) + " PRIMARY KEY (" + quoteColumns(cols) + ")"),
    dropPrimaryKey: (table, name) => new Query("ALTER TABLE " + quoteTable(table) + " DROP CONSTRAINT " + quoteColumn(name)),
    addForeignKey: (table, name, cols, refCols, refTable, ...options) =>
      new Query(
        ["ALTER TABLE " + quoteTable(table) + " ADD CONSTRAINT " + quoteColumn(name) + " FOREIGN KEY (" + quoteColumns(cols) +
          ") REFERENCES " + quoteTable(refTable) + " (" + quoteColumns(refCols) + ")"].concat(options).join(" "),
      ),
    dropForeignKey: (table, name) => new Query("ALTER TABLE " + quoteTable(table) + " DROP CONSTRAINT " + quoteColumn(name)),
    createIndex: (table, name, ...cols) =>
      new Query("CREATE INDEX " + quoteColumn(name) + " ON " + quoteTable(table) + " (" + quoteColumns(cols) + ")"),
    createUniqueIndex: (table, name, ...cols) =>
      new Query("CREATE UNIQUE INDEX " + quoteColumn(name) + " ON " + quoteTable(table) + " (" + quoteColumns(cols) + ")"),
    dropIndex: (table, name) => new Query("DROP INDEX " + quoteColumn(name)),
  };

  // $app.recordQuery(): a select of the collection's rows, read back as records
  function recordQuery(collection) {
    const name = typeof collection === "string" ? $app.findCollectionByNameOrId(collection).name : collection.name;
    return new SelectQuery([quoteSimple(name) + ".*"], name).from(name);
  }

  class MailerMessage {
    constructor(data) {
      Object.assign(this, { to: [], subject: "", html: "" }, data || {});
    }
  }

  const $app = {
    findCollectionByNameOrId: (nameOrId) => new Collection(call("collection", collectionName(nameOrId))),
    findAllCollections: (...types) =>
      call("collections")
        .map((c) => new Collection(c))
        .filter((c) => !types.length || types.includes(c.type)),
    // creates or updates collections from their definitions (and with deleteMissing, deletes
    // the other non-system collections)
    importCollections: (collections, deleteMissing) => {
      call("importCollections", JSON.parse(JSON.stringify(collections || [])), !!deleteMissing);
    },
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
    save: (model) => {
      if (model instanceof Collection) {
        model._load(call("saveCollection", JSON.parse(JSON.stringify(model))));
        return;
      }
      if (model && model._settings) {
        const saved = call("saveSettings", JSON.parse(JSON.stringify(model)));
        Object.assign(model, saved);
        return;
      }
      if (!(model instanceof Record)) throw new BadRequestError("$app.save() expects a Record, a Collection or the settings.");
      const saved = call("save", model._ref.name || model._ref.id, model._changes());
      model._data = {};
      model.load(saved);
      model._ref = { id: saved.collectionId, name: saved.collectionName };
      model._original = clone(model._data);
    },
    delete: (model) => {
      if (model instanceof Collection) {
        call("deleteCollection", model.id || model.name);
        return;
      }
      if (!(model instanceof Record)) throw new BadRequestError("$app.delete() expects a Record or a Collection.");
      call("delete", model._ref.name || model._ref.id, model.id);
    },
    // in migrations everything already runs in the migration's transaction
    runInTransaction: (fn) => fn($app),
    db: () => dbBuilder,
    recordQuery,
    settings: () => settingsModel(call("settings")),
    // the context argument of PocketBase's signature is accepted and ignored
    createBackup: (ctx, name) => {
      call("createBackup", typeof ctx === "string" && name === undefined ? ctx : String(name || ""));
    },
    restoreBackup: (ctx, name) => {
      call("restoreBackup", typeof ctx === "string" && name === undefined ? ctx : String(name || ""));
    },
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

  // the auth emails, as the auth endpoints send them (the first argument, the app, is
  // ignored: PocketBase's functions take it)
  const recordRef = (record) => {
    if (!(record instanceof Record)) throw new TypeError("$mails: expected a record");
    return [record._ref.name || record._ref.id, record.id];
  };
  const $mails = {
    sendRecordVerification: (_app, record) => call("sendRecordMail", "verification", ...recordRef(record)),
    sendRecordPasswordReset: (_app, record) => call("sendRecordMail", "passwordReset", ...recordRef(record)),
    sendRecordChangeEmail: (_app, record, newEmail) => call("sendRecordMail", "emailChange", ...recordRef(record), String(newEmail)),
    sendRecordOTP: (_app, record, otpId, password) => call("sendRecordMail", "otp", ...recordRef(record), String(otpId), String(password)),
    sendRecordAuthAlert: (_app, record, info) => call("sendRecordMail", "authAlert", ...recordRef(record), String(info || "")),
  };

  // Go's html/template (template.js): $template.loadFiles(`${__hooks}/views/page.html`).render(data)
  const $template = g.__newTemplateRegistry((path) => call("readTextFile", String(path)));

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
    const e = { app: $app, collection: new Collection(raw.collection) };
    e.next = () => {
      const res = callNext({ collection: e.collection });
      e.collection = new Collection(res.collection);
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

  // a record about to be sent to a client: changes to e.record (set, hide, unhide) are sent
  function enrichEvent(raw) {
    const e = { app: $app, collection: raw.collection, record: Record._stored(raw.record) };
    const info = raw.requestInfo || {};
    const auth = raw.auth ? stored(raw.auth) : null;
    e.requestInfo = () => ({
      method: info.method,
      query: Object.assign({}, info.query || {}),
      headers: Object.assign({}, info.headers || {}),
      body: clone(info.body || {}),
      auth,
      context: "default",
    });
    const state = () => ({ record: e.record.publicExport(), unhide: Array.from(e.record._unhidden) });
    e.next = () => {
      const res = callNext(state());
      e.record = Record._stored(res.record);
    };
    e._state = state;
    return e;
  }

  // auth requests: e.record (null when no record matched) and the hook's own data
  // (e.identity, e.password, e.token, e.meta, e.oauth2User, e.createData, e.newEmail, ...)
  function authRequestEvent(raw) {
    const e = requestEvent(raw);
    e.collection = raw.collection;
    const setRecord = (r) => {
      e.record = r ? new Record(raw.collection, r) : null;
      if (e.record) e.record._original = clone(e.record._data);
    };
    setRecord(raw.record);
    const keys = Object.keys(raw.data || {});
    for (const key of keys) e[key] = clone(raw.data[key]);
    if (e.otp) {
      const otp = e.otp;
      otp.get = (k) => (otp[k] === undefined ? null : otp[k]);
    }
    const state = () => {
      const data = {};
      for (const key of keys) data[key] = e[key];
      const record = e.record instanceof Record ? e.record._fields() : e.record || null;
      return { record, data };
    };
    e.next = () => {
      const res = callNext(state());
      setRecord(res.record);
      for (const key of keys) if (key in res.data) e[key] = res.data[key];
    };
    e._state = () => Object.assign(state(), { response: e._response });
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

  // a realtime client (e.client): its id, subscriptions and auth record (e.client.get("auth"))
  function realtimeClient(raw) {
    const values = {};
    return {
      id: () => raw.id,
      subscriptions: () => (raw.subscriptions || []).slice(),
      hasSubscription: (topic) => (raw.subscriptions || []).includes(topic),
      get: (key) => (key === "auth" ? stored(raw.auth) : key in values ? values[key] : null),
      set: (key, value) => {
        values[key] = value;
      },
      unset: (key) => {
        delete values[key];
      },
      _raw: raw,
    };
  }

  // the realtime, file, settings, backup and terminate hooks: the event's values are
  // properties (e.client, e.message, e.subscriptions, e.settings, e.servedPath, ...);
  // changes to them are used by e.next() and the action
  function dataEvent(raw) {
    const e = raw.requestInfo ? requestEvent(raw) : { app: $app, _response: null };
    if (raw.collection) e.collection = raw.collection;
    const keys = Object.keys(raw.data || {});
    const valueIn = (key, v) => {
      if (key === "client" && v) return realtimeClient(v);
      if (key === "record" && v) return Record._stored(v);
      if (key === "collection" && v && !raw.collection) return new Collection(v);
      if (key === "collections" && Array.isArray(v)) return v.map((c) => new Collection(c));
      return clone(v);
    };
    const valueOut = (key, v) => {
      if (key === "client") return v && v._raw ? v._raw : v;
      if (v instanceof Record) return v.publicExport();
      if (v instanceof Collection) return JSON.parse(JSON.stringify(v));
      if (Array.isArray(v)) return v.map((x) => (x instanceof Collection ? JSON.parse(JSON.stringify(x)) : x));
      return v === undefined ? null : v;
    };
    const load = (data) => {
      for (const key of keys) if (key in data) e[key] = valueIn(key, data[key]);
    };
    const state = () => {
      const data = {};
      for (const key of keys) data[key] = valueOut(key, e[key]);
      return { data };
    };
    // as in PocketBase, a list's result.items are the same objects as e.collections
    const link = () => {
      if (e.result && Array.isArray(e.collections) && Array.isArray(e.result.items)) e.result.items = e.collections;
    };
    load(raw.data || {});
    link();
    e.next = () => {
      load(callNext(state()).data || {});
      link();
    };
    e._state = () => Object.assign(state(), { response: e._response || null });
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
    auth: authRequestEvent,
    enrich: enrichEvent,
    route: routeEvent,
    middleware: middlewareEvent,
    simple: simpleEvent,
    data: dataEvent,
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
  const authHooks = {
    onRecordAuthRequest: "recordAuthRequest",
    onRecordAuthWithPasswordRequest: "recordAuthWithPasswordRequest",
    onRecordAuthWithOAuth2Request: "recordAuthWithOAuth2Request",
    onRecordAuthRefreshRequest: "recordAuthRefreshRequest",
    onRecordRequestOTPRequest: "recordRequestOTPRequest",
    onRecordAuthWithOTPRequest: "recordAuthWithOTPRequest",
    onRecordRequestVerificationRequest: "recordRequestVerificationRequest",
    onRecordConfirmVerificationRequest: "recordConfirmVerificationRequest",
    onRecordRequestPasswordResetRequest: "recordRequestPasswordResetRequest",
    onRecordConfirmPasswordResetRequest: "recordConfirmPasswordResetRequest",
    onRecordRequestEmailChangeRequest: "recordRequestEmailChangeRequest",
    onRecordConfirmEmailChangeRequest: "recordConfirmEmailChangeRequest",
  };
  for (const name of Object.keys(authHooks)) {
    g[name] = (fn, ...c) => register(authHooks[name], "auth", fn, { collections: collections(c) });
  }
  g.onRecordEnrich = (fn, ...c) => register("recordEnrich", "enrich", fn, { collections: collections(c) });
  // hooks without collection tags
  for (const [name, kind] of Object.entries({
    onRealtimeConnectRequest: "realtimeConnectRequest",
    onRealtimeSubscribeRequest: "realtimeSubscribeRequest",
    onRealtimeMessageSend: "realtimeMessageSend",
    onSettingsListRequest: "settingsListRequest",
    onSettingsUpdateRequest: "settingsUpdateRequest",
    onSettingsReload: "settingsReload",
    onBackupCreate: "backupCreate",
    onBackupRestore: "backupRestore",
    onTerminate: "terminate",
    onCollectionsListRequest: "collectionsListRequest",
    onCollectionViewRequest: "collectionViewRequest",
    onCollectionCreateRequest: "collectionCreateRequest",
    onCollectionUpdateRequest: "collectionUpdateRequest",
    onCollectionDeleteRequest: "collectionDeleteRequest",
    onCollectionsImportRequest: "collectionsImportRequest",
    onBatchRequest: "batchRequest",
  })) {
    g[name] = (fn) => register(kind, "data", fn);
  }
  // file hooks, optionally limited to collections
  g.onFileDownloadRequest = (fn, ...c) => register("fileDownloadRequest", "data", fn, { collections: collections(c) });
  g.onFileTokenRequest = (fn, ...c) => register("fileTokenRequest", "data", fn, { collections: collections(c) });
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

  // ------------------------------------------------------------------ migrations

  // a migration file calls migrate(up, down); each gets the app (in the migration's transaction)
  let migration = null;
  g.migrate = (up, down) => {
    if (typeof up !== "function") throw new TypeError("migrate: up must be a function");
    migration = { up, down: typeof down === "function" ? down : null };
  };
  // called by the runtime after evaluating a migration file: runs its up or down function
  g.__runMigration = (direction) => {
    const m = migration;
    migration = null;
    try {
      if (!m) throw new Error("the file doesn't call migrate()");
      const fn = direction === "down" ? m.down : m.up;
      if (fn) fn($app);
      return JSON.stringify({ ok: null });
    } catch (err) {
      if (err instanceof ApiError) {
        const data = err.data && Object.keys(err.data).length ? " " + JSON.stringify(err.data) : "";
        return JSON.stringify({ err: err.message + data });
      }
      return JSON.stringify({ err: err && err.stack ? String(err) + "\n" + err.stack : String(err) });
    }
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
    Collection,
    Field,
    ...fieldClasses,
    unmarshal,
    DynamicModel,
    MailerMessage,
    arrayOf,
    $app,
    $dbx,
    $apis,
    $filesystem,
    $security,
    $http,
    $os,
    $mails,
    $template,
    sleep,
    toString,
    require,
  });
})(globalThis);
