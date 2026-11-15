// Types of what hook files (*.pb.js) and migrations can use. Written by slonbase
// (`slonbase types`, and by itself in --dev): don't edit, it is replaced.
//
// Put this at the top of a hook file for completion and checks in your editor:
//   /// <reference path="../slonbase_data/types.d.ts" />

// ------------------------------------------------------------------ records

// (the class is called RecordModel here because TypeScript has a type named Record of its
// own; in hook files it is `Record`, as in PocketBase)

declare class RecordModel<T = { [field: string]: any }> {
  constructor(collection?: Collection | string, data?: Partial<T>);
  id: string;
  collection(): Collection;
  isNew(): boolean;
  markAsNew(): void;
  get<K extends keyof T & string>(field: K): T[K];
  set<K extends keyof T & string>(field: K, value: T[K]): void;
  load(data: Partial<T>): void;
  getString(field: string): string;
  getInt(field: string): number;
  getFloat(field: string): number;
  getBool(field: string): boolean;
  getStringSlice(field: string): string[];
  getDateTime(field: string): string;
  email(): string;
  setEmail(email: string): void;
  verified(): boolean;
  setVerified(verified: boolean): void;
  emailVisibility(): boolean;
  setEmailVisibility(visible: boolean): void;
  setPassword(password: string): void;
  validatePassword(password: string): boolean;
  newAuthToken(): string;
  newStaticAuthToken(durationSeconds?: number): string;
  newVerificationToken(): string;
  newPasswordResetToken(): string;
  newEmailChangeToken(newEmail: string): string;
  newFileToken(): string;
  isSuperuser(): boolean;
  expandedOne(field: string): RecordModel | null;
  expandedAll(field: string): RecordModel[];
  expand(): { [field: string]: any };
  hide(...fields: string[]): this;
  unhide(...fields: string[]): this;
  publicExport(): { [field: string]: any };
  toJSON(): { [field: string]: any };
  clone(): this;
}

/** `new Record(collection, data)`: a new record of a collection. */
declare const Record: typeof RecordModel;

declare class Collection {
  constructor(data?: { [key: string]: any });
  id: string;
  name: string;
  type: "base" | "auth" | "view";
  system: boolean;
  fields: Field[];
  indexes: string[];
  listRule: string | null;
  viewRule: string | null;
  createRule: string | null;
  updateRule: string | null;
  deleteRule: string | null;
  isBase(): boolean;
  isAuth(): boolean;
  isView(): boolean;
  [key: string]: any;
}

declare class Field {
  constructor(data?: { [key: string]: any });
  id: string;
  name: string;
  type: string;
  [key: string]: any;
}

declare class DynamicModel {
  constructor(shape?: { [key: string]: any });
  [key: string]: any;
}

declare function arrayOf<T>(model: T): T[];
declare function unmarshal<T>(data: any, into: T): T;

// ------------------------------------------------------------------ errors

declare class ApiError extends Error {
  constructor(status?: number, message?: string, data?: any);
  status: number;
  data: any;
}
declare class BadRequestError extends ApiError { constructor(message?: string, data?: any); }
declare class UnauthorizedError extends ApiError { constructor(message?: string, data?: any); }
declare class ForbiddenError extends ApiError { constructor(message?: string, data?: any); }
declare class NotFoundError extends ApiError { constructor(message?: string, data?: any); }
declare class TooManyRequestsError extends ApiError { constructor(message?: string, data?: any); }
declare class InternalServerError extends ApiError { constructor(message?: string, data?: any); }

// ------------------------------------------------------------------ the app

declare namespace slonbase {
  interface Expression {}

  interface Query {
    bind(params: { [name: string]: any }): Query;
    all<T>(result: T[]): T[];
    one<T>(result: T): T;
    column(result: any[]): any[];
    execute(): { rowsAffected: number };
  }

