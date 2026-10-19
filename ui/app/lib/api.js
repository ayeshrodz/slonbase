// The API client: JSON requests with the superuser token, errors as ApiError.

// "" for the main installation, "/p/{name}" for a project (the dashboard is at BASE + "/_/")
export const BASE = location.pathname.replace(/\/_(\/.*)?$/, "");
// the project this dashboard manages, or null for the main installation
export const PROJECT = BASE.startsWith("/p/") ? decodeURIComponent(BASE.slice(3)) : null;
// each project has its own superusers, so its own session
const MAIN_TOKEN_KEY = "slonbase.auth";
const TOKEN_KEY = MAIN_TOKEN_KEY + BASE;

export class ApiError extends Error {
  constructor(status, message, data) {
    super(message || "Request failed.");
    this.status = status;
    this.data = data || {};
  }
  // field errors as { field: message }
  fieldErrors() {
    const out = {};
    const walk = (obj, prefix) => {
      for (const [key, value] of Object.entries(obj || {})) {
        if (value && typeof value === "object" && "message" in value && "code" in value) {
          out[prefix + key] = value.message;
        } else if (value && typeof value === "object") {
          walk(value, prefix + key + ".");
        }
      }
    };
    walk(this.data, "");
    return out;
  }
}

function readAuth() {
  try {
    return JSON.parse(localStorage.getItem(TOKEN_KEY) || "null");
  } catch (_) {
    return null;
  }
}

let auth = readAuth();
const listeners = new Set();

export const session = {
  get token() {
    return auth ? auth.token : "";
  },
  get record() {
    return auth ? auth.record : null;
  },
  set(token, record) {
    auth = token ? { token, record } : null;
    try {
      if (auth) localStorage.setItem(TOKEN_KEY, JSON.stringify(auth));
      else localStorage.removeItem(TOKEN_KEY);
    } catch (_) {}
    listeners.forEach((fn) => fn(auth));
  },
  clear() {
    this.set(null);
  },
  subscribe(fn) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
  // seconds until the token expires (negative when expired)
  expiresIn() {
    if (!auth) return -1;
    try {
      const payload = JSON.parse(atob(auth.token.split(".")[1].replace(/-/g, "+").replace(/_/g, "/")));
      return payload.exp - Date.now() / 1000;
    } catch (_) {
      return -1;
    }
  },
};

export async function request(method, path, body, options = {}) {
  const headers = Object.assign({}, options.headers || {});
  if (session.token && !options.noAuth) headers.Authorization = session.token;
  let payload = body;
  if (body !== undefined && !(body instanceof FormData) && typeof body !== "string") {
    headers["Content-Type"] = "application/json";
    payload = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch(path.startsWith("/") ? BASE + path : path, { method, headers, body: payload, signal: options.signal });
  } catch (err) {
    if (err.name === "AbortError") throw err;
    throw new ApiError(0, "Can't reach the server.");
  }
  if (res.status === 204) return null;
  const type = res.headers.get("content-type") || "";
  const data = type.includes("application/json") ? await res.json().catch(() => null) : await res.text();
  if (!res.ok) {
    if (res.status === 401 && session.token && !options.noAuth) {
      session.clear();
    }
    const message = data && typeof data === "object" ? data.message : String(data || res.statusText);
    throw new ApiError(res.status, message, data && data.data);
  }
  return data;
}

export const api = {
  get: (path, options) => request("GET", path, undefined, options),
  post: (path, body, options) => request("POST", path, body, options),
  patch: (path, body, options) => request("PATCH", path, body, options),
  put: (path, body, options) => request("PUT", path, body, options),
  delete: (path, options) => request("DELETE", path, undefined, options),
};

export function qs(params) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") search.set(key, value);
  }
  const s = search.toString();
  return s ? "?" + s : "";
}

export const enc = encodeURIComponent;

// keeps the token fresh while the dashboard is open
// The projects, listed with the main installation's session: this dashboard's own in the
// main dashboard, or one stored in this browser when managing a project. Null without one.
export async function listProjects() {
  let token = session.token;
  if (PROJECT) {
    try {
      token = (JSON.parse(localStorage.getItem(MAIN_TOKEN_KEY) || "null") || {}).token;
    } catch (_) {
      token = null;
    }
  }
  if (!token) return null;
  try {
    const res = await fetch("/api/projects", { headers: { Authorization: token } });
    return res.ok ? (await res.json()).items : null;
  } catch (_) {
    return null;
  }
}

// the dashboard of a project (or of the main installation, for null)
export function dashboardUrl(project) {
  return project ? `/p/${encodeURIComponent(project)}/_/` : "/_/";
}

export async function refreshAuth() {
  if (!session.token) return;
  try {
    const res = await api.post("/api/collections/_superusers/auth-refresh");
    session.set(res.token, res.record);
  } catch (_) {}
}

// a short-lived token for protected files
let fileToken = { value: "", until: 0 };
export async function getFileToken() {
  if (fileToken.until > Date.now()) return fileToken.value;
  const res = await api.post("/api/files/token");
  fileToken = { value: res.token, until: Date.now() + 100_000 };
  return res.token;
}

export function fileUrl(record, name, opts = {}) {
  if (!name) return "";
  const params = {};
  if (opts.thumb) params.thumb = opts.thumb;
  if (opts.token) params.token = opts.token;
  if (opts.download) params.download = 1;
  return `${BASE}/api/files/${enc(record.collectionId)}/${enc(record.id)}/${enc(name)}${qs(params)}`;
}

// realtime: one SSE connection, subscriptions by topic
export function realtime(topics, onEvent) {
  let source = null;
  let closed = false;
  const connect = () => {
    source = new EventSource(BASE + "/api/realtime");
    source.addEventListener("PB_CONNECT", async (e) => {
      const { clientId } = JSON.parse(e.data);
      try {
        await api.post("/api/realtime", { clientId, subscriptions: topics });
      } catch (_) {}
    });
    for (const topic of topics) {
      source.addEventListener(topic, (e) => {
        try {
          onEvent(topic, JSON.parse(e.data));
        } catch (_) {}
      });
    }
    source.onerror = () => {
      source.close();
      if (!closed) setTimeout(connect, 3000);
    };
  };
  connect();
  return () => {
    closed = true;
    if (source) source.close();
  };
}