  interface SelectQuery {
    select(...columns: string[]): SelectQuery;
    andSelect(...columns: string[]): SelectQuery;
    distinct(distinct: boolean): SelectQuery;
    selectOption(option: string): SelectQuery;
    from(...tables: string[]): SelectQuery;
    where(expression: Expression): SelectQuery;
    andWhere(expression: Expression): SelectQuery;
    orWhere(expression: Expression): SelectQuery;
    join(type: string, table: string, on?: Expression): SelectQuery;
    innerJoin(table: string, on?: Expression): SelectQuery;
    leftJoin(table: string, on?: Expression): SelectQuery;
    rightJoin(table: string, on?: Expression): SelectQuery;
    orderBy(...columns: string[]): SelectQuery;
    andOrderBy(...columns: string[]): SelectQuery;
    groupBy(...columns: string[]): SelectQuery;
    andGroupBy(...columns: string[]): SelectQuery;
    having(expression: Expression): SelectQuery;
    andHaving(expression: Expression): SelectQuery;
    orHaving(expression: Expression): SelectQuery;
    union(query: SelectQuery): SelectQuery;
    unionAll(query: SelectQuery): SelectQuery;
    limit(n: number): SelectQuery;
    offset(n: number): SelectQuery;
    bind(params: { [name: string]: any }): SelectQuery;
    andBind(params: { [name: string]: any }): SelectQuery;
    build(): Query;
    all<T>(result: T[]): T[];
    one<T>(result: T): T;
    column(result: any[]): any[];
  }

  interface Builder {
    newQuery(sql: string): Query;
    select(...columns: string[]): SelectQuery;
    insert(table: string, columns: { [column: string]: any }): Query;
    upsert(table: string, columns: { [column: string]: any }, ...constraints: string[]): Query;
    update(table: string, columns: { [column: string]: any }, where?: Expression): Query;
    delete(table: string, where?: Expression): Query;
    createTable(table: string, columns: { [column: string]: string }, ...options: string[]): Query;
    renameTable(from: string, to: string): Query;
    dropTable(table: string): Query;
    truncateTable(table: string): Query;
    addColumn(table: string, column: string, type: string): Query;
    dropColumn(table: string, column: string): Query;
    renameColumn(table: string, from: string, to: string): Query;
    alterColumn(table: string, column: string, type: string): Query;
    createIndex(table: string, name: string, ...columns: string[]): Query;
    createUniqueIndex(table: string, name: string, ...columns: string[]): Query;
    dropIndex(table: string, name: string): Query;
  }

  interface MailMessage {
    to: string | { address: string; name?: string } | Array<string | { address: string; name?: string }>;
    subject: string;
    html?: string;
    text?: string;
  }

  interface Settings {
    /** The values of a settings page declared with settingsAdd(), with its defaults. */
    ext(name: string): { [field: string]: any };
    [section: string]: any;
  }

  interface App {
    findCollectionByNameOrId(nameOrId: string): Collection;
    findAllCollections(...types: string[]): Collection[];
    importCollections(collections: any[], deleteMissing?: boolean): void;
    findRecordById<N extends CollectionName>(collection: N, id: string): RecordModel<Collections[N]>;
    findRecordById(collection: string | Collection, id: string): RecordModel;
    findRecordsByIds<N extends CollectionName>(collection: N, ids: string[]): RecordModel<Collections[N]>[];
    findRecordsByIds(collection: string | Collection, ids: string[]): RecordModel[];
    findRecordsByFilter<N extends CollectionName>(collection: N, filter: string, sort?: string, limit?: number, offset?: number, params?: { [name: string]: any }): RecordModel<Collections[N]>[];
    findRecordsByFilter(collection: string | Collection, filter: string, sort?: string, limit?: number, offset?: number, params?: { [name: string]: any }): RecordModel[];
    findFirstRecordByFilter<N extends CollectionName>(collection: N, filter: string, params?: { [name: string]: any }): RecordModel<Collections[N]>;
    findFirstRecordByFilter(collection: string | Collection, filter: string, params?: { [name: string]: any }): RecordModel;
    findFirstRecordByData<N extends CollectionName>(collection: N, field: string, value: any): RecordModel<Collections[N]>;
    findFirstRecordByData(collection: string | Collection, field: string, value: any): RecordModel;
    findAuthRecordByEmail(collection: string | Collection, email: string): RecordModel;
    countRecords(collection: string | Collection, filter?: string, params?: { [name: string]: any }): number;
    save(model: RecordModel | Collection): void;
    delete(model: RecordModel | Collection): void;
    runInTransaction(fn: (txApp: App) => void): void;
    db(): Builder;
    settings(): Settings;
    createBackup(name?: string): void;
    restoreBackup(name: string): void;
    newMailClient(): { send(message: MailMessage): void };
    logger(): { log(...a: any[]): void; info(...a: any[]): void; debug(...a: any[]): void; warn(...a: any[]): void; error(...a: any[]): void };
    isBootstrapped(): boolean;
  }

  // ---------------------------------------------------------------- events

  interface Event {
    app: App;
    /** Runs the rest (the other handlers, then the action). */
    next(): void;
  }

  interface RequestInfo {
    method: string;
    query: { [name: string]: string };
    headers: { [name: string]: string };
    body: { [name: string]: any };
    auth: RecordModel | null;
  }

  interface RequestEvent extends Event {
    auth: RecordModel | null;
    request: {
      method: string;
      url: { path: string; query(): { get(name: string): string } };
      header: { get(name: string): string };
      pathValue(name: string): string;
      [key: string]: any;
    };
    requestInfo(): RequestInfo;
    hasSuperuserAuth(): boolean;
    realIP(): string;
    remoteIP(): string;
    findUploadedFiles(field: string): any[];
    json(status: number, data: any): void;
    string(status: number, text: string): void;
    html(status: number, html: string): void;
    noContent(status: number): void;
    redirect(status: number, url: string): void;
    blob(status: number, contentType: string, data: string | number[]): void;
    error(status: number, message: string, data?: any): ApiError;
    badRequestError(message: string, data?: any): ApiError;
    notFoundError(message: string, data?: any): ApiError;
    forbiddenError(message: string, data?: any): ApiError;
    unauthorizedError(message: string, data?: any): ApiError;
    tooManyRequestsError(message: string, data?: any): ApiError;
    internalServerError(message: string, data?: any): ApiError;
    [key: string]: any;
  }

  interface RecordRequestEvent extends RequestEvent { collection: Collection; record: RecordModel; }
  interface RecordsListRequestEvent extends RequestEvent {
    collection: Collection;
    records: RecordModel[];
    result: { page: number; perPage: number; totalItems: number; totalPages: number; items: RecordModel[] };
  }
  interface RecordEvent extends Event { record: RecordModel; collection?: Collection; [key: string]: any; }
  interface RecordErrorEvent extends RecordEvent { error: ApiError; }
  interface CollectionEvent extends Event { collection: Collection; [key: string]: any; }
  interface CollectionErrorEvent extends CollectionEvent { error: ApiError; }
  interface RecordAuthRequestEvent extends RequestEvent { collection: Collection; record: RecordModel; token: string; meta: any; authMethod: string; }
  interface RecordEnrichEvent extends Event { record: RecordModel; requestInfo: RequestInfo; }
  interface MailerEvent extends Event { message: MailMessage & { [key: string]: any }; record?: RecordModel; collection?: Collection; meta?: any; }
  interface DataEvent extends RequestEvent {}
  /** A record or a collection (the "_collections" tag). */
  interface ModelEvent extends Event { model: RecordModel | Collection; [key: string]: any; }

  type Handler<E> = (e: E) => void;
  type CollectionArg = string | Collection;

  // ---------------------------------------------------------------- extensions

  interface SettingsField {
    name: string;
    label?: string;
    type: "text" | "textarea" | "number" | "bool" | "select" | "secret";
    help?: string;
    options?: string[];
    default?: any;
    required?: boolean;
  }

  /** What an automation's step function is told about its run. */
  interface StepContext {
    automation: { id: string; name: string };
    now: string;
    item: any;
    index: number;
    trigger: { type: "schedule" | "manual" } | { type: "record"; action: "create" | "update" | "delete"; collection: string; record: { [field: string]: any }; changed: string[] };
  }

  interface HttpResponse {
    statusCode: number;
    headers: { [name: string]: string[] };
    cookies: { [name: string]: any };
    raw: string;
    json: any;
  }
}

declare const $app: slonbase.App;

declare const $dbx: {
  exp(sql: string, params?: { [name: string]: any }): slonbase.Expression;
  hashExp(values: { [column: string]: any }): slonbase.Expression;
  not(e: slonbase.Expression): slonbase.Expression;
  and(...e: slonbase.Expression[]): slonbase.Expression;
  or(...e: slonbase.Expression[]): slonbase.Expression;
  in(column: string, ...values: any[]): slonbase.Expression;
  notIn(column: string, ...values: any[]): slonbase.Expression;
  like(column: string, ...values: string[]): slonbase.Expression;
  orLike(column: string, ...values: string[]): slonbase.Expression;
  notLike(column: string, ...values: string[]): slonbase.Expression;
  orNotLike(column: string, ...values: string[]): slonbase.Expression;
  exists(e: slonbase.Expression): slonbase.Expression;
  notExists(e: slonbase.Expression): slonbase.Expression;
  between(column: string, from: any, to: any): slonbase.Expression;
  notBetween(column: string, from: any, to: any): slonbase.Expression;
  [key: string]: any;
};

declare const $http: {
  send(options: { url: string; method?: string; body?: any; data?: any; headers?: { [name: string]: string }; timeout?: number }): slonbase.HttpResponse;
};

declare const $os: {
  getenv(name: string): string;
};

declare const $security: {
  randomString(length: number): string;
  randomStringWithAlphabet(length: number, alphabet: string): string;
  randomStringByRegex(pattern: string): string;
  pseudorandomString(length: number): string;
  equal(a: string, b: string): boolean;
  [key: string]: any;
};

declare const $mails: {
  sendRecordVerification(app: slonbase.App, record: RecordModel): void;
  sendRecordPasswordReset(app: slonbase.App, record: RecordModel): void;
  sendRecordChangeEmail(app: slonbase.App, record: RecordModel, newEmail: string): void;
  sendRecordOTP(app: slonbase.App, record: RecordModel, otpId: string, password: string): void;
  sendRecordAuthAlert(app: slonbase.App, record: RecordModel, info?: any): void;
};

/** The email templates of the dashboard (Automations, Templates). */
declare const $templates: {
  /** Sends a template to one address or a list, rendered with data. Returns how many were sent. */
  send(id: string, to: string | string[], data?: { [name: string]: any }): number;
};

declare const $filesystem: {
  fileFromBytes(data: string | number[], name: string): any;
  fileFromPath(path: string): any;
  fileFromURL(url: string): any;
};

declare const $apis: {
  requireAuth(...collections: string[]): slonbase.Handler<slonbase.RequestEvent>;
  requireSuperuserAuth(): slonbase.Handler<slonbase.RequestEvent>;
  requireSuperuserOrOwnerAuth(param?: string): slonbase.Handler<slonbase.RequestEvent>;
  requireGuestOnly(): slonbase.Handler<slonbase.RequestEvent>;
};

declare const $template: { [key: string]: any };

declare class MailerMessage {
  constructor(message?: Partial<slonbase.MailMessage>);
  [key: string]: any;
}

declare function sleep(milliseconds: number): void;
declare function toString(value: any): string;
declare function toBytes(value: any, maxBytes?: number): number[];
declare function readerToString(reader: any, maxBytes?: number): string;
declare function require(module: string): any;
declare const __hooks: string;

// ------------------------------------------------------------------ routes, jobs, extensions

declare function routerAdd(method: string, path: string, handler: slonbase.Handler<slonbase.RequestEvent>, ...middlewares: Array<slonbase.Handler<slonbase.RequestEvent>>): void;
declare function routerUse(...middlewares: Array<slonbase.Handler<slonbase.RequestEvent>>): void;
/** A job on a schedule: minute hour day month weekday. */
declare function cronAdd(id: string, expression: string, handler: () => void): void;
declare function cronRemove(id: string): void;
/** A function automations can run as a step: what it returns is the step's result. */
declare function automationStep(name: string, handler: (input: { [name: string]: any }, context: slonbase.StepContext) => any): void;
/** A settings page of the dashboard (Settings, Extensions); read with $app.settings().ext(name). */
declare function settingsAdd(name: string, form: { title?: string; fields: slonbase.SettingsField[] }): void;
declare function migrate(up: (app: slonbase.App) => void, down?: (app: slonbase.App) => void): void;

// ------------------------------------------------------------------ hooks

declare function onBootstrap(handler: slonbase.Handler<slonbase.Event>): void;
declare function onServe(handler: slonbase.Handler<slonbase.Event>): void;
declare function onTerminate(handler: slonbase.Handler<slonbase.Event & { isRestart: boolean }>): void;
declare function onRecordCreateRequest(handler: slonbase.Handler<slonbase.RecordRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordUpdateRequest(handler: slonbase.Handler<slonbase.RecordRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordDeleteRequest(handler: slonbase.Handler<slonbase.RecordRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordViewRequest(handler: slonbase.Handler<slonbase.RecordRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordsListRequest(handler: slonbase.Handler<slonbase.RecordsListRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordCreate(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordUpdate(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordDelete(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordValidate(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordCreateExecute(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordUpdateExecute(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordDeleteExecute(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAfterCreateSuccess(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAfterUpdateSuccess(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAfterDeleteSuccess(handler: slonbase.Handler<slonbase.RecordEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onModelValidate(handler: slonbase.Handler<slonbase.ModelEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAfterCreateError(handler: slonbase.Handler<slonbase.RecordErrorEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAfterUpdateError(handler: slonbase.Handler<slonbase.RecordErrorEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAfterDeleteError(handler: slonbase.Handler<slonbase.RecordErrorEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordEnrich(handler: slonbase.Handler<slonbase.RecordEnrichEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAuthRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAuthWithPasswordRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAuthWithOAuth2Request(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAuthRefreshRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordRequestOTPRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordAuthWithOTPRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordRequestVerificationRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordConfirmVerificationRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordRequestPasswordResetRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordConfirmPasswordResetRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordRequestEmailChangeRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRecordConfirmEmailChangeRequest(handler: slonbase.Handler<slonbase.RecordAuthRequestEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionCreate(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionUpdate(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionDelete(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionValidate(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionCreateExecute(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionUpdateExecute(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionDeleteExecute(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionAfterCreateSuccess(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionAfterUpdateSuccess(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionAfterDeleteSuccess(handler: slonbase.Handler<slonbase.CollectionEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionAfterCreateError(handler: slonbase.Handler<slonbase.CollectionErrorEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionAfterUpdateError(handler: slonbase.Handler<slonbase.CollectionErrorEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionAfterDeleteError(handler: slonbase.Handler<slonbase.CollectionErrorEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onMailerSend(handler: slonbase.Handler<slonbase.MailerEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onMailerRecordVerificationSend(handler: slonbase.Handler<slonbase.MailerEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onMailerRecordPasswordResetSend(handler: slonbase.Handler<slonbase.MailerEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onMailerRecordEmailChangeSend(handler: slonbase.Handler<slonbase.MailerEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onMailerRecordOTPSend(handler: slonbase.Handler<slonbase.MailerEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onMailerRecordAuthAlertSend(handler: slonbase.Handler<slonbase.MailerEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRealtimeConnectRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRealtimeSubscribeRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onRealtimeMessageSend(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onFileDownloadRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onFileTokenRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onSettingsListRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onSettingsUpdateRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onSettingsReload(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onBackupCreate(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onBackupRestore(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionsListRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionViewRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionCreateRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionUpdateRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionDeleteRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onCollectionsImportRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onBatchRequest(handler: slonbase.Handler<slonbase.DataEvent>, ...collections: slonbase.CollectionArg[]): void;
declare function onModelCreate(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelCreateExecute(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelAfterCreateSuccess(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelAfterCreateError(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelUpdate(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelUpdateExecute(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelAfterUpdateSuccess(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelAfterUpdateError(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelDelete(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelDeleteExecute(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelAfterDeleteSuccess(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
declare function onModelAfterDeleteError(handler: slonbase.Handler<slonbase.ModelEvent>, ...tags: string[]): void;
